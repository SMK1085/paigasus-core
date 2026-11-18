// SPDX-License-Identifier: Apache-2.0

//! The JetStream side of the decision log (SMA-755 § 5.7, § 5.10). It holds the stream constants, the subject
//! of an event, the NATS connection of the relay and the classification of a publish error (P-14, P-20).
//!
//! The stream values live once in `ops/nats/subjects.env`. A unit test below pins these constants to that file.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_nats::jetstream::{self, context::PublishErrorKind};
use secrecy::ExposeSecret;
use uuid::Uuid;

use crate::config::DecisionLogNatsConfig;

/// The name of the stream.
pub const STREAM: &str = "GATEWAY_DECISIONS";
/// The subject prefix. The stream takes `gateway.decision.>`.
pub const SUBJECT_PREFIX: &str = "gateway.decision";
/// The subject token of an event that has no org.
pub const NO_ORG_TOKEN: &str = "_none";
/// The inbox prefix of `gateway-decision-publisher`. Another prefix does not fail at connect: each PubAck
/// would never arrive and each publish would time out.
pub const INBOX_PREFIX: &str = "_INBOX_GW_DEC";
/// The client name that the broker shows for this connection.
const CLIENT_NAME: &str = "paigasus-gateway-decision-relay";
/// The lower-case start of the text that the broker sends when a publish is not allowed. The broker text is
/// `Permissions Violation for Publish to "<subject>"`; the prefix also covers the older `... for Publication`.
const PERMISSION_VIOLATION: &str = "permissions violation for publ";

/// Whether a server error text is a refused publish. The match is case-insensitive.
#[must_use]
pub fn is_publish_violation(text: &str) -> bool {
    text.to_lowercase().contains(PERMISSION_VIOLATION)
}

/// Whether the broker reported a refused publish since the generation `before` was read.
#[must_use]
pub fn violation_since(generation: &AtomicU64, before: u64) -> bool {
    generation.load(Ordering::SeqCst) != before
}

/// 30 days (P-20).
pub const MAX_AGE_SECS: u64 = 2_592_000;
/// 90 % of the default 20 GiB account disk (P-20).
pub const MAX_BYTES: i64 = 19_327_352_832;
/// The duplicate window, 2 minutes (P-20).
pub const DUPLICATE_WINDOW_SECS: u64 = 120;
/// The largest message, 64 KiB (P-20).
pub const MAX_MESSAGE_SIZE: i32 = 65_536;

/// Why the NATS client could not be made. The `Display` texts never hold the URL.
#[derive(Debug, thiserror::Error)]
pub enum NatsError {
    /// The creds file could not be read.
    #[error("decision_log.nats.creds_file {path} could not be read")]
    Credentials {
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// The creds file is not a creds file or a bare nkey seed.
    #[error("decision_log.nats.creds_file {path} could not be parsed")]
    CredentialsParse {
        path: String,
        #[source]
        source: paigasus_nats_creds::CredsError,
    },
    /// The client could not be built. Only the error kind is kept: the source of an address error can quote
    /// the URL, which can hold credentials.
    #[error("decision_log.nats connect failed: {0}")]
    Connect(async_nats::ConnectErrorKind),
}

/// The stream config. `provision-decisions.sh` makes the same stream from `ops/nats/subjects.env`. The relay
/// calls `get_or_create_stream` with this value, so a stream that the relay makes is never unbounded.
#[must_use]
pub fn stream_config() -> jetstream::stream::Config {
    jetstream::stream::Config {
        name: STREAM.to_owned(),
        subjects: vec![format!("{SUBJECT_PREFIX}.>")],
        retention: jetstream::stream::RetentionPolicy::Limits,
        storage: jetstream::stream::StorageType::File,
        max_age: Duration::from_secs(MAX_AGE_SECS),
        max_bytes: MAX_BYTES,
        discard: jetstream::stream::DiscardPolicy::Old,
        duplicate_window: Duration::from_secs(DUPLICATE_WINDOW_SECS),
        max_message_size: MAX_MESSAGE_SIZE,
        num_replicas: 1,
        ..Default::default()
    }
}

/// The subject of an event. An empty `org_id` gives `gateway.decision._none`. A canonical lower-case
/// hyphenated UUID gives `gateway.decision.<uuid>`. Anything else gives `None`: the value could carry a
/// wildcard or a dot, and a forged org must not reach a subject.
#[must_use]
pub fn subject_for(org_id: &str) -> Option<String> {
    if org_id.is_empty() {
        return Some(format!("{SUBJECT_PREFIX}.{NO_ORG_TOKEN}"));
    }
    let parsed = Uuid::parse_str(org_id).ok()?;
    (parsed.hyphenated().to_string() == org_id).then(|| format!("{SUBJECT_PREFIX}.{org_id}"))
}

/// The class of a publish error (P-14).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublishClass {
    /// Keep the entry in Redis and try again later.
    Transient,
    /// Ack and delete the entry, and count it as `publish_rejected`. The text is the class for the log.
    Permanent(&'static str),
}

/// Classify a publish error. `async-nats` reports a refused publish as `TimedOut`, so `violation_seen` (set
/// by the connection event callback) turns a `TimedOut` into a permanent error.
#[must_use]
pub fn classify(kind: PublishErrorKind, violation_seen: bool) -> PublishClass {
    match kind {
        PublishErrorKind::MaxPayloadExceeded => PublishClass::Permanent("max_payload_exceeded"),
        PublishErrorKind::StreamNotFound => PublishClass::Permanent("stream_not_found"),
        PublishErrorKind::TimedOut if violation_seen => PublishClass::Permanent("permission_violation"),
        PublishErrorKind::TimedOut | PublishErrorKind::BrokenPipe | PublishErrorKind::MaxAckPending => PublishClass::Transient,
        // `Other` covers a refused subject and a PubAck error, for example a message above the stream limit.
        // The relay never sets an expected last id or sequence, so those two kinds are not expected either.
        PublishErrorKind::Other | PublishErrorKind::WrongLastMessageId | PublishErrorKind::WrongLastSequence => PublishClass::Permanent("other"),
    }
}

/// Make the JetStream context. The creds file is read and parsed once here, so a missing or bad file is a boot
/// error that names the path. Then the callback re-reads it on every connect attempt, so a rotation takes
/// effect on the next reconnect. A broker that is down is not an error: the client connects in the background.
/// `violations` is a generation counter: the event callback adds 1 each time that the broker reports a refused
/// publish. A publish reads it before it starts and calls a `TimedOut` permanent only if it changed (P-14).
///
/// # Errors
///
/// [`NatsError`] when the creds file cannot be read or parsed, or when the options are refused.
pub async fn connect(cfg: &DecisionLogNatsConfig, violations: Arc<AtomicU64>) -> Result<jetstream::Context, NatsError> {
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
    opts = opts.event_callback(move |event| {
        let violations = Arc::clone(&violations);
        async move {
            match &event {
                async_nats::Event::ServerError(err) if is_publish_violation(&err.to_string()) => {
                    violations.fetch_add(1, Ordering::SeqCst);
                    tracing::error!("the decision-log NATS publisher has no permission to publish");
                }
                async_nats::Event::Connected => tracing::info!("the decision-log NATS client is connected"),
                async_nats::Event::Disconnected | async_nats::Event::Closed => tracing::warn!(event = %event, "the decision-log NATS connection event"),
                _ => {}
            }
        }
    });
    let client = opts.connect(cfg.url.expose_secret()).await.map_err(|e| NatsError::Connect(e.kind()))?;
    Ok(jetstream::new(client))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subject_for_maps_the_three_cases() {
        assert_eq!(subject_for("").as_deref(), Some("gateway.decision._none"));
        let org = "0190a100-0000-7000-8000-0000000000a1";
        assert_eq!(subject_for(org), Some(format!("gateway.decision.{org}")));
        for bad in [
            "0190A100-0000-7000-8000-0000000000A1",
            "{0190a100-0000-7000-8000-0000000000a1}",
            "0190a10000007000800000000000a1a1",
            "a.b",
            "*",
            ">",
            "gateway.decision.x",
            " ",
            "urn:uuid:0190a100-0000-7000-8000-0000000000a1",
        ] {
            assert_eq!(subject_for(bad), None, "{bad:?} must be refused");
        }
    }

    #[test]
    fn the_violation_matcher_covers_both_broker_spellings() {
        assert!(is_publish_violation("nats: Permissions Violation for Publish to \"gateway.decision.x\""));
        assert!(is_publish_violation("nats: Permissions Violation for Publication to \"x\""));
        assert!(is_publish_violation("PERMISSIONS VIOLATION FOR PUBLISH TO \"x\""));
        assert!(!is_publish_violation("nats: Permissions Violation for Subscription to \"x\""));
        assert!(!is_publish_violation("nats: authorization violation"));
    }

    #[test]
    fn a_timeout_is_permanent_only_when_a_violation_came_during_that_publish() {
        let generation = AtomicU64::new(0);
        // A violation that came before this publish started does not count.
        generation.fetch_add(1, Ordering::SeqCst);
        let before = generation.load(Ordering::SeqCst);
        assert_eq!(classify(PublishErrorKind::TimedOut, violation_since(&generation, before)), PublishClass::Transient);
        // A violation during the publish does.
        generation.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            classify(PublishErrorKind::TimedOut, violation_since(&generation, before)),
            PublishClass::Permanent("permission_violation")
        );
        // The next publish reads the new generation and is transient again.
        let before = generation.load(Ordering::SeqCst);
        assert_eq!(classify(PublishErrorKind::TimedOut, violation_since(&generation, before)), PublishClass::Transient);
    }

    #[test]
    fn classify_covers_every_kind_with_and_without_the_flag() {
        use PublishErrorKind::*;
        let table = [
            (MaxPayloadExceeded, PublishClass::Permanent("max_payload_exceeded"), PublishClass::Permanent("max_payload_exceeded")),
            (StreamNotFound, PublishClass::Permanent("stream_not_found"), PublishClass::Permanent("stream_not_found")),
            (Other, PublishClass::Permanent("other"), PublishClass::Permanent("other")),
            (WrongLastMessageId, PublishClass::Permanent("other"), PublishClass::Permanent("other")),
            (WrongLastSequence, PublishClass::Permanent("other"), PublishClass::Permanent("other")),
            (TimedOut, PublishClass::Transient, PublishClass::Permanent("permission_violation")),
            (BrokenPipe, PublishClass::Transient, PublishClass::Transient),
            (MaxAckPending, PublishClass::Transient, PublishClass::Transient),
        ];
        for (kind, without, with) in table {
            assert_eq!(classify(kind, false), without, "{kind:?} without the flag");
            assert_eq!(classify(kind, true), with, "{kind:?} with the flag");
        }
    }

    /// The value of `name` in the file, without quotes.
    fn var(env: &str, name: &str) -> String {
        let line = env.lines().find(|l| l.starts_with(&format!("{name}="))).unwrap_or_else(|| panic!("{name} is not in subjects.env"));
        line[name.len() + 1..].trim().trim_matches('"').to_owned()
    }

    #[test]
    fn the_stream_config_matches_subjects_env() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../ops/nats/subjects.env");
        let env = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        // `DECISIONS_MAX_BYTES` is bash arithmetic, which Rust cannot evaluate. Read the default of the disk
        // setting (`${DECISIONS_ACCOUNT_DISK_BYTES:-N}`) and apply the 90 % rule of the file here.
        let disk = var(&env, "DECISIONS_ACCOUNT_DISK_BYTES");
        let default = disk
            .strip_prefix("${DECISIONS_ACCOUNT_DISK_BYTES:-")
            .and_then(|s| s.strip_suffix('}'))
            .unwrap_or_else(|| panic!("unexpected disk form: {disk}"));
        let disk_bytes: i64 = default.parse().expect("the default disk is an integer");
        assert_eq!(
            var(&env, "DECISIONS_MAX_BYTES"),
            "$(( DECISIONS_ACCOUNT_DISK_BYTES * 9 / 10 ))",
            "the rule that this test applies changed"
        );
        let cfg = stream_config();
        assert_eq!(cfg.name, var(&env, "DECISIONS_STREAM_NAME"));
        assert_eq!(cfg.subjects, vec![var(&env, "DECISIONS_SUBJECTS")]);
        assert_eq!(cfg.max_age, Duration::from_secs(var(&env, "DECISIONS_MAX_AGE_SECS").parse().expect("integer")));
        assert_eq!(cfg.max_bytes, disk_bytes * 9 / 10);
        assert_eq!(cfg.duplicate_window, Duration::from_secs(var(&env, "DECISIONS_DUPE_WINDOW_SECS").parse().expect("integer")));
        assert_eq!(cfg.max_message_size, var(&env, "DECISIONS_MAX_MSG_SIZE").parse::<i32>().expect("integer"));
        assert_eq!(var(&env, "GATEWAY_ACCOUNT"), "PAIGASUS_GATEWAY");
        assert_eq!(cfg.num_replicas, 1);
        assert_eq!(cfg.discard, jetstream::stream::DiscardPolicy::Old);
        assert_eq!(cfg.storage, jetstream::stream::StorageType::File);
        assert_eq!(cfg.retention, jetstream::stream::RetentionPolicy::Limits);
        assert_eq!(format!("{SUBJECT_PREFIX}.>"), cfg.subjects[0]);
    }
}
