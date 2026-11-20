// SPDX-License-Identifier: Apache-2.0

//! The JetStream side of the decision log (SMA-755 § 5.7, § 5.10). It holds the stream constants, the subject
//! of an event, the NATS connection of the relay and the classification of a publish error (P-14, P-20).
//!
//! A missing stream is a TRANSIENT publish error (R9): `async-nats` reports a JetStream that is not ready (a restart,
//! a leader election) as `StreamNotFound`. So the relay keeps the entry and makes the stream again.
//!
//! A PubAck error comes as `PublishErrorKind::Other` with the JetStream API error in its source. The relay reads the
//! server error code from it. A resource or availability code (a full account disk, for example) is TRANSIENT, so a
//! full disk does not delete valid events. A code that rejects the message itself is PERMANENT. The class text in
//! the log names the code group (`message_too_large`, `jetstream_rejected`, `other` and so on).
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
/// The name of the generation record bucket (SMA-821). The bucket is the stream `KV_GATEWAY_DECISIONS_GEN`.
pub const GEN_BUCKET: &str = "GATEWAY_DECISIONS_GEN";
/// The size limit of the generation bucket, in bytes. It must stay inside the 10 % of the account disk that the
/// decision stream leaves free.
pub const GEN_MAX_BYTES: i64 = 1_048_576;
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
/// 90 % of the default 20 GiB account disk (P-20). The relay uses a smaller value when the account limit is smaller
/// ([`max_bytes_for`]).
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

/// The `max_bytes` of the stream for the account storage limit `account_max_storage` (P-20). The rule is the rule of
/// `subjects.env`: 90 % of the account disk, with integer division. The result is never above [`MAX_BYTES`] and
/// never below 1. A limit of 0 or less (no limit) gives [`MAX_BYTES`].
#[must_use]
pub fn max_bytes_for(account_max_storage: i64) -> i64 {
    if account_max_storage <= 0 {
        return MAX_BYTES;
    }
    let ninety = i128::from(account_max_storage) * 9 / 10;
    i64::try_from(ninety).unwrap_or(MAX_BYTES).clamp(1, MAX_BYTES)
}

/// The file storage limit of the account in a `$JS.API.INFO` reply. The untiered limit comes first. An account
/// with tiered limits has its R1 limit in the tier `R1` (the stream has one replica). -1 when there is no limit.
#[must_use]
pub fn account_max_storage(account: &jetstream::account::Account) -> i64 {
    let tier = account.tiers.get("R1").and_then(|t| t.limits.max_storage);
    match account.limits.max_storage {
        Some(n) if n > 0 => n,
        _ => tier.filter(|n| *n > 0).unwrap_or(-1),
    }
}

/// The per-stream file storage limit of the account (`storage_max_stream_bytes`) in a `$JS.API.INFO` reply. The
/// untiered limit comes first, then the tier `R1`, as in [`account_max_storage`]. `None` when there is no such
/// limit. A broker refuses a stream with a `max_bytes` above this limit.
#[must_use]
pub fn account_max_stream_bytes(account: &jetstream::account::Account) -> Option<i64> {
    let tier = account.tiers.get("R1").and_then(|t| t.limits.storage_max_stream_bytes);
    account.limits.storage_max_stream_bytes.filter(|n| *n > 0).or_else(|| tier.filter(|n| *n > 0))
}

/// The `max_bytes` of the stream for an account: [`max_bytes_for`] of the account storage limit, and never above
/// the per-stream limit of the account ([`account_max_stream_bytes`]).
#[must_use]
pub fn max_bytes_for_account(account: &jetstream::account::Account) -> i64 {
    let bytes = max_bytes_for(account_max_storage(account));
    account_max_stream_bytes(account).map_or(bytes, |cap| bytes.min(cap))
}

/// The stream config, with `max_bytes` from [`max_bytes_for_account`]. `provision-decisions.sh` makes the same
/// stream from `ops/nats/subjects.env`, with one difference: the script does not read a per-stream limit of the
/// account, because the account that it makes has none. The relay calls `get_or_create_stream` with this value, so a stream that the relay
/// makes is never unbounded.
#[must_use]
pub fn stream_config(max_bytes: i64) -> jetstream::stream::Config {
    jetstream::stream::Config {
        name: STREAM.to_owned(),
        subjects: vec![format!("{SUBJECT_PREFIX}.>")],
        retention: jetstream::stream::RetentionPolicy::Limits,
        storage: jetstream::stream::StorageType::File,
        max_age: Duration::from_secs(MAX_AGE_SECS),
        max_bytes,
        discard: jetstream::stream::DiscardPolicy::Old,
        duplicate_window: Duration::from_secs(DUPLICATE_WINDOW_SECS),
        max_message_size: MAX_MESSAGE_SIZE,
        num_replicas: 1,
        ..Default::default()
    }
}

/// The config of the generation bucket. `provision-decisions.sh` makes the same bucket from `ops/nats/subjects.env`,
/// and a unit test pins the two together. History is 1, because only the newest value of a key matters.
#[must_use]
pub fn gen_kv_config() -> jetstream::kv::Config {
    jetstream::kv::Config {
        bucket: GEN_BUCKET.to_owned(),
        history: 1,
        storage: jetstream::stream::StorageType::File,
        num_replicas: 1,
        max_age: Duration::ZERO,
        max_bytes: GEN_MAX_BYTES,
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
    /// A `TimedOut` with a violation while other publishes were in flight: the violation can belong to another
    /// publish. Keep the entry in Redis and publish it again alone.
    Ambiguous,
}

/// The JetStream API error in a PubAck: the status (`code`) and the server error code (`err_code`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiError {
    /// The status, for example 400 or 503.
    pub status: usize,
    /// The server error code, for example 10023. 0 when the broker sent none.
    pub code: u64,
}

/// The JetStream API error of a publish error. `async-nats` puts the PubAck error (`jetstream::Error`) in the
/// source of a `PublishErrorKind::Other`. `None` when the source is another error (for example an invalid
/// subject) or when there is no source.
#[must_use]
pub fn api_error_of(err: &jetstream::context::PublishError) -> Option<ApiError> {
    let source = std::error::Error::source(err)?;
    source.downcast_ref::<jetstream::Error>().map(|e| ApiError {
        status: e.code(),
        code: e.error_code().0,
    })
}

/// The server error codes of a resource or availability condition of the broker, the account or the stream. The
/// condition is not a property of the message, so the relay tries the message again. The meanings are from the
/// `nats-server` v2.10.14 error table (`server/errors.json`) and its publish path (`processJetStreamMsg`):
/// - 10002 `JSAccountResourcesExceededErr`: the account storage limit is full.
/// - 10023 `JSInsufficientResourcesErr`: the server JetStream limits are full.
/// - 10028 `JSMemoryResourcesExceededErr` and 10047 `JSStorageResourcesExceededErr`: no memory or disk is free.
/// - 10077 `JSStreamStoreFailedF`: the store refused the write (for example a disk error or a closed store).
/// - 10120 (`ErrorCode::NO_LIMITS` in `async-nats`): the account has no JetStream limit. Each message fails until
///   an operator sets one.
/// - 10008 `JSClusterNotAvailErr`, 10009 (`ErrorCode::CLUSTER_NOT_LEADER`), 10063 `JSStreamSequenceNotMatchErr`,
///   10118 `JSStreamOfflineErr`, 10039 `JSNotEnabledForAccountErr` and 10076 `JSNotEnabledErr`: JetStream is
///   not available now.
///
/// - 10060 `JSStreamNotMatchErr` ("expected stream does not match"): the relay sends `Nats-Expected-Stream:
///   GATEWAY_DECISIONS`, and another stream holds the subject. This is a configuration fault of the broker, not a
///   property of the message. The relay keeps the event until an operator repairs the streams.
///
/// Three codes come from newer servers. Their names and texts are from the `async-nats` 0.53 `ErrorCode` table:
/// - 10158 `STREAM_DUPLICATE_MESSAGE_CONFLICT` ("Stream duplicate message conflict"): the broker still
///   processes a message with the same id. A retry gets the result of that message.
/// - 10167 `STREAM_TOO_MANY_REQUESTS` ("Stream too many requests"): the stream is busy now.
/// - 10194 `STREAM_OFFLINE_REASON` ("Stream offline reason"): the stream is offline now.
pub const TRANSIENT_CODES: [u64; 16] = [10002, 10023, 10028, 10047, 10077, 10120, 10008, 10009, 10063, 10118, 10039, 10076, 10060, 10158, 10167, 10194];

/// The class of a PubAck error. A status 503 with no server code is transient too: `nats-server` sends it when a
/// clustered stream cannot propose the message. Each other code rejects this message, for example 10054
/// (`JSStreamMessageExceedsMaximumErr`: the message is larger than `max_msg_size`).
fn classify_api(api: ApiError) -> PublishClass {
    if TRANSIENT_CODES.contains(&api.code) || (api.code == 0 && api.status == 503) {
        return PublishClass::Transient;
    }
    match api.code {
        10054 => PublishClass::Permanent("message_too_large"),
        10097 => PublishClass::Permanent("header_too_large"),
        10109 => PublishClass::Permanent("stream_sealed"),
        _ => PublishClass::Permanent("jetstream_rejected"),
    }
}

/// Classify a publish error. `async-nats` reports a refused publish as `TimedOut`, so `violation_seen` (set
/// by the connection event callback) turns a `TimedOut` into a permanent error. The violation counter is shared
/// by all publishes. When `concurrent` is true (other publishes ran at the same time), the violation can belong
/// to another publish, so the result is `Ambiguous`, not `Permanent`. `api` is the JetStream API error of the
/// PubAck ([`api_error_of`]). It sets the class of an `Other`.
#[must_use]
pub fn classify(kind: PublishErrorKind, api: Option<ApiError>, violation_seen: bool, concurrent: bool) -> PublishClass {
    match kind {
        PublishErrorKind::MaxPayloadExceeded => PublishClass::Permanent("max_payload_exceeded"),
        PublishErrorKind::TimedOut if violation_seen && concurrent => PublishClass::Ambiguous,
        PublishErrorKind::TimedOut if violation_seen => PublishClass::Permanent("permission_violation"),
        // `async-nats` maps a NO_RESPONDERS reply to `StreamNotFound`. A JetStream that is not ready (a restart, a
        // leader election) answers that way, so "no stream for the subject" is retried (R9). The relay makes the
        // stream again with `get_or_create_stream` on the next iteration.
        PublishErrorKind::StreamNotFound | PublishErrorKind::TimedOut | PublishErrorKind::BrokenPipe | PublishErrorKind::MaxAckPending => PublishClass::Transient,
        // `async-nats` 0.53 maps each PubAck error code to `Other`, except 10070 and 10071
        // (`PublishAckFuture::next_with_timeout`). So the server error code sets the class. An `Other` with no API
        // error (an invalid subject, a PubAck that does not parse) is permanent.
        PublishErrorKind::Other => api.map_or(PublishClass::Permanent("other"), classify_api),
        // The relay never sets an expected last id or sequence, so these two kinds are not expected.
        PublishErrorKind::WrongLastMessageId | PublishErrorKind::WrongLastSequence => PublishClass::Permanent("other"),
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
        assert_eq!(classify(PublishErrorKind::TimedOut, None, violation_since(&generation, before), false), PublishClass::Transient);
        // A violation during the publish does.
        generation.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            classify(PublishErrorKind::TimedOut, None, violation_since(&generation, before), false),
            PublishClass::Permanent("permission_violation")
        );
        // The next publish reads the new generation and is transient again.
        let before = generation.load(Ordering::SeqCst);
        assert_eq!(classify(PublishErrorKind::TimedOut, None, violation_since(&generation, before), false), PublishClass::Transient);
    }

    #[test]
    fn a_concurrent_timeout_with_a_violation_is_ambiguous_a_solo_one_is_permanent() {
        assert_eq!(classify(PublishErrorKind::TimedOut, None, true, true), PublishClass::Ambiguous);
        assert_eq!(classify(PublishErrorKind::TimedOut, None, true, false), PublishClass::Permanent("permission_violation"));
        assert_eq!(classify(PublishErrorKind::TimedOut, None, false, true), PublishClass::Transient);
        assert_eq!(classify(PublishErrorKind::TimedOut, None, false, false), PublishClass::Transient);
        // Only a timeout is ambiguous: a hard error stays what it is.
        assert_eq!(classify(PublishErrorKind::MaxPayloadExceeded, None, true, true), PublishClass::Permanent("max_payload_exceeded"));
        assert_eq!(classify(PublishErrorKind::BrokenPipe, None, true, true), PublishClass::Transient);
    }

    #[test]
    fn classify_covers_every_kind_with_and_without_the_flag() {
        use PublishErrorKind::*;
        let table = [
            (MaxPayloadExceeded, PublishClass::Permanent("max_payload_exceeded"), PublishClass::Permanent("max_payload_exceeded")),
            (StreamNotFound, PublishClass::Transient, PublishClass::Transient),
            (Other, PublishClass::Permanent("other"), PublishClass::Permanent("other")),
            (WrongLastMessageId, PublishClass::Permanent("other"), PublishClass::Permanent("other")),
            (WrongLastSequence, PublishClass::Permanent("other"), PublishClass::Permanent("other")),
            (TimedOut, PublishClass::Transient, PublishClass::Permanent("permission_violation")),
            (BrokenPipe, PublishClass::Transient, PublishClass::Transient),
            (MaxAckPending, PublishClass::Transient, PublishClass::Transient),
        ];
        for (kind, without, with) in table {
            assert_eq!(classify(kind, None, false, false), without, "{kind:?} without the flag");
            assert_eq!(classify(kind, None, true, false), with, "{kind:?} with the flag");
            assert_eq!(classify(kind, None, false, true), without, "{kind:?} concurrent without the flag");
        }
    }

    fn api(status: usize, code: u64) -> Option<ApiError> {
        Some(ApiError { status, code })
    }

    #[test]
    fn classify_sets_the_class_of_an_other_from_the_jetstream_code() {
        let transient = [
            (400, 10002), // account resources exceeded
            (503, 10023), // insufficient resources
            (500, 10028), // memory resources exceeded
            (500, 10047), // storage resources exceeded
            (503, 10077), // stream store failed
            (500, 10120), // no limits
            (503, 10008), // cluster not available
            (500, 10009), // cluster not leader
            (503, 10063), // stream sequence does not match
            (500, 10118), // stream offline
            (503, 10039), // JetStream not enabled for the account
            (503, 10076), // JetStream not enabled
            (400, 10060), // expected stream does not match: another stream holds the subject
            (400, 10158), // duplicate message conflict: the same message id is in process (newer servers)
            (429, 10167), // stream too many requests (newer servers)
            (500, 10194), // stream offline reason (newer servers)
            (503, 0),     // a propose failure: status 503 and no code
        ];
        for (status, code) in transient {
            for (violation, concurrent) in [(false, false), (true, false), (true, true)] {
                assert_eq!(classify(PublishErrorKind::Other, api(status, code), violation, concurrent), PublishClass::Transient, "{code}");
            }
        }
        assert_eq!(TRANSIENT_CODES.len() + 1, transient.len(), "the table covers each transient code");
        let permanent = [
            (400, 10054, "message_too_large"),
            (400, 10097, "header_too_large"),
            (400, 10109, "stream_sealed"),
            (500, 10111, "jetstream_rejected"), // rollup failed
            (400, 10070, "jetstream_rejected"), // wrong last message id, if a future client maps it to `Other`
            (400, 0, "jetstream_rejected"),     // no code and not 503
            (500, 0, "jetstream_rejected"),
        ];
        for (status, code, class) in permanent {
            assert_eq!(classify(PublishErrorKind::Other, api(status, code), false, false), PublishClass::Permanent(class), "{code}");
        }
        assert_eq!(classify(PublishErrorKind::Other, None, false, false), PublishClass::Permanent("other"));
        // The code only matters for `Other`. Each other kind keeps its own class.
        assert_eq!(
            classify(PublishErrorKind::MaxPayloadExceeded, api(500, 10047), false, false),
            PublishClass::Permanent("max_payload_exceeded")
        );
        assert_eq!(classify(PublishErrorKind::WrongLastSequence, api(400, 10071), false, false), PublishClass::Permanent("other"));
        assert_eq!(classify(PublishErrorKind::TimedOut, api(500, 10054), false, false), PublishClass::Transient);
    }

    #[test]
    fn api_error_of_reads_the_puback_error_from_the_source() {
        let body = r#"{"code":500,"err_code":10047,"description":"insufficient storage resources available"}"#;
        let source: jetstream::Error = serde_json::from_str(body).expect("a PubAck error parses");
        let err = jetstream::context::PublishError::with_source(PublishErrorKind::Other, source);
        assert_eq!(api_error_of(&err), api(500, 10047));
        // A body with no `err_code` gives code 0.
        let source: jetstream::Error = serde_json::from_str(r#"{"code":503,"description":"proposal failed"}"#).expect("parses");
        let err = jetstream::context::PublishError::with_source(PublishErrorKind::Other, source);
        assert_eq!(api_error_of(&err), api(503, 0));
        // Another source, or no source, gives `None`.
        let err = jetstream::context::PublishError::with_source(PublishErrorKind::Other, std::io::Error::other("invalid subject"));
        assert_eq!(api_error_of(&err), None);
        assert_eq!(api_error_of(&jetstream::context::PublishError::new(PublishErrorKind::TimedOut)), None);
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
        assert_eq!(MAX_BYTES, disk_bytes * 9 / 10, "the default max_bytes is 90 % of the default disk");
        assert_eq!(max_bytes_for(disk_bytes), MAX_BYTES, "the relay rule gives the same value for the default disk");
        let cfg = stream_config(max_bytes_for(disk_bytes));
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
        assert_eq!(GEN_BUCKET, var(&env, "DECISIONS_GEN_BUCKET"));
        assert_eq!(GEN_MAX_BYTES.to_string(), var(&env, "DECISIONS_GEN_MAX_BYTES"));
        let kv = gen_kv_config();
        assert_eq!(kv.bucket, var(&env, "DECISIONS_GEN_BUCKET"));
        assert_eq!(kv.history, 1);
        assert_eq!(kv.storage, jetstream::stream::StorageType::File);
        assert_eq!(kv.num_replicas, 1);
        assert_eq!(kv.max_age, Duration::ZERO);
        assert_eq!(kv.max_bytes, GEN_MAX_BYTES);
        assert_eq!(format!("{SUBJECT_PREFIX}.>"), cfg.subjects[0]);
        // The relay reads the account limit with `$JS.API.INFO` before it makes the stream, so the publisher
        // identity must have that grant.
        let block = env.split("GW_DEC_PUBLISHER_PUB=(").nth(1).and_then(|s| s.split(')').next()).expect("the publisher grants");
        assert!(block.lines().any(|l| l.trim() == "'$JS.API.INFO'"), "GW_DEC_PUBLISHER_PUB grants $JS.API.INFO: {block}");
    }

    #[test]
    fn max_bytes_for_takes_ninety_percent_of_a_smaller_account_limit() {
        // No limit: the default.
        assert_eq!(max_bytes_for(-1), MAX_BYTES);
        assert_eq!(max_bytes_for(0), MAX_BYTES);
        assert_eq!(max_bytes_for(i64::MIN), MAX_BYTES);
        // A smaller limit: 90 %, with integer division as in `subjects.env`.
        assert_eq!(max_bytes_for(268_435_456), 241_591_910);
        assert_eq!(max_bytes_for(1_073_741_824), 966_367_641);
        assert_eq!(max_bytes_for(11), 9);
        assert_eq!(max_bytes_for(19), 17);
        // The result is never 0, which JetStream would read as "no limit".
        assert_eq!(max_bytes_for(1), 1);
        // The default disk gives the default; a larger limit does not raise it.
        assert_eq!(max_bytes_for(21_474_836_480), MAX_BYTES);
        assert_eq!(max_bytes_for(21_474_836_481), MAX_BYTES);
        assert_eq!(max_bytes_for(1 << 50), MAX_BYTES);
        assert_eq!(max_bytes_for(i64::MAX), MAX_BYTES);
        // Just below the default disk: 90 % of it is below the default.
        assert_eq!(max_bytes_for(21_474_836_470), 19_327_352_823);
        assert_eq!(stream_config(max_bytes_for(268_435_456)).max_bytes, 241_591_910);
    }

    #[test]
    fn max_bytes_for_account_never_goes_above_the_per_stream_limit() {
        use jetstream::account::{Account, Tier};
        let mut account = Account::default();
        assert_eq!(account_max_stream_bytes(&account), None, "no per-stream limit");
        assert_eq!(max_bytes_for_account(&account), MAX_BYTES);
        account.limits.max_storage = Some(21_474_836_480);
        account.limits.storage_max_stream_bytes = Some(1_073_741_824);
        assert_eq!(max_bytes_for_account(&account), 1_073_741_824, "the per-stream limit is below 90 % of the disk");
        account.limits.storage_max_stream_bytes = Some(i64::MAX);
        assert_eq!(max_bytes_for_account(&account), MAX_BYTES, "a larger per-stream limit does not raise the value");
        account.limits.storage_max_stream_bytes = Some(-1);
        assert_eq!(max_bytes_for_account(&account), MAX_BYTES, "-1 is no limit");
        account.limits.storage_max_stream_bytes = Some(0);
        assert_eq!(max_bytes_for_account(&account), MAX_BYTES, "0 is no limit");
        // A tiered account has its limits in the tier `R1`.
        account.limits.max_storage = Some(0);
        let mut r1 = Tier::default();
        r1.limits.max_storage = Some(268_435_456);
        r1.limits.storage_max_stream_bytes = Some(104_857_600);
        account.tiers.insert("R1".to_owned(), r1);
        assert_eq!(account_max_stream_bytes(&account), Some(104_857_600));
        assert_eq!(max_bytes_for_account(&account), 104_857_600);
        // 90 % of the account disk stays the limit when it is below the per-stream limit.
        account.tiers.get_mut("R1").expect("the tier").limits.storage_max_stream_bytes = Some(268_435_456);
        assert_eq!(max_bytes_for_account(&account), 241_591_910);
    }

    #[test]
    fn account_max_storage_reads_the_untiered_limit_then_the_r1_tier() {
        use jetstream::account::{Account, Tier};
        let mut account = Account::default();
        assert_eq!(account_max_storage(&account), -1, "no limit");
        account.limits.max_storage = Some(268_435_456);
        assert_eq!(account_max_storage(&account), 268_435_456);
        let mut r1 = Tier::default();
        r1.limits.max_storage = Some(1_048_576);
        account.tiers.insert("R1".to_owned(), r1);
        assert_eq!(account_max_storage(&account), 268_435_456, "the untiered limit comes first");
        account.limits.max_storage = Some(0);
        assert_eq!(account_max_storage(&account), 1_048_576, "a tiered account has 0 as its untiered limit");
        account.limits.max_storage = None;
        assert_eq!(account_max_storage(&account), 1_048_576);
        let mut r3 = Tier::default();
        r3.limits.max_storage = Some(5);
        account.tiers.clear();
        account.tiers.insert("R3".to_owned(), r3);
        assert_eq!(account_max_storage(&account), -1, "only the R1 tier counts for a stream with one replica");
    }
}
