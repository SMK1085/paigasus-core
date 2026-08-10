// SPDX-License-Identifier: Apache-2.0

//! `PgOutboxListener` (SMA-489): turns Postgres `LISTEN` notifications into relay wakeups.
//!
//! The writer half is `PgOutbox::enqueue`, which emits `pg_notify('iam_outbox_event','')` inside
//! each mutation's own transaction — Postgres holds it until commit and drops it on rollback
//! (D2). This half owns the subscription and pokes an `Arc<Notify>` that `OutboxRelay::run`
//! races against its poll sleep (D5). The two never reference each other; the `Notify` is the
//! whole interface, which is what keeps sqlx out of the relay and the drain loop out of here.
//!
//! **Never fatal (D7).** A listener that cannot connect logs, zeroes its gauge and retries
//! forever; boot never fails and the replica never leaves rotation. Delivery simply reverts to
//! the poll interval meanwhile, which is why the poll is retained (D8).
//!
//! **Why `try_recv` and not `recv` (D15).** `PgListener` defaults to `eager_reconnect: true` and
//! reconnects INTERNALLY inside `try_recv` (`sqlx-postgres-0.8.6/src/listener.rs:298-303`),
//! re-issuing `LISTEN`; `recv()` loops over that, so it almost never returns `Err`. Driving the
//! gauge off `recv()` errors would have left `iam_outbox_listener_connected` pinned at 1 and
//! `..._reconnects_total` at 0 straight through a real outage. With `eager_reconnect(false)`,
//! `try_recv() -> Ok(None)` is the explicit "reconnected, may have missed notifications" signal.
//!
//! **Keepalives are a SERVER-side setting here, and they are an operator knob (D15).** Two
//! separate things get confused in this area, so both are stated plainly.
//!
//! *Client-side keepalives do not exist in sqlx 0.8.6.* There are no keepalive setters on
//! `PgConnectOptions` — the string `keepalive` does not occur anywhere in that crate's source —
//! and configuring the socket directly would need a new dependency (`socket2`). So this process
//! cannot shorten its own detection of a dead peer; `try_recv` has no read timeout, and a
//! silently-dropped connection is noticed only when the OS default keepalive expires (~2 h on
//! Linux). What that costs is *client-side recovery speed*, and nothing else.
//!
//! *It would not have fixed the queue-fill anyway.* The D4 hazard is that Postgres's async
//! notification queue fills up, at which point every transaction calling `NOTIFY` fails AT COMMIT
//! — i.e. every IAM mutation. The queue cannot be truncated past the oldest backend still
//! listening, and that backend is on the SERVER. A client-side keepalive only makes THIS process
//! notice and reconnect; it never reaps the server's half, which is the half holding the queue.
//! Client keepalives were never the mitigation for D4.
//!
//! *The lever that does work* is the server-side GUC family `tcp_keepalives_idle` /
//! `tcp_keepalives_interval` / `tcp_keepalives_count` — all `PGC_USERSET`, so they can be set per
//! session with no code here. sqlx accepts them as URL query parameters in the `options[key]=value`
//! form (`sqlx-postgres-0.8.6/src/options/parse.rs:101-105`), so they go on
//! `[outbox].listen_database_url`:
//!
//! ```text
//! postgres://…?options[tcp_keepalives_idle]=30&options[tcp_keepalives_interval]=10&options[tcp_keepalives_count]=3
//! ```
//!
//! This stays an operator knob rather than a hardcoded connect option on purpose: a startup
//! `options` parameter is rejected outright by PgBouncer and unsupported by RDS Proxy and
//! Supavisor, so hardcoding it would turn "no nudge behind this pooler" into "the listener never
//! connects at all". See the SMA-489 runbook for when to set it.
//!
//! Absent that knob, the earliest in-process signal is the watchdog warning below correlated with
//! a flat `iam_outbox_listener_notifications_total` — which is precisely why the watchdog exists.

use std::future::Future;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use metrics::{counter, gauge};
use paigasus_observability::names;
use sea_orm::sqlx::postgres::{PgConnectOptions, PgListener, PgPoolOptions};
use tokio::sync::Notify;

/// The channel `PgOutbox::enqueue` notifies. Must match `pg_outbox::WAKE_CHANNEL`.
const WAKE_CHANNEL: &str = "iam_outbox_event";

/// `application_name` set on the listener's own connection. This is an operator aid, not a
/// behavioural knob: it makes the connection identifiable in `pg_stat_activity` (e.g. `WHERE
/// application_name = '...'`), which is exactly what's needed to spot the wedged-listener case
/// the SMA-489 runbook describes — distinguishing this single long-lived LISTEN connection from
/// every other backend the service opens.
const LISTENER_APPLICATION_NAME: &str = "paigasus-iam-outbox-listener";

const BACKOFF_START: Duration = Duration::from_millis(250);
const BACKOFF_CAP: Duration = Duration::from_secs(30);

/// Subscribes to [`WAKE_CHANNEL`] and pokes `wake` on every notification.
pub struct PgOutboxListener {
    url: String,
    wake: Arc<Notify>,
    watchdog: Duration,
}

impl PgOutboxListener {
    /// `watchdog` bounds how long the listener stays silent before warning. It NEVER forces a
    /// reconnect: silence is the normal state of a quiet deployment (no mutations means no
    /// notifications), so reconnecting on it would churn a connection every period while proving
    /// nothing. It only gives an operator a log line to correlate with
    /// `iam_outbox_listener_notifications_total` staying flat — which, absent the TCP keepalives
    /// sqlx 0.8.6 cannot set (see the module docs), is the earliest in-process hint of a
    /// half-open connection.
    #[must_use]
    pub fn new(url: String, wake: Arc<Notify>, watchdog: Duration) -> Self {
        PgOutboxListener { url, wake, watchdog }
    }

    /// Opens a PRIVATE single-connection pool. Private on purpose (D6): a slot taken from
    /// SeaORM's pool would compete with request handling and with the relay's own tick, which
    /// already holds one for `batch_size × publish-latency`. Going through a pool at all is
    /// forced by `PgListener::connect(&str)` not accepting connect options.
    async fn connect(&self) -> Result<PgListener, sea_orm::sqlx::Error> {
        let opts = PgConnectOptions::from_str(&self.url)?.application_name(LISTENER_APPLICATION_NAME);
        let pool = PgPoolOptions::new().max_connections(1).connect_with(opts).await?;
        let mut listener = PgListener::connect_with(&pool).await?;
        listener.eager_reconnect(false);
        listener.listen(WAKE_CHANNEL).await?;
        Ok(listener)
    }

    /// Runs until `shutdown` resolves. Shutdown is raced against the backoff sleep AND the
    /// connect attempt, not only against `try_recv`: with a 30 s backoff cap on top of sqlx's
    /// 30 s pool acquire timeout, a replica whose Postgres is unreachable could otherwise take
    /// ~a minute to honour SIGTERM, and SMA-471 D11 already flagged overrunning
    /// `terminationGracePeriodSeconds` as a real problem for this service.
    pub async fn run<S>(self, shutdown: S)
    where
        S: Future<Output = ()> + Send,
    {
        tokio::pin!(shutdown);
        gauge!(names::IAM_OUTBOX_LISTENER_CONNECTED).set(0.0);
        counter!(names::IAM_OUTBOX_LISTENER_NOTIFICATIONS_TOTAL).increment(0);
        counter!(names::IAM_OUTBOX_LISTENER_RECONNECTS_TOTAL).increment(0);

        let mut backoff = BACKOFF_START;
        let mut connected_before = false;

        'outer: loop {
            let listener = tokio::select! {
                biased;
                () = &mut shutdown => break 'outer,
                r = self.connect() => r,
            };

            let mut listener = match listener {
                Ok(l) => l,
                Err(e) => {
                    // NEVER log `self.url`. It is a DSN, credentials included — which is exactly
                    // why the config field it came from is a `config::RedactedUrl`; that type
                    // keeps the value out of the config's Debug/`readyz` dumps, and this call
                    // site is the other half of the same rule.
                    tracing::warn!(error = %e, "outbox listener could not connect; delivery stays poll-only until it recovers");
                    gauge!(names::IAM_OUTBOX_LISTENER_CONNECTED).set(0.0);
                    tokio::select! {
                        biased;
                        () = &mut shutdown => break 'outer,
                        () = tokio::time::sleep(backoff) => {}
                    }
                    backoff = (backoff * 2).min(BACKOFF_CAP);
                    continue;
                }
            };

            backoff = BACKOFF_START;
            gauge!(names::IAM_OUTBOX_LISTENER_CONNECTED).set(1.0);
            // The ONLY site that increments `reconnects_total`, which is what keeps it at exactly
            // one per connection loss: every loss (`Ok(None)` or `Err`) breaks to this loop, and a
            // reconnect that has not succeeded yet is not a reconnect, so a long outage counts
            // once when it finally recovers rather than once per failed attempt. `connected_before`
            // excludes the very first connect, which is not a RE-connect.
            if connected_before {
                counter!(names::IAM_OUTBOX_LISTENER_RECONNECTS_TOTAL).increment(1);
            }
            connected_before = true;
            tracing::info!(channel = WAKE_CHANNEL, "outbox listener connected");

            loop {
                // The watchdog arm cancels an in-flight `try_recv`. That is safe: sqlx documents
                // `PgStream::recv_unchecked` as cancel-safe and does not touch the read buffer
                // until a whole message has arrived (`connection/stream.rs:79-83`), and the
                // buffer lives on the connection rather than in the dropped future — so a
                // partially-read notification is simply re-read on the next `try_recv`. The arm
                // also `continue`s without touching the connection.
                let received = tokio::select! {
                    biased;
                    () = &mut shutdown => break 'outer,
                    r = listener.try_recv() => r,
                    () = tokio::time::sleep(self.watchdog) => {
                        tracing::warn!(
                            silent_for_secs = self.watchdog.as_secs(),
                            "outbox listener has received no notification for a while — normal on a quiet deployment, but if mutations ARE committing check that the connection is not fronted by a transaction-mode pooler (LISTEN is unsupported there)"
                        );
                        continue;
                    }
                };

                match received {
                    Ok(Some(_)) => {
                        counter!(names::IAM_OUTBOX_LISTENER_NOTIFICATIONS_TOTAL).increment(1);
                        // Coalescing lives here: `notify_one` stores at most ONE permit, so a
                        // burst arriving mid-tick yields exactly one extra tick, and a
                        // notification arriving with no waiter registered is not lost.
                        self.wake.notify_one();
                    }
                    // With `eager_reconnect(false)` this means "the connection dropped", NOT "it
                    // was re-established": sqlx would rebuild it only LAZILY, inside the next
                    // `try_recv`. So the connection is genuinely down at this point and the gauge
                    // must say so. Breaking to the outer loop — rather than falling through to
                    // that lazy path — keeps one loss equal to exactly one
                    // `reconnects_total` increment (the outer loop counts it on the successful
                    // re-establish); incrementing here as well would double-count every loss whose
                    // lazy reconnect then failed. It also re-issues `LISTEN` explicitly, which is
                    // the point of opting out of eager reconnect in the first place. Notifications
                    // sent during the gap are gone — Postgres does not queue for an absent
                    // listener — and the poll covers them (D8).
                    Ok(None) => {
                        tracing::warn!("outbox listener lost its connection; reconnecting, and notifications missed during the gap will be picked up by the poll");
                        gauge!(names::IAM_OUTBOX_LISTENER_CONNECTED).set(0.0);
                        break;
                    }
                    Err(e) => {
                        tracing::warn!(error = %e, "outbox listener connection failed; reconnecting");
                        gauge!(names::IAM_OUTBOX_LISTENER_CONNECTED).set(0.0);
                        break;
                    }
                }
            }
        }

        gauge!(names::IAM_OUTBOX_LISTENER_CONNECTED).set(0.0);
        tracing::info!("outbox listener stopped");
    }
}
