// SPDX-License-Identifier: Apache-2.0

//! The tool-policy invalidation subscription (SMA-752 § 6.3.5, D4, Q1-A).
//!
//! Each gateway replica opens a CORE NATS subscription on [`POLICY_SUBJECT`]. A JetStream publish
//! is also delivered to the core subscribers of its subject, so each replica gets each event
//! (measured against `nats:2.10.14`, D24). The client acks nothing and makes no JetStream API call:
//! the `gateway-consumer` grant is one `subscribe` on `iam.policy.>` (`ops/nats/permissions.md`
//! § 3.1).
//!
//! - Each message calls [`Invalidate::invalidate_all`] (D16). The payload is not read: an event
//!   has no tenant scope, so every event clears every tenant.
//! - Each connect and each reconnect also calls it, because messages may have been missed while
//!   the client was away. So do a new subscription after the old one ended and a slow-consumer
//!   event (the client dropped messages). Delivery is at most once; the cache refresh is the
//!   backstop.
//! - A NATS outage never blocks a request: [`connect`] returns at once and the client connects in
//!   the background (`retry_on_initial_connect`).
//!
//! Logs and metrics name the subject only. They never hold a payload, the URL or a credential.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use metrics::{counter, gauge};
use paigasus_observability::names;
use secrecy::ExposeSecret;
use tokio::sync::watch;

use crate::application::tool_policy::Invalidate;
use crate::config::ToolPolicyNatsConfig;

/// The one subject the `gateway-consumer` identity may subscribe to outside its inbox.
pub const POLICY_SUBJECT: &str = "iam.policy.>";
/// The inbox prefix of `gateway-consumer` (`ops/nats/permissions.md` § 7). Another prefix does not
/// fail at connect: every request on it would time out.
pub const INBOX_PREFIX: &str = "_INBOX_GW";
/// The client name the broker shows for this connection.
const CLIENT_NAME: &str = "paigasus-gateway-tool-policy";
/// How long `run` waits before it subscribes again after the subscription ended.
const RESUBSCRIBE_DELAY: Duration = Duration::from_secs(5);

/// Why the subscription could not start. The `Display` texts never hold the URL.
#[derive(Debug, thiserror::Error)]
pub enum NatsError {
    /// The creds file could not be read.
    #[error("tool_policy.nats.creds_file {path} could not be read")]
    Credentials {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// The creds file is not a creds file or a bare nkey seed.
    #[error("tool_policy.nats.creds_file {path} could not be parsed")]
    CredentialsParse {
        path: String,
        #[source]
        source: paigasus_nats_creds::CredsError,
    },
    /// The client could not be built. Only the error kind is kept: the source of an address error
    /// can quote the URL, which can hold credentials.
    #[error("tool_policy.nats connect failed: {0}")]
    Connect(async_nats::ConnectErrorKind),
    /// The subscription could not be sent to the client.
    #[error("tool_policy.nats subscribe to {POLICY_SUBJECT} failed")]
    Subscribe(#[source] async_nats::SubscribeError),
}

/// A connected (or connecting) client with its subscription on [`POLICY_SUBJECT`].
pub struct PolicyEvents {
    client: async_nats::Client,
    subscriber: async_nats::Subscriber,
    target: Arc<dyn Invalidate>,
}

/// Build the client and subscribe on [`POLICY_SUBJECT`].
///
/// The creds file is read and parsed once here, so a missing or bad file is a boot error that
/// names the path. Then the callback re-reads it on every connect attempt (SMA-493 D8), so a
/// rotation takes effect on the next reconnect. An optional `ca_file` REPLACES the system trust
/// store (`add_root_certificates`), as `outbox.publisher.root_ca_bundle` does in IAM. async-nats 0.53 makes `add_root_certificates`
/// imply `require_tls(true)`, so config validation refuses a `ca_file` with a plain `nats://` url.
///
/// Reconnect backoff (§ 8, the builder's choice): the async-nats 0.51 defaults. That is no limit
/// on reconnects (`max_reconnects(None)`), a delay of 0 for the first attempt and then
/// `2^(n-1)` ms capped at 4 s, and a 5 s connection timeout per attempt. A replica that cannot
/// reach NATS keeps trying, and its refresh loop keeps the cache within `refresh_secs`.
///
/// # Errors
///
/// [`NatsError`] when the creds file cannot be read or parsed, when the options are refused, or
/// when the subscribe command cannot be sent. A broker that is down is NOT an error.
pub async fn connect(cfg: &ToolPolicyNatsConfig, target: Arc<dyn Invalidate>) -> Result<PolicyEvents, NatsError> {
    let mut opts = match &cfg.creds_file {
        Some(path) => {
            let raw = tokio::fs::read_to_string(path).await.map_err(|source| NatsError::Credentials { path: path.clone(), source })?;
            paigasus_nats_creds::parse_credentials(&raw).map_err(|source| NatsError::CredentialsParse { path: path.clone(), source })?;
            let path = path.clone();
            async_nats::ConnectOptions::with_auth_callback(move |nonce| {
                let path = path.clone();
                async move { paigasus_nats_creds::auth_from_credentials(&path, &nonce).await }
            })
        }
        None => async_nats::ConnectOptions::new(),
    };
    opts = opts.name(CLIENT_NAME).custom_inbox_prefix(INBOX_PREFIX).retry_on_initial_connect().max_reconnects(None);
    if let Some(ca) = &cfg.ca_file {
        opts = opts.add_root_certificates(PathBuf::from(ca));
    }

    gauge!(names::GATEWAY_TOOL_POLICY_NATS_CONNECTED).set(0.0);
    let on_event = Arc::clone(&target);
    opts = opts.event_callback(move |event| {
        let target = Arc::clone(&on_event);
        async move { handle_event(&event, target.as_ref()) }
    });

    let client = opts.connect(cfg.url.expose_secret()).await.map_err(|e| NatsError::Connect(e.kind()))?;
    let subscriber = client.subscribe(POLICY_SUBJECT).await.map_err(NatsError::Subscribe)?;
    tracing::info!(subject = POLICY_SUBJECT, "the tool-policy NATS subscription is set up; the client connects in the background");
    Ok(PolicyEvents { client, subscriber, target })
}

/// A connection event: a connect invalidates (messages may have been missed) and sets the gauge.
fn handle_event(event: &async_nats::Event, target: &dyn Invalidate) {
    match event {
        async_nats::Event::Connected => {
            gauge!(names::GATEWAY_TOOL_POLICY_NATS_CONNECTED).set(1.0);
            target.invalidate_all();
            counter!(names::GATEWAY_TOOL_POLICY_NATS_INVALIDATIONS_TOTAL, "cause" => "connect").increment(1);
            tracing::info!("tool-policy NATS connected; the policy cache is invalidated");
        }
        async_nats::Event::Disconnected | async_nats::Event::Closed => {
            gauge!(names::GATEWAY_TOOL_POLICY_NATS_CONNECTED).set(0.0);
            tracing::warn!(event = %event, "tool-policy NATS connection event; the cache refresh is the backstop");
        }
        // The client dropped messages for this subscription. A dropped message can be a policy
        // event, so the cache is invalidated.
        async_nats::Event::SlowConsumer(_) => {
            target.invalidate_all();
            counter!(names::GATEWAY_TOOL_POLICY_NATS_INVALIDATIONS_TOTAL, "cause" => "slow_consumer").increment(1);
            tracing::warn!(event = %event, "tool-policy NATS slow consumer; the policy cache is invalidated");
        }
        // A permission violation names the subject, and nothing else.
        async_nats::Event::ServerError(e) => tracing::error!("tool-policy NATS server error: {e}"),
        async_nats::Event::ClientError(e) => tracing::error!("tool-policy NATS client error: {e}"),
        _ => tracing::info!(event = %event, "tool-policy NATS connection event"),
    }
}

impl PolicyEvents {
    /// The client, for tests (`force_reconnect`, `flush`).
    pub fn client(&self) -> &async_nats::Client {
        &self.client
    }

    /// Unsubscribe while the client stays open, for tests: [`Self::run`] then sees the
    /// subscription end and subscribes again.
    ///
    /// # Errors
    ///
    /// [`async_nats::UnsubscribeError`] when the client is closed.
    #[doc(hidden)]
    pub async fn end_subscription(&mut self) -> Result<(), async_nats::UnsubscribeError> {
        self.subscriber.unsubscribe().await
    }

    /// Invalidate on each message until `shutdown` changes (or its sender is dropped).
    ///
    /// The subscription ends only when the client removes it: an unsubscribe, or a client that
    /// closed. A permission violation from the server does not end it; it arrives as a
    /// [`async_nats::Event::ServerError`]. A broker outage does not end it either: the client
    /// subscribes again on each reconnect.
    ///
    /// When the subscription ends, the loop logs an error and subscribes again after
    /// [`RESUBSCRIBE_DELAY`]. Events may have been missed in the gap, so a new subscription
    /// invalidates (`cause="resubscribe"`). When the subscribe fails, the client is closed: the
    /// subject is a valid constant, so no other error kind is possible. A closed client never
    /// opens again, so the loop logs one error and waits for `shutdown`. The cache refresh is then
    /// the only backstop. The loop never ends the server task for this: a NATS problem must not
    /// stop the gateway (§ 6.3.5), and the supervisor stops the gateway when any task ends.
    ///
    /// # Errors
    ///
    /// None today; the `Result` is the shape of a server task in the composition root.
    pub async fn run(self, mut shutdown: watch::Receiver<()>) -> anyhow::Result<()> {
        let PolicyEvents { client, mut subscriber, target } = self;
        loop {
            tokio::select! {
                _ = shutdown.changed() => return Ok(()),
                msg = subscriber.next() => match msg {
                    Some(msg) => {
                        target.invalidate_all();
                        counter!(names::GATEWAY_TOOL_POLICY_NATS_INVALIDATIONS_TOTAL, "cause" => "message").increment(1);
                        tracing::debug!(subject = %msg.subject, "tool-policy event; the policy cache is invalidated");
                    }
                    None => {
                        tracing::error!(subject = POLICY_SUBJECT, "the tool-policy NATS subscription ended; subscribing again");
                        tokio::select! {
                            _ = shutdown.changed() => return Ok(()),
                            () = tokio::time::sleep(RESUBSCRIBE_DELAY) => {}
                        }
                        match client.subscribe(POLICY_SUBJECT).await {
                            Ok(s) => {
                                subscriber = s;
                                target.invalidate_all();
                                counter!(names::GATEWAY_TOOL_POLICY_NATS_INVALIDATIONS_TOTAL, "cause" => "resubscribe").increment(1);
                                tracing::info!(subject = POLICY_SUBJECT, "tool-policy NATS subscribed again; the policy cache is invalidated");
                            }
                            Err(e) => {
                                tracing::error!(
                                    subject = POLICY_SUBJECT,
                                    "tool-policy NATS subscribe failed ({e}); the client is closed, so only the cache refresh invalidates until shutdown"
                                );
                                let _ = shutdown.changed().await;
                                return Ok(());
                            }
                        }
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[derive(Default)]
    struct Counting(AtomicUsize);

    impl Invalidate for Counting {
        fn invalidate_all(&self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// A slow consumer drops messages, and a dropped message can be a policy event, so the event
    /// invalidates.
    #[test]
    fn a_slow_consumer_invalidates() {
        let target = Counting::default();
        handle_event(&async_nats::Event::SlowConsumer(1), &target);
        assert_eq!(target.0.load(Ordering::SeqCst), 1);
    }

    /// A disconnect does not invalidate: the connect that follows does.
    #[test]
    fn a_disconnect_does_not_invalidate() {
        let target = Counting::default();
        handle_event(&async_nats::Event::Disconnected, &target);
        assert_eq!(target.0.load(Ordering::SeqCst), 0);
    }
}
