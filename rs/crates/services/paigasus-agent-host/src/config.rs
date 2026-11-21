// SPDX-License-Identifier: Apache-2.0

//! Layered configuration: defaults, then `agent-host.toml`, then `AGENT_HOST_*` variables.

use std::net::SocketAddr;
use std::time::Duration;

use figment::providers::{Env, Format, Serialized, Toml};
use figment::{Figment, error::Error as FigmentError};
use paigasus_agent_host_core::claim::ClaimConfig;
use paigasus_agent_host_core::memory::MemoryCaps;
use paigasus_agent_host_core::mid_run::MidRunPolicy;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::adapters::nats::channels::filters_overlap;
use crate::adapters::nats::{DURABLE_NAME, STREAM_NAME};
use crate::application::lease_keeper::{KeeperTiming, KeeperTimingError};

/// A connection URL that can hold a password. `Debug` and `Serialize` print a fixed placeholder.
/// Read the real value with [`RedactedUrl::as_str`].
#[derive(Clone, PartialEq, Eq)]
pub struct RedactedUrl(String);

impl RedactedUrl {
    /// The real URL, for a connection constructor. Never log the result.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for RedactedUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("RedactedUrl").field(&"<redacted>").finish()
    }
}

impl<'de> Deserialize<'de> for RedactedUrl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer).map(RedactedUrl)
    }
}

impl Serialize for RedactedUrl {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str("<redacted>")
    }
}

/// The service configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Config {
    pub http_addr: SocketAddr,
    /// The Postgres connection string. The agent host needs its own database.
    pub database_url: RedactedUrl,
    pub log_level: String,
    pub metrics: MetricsConfig,
    pub migration: MigrationConfig,
    pub worker: WorkerConfig,
    pub lease: LeaseConfig,
    pub claim: ClaimSettings,
    pub ingress: IngressConfig,
    /// The IAM link of the ingress. The HTTP ingress is on only when this section is set.
    #[serde(default)]
    pub iam: Option<IamClientConfig>,
    pub mid_run: MidRunConfig,
    /// The NATS source (spec §9.1). No `[nats]` section means no NATS task.
    #[serde(default)]
    pub nats: Option<NatsConfig>,
    pub memory: MemoryConfig,
}

/// The agent memory store.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MemoryConfig {
    /// The store backend. A backend switch does not move data (spec 6.8).
    pub backend: MemoryBackend,
    pub caps: MemoryCapsConfig,
}

/// The memory backends. Only Postgres exists now. SMA-829 adds `s3`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum MemoryBackend {
    Postgres,
}

/// The memory limits. Every value is greater than 0. `max_loaded_file_bytes` is at most `max_file_bytes`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MemoryCapsConfig {
    pub max_file_bytes: u64,
    pub max_store_bytes: u64,
    pub max_store_files: u64,
    pub max_loaded_file_bytes: u64,
}

impl MemoryCapsConfig {
    /// The limits as the core type.
    #[must_use]
    pub fn to_caps(&self) -> MemoryCaps {
        MemoryCaps {
            max_file_bytes: self.max_file_bytes,
            max_store_bytes: self.max_store_bytes,
            max_store_files: self.max_store_files,
            max_loaded_file_bytes: self.max_loaded_file_bytes,
        }
    }
}

/// The NATS source (spec §9.1).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct NatsConfig {
    pub url: RedactedUrl,
    /// The credentials file of the `agent-host-consumer` identity.
    #[serde(default)]
    pub creds_file: Option<String>,
    /// A CA bundle for the NATS TLS link. The bundle replaces the system root certificates, so it
    /// must hold every root that the server chain needs.
    #[serde(default)]
    pub ca_file: Option<String>,
    pub stream: String,
    pub durable: String,
    pub channels: Vec<ChannelConfig>,
}

/// One channel: one subject filter that feeds exactly one inbox (spec §9.1).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ChannelConfig {
    pub subject: String,
    pub inbox: Uuid,
    /// The CloudEvents `source` of the events of this channel. It starts with `urn:paigasus:agents:nats:`.
    pub source: String,
}

/// The required prefix of a channel `source`.
const NATS_SOURCE_PREFIX: &str = "urn:paigasus:agents:nats:";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MetricsConfig {
    pub enabled: bool,
    /// A separate metrics listener. Its port must differ from the port of `http_addr`.
    pub addr: Option<SocketAddr>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MigrationConfig {
    /// Seconds to wait for the migration advisory lock. Range 1 to 3600.
    pub lock_wait_secs: u64,
}

impl MigrationConfig {
    /// The configured wait as a `Duration`.
    #[must_use]
    pub fn lock_wait(&self) -> Duration {
        Duration::from_secs(self.lock_wait_secs)
    }
}

/// The worker switch. The value is read and checked. It has no effect yet: the worker arrives in
/// a later task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct WorkerConfig {
    pub enabled: bool,
}

/// The inbox lease (spec §8.6, §8.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct LeaseConfig {
    /// The time to live of a lease. Range 3 to 600, and at least three heartbeat intervals.
    pub ttl_secs: u64,
    /// The time between two heartbeats. Range 1 to 200.
    pub heartbeat_interval_secs: u64,
}

impl LeaseConfig {
    /// The keeper timing of this config. After [`Config::validate`] it is always `Ok`.
    pub fn keeper_timing(&self) -> Result<KeeperTiming, KeeperTimingError> {
        KeeperTiming::new(Duration::from_secs(self.ttl_secs), Duration::from_secs(self.heartbeat_interval_secs))
    }

    /// The statement timeout of one heartbeat: half the heartbeat interval, and at least 1 ms.
    /// It is shorter than the interval (spec §8.6). It is never 0 ms, because a Postgres
    /// `statement_timeout` of 0 removes the bound.
    #[must_use]
    pub fn heartbeat_statement_timeout(&self) -> Duration {
        (Duration::from_secs(self.heartbeat_interval_secs) / 2).max(Duration::from_millis(1))
    }
}

/// The claim settings (spec §8.4, §8.7).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct ClaimSettings {
    /// The maximum number of events in one claim. Range 1 to 500.
    pub batch_limit: u32,
    /// The attempts after which an event is dead. Range 1 to 100.
    pub max_attempts: u32,
    /// The time to live of a thread. Range 60 to 31536000.
    pub thread_ttl_secs: u64,
}

impl ClaimSettings {
    /// The `ClaimConfig` of the core crate.
    #[must_use]
    pub fn to_claim_config(&self) -> ClaimConfig {
        ClaimConfig {
            batch_limit: self.batch_limit,
            max_attempts: self.max_attempts,
            thread_ttl: Duration::from_secs(self.thread_ttl_secs),
        }
    }
}

/// The HTTP ingress (spec §5.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct IngressConfig {
    /// The largest request body. Range 1024 to 4194304.
    pub max_body_bytes: usize,
}

/// The outbound IAM gRPC link (spec §6.2).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct IamClientConfig {
    pub grpc_addr: String,
    pub tls: IamTlsConfig,
    /// The timeout of one IAM call. Range 100 to 30000.
    #[serde(default = "default_call_timeout_ms")]
    pub call_timeout_ms: u64,
}

fn default_call_timeout_ms() -> u64 {
    2000
}

/// D8: TLS is the default transport to `paigasus-iam`'s gRPC endpoint — the introspect call
/// carries raw caller-presented API keys, so the link must be encrypted unless an operator
/// EXPLICITLY opts into `loopback_insecure`, and even then only when `iam.grpc_addr` resolves
/// to a loopback host (checked in [`Config::validate`] — `Deserialize` alone can't see
/// the sibling `grpc_addr` field to validate against). An enum (rather than a `bool` `enabled`
/// flag + always-present cert-path options) makes "insecure but cert paths configured" or
/// "secure but disabled" unrepresentable at the type level; G4 matches on this to build the
/// tonic channel.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum IamTlsConfig {
    Tls {
        /// Custom CA bundle to trust. The client PINS to this CA alone — it REPLACES the
        /// system trust store, not adds to it. `None` = system trust store only.
        #[serde(default)]
        ca_cert_path: Option<String>,
        /// mTLS client certificate. Must be set together with `client_key_path` or not at
        /// all — G4 validates that pairing when it builds the tonic channel.
        #[serde(default)]
        client_cert_path: Option<String>,
        #[serde(default)]
        client_key_path: Option<String>,
    },
    LoopbackInsecure,
}

/// The mid-run policy (spec §11). Only `queue` passes [`Config::validate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct MidRunConfig {
    pub policy: MidRunPolicy,
}

/// The config name of a mid-run policy, for the boot error.
fn policy_name(policy: &MidRunPolicy) -> &'static str {
    match policy {
        MidRunPolicy::Queue => "queue",
        MidRunPolicy::Fold => "fold",
        MidRunPolicy::Interrupt => "interrupt",
    }
}

/// A configuration value that is not valid.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ConfigError {
    #[error("metrics.addr must use a different port than http_addr")]
    MetricsPortClash,
    #[error("migration.lock_wait_secs must be between 1 and 3600 (got {0})")]
    LockWaitOutOfRange(u64),
    #[error("lease.ttl_secs must be between 3 and 600 (got {0})")]
    LeaseTtlOutOfRange(u64),
    #[error("lease.heartbeat_interval_secs must be between 1 and 200 (got {0})")]
    HeartbeatIntervalOutOfRange(u64),
    #[error("lease.ttl_secs ({ttl}) must be at least 3 x lease.heartbeat_interval_secs ({interval})")]
    LeaseTtlBelowThreeHeartbeats { ttl: u64, interval: u64 },
    #[error("claim.batch_limit must be between 1 and 500 (got {0})")]
    BatchLimitOutOfRange(u32),
    #[error("claim.max_attempts must be between 1 and 100 (got {0})")]
    MaxAttemptsOutOfRange(u32),
    #[error("claim.thread_ttl_secs must be between 60 and 31536000 (got {0})")]
    ThreadTtlOutOfRange(u64),
    #[error("ingress.max_body_bytes must be between 1024 and 4194304 (got {0})")]
    MaxBodyBytesOutOfRange(usize),
    #[error("iam.call_timeout_ms must be between 100 and 30000 (got {0})")]
    IamCallTimeoutOutOfRange(u64),
    #[error("iam.tls = \"loopback_insecure\" needs iam.grpc_addr to be a loopback host (127.0.0.0/8, localhost or ::1): the link carries raw credentials")]
    IamLoopbackInsecureNotLoopback,
    #[error("iam.tls = \"tls\" needs iam.grpc_addr to use the https scheme: an http address sends the credentials in plain text")]
    IamTlsNeedsHttps,
    #[error("mid_run.policy = \"{}\" is not supported: it needs the Helikon turn-boundary change (ADR (h)); use \"queue\"", policy_name(.0))]
    MidRunPolicyNotSupported(MidRunPolicy),
    #[error("nats.channels: the filters \"{a}\" and \"{b}\" can match the same subject")]
    NatsChannelsOverlap { a: String, b: String },
    #[error("nats.channels: the source \"{0}\" must start with \"urn:paigasus:agents:nats:\"")]
    NatsSourcePrefix(String),
    #[error("nats.channels: the source \"{0}\" has nothing after \"urn:paigasus:agents:nats:\"")]
    NatsSourceEmpty(String),
    #[error("nats.channels: a source has {0} characters; the limit is {1}")]
    NatsSourceTooLong(usize, usize),
    #[error("nats.channels: the source \"{0}\" is not a URI-reference (RFC 3986)")]
    NatsSourceNotUri(String),
    #[error("nats.channels: the source \"{0}\" is used by two channels")]
    NatsDuplicateSource(String),
    #[error("nats: the section needs at least one channel")]
    NatsNoChannel,
    #[error("nats.durable must be \"agent-host-inbox\" (got \"{0}\"): the NATS identity grants only that durable")]
    NatsDurableName(String),
    #[error("nats.stream must be \"IAM_EVENTS\" (got \"{0}\"): the NATS identity grants only that stream")]
    NatsStreamName(String),
    #[error("memory.caps.{0} must be greater than 0")]
    MemoryCapZero(&'static str),
    #[error("memory.caps.max_loaded_file_bytes must not be greater than memory.caps.max_file_bytes")]
    MemoryLoadedOverFile,
}

/// The defaults. `database_url` has none: it is required. `iam` has none: no `[iam]` section
/// means no ingress.
#[derive(Serialize)]
struct Defaults {
    http_addr: &'static str,
    log_level: &'static str,
    metrics: MetricsDefaults,
    migration: MigrationConfigDefaults,
    worker: WorkerConfig,
    lease: LeaseConfig,
    claim: ClaimSettings,
    ingress: IngressConfig,
    mid_run: MidRunConfig,
    memory: MemoryDefaults,
}

#[derive(Serialize)]
struct MemoryDefaults {
    backend: &'static str,
    caps: MemoryCapsDefaults,
}

#[derive(Serialize)]
struct MemoryCapsDefaults {
    max_file_bytes: u64,
    max_store_bytes: u64,
    max_store_files: u64,
    max_loaded_file_bytes: u64,
}

#[derive(Serialize)]
struct MetricsDefaults {
    enabled: bool,
}

#[derive(Serialize)]
struct MigrationConfigDefaults {
    lock_wait_secs: u64,
}

/// True when `grpc_addr`'s host is a loopback address (`127.0.0.0/8`, `localhost`, or an IPv6
/// loopback such as `::1`).
///
/// The host comes from the URI AUTHORITY, parsed by the `http` crate's `Uri`: the same parser
/// that `tonic::transport::Channel::from_shared` uses, so this check reads the host that the
/// channel connects to. A text split is not safe here. The old `split("://").last()` read the
/// text after the LAST `://`, so `http://192.0.2.10:9090/p/http://127.0.0.1:9090` passed the
/// check while the channel connected to `192.0.2.10` in plaintext. A userinfo part is not the
/// host either: in `http://127.0.0.1@192.0.2.10/` the host is `192.0.2.10`. A value that does
/// not parse as a URI with a host is not loopback.
///
/// The host is then classified by parsing it as a real IP and asking
/// [`Ipv4Addr::is_loopback`]/[`Ipv6Addr::is_loopback`] — NOT a string prefix match. A prefix
/// match like `host.starts_with("127.")` is trivially bypassable: `127.evil.com` and
/// `127.0.0.1.attacker.com` both pass it yet are DNS-resolvable to arbitrary addresses, which
/// would let an operator disable TLS on a link that then carries raw API keys off-loopback
/// (D8). `localhost` is matched by EXACT equality for the same reason (`localhost.evil.com`
/// must NOT match).
///
/// [`Ipv4Addr::is_loopback`]: std::net::Ipv4Addr::is_loopback
/// [`Ipv6Addr::is_loopback`]: std::net::Ipv6Addr::is_loopback
fn is_loopback_host(grpc_addr: &str) -> bool {
    use std::net::{Ipv4Addr, Ipv6Addr};

    let Ok(uri) = grpc_addr.parse::<axum::http::Uri>() else {
        return false;
    };
    let Some(host) = uri.host() else {
        return false;
    };
    // `Uri::host` keeps the brackets of an IPv6 literal ("[::1]"). Strip them, so the
    // `Ipv6Addr` parse below succeeds.
    let host = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);

    host == "localhost" || host.parse::<Ipv4Addr>().map(|ip| ip.is_loopback()).unwrap_or(false) || host.parse::<Ipv6Addr>().map(|ip| ip.is_loopback()).unwrap_or(false)
}

/// True when `s` is a URI-reference (RFC 3986 section 4.1): it uses only the characters that RFC
/// 3986 allows, each `%` starts a valid percent escape, and each component holds only its own
/// characters. A space, a control character and a non-ASCII character are not allowed. The
/// characters `<>"{}|\^` and the backtick are not allowed either.
///
/// The component rules: the first `#` starts the fragment, and the fragment has no second `#`.
/// `[` and `]` are allowed only in the authority (an IP literal), never in the path, the query or
/// the fragment. So `urn:paigasus:agents:nats:a#b#c` is refused.
fn is_uri_reference(s: &str) -> bool {
    if !has_uri_characters(s) {
        return false;
    }
    let (before_fragment, fragment) = s.split_once('#').unwrap_or((s, ""));
    if fragment.contains(['#', '[', ']']) {
        return false;
    }
    let (hier, query) = before_fragment.split_once('?').unwrap_or((before_fragment, ""));
    if query.contains(['[', ']']) {
        return false;
    }
    // The scheme ends at the first `:` before any `/`. A `//` after it (or at the start of a
    // relative reference) starts the authority, which ends at the next `/`.
    let after_scheme = match hier.find([':', '/']) {
        Some(i) if hier.as_bytes()[i] == b':' => &hier[i + 1..],
        _ => hier,
    };
    let path = match after_scheme.strip_prefix("//") {
        Some(rest) => rest.find('/').map_or("", |i| &rest[i..]),
        None => after_scheme,
    };
    !path.contains(['[', ']'])
}

/// True when `s` uses only the characters that RFC 3986 allows in a URI-reference, and each `%`
/// starts a valid percent escape.
fn has_uri_characters(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' => {
                if !(i + 2 < b.len() && b[i + 1].is_ascii_hexdigit() && b[i + 2].is_ascii_hexdigit()) {
                    return false;
                }
                i += 3;
                continue;
            }
            c if c.is_ascii_alphanumeric() => {}
            b'-' | b'.' | b'_' | b'~' | b':' | b'/' | b'?' | b'#' | b'[' | b']' | b'@' | b'!' | b'$' | b'&' | b'\'' | b'(' | b')' | b'*' | b'+' | b',' | b';' | b'=' => {}
            _ => return false,
        }
        i += 1;
    }
    true
}

/// The channel rules of spec §9.1.
fn validate_nats_channels(channels: &[ChannelConfig]) -> Result<(), ConfigError> {
    if channels.is_empty() {
        return Err(ConfigError::NatsNoChannel);
    }
    for (i, a) in channels.iter().enumerate() {
        if !a.source.starts_with(NATS_SOURCE_PREFIX) {
            return Err(ConfigError::NatsSourcePrefix(a.source.clone()));
        }
        if a.source.len() == NATS_SOURCE_PREFIX.len() {
            return Err(ConfigError::NatsSourceEmpty(a.source.clone()));
        }
        // The CloudEvents `source` is a URI. A longer one makes the consumer refuse every event.
        let chars = a.source.chars().count();
        if chars > paigasus_cloudevents::URI_MAX_CHARS {
            return Err(ConfigError::NatsSourceTooLong(chars, paigasus_cloudevents::URI_MAX_CHARS));
        }
        if !is_uri_reference(&a.source) {
            return Err(ConfigError::NatsSourceNotUri(a.source.clone()));
        }
        for b in &channels[i + 1..] {
            if a.source == b.source {
                return Err(ConfigError::NatsDuplicateSource(a.source.clone()));
            }
            if filters_overlap(&a.subject, &b.subject) {
                return Err(ConfigError::NatsChannelsOverlap {
                    a: a.subject.clone(),
                    b: b.subject.clone(),
                });
            }
        }
    }
    Ok(())
}

impl Config {
    /// The figment chain: defaults, `agent-host.toml`, then `AGENT_HOST_*` (nested split `__`).
    #[must_use]
    pub fn figment() -> Figment {
        Figment::from(Serialized::defaults(Defaults {
            http_addr: "0.0.0.0:8086",
            log_level: "info",
            metrics: MetricsDefaults { enabled: true },
            migration: MigrationConfigDefaults { lock_wait_secs: 120 },
            worker: WorkerConfig { enabled: true },
            lease: LeaseConfig {
                ttl_secs: 30,
                heartbeat_interval_secs: 10,
            },
            claim: ClaimSettings {
                batch_limit: 20,
                max_attempts: 5,
                thread_ttl_secs: 86_400,
            },
            ingress: IngressConfig { max_body_bytes: 262_144 },
            mid_run: MidRunConfig { policy: MidRunPolicy::Queue },
            memory: MemoryDefaults {
                backend: "postgres",
                caps: MemoryCapsDefaults {
                    max_file_bytes: 65_536,
                    max_store_bytes: 1_048_576,
                    max_store_files: 512,
                    max_loaded_file_bytes: 8_192,
                },
            },
        }))
        .merge(Toml::file("agent-host.toml"))
        .merge(Env::prefixed("AGENT_HOST_").split("__"))
    }

    // `figment::Error` is large (about 200 bytes), and clippy flags it in a `Result`. The public
    // signature returns it directly, so the lint is allowed here.
    #[allow(clippy::result_large_err)]
    pub fn load() -> Result<Self, FigmentError> {
        Self::figment().extract()
    }

    /// Boot-time checks that serde does not make. Each range check runs before the rule that
    /// pairs `lease.ttl_secs` with `lease.heartbeat_interval_secs`.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Compare the port only. Two addresses with one port still fail at bind time.
        if let Some(addr) = self.metrics.addr
            && addr.port() == self.http_addr.port()
        {
            return Err(ConfigError::MetricsPortClash);
        }
        if !(1..=3600).contains(&self.migration.lock_wait_secs) {
            return Err(ConfigError::LockWaitOutOfRange(self.migration.lock_wait_secs));
        }
        let lease = &self.lease;
        if !(3..=600).contains(&lease.ttl_secs) {
            return Err(ConfigError::LeaseTtlOutOfRange(lease.ttl_secs));
        }
        if !(1..=200).contains(&lease.heartbeat_interval_secs) {
            return Err(ConfigError::HeartbeatIntervalOutOfRange(lease.heartbeat_interval_secs));
        }
        // Both values are in range here, so the product cannot overflow.
        if lease.ttl_secs < 3 * lease.heartbeat_interval_secs {
            return Err(ConfigError::LeaseTtlBelowThreeHeartbeats {
                ttl: lease.ttl_secs,
                interval: lease.heartbeat_interval_secs,
            });
        }
        if !(1..=500).contains(&self.claim.batch_limit) {
            return Err(ConfigError::BatchLimitOutOfRange(self.claim.batch_limit));
        }
        if !(1..=100).contains(&self.claim.max_attempts) {
            return Err(ConfigError::MaxAttemptsOutOfRange(self.claim.max_attempts));
        }
        if !(60..=31_536_000).contains(&self.claim.thread_ttl_secs) {
            return Err(ConfigError::ThreadTtlOutOfRange(self.claim.thread_ttl_secs));
        }
        if !(1024..=4_194_304).contains(&self.ingress.max_body_bytes) {
            return Err(ConfigError::MaxBodyBytesOutOfRange(self.ingress.max_body_bytes));
        }
        if let Some(iam) = &self.iam {
            if !(100..=30_000).contains(&iam.call_timeout_ms) {
                return Err(ConfigError::IamCallTimeoutOutOfRange(iam.call_timeout_ms));
            }
            if matches!(iam.tls, IamTlsConfig::LoopbackInsecure) && !is_loopback_host(&iam.grpc_addr) {
                return Err(ConfigError::IamLoopbackInsecureNotLoopback);
            }
            // tonic picks the transport from the scheme: an `http://` address stays plain text,
            // also when a TLS config is set.
            if matches!(iam.tls, IamTlsConfig::Tls { .. }) && !iam.grpc_addr.get(..8).is_some_and(|p| p.eq_ignore_ascii_case("https://")) {
                return Err(ConfigError::IamTlsNeedsHttps);
            }
        }
        if self.mid_run.policy != MidRunPolicy::Queue {
            return Err(ConfigError::MidRunPolicyNotSupported(self.mid_run.policy));
        }
        if let Some(nats) = &self.nats {
            if nats.durable != DURABLE_NAME {
                return Err(ConfigError::NatsDurableName(nats.durable.clone()));
            }
            if nats.stream != STREAM_NAME {
                return Err(ConfigError::NatsStreamName(nats.stream.clone()));
            }
            validate_nats_channels(&nats.channels)?;
        }
        let caps = &self.memory.caps;
        for (name, value) in [
            ("max_file_bytes", caps.max_file_bytes),
            ("max_store_bytes", caps.max_store_bytes),
            ("max_store_files", caps.max_store_files),
            ("max_loaded_file_bytes", caps.max_loaded_file_bytes),
        ] {
            if value == 0 {
                return Err(ConfigError::MemoryCapZero(name));
            }
        }
        if caps.max_loaded_file_bytes > caps.max_file_bytes {
            return Err(ConfigError::MemoryLoadedOverFile);
        }
        Ok(())
    }
}

// `figment::Jail::expect_with` fixes its closure error type to `figment::Error`.
#[cfg(test)]
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;

    fn with_url() -> Config {
        Config::figment().extract().expect("config")
    }

    #[test]
    fn defaults_bind_the_container_port() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            let cfg = with_url();
            assert_eq!(cfg.http_addr.to_string(), "0.0.0.0:8086");
            assert_eq!(cfg.log_level, "info");
            assert!(cfg.metrics.enabled);
            assert_eq!(cfg.metrics.addr, None);
            assert_eq!(cfg.migration.lock_wait_secs, 120);
            assert_eq!(cfg.database_url.as_str(), "postgres://u:p@localhost/db");
            Ok(())
        });
    }

    #[test]
    fn database_url_is_required() {
        figment::Jail::expect_with(|jail| {
            jail.clear_env();
            let err = Config::load().expect_err("database_url is missing");
            assert!(err.to_string().contains("database_url"), "the error must name database_url: {err}");
            // Positive control: with the variable set, the same load succeeds.
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            assert!(Config::load().is_ok());
            Ok(())
        });
    }

    #[test]
    fn debug_redacts_the_database_url() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:s3cret@localhost/db");
            let cfg = with_url();
            assert!(!format!("{cfg:?}").contains("s3cret"));
            Ok(())
        });
    }

    #[test]
    fn the_toml_file_and_nested_env_keys_are_read() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.set_env("AGENT_HOST_METRICS__ADDR", "127.0.0.1:9100");
            jail.create_file("agent-host.toml", "log_level = \"debug\"\n")?;
            let cfg = Config::load()?;
            assert_eq!(cfg.log_level, "debug");
            assert_eq!(cfg.metrics.addr, Some("127.0.0.1:9100".parse().unwrap()));
            Ok(())
        });
    }

    #[test]
    fn metrics_addr_on_the_http_port_is_rejected() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.set_env("AGENT_HOST_METRICS__ADDR", "127.0.0.1:8086");
            let cfg = with_url();
            assert_eq!(cfg.validate(), Err(ConfigError::MetricsPortClash));
            // The negative half: a different port is accepted.
            jail.set_env("AGENT_HOST_METRICS__ADDR", "127.0.0.1:9100");
            assert_eq!(with_url().validate(), Ok(()));
            Ok(())
        });
    }

    #[test]
    fn lock_wait_outside_1_to_3600_is_rejected() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            let mut cfg = with_url();
            for (secs, ok) in [(0, false), (1, true), (3600, true), (3601, false)] {
                cfg.migration.lock_wait_secs = secs;
                assert_eq!(cfg.validate().is_ok(), ok, "lock_wait_secs = {secs}");
            }
            Ok(())
        });
    }

    fn loopback_iam() -> IamClientConfig {
        IamClientConfig {
            grpc_addr: "http://127.0.0.1:9090".to_owned(),
            tls: IamTlsConfig::LoopbackInsecure,
            call_timeout_ms: 2000,
        }
    }

    #[test]
    fn the_new_defaults_match_the_spec() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            let cfg = with_url();
            assert!(cfg.worker.enabled);
            assert_eq!(cfg.lease.ttl_secs, 30);
            assert_eq!(cfg.lease.heartbeat_interval_secs, 10);
            assert_eq!(cfg.claim.batch_limit, 20);
            assert_eq!(cfg.claim.max_attempts, 5);
            assert_eq!(cfg.claim.thread_ttl_secs, 86400);
            assert_eq!(cfg.ingress.max_body_bytes, 262_144);
            assert_eq!(cfg.mid_run.policy, MidRunPolicy::Queue);
            assert_eq!(cfg.iam, None);
            assert_eq!(cfg.validate(), Ok(()));
            Ok(())
        });
    }

    #[test]
    fn memory_defaults() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            let cfg = with_url();
            assert_eq!(cfg.memory.backend, MemoryBackend::Postgres);
            let caps = cfg.memory.caps.to_caps();
            assert_eq!(caps.max_file_bytes, 65_536);
            assert_eq!(caps.max_store_bytes, 1_048_576);
            assert_eq!(caps.max_store_files, 512);
            assert_eq!(caps.max_loaded_file_bytes, 8_192);
            assert_eq!(caps, MemoryCaps::default());
            assert_eq!(cfg.validate(), Ok(()));
            Ok(())
        });
    }

    /// One §8.7 range: the key, a setter, the bounds, and the error of a value out of range.
    struct RangeRow {
        key: &'static str,
        set: fn(&mut Config, u64),
        min: u64,
        max: u64,
        err: fn(u64) -> ConfigError,
    }

    #[test]
    fn each_range_is_enforced_at_both_ends() {
        // Ruling R5: the paired lease key is set so that each bound is tested in a valid
        // combination (ttl 3 with interval 1; interval 200 with ttl 600).
        let rows = [
            RangeRow {
                key: "lease.ttl_secs",
                set: |c, v| {
                    c.lease.heartbeat_interval_secs = 1;
                    c.lease.ttl_secs = v;
                },
                min: 3,
                max: 600,
                err: ConfigError::LeaseTtlOutOfRange,
            },
            RangeRow {
                key: "lease.heartbeat_interval_secs",
                set: |c, v| {
                    c.lease.ttl_secs = 600;
                    c.lease.heartbeat_interval_secs = v;
                },
                min: 1,
                max: 200,
                err: ConfigError::HeartbeatIntervalOutOfRange,
            },
            RangeRow {
                key: "claim.batch_limit",
                set: |c, v| c.claim.batch_limit = u32::try_from(v).unwrap(),
                min: 1,
                max: 500,
                err: |v| ConfigError::BatchLimitOutOfRange(u32::try_from(v).unwrap()),
            },
            RangeRow {
                key: "claim.max_attempts",
                set: |c, v| c.claim.max_attempts = u32::try_from(v).unwrap(),
                min: 1,
                max: 100,
                err: |v| ConfigError::MaxAttemptsOutOfRange(u32::try_from(v).unwrap()),
            },
            RangeRow {
                key: "claim.thread_ttl_secs",
                set: |c, v| c.claim.thread_ttl_secs = v,
                min: 60,
                max: 31_536_000,
                err: ConfigError::ThreadTtlOutOfRange,
            },
            RangeRow {
                key: "ingress.max_body_bytes",
                set: |c, v| c.ingress.max_body_bytes = usize::try_from(v).unwrap(),
                min: 1024,
                max: 4_194_304,
                err: |v| ConfigError::MaxBodyBytesOutOfRange(usize::try_from(v).unwrap()),
            },
            RangeRow {
                key: "iam.call_timeout_ms",
                set: |c, v| c.iam = Some(IamClientConfig { call_timeout_ms: v, ..loopback_iam() }),
                min: 100,
                max: 30_000,
                err: ConfigError::IamCallTimeoutOutOfRange,
            },
        ];
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            for row in &rows {
                for (value, ok) in [(row.min - 1, false), (row.min, true), (row.max, true), (row.max + 1, false)] {
                    let mut cfg = with_url();
                    (row.set)(&mut cfg, value);
                    let expected = if ok { Ok(()) } else { Err((row.err)(value)) };
                    assert_eq!(cfg.validate(), expected, "{} = {value}", row.key);
                }
            }
            Ok(())
        });
    }

    #[test]
    fn memory_from_env() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.set_env("AGENT_HOST_MEMORY__CAPS__MAX_FILE_BYTES", "1000");
            jail.set_env("AGENT_HOST_MEMORY__CAPS__MAX_LOADED_FILE_BYTES", "500");
            let cfg = with_url();
            assert_eq!(cfg.memory.caps.max_file_bytes, 1000);
            assert_eq!(cfg.memory.caps.max_loaded_file_bytes, 500);
            assert_eq!(cfg.memory.caps.max_store_bytes, 1_048_576);
            assert_eq!(cfg.validate(), Ok(()));
            Ok(())
        });
    }

    #[test]
    fn memory_backend_s3_is_refused_for_now() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.set_env("AGENT_HOST_MEMORY__BACKEND", "s3");
            let err = Config::load().expect_err("s3 is not a backend yet");
            assert!(err.to_string().contains("s3"), "the error must name the value: {err}");
            // Positive control: the postgres value loads.
            jail.set_env("AGENT_HOST_MEMORY__BACKEND", "postgres");
            assert!(Config::load().is_ok());
            Ok(())
        });
    }

    #[test]
    fn memory_cap_zero_is_invalid() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            for name in ["max_file_bytes", "max_store_bytes", "max_store_files", "max_loaded_file_bytes"] {
                let mut cfg = with_url();
                match name {
                    "max_file_bytes" => cfg.memory.caps.max_file_bytes = 0,
                    "max_store_bytes" => cfg.memory.caps.max_store_bytes = 0,
                    "max_store_files" => cfg.memory.caps.max_store_files = 0,
                    _ => cfg.memory.caps.max_loaded_file_bytes = 0,
                }
                assert_eq!(cfg.validate(), Err(ConfigError::MemoryCapZero(name)), "{name} = 0");
            }
            Ok(())
        });
    }

    #[test]
    fn the_ttl_must_be_three_heartbeats() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            let mut cfg = with_url();
            cfg.lease.heartbeat_interval_secs = 10;
            cfg.lease.ttl_secs = 29;
            assert_eq!(cfg.validate(), Err(ConfigError::LeaseTtlBelowThreeHeartbeats { ttl: 29, interval: 10 }));
            cfg.lease.ttl_secs = 30;
            assert_eq!(cfg.validate(), Ok(()));
            Ok(())
        });
    }

    #[test]
    fn fold_and_interrupt_are_refused_at_boot() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file("agent-host.toml", "[mid_run]\npolicy = \"fold\"\n")?;
            let mut cfg = Config::load()?;
            let err = cfg.validate().expect_err("fold is refused");
            assert_eq!(err, ConfigError::MidRunPolicyNotSupported(MidRunPolicy::Fold));
            assert!(err.to_string().contains("turn-boundary"), "{err}");
            assert_eq!(
                err.to_string(),
                "mid_run.policy = \"fold\" is not supported: it needs the Helikon turn-boundary change (ADR (h)); use \"queue\""
            );
            cfg.mid_run.policy = MidRunPolicy::Interrupt;
            let err = cfg.validate().expect_err("interrupt is refused");
            assert_eq!(err, ConfigError::MidRunPolicyNotSupported(MidRunPolicy::Interrupt));
            assert!(err.to_string().starts_with("mid_run.policy = \"interrupt\" is not supported"), "{err}");
            cfg.mid_run.policy = MidRunPolicy::Queue;
            assert_eq!(cfg.validate(), Ok(()));
            Ok(())
        });
    }

    #[test]
    fn iam_call_timeout_defaults_to_2000() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file("agent-host.toml", "[iam]\ngrpc_addr = \"http://127.0.0.1:9090\"\ntls = { mode = \"loopback_insecure\" }\n")?;
            let cfg = Config::load()?;
            assert_eq!(cfg.iam, Some(loopback_iam()));
            assert_eq!(cfg.validate(), Ok(()));
            Ok(())
        });
    }

    #[test]
    fn loopback_insecure_needs_a_loopback_host() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            let mut cfg = with_url();
            cfg.iam = Some(IamClientConfig {
                grpc_addr: "http://10.0.0.1:9090".to_owned(),
                ..loopback_iam()
            });
            assert_eq!(cfg.validate(), Err(ConfigError::IamLoopbackInsecureNotLoopback));
            // A loopback address after a second `://` in the path is not the host (PR 518).
            for addr in ["http://192.0.2.10:9090/path/http://127.0.0.1:9090", "http://127.0.0.1@192.0.2.10/"] {
                cfg.iam = Some(IamClientConfig {
                    grpc_addr: addr.to_owned(),
                    ..loopback_iam()
                });
                assert_eq!(cfg.validate(), Err(ConfigError::IamLoopbackInsecureNotLoopback), "{addr}");
            }
            cfg.iam = Some(loopback_iam());
            assert_eq!(cfg.validate(), Ok(()));
            // TLS needs no loopback host.
            cfg.iam = Some(IamClientConfig {
                grpc_addr: "https://10.0.0.1:9090".to_owned(),
                tls: IamTlsConfig::Tls {
                    ca_cert_path: None,
                    client_cert_path: None,
                    client_key_path: None,
                },
                call_timeout_ms: 2000,
            });
            assert_eq!(cfg.validate(), Ok(()));
            Ok(())
        });
    }

    #[test]
    fn the_claim_and_keeper_settings_come_from_the_config() {
        let claim = ClaimSettings {
            batch_limit: 7,
            max_attempts: 3,
            thread_ttl_secs: 120,
        };
        assert_eq!(
            claim.to_claim_config(),
            ClaimConfig {
                batch_limit: 7,
                max_attempts: 3,
                thread_ttl: Duration::from_secs(120),
            }
        );
        let lease = LeaseConfig {
            ttl_secs: 30,
            heartbeat_interval_secs: 10,
        };
        assert_eq!(lease.keeper_timing(), KeeperTiming::new(Duration::from_secs(30), Duration::from_secs(10)));
        assert!(lease.keeper_timing().is_ok());
    }

    #[test]
    fn the_heartbeat_statement_timeout_is_at_least_1ms_and_shorter_than_the_interval() {
        for interval in [1, 2, 10, 200] {
            let lease = LeaseConfig {
                ttl_secs: 600,
                heartbeat_interval_secs: interval,
            };
            let timeout = lease.heartbeat_statement_timeout();
            // The value goes into `SET LOCAL statement_timeout = '<n>ms'`, where 0 disables it.
            assert!(timeout.as_millis() >= 1, "interval {interval}: {timeout:?}");
            assert!(timeout < Duration::from_secs(interval), "interval {interval}: {timeout:?}");
        }
    }

    // `is_loopback_host`: copied from the gateway with its tests. A prefix match such as
    // `starts_with("127.")` is bypassed by a crafted host name.
    #[test]
    fn is_loopback_host_rejects_crafted_hostnames() {
        assert!(!is_loopback_host("https://127.evil.com:9090"));
        assert!(!is_loopback_host("https://127.0.0.1.attacker.example.com:9090"));
        assert!(!is_loopback_host("https://localhost.evil.com:9090"));
        assert!(!is_loopback_host("https://iam.internal.example.com:9090"));
    }

    // The host comes from the URI authority only (CodeRabbit, PR 518). A loopback address in the
    // PATH, after a second `://`, is not the host: the channel connects to the authority.
    #[test]
    fn is_loopback_host_reads_the_authority_not_a_later_scheme_in_the_path() {
        assert!(!is_loopback_host("http://192.0.2.10:9090/path/http://127.0.0.1:9090"));
        assert!(!is_loopback_host("http://iam.internal.example.com:9090/proxy://127.0.0.1:9090"));
        assert!(!is_loopback_host("http://192.0.2.10:9090/http://[::1]:9090"));
        assert!(!is_loopback_host("http://192.0.2.10/?next=http://localhost"));
        // A loopback authority with a path is still loopback.
        assert!(is_loopback_host("http://127.0.0.1:9090/path/http://192.0.2.10:9090"));
    }

    // A userinfo part is not the host: `127.0.0.1@` names a user, and the host is after the `@`.
    #[test]
    fn is_loopback_host_does_not_read_userinfo_as_the_host() {
        assert!(!is_loopback_host("http://127.0.0.1@192.0.2.10/"));
        assert!(!is_loopback_host("http://localhost:9090@192.0.2.10:9090"));
        assert!(!is_loopback_host("http://[::1]@192.0.2.10:9090"));
        assert!(!is_loopback_host("http://user:pass@iam.internal.example.com:9090"));
    }

    // A value with no parsable authority is not loopback (fail closed).
    #[test]
    fn is_loopback_host_fails_closed_on_a_value_that_is_not_a_uri() {
        assert!(!is_loopback_host(""));
        assert!(!is_loopback_host("not a uri"));
        assert!(!is_loopback_host("/127.0.0.1:9090"));
    }

    #[test]
    fn is_loopback_host_accepts_real_loopback_addresses() {
        assert!(is_loopback_host("http://127.0.0.1:9090"));
        // Every address in 127.0.0.0/8 is loopback.
        assert!(is_loopback_host("http://127.0.0.5:9090"));
        assert!(is_loopback_host("http://localhost:9090"));
        // An IPv6 literal: the brackets are removed before the parse.
        assert!(is_loopback_host("http://[::1]:9090"));
        // A bare host with no scheme and no port.
        assert!(is_loopback_host("127.0.0.1"));
    }

    fn channel(subject: &str, n: u128, source: &str) -> ChannelConfig {
        ChannelConfig {
            subject: subject.to_string(),
            inbox: Uuid::from_u128(n),
            source: source.to_string(),
        }
    }

    fn with_channels(channels: Vec<ChannelConfig>) -> Config {
        let mut cfg = with_url();
        cfg.nats = Some(NatsConfig {
            url: RedactedUrl("nats://localhost:4222".to_string()),
            creds_file: None,
            ca_file: None,
            stream: "IAM_EVENTS".to_string(),
            durable: "agent-host-inbox".to_string(),
            channels,
        });
        cfg
    }

    #[test]
    fn two_overlapping_channels_are_refused() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            let cfg = with_channels(vec![
                channel("iam.>", 1, "urn:paigasus:agents:nats:a"),
                channel("iam.organization.created", 2, "urn:paigasus:agents:nats:b"),
            ]);
            assert_eq!(
                cfg.validate(),
                Err(ConfigError::NatsChannelsOverlap {
                    a: "iam.>".to_string(),
                    b: "iam.organization.created".to_string()
                })
            );
            let ok = with_channels(vec![
                channel("iam.organization.>", 1, "urn:paigasus:agents:nats:a"),
                channel("iam.principal.>", 2, "urn:paigasus:agents:nats:b"),
            ]);
            assert_eq!(ok.validate(), Ok(()));
            Ok(())
        });
    }

    #[test]
    fn a_source_without_the_nats_prefix_is_refused() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            let cfg = with_channels(vec![channel("iam.>", 1, "urn:paigasus:agents:http:a")]);
            assert_eq!(cfg.validate(), Err(ConfigError::NatsSourcePrefix("urn:paigasus:agents:http:a".to_string())));
            Ok(())
        });
    }

    #[test]
    fn a_source_that_is_only_the_prefix_is_refused() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            let cfg = with_channels(vec![channel("iam.>", 1, "urn:paigasus:agents:nats:")]);
            assert_eq!(cfg.validate(), Err(ConfigError::NatsSourceEmpty("urn:paigasus:agents:nats:".to_string())));
            Ok(())
        });
    }

    #[test]
    fn a_source_that_is_not_a_uri_reference_is_refused() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            for bad in [
                "urn:paigasus:agents:nats:iam west",
                "urn:paigasus:agents:nats:a%zz",
                "urn:paigasus:agents:nats:a%4",
                "urn:paigasus:agents:nats:\u{e4}",
                "urn:paigasus:agents:nats:a<b",
                // The component rules: one fragment, and no `[` or `]` outside an authority.
                "urn:paigasus:agents:nats:a#b#c",
                "urn:paigasus:agents:nats:a[1]",
                "urn:paigasus:agents:nats:a?q=[1]",
                "urn:paigasus:agents:nats:a#[x]",
            ] {
                assert_eq!(with_channels(vec![channel("iam.>", 1, bad)]).validate(), Err(ConfigError::NatsSourceNotUri(bad.to_string())), "{bad}");
            }
            assert_eq!(with_channels(vec![channel("iam.>", 1, "urn:paigasus:agents:nats:iam-west_1.x~y%41")]).validate(), Ok(()));
            Ok(())
        });
    }

    #[test]
    fn is_uri_reference_checks_the_components() {
        assert!(is_uri_reference("urn:paigasus:agents:nats:a#frag?x/y"));
        assert!(is_uri_reference("http://[::1]:9090/p?q#f"));
        assert!(is_uri_reference("//[::1]/p"));
        assert!(!is_uri_reference("http://h/p[1]"));
        assert!(!is_uri_reference("a#b#c"));
    }

    #[test]
    fn tls_needs_an_https_iam_address() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            let tls = |addr: &str| IamClientConfig {
                grpc_addr: addr.to_owned(),
                tls: IamTlsConfig::Tls {
                    ca_cert_path: None,
                    client_cert_path: None,
                    client_key_path: None,
                },
                call_timeout_ms: 2000,
            };
            let mut cfg = with_url();
            for bad in ["http://iam.internal:9090", "iam.internal:9090", "HTTP://x:1"] {
                cfg.iam = Some(tls(bad));
                assert_eq!(cfg.validate(), Err(ConfigError::IamTlsNeedsHttps), "{bad}");
            }
            cfg.iam = Some(tls("HTTPS://iam.internal:9090"));
            assert_eq!(cfg.validate(), Ok(()));
            Ok(())
        });
    }

    #[test]
    fn a_source_over_the_uri_limit_is_refused() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            let max = paigasus_cloudevents::URI_MAX_CHARS;
            let at_limit = format!("{NATS_SOURCE_PREFIX}{}", "a".repeat(max - NATS_SOURCE_PREFIX.len()));
            assert_eq!(at_limit.chars().count(), max);
            assert_eq!(with_channels(vec![channel("iam.>", 1, &at_limit)]).validate(), Ok(()));
            let over = format!("{at_limit}a");
            assert_eq!(with_channels(vec![channel("iam.>", 1, &over)]).validate(), Err(ConfigError::NatsSourceTooLong(max + 1, max)));
            Ok(())
        });
    }

    #[test]
    fn two_channels_with_one_source_are_refused() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            let cfg = with_channels(vec![channel("a.>", 1, "urn:paigasus:agents:nats:x"), channel("b.>", 2, "urn:paigasus:agents:nats:x")]);
            assert_eq!(cfg.validate(), Err(ConfigError::NatsDuplicateSource("urn:paigasus:agents:nats:x".to_string())));
            Ok(())
        });
    }

    #[test]
    fn another_durable_or_stream_is_refused() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            let mut cfg = with_channels(vec![channel("iam.>", 1, "urn:paigasus:agents:nats:a")]);
            assert_eq!(cfg.validate(), Ok(()));
            cfg.nats.as_mut().expect("nats").durable = "other".to_string();
            assert_eq!(cfg.validate(), Err(ConfigError::NatsDurableName("other".to_string())));
            let mut cfg = with_channels(vec![channel("iam.>", 1, "urn:paigasus:agents:nats:a")]);
            cfg.nats.as_mut().expect("nats").stream = "OTHER".to_string();
            assert_eq!(cfg.validate(), Err(ConfigError::NatsStreamName("OTHER".to_string())));
            Ok(())
        });
    }

    #[test]
    fn a_nats_section_without_a_channel_is_refused() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            assert_eq!(with_channels(vec![]).validate(), Err(ConfigError::NatsNoChannel));
            Ok(())
        });
    }

    #[test]
    fn the_nats_section_is_optional() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            let cfg = with_url();
            assert!(cfg.nats.is_none());
            assert_eq!(cfg.validate(), Ok(()));
            Ok(())
        });
    }

    #[test]
    fn the_nats_section_is_read_from_toml() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            jail.create_file(
                "agent-host.toml",
                "[nats]\nurl = \"nats://h:4222\"\nstream = \"IAM_EVENTS\"\ndurable = \"agent-host-inbox\"\n\n[[nats.channels]]\nsubject = \"iam.organization.>\"\ninbox = \"00000000-0000-0000-0000-000000000001\"\nsource = \"urn:paigasus:agents:nats:iam-org\"\n",
            )?;
            let cfg = Config::load()?;
            let nats = cfg.nats.as_ref().expect("nats");
            assert_eq!(nats.channels.len(), 1);
            assert_eq!(nats.channels[0].inbox, Uuid::from_u128(1));
            assert_eq!(cfg.validate(), Ok(()));
            Ok(())
        });
    }

    #[test]
    fn loaded_cap_over_file_cap_is_invalid() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("AGENT_HOST_DATABASE_URL", "postgres://u:p@localhost/db");
            let mut cfg = with_url();
            cfg.memory.caps.max_file_bytes = 100;
            cfg.memory.caps.max_loaded_file_bytes = 101;
            assert_eq!(cfg.validate(), Err(ConfigError::MemoryLoadedOverFile));
            // The boundary is accepted.
            cfg.memory.caps.max_loaded_file_bytes = 100;
            assert_eq!(cfg.validate(), Ok(()));
            Ok(())
        });
    }
}
