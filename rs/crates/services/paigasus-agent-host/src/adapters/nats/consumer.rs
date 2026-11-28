// SPDX-License-Identifier: Apache-2.0

//! The JetStream pull consumer of the inbox (spec §9.2, §9.3).
//!
//! The host never creates a stream or a consumer. `ops/nats/provision.sh` makes the durable. At
//! boot, [`bind_channels`] reads each configured inbox, and [`connect_and_check`] reads
//! `CONSUMER.INFO` of the durable and compares its filters with the channel subjects. Then
//! [`NatsSource::run`] reads the messages and acks each one by the rules of [`handle_message`].
//!
//! Logs never hold a payload, the URL or a credential.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_nats::jetstream::{self, AckKind, consumer::PullConsumer};
use futures::StreamExt;
use paigasus_agent_host_core::ids::EventId;
use paigasus_agent_host_core::inbox::{Inbox, NewInboxEvent};
use paigasus_agent_host_core::ports::{InboxRepository, InsertOutcome, RepositoryError};
use paigasus_observability::names;
use tokio::sync::watch;
use tracing::Instrument;
use uuid::Uuid;

use super::INBOX_PREFIX;
use super::channels::subject_matches;
use super::tenancy::names_org;
use crate::adapters::id::mint_uuid;
use crate::application::ingest::{Channel, ingest};
use crate::config::NatsConfig;

/// The longest nak delay: the cap of the database back-off and the delay of an unmapped subject.
const MAX_NAK_DELAY: Duration = Duration::from_secs(60);
/// How long the boot waits for `CONSUMER.INFO`, connect time included (plan choice).
const CONSUMER_INFO_BUDGET: Duration = Duration::from_secs(30);
/// The pause between two `CONSUMER.INFO` attempts while NATS does not answer.
const CONSUMER_INFO_RETRY: Duration = Duration::from_secs(1);
/// The pause before `run` opens the message stream again after it ended or failed to open.
const REOPEN_DELAY: Duration = Duration::from_secs(1);
/// The client name the broker shows for this connection.
const CLIENT_NAME: &str = "paigasus-agent-host-inbox";

/// One configured channel, with its inbox read from the database at boot.
#[derive(Debug, Clone)]
pub struct BoundChannel {
    /// The subject filter of the channel. It is one filter subject of the durable.
    pub filter: String,
    pub inbox: Inbox,
    /// The CloudEvents `source` that the host sets on each event of this channel.
    pub source: String,
}

/// Why the NATS source cannot start. Each one stops the boot (spec §9.1). The texts never hold the
/// URL or a credential.
#[derive(Debug, thiserror::Error)]
pub enum NatsBootError {
    /// A channel names an inbox that has no row. Create the inbox first, then add the channel.
    #[error("nats.channels: the inbox {0} does not exist")]
    UnknownInbox(Uuid),
    /// The inbox lookup failed.
    #[error("nats.channels: the inbox lookup failed")]
    Store(#[source] RepositoryError),
    /// The creds file could not be read.
    #[error("nats.creds_file {path} could not be read")]
    Credentials {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// The creds file is not a creds file or a bare nkey seed.
    #[error("nats.creds_file {path} could not be parsed")]
    CredentialsParse {
        path: String,
        #[source]
        source: paigasus_nats_creds::CredsError,
    },
    /// The client could not be built. Only the kind is kept: an address error can quote the URL.
    #[error("nats connect failed: {0}")]
    Connect(async_nats::ConnectErrorKind),
    /// The server answered `CONSUMER.INFO` with an error, for example "consumer not found".
    #[error("nats: CONSUMER.INFO of {stream}.{durable} failed: {reason}")]
    ConsumerInfo { stream: String, durable: String, reason: String },
    /// No answer to `CONSUMER.INFO` in the boot budget. A permission violation looks the same.
    #[error("nats: no answer to CONSUMER.INFO of {stream}.{durable} within {budget:?}")]
    ConsumerInfoTimeout { stream: String, durable: String, budget: Duration },
    /// The filters of the durable are not the configured channel subjects.
    #[error("nats: the durable filters {server:?} are not the configured channel subjects {configured:?}")]
    FilterMismatch { configured: Vec<String>, server: Vec<String> },
    /// The durable is not a pull consumer, or it has no explicit ack, or it limits the deliveries.
    #[error("nats: the durable {durable} must be a pull consumer with explicit ack and max_deliver -1: {problem}")]
    DeliverySettings { durable: String, problem: String },
}

/// Read the inbox of each configured channel. An inbox with no row stops the boot.
///
/// # Errors
///
/// [`NatsBootError::UnknownInbox`] or [`NatsBootError::Store`].
pub async fn bind_channels(cfg: &NatsConfig, inboxes: &dyn InboxRepository) -> Result<Vec<BoundChannel>, NatsBootError> {
    let mut bound = Vec::with_capacity(cfg.channels.len());
    for channel in &cfg.channels {
        let inbox = inboxes
            .get_by_uuid(channel.inbox)
            .await
            .map_err(NatsBootError::Store)?
            .ok_or(NatsBootError::UnknownInbox(channel.inbox))?;
        bound.push(BoundChannel {
            filter: channel.subject.clone(),
            inbox,
            source: channel.source.clone(),
        });
    }
    Ok(bound)
}

/// Compare the configured subjects with the filters of the durable, as sets. The server reports
/// one filter in `filter_subject` or in `filter_subjects`, so both fields are read as one set
/// (Review Focus 3). An empty server set is a mismatch: that durable reads the whole stream.
///
/// # Errors
///
/// [`NatsBootError::FilterMismatch`] when the two sets differ.
pub fn check_filters(configured: &[String], filter_subject: &str, filter_subjects: &[String]) -> Result<(), NatsBootError> {
    let want: BTreeSet<&str> = configured.iter().map(String::as_str).collect();
    let have: BTreeSet<&str> = filter_subjects.iter().map(String::as_str).chain(Some(filter_subject)).filter(|s| !s.is_empty()).collect();
    if want == have {
        return Ok(());
    }
    Err(NatsBootError::FilterMismatch {
        configured: want.into_iter().map(str::to_owned).collect(),
        server: have.into_iter().map(str::to_owned).collect(),
    })
}

/// The delivery settings the host relies on (AG2): a pull consumer, explicit ack, and no limit on
/// the deliveries. A limit would drop an event after the last delivery, with no row.
///
/// # Errors
///
/// [`NatsBootError::DeliverySettings`] that names the first wrong setting.
pub fn check_delivery(durable: &str, config: &jetstream::consumer::Config) -> Result<(), NatsBootError> {
    let problem = if config.deliver_subject.is_some() {
        Some(format!("deliver_subject is {:?}, so it is a push consumer", config.deliver_subject))
    } else if config.ack_policy != jetstream::consumer::AckPolicy::Explicit {
        Some(format!("ack_policy is {:?}", config.ack_policy))
    } else if config.max_deliver != -1 {
        Some(format!("max_deliver is {}", config.max_deliver))
    } else {
        None
    };
    match problem {
        None => Ok(()),
        Some(problem) => Err(NatsBootError::DeliverySettings { durable: durable.to_owned(), problem }),
    }
}

/// What the consumer does with a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    Ack,
    /// Nak, and ask for a redelivery after this delay.
    Nak(Duration),
    /// Never deliver again.
    Term,
}

/// The nak delay after a database error: 1 s for the first delivery, then doubling, at most 60 s.
fn back_off(delivered: u64) -> Duration {
    let exponent = delivered.saturating_sub(1).min(6);
    Duration::from_secs(1u64 << exponent).min(MAX_NAK_DELAY)
}

/// The ack rules of spec §9.3 for one message. `delivered` is the delivery count of the message
/// (1 for the first delivery). The caller puts the stream sequence in the log span.
pub async fn handle_message(channels: &[BoundChannel], inboxes: &dyn InboxRepository, subject: &str, payload: &[u8], delivered: u64) -> Disposition {
    // Config validation refuses two channels that can match one subject, so at most one matches.
    let Some(channel) = channels.iter().find(|c| subject_matches(&c.filter, subject)) else {
        metrics::counter!(names::AGENT_HOST_NATS_MESSAGES_UNMAPPED_TOTAL).increment(1);
        tracing::error!(subject, "no channel maps this NATS subject; the durable filters and the channels differ");
        return Disposition::Nak(MAX_NAK_DELAY);
    };
    let mut event = match paigasus_cloudevents::parse(payload) {
        Ok(event) => event,
        Err(e) => {
            metrics::counter!(names::AGENT_HOST_NATS_MESSAGES_TERMINATED_TOTAL).increment(1);
            tracing::warn!(subject, error = %e, "a NATS message body is not a valid CloudEvent; it is terminated");
            return Disposition::Term;
        }
    };
    // The host sets the source from the channel (AG3). The actor and a caller thread are dropped.
    if event.set_source(&channel.source).is_err() {
        // Only a channel source over the length limit fails here, and it fails for every message.
        tracing::error!(subject, "the channel source is not a valid CloudEvents source");
        return Disposition::Nak(MAX_NAK_DELAY);
    }
    event.remove_extension("actorprn");
    event.remove_extension("threadid");
    // The tenancy rule (spec §9.4, AG6): the event must name the inbox org.
    if !names_org(event.subject(), channel.inbox.org()) {
        metrics::counter!(names::AGENT_HOST_NATS_MESSAGES_TENANCY_SKIPPED_TOTAL).increment(1);
        tracing::debug!(subject, inbox = %channel.inbox.id().uuid(), ce_id = event.id(), "the event does not name the inbox org; it is skipped");
        return Disposition::Ack;
    }
    let new_event = NewInboxEvent {
        id: EventId::from_uuid(mint_uuid()),
        inbox: channel.inbox.id().clone(),
        source: event.source().to_owned(),
        ce_id: event.id().to_owned(),
        ce_type: event.ty().to_owned(),
        thread: None,
        envelope: event.to_json(),
    };
    match ingest(inboxes, new_event, Channel::Nats).await {
        Ok(InsertOutcome::AgentArchived) => {
            tracing::warn!(subject, inbox = %channel.inbox.id().uuid(), ce_id = event.id(), "the agent of the inbox is archived; the event is terminated");
            Disposition::Term
        }
        Ok(_) => Disposition::Ack,
        Err(e) => {
            let delay = back_off(delivered);
            tracing::warn!(subject, error = %e, delivered, ?delay, "the NATS event insert failed; it is nacked");
            Disposition::Nak(delay)
        }
    }
}

/// A connected client with the checked durable.
pub struct NatsSource {
    consumer: PullConsumer,
    channels: Vec<BoundChannel>,
}

/// Connect, read `CONSUMER.INFO` of the durable (at most 30 s, connect time included) and compare
/// its filters with the channel subjects.
///
/// The creds file is read and parsed once here, so a bad file is a boot error that names the path.
/// The auth callback then re-reads it on every connect attempt (SMA-493 D8). The client connects in
/// the background and reconnects without limit. An optional `ca_file` replaces the system trust
/// store.
///
/// # Errors
///
/// [`NatsBootError`] when the creds file is bad, the options are refused, `CONSUMER.INFO` fails or
/// does not answer in time, or the filters differ.
pub async fn connect_and_check(cfg: &NatsConfig, channels: &[BoundChannel]) -> Result<NatsSource, NatsBootError> {
    let mut opts = match &cfg.creds_file {
        Some(path) => {
            let raw = tokio::fs::read_to_string(path).await.map_err(|source| NatsBootError::Credentials { path: path.clone(), source })?;
            paigasus_nats_creds::parse_credentials(&raw).map_err(|source| NatsBootError::CredentialsParse { path: path.clone(), source })?;
            let path = path.clone();
            async_nats::ConnectOptions::with_auth_callback(move |nonce| {
                let path = path.clone();
                async move { paigasus_nats_creds::auth_from_credentials(&path, &nonce).await }
            })
        }
        None => async_nats::ConnectOptions::new(),
    };
    // The identity may subscribe only on its own inbox prefix. Another prefix times out every request.
    opts = opts.name(CLIENT_NAME).custom_inbox_prefix(INBOX_PREFIX).retry_on_initial_connect().max_reconnects(None);
    if let Some(ca) = &cfg.ca_file {
        opts = opts.add_root_certificates(PathBuf::from(ca));
    }
    let client = opts.connect(cfg.url.as_str()).await.map_err(|e| NatsBootError::Connect(e.kind()))?;
    let js = jetstream::new(client);

    let consumer = tokio::time::timeout(CONSUMER_INFO_BUDGET, consumer_info(&js, &cfg.stream, &cfg.durable))
        .await
        .map_err(|_| NatsBootError::ConsumerInfoTimeout {
            stream: cfg.stream.clone(),
            durable: cfg.durable.clone(),
            budget: CONSUMER_INFO_BUDGET,
        })??;
    let info = consumer.cached_info();
    let configured: Vec<String> = channels.iter().map(|c| c.filter.clone()).collect();
    check_filters(&configured, &info.config.filter_subject, &info.config.filter_subjects)?;
    check_delivery(&cfg.durable, &info.config)?;
    tracing::info!(stream = %cfg.stream, durable = %cfg.durable, channels = channels.len(), "the NATS durable is checked");
    Ok(NatsSource {
        consumer,
        channels: channels.to_vec(),
    })
}

/// `CONSUMER.INFO` only: the identity has no `STREAM.INFO`. A transport error (no answer yet) is
/// tried again. An error answer of the server is final.
async fn consumer_info(js: &jetstream::Context, stream: &str, durable: &str) -> Result<PullConsumer, NatsBootError> {
    use async_nats::jetstream::stream::ConsumerErrorKind;
    loop {
        match js.get_consumer_from_stream::<jetstream::consumer::pull::Config, _, _>(durable, stream).await {
            Ok(consumer) => return Ok(consumer),
            Err(e) if matches!(e.kind(), ConsumerErrorKind::TimedOut | ConsumerErrorKind::Request) => {
                tracing::debug!(error = %e, "CONSUMER.INFO got no answer yet; trying again");
                tokio::time::sleep(CONSUMER_INFO_RETRY).await;
            }
            Err(e) => {
                return Err(NatsBootError::ConsumerInfo {
                    stream: stream.to_owned(),
                    durable: durable.to_owned(),
                    reason: e.to_string(),
                });
            }
        }
    }
}

impl NatsSource {
    /// Read the messages until the shutdown watch fires. Each message is handled in full before
    /// the next one, so a shutdown does not cut a message in half. A message that is not acked
    /// (an ack lost on the wire, a stop) is delivered again after the ack wait, and the dedup
    /// keeps one row (AG2).
    ///
    /// # Errors
    ///
    /// Only when the durable is deleted: the host never creates it again.
    pub async fn run(self, inboxes: Arc<dyn InboxRepository>, mut shutdown: watch::Receiver<()>) -> anyhow::Result<()> {
        use async_nats::jetstream::consumer::pull::MessagesErrorKind;
        loop {
            let opened = tokio::select! {
                r = self.consumer.messages() => r,
                _ = shutdown.changed() => return Ok(()),
            };
            match opened {
                Ok(mut messages) => loop {
                    let next = tokio::select! {
                        next = messages.next() => next,
                        _ = shutdown.changed() => return Ok(()),
                    };
                    match next {
                        Some(Ok(msg)) => self.process(&*inboxes, msg).await,
                        Some(Err(e)) if matches!(e.kind(), MessagesErrorKind::ConsumerDeleted) => {
                            anyhow::bail!("the NATS durable was deleted; provision it again with ops/nats/provision.sh");
                        }
                        Some(Err(e)) => tracing::warn!(error = %e, "the NATS message stream reported an error"),
                        None => {
                            tracing::warn!("the NATS message stream ended; opening it again");
                            break;
                        }
                    }
                },
                Err(e) => tracing::warn!(error = %e, "the NATS message stream did not open; trying again"),
            }
            tokio::select! {
                () = tokio::time::sleep(REOPEN_DELAY) => {}
                _ = shutdown.changed() => return Ok(()),
            }
        }
    }

    /// Handle one message and send its ack.
    async fn process(&self, inboxes: &dyn InboxRepository, msg: jetstream::Message) {
        let (stream_sequence, delivered) = match msg.info() {
            Ok(info) => (info.stream_sequence, u64::try_from(info.delivered).unwrap_or(1)),
            // Not a JetStream message. It cannot be acked.
            Err(e) => {
                tracing::warn!(error = %e, "a NATS message has no JetStream metadata");
                return;
            }
        };
        let span = tracing::info_span!("nats_message", stream_sequence, delivered);
        let disposition = handle_message(&self.channels, inboxes, msg.subject.as_str(), &msg.payload, delivered).instrument(span.clone()).await;
        let sent = match disposition {
            Disposition::Ack => msg.ack().await,
            Disposition::Nak(delay) => msg.ack_with(AckKind::Nak(Some(delay))).await,
            Disposition::Term => msg.ack_with(AckKind::Term).await,
        };
        if let Err(e) = sent {
            // The ack wait then redelivers the message, and the dedup keeps one row.
            span.in_scope(|| tracing::warn!(error = %e, ?disposition, "the NATS ack was not sent"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use jiff::Timestamp;
    use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
    use paigasus_agent_host_core::ids::{AgentId, InboxId, PrincipalId};

    /// IAM's encoding of `iam.organization.created` for the org `u(0x500)` (the Task 1 fixture).
    const FIX_ORG: &[u8] = include_bytes!("../../../../../libs/paigasus-cloudevents/tests/fixtures/iam/organization_created_trailing_zeros.json");
    const SOURCE: &str = "urn:paigasus:agents:nats:iam-org";

    fn u(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    /// FIX_ORG with an `actorprn` and a `threadid` extension added (R7).
    fn fix_org_with_actor_and_thread() -> Vec<u8> {
        let mut v: serde_json::Value = serde_json::from_slice(FIX_ORG).expect("fixture json");
        let o = v.as_object_mut().expect("object");
        o.insert("actorprn".into(), serde_json::json!(format!("prn:pgs:iam:::principal/{}", u(0x77))));
        o.insert("threadid".into(), serde_json::json!(format!("prn:pgs:agents::{}:thread/{}", u(0x500), u(0x78))));
        serde_json::to_vec(&v).expect("bytes")
    }

    /// An in-memory repository: a dedup on `(inbox, source, ce_id)`, or a store error.
    struct Memory {
        fail: bool,
        archived: bool,
        rows: Mutex<Vec<NewInboxEvent>>,
    }

    impl Memory {
        fn new() -> Self {
            Memory {
                fail: false,
                archived: false,
                rows: Mutex::new(Vec::new()),
            }
        }
        fn failing() -> Self {
            Memory {
                fail: true,
                archived: false,
                rows: Mutex::new(Vec::new()),
            }
        }
        /// A repository whose inbox has no active agent.
        fn archived() -> Self {
            Memory {
                fail: false,
                archived: true,
                rows: Mutex::new(Vec::new()),
            }
        }
        fn rows(&self) -> Vec<NewInboxEvent> {
            self.rows.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl InboxRepository for Memory {
        async fn get_by_uuid(&self, _id: Uuid) -> Result<Option<Inbox>, RepositoryError> {
            Ok(None)
        }
        async fn insert_event(&self, event: NewInboxEvent) -> Result<InsertOutcome, RepositoryError> {
            if self.fail {
                return Err(RepositoryError::Store("down".into()));
            }
            if self.archived {
                return Ok(InsertOutcome::AgentArchived);
            }
            let mut rows = self.rows.lock().unwrap();
            if let Some(row) = rows.iter().find(|r| r.inbox == event.inbox && r.source == event.source && r.ce_id == event.ce_id) {
                return Ok(InsertOutcome::Duplicate { id: row.id, payload_matches: true });
            }
            let id = event.id;
            rows.push(event);
            Ok(InsertOutcome::Inserted { id })
        }
    }

    fn channel(org: u128) -> BoundChannel {
        let inbox = Inbox::new(InboxId::new(u(org), u(0x10)), AgentId::new(u(org), u(0x11)), PrincipalId::from_uuid(u(0x12)), Timestamp::UNIX_EPOCH).expect("inbox");
        BoundChannel {
            filter: "iam.organization.>".into(),
            inbox,
            source: SOURCE.into(),
        }
    }

    /// The unlabeled counters of one snapshot. A snapshot resets the counters, so take one.
    fn counters(snapshotter: &Snapshotter) -> HashMap<String, u64> {
        snapshotter
            .snapshot()
            .into_vec()
            .into_iter()
            .filter_map(|(key, _, _, v)| {
                let DebugValue::Counter(n) = v else { return None };
                Some((key.key().name().to_string(), n))
            })
            .collect()
    }

    fn value(snap: &HashMap<String, u64>, name: &str) -> u64 {
        snap.get(name).copied().unwrap_or(0)
    }

    #[tokio::test]
    async fn an_org_event_for_the_inbox_org_is_inserted_and_acked() {
        let repo = Memory::new();
        let channels = [channel(0x500)];
        let d = handle_message(&channels, &repo, "iam.organization.created", &fix_org_with_actor_and_thread(), 1).await;
        assert_eq!(d, Disposition::Ack);
        let rows = repo.rows();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.source, SOURCE);
        assert_eq!(row.envelope["source"], SOURCE);
        assert_eq!(row.inbox, *channels[0].inbox.id());
        assert_eq!(row.thread, None);
        assert_eq!(row.ce_id, "00000000-0000-0000-0000-000000000002");
        assert_eq!(row.ce_type, "iam.organization.created");
        let env = row.envelope.as_object().expect("object");
        assert!(!env.contains_key("actorprn"), "actorprn is dropped: {env:?}");
        assert!(!env.contains_key("threadid"), "threadid is dropped: {env:?}");
        // id, type, time, subject and data are kept.
        assert_eq!(env["id"], "00000000-0000-0000-0000-000000000002");
        assert_eq!(env["type"], "iam.organization.created");
        assert_eq!(env["time"], "2026-08-07T12:30:45.100000Z");
        assert_eq!(env["subject"], format!("prn:pgs:iam:::organization/{}", u(0x500)));
        assert_eq!(env["data"], serde_json::json!({"slug": "acme"}));
    }

    /// An event for an inbox without an active agent is terminated, not acked: it is never stored
    /// and it is not redelivered.
    #[tokio::test]
    async fn an_event_for_an_archived_agent_is_terminated() {
        let repo = Memory::archived();
        let d = handle_message(&[channel(0x500)], &repo, "iam.organization.created", FIX_ORG, 1).await;
        assert_eq!(d, Disposition::Term);
        assert!(repo.rows().is_empty());
    }

    #[tokio::test]
    async fn a_duplicate_is_acked() {
        let repo = Memory::new();
        let channels = [channel(0x500)];
        assert_eq!(handle_message(&channels, &repo, "iam.organization.created", FIX_ORG, 1).await, Disposition::Ack);
        assert_eq!(handle_message(&channels, &repo, "iam.organization.created", FIX_ORG, 1).await, Disposition::Ack);
        assert_eq!(repo.rows().len(), 1);
    }

    #[tokio::test]
    async fn an_event_of_another_org_is_acked_and_skipped() {
        let recorder = DebuggingRecorder::new();
        let snap = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let repo = Memory::new();
        let d = handle_message(&[channel(0x600)], &repo, "iam.organization.created", FIX_ORG, 1).await;
        assert_eq!(d, Disposition::Ack);
        assert!(repo.rows().is_empty());
        assert_eq!(value(&counters(&snap), names::AGENT_HOST_NATS_MESSAGES_TENANCY_SKIPPED_TOTAL), 1);
    }

    #[tokio::test]
    async fn a_bad_body_is_terminated() {
        let recorder = DebuggingRecorder::new();
        let snap = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let repo = Memory::new();
        let d = handle_message(&[channel(0x500)], &repo, "iam.organization.created", b"not json", 1).await;
        assert_eq!(d, Disposition::Term);
        assert!(repo.rows().is_empty());
        assert_eq!(value(&counters(&snap), names::AGENT_HOST_NATS_MESSAGES_TERMINATED_TOTAL), 1);
    }

    /// A NUL in `data` makes the body a malformed event: terminated, not nacked, so it is never
    /// redelivered.
    #[tokio::test]
    async fn a_nul_in_data_is_terminated() {
        let recorder = DebuggingRecorder::new();
        let snap = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let repo = Memory::new();
        let body = br#"{"specversion":"1.0","id":"e1","source":"s","type":"iam.organization.created","data":{"k":{"v":"a\u0000b"}}}"#;
        let d = handle_message(&[channel(0x500)], &repo, "iam.organization.created", body, 1).await;
        assert_eq!(d, Disposition::Term);
        assert!(repo.rows().is_empty());
        assert_eq!(value(&counters(&snap), names::AGENT_HOST_NATS_MESSAGES_TERMINATED_TOTAL), 1);
    }

    #[tokio::test]
    async fn an_unmapped_subject_is_nacked_with_the_maximum_delay() {
        let recorder = DebuggingRecorder::new();
        let snap = recorder.snapshotter();
        let _guard = metrics::set_default_local_recorder(&recorder);
        let repo = Memory::new();
        let d = handle_message(&[channel(0x500)], &repo, "iam.role.granted", FIX_ORG, 1).await;
        assert_eq!(d, Disposition::Nak(Duration::from_secs(60)));
        assert!(repo.rows().is_empty());
        assert_eq!(value(&counters(&snap), names::AGENT_HOST_NATS_MESSAGES_UNMAPPED_TOTAL), 1);
    }

    #[tokio::test]
    async fn a_store_error_backs_off_and_doubles() {
        let repo = Memory::failing();
        let channels = [channel(0x500)];
        for (delivered, secs) in [(1, 1), (2, 2), (3, 4), (7, 60), (8, 60), (u64::MAX, 60)] {
            let d = handle_message(&channels, &repo, "iam.organization.created", FIX_ORG, delivered).await;
            assert_eq!(d, Disposition::Nak(Duration::from_secs(secs)), "delivered {delivered}");
        }
    }

    #[test]
    fn check_filters_reads_both_server_fields() {
        let one = ["iam.organization.>".to_string()];
        // Review Focus 3: one filter, reported in `filter_subject` only.
        assert!(check_filters(&one, "iam.organization.>", &[]).is_ok());
        // The same filter in `filter_subjects` only.
        assert!(check_filters(&one, "", &["iam.organization.>".to_string()]).is_ok());
        // Another filter.
        assert!(matches!(check_filters(&one, "", &["iam.role.>".to_string()]), Err(NatsBootError::FilterMismatch { .. })));
        assert!(matches!(check_filters(&one, "iam.role.>", &[]), Err(NatsBootError::FilterMismatch { .. })));
        // Two configured, one on the server.
        let two = ["iam.organization.>".to_string(), "iam.team.>".to_string()];
        assert!(matches!(check_filters(&two, "iam.organization.>", &[]), Err(NatsBootError::FilterMismatch { .. })));
        // Two on both sides, in another order.
        assert!(check_filters(&two, "", &["iam.team.>".to_string(), "iam.organization.>".to_string()]).is_ok());
        // No filter on the server: the durable reads the whole stream.
        assert!(matches!(check_filters(&one, "", &[]), Err(NatsBootError::FilterMismatch { .. })));
    }
}
