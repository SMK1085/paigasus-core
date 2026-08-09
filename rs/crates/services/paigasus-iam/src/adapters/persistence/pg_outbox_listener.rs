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
//! **No TCP keepalives are available — a deviation from D15, and the one caveat worth carrying
//! forward.** sqlx-postgres 0.8.6 exposes NO keepalive setters on `PgConnectOptions`; the string
//! `keepalive` does not occur anywhere in that crate's source. They cannot be smuggled in through
//! `database_url` either — unrecognised URL parameters are logged and discarded
//! (`sqlx-postgres-0.8.6/src/options/parse.rs:107`). Setting socket-level keepalives would need a
//! new dependency (`socket2`), which this change is not permitted to add.
//!
//! The consequence is real. `try_recv` has no read timeout, so a silently-dropped connection can
//! leave Postgres believing this session is alive and LISTENing — the half-open case that fills
//! the async notification queue. A full queue makes every transaction calling `NOTIFY` fail AT
//! COMMIT, i.e. every IAM mutation (D4). Nothing in this process shortens that window: recovery
//! waits on the OS default keepalive, which on Linux is ~2 h. The only in-process signal is the
//! watchdog warning below, correlated with a flat
//! `iam_outbox_listener_notifications_total` — which is precisely why the watchdog exists.

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
        let opts = PgConnectOptions::from_str(&self.url)?;
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
                    // NEVER log `self.url` — `IamConfig.database_url` is not redacted in the
                    // config's derived Debug/Serialize, unlike `PublisherConfig::url`.
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
                    // `eager_reconnect(false)` makes this "the connection dropped and was
                    // re-established; notifications may have been missed". The poll covers the
                    // gap (D8) — Postgres does not queue for an absent listener.
                    Ok(None) => {
                        counter!(names::IAM_OUTBOX_LISTENER_RECONNECTS_TOTAL).increment(1);
                        tracing::warn!("outbox listener reconnected; notifications during the gap were dropped and will be picked up by the poll");
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
