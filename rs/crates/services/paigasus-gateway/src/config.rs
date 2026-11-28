// SPDX-License-Identifier: Apache-2.0

//! Service configuration via figment: built-in defaults < `gateway.toml` < `GATEWAY_*` env.

use figment::providers::{Env, Format, Serialized, Toml};
use figment::{Figment, error::Error as FigmentError};
use paigasus_gateway_core::{OnUnavailable, TenantMode};
use paigasus_kernel::detect::Severity;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt;
use std::net::SocketAddr;
use std::num::NonZeroU64;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use crate::application::decision_log::DigestKey;
use crate::application::detectors::pipeline::BUILT_IN_NAMES;
use crate::application::tool_policy::FINDING_KINDS;
use crate::domain::detectors::OrgScope;
use crate::domain::limits::{BudgetPeriod, LimitRules, MAX_REQUESTS_PER_MINUTE, MAX_TOKENS_PER_PERIOD, OrgOverride};
use crate::domain::parse_org_uuid;

/// M0 walking-skeleton config (see the module doc): one HTTP port, an IAM gRPC client
/// endpoint (G4 dials it), and the single OpenAI upstream (G6 calls out to it). No database,
/// no gRPC server of its own — mirrors `paigasus-iam::config::IamConfig`'s figment pattern
/// but is deliberately much smaller. Does NOT derive `PartialEq`: `upstream` transitively
/// carries `OpenAiConfig::api_key: SecretString`, which `secrecy` does not implement
/// `PartialEq` for (see `OpenAiConfig`'s doc).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct GatewayConfig {
    pub http_addr: SocketAddr,
    pub connect_timeout_secs: u64,
    pub first_byte_timeout_secs: u64,
    pub stream_idle_timeout_secs: u64,
    pub max_request_bytes: usize,
    pub iam: IamClientConfig,
    pub upstream: UpstreamConfig,
    pub log_level: String,
    pub metrics: MetricsConfig,
    /// SMA-505: whether streamed (SSE) chat completions are served. `false` rejects a request
    /// carrying `stream: true` with `400` and `param: "stream"` — the OpenAI idiom for an
    /// unsupported request parameter — and withdraws the `gateway.chat.stream` capability.
    /// Non-streaming requests are unaffected. `400` rather than `501` because OpenAI-compatible
    /// SDKs commonly retry 5xx, which would turn a deliberate configuration choice into
    /// repeated load.
    pub stream_enabled: bool,
    /// SMA-677: the optional rate limit and token budget on `POST /v1/chat/completions`. `None`
    /// (no `[limits]` table, the default) means no limit at all and no Redis connection (A6, Q5).
    /// The `Defaults` layer has no `limits` entry on purpose.
    #[serde(default)]
    pub limits: Option<LimitsConfig>,
    /// SMA-700: the DPoP scheme on the protected routes. Off by default (D4). The chart does not
    /// deploy the gateway, so the operator sets `GATEWAY_DPOP__ENABLED`. Turn DPoP on in IAM first.
    #[serde(default)]
    pub dpop: GatewayDpopConfig,
    /// SMA-749: tool-call interception on `POST /v1/chat/completions`. Off by default (D26).
    #[serde(default)]
    pub tool_interception: ToolInterceptionConfig,
    /// SMA-769: the `POST /v1/responses` route. Off by default (D8).
    #[serde(default)]
    pub responses: ResponsesConfig,
    /// SMA-750: the `POST /v1/messages` route. Off by default (D3).
    #[serde(default)]
    pub messages: MessagesConfig,
    /// SMA-752: tool-call decisions from Cedar policies. Off by default (D8).
    #[serde(default)]
    pub tool_policy: ToolPolicyConfig,
    /// SMA-755: the tool decision log. Off by default (§ 5.12).
    #[serde(default)]
    pub decision_log: DecisionLogConfig,
    /// SMA-753: the tool-call history store. `None` (no `[tool_history]` table, the default)
    /// means no history and no Redis connection (D12). The `Defaults` layer has no entry on purpose.
    #[serde(default)]
    pub tool_history: Option<ToolHistoryConfig>,
}

/// The IAM gRPC client endpoint G4 dials (`Introspect`/authorization calls). `tls` governs
/// how that channel is secured — see [`IamTlsConfig`]'s doc for the D8 rationale.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct IamClientConfig {
    pub grpc_addr: String,
    pub tls: IamTlsConfig,
    /// SMA-775: the file that holds the gateway service credential (an IAM API key with the
    /// `tool_policy_reader` role). The IAM policy source sends it on the two policy fetch calls and
    /// on no other call. The file is read again on every fetch, so a rotation needs no restart.
    /// Required when `tool_policy.source` is `kind = "iam"`.
    #[serde(default)]
    pub service_credential_file: Option<String>,
}

/// D8: TLS is the default transport to `paigasus-iam`'s gRPC endpoint — the introspect call
/// carries raw caller-presented API keys, so the link must be encrypted unless an operator
/// EXPLICITLY opts into `loopback_insecure`, and even then only when `iam.grpc_addr` resolves
/// to a loopback host (checked in [`GatewayConfig::validate`] — `Deserialize` alone can't see
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

impl Default for IamClientConfig {
    fn default() -> Self {
        IamClientConfig {
            grpc_addr: "https://127.0.0.1:9090".to_string(),
            tls: IamTlsConfig::default(),
            service_credential_file: None,
        }
    }
}

impl Default for IamTlsConfig {
    fn default() -> Self {
        IamTlsConfig::Tls {
            ca_cert_path: None,
            client_cert_path: None,
            client_key_path: None,
        }
    }
}

/// Outbound upstream(s) the gateway calls on the caller's behalf. Only OpenAI today (G6 is
/// the first consumer) — a future upstream would add a sibling field here.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct UpstreamConfig {
    pub openai: OpenAiConfig,
    /// SMA-750: the Anthropic egress target of `POST /v1/messages`. Absent by default. The table
    /// is required when `[messages] enabled = true`.
    #[serde(default)]
    pub anthropic: Option<AnthropicConfig>,
}

fn default_anthropic_base_url() -> String {
    "https://api.anthropic.com".to_string()
}

fn default_anthropic_version() -> String {
    "2023-06-01".to_string()
}

const fn default_anthropic_non_stream_timeout_secs() -> u64 {
    600
}

/// The longest `messages.max_hold_secs` (24 hours).
const MAX_MESSAGES_HOLD_SECS: u64 = 86_400;

/// `[upstream.anthropic]` (SMA-750 D3): the Anthropic egress target. Every key has a default
/// except `api_key`, which has none. A misspelt key fails extraction.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AnthropicConfig {
    #[serde(default = "default_anthropic_base_url")]
    pub base_url: String,
    /// Read only from `GATEWAY_UPSTREAM__ANTHROPIC__API_KEY`, never from `gateway.toml`. It is
    /// never serialized, and `SecretString` redacts it on `Debug`.
    #[serde(skip_serializing)]
    pub api_key: SecretString,
    /// Extra trust anchors, as a path to a PEM bundle. This ADDS to the trust store. The rule is
    /// the same as `upstream.openai.extra_ca_bundle_path`.
    #[serde(default)]
    pub extra_ca_bundle_path: Option<String>,
    /// The `anthropic-version` value that the gateway sends when the caller sends none.
    #[serde(default = "default_anthropic_version")]
    pub default_version: String,
    /// The time limit of one non-stream upstream call, in seconds.
    #[serde(default = "default_anthropic_non_stream_timeout_secs")]
    pub non_stream_timeout_secs: u64,
}

/// `[messages]` (SMA-750 D3, D15). Off by default: with `enabled = false`, `/v1/messages` returns
/// 404, and the key is the rollback with no deploy. A misspelt key fails extraction.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct MessagesConfig {
    pub enabled: bool,
    /// The cap on one request body of the route.
    pub max_request_bytes: usize,
    /// One SSE record larger than this is a failure, never skipped.
    pub max_record_bytes: usize,
    /// The queued bytes of one stream. Also the cap on one non-stream body.
    pub max_held_bytes: usize,
    /// The longest time that one tool call is held.
    pub max_hold_secs: u64,
}

impl Default for MessagesConfig {
    fn default() -> Self {
        MessagesConfig {
            enabled: false,
            max_request_bytes: 33_554_432,
            max_record_bytes: 8_388_608,
            max_held_bytes: 33_554_432,
            max_hold_secs: 600,
        }
    }
}

impl MessagesConfig {
    /// Every value is at least 1, one record must fit in the held bytes, and `max_hold_secs` is
    /// at most 86 400. Each message names the key. Checked even when off.
    pub fn validate(&self) -> Result<(), String> {
        for (key, v) in [
            ("messages.max_request_bytes", self.max_request_bytes as u64),
            ("messages.max_record_bytes", self.max_record_bytes as u64),
            ("messages.max_held_bytes", self.max_held_bytes as u64),
            ("messages.max_hold_secs", self.max_hold_secs),
        ] {
            if v == 0 {
                return Err(format!("{key} must be at least 1"));
            }
        }
        if self.max_record_bytes > self.max_held_bytes {
            return Err(format!(
                "messages.max_record_bytes ({}) must not exceed messages.max_held_bytes ({})",
                self.max_record_bytes, self.max_held_bytes
            ));
        }
        if self.max_hold_secs > MAX_MESSAGES_HOLD_SECS {
            return Err(format!("messages.max_hold_secs must be at most {MAX_MESSAGES_HOLD_SECS}; got {}", self.max_hold_secs));
        }
        Ok(())
    }
}

impl AnthropicConfig {
    fn validate(&self) -> Result<(), String> {
        if self.api_key.expose_secret().is_empty() {
            return Err("upstream.anthropic.api_key must not be empty (set GATEWAY_UPSTREAM__ANTHROPIC__API_KEY)".to_string());
        }
        // The key becomes the `x-api-key` header value. A value that is not a valid header value
        // (for example a trailing newline) would fail on every request. The key is not trimmed
        // and never appears in the message.
        if axum::http::HeaderValue::from_str(self.api_key.expose_secret()).is_err() {
            return Err("upstream.anthropic.api_key has an invalid character (a header value cannot hold a newline or a control character; remove it from the secret)".to_string());
        }
        // The client builds every URL as `base_url + route path`: it needs an absolute URL with a
        // scheme and a host. Without one, every request fails at run time.
        match reqwest::Url::parse(&self.base_url) {
            Ok(u) if matches!(u.scheme(), "http" | "https") && u.host_str().is_some() && u.query().is_none() && u.fragment().is_none() => {}
            _ => {
                return Err(format!(
                    "upstream.anthropic.base_url must be an absolute http or https URL with a host and no query or fragment (for example https://api.anthropic.com); got {:?}",
                    self.base_url
                ));
            }
        }
        if axum::http::HeaderValue::from_str(&self.default_version).is_err() || self.default_version.trim().is_empty() {
            return Err(format!("upstream.anthropic.default_version is not a valid header value: {:?}", self.default_version));
        }
        if self.non_stream_timeout_secs == 0 {
            return Err("upstream.anthropic.non_stream_timeout_secs must be at least 1".to_string());
        }
        if let Some(path) = self.extra_ca_bundle_path.as_deref() {
            if path.trim().is_empty() {
                return Err("upstream.anthropic.extra_ca_bundle_path must not be empty (omit the key entirely to use the default trust store)".to_string());
            }
            if path != path.trim() {
                return Err(format!(
                    "upstream.anthropic.extra_ca_bundle_path has leading/trailing whitespace, which is never valid config: {path:?}"
                ));
            }
        }
        Ok(())
    }
}

/// The OpenAI egress target (G6 consumes this to build the outbound client).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OpenAiConfig {
    pub base_url: String,
    /// Sourced from `GATEWAY_UPSTREAM__OPENAI__API_KEY`, never from `gateway.toml` (see
    /// `gateway.toml.example`). `#[serde(skip_serializing)]` so a config log/`readyz` dump
    /// (`GatewayConfig` derives `Serialize`) never emits it; `SecretString`'s own `Debug`
    /// already redacts it for the derived `Debug` path — mirrors iam's `RawPepper` redaction
    /// posture, but `secrecy`'s `serde` feature already supplies `Deserialize` for
    /// `SecretString`, so no hand-rolled newtype is needed here.
    #[serde(skip_serializing)]
    pub api_key: SecretString,
    /// Extra trust anchors for the outbound upstream calls, as a path to a PEM bundle.
    ///
    /// **This ADDS to the trust store, it does not replace it** — the client trusts the
    /// operating-system store (reqwest 0.13 uses `rustls-platform-verifier`; there is no
    /// compiled-in Mozilla floor, and on Linux an empty system store with no bundle is a client
    /// build error), AND every certificate here. The
    /// opposite of the sibling `iam.tls.ca_cert_path`, which PINS; hence the `extra_` prefix.
    /// For a self-hosted vLLM/LiteLLM upstream behind a corporate CA (SMA-558).
    ///
    /// **ROOTS ONLY** — every certificate here becomes an unconstrained trust anchor for every
    /// request this client makes, to any host it reaches. The anchors go onto the OpenAI egress
    /// client's own `reqwest::ClientBuilder`, NOT the whole process: the IAM `tonic` link builds
    /// its own TLS config and never consults them (SMA-570). Read once at boot; an unreadable,
    /// malformed or certificate-free bundle is a hard boot failure. Mirrors
    /// `paigasus-iam`'s `authn.extra_ca_bundle_path`.
    #[serde(default)]
    pub extra_ca_bundle_path: Option<String>,
}

/// `GET /metrics` (SMA-446 Unit 2): whether the Prometheus recorder is installed at all, and
/// where the route is served. `addr: None` (the default) merges `/metrics` onto `http_addr` (the
/// same port as the chat/health routes); `Some(addr)` serves it on its OWN listener instead — the
/// RECOMMENDED posture for a public gateway, so operational metrics never share a port with
/// public traffic. `enabled = false` skips installing the recorder entirely (no route, no global
/// `metrics`-facade recorder in this process). No `PartialEq`/`Eq` derive — mirrors the file's
/// existing sub-configs (`IamClientConfig` is the only one that derives them, for its own
/// equality-based tests).
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MetricsConfig {
    pub enabled: bool,
    pub addr: Option<SocketAddr>,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        MetricsConfig { enabled: true, addr: None }
    }
}

/// `[limits]` (SMA-677 § 4.1). An unset key means no limit on that dimension (D11). A misspelt
/// key fails extraction (`deny_unknown_fields`): a typo must not switch a budget off silently.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct LimitsConfig {
    /// D17: `memory` (one replica, the default) or `redis` (shared across replicas).
    pub backend: LimitsBackend,
    /// D17: required for `backend = "redis"`, rejected for `memory`. It can carry a password, so
    /// it is skipped on `Serialize`, and `SecretString` redacts it on `Debug` — the same pattern
    /// as `upstream.openai.api_key` (Q15). Set it through `GATEWAY_LIMITS__REDIS_URL`.
    #[serde(skip_serializing)]
    pub redis_url: Option<SecretString>,
    pub principal_requests_per_minute: Option<u64>,
    pub org_requests_per_minute: Option<u64>,
    pub tokens_per_period: Option<u64>,
    /// D6: `daily`, `weekly` (ISO week) or `monthly` (default), in UTC.
    pub budget_period: BudgetPeriod,
    /// D12: `[[limits.org]]`, TOML only (figment's environment provider cannot express an array
    /// of tables). With `backend = "redis"`, every replica must carry the same list.
    pub org: Vec<OrgLimitsConfig>,
}

/// One `[[limits.org]]` entry (D12).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OrgLimitsConfig {
    /// The org UUID in the 36-character form (the `domain::parse_org_uuid` rule).
    pub id: String,
    #[serde(default)]
    pub org_requests_per_minute: Option<u64>,
    #[serde(default)]
    pub tokens_per_period: Option<u64>,
    /// D11: no org rate and no budget; the principal rate still applies.
    #[serde(default)]
    pub exempt: bool,
}

/// D17: which limit store backs `[limits]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LimitsBackend {
    #[default]
    Memory,
    Redis,
}

/// D10: which store backs `[tool_history]`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryBackend {
    #[default]
    Memory,
    Redis,
}

/// `[tool_history]` (SMA-753 § D10). A misspelt key fails extraction (`deny_unknown_fields`).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolHistoryConfig {
    /// `memory` (one replica, the default) or `redis` (shared across replicas).
    pub backend: HistoryBackend,
    /// Required for `backend = "redis"`, rejected for `memory`. It can carry a password, so it is
    /// skipped on `Serialize` (the `limits.redis_url` pattern). Set it through `GATEWAY_TOOL_HISTORY__REDIS_URL`.
    #[serde(skip_serializing)]
    pub redis_url: Option<SecretString>,
    /// The time limit of one store call: `"<n>ms"` or `"<n>s"`. It must be shorter than
    /// `tool_interception.decide_timeout_secs`.
    pub deadline: String,
    /// The key of the field digests (SMA-753 P3), at least 32 bytes. With no key the history
    /// stores no string, bool or decimal value. Set it through `GATEWAY_TOOL_HISTORY__DIGEST_KEY`.
    #[serde(skip_serializing)]
    pub digest_key: Option<SecretString>,
    /// The key that the current key replaced. A literal in a policy gets a digest under both keys.
    /// It needs `digest_key`. Set it through `GATEWAY_TOOL_HISTORY__PREVIOUS_DIGEST_KEY`.
    #[serde(skip_serializing)]
    pub previous_digest_key: Option<SecretString>,
}

impl Default for ToolHistoryConfig {
    fn default() -> Self {
        ToolHistoryConfig {
            backend: HistoryBackend::default(),
            redis_url: None,
            deadline: "1s".to_string(),
            digest_key: None,
            previous_digest_key: None,
        }
    }
}

/// Parse `"<n>ms"` or `"<n>s"`, where n is a positive integer.
///
/// # Errors
/// A message that states the accepted form.
pub fn parse_deadline(s: &str) -> Result<Duration, String> {
    let bad = || format!("{s:?} is not a duration: use \"<n>ms\" or \"<n>s\" with n a positive integer");
    let (digits, to_duration): (&str, fn(u64) -> Duration) = if let Some(d) = s.strip_suffix("ms") {
        (d, Duration::from_millis)
    } else if let Some(d) = s.strip_suffix('s') {
        (d, Duration::from_secs)
    } else {
        return Err(bad());
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(bad());
    }
    match digits.parse::<u64>() {
        Ok(n) if n > 0 => Ok(to_duration(n)),
        _ => Err(bad()),
    }
}

impl ToolHistoryConfig {
    /// # Errors
    /// A message that names the key of the first invalid value.
    pub fn validate(&self, ti: &ToolInterceptionConfig) -> Result<(), String> {
        let deadline = parse_deadline(&self.deadline).map_err(|e| format!("tool_history.deadline: {e}"))?;
        if deadline >= Duration::from_secs(ti.decide_timeout_secs) {
            return Err(format!(
                "tool_history.deadline ({}) must be less than tool_interception.decide_timeout_secs ({})",
                self.deadline, ti.decide_timeout_secs
            ));
        }
        crate::adapters::history::HmacHistoryDigester::from_config(self)?;
        match self.backend {
            HistoryBackend::Redis if is_blank(&self.redis_url) => Err("tool_history.backend = \"redis\" requires tool_history.redis_url (set GATEWAY_TOOL_HISTORY__REDIS_URL)".to_string()),
            HistoryBackend::Memory if self.redis_url.is_some() => {
                Err("tool_history.redis_url is set but tool_history.backend is \"memory\": the history would not be shared; set tool_history.backend = \"redis\" or unset the URL".to_string())
            }
            _ => Ok(()),
        }
    }

    /// Only after `validate`. An invalid value reads as the default of one second.
    #[must_use]
    pub fn deadline(&self) -> Duration {
        parse_deadline(&self.deadline).unwrap_or(Duration::from_secs(1))
    }
}

/// `[dpop]` (SMA-700 § 4.10). With `enabled`, the gateway accepts `Authorization: DPoP <token>`
/// with one `DPoP` proof header, forwards the proof to IAM, and sends a `WWW-Authenticate: DPoP`
/// challenge on a 401. A misspelt key fails extraction.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct GatewayDpopConfig {
    pub enabled: bool,
}

/// `[tool_interception]` (SMA-749 D26). Off by default: with `enabled = false` the chat relay is
/// exactly the relay without interception (D27). A misspelt key fails extraction, so a typo can
/// never leave a security control silently off.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ToolInterceptionConfig {
    pub enabled: bool,
    /// D8: one SSE record larger than this is a failure, never skipped.
    pub max_record_bytes: usize,
    /// D13: the queued bytes of one stream, across every hold. Also the cap on one non-stream
    /// body (spec 5.2).
    pub max_held_bytes: usize,
    /// D14: from a hold's first frame to its completion. The default equals the stream idle
    /// timeout: a file-writing call with 40 KB of arguments takes about 200 s.
    pub max_hold_secs: u64,
    /// D14: how long the decider may take for one hold.
    pub decide_timeout_secs: u64,
    /// D15: the keep-alive comment interval while a call is held.
    pub keepalive_secs: u64,
    /// D16: `block` (default) or `allow` (observe mode).
    pub on_failure: OnFailureConfig,
}

/// `[responses]` (SMA-769 D8, D15). Off by default: with `enabled = false`, `/v1/responses`
/// returns 404, and the key is the rollback with no deploy. A misspelt key fails extraction.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ResponsesConfig {
    pub enabled: bool,
    /// D15: the cap on one request body of the route.
    pub max_request_bytes: usize,
    /// D15: one SSE record larger than this is a failure, never skipped.
    pub max_record_bytes: usize,
    /// D15: the queued bytes of one stream. Also the cap on one non-stream body.
    pub max_held_bytes: usize,
}

impl Default for ResponsesConfig {
    fn default() -> Self {
        ResponsesConfig {
            enabled: false,
            max_request_bytes: 16_777_216,
            max_record_bytes: 16_777_216,
            max_held_bytes: 33_554_432,
        }
    }
}

impl ResponsesConfig {
    /// Every value is at least 1, and one record must fit in the held bytes. Each message names
    /// the key. Checked even when off, so turning the route on later never meets a bad config.
    pub fn validate(&self) -> Result<(), String> {
        for (key, v) in [
            ("responses.max_request_bytes", self.max_request_bytes),
            ("responses.max_record_bytes", self.max_record_bytes),
            ("responses.max_held_bytes", self.max_held_bytes),
        ] {
            if v == 0 {
                return Err(format!("{key} must be at least 1"));
            }
        }
        if self.max_record_bytes > self.max_held_bytes {
            return Err(format!(
                "responses.max_record_bytes ({}) must not exceed responses.max_held_bytes ({})",
                self.max_record_bytes, self.max_held_bytes
            ));
        }
        Ok(())
    }
}

/// D16.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OnFailureConfig {
    #[default]
    Block,
    Allow,
}

impl Default for ToolInterceptionConfig {
    fn default() -> Self {
        let l = paigasus_gateway_core::Limits::defaults();
        ToolInterceptionConfig {
            enabled: false,
            max_record_bytes: l.max_record_bytes,
            max_held_bytes: l.max_held_bytes,
            max_hold_secs: l.max_hold.as_secs(),
            decide_timeout_secs: l.decide_timeout.as_secs(),
            keepalive_secs: l.keepalive.as_secs(),
            on_failure: OnFailureConfig::Block,
        }
    }
}

/// The upper bound of every `tool_interception.*_secs` value: one day.
const MAX_TOOL_INTERCEPTION_SECS: u64 = 86_400;

impl ToolInterceptionConfig {
    /// Every rule of spec 5.2. Each message names the key. Checked even when off, so turning it
    /// on later never meets a config that was always invalid.
    pub fn validate(&self) -> Result<(), String> {
        for (key, v) in [
            ("tool_interception.max_record_bytes", self.max_record_bytes as u64),
            ("tool_interception.max_held_bytes", self.max_held_bytes as u64),
            ("tool_interception.max_hold_secs", self.max_hold_secs),
            ("tool_interception.decide_timeout_secs", self.decide_timeout_secs),
            ("tool_interception.keepalive_secs", self.keepalive_secs),
        ] {
            if v == 0 {
                return Err(format!("{key} must be at least 1"));
            }
        }
        // `Instant + Duration` panics on overflow, and the interceptor adds each of these to a clock reading.
        for (key, v) in [
            ("tool_interception.max_hold_secs", self.max_hold_secs),
            ("tool_interception.decide_timeout_secs", self.decide_timeout_secs),
            ("tool_interception.keepalive_secs", self.keepalive_secs),
        ] {
            if v > MAX_TOOL_INTERCEPTION_SECS {
                return Err(format!("{key} must be at most {MAX_TOOL_INTERCEPTION_SECS} (one day); got {v}"));
            }
        }
        if self.max_record_bytes > self.max_held_bytes {
            return Err(format!(
                "tool_interception.max_record_bytes ({}) must not exceed tool_interception.max_held_bytes ({})",
                self.max_record_bytes, self.max_held_bytes
            ));
        }
        if self.keepalive_secs >= self.max_hold_secs {
            return Err(format!(
                "tool_interception.keepalive_secs ({}) must be less than tool_interception.max_hold_secs ({})",
                self.keepalive_secs, self.max_hold_secs
            ));
        }
        Ok(())
    }

    #[must_use]
    pub fn limits(&self) -> paigasus_gateway_core::Limits {
        paigasus_gateway_core::Limits {
            max_record_bytes: self.max_record_bytes,
            max_held_bytes: self.max_held_bytes,
            max_hold: std::time::Duration::from_secs(self.max_hold_secs),
            decide_timeout: std::time::Duration::from_secs(self.decide_timeout_secs),
            keepalive: std::time::Duration::from_secs(self.keepalive_secs),
            on_failure: match self.on_failure {
                OnFailureConfig::Block => paigasus_gateway_core::OnFailure::Block,
                OnFailureConfig::Allow => paigasus_gateway_core::OnFailure::Allow,
            },
        }
    }
}

/// D11/D23: `0` refuses every request (never what an operator means), and a value above the cap
/// would break the D23 arithmetic bound.
fn check_limit(name: &str, value: Option<u64>, cap: u64) -> Result<(), String> {
    match value {
        Some(0) => Err(format!("{name} must be at least 1 (unset it for no limit; 0 would refuse every request)")),
        Some(v) if v > cap => Err(format!("{name} must be at most {cap}; got {v}")),
        _ => Ok(()),
    }
}

impl LimitsConfig {
    /// Every `[limits]` rule of spec § 4.1. Each message names the key.
    pub fn validate(&self) -> Result<(), String> {
        check_limit("limits.principal_requests_per_minute", self.principal_requests_per_minute, MAX_REQUESTS_PER_MINUTE)?;
        check_limit("limits.org_requests_per_minute", self.org_requests_per_minute, MAX_REQUESTS_PER_MINUTE)?;
        check_limit("limits.tokens_per_period", self.tokens_per_period, MAX_TOKENS_PER_PERIOD)?;

        let mut seen = HashSet::new();
        for (i, entry) in self.org.iter().enumerate() {
            let Some(id) = parse_org_uuid(&entry.id) else {
                return Err(format!("limits.org[{i}].id must be one organization UUID in the 36-character form; got {:?}", entry.id));
            };
            if !seen.insert(id) {
                return Err(format!("limits.org[{i}].id {:?} names an org that an earlier [[limits.org]] entry already lists", entry.id));
            }
            check_limit(&format!("limits.org[{i}].org_requests_per_minute"), entry.org_requests_per_minute, MAX_REQUESTS_PER_MINUTE)?;
            check_limit(&format!("limits.org[{i}].tokens_per_period"), entry.tokens_per_period, MAX_TOKENS_PER_PERIOD)?;
            if entry.exempt && (entry.org_requests_per_minute.is_some() || entry.tokens_per_period.is_some()) {
                return Err(format!("limits.org[{i}] sets exempt = true together with a limit; an exempt org has no org rate and no budget"));
            }
        }

        let url_set = self.redis_url.as_ref().is_some_and(|url| !url.expose_secret().trim().is_empty());
        match self.backend {
            LimitsBackend::Redis if !url_set => Err("limits.backend = \"redis\" requires limits.redis_url (set GATEWAY_LIMITS__REDIS_URL)".to_string()),
            LimitsBackend::Memory if self.redis_url.is_some() => {
                Err("limits.redis_url is set but limits.backend is \"memory\": the counts would not be shared; set limits.backend = \"redis\" or unset the URL".to_string())
            }
            _ => Ok(()),
        }
    }

    /// The domain rules. Call after `validate`: an entry with a bad id is skipped, and a `0` value
    /// reads as "no limit" rather than panicking.
    pub fn rules(&self) -> LimitRules {
        LimitRules {
            principal_requests_per_minute: self.principal_requests_per_minute.and_then(NonZeroU64::new),
            org_requests_per_minute: self.org_requests_per_minute.and_then(NonZeroU64::new),
            tokens_per_period: self.tokens_per_period.and_then(NonZeroU64::new),
            budget_period: self.budget_period,
            overrides: self
                .org
                .iter()
                .filter_map(|entry| {
                    parse_org_uuid(&entry.id).map(|id| {
                        (
                            id,
                            OrgOverride {
                                org_requests_per_minute: entry.org_requests_per_minute.and_then(NonZeroU64::new),
                                tokens_per_period: entry.tokens_per_period.and_then(NonZeroU64::new),
                                exempt: entry.exempt,
                            },
                        )
                    })
                })
                .collect(),
        }
    }
}

// Only the fields that HAVE a default — every `GatewayConfig` field has one (unlike iam's
// `database_url`/`authn.issuers`, nothing here is a hard-required, default-less value).
#[derive(Serialize)]
struct Defaults {
    http_addr: SocketAddr,
    connect_timeout_secs: u64,
    first_byte_timeout_secs: u64,
    stream_idle_timeout_secs: u64,
    max_request_bytes: usize,
    iam: IamClientConfig,
    upstream: UpstreamDefaults,
    log_level: String,
    metrics: MetricsConfig,
    stream_enabled: bool,
}

#[derive(Serialize, Default)]
struct UpstreamDefaults {
    openai: OpenAiDefaults,
}

// Mirrors `OpenAiConfig` minus `extra_ca_bundle_path` — the second such omission in this
// codebase (see iam's `AuthnDefaults`, the first): it is an `Option` carrying
// `#[serde(default)]`, which already resolves to `None` without a defaults-layer entry; adding
// one would serialize a null into the layer for no gain. Also EXCEPT `api_key`'s TYPE: this
// struct only ever feeds figment's default LAYER (`Serialized::defaults`), never gets
// logged/dumped itself, so a plain (unredacted) `String` is fine here — figment still
// deserializes the eventual merged value into `OpenAiConfig.api_key: SecretString` regardless of
// which layer (default/toml/env) supplied it. The default is the empty string, deliberately
// INVALID (an empty key can never authenticate to OpenAI) — caught by `GatewayConfig::validate`
// rather than at figment extraction, mirroring iam's `ApiKeyDefaults`/`RawPepper` pattern exactly.
#[derive(Serialize)]
struct OpenAiDefaults {
    base_url: String,
    api_key: String,
}

impl Default for Defaults {
    fn default() -> Self {
        Defaults {
            http_addr: "0.0.0.0:8088".parse().expect("valid addr"),
            connect_timeout_secs: 10,
            first_byte_timeout_secs: 30,
            stream_idle_timeout_secs: 300,
            max_request_bytes: 1_048_576, // 1 MiB
            iam: IamClientConfig::default(),
            upstream: UpstreamDefaults::default(),
            log_level: "info".to_string(),
            metrics: MetricsConfig::default(),
            stream_enabled: true,
        }
    }
}

impl Default for OpenAiDefaults {
    fn default() -> Self {
        OpenAiDefaults {
            base_url: "https://api.openai.com".to_string(),
            api_key: String::new(),
        }
    }
}

/// True when `grpc_addr`'s host is a loopback address (`127.0.0.0/8`, `localhost`, or an IPv6
/// loopback such as `::1`).
///
/// The host comes from the URI authority, read with [`axum::http::Uri`]. A string split is not
/// safe: it reads a loopback address from the path (`http://192.0.2.10/x/http://127.0.0.1`) or
/// from the userinfo (`http://127.0.0.1@192.0.2.10/`), and the channel connects to neither. A
/// value that is not a URI is not loopback (fail closed).
///
/// The host is then classified by parsing it as a real IP and asking
/// [`Ipv4Addr::is_loopback`]/[`Ipv6Addr::is_loopback`] — NOT a string prefix match. A prefix
/// match like `host.starts_with("127.")` is trivially bypassable: `127.evil.com` and
/// `127.0.0.1.attacker.com` both pass it yet are DNS-resolvable to arbitrary addresses, which
/// would let an operator disable TLS on a link that then carries raw API keys off-loopback
/// (D8). `localhost` is matched by EXACT equality for the same reason (`localhost.evil.com`
/// must NOT match).
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

impl GatewayConfig {
    // `.split("__")` maps a DOUBLE-underscore in a `GATEWAY_*` env var to struct nesting, so
    // the OpenAI key can be injected without a config file: `GATEWAY_UPSTREAM__OPENAI__API_KEY`
    // -> `upstream.openai.api_key` (mirrors `paigasus-iam::config::IamConfig::figment`'s exact
    // rationale/split choice).
    #[must_use]
    pub fn figment() -> Figment {
        Figment::from(Serialized::defaults(Defaults::default()))
            .merge(Toml::file("gateway.toml"))
            .merge(Env::prefixed("GATEWAY_").split("__"))
    }

    // `figment::Error` is a large enum (~208B); allow the size lint narrowly rather than
    // reshape the public signature the brief specifies (`main` calls this directly).
    #[allow(clippy::result_large_err)]
    pub fn load() -> Result<Self, FigmentError> {
        Self::figment().extract()
    }

    /// The detector scan budget when `tool_policy.detectors.max_scan_bytes` is unset: the largest
    /// request cap of the routes in use. `responses.max_request_bytes` always counts, so the
    /// default does not drop when `[responses]` is off. `messages.max_request_bytes` counts only
    /// when `[messages]` is on.
    #[must_use]
    pub fn default_detector_scan_bytes(&self) -> usize {
        let mut n = self.max_request_bytes.max(self.responses.max_request_bytes);
        if self.messages.enabled {
            n = n.max(self.messages.max_request_bytes);
        }
        n
    }

    /// Boot-time validation beyond what serde/figment structurally enforce: every
    /// `*_timeout_secs` is non-zero, `max_request_bytes` is non-zero, the OpenAI API key is
    /// non-empty (an empty key is a misconfiguration — reject at boot rather than fail every
    /// request at runtime), and — D8 — an `iam.tls = loopback_insecure` opt-out is only valid
    /// when `iam.grpc_addr` is a loopback host (the introspect link otherwise carries raw
    /// caller-presented API keys in the clear).
    pub fn validate(&self) -> Result<(), String> {
        for (name, secs) in [
            ("connect_timeout_secs", self.connect_timeout_secs),
            ("first_byte_timeout_secs", self.first_byte_timeout_secs),
            ("stream_idle_timeout_secs", self.stream_idle_timeout_secs),
        ] {
            if secs == 0 {
                return Err(format!("{name} must be at least 1 (0 disables the corresponding timeout)"));
            }
        }

        if self.max_request_bytes == 0 {
            return Err("max_request_bytes must be at least 1".to_string());
        }

        if self.upstream.openai.api_key.expose_secret().is_empty() {
            return Err("upstream.openai.api_key must not be empty (an empty key is a misconfiguration)".to_string());
        }

        // Empty and padded are the same operator mistake, and both would otherwise reach
        // `std::fs::read`. Mirrors iam's rule on `authn.extra_ca_bundle_path`.
        if let Some(path) = self.upstream.openai.extra_ca_bundle_path.as_deref() {
            if path.trim().is_empty() {
                return Err("upstream.openai.extra_ca_bundle_path must not be empty (omit the key entirely to use the default trust store)".to_string());
            }
            if path != path.trim() {
                return Err(format!("upstream.openai.extra_ca_bundle_path has leading/trailing whitespace, which is never valid config: {path:?}"));
            }
        }

        if matches!(self.iam.tls, IamTlsConfig::LoopbackInsecure) && !is_loopback_host(&self.iam.grpc_addr) {
            return Err(format!(
                "iam.tls = \"loopback_insecure\" requires iam.grpc_addr to be a loopback host (127.0.0.0/8, localhost, or ::1) — the introspect link otherwise carries raw API keys in the clear (D8); got {:?}",
                self.iam.grpc_addr
            ));
        }

        // A same-PORT check, not exact-address-equality: an unequal-but-same-port pair (e.g.
        // `0.0.0.0:8088` metrics vs `127.0.0.1:8088` http) passes exact equality yet both
        // listeners still try to claim the same port and fail at bind time with `AddrInUse`.
        if let Some(addr) = self.metrics.addr
            && addr.port() == self.http_addr.port()
        {
            return Err("metrics.addr must use a different port than http_addr".to_string());
        }

        if let Some(limits) = &self.limits {
            limits.validate()?;
        }
        self.tool_interception.validate()?;
        self.responses.validate()?;
        self.messages.validate()?;
        if let Some(anthropic) = &self.upstream.anthropic {
            anthropic.validate()?;
        }
        if self.messages.enabled && self.upstream.anthropic.is_none() {
            return Err("messages.enabled = true requires the [upstream.anthropic] table (set GATEWAY_UPSTREAM__ANTHROPIC__API_KEY)".to_string());
        }
        self.tool_policy.validate(&self.tool_interception)?;
        // SMA-856: the provider table is validated even when the detectors are off.
        self.tool_policy
            .detectors
            .validate_webhooks(self.tool_policy.detectors.max_scan_bytes.unwrap_or(self.default_detector_scan_bytes()))?;
        if let Some(tool_history) = &self.tool_history {
            tool_history.validate(&self.tool_interception)?;
        }
        self.decision_log.validate(&self.tool_policy)?;
        // SMA-775: the cross-table rule that `ToolPolicyConfig::validate` cannot see (it has no `iam`).
        // A blank or padded path is refused at all times (as a blank `tool_policy.nats.creds_file` is).
        // Only "the file is required" waits for `enabled`.
        match self.iam.service_credential_file.as_deref() {
            Some(p) if p.trim().is_empty() => return Err("iam.service_credential_file must not be empty (omit the key to use none)".to_string()),
            Some(p) if p != p.trim() => return Err("iam.service_credential_file has leading or trailing whitespace, which is never valid config".to_string()),
            _ => {}
        }
        if self.tool_policy.enabled && matches!(self.tool_policy.source, Some(PolicySourceConfig::Iam {})) && self.iam.service_credential_file.is_none() {
            return Err("iam.service_credential_file is required when tool_policy.source kind = \"iam\"".to_string());
        }
        Ok(())
    }
}

/// `[tool_policy]` (SMA-752 § 6.4). Off by default: with `enabled = false` the gateway behaves as
/// before. `GatewayConfig.tool_policy` is not in the `Defaults` layer. A misspelt key fails
/// extraction, so a typo can never leave a security control silently off.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct ToolPolicyConfig {
    pub enabled: bool,
    /// D8: the mode of an org that has no `[[tool_policy.org]]` entry.
    pub default_mode: TenantModeConfig,
    /// D9, D34: the global value. Required when `enabled`.
    pub on_policy_unavailable: Option<OnUnavailableConfig>,
    pub refresh_secs: u64,
    pub max_stale_secs: u64,
    pub idle_evict_secs: u64,
    /// D31: the time limit of one policy load. It must be shorter than `tool_interception.decide_timeout_secs`.
    pub load_timeout_ms: u64,
    pub source: Option<PolicySourceConfig>,
    pub nats: Option<ToolPolicyNatsConfig>,
    /// TOML only (figment's environment provider cannot express an array of tables).
    pub org: Vec<OrgToolPolicyConfig>,
    /// SMA-855 § 6.10: the tool-call detectors. The table is optional.
    pub detectors: DetectorsConfig,
}

impl Default for ToolPolicyConfig {
    fn default() -> Self {
        ToolPolicyConfig {
            enabled: false,
            default_mode: TenantModeConfig::Off,
            on_policy_unavailable: None,
            refresh_secs: 30,
            max_stale_secs: 300,
            idle_evict_secs: 3600,
            load_timeout_ms: 1000,
            source: None,
            nats: None,
            org: Vec::new(),
            detectors: DetectorsConfig::default(),
        }
    }
}

/// D35: the time that the pipeline deadline leaves for the work it does not cover (the built-in
/// run, the Cedar `decide` of each call, the kernel host pass and the scheduler delay). The value
/// is provisional. The builder sets it from a measured p99. This is the only place that holds it.
pub const DETECTOR_MARGIN_MS: u64 = 250;

/// The smallest `tool_policy.detectors.max_scan_bytes` (§ 6.10).
const MIN_DETECTOR_SCAN_BYTES: usize = 65_536;

/// `[tool_policy.detectors]` (SMA-855 § 6.10, D23). The table is optional. A misspelt key fails
/// extraction.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DetectorsConfig {
    /// `false` builds no pipeline and runs no screen. That is fail open for a policy that reads
    /// detector output, so it is a switch for an incident.
    pub enabled: bool,
    /// An upper bound for the webhook and judge stages of one run.
    pub timeout_ms: u64,
    /// The text that one run scans. `None` (the default) uses `GatewayConfig::default_detector_scan_bytes`:
    /// the largest request cap of any route, so a request at the default caps is scanned in full
    /// (S12). There is no upper bound (R10).
    pub max_scan_bytes: Option<usize>,
    /// SMA-856: the webhook providers, one `[[tool_policy.detectors.webhook]]` table each.
    #[serde(default)]
    pub webhook: Vec<WebhookConfig>,
}

impl Default for DetectorsConfig {
    fn default() -> Self {
        DetectorsConfig {
            enabled: true,
            timeout_ms: 500,
            max_scan_bytes: None,
            webhook: Vec::new(),
        }
    }
}

/// The orgs of a webhook provider as the config spells them: `"*"` (a shared provider) or a list of
/// org UUIDs. `All` is valid only as `"*"`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum WebhookOrgs {
    All(String),
    List(Vec<String>),
}

/// The severity of the finding that a failed provider gives (D16).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FailureSeverityConfig {
    Info,
    Low,
    Medium,
    High,
}

impl From<FailureSeverityConfig> for Severity {
    fn from(s: FailureSeverityConfig) -> Self {
        match s {
            FailureSeverityConfig::Info => Severity::Info,
            FailureSeverityConfig::Low => Severity::Low,
            FailureSeverityConfig::Medium => Severity::Medium,
            FailureSeverityConfig::High => Severity::High,
        }
    }
}

fn default_webhook_timeout_ms() -> u64 {
    300
}
fn default_webhook_max_request_bytes() -> usize {
    1_048_576
}
fn default_webhook_max_in_flight() -> usize {
    64
}
fn default_failure_severity() -> FailureSeverityConfig {
    FailureSeverityConfig::High
}

/// One `[[tool_policy.detectors.webhook]]` provider (SMA-856 § 6.2). `Serialize` and `Debug` are
/// written by hand: they print the name, the URL host, the count of orgs and the limits. They never
/// print the URL path, a secret env name, a secret or the CA path (A4).
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebhookConfig {
    /// `[a-z0-9_]{1,64}`, unique, not reserved.
    pub name: String,
    /// `https`, no userinfo, no query. `http` only for a loopback host.
    pub url: String,
    pub orgs: WebhookOrgs,
    /// The env var that holds the secret (the exact bytes are the key). Not `GATEWAY_*`.
    pub secret_env: String,
    /// The env var that holds the previous secret, for a rotation.
    #[serde(default)]
    pub previous_secret_env: Option<String>,
    #[serde(default = "default_webhook_timeout_ms")]
    pub timeout_ms: u64,
    #[serde(default = "default_failure_severity")]
    pub failure_severity: FailureSeverityConfig,
    /// The finding kinds that the provider may return.
    pub kinds: Vec<String>,
    #[serde(default = "default_webhook_max_request_bytes")]
    pub max_request_bytes: usize,
    #[serde(default = "default_webhook_max_in_flight")]
    pub max_in_flight: usize,
    /// `true` sends no request; every call of an opted-in tenant gets a `retired` failure (D27).
    /// Keep the secret env var set until you remove the entry. A retired provider still needs its
    /// secret at boot (D20).
    #[serde(default)]
    pub retired: bool,
    /// This ADDS to the trust store, it does not replace it. The client trusts the operating-system
    /// store and every certificate here. Roots only. Read once at boot. An unreadable, malformed or
    /// empty bundle is a hard boot failure.
    #[serde(default)]
    pub extra_ca_bundle_path: Option<String>,
}

impl WebhookConfig {
    fn host(&self) -> String {
        reqwest::Url::parse(&self.url).ok().and_then(|u| u.host_str().map(str::to_owned)).unwrap_or_default()
    }

    /// The orgs of the provider. Meaningful after `validate`.
    #[must_use]
    pub fn org_scope(&self) -> OrgScope {
        match &self.orgs {
            WebhookOrgs::All(_) => OrgScope::All,
            WebhookOrgs::List(list) => OrgScope::Only(list.iter().filter_map(|s| parse_org_uuid(s)).collect()),
        }
    }

    /// The parsed URL.
    ///
    /// # Panics
    /// Panics when the URL does not parse, which `validate` refuses.
    #[must_use]
    pub fn parsed_url(&self) -> reqwest::Url {
        reqwest::Url::parse(&self.url).expect("validate accepted the URL")
    }

    /// The declared kinds as a set.
    #[must_use]
    pub fn kind_set(&self) -> BTreeSet<String> {
        self.kinds.iter().cloned().collect()
    }
}

impl fmt::Debug for WebhookConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let orgs = match &self.orgs {
            WebhookOrgs::All(_) => "*".to_string(),
            WebhookOrgs::List(l) => l.len().to_string(),
        };
        f.debug_struct("WebhookConfig")
            .field("name", &self.name)
            .field("host", &self.host())
            .field("orgs", &orgs)
            .field("timeout_ms", &self.timeout_ms)
            .field("max_request_bytes", &self.max_request_bytes)
            .field("max_in_flight", &self.max_in_flight)
            .field("retired", &self.retired)
            .finish_non_exhaustive()
    }
}

impl Serialize for WebhookConfig {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        #[derive(Serialize)]
        #[serde(untagged)]
        enum OrgsDump {
            All(&'static str),
            Count(usize),
        }
        let orgs = match &self.orgs {
            WebhookOrgs::All(_) => OrgsDump::All("*"),
            WebhookOrgs::List(l) => OrgsDump::Count(l.len()),
        };
        let mut st = serializer.serialize_struct("WebhookConfig", 7)?;
        st.serialize_field("name", &self.name)?;
        st.serialize_field("host", &self.host())?;
        st.serialize_field("orgs", &orgs)?;
        st.serialize_field("timeout_ms", &self.timeout_ms)?;
        st.serialize_field("max_request_bytes", &self.max_request_bytes)?;
        st.serialize_field("max_in_flight", &self.max_in_flight)?;
        st.serialize_field("retired", &self.retired)?;
        st.end()
    }
}

/// The most providers (D18).
const MAX_WEBHOOK_PROVIDERS: usize = 8;
/// The shortest webhook secret in bytes (D20).
const MIN_WEBHOOK_SECRET_BYTES: usize = 32;

fn is_snake_ident(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// D20: `[A-Z][A-Z0-9_]{0,127}`, not starting with `GATEWAY_`.
fn is_secret_env_name(s: &str) -> bool {
    let mut bytes = s.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_uppercase()) && s.len() <= 128 && bytes.all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_') && !s.starts_with("GATEWAY_")
}

impl DetectorsConfig {
    /// SMA-856 § 6.2: every rule of the provider table. Each message names the provider and the
    /// field. `max_scan_bytes` is the effective scan budget.
    pub fn validate_webhooks(&self, max_scan_bytes: usize) -> Result<(), String> {
        const P: &str = "tool_policy.detectors";
        if self.webhook.len() > MAX_WEBHOOK_PROVIDERS {
            return Err(format!("{P}.webhook has {} providers, at most {MAX_WEBHOOK_PROVIDERS} are allowed", self.webhook.len()));
        }
        let timeout_max = self.timeout_ms.saturating_sub(50);
        let mut names = HashSet::new();
        let mut urls = HashSet::new();
        for (i, w) in self.webhook.iter().enumerate() {
            if !is_snake_ident(&w.name, 64) {
                return Err(format!("{P}.webhook[{i}].name must match [a-z0-9_]{{1,64}}; got {:?}", w.name));
            }
            if BUILT_IN_NAMES.contains(&w.name.as_str()) || ["judge", "webhook", "other"].contains(&w.name.as_str()) {
                return Err(format!("{P}.webhook[{i}].name {:?} is a reserved detector name", w.name));
            }
            if !names.insert(w.name.as_str()) {
                return Err(format!("{P}.webhook[{i}].name {:?} is already used by an earlier provider", w.name));
            }
            let at = format!("{P}.webhook[{}]", w.name);
            match reqwest::Url::parse(&w.url) {
                Err(_) => return Err(format!("{at}.url must be an absolute URL")),
                Ok(u) => {
                    let scheme_ok = u.scheme() == "https" || (u.scheme() == "http" && is_loopback_host(u.as_str()));
                    if !scheme_ok || u.host_str().is_none() {
                        return Err(format!("{at}.url must use https (http is accepted only for a loopback host)"));
                    }
                    if !u.username().is_empty() || u.password().is_some() {
                        return Err(format!("{at}.url must not hold user information"));
                    }
                    if u.query().is_some() {
                        return Err(format!("{at}.url must not hold a query"));
                    }
                    if u.fragment().is_some() {
                        return Err(format!("{at}.url must not hold a fragment"));
                    }
                    if !urls.insert(u.as_str().to_owned()) {
                        return Err(format!("{at}.url is already used by an earlier provider"));
                    }
                }
            }
            match &w.orgs {
                WebhookOrgs::All(s) if s == "*" => {}
                WebhookOrgs::All(_) => return Err(format!("{at}.orgs must be \"*\" or a list of org UUIDs")),
                WebhookOrgs::List(l) if l.is_empty() => return Err(format!("{at}.orgs must not be empty")),
                WebhookOrgs::List(l) => {
                    if let Some(bad) = l.iter().find(|s| parse_org_uuid(s).is_none()) {
                        return Err(format!("{at}.orgs holds {bad:?}, which is not an org UUID in the 36-character form"));
                    }
                }
            }
            if !is_secret_env_name(&w.secret_env) {
                return Err(format!("{at}.secret_env must match [A-Z][A-Z0-9_]{{0,127}} and must not start with GATEWAY_"));
            }
            if w.previous_secret_env.as_deref().is_some_and(|n| !is_secret_env_name(n)) {
                return Err(format!("{at}.previous_secret_env must match [A-Z][A-Z0-9_]{{0,127}} and must not start with GATEWAY_"));
            }
            if w.timeout_ms == 0 || w.timeout_ms > timeout_max {
                return Err(format!("{at}.timeout_ms must be from 1 to {timeout_max} ({P}.timeout_ms minus 50); got {}", w.timeout_ms));
            }
            if w.kinds.is_empty() || w.kinds.len() > 16 {
                return Err(format!("{at}.kinds must hold 1 to 16 kinds; got {}", w.kinds.len()));
            }
            let mut seen = HashSet::new();
            for k in &w.kinds {
                if !is_snake_ident(k, 64) {
                    return Err(format!("{at}.kinds holds {k:?}, which does not match [a-z0-9_]{{1,64}}"));
                }
                if FINDING_KINDS.contains(&k.as_str()) {
                    return Err(format!("{at}.kinds holds {k:?}, which is a reserved finding kind"));
                }
                if !seen.insert(k.as_str()) {
                    return Err(format!("{at}.kinds holds {k:?} more than once"));
                }
            }
            if w.max_request_bytes == 0 || w.max_request_bytes > max_scan_bytes {
                return Err(format!("{at}.max_request_bytes must be from 1 to {max_scan_bytes} ({P}.max_scan_bytes); got {}", w.max_request_bytes));
            }
            if w.max_in_flight == 0 || w.max_in_flight > 1024 {
                return Err(format!("{at}.max_in_flight must be from 1 to 1024; got {}", w.max_in_flight));
            }
            if let Some(path) = w.extra_ca_bundle_path.as_deref() {
                if path.trim().is_empty() {
                    return Err(format!("{at}.extra_ca_bundle_path must not be empty (omit the key to use the default trust store)"));
                }
                if path != path.trim() {
                    return Err(format!("{at}.extra_ca_bundle_path has leading or trailing whitespace, which is never valid config"));
                }
            }
        }
        Ok(())
    }
}

/// Reads environment variables. Tests inject a map, so they need no `set_var`.
pub trait EnvReader {
    fn var(&self, name: &str) -> Option<String>;
}

/// The process environment.
pub struct ProcessEnv;

impl EnvReader for ProcessEnv {
    fn var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

impl EnvReader for HashMap<String, String> {
    fn var(&self, name: &str) -> Option<String> {
        self.get(name).cloned()
    }
}

/// The secrets of one provider. `Debug` prints no value.
pub struct ProviderSecrets {
    pub current: SecretString,
    pub previous: Option<SecretString>,
}

impl fmt::Debug for ProviderSecrets {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProviderSecrets").field("previous", &self.previous.is_some()).finish_non_exhaustive()
    }
}

/// D20: reads and checks the secret of each provider. An error names the provider, the field and the
/// env var, and never holds the value.
pub fn load_webhook_secrets(config: &GatewayConfig, env: &dyn EnvReader) -> Result<BTreeMap<String, ProviderSecrets>, String> {
    let read = |provider: &str, field: &str, var: &str| -> Result<SecretString, String> {
        let at = format!("tool_policy.detectors.webhook[{provider}].{field}");
        let Some(value) = env.var(var) else {
            return Err(format!("{at}: the environment variable {var} is not set (or is not valid UTF-8)"));
        };
        if value.len() < MIN_WEBHOOK_SECRET_BYTES {
            return Err(format!("{at}: the environment variable {var} must hold at least {MIN_WEBHOOK_SECRET_BYTES} bytes"));
        }
        if value != value.trim() {
            return Err(format!("{at}: the environment variable {var} has leading or trailing whitespace"));
        }
        Ok(SecretString::from(value))
    };
    let mut out = BTreeMap::new();
    for w in &config.tool_policy.detectors.webhook {
        let current = read(&w.name, "secret_env", &w.secret_env)?;
        let previous = w.previous_secret_env.as_deref().map(|v| read(&w.name, "previous_secret_env", v)).transpose()?;
        out.insert(w.name.clone(), ProviderSecrets { current, previous });
    }
    Ok(out)
}

/// What the gateway reads at run time from `[tool_policy.detectors]` (built by
/// `ToolPolicyConfig::detector_settings`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectorSettings {
    pub enabled: bool,
    pub timeout: Duration,
    pub max_scan_bytes: usize,
    /// `DETECTOR_MARGIN_MS` as a `Duration`.
    pub margin: Duration,
}

/// The tenant mode as the config spells it (D8). D33: there is no `floor_mode`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantModeConfig {
    #[default]
    Off,
    LogOnly,
    Enforce,
}

/// `on_policy_unavailable` as the config spells it (D9, D34).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OnUnavailableConfig {
    Open,
    Closed,
}

/// Where the policy text comes from.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
pub enum PolicySourceConfig {
    /// A directory of policy files.
    File { dir: String },
    /// SMA-775: the compiled sets and the tenant settings from paigasus-iam over gRPC. It needs
    /// `iam.service_credential_file`, and it refuses `[[tool_policy.org]]` entries (IAM holds the
    /// mode and the fail mode of each org).
    Iam {},
}

/// `[tool_policy.nats]`: the invalidation subscription.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ToolPolicyNatsConfig {
    /// The URL can hold credentials, so it is skipped on `Serialize`, and `SecretString` redacts it on `Debug` (the `limits.redis_url` pattern).
    #[serde(skip_serializing)]
    pub url: SecretString,
    pub creds_file: Option<String>,
    pub ca_file: Option<String>,
}

/// `[decision_log]` (SMA-755 § 5.12). Off by default: with `enabled = false` the gateway only writes the tracing line.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DecisionLogConfig {
    pub enabled: bool,
    /// The URL can hold credentials: skipped on `Serialize`, redacted on `Debug` (the `limits.redis_url` pattern).
    #[serde(skip_serializing)]
    pub redis_url: Option<SecretString>,
    pub key_prefix: String,
    pub stream_max_len: u64,
    pub queue_capacity: usize,
    /// The HMAC key of the argument digest, at least 32 bytes. Never serialized.
    #[serde(skip_serializing)]
    pub digest_key: Option<SecretString>,
    pub digest_key_id: String,
    pub nats: Option<DecisionLogNatsConfig>,
}

impl Default for DecisionLogConfig {
    fn default() -> Self {
        DecisionLogConfig {
            enabled: false,
            redis_url: None,
            key_prefix: "paigasus:gateway".to_string(),
            stream_max_len: 100_000,
            queue_capacity: 4096,
            digest_key: None,
            digest_key_id: String::new(),
            nats: None,
        }
    }
}

/// `[decision_log.nats]`: the relay's publisher connection. The key names match `[tool_policy.nats]` (P-21).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DecisionLogNatsConfig {
    #[serde(skip_serializing)]
    pub url: SecretString,
    pub creds_file: Option<String>,
    /// A PEM bundle that REPLACES the system trust store: async-nats loads the native system roots only when no CA
    /// file is set. Absent: the native system roots. The export CLI flag `--ca` behaves the same way.
    pub ca_file: Option<String>,
}

fn is_blank(secret: &Option<SecretString>) -> bool {
    secret.as_ref().is_none_or(|s| s.expose_secret().trim().is_empty())
}

impl DecisionLogConfig {
    /// # Errors
    /// A message that names the key of the first invalid value.
    pub fn validate(&self, tp: &ToolPolicyConfig) -> Result<(), String> {
        if self.key_prefix.trim().is_empty() {
            return Err("decision_log.key_prefix must not be empty".to_string());
        }
        if self.key_prefix != self.key_prefix.trim() {
            return Err(format!("decision_log.key_prefix has leading/trailing whitespace, which is never valid config: {:?}", self.key_prefix));
        }
        if self.stream_max_len == 0 {
            return Err("decision_log.stream_max_len must be at least 1".to_string());
        }
        if self.queue_capacity == 0 {
            return Err("decision_log.queue_capacity must be at least 1".to_string());
        }
        if let Some(nats) = &self.nats {
            if nats.url.expose_secret().trim().is_empty() {
                return Err("decision_log.nats.url must not be empty (omit the [decision_log.nats] table to run without it)".to_string());
            }
            for (key, v) in [("decision_log.nats.creds_file", &nats.creds_file), ("decision_log.nats.ca_file", &nats.ca_file)] {
                if v.as_deref().is_some_and(|p| p.trim().is_empty()) {
                    return Err(format!("{key} must not be empty (omit the key to use none)"));
                }
            }
        }
        if let Some(key) = &self.digest_key
            && let Err(e) = DigestKey::new(self.digest_key_id.clone(), SecretString::from(key.expose_secret().to_owned()))
        {
            return Err(format!("decision_log.digest_key: {e}"));
        }
        if !self.enabled {
            return Ok(());
        }
        if !tp.enabled {
            return Err("decision_log.enabled = true requires tool_policy.enabled = true: the log records tool-policy decisions".to_string());
        }
        if is_blank(&self.redis_url) {
            return Err("decision_log.redis_url is required when decision_log.enabled = true (set GATEWAY_DECISION_LOG__REDIS_URL)".to_string());
        }
        if is_blank(&self.digest_key) {
            return Err("decision_log.digest_key is required when decision_log.enabled = true (at least 32 bytes)".to_string());
        }
        if self.digest_key_id.trim().is_empty() {
            return Err("decision_log.digest_key_id is required when decision_log.enabled = true".to_string());
        }
        if self.nats.is_none() {
            return Err("decision_log.nats is required when decision_log.enabled = true: without it events wait in Redis until the backlog cap drops them".to_string());
        }
        Ok(())
    }

    /// The digest key, when a valid one is configured.
    #[must_use]
    pub fn digest_key(&self) -> Option<DigestKey> {
        let key = self.digest_key.as_ref()?;
        DigestKey::new(self.digest_key_id.clone(), SecretString::from(key.expose_secret().to_owned())).ok()
    }
}

/// One `[[tool_policy.org]]` entry (D7, D34).
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct OrgToolPolicyConfig {
    /// The org UUID in the 36-character form (the `domain::parse_org_uuid` rule).
    pub id: String,
    pub mode: TenantModeConfig,
    #[serde(default)]
    pub on_policy_unavailable: Option<OnUnavailableConfig>,
    /// SMA-856 D9: the webhook providers that this org opts in to. Used only when the policy source
    /// is not IAM (with IAM, the org settings in IAM hold the opt-in list).
    #[serde(default)]
    pub webhooks: Vec<String>,
}

impl From<TenantModeConfig> for TenantMode {
    fn from(m: TenantModeConfig) -> Self {
        match m {
            TenantModeConfig::Off => TenantMode::Off,
            TenantModeConfig::LogOnly => TenantMode::LogOnly,
            TenantModeConfig::Enforce => TenantMode::Enforce,
        }
    }
}

impl From<OnUnavailableConfig> for OnUnavailable {
    fn from(o: OnUnavailableConfig) -> Self {
        match o {
            OnUnavailableConfig::Open => OnUnavailable::Open,
            OnUnavailableConfig::Closed => OnUnavailable::Closed,
        }
    }
}

/// The upper bound of every `tool_policy.*_secs` value: one day.
const MAX_TOOL_POLICY_SECS: u64 = 86_400;

impl ToolPolicyConfig {
    /// Every rule of spec § 6.4. Each message names the key. All rules are checked even when
    /// `enabled = false`, except the three "required when enabled" rules and the D31 cross-table
    /// rule (`load_timeout_ms` against `tool_interception.decide_timeout_secs`).
    pub fn validate(&self, ti: &ToolInterceptionConfig) -> Result<(), String> {
        for (key, v) in [
            ("tool_policy.refresh_secs", self.refresh_secs),
            ("tool_policy.max_stale_secs", self.max_stale_secs),
            ("tool_policy.idle_evict_secs", self.idle_evict_secs),
        ] {
            if v == 0 {
                return Err(format!("{key} must be at least 1"));
            }
            if v > MAX_TOOL_POLICY_SECS {
                return Err(format!("{key} must be at most {MAX_TOOL_POLICY_SECS} (one day); got {v}"));
            }
        }
        if self.load_timeout_ms == 0 {
            return Err("tool_policy.load_timeout_ms must be at least 1".to_string());
        }
        if self.load_timeout_ms > MAX_TOOL_POLICY_SECS * 1000 {
            return Err(format!(
                "tool_policy.load_timeout_ms must be at most {} (one day); got {}",
                MAX_TOOL_POLICY_SECS * 1000,
                self.load_timeout_ms
            ));
        }
        if self.max_stale_secs < self.refresh_secs {
            return Err(format!(
                "tool_policy.max_stale_secs ({}) must not be less than tool_policy.refresh_secs ({})",
                self.max_stale_secs, self.refresh_secs
            ));
        }
        // D31: a load that can outlast the decider's time limit would turn a slow source into a decider timeout.
        // Checked only when enabled: a disabled table must not refuse a `[tool_interception]` that is valid alone.
        if self.enabled && self.load_timeout_ms >= ti.decide_timeout_secs.saturating_mul(1000) {
            return Err(format!(
                "tool_policy.load_timeout_ms ({}) must be less than tool_interception.decide_timeout_secs ({}) in milliseconds",
                self.load_timeout_ms, ti.decide_timeout_secs
            ));
        }
        self.detectors.validate(self.enabled, ti)?;
        let mut seen = HashSet::new();
        for (i, entry) in self.org.iter().enumerate() {
            let Some(id) = parse_org_uuid(&entry.id) else {
                return Err(format!("tool_policy.org[{i}].id must be one organization UUID in the 36-character form; got {:?}", entry.id));
            };
            if !seen.insert(id) {
                return Err(format!("tool_policy.org[{i}].id {:?} names an org that an earlier [[tool_policy.org]] entry already lists", entry.id));
            }
            for name in &entry.webhooks {
                let Some(provider) = self.detectors.webhook.iter().find(|w| &w.name == name) else {
                    return Err(format!(
                        "tool_policy.org[{i}].webhooks names {name:?}, which is not a registered [[tool_policy.detectors.webhook]] provider"
                    ));
                };
                if !provider.org_scope().permits(id) {
                    return Err(format!("tool_policy.org[{i}].webhooks names {name:?}, but the orgs of webhook[{name}] do not hold org {:?}", entry.id));
                }
            }
        }
        // SMA-775: with the IAM source, IAM holds the mode and the fail mode of each org. An entry
        // here would look as if it applied, so it is refused.
        if matches!(self.source, Some(PolicySourceConfig::Iam {})) && !self.org.is_empty() {
            return Err("[[tool_policy.org]] entries are not allowed with tool_policy.source kind = \"iam\": IAM holds the mode and the fail mode of each org (remove the entries)".to_string());
        }
        if let Some(nats) = &self.nats {
            if nats.url.expose_secret().trim().is_empty() {
                return Err("tool_policy.nats.url must not be empty (omit the [tool_policy.nats] table to run without invalidation events)".to_string());
            }
            for (key, v) in [("tool_policy.nats.creds_file", &nats.creds_file), ("tool_policy.nats.ca_file", &nats.ca_file)] {
                if v.as_deref().is_some_and(|p| p.trim().is_empty()) {
                    return Err(format!("{key} must not be empty (omit the key to use none)"));
                }
            }
            // async-nats 0.53: `add_root_certificates` implies `require_tls(true)`, so a CA file
            // with a plain `nats://` url fails at every connect. Refuse it at boot.
            if nats.ca_file.is_some() {
                let url = nats.url.expose_secret().trim();
                let scheme_is_tls = url.split_once("://").is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("tls"));
                if !scheme_is_tls {
                    return Err("tool_policy.nats.ca_file requires a tls:// url — async-nats 0.53 makes a named CA file imply require_tls(true), so a plain nats:// broker would fail to connect; remove ca_file or use a tls:// url".to_string());
                }
            }
        }
        if self.enabled {
            if self.on_policy_unavailable.is_none() {
                return Err("tool_policy.on_policy_unavailable is required when tool_policy.enabled = true (\"open\" or \"closed\")".to_string());
            }
            if !ti.enabled {
                return Err("tool_policy.enabled = true requires tool_interception.enabled = true".to_string());
            }
            match &self.source {
                None => return Err("tool_policy.enabled = true requires tool_policy.source (for example kind = \"file\")".to_string()),
                Some(PolicySourceConfig::File { dir }) if dir.trim().is_empty() => return Err("tool_policy.source.dir must not be empty".to_string()),
                Some(_) => {}
            }
        }
        Ok(())
    }

    /// SMA-856 D9: the opt-in list of each org in the TOML, sorted and unique. An org with an empty
    /// list has no entry. Meaningful only after `validate` returned `Ok`.
    #[must_use]
    pub fn org_webhooks(&self) -> HashMap<Uuid, Arc<[String]>> {
        let mut out = HashMap::new();
        for entry in &self.org {
            let Some(id) = parse_org_uuid(&entry.id) else { continue };
            let names: BTreeSet<&String> = entry.webhooks.iter().collect();
            if !names.is_empty() {
                out.insert(id, names.into_iter().cloned().collect::<Arc<[String]>>());
            }
        }
        out
    }

    /// The runtime view of `[tool_policy.detectors]`. Meaningful only after `validate` returned `Ok`.
    #[must_use]
    pub fn detector_settings(&self, default_scan_bytes: usize) -> DetectorSettings {
        DetectorSettings {
            enabled: self.detectors.enabled,
            timeout: Duration::from_millis(self.detectors.timeout_ms),
            max_scan_bytes: self.detectors.max_scan_bytes.unwrap_or(default_scan_bytes),
            margin: Duration::from_millis(DETECTOR_MARGIN_MS),
        }
    }

    /// The runtime view: durations, the per-org map keyed by the parsed `Uuid`, and the defaults.
    /// The values are meaningful only after `validate` returned `Ok`.
    ///
    /// # Panics
    /// Panics on an org id that is not a valid UUID, which `validate` refuses. A silent drop would
    /// give that org the default mode, so the precondition is enforced loudly instead.
    #[must_use]
    pub fn settings(&self) -> ToolPolicySettings {
        ToolPolicySettings {
            enabled: self.enabled,
            default_mode: self.default_mode.into(),
            // When the key is unset, `validate` has already refused `enabled = true`. Closed is the fail-safe value.
            on_policy_unavailable: self.on_policy_unavailable.map_or(OnUnavailable::Closed, Into::into),
            refresh: Duration::from_secs(self.refresh_secs),
            max_stale: Duration::from_secs(self.max_stale_secs),
            idle_evict: Duration::from_secs(self.idle_evict_secs),
            load_timeout: Duration::from_millis(self.load_timeout_ms),
            orgs: self
                .org
                .iter()
                .map(|e| {
                    let id = parse_org_uuid(&e.id).expect("ToolPolicyConfig::settings needs a config that passed validate: tool_policy.org id is not a valid UUID");
                    (id, (e.mode.into(), e.on_policy_unavailable.map(OnUnavailable::from)))
                })
                .collect(),
            // Only the IAM source returns the tenant settings. The file source has none.
            settings_from_source: matches!(self.source, Some(PolicySourceConfig::Iam {})),
        }
    }
}

impl DetectorsConfig {
    /// The rules of spec § 6.10. Each message names the key. The D35 margin rule holds only when
    /// `[tool_policy]` is enabled, so a disabled table never refuses a valid `[tool_interception]`.
    fn validate(&self, tool_policy_enabled: bool, ti: &ToolInterceptionConfig) -> Result<(), String> {
        if self.timeout_ms == 0 {
            return Err("tool_policy.detectors.timeout_ms must be at least 1".to_string());
        }
        if let Some(n) = self.max_scan_bytes
            && n < MIN_DETECTOR_SCAN_BYTES
        {
            return Err(format!("tool_policy.detectors.max_scan_bytes must be at least {MIN_DETECTOR_SCAN_BYTES}; got {n}"));
        }
        let decide_ms = ti.decide_timeout_secs.saturating_mul(1000);
        if tool_policy_enabled && self.timeout_ms.saturating_add(DETECTOR_MARGIN_MS) > decide_ms {
            return Err(format!(
                "tool_policy.detectors.timeout_ms ({}) plus the {DETECTOR_MARGIN_MS} ms margin must not exceed tool_interception.decide_timeout_secs ({}) in milliseconds",
                self.timeout_ms, ti.decide_timeout_secs
            ));
        }
        Ok(())
    }
}

/// What the gateway reads at run time from `[tool_policy]` (built once by `ToolPolicyConfig::settings`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPolicySettings {
    pub enabled: bool,
    pub default_mode: TenantMode,
    /// The global `on_policy_unavailable` (D34).
    pub on_policy_unavailable: OnUnavailable,
    pub refresh: Duration,
    pub max_stale: Duration,
    pub idle_evict: Duration,
    pub load_timeout: Duration,
    /// Per org: the mode and the optional `on_policy_unavailable` override.
    pub orgs: HashMap<Uuid, (TenantMode, Option<OnUnavailable>)>,
    /// The source returns the tenant mode and fail mode (the IAM source). The request side then
    /// takes them from the cache, and `default_mode` and `orgs` do not apply to an org scope.
    pub settings_from_source: bool,
}

impl ToolPolicySettings {
    /// The tenant mode and the effective `on_policy_unavailable` of an org (D7, D34).
    #[must_use]
    pub fn tenant(&self, org: Uuid) -> (TenantMode, OnUnavailable) {
        match self.orgs.get(&org) {
            Some((mode, over)) => (*mode, over.unwrap_or(self.on_policy_unavailable)),
            None => (self.default_mode, self.on_policy_unavailable),
        }
    }
}

#[cfg(test)]
// `figment::Jail::expect_with` fixes its closure's `Err` type to `figment::Error`
// (~208B) — not something callers control, so the size lint is allowed here, scoped
// to this test module's Jail-based tests, rather than reshaped away.
#[allow(clippy::result_large_err)]
mod tests {
    use super::*;

    fn valid_toml() -> &'static str {
        r#"
            [upstream.openai]
            api_key = "sk-test-key"
        "#
    }

    // --- `is_loopback_host` direct unit tests (D8 hardening) ---------------------------------
    // A prefix match like `starts_with("127.")` is trivially bypassable by a crafted,
    // DNS-resolvable hostname; these lock in the IP-parse-based classification.

    #[test]
    fn is_loopback_host_rejects_crafted_hostnames() {
        // `127.evil.com` / `127.0.0.1.attacker...` defeat a naive `starts_with("127.")`;
        // `localhost.evil.com` defeats a naive `starts_with("localhost")` — all are ordinary
        // DNS names resolvable to arbitrary addresses, so none is loopback.
        assert!(!is_loopback_host("https://127.evil.com:9090"));
        assert!(!is_loopback_host("https://127.0.0.1.attacker.example.com:9090"));
        assert!(!is_loopback_host("https://localhost.evil.com:9090"));
        assert!(!is_loopback_host("https://iam.internal.example.com:9090"));
    }

    // The host comes from the URI authority only. A loopback address in the PATH, after a
    // second `://`, is not the host: the channel connects to the authority.
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
        // Any address in 127.0.0.0/8 is loopback (that's what `Ipv4Addr::is_loopback` gives).
        assert!(is_loopback_host("http://127.0.0.5:9090"));
        assert!(is_loopback_host("http://localhost:9090"));
        // IPv6 loopback literal — bracket stripping must yield `::1` (not `[::1]`) so the
        // `Ipv6Addr` parse succeeds.
        assert!(is_loopback_host("http://[::1]:9090"));
        // A bare host with no scheme and no port still classifies correctly.
        assert!(is_loopback_host("127.0.0.1"));
    }

    #[test]
    fn defaults_land_with_a_valid_api_key() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", valid_toml())?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert_eq!(cfg.http_addr.to_string(), "0.0.0.0:8088");
            assert_eq!(cfg.connect_timeout_secs, 10);
            assert_eq!(cfg.first_byte_timeout_secs, 30);
            assert_eq!(cfg.stream_idle_timeout_secs, 300);
            assert_eq!(cfg.max_request_bytes, 1_048_576);
            assert_eq!(cfg.log_level, "info");
            assert_eq!(cfg.iam.grpc_addr, "https://127.0.0.1:9090");
            assert_eq!(
                cfg.iam.tls,
                IamTlsConfig::Tls {
                    ca_cert_path: None,
                    client_cert_path: None,
                    client_key_path: None
                }
            );
            assert_eq!(cfg.upstream.openai.base_url, "https://api.openai.com");
            assert_eq!(cfg.upstream.openai.api_key.expose_secret(), "sk-test-key");
            assert!(cfg.stream_enabled, "stream_enabled must default to true");
            assert!(cfg.validate().is_ok(), "a valid config should pass validation");
            Ok(())
        });
    }

    #[test]
    fn missing_api_key_loads_fine_but_fails_validate() {
        figment::Jail::expect_with(|jail| {
            jail.clear_env();
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.upstream.openai.api_key.expose_secret().is_empty());
            assert!(cfg.validate().is_err(), "an unset api key must fail validate(), not figment extraction");
            Ok(())
        });
    }

    #[test]
    fn api_key_from_env() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("GATEWAY_UPSTREAM__OPENAI__API_KEY", "sk-env-key");
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert_eq!(cfg.upstream.openai.api_key.expose_secret(), "sk-env-key");
            assert!(cfg.validate().is_ok());
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_a_zero_connect_timeout() {
        figment::Jail::expect_with(|jail| {
            // The override MUST precede `[upstream.openai]` — TOML keys after a table header
            // belong to that table, not the document root, so appending would silently land
            // inside `[upstream.openai]` instead of overriding the root-level field.
            jail.create_file("gateway.toml", &format!("connect_timeout_secs = 0\n{}", valid_toml()))?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected connect_timeout_secs = 0 to fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_a_zero_first_byte_timeout() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", &format!("first_byte_timeout_secs = 0\n{}", valid_toml()))?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected first_byte_timeout_secs = 0 to fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_a_zero_stream_idle_timeout() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", &format!("stream_idle_timeout_secs = 0\n{}", valid_toml()))?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected stream_idle_timeout_secs = 0 to fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_zero_max_request_bytes() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", &format!("max_request_bytes = 0\n{}", valid_toml()))?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected max_request_bytes = 0 to fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_an_empty_ca_bundle_path() {
        // `GATEWAY_UPSTREAM__OPENAI__EXTRA_CA_BUNDLE_PATH=` deserializes to Some(""), not None,
        // which would otherwise reach `std::fs::read("")` and fail with a confusing empty path.
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "gateway.toml",
                r#"
                    [upstream.openai]
                    api_key = "sk-test-key"
                    extra_ca_bundle_path = ""
                "#,
            )?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "an empty bundle path must be rejected");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_a_padded_ca_bundle_path() {
        // Likelier than the empty case in practice: a heredoc'd Kubernetes secret or an env
        // override carries a trailing newline, and the resulting error would name a path that
        // reads as correct. Mirrors iam's rule on `authn.extra_ca_bundle_path`.
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "gateway.toml",
                r#"
                    [upstream.openai]
                    api_key = "sk-test-key"
                    extra_ca_bundle_path = "/etc/paigasus/corp-ca.pem\n"
                "#,
            )?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            let err = cfg.validate().expect_err("a padded bundle path must be rejected");
            assert!(err.contains("whitespace"), "the message must name the defect: {err}");
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_loopback_insecure_with_a_non_loopback_grpc_addr() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "gateway.toml",
                r#"
                    [upstream.openai]
                    api_key = "sk-test-key"

                    [iam]
                    grpc_addr = "https://iam.internal.example.com:9090"

                    [iam.tls]
                    mode = "loopback_insecure"
                "#,
            )?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.validate().is_err(), "expected loopback_insecure with a non-loopback grpc_addr to fail validation");
            Ok(())
        });
    }

    #[test]
    fn validate_accepts_loopback_insecure_with_a_loopback_grpc_addr() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "gateway.toml",
                r#"
                    [upstream.openai]
                    api_key = "sk-test-key"

                    [iam]
                    grpc_addr = "http://127.0.0.1:9090"

                    [iam.tls]
                    mode = "loopback_insecure"
                "#,
            )?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.validate().is_ok(), "expected loopback_insecure with a loopback grpc_addr to pass validation");
            Ok(())
        });
    }

    #[test]
    fn validate_accepts_loopback_insecure_with_localhost() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "gateway.toml",
                r#"
                    [upstream.openai]
                    api_key = "sk-test-key"

                    [iam]
                    grpc_addr = "http://localhost:9090"

                    [iam.tls]
                    mode = "loopback_insecure"
                "#,
            )?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.validate().is_ok(), "expected loopback_insecure with localhost to pass validation");
            Ok(())
        });
    }

    #[test]
    fn metrics_addr_must_differ_from_http_addr() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", valid_toml())?;
            let mut cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            cfg.metrics.enabled = true;
            cfg.metrics.addr = Some(cfg.http_addr); // exact collision
            assert!(cfg.validate().is_err(), "metrics.addr == http_addr is a config error");
            Ok(())
        });
    }

    #[test]
    fn metrics_addr_same_port_different_host_as_http_addr_is_rejected() {
        figment::Jail::expect_with(|jail| {
            // A wildcard-vs-loopback pair on the SAME port is NOT caught by exact-address
            // equality but both listeners still fail at bind with `AddrInUse` — validate() must
            // reject this too, not just an exact-address match.
            jail.create_file("gateway.toml", &format!("http_addr = \"127.0.0.1:8088\"\n{}", valid_toml()))?;
            let mut cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            cfg.metrics.enabled = true;
            cfg.metrics.addr = Some("0.0.0.0:8088".parse().expect("valid addr"));
            assert!(cfg.validate().is_err(), "metrics.addr and http_addr sharing a port on different hosts is a config error");
            Ok(())
        });
    }

    #[test]
    fn metrics_addr_distinct_port_from_http_addr_is_ok() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", valid_toml())?;
            let mut cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            cfg.metrics.enabled = true;
            cfg.metrics.addr = Some("0.0.0.0:9999".parse().expect("valid addr"));
            assert!(cfg.validate().is_ok(), "metrics.addr on a distinct port from http_addr should validate");
            Ok(())
        });
    }

    #[test]
    fn api_key_is_never_in_serialized_output() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", valid_toml())?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            let dumped = serde_json::to_string(&cfg).expect("serialize");
            assert!(!dumped.contains("sk-test-key"), "the configured API key must never appear in Serialize output: {dumped}");
            assert!(!dumped.contains("api_key"), "the api_key field itself must be entirely absent from Serialize output: {dumped}");
            let debugged = format!("{cfg:?}");
            assert!(!debugged.contains("sk-test-key"), "the configured API key must never appear in Debug output: {debugged}");
            Ok(())
        });
    }

    use crate::domain::limits::{BudgetPeriod, OrgOverride};
    use std::num::NonZeroU64;

    const ORG_A: &str = "0190a100-0000-7000-8000-0000000000a1";

    fn limits_toml(body: &str) -> String {
        format!("{}\n[limits]\n{body}\n", valid_toml())
    }

    #[test]
    fn no_limits_table_means_no_limits() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", valid_toml())?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.limits.is_none(), "A6/Q5: limits are off unless configured");
            assert!(cfg.validate().is_ok());
            Ok(())
        });
    }

    #[test]
    fn an_empty_limits_table_gives_only_empty_policies() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", &limits_toml(""))?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            let limits = cfg.limits.as_ref().expect("an empty [limits] table is Some");
            assert_eq!(limits.backend, LimitsBackend::Memory, "the backend defaults to memory");
            assert_eq!(limits.budget_period, BudgetPeriod::Monthly, "the period defaults to monthly");
            assert!(cfg.validate().is_ok());
            assert!(limits.rules().every_policy_is_empty());
            Ok(())
        });
    }

    #[test]
    fn a_full_limits_table_loads_and_converts_to_rules() {
        figment::Jail::expect_with(|jail| {
            let body = format!(
                "principal_requests_per_minute = 60\norg_requests_per_minute = 600\ntokens_per_period = 5000000\nbudget_period = \"weekly\"\n\n[[limits.org]]\nid = \"{ORG_A}\"\norg_requests_per_minute = 1200\ntokens_per_period = 20000000\n\n[[limits.org]]\nid = \"0190a100-0000-7000-8000-0000000000b2\"\nexempt = true\n"
            );
            jail.create_file("gateway.toml", &limits_toml(&body))?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
            let rules = cfg.limits.as_ref().expect("the table is set").rules();
            assert_eq!(rules.principal_requests_per_minute, NonZeroU64::new(60));
            assert_eq!(rules.org_requests_per_minute, NonZeroU64::new(600));
            assert_eq!(rules.tokens_per_period, NonZeroU64::new(5_000_000));
            assert_eq!(rules.budget_period, BudgetPeriod::Weekly);
            let a = uuid::Uuid::try_parse(ORG_A).expect("a uuid");
            assert_eq!(
                rules.overrides[&a],
                OrgOverride {
                    org_requests_per_minute: NonZeroU64::new(1200),
                    tokens_per_period: NonZeroU64::new(20_000_000),
                    exempt: false
                }
            );
            let b = uuid::Uuid::try_parse("0190a100-0000-7000-8000-0000000000b2").expect("a uuid");
            assert!(rules.overrides[&b].exempt);
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_each_bad_limits_value_and_names_the_key() {
        let cases: &[(&str, &str)] = &[
            ("principal_requests_per_minute = 0", "limits.principal_requests_per_minute must be at least 1"),
            ("org_requests_per_minute = 0", "limits.org_requests_per_minute must be at least 1"),
            ("tokens_per_period = 0", "limits.tokens_per_period must be at least 1"),
            ("principal_requests_per_minute = 1000000001", "limits.principal_requests_per_minute must be at most 1000000000"),
            ("org_requests_per_minute = 1000000001", "limits.org_requests_per_minute must be at most 1000000000"),
            ("tokens_per_period = 1000000000001", "limits.tokens_per_period must be at most 1000000000000"),
            ("[[limits.org]]\nid = \"acme\"", "limits.org[0].id must be one organization UUID"),
            ("[[limits.org]]\nid = \"0190a1000000700080000000000000a1\"", "limits.org[0].id must be one organization UUID"),
            (
                "[[limits.org]]\nid = \"0190a100-0000-7000-8000-0000000000a1\"\n[[limits.org]]\nid = \"0190A100-0000-7000-8000-0000000000A1\"",
                "limits.org[1].id",
            ),
            (
                "[[limits.org]]\nid = \"0190a100-0000-7000-8000-0000000000a1\"\norg_requests_per_minute = 0",
                "limits.org[0].org_requests_per_minute must be at least 1",
            ),
            (
                "[[limits.org]]\nid = \"0190a100-0000-7000-8000-0000000000a1\"\ntokens_per_period = 1000000000001",
                "limits.org[0].tokens_per_period must be at most",
            ),
            (
                "[[limits.org]]\nid = \"0190a100-0000-7000-8000-0000000000a1\"\nexempt = true\ntokens_per_period = 5",
                "limits.org[0] sets exempt = true together with a limit",
            ),
            ("backend = \"redis\"", "limits.backend = \"redis\" requires limits.redis_url"),
            ("backend = \"redis\"\nredis_url = \"  \"", "limits.backend = \"redis\" requires limits.redis_url"),
            ("redis_url = \"redis://127.0.0.1:6379\"", "limits.redis_url is set but limits.backend is \"memory\""),
        ];
        for (body, want) in cases {
            figment::Jail::expect_with(|jail| {
                jail.create_file("gateway.toml", &limits_toml(body))?;
                let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
                let err = cfg.validate().expect_err(body);
                assert!(err.contains(want), "{body}: got {err}");
                Ok(())
            });
        }
    }

    #[test]
    fn an_unknown_budget_period_fails_extraction() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", &limits_toml("budget_period = \"yearly\""))?;
            assert!(GatewayConfig::figment().extract::<GatewayConfig>().is_err());
            Ok(())
        });
    }

    /// Review Focus 1: a typo must fail the boot, not switch a budget off silently.
    #[test]
    fn a_misspelt_limits_key_fails_extraction() {
        for body in ["token_per_period = 5", "[[limits.org]]\nid = \"0190a100-0000-7000-8000-0000000000a1\"\nexmpt = true"] {
            figment::Jail::expect_with(|jail| {
                jail.create_file("gateway.toml", &limits_toml(body))?;
                let err = GatewayConfig::figment().extract::<GatewayConfig>().expect_err(body);
                assert!(err.to_string().contains("unknown field"), "{body}: {err}");
                Ok(())
            });
        }
    }

    /// Review Focus 2: the override id and the scope PRN may use different letter case.
    #[test]
    fn an_upper_case_override_id_applies_to_the_canonical_org() {
        figment::Jail::expect_with(|jail| {
            jail.create_file(
                "gateway.toml",
                &limits_toml("org_requests_per_minute = 10\n[[limits.org]]\nid = \"0190A100-0000-7000-8000-0000000000A1\"\norg_requests_per_minute = 99"),
            )?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.validate().is_ok());
            let lower = uuid::Uuid::try_parse(ORG_A).expect("a uuid");
            assert_eq!(cfg.limits.as_ref().expect("set").rules().policy_for(Some(lower)).org_requests_per_minute, NonZeroU64::new(99));
            Ok(())
        });
    }

    #[test]
    fn every_limits_key_reads_from_the_environment() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", valid_toml())?;
            jail.set_env("GATEWAY_LIMITS__BACKEND", "redis");
            jail.set_env("GATEWAY_LIMITS__REDIS_URL", "redis://:hunter2@127.0.0.1:6379/2");
            jail.set_env("GATEWAY_LIMITS__PRINCIPAL_REQUESTS_PER_MINUTE", "60");
            jail.set_env("GATEWAY_LIMITS__ORG_REQUESTS_PER_MINUTE", "600");
            jail.set_env("GATEWAY_LIMITS__TOKENS_PER_PERIOD", "5000000");
            jail.set_env("GATEWAY_LIMITS__BUDGET_PERIOD", "daily");
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
            let limits = cfg.limits.as_ref().expect("the env vars create the table");
            assert_eq!(limits.backend, LimitsBackend::Redis);
            assert_eq!(limits.redis_url.as_ref().map(|u| u.expose_secret().to_owned()), Some("redis://:hunter2@127.0.0.1:6379/2".to_owned()));
            assert_eq!(
                (limits.principal_requests_per_minute, limits.org_requests_per_minute, limits.tokens_per_period),
                (Some(60), Some(600), Some(5_000_000))
            );
            assert_eq!(limits.budget_period, BudgetPeriod::Daily);
            Ok(())
        });
    }

    fn th_toml(body: &str) -> String {
        format!("{}\n[tool_history]\n{body}\n", valid_toml())
    }

    #[test]
    fn parse_deadline_accepts_ms_and_s() {
        assert_eq!(parse_deadline("1s"), Ok(Duration::from_millis(1000)));
        assert_eq!(parse_deadline("250ms"), Ok(Duration::from_millis(250)));
    }

    #[test]
    fn parse_deadline_refuses_other_forms() {
        for s in ["", "0s", "0ms", "1.5s", "1m", " 1s", "-1s", "s", "ms", "1 s", "+1s"] {
            assert!(parse_deadline(s).is_err(), "{s:?} must be refused");
        }
    }

    #[test]
    fn no_tool_history_table_is_none() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", valid_toml())?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.tool_history.is_none(), "D12: no table means no history");
            Ok(())
        });
    }

    #[test]
    fn the_default_deadline_is_one_second() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", &th_toml(""))?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            let th = cfg.tool_history.as_ref().expect("an empty table is Some");
            assert_eq!(th.backend, HistoryBackend::Memory);
            assert_eq!(th.deadline, "1s");
            assert_eq!(th.deadline(), Duration::from_secs(1));
            assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
            Ok(())
        });
    }

    #[test]
    fn validate_rejects_each_bad_tool_history_value_and_names_the_key() {
        let cases = [
            ("deadline = \"\"", "tool_history.deadline"),
            ("deadline = \"0s\"", "tool_history.deadline"),
            ("deadline = \"1.5s\"", "tool_history.deadline"),
            ("deadline = \"1m\"", "tool_history.deadline"),
            (
                "deadline = \"5s\"\n[tool_interception]\ndecide_timeout_secs = 5",
                "tool_history.deadline (5s) must be less than tool_interception.decide_timeout_secs (5)",
            ),
            ("backend = \"redis\"", "tool_history.backend = \"redis\" requires tool_history.redis_url"),
            ("backend = \"redis\"\nredis_url = \"  \"", "tool_history.backend = \"redis\" requires tool_history.redis_url"),
            ("redis_url = \"redis://127.0.0.1:6379\"", "tool_history.redis_url is set but tool_history.backend is \"memory\""),
        ];
        for (body, want) in cases {
            figment::Jail::expect_with(|jail| {
                // A `[tool_interception]` table inside `body` must follow the `[tool_history]` keys.
                jail.create_file("gateway.toml", &th_toml(body))?;
                let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
                let err = cfg.validate().expect_err(body);
                assert!(err.contains(want), "{body}: got {err}");
                Ok(())
            });
        }
    }

    #[test]
    fn a_misspelt_tool_history_key_fails_extraction() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", &th_toml("deadlin = \"1s\""))?;
            assert!(GatewayConfig::figment().extract::<GatewayConfig>().is_err());
            Ok(())
        });
    }

    #[test]
    fn the_redis_url_comes_from_the_environment() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", valid_toml())?;
            jail.set_env("GATEWAY_TOOL_HISTORY__BACKEND", "redis");
            jail.set_env("GATEWAY_TOOL_HISTORY__REDIS_URL", "redis://:hunter2@127.0.0.1:6379/3");
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
            let th = cfg.tool_history.as_ref().expect("the env vars create the table");
            assert_eq!(th.backend, HistoryBackend::Redis);
            assert_eq!(th.redis_url.as_ref().map(|u| u.expose_secret().to_owned()), Some("redis://:hunter2@127.0.0.1:6379/3".to_owned()));
            Ok(())
        });
    }

    #[test]
    fn the_history_keys_come_from_the_environment() {
        const K1: &str = "0123456789abcdef0123456789abcdef-one";
        const K2: &str = "0123456789abcdef0123456789abcdef-two";
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", valid_toml())?;
            jail.set_env("GATEWAY_TOOL_HISTORY__DIGEST_KEY", K1);
            jail.set_env("GATEWAY_TOOL_HISTORY__PREVIOUS_DIGEST_KEY", K2);
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
            let th = cfg.tool_history.as_ref().expect("the env vars create the table");
            assert_eq!(th.digest_key.as_ref().map(|k| k.expose_secret().to_owned()), Some(K1.to_owned()));
            assert_eq!(th.previous_digest_key.as_ref().map(|k| k.expose_secret().to_owned()), Some(K2.to_owned()));
            let json = serde_json::to_string(th).expect("serializes");
            assert!(!json.contains(K1) && !json.contains(K2) && !json.contains("digest_key"), "{json}");
            Ok(())
        });
    }

    #[test]
    fn a_pr1_config_with_no_key_still_validates() {
        let ti = ToolInterceptionConfig::default();
        assert!(ToolHistoryConfig::default().validate(&ti).is_ok());
    }

    #[test]
    fn a_bad_history_key_names_its_config_key() {
        let ti = ToolInterceptionConfig::default();
        let short = ToolHistoryConfig {
            digest_key: Some(SecretString::from("short".to_owned())),
            ..ToolHistoryConfig::default()
        };
        assert!(short.validate(&ti).unwrap_err().starts_with("tool_history.digest_key: "));
        let orphan = ToolHistoryConfig {
            previous_digest_key: Some(SecretString::from("0123456789abcdef0123456789abcdef".to_owned())),
            ..ToolHistoryConfig::default()
        };
        assert!(orphan.validate(&ti).unwrap_err().starts_with("tool_history.previous_digest_key: "));
    }

    #[test]
    fn the_redis_url_is_not_serialized() {
        let th = ToolHistoryConfig {
            backend: HistoryBackend::Redis,
            redis_url: Some(SecretString::from("redis://:hunter2@h:6379".to_owned())),
            ..ToolHistoryConfig::default()
        };
        let json = serde_json::to_string(&th).expect("serializes");
        assert!(!json.contains("hunter2") && !json.contains("redis_url"), "{json}");
    }

    fn ti_toml(body: &str) -> String {
        format!("{}\n[tool_interception]\n{body}\n", valid_toml())
    }

    #[test]
    fn tool_interception_is_off_by_default_with_the_spec_limits() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", valid_toml())?;
            let cfg = GatewayConfig::figment().extract::<GatewayConfig>()?;
            let ti = &cfg.tool_interception;
            assert!(!ti.enabled);
            assert_eq!(
                (ti.max_record_bytes, ti.max_held_bytes, ti.max_hold_secs, ti.decide_timeout_secs, ti.keepalive_secs),
                (1_048_576, 4_194_304, 300, 5, 10)
            );
            assert_eq!(ti.on_failure, OnFailureConfig::Block);
            assert_eq!(ti.limits(), paigasus_gateway_core::Limits::defaults());
            cfg.validate().map_err(figment::Error::from)?;
            Ok(())
        });
    }

    #[test]
    fn tool_interception_reads_the_table_and_the_environment() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", &ti_toml("enabled = true\non_failure = \"allow\"\nmax_hold_secs = 120"))?;
            jail.set_env("GATEWAY_TOOL_INTERCEPTION__KEEPALIVE_SECS", "15");
            let cfg = GatewayConfig::figment().extract::<GatewayConfig>()?;
            assert!(cfg.tool_interception.enabled);
            assert_eq!(cfg.tool_interception.on_failure, OnFailureConfig::Allow);
            assert_eq!(cfg.tool_interception.limits().max_hold, std::time::Duration::from_secs(120));
            assert_eq!(cfg.tool_interception.limits().on_failure, paigasus_gateway_core::OnFailure::Allow);
            assert_eq!(cfg.tool_interception.keepalive_secs, 15);
            Ok(())
        });
    }

    #[test]
    fn a_misspelt_tool_interception_key_fails_extraction() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", &ti_toml("enabeld = true"))?;
            let err = GatewayConfig::figment().extract::<GatewayConfig>().expect_err("a typo must not leave interception silently off");
            assert!(err.to_string().contains("unknown field"), "{err}");
            Ok(())
        });
    }

    #[test]
    fn tool_interception_validation_names_each_key() {
        for (body, key) in [
            ("max_hold_secs = 86401", "tool_interception.max_hold_secs"),
            ("decide_timeout_secs = 86401", "tool_interception.decide_timeout_secs"),
            ("max_hold_secs = 86400\nkeepalive_secs = 86401", "tool_interception.keepalive_secs"),
            ("keepalive_secs = 9223372036854775807", "tool_interception.keepalive_secs"),
            ("max_record_bytes = 0", "tool_interception.max_record_bytes"),
            ("max_held_bytes = 0", "tool_interception.max_held_bytes"),
            ("max_hold_secs = 0", "tool_interception.max_hold_secs"),
            ("decide_timeout_secs = 0", "tool_interception.decide_timeout_secs"),
            ("keepalive_secs = 0", "tool_interception.keepalive_secs"),
            ("max_record_bytes = 10\nmax_held_bytes = 5", "tool_interception.max_record_bytes"),
            ("keepalive_secs = 300", "tool_interception.keepalive_secs"),
        ] {
            figment::Jail::expect_with(|jail| {
                jail.create_file("gateway.toml", &ti_toml(body))?;
                let cfg = GatewayConfig::figment().extract::<GatewayConfig>()?;
                let err = cfg.validate().expect_err(body);
                assert!(err.contains(key), "{body}: {err}");
                if body.contains("max_held_bytes = 5") {
                    assert!(err.contains("must not exceed"), "{body}: {err}");
                }
                Ok(())
            });
        }
    }

    fn responses_toml(body: &str) -> String {
        format!("{}\n[responses]\n{body}\n", valid_toml())
    }

    #[test]
    fn responses_defaults_match_the_spec() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", valid_toml())?;
            let cfg = GatewayConfig::figment().extract::<GatewayConfig>()?;
            let r = &cfg.responses;
            assert!(!r.enabled);
            assert_eq!((r.max_request_bytes, r.max_record_bytes, r.max_held_bytes), (16_777_216, 16_777_216, 33_554_432));
            cfg.validate().map_err(figment::Error::from)?;
            Ok(())
        });
    }

    #[test]
    fn responses_keys_load_from_toml_and_env() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", &responses_toml("enabled = true\nmax_record_bytes = 1024"))?;
            let cfg = GatewayConfig::figment().extract::<GatewayConfig>()?;
            assert!(cfg.responses.enabled);
            assert_eq!(cfg.responses.max_record_bytes, 1024);
            Ok(())
        });
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", valid_toml())?;
            jail.set_env("GATEWAY_RESPONSES__ENABLED", "true");
            jail.set_env("GATEWAY_RESPONSES__MAX_REQUEST_BYTES", "2048");
            let cfg = GatewayConfig::figment().extract::<GatewayConfig>()?;
            assert!(cfg.responses.enabled);
            assert_eq!(cfg.responses.max_request_bytes, 2048);
            Ok(())
        });
    }

    #[test]
    fn responses_validation_names_the_key() {
        for (body, key) in [
            ("max_request_bytes = 0", "responses.max_request_bytes"),
            ("max_record_bytes = 0", "responses.max_record_bytes"),
            ("max_held_bytes = 0", "responses.max_held_bytes"),
            ("max_record_bytes = 10\nmax_held_bytes = 5", "responses.max_record_bytes"),
        ] {
            figment::Jail::expect_with(|jail| {
                jail.create_file("gateway.toml", &responses_toml(body))?;
                let cfg = GatewayConfig::figment().extract::<GatewayConfig>()?;
                let err = cfg.validate().expect_err(body);
                assert!(err.contains(key), "{body}: {err}");
                Ok(())
            });
        }
    }

    #[test]
    fn an_unknown_responses_key_fails() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", &responses_toml("enabeld = true"))?;
            let err = GatewayConfig::figment().extract::<GatewayConfig>().expect_err("a typo must not leave the route silently off");
            assert!(err.to_string().contains("unknown field"), "{err}");
            Ok(())
        });
    }

    #[test]
    fn dpop_is_off_by_default_and_reads_from_the_environment() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", valid_toml())?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(!cfg.dpop.enabled, "D4: off by default");
            jail.set_env("GATEWAY_DPOP__ENABLED", "true");
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            assert!(cfg.dpop.enabled);
            assert!(cfg.validate().is_ok());
            Ok(())
        });
    }

    #[test]
    fn a_misspelt_dpop_key_fails_extraction() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", &format!("{}\n[dpop]\nenable = true\n", valid_toml()))?;
            assert!(GatewayConfig::figment().extract::<GatewayConfig>().is_err(), "a typo must not leave DPoP silently off");
            Ok(())
        });
    }

    /// D17: the Redis URL can carry a password, and `GatewayConfig` derives Debug and Serialize.
    #[test]
    fn the_redis_url_never_reaches_debug_or_serialize() {
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", valid_toml())?;
            jail.set_env("GATEWAY_LIMITS__BACKEND", "redis");
            jail.set_env("GATEWAY_LIMITS__REDIS_URL", "rediss://:hunter2@redis.internal:6380/2");
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            let serialized = serde_json::to_string(&cfg).expect("the config serializes");
            assert!(!serialized.contains("hunter2") && !serialized.contains("redis.internal"), "{serialized}");
            let debug = format!("{cfg:?}");
            assert!(!debug.contains("hunter2") && !debug.contains("redis.internal"), "{debug}");
            Ok(())
        });
    }

    // --- SMA-752 `[tool_policy]` ---------------------------------------------------------------

    fn tp_toml(body: &str) -> String {
        format!("{}\n[tool_policy]\n{body}\n", valid_toml())
    }

    /// A valid enabled `[tool_policy]` body plus the `[tool_interception]` it needs.
    fn tp_enabled_toml(extra: &str) -> String {
        format!(
            "{}\n[tool_interception]\nenabled = true\n\n[tool_policy]\nenabled = true\non_policy_unavailable = \"closed\"\nsource = {{ kind = \"file\", dir = \"/etc/policies\" }}\n{extra}\n",
            valid_toml()
        )
    }

    fn tp_load(toml: &str) -> Result<GatewayConfig, String> {
        let mut out = Err("unset".to_string());
        figment::Jail::expect_with(|jail| {
            jail.create_file("gateway.toml", toml)?;
            out = GatewayConfig::figment().extract::<GatewayConfig>().map_err(|e| e.to_string());
            Ok(())
        });
        out
    }

    #[test]
    fn tool_policy_is_off_by_default() {
        let cfg = tp_load(valid_toml()).expect("loads");
        let tp = &cfg.tool_policy;
        assert!(!tp.enabled);
        assert_eq!(tp.default_mode, TenantModeConfig::Off);
        assert_eq!((tp.refresh_secs, tp.max_stale_secs, tp.idle_evict_secs, tp.load_timeout_ms), (30, 300, 3600, 1000));
        assert!(tp.on_policy_unavailable.is_none() && tp.source.is_none() && tp.nats.is_none() && tp.org.is_empty());
        assert!(cfg.validate().is_ok());
        let s = tp.settings();
        assert_eq!(s.default_mode, TenantMode::Off);
        assert_eq!(
            (s.refresh, s.max_stale, s.idle_evict, s.load_timeout),
            (Duration::from_secs(30), Duration::from_secs(300), Duration::from_secs(3600), Duration::from_millis(1000))
        );
    }

    #[test]
    fn a_full_enabled_table_loads_and_validates() {
        let cfg = tp_load(&tp_enabled_toml(
            "default_mode = \"log_only\"\n[tool_policy.nats]\nurl = \"tls://x:4222\"\ncreds_file = \"/c\"\nca_file = \"/ca\"",
        ))
        .expect("loads");
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
        assert_eq!(cfg.tool_policy.settings().default_mode, TenantMode::LogOnly);
        assert_eq!(cfg.tool_policy.source, Some(PolicySourceConfig::File { dir: "/etc/policies".to_string() }));
        let json = serde_json::to_string(&cfg.tool_policy).expect("serialises");
        assert!(!json.contains("x:4222"), "the NATS url must not be serialised");
    }

    #[test]
    fn a_ca_file_with_a_plaintext_nats_url_is_rejected() {
        let cfg = tp_load(&tp_enabled_toml("default_mode = \"log_only\"\n[tool_policy.nats]\nurl = \"nats://x:4222\"\nca_file = \"/ca\"")).expect("loads");
        let err = cfg.validate().expect_err("ca_file implies require_tls");
        assert!(err.contains("tool_policy.nats.ca_file requires a tls:// url"), "{err}");
    }

    #[test]
    fn a_plaintext_nats_url_without_a_ca_file_still_validates() {
        let cfg = tp_load(&tp_enabled_toml("default_mode = \"log_only\"\n[tool_policy.nats]\nurl = \"nats://x:4222\"")).expect("loads");
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
    }

    #[test]
    fn enabled_without_on_policy_unavailable_fails() {
        let toml = format!(
            "{}\n[tool_interception]\nenabled = true\n[tool_policy]\nenabled = true\nsource = {{ kind = \"file\", dir = \"/p\" }}\n",
            valid_toml()
        );
        let err = tp_load(&toml).expect("loads").validate().expect_err("D9");
        assert!(err.contains("tool_policy.on_policy_unavailable"), "{err}");
    }

    #[test]
    fn enabled_without_tool_interception_fails() {
        let toml = format!(
            "{}\n[tool_policy]\nenabled = true\non_policy_unavailable = \"open\"\nsource = {{ kind = \"file\", dir = \"/p\" }}\n",
            valid_toml()
        );
        let err = tp_load(&toml).expect("loads").validate().expect_err("needs interception");
        assert!(err.contains("tool_interception.enabled"), "{err}");
    }

    #[test]
    fn enabled_without_a_source_fails() {
        let toml = format!(
            "{}\n[tool_interception]\nenabled = true\n[tool_policy]\nenabled = true\non_policy_unavailable = \"open\"\n",
            valid_toml()
        );
        let err = tp_load(&toml).expect("loads").validate().expect_err("needs a source");
        assert!(err.contains("tool_policy.source"), "{err}");
    }

    #[test]
    fn the_required_when_enabled_rules_do_not_apply_when_off() {
        let cfg = tp_load(&tp_toml("default_mode = \"enforce\"")).expect("loads");
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn an_unknown_key_fails_extraction() {
        for body in [
            "defualt_mode = \"off\"".to_string(),
            "[[tool_policy.org]]\nid = \"0190a100-0000-7000-8000-0000000000a1\"\nmode = \"off\"\nmdoe = \"off\"".to_string(),
            "[tool_policy.nats]\nurl = \"nats://x\"\nurl2 = \"y\"".to_string(),
            "source = { kind = \"file\", dir = \"/p\", extra = 1 }".to_string(),
            "floor_mode = \"enforce\"".to_string(),
            "source = { kind = \"s3\", dir = \"/p\" }".to_string(),
        ] {
            assert!(tp_load(&tp_toml(&body)).is_err(), "must fail: {body}");
        }
    }

    // --- SMA-775 the IAM policy source ---------------------------------------------------------

    fn tp_iam_toml(iam: &str, extra: &str) -> String {
        format!(
            "{}\n[iam]\n{iam}\n[tool_interception]\nenabled = true\n\n[tool_policy]\nenabled = true\non_policy_unavailable = \"closed\"\nsource = {{ kind = \"iam\" }}\n{extra}\n",
            valid_toml()
        )
    }

    #[test]
    fn the_iam_source_loads_validates_and_takes_the_settings_from_the_source() {
        let cfg = tp_load(&tp_iam_toml("service_credential_file = \"/run/secrets/gw-key\"", "")).expect("loads");
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
        assert_eq!(cfg.tool_policy.source, Some(PolicySourceConfig::Iam {}));
        assert_eq!(cfg.iam.service_credential_file.as_deref(), Some("/run/secrets/gw-key"));
        assert!(cfg.tool_policy.settings().settings_from_source);
    }

    #[test]
    fn the_file_source_and_no_source_do_not_take_the_settings_from_the_source() {
        let cfg = tp_load(&tp_enabled_toml("")).expect("loads");
        assert!(!cfg.tool_policy.settings().settings_from_source);
        assert!(!tp_load(valid_toml()).expect("loads").tool_policy.settings().settings_from_source);
    }

    #[test]
    fn the_iam_source_needs_a_credential_file() {
        let err = tp_load(&tp_iam_toml("", "")).expect("loads").validate().expect_err("needs a file");
        assert!(err.contains("iam.service_credential_file"), "{err}");
        for bad in ["service_credential_file = \"  \"", "service_credential_file = \" /k\""] {
            let err = tp_load(&tp_iam_toml(bad, "")).expect("loads").validate().expect_err("bad path");
            assert!(err.contains("iam.service_credential_file"), "{err}");
        }
    }

    #[test]
    fn the_iam_source_refuses_org_entries() {
        let org = "[[tool_policy.org]]\nid = \"0190a100-0000-7000-8000-0000000000a1\"\nmode = \"enforce\"";
        let err = tp_load(&tp_iam_toml("service_credential_file = \"/k\"", org)).expect("loads").validate().expect_err("org entries");
        assert!(err.contains("tool_policy.org") && err.contains("iam"), "{err}");
    }

    #[test]
    fn the_iam_source_refuses_org_entries_also_when_disabled() {
        let org = "[[tool_policy.org]]\nid = \"0190a100-0000-7000-8000-0000000000a1\"\nmode = \"enforce\"";
        let toml = tp_iam_toml("service_credential_file = \"/k\"", org).replace("enabled = true\n\n[tool_policy]\nenabled = true", "enabled = true\n\n[tool_policy]\nenabled = false");
        assert!(toml.contains("enabled = false"));
        let err = tp_load(&toml).expect("loads").validate().expect_err("org entries");
        assert!(err.contains("tool_policy.org") && err.contains("iam"), "{err}");
    }

    #[test]
    fn a_blank_or_padded_credential_file_fails_also_when_disabled() {
        for bad in ["service_credential_file = \"  \"", "service_credential_file = \" /k\""] {
            let toml = format!("{}\n[iam]\n{bad}\n", valid_toml());
            let err = tp_load(&toml).expect("loads").validate().expect_err("bad path");
            assert!(err.contains("iam.service_credential_file"), "{err}");
        }
        // "Required" still waits for `enabled`.
        assert!(tp_load(valid_toml()).expect("loads").validate().is_ok());
    }

    #[test]
    fn a_blank_source_dir_fails_when_enabled() {
        let toml = format!(
            "{}\n[tool_interception]\nenabled = true\n[tool_policy]\nenabled = true\non_policy_unavailable = \"open\"\nsource = {{ kind = \"file\", dir = \"  \" }}\n",
            valid_toml()
        );
        let err = tp_load(&toml).expect("loads").validate().expect_err("blank dir");
        assert!(err.contains("tool_policy.source.dir"), "{err}");
    }

    #[test]
    fn a_blank_nats_value_fails() {
        for (body, key) in [
            ("[tool_policy.nats]\nurl = \"  \"", "tool_policy.nats.url"),
            ("[tool_policy.nats]\nurl = \"nats://x\"\ncreds_file = \"\"", "tool_policy.nats.creds_file"),
            ("[tool_policy.nats]\nurl = \"nats://x\"\nca_file = \" \"", "tool_policy.nats.ca_file"),
        ] {
            let err = tp_load(&tp_toml(body)).expect("loads").validate().expect_err(body);
            assert!(err.contains(key), "{body}: {err}");
        }
    }

    #[test]
    fn the_nats_url_never_reaches_debug() {
        let cfg = tp_load(&tp_toml("[tool_policy.nats]\nurl = \"nats://user:hunter2@n:4222\"")).expect("loads");
        assert!(!format!("{cfg:?}").contains("hunter2"), "the NATS url must not appear in Debug output");
    }

    #[test]
    fn a_duplicate_org_fails() {
        let body = "[[tool_policy.org]]\nid = \"0190a100-0000-7000-8000-0000000000a1\"\nmode = \"off\"\n[[tool_policy.org]]\nid = \"0190a100-0000-7000-8000-0000000000a1\"\nmode = \"enforce\"";
        let err = tp_load(&tp_toml(body)).expect("loads").validate().expect_err("duplicate");
        assert!(err.contains("tool_policy.org[1].id"), "{err}");
    }

    #[test]
    fn a_bad_org_id_fails() {
        for id in ["acme", "0190a1000000700080000000000000a1"] {
            let body = format!("[[tool_policy.org]]\nid = \"{id}\"\nmode = \"off\"");
            let err = tp_load(&tp_toml(&body)).expect("loads").validate().expect_err("bad id");
            assert!(err.contains("tool_policy.org[0].id must be one organization UUID"), "{err}");
        }
    }

    /// Review Focus 5: the entry id and the scope PRN may use different letter case.
    #[test]
    fn an_upper_case_org_id_applies_to_the_canonical_org() {
        let body = "[[tool_policy.org]]\nid = \"0190A100-0000-7000-8000-0000000000A1\"\nmode = \"enforce\"";
        let cfg = tp_load(&tp_toml(body)).expect("loads");
        assert!(cfg.validate().is_ok());
        let lower = Uuid::try_parse(ORG_A).expect("a uuid");
        assert_eq!(cfg.tool_policy.settings().tenant(lower).0, TenantMode::Enforce);
        // A lower-case second entry for the same org is a duplicate.
        let dup = format!("{body}\n[[tool_policy.org]]\nid = \"{ORG_A}\"\nmode = \"off\"");
        let err = tp_load(&tp_toml(&dup)).expect("loads").validate().expect_err("case-only duplicate");
        assert!(err.contains("tool_policy.org[1].id"), "{err}");
    }

    #[test]
    fn a_per_org_override_wins() {
        let other = "0190a100-0000-7000-8000-0000000000b2";
        let body = format!(
            "default_mode = \"log_only\"\non_policy_unavailable = \"closed\"\n[[tool_policy.org]]\nid = \"{ORG_A}\"\nmode = \"enforce\"\non_policy_unavailable = \"open\"\n[[tool_policy.org]]\nid = \"{other}\"\nmode = \"off\""
        );
        let cfg = tp_load(&tp_toml(&body)).expect("loads");
        assert!(cfg.validate().is_ok());
        let s = cfg.tool_policy.settings();
        assert_eq!(s.tenant(Uuid::try_parse(ORG_A).expect("uuid")), (TenantMode::Enforce, OnUnavailable::Open));
        assert_eq!(
            s.tenant(Uuid::try_parse(other).expect("uuid")),
            (TenantMode::Off, OnUnavailable::Closed),
            "no override: the global value"
        );
        assert_eq!(
            s.tenant(Uuid::nil()),
            (TenantMode::LogOnly, OnUnavailable::Closed),
            "unlisted org: the default mode and the global value"
        );
    }

    #[test]
    fn load_timeout_must_be_shorter_than_decide_timeout() {
        let enabled = |decide: u64| {
            format!(
                "{}\n[tool_interception]\nenabled = true\ndecide_timeout_secs = {decide}\n[tool_policy]\nenabled = true\non_policy_unavailable = \"closed\"\nsource = {{ kind = \"file\", dir = \"/p\" }}\nload_timeout_ms = 5000\n",
                valid_toml()
            )
        };
        let cfg = tp_load(&enabled(5)).expect("loads");
        let err = cfg.validate().expect_err("D31");
        assert!(err.contains("tool_policy.load_timeout_ms"), "{err}");
        let ok = tp_load(&enabled(6)).expect("loads");
        assert!(ok.validate().is_ok(), "{:?}", ok.validate());
    }

    #[test]
    fn a_disabled_table_does_not_constrain_decide_timeout() {
        // A config that is valid on main: decide_timeout_secs = 1 and no [tool_policy] table. The default
        // load_timeout_ms (1000) is not less than 1000 ms, but the cross-table rule holds only when enabled.
        let cfg = tp_load(&format!("{}\n[tool_interception]\ndecide_timeout_secs = 1\n", valid_toml())).expect("loads");
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
        let cfg = tp_load(&format!("{}\n[tool_interception]\nenabled = true\ndecide_timeout_secs = 1\n", valid_toml())).expect("loads");
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
    }

    #[test]
    fn bounds() {
        for (body, key) in [
            ("refresh_secs = 0", "tool_policy.refresh_secs must be at least 1"),
            ("max_stale_secs = 0", "tool_policy.max_stale_secs must be at least 1"),
            ("idle_evict_secs = 0", "tool_policy.idle_evict_secs must be at least 1"),
            ("load_timeout_ms = 0", "tool_policy.load_timeout_ms must be at least 1"),
            ("refresh_secs = 86401", "tool_policy.refresh_secs must be at most 86400"),
            ("max_stale_secs = 86401", "tool_policy.max_stale_secs must be at most 86400"),
            ("idle_evict_secs = 86401", "tool_policy.idle_evict_secs must be at most 86400"),
            ("load_timeout_ms = 86400001", "tool_policy.load_timeout_ms must be at most"),
            ("refresh_secs = 400", "tool_policy.max_stale_secs (300) must not be less than tool_policy.refresh_secs (400)"),
        ] {
            let err = tp_load(&tp_toml(body)).expect("loads").validate().expect_err(body);
            assert!(err.contains(key), "{body}: {err}");
        }
    }

    // --- SMA-855 `[tool_policy.detectors]` -------------------------------------------------------

    /// A TOML that enables tool policy with the given `decide_timeout_secs` and detector body.
    fn det_toml(decide: u64, body: &str) -> String {
        format!(
            "{}\n[tool_interception]\nenabled = true\ndecide_timeout_secs = {decide}\n\n[tool_policy]\nenabled = true\non_policy_unavailable = \"closed\"\nsource = {{ kind = \"file\", dir = \"/p\" }}\n\n[tool_policy.detectors]\n{body}\n",
            valid_toml()
        )
    }

    #[test]
    fn detectors_defaults_pass() {
        let cfg = tp_load(&tp_enabled_toml("")).expect("loads");
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
        let s = cfg.tool_policy.detector_settings(cfg.default_detector_scan_bytes());
        assert!(s.enabled);
        assert_eq!(s.timeout, Duration::from_millis(500));
        assert_eq!(s.max_scan_bytes, 16_777_216);
        assert_eq!(s.margin, Duration::from_millis(DETECTOR_MARGIN_MS));
        // The default scan budget covers the largest request cap of any route (S12, R10).
        assert!(s.max_scan_bytes >= cfg.responses.max_request_bytes);
    }

    #[test]
    fn detectors_disabled_loads_and_validates() {
        let cfg = tp_load(&det_toml(5, "enabled = false")).expect("loads");
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
        assert!(!cfg.tool_policy.detector_settings(cfg.default_detector_scan_bytes()).enabled);
    }

    #[test]
    fn timeout_equal_to_decide_timeout_fails() {
        let cfg = tp_load(&det_toml(2, "timeout_ms = 2000")).expect("loads");
        let err = cfg.validate().expect_err("D35");
        assert!(err.contains("tool_policy.detectors.timeout_ms"), "{err}");
        assert!(err.contains("tool_interception.decide_timeout_secs"), "{err}");
    }

    #[test]
    fn timeout_inside_the_margin_fails_and_at_the_margin_passes() {
        let decide_ms = 3_000_u64;
        let inside = decide_ms - DETECTOR_MARGIN_MS + 1;
        let cfg = tp_load(&det_toml(3, &format!("timeout_ms = {inside}"))).expect("loads");
        let err = cfg.validate().expect_err("inside the margin");
        assert!(err.contains("tool_policy.detectors.timeout_ms"), "{err}");
        let exact = decide_ms - DETECTOR_MARGIN_MS;
        let cfg = tp_load(&det_toml(3, &format!("timeout_ms = {exact}"))).expect("loads");
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
    }

    #[test]
    fn the_margin_rule_holds_only_when_tool_policy_is_enabled() {
        let toml = format!("{}\n[tool_interception]\ndecide_timeout_secs = 1\n\n[tool_policy.detectors]\ntimeout_ms = 5000\n", valid_toml());
        let cfg = tp_load(&toml).expect("loads");
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
    }

    #[test]
    fn an_unknown_detectors_key_fails() {
        let err = tp_load(&det_toml(5, "typo = 1")).expect_err("deny_unknown_fields");
        assert!(err.contains("typo"), "{err}");
    }

    #[test]
    fn small_scan_budget_and_zero_timeout_fail() {
        let cfg = tp_load(&det_toml(5, "max_scan_bytes = 65535")).expect("loads");
        let err = cfg.validate().expect_err("scan budget");
        assert!(err.contains("tool_policy.detectors.max_scan_bytes"), "{err}");
        let cfg = tp_load(&det_toml(5, "max_scan_bytes = 65536")).expect("loads");
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
        let cfg = tp_load(&det_toml(5, "timeout_ms = 0")).expect("loads");
        let err = cfg.validate().expect_err("zero timeout");
        assert!(err.contains("tool_policy.detectors.timeout_ms"), "{err}");
    }

    #[test]
    fn the_example_file_parses() {
        let example = include_str!("../gateway.toml.example");
        let start = example.find("# [tool_policy.detectors]").expect("the example holds the detectors block");
        let block: Vec<String> = example[start..]
            .lines()
            .take_while(|l| l.starts_with('#') && !l.trim_start_matches('#').trim().is_empty() || l.trim() == "#")
            .map(|l| l.strip_prefix("# ").or_else(|| l.strip_prefix('#')).unwrap_or(l).to_string())
            .collect();
        let cfg = tp_load(&format!("{}\n{}\n", valid_toml(), block.join("\n"))).expect("the uncommented block parses");
        let s = cfg.tool_policy.detector_settings(cfg.default_detector_scan_bytes());
        assert!(s.enabled);
        assert_eq!(s.timeout, Duration::from_millis(500));
        assert_eq!(s.max_scan_bytes, 16_777_216);
    }

    // --- SMA-856 `[[tool_policy.detectors.webhook]]` and the secret loader --------------------------

    const WH_ORG_A: &str = "6f1c0000-0000-4000-8000-000000000001";
    const WH_ORG_B: &str = "6f1c0000-0000-4000-8000-000000000002";

    /// `det_toml(5, "timeout_ms = 2000")` plus one valid webhook table. `fields` replaces a key (matched by its name).
    fn wh_toml(fields: &str) -> String {
        let mut lines: Vec<String> = vec![
            "name = \"acme_dlp\"".to_string(),
            "url = \"https://dlp.acme.example/v1/findings\"".to_string(),
            format!("orgs = [\"{WH_ORG_A}\"]"),
            "secret_env = \"ACME_DLP_WEBHOOK_SECRET\"".to_string(),
            "kinds = [\"pii_email\", \"pii_iban\"]".to_string(),
        ];
        for extra in fields.lines().filter(|l| !l.trim().is_empty()) {
            let key = extra.split('=').next().unwrap_or("").trim().to_owned();
            lines.retain(|l| l.split('=').next().unwrap_or("").trim() != key);
            lines.push(extra.to_owned());
        }
        format!("{}\n[[tool_policy.detectors.webhook]]\n{}\n", det_toml(5, "timeout_ms = 2000"), lines.join("\n"))
    }

    fn env_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn a_valid_webhook_table_loads_with_its_defaults() {
        let cfg = tp_load(&wh_toml("")).expect("loads");
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
        let w = &cfg.tool_policy.detectors.webhook[0];
        assert_eq!((w.timeout_ms, w.max_request_bytes, w.max_in_flight, w.retired), (300, 1_048_576, 64, false));
        assert_eq!(Severity::from(w.failure_severity), Severity::High);
        assert_eq!(w.kind_set().len(), 2);
        assert_eq!(w.org_scope(), OrgScope::Only(BTreeSet::from([parse_org_uuid(WH_ORG_A).unwrap()])));
    }

    #[test]
    fn each_webhook_rule_fails_and_names_the_provider_and_the_field() {
        let cases: &[(&str, &str)] = &[
            ("name = \"Acme\"", "webhook[0].name"),
            ("name = \"kernel\"", "webhook[0].name"),
            ("name = \"webhook\"", "webhook[0].name"),
            ("name = \"judge\"", "webhook[0].name"),
            ("name = \"other\"", "webhook[0].name"),
            ("name = \"built_in\"", "webhook[0].name"),
            ("name = \"secret_in_args\"", "webhook[0].name"),
            ("url = \"http://dlp.acme.example/x\"", "webhook[acme_dlp].url"),
            ("url = \"https://u:p@dlp.acme.example/x\"", "webhook[acme_dlp].url"),
            ("url = \"https://dlp.acme.example/x?a=1\"", "webhook[acme_dlp].url"),
            ("url = \"https://dlp.acme.example/x#frag\"", "webhook[acme_dlp].url"),
            ("url = \"not a url\"", "webhook[acme_dlp].url"),
            ("orgs = []", "webhook[acme_dlp].orgs"),
            ("orgs = \"all\"", "webhook[acme_dlp].orgs"),
            ("orgs = [\"not-a-uuid\"]", "webhook[acme_dlp].orgs"),
            ("secret_env = \"GATEWAY_X_SECRET\"", "webhook[acme_dlp].secret_env"),
            ("secret_env = \"lower\"", "webhook[acme_dlp].secret_env"),
            ("previous_secret_env = \"1BAD\"", "webhook[acme_dlp].previous_secret_env"),
            ("timeout_ms = 0", "webhook[acme_dlp].timeout_ms"),
            ("timeout_ms = 1951", "webhook[acme_dlp].timeout_ms"),
            ("kinds = []", "webhook[acme_dlp].kinds"),
            ("kinds = [\"secret_in_args\"]", "webhook[acme_dlp].kinds"),
            ("kinds = [\"other\"]", "webhook[acme_dlp].kinds"),
            ("kinds = [\"a\", \"a\"]", "webhook[acme_dlp].kinds"),
            ("kinds = [\"Bad\"]", "webhook[acme_dlp].kinds"),
            ("max_request_bytes = 0", "webhook[acme_dlp].max_request_bytes"),
            ("max_request_bytes = 16777217", "webhook[acme_dlp].max_request_bytes"),
            ("max_in_flight = 0", "webhook[acme_dlp].max_in_flight"),
            ("max_in_flight = 1025", "webhook[acme_dlp].max_in_flight"),
            ("extra_ca_bundle_path = \"\"", "webhook[acme_dlp].extra_ca_bundle_path"),
            ("extra_ca_bundle_path = \" /x\"", "webhook[acme_dlp].extra_ca_bundle_path"),
        ];
        for (field, want) in cases {
            let err = tp_load(&wh_toml(field)).and_then(|c| c.validate()).expect_err(field);
            assert!(err.contains(want), "{field}: {err}");
        }
    }

    #[test]
    fn the_webhook_limits_hold_at_their_edges() {
        for field in [
            "timeout_ms = 1950",
            "timeout_ms = 1",
            "max_request_bytes = 16777216",
            "max_in_flight = 1024",
            "failure_severity = \"info\"",
            "retired = true",
            "extra_ca_bundle_path = \"/etc/ca.pem\"",
            "secret_env = \"A\"",
        ] {
            let cfg = tp_load(&wh_toml(field)).expect(field);
            assert!(cfg.validate().is_ok(), "{field}: {:?}", cfg.validate());
        }
        let sixteen: Vec<String> = (0..16).map(|i| format!("\"k{i}\"")).collect();
        let cfg = tp_load(&wh_toml(&format!("kinds = [{}]", sixteen.join(",")))).expect("loads");
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
    }

    #[test]
    fn too_many_kinds_providers_names_and_urls_fail() {
        let seventeen: Vec<String> = (0..17).map(|i| format!("\"k{i}\"")).collect();
        let err = tp_load(&wh_toml(&format!("kinds = [{}]", seventeen.join(",")))).and_then(|c| c.validate()).expect_err("17 kinds");
        assert!(err.contains("webhook[acme_dlp].kinds"), "{err}");

        let table = |name: &str, url: &str| {
            format!(
                "\n[[tool_policy.detectors.webhook]]\nname = \"{name}\"\nurl = \"{url}\"\norgs = \"*\"\nsecret_env = \"S_{}\"\nkinds = [\"k_{name}\"]\n",
                name.to_uppercase()
            )
        };
        let base = det_toml(5, "timeout_ms = 2000");
        let nine: String = (0..9).map(|i| table(&format!("p{i}"), &format!("https://p{i}.example/x"))).collect();
        let err = tp_load(&format!("{base}{nine}")).and_then(|c| c.validate()).expect_err("9 providers");
        assert!(err.contains("at most 8"), "{err}");
        let eight: String = (0..8).map(|i| table(&format!("p{i}"), &format!("https://p{i}.example/x"))).collect();
        let cfg = tp_load(&format!("{base}{eight}")).expect("loads");
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());

        let dup_name = format!("{base}{}{}", table("p0", "https://a.example/x"), table("p0", "https://b.example/x"));
        let err = tp_load(&dup_name).and_then(|c| c.validate()).expect_err("duplicate name");
        assert!(err.contains("webhook[1].name") && err.contains("p0"), "{err}");
        let dup_url = format!("{base}{}{}", table("p0", "https://a.example/x"), table("p1", "https://a.example/x"));
        let err = tp_load(&dup_url).and_then(|c| c.validate()).expect_err("duplicate url");
        assert!(err.contains("webhook[p1].url"), "{err}");
    }

    #[test]
    fn http_is_accepted_only_for_a_loopback_host() {
        for url in ["http://127.0.0.1:9/x", "http://localhost:9/x"] {
            let cfg = tp_load(&wh_toml(&format!("url = \"{url}\""))).expect("loads");
            assert!(cfg.validate().is_ok(), "{url}: {:?}", cfg.validate());
        }
        let err = tp_load(&wh_toml("url = \"http://10.0.0.1/x\"")).and_then(|c| c.validate()).expect_err("non-loopback http");
        assert!(err.contains("webhook[acme_dlp].url"), "{err}");
    }

    #[test]
    fn orgs_star_is_a_shared_provider() {
        let cfg = tp_load(&wh_toml("orgs = \"*\"")).expect("loads");
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
        assert_eq!(cfg.tool_policy.detectors.webhook[0].org_scope(), OrgScope::All);
    }

    #[test]
    fn the_webhook_table_is_validated_when_the_detectors_are_off() {
        let toml = wh_toml("name = \"Acme\"").replace("[tool_policy.detectors]\ntimeout_ms = 2000", "[tool_policy.detectors]\nenabled = false\ntimeout_ms = 2000");
        assert!(toml.contains("enabled = false"));
        let err = tp_load(&toml).and_then(|c| c.validate()).expect_err("still validated");
        assert!(err.contains("webhook[0].name"), "{err}");
    }

    #[test]
    fn the_timeout_bound_follows_the_detectors_timeout() {
        let with = |detectors_ms: u64, provider_ms: u64| {
            let toml = wh_toml(&format!("timeout_ms = {provider_ms}")).replace("timeout_ms = 2000", &format!("timeout_ms = {detectors_ms}"));
            tp_load(&toml).and_then(|c| c.validate())
        };
        assert!(with(500, 450).is_ok(), "{:?}", with(500, 450));
        let err = with(500, 451).expect_err("451 > 450");
        assert!(err.contains("webhook[acme_dlp].timeout_ms"), "{err}");
    }

    #[test]
    fn an_unknown_webhook_key_fails() {
        let err = tp_load(&wh_toml("typo = 1")).expect_err("deny_unknown_fields");
        assert!(err.contains("typo"), "{err}");
    }

    #[test]
    fn org_webhooks_must_name_a_registered_permitted_provider() {
        let org = |id: &str, webhooks: &str| format!("\n[[tool_policy.org]]\nid = \"{id}\"\nmode = \"enforce\"\nwebhooks = {webhooks}\n");
        let load = |fields: &str, org_entry: &str| tp_load(&format!("{}{org_entry}", wh_toml(fields)));
        let err = load("", &org(WH_ORG_A, "[\"nobody\"]")).and_then(|c| c.validate()).expect_err("unregistered");
        assert!(err.contains("tool_policy.org[0].webhooks"), "{err}");
        let err = load(&format!("orgs = [\"{WH_ORG_B}\"]"), &org(WH_ORG_A, "[\"acme_dlp\"]"))
            .and_then(|c| c.validate())
            .expect_err("not permitted");
        assert!(err.contains("tool_policy.org[0].webhooks"), "{err}");
        let cfg = load("", &org(WH_ORG_A, "[\"acme_dlp\"]")).expect("loads");
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
        let cfg = load("orgs = \"*\"", &org(WH_ORG_A, "[\"acme_dlp\"]")).expect("loads");
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
        let cfg = load("", &org(WH_ORG_A, "[\"acme_dlp\", \"acme_dlp\"]")).expect("loads");
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
        let map = cfg.tool_policy.org_webhooks();
        assert_eq!(map.len(), 1);
        assert_eq!(&*map[&parse_org_uuid(WH_ORG_A).unwrap()], ["acme_dlp".to_string()]);
    }

    #[test]
    fn the_secret_loader_checks_each_secret() {
        let ok = "k".repeat(32);
        let cfg = tp_load(&wh_toml("previous_secret_env = \"ACME_DLP_WEBHOOK_SECRET_OLD\"")).unwrap();
        let got = load_webhook_secrets(&cfg, &env_of(&[("ACME_DLP_WEBHOOK_SECRET", &ok), ("ACME_DLP_WEBHOOK_SECRET_OLD", &ok)])).unwrap();
        assert_eq!(got["acme_dlp"].current.expose_secret(), ok);
        assert_eq!(got["acme_dlp"].previous.as_ref().map(|p| p.expose_secret().to_owned()), Some(ok.clone()));
        for (value, why) in [
            (None, "is not set"),
            (Some("k".repeat(31)), "at least 32 bytes"),
            (Some(format!("{ok}\n")), "whitespace"),
            (Some(format!(" {ok}")), "whitespace"),
        ] {
            let e = value.map_or_else(HashMap::new, |v| env_of(&[("ACME_DLP_WEBHOOK_SECRET", &v), ("ACME_DLP_WEBHOOK_SECRET_OLD", &ok)]));
            let err = load_webhook_secrets(&cfg, &e).expect_err(why);
            assert!(err.contains("webhook[acme_dlp].secret_env") && err.contains(why), "{err}");
            assert!(!err.contains(&ok), "the error holds no secret");
        }
        // A bad previous secret names `previous_secret_env`.
        let err = load_webhook_secrets(&cfg, &env_of(&[("ACME_DLP_WEBHOOK_SECRET", &ok)])).expect_err("missing previous");
        assert!(err.contains("webhook[acme_dlp].previous_secret_env") && err.contains("is not set"), "{err}");
        let short = "p".repeat(5);
        let err = load_webhook_secrets(&cfg, &env_of(&[("ACME_DLP_WEBHOOK_SECRET", &ok), ("ACME_DLP_WEBHOOK_SECRET_OLD", &short)])).expect_err("short previous");
        assert!(err.contains("previous_secret_env") && !err.contains(&short), "{err}");
    }

    #[test]
    fn the_secret_loader_gives_one_entry_per_provider_and_a_secret_has_no_debug_value() {
        let ok = "canary-secret-value-0123456789abcdefXYZ";
        let cfg = tp_load(&wh_toml("")).unwrap();
        let got = load_webhook_secrets(&cfg, &env_of(&[("ACME_DLP_WEBHOOK_SECRET", ok)])).unwrap();
        assert_eq!(got.keys().collect::<Vec<_>>(), ["acme_dlp"]);
        assert!(got["acme_dlp"].previous.is_none());
        let dbg = format!("{:?}", got["acme_dlp"]);
        assert!(!dbg.contains("canary") && !dbg.contains(ok), "{dbg}");
        let none = load_webhook_secrets(&tp_load(valid_toml()).unwrap(), &HashMap::new()).unwrap();
        assert!(none.is_empty());
    }

    #[test]
    fn the_process_env_reader_reads_the_process_environment() {
        assert!(ProcessEnv.var("PAIGASUS_SMA856_SURELY_UNSET_VARIABLE").is_none());
        assert!(ProcessEnv.var("PATH").is_some());
    }

    #[test]
    fn the_readiness_dump_holds_no_secret_and_no_path() {
        let secret = "canary-secret-value-0123456789abcdefXYZ";
        let cfg = tp_load(&wh_toml("url = \"https://dlp.acme.example/v1/canary-path-77\"\nextra_ca_bundle_path = \"/etc/canary-ca.pem\"")).unwrap();
        assert!(cfg.validate().is_ok());
        let w = &cfg.tool_policy.detectors.webhook[0];
        assert_eq!(w.parsed_url().host_str(), Some("dlp.acme.example"));
        let json = serde_json::to_string(&cfg).unwrap();
        let dbg = format!("{cfg:?}");
        for text in [&json, &dbg] {
            assert!(text.contains("acme_dlp") && text.contains("dlp.acme.example"), "{text}");
            assert!(!text.contains("canary-path-77") && !text.contains("canary-ca.pem") && !text.contains(secret), "{text}");
            assert!(!text.contains("ACME_DLP_WEBHOOK_SECRET"), "the env var name is not dumped either: {text}");
        }
        assert!(json.contains("\"orgs\":1"), "{json}");
        let star = tp_load(&wh_toml("orgs = \"*\"")).unwrap();
        assert!(serde_json::to_string(&star).unwrap().contains("\"orgs\":\"*\""));
    }

    #[test]
    fn the_example_webhook_block_parses() {
        let example = include_str!("../gateway.toml.example");
        let start = example.find("# [[tool_policy.detectors.webhook]]").expect("the example holds the webhook block");
        let block: Vec<String> = example[start..]
            .lines()
            .take_while(|l| l.starts_with('#') && !l.trim_start_matches('#').trim().is_empty() || l.trim() == "#")
            .map(|l| l.strip_prefix("# ").or_else(|| l.strip_prefix('#')).unwrap_or(l).to_string())
            .collect();
        let cfg = tp_load(&format!("{}\n[tool_policy.detectors]\ntimeout_ms = 500\n\n{}\n", valid_toml(), block.join("\n"))).expect("the uncommented block parses");
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
        let list = &cfg.tool_policy.detectors.webhook;
        assert_eq!(list.len(), 1);
        let w = &list[0];
        assert_eq!((w.name.as_str(), w.timeout_ms, w.max_in_flight, w.retired), ("acme_dlp", 300, 64, false));
        assert_eq!(Severity::from(w.failure_severity), Severity::High);
    }

    // --- SMA-750 `[upstream.anthropic]` and `[messages]` ----------------------------------------

    fn anthropic_toml(extra: &str) -> String {
        format!("{}\n[upstream.anthropic]\napi_key = \"sk-ant-test\"\n{extra}\n", valid_toml())
    }

    #[test]
    fn anthropic_and_messages_defaults_match_the_spec() {
        let cfg = tp_load(&anthropic_toml("")).expect("loads");
        let a = cfg.upstream.anthropic.as_ref().expect("the table is present");
        assert_eq!(a.base_url, "https://api.anthropic.com");
        assert_eq!(a.default_version, "2023-06-01");
        assert_eq!(a.non_stream_timeout_secs, 600);
        assert!(a.extra_ca_bundle_path.is_none());
        assert_eq!(
            cfg.messages,
            MessagesConfig {
                enabled: false,
                max_request_bytes: 33_554_432,
                max_record_bytes: 8_388_608,
                max_held_bytes: 33_554_432,
                max_hold_secs: 600,
            }
        );
        assert!(cfg.validate().is_ok(), "{:?}", cfg.validate());
        let bare = tp_load(valid_toml()).expect("loads");
        assert!(bare.upstream.anthropic.is_none());
        assert!(bare.validate().is_ok());
    }

    #[test]
    fn the_anthropic_key_loads_from_the_environment_only() {
        figment::Jail::expect_with(|jail| {
            jail.set_env("GATEWAY_UPSTREAM__ANTHROPIC__API_KEY", "sk-ant-env");
            jail.create_file("gateway.toml", valid_toml())?;
            let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
            let a = cfg.upstream.anthropic.as_ref().expect("the env key makes the table");
            assert_eq!(a.api_key.expose_secret(), "sk-ant-env");
            assert_eq!(a.base_url, "https://api.anthropic.com");
            assert!(cfg.validate().is_ok());
            let json = serde_json::to_string(&cfg).expect("serializes");
            assert!(!json.contains("sk-ant-env"), "the key must not be serialized");
            let v: serde_json::Value = serde_json::from_str(&json).expect("json");
            assert!(v["upstream"]["anthropic"].get("api_key").is_none());
            Ok(())
        });
    }

    #[test]
    fn an_empty_anthropic_key_fails() {
        let cfg = tp_load(&anthropic_toml("").replace("sk-ant-test", "")).expect("loads");
        let err = cfg.validate().expect_err("empty key");
        assert!(err.contains("upstream.anthropic.api_key"), "{err}");
    }

    #[test]
    fn an_anthropic_key_that_is_not_a_header_value_fails_without_printing_it() {
        // A trailing newline (a Kubernetes secret made from a file) is the usual cause. The key
        // is not trimmed: trimming would hide a fault in the secret store.
        for bad in ["sk-ant-secret-body\\n", "sk-ant-secret-body\\r", "sk-ant-\\u0001-secret-body"] {
            let cfg = tp_load(&anthropic_toml("").replace("sk-ant-test", bad)).expect("loads");
            let err = cfg.validate().expect_err("invalid header value");
            assert!(err.contains("upstream.anthropic.api_key"), "{err}");
            assert!(err.contains("invalid character"), "{err}");
            assert!(!err.contains("secret-body"), "the key must not appear in the error: {err}");
        }
    }

    #[test]
    fn an_anthropic_base_url_must_be_absolute_with_a_host() {
        for bad in [
            "api.anthropic.com",
            "/v1",
            "",
            "https://",
            "ftp://api.anthropic.com",
            "http:",
            "https://proxy.internal/anthropic#gateway",
            "https://proxy.internal/anthropic?x=1",
            "https://proxy.internal/anthropic?",
            "https://proxy.internal/anthropic#",
        ] {
            let cfg = tp_load(&anthropic_toml(&format!("base_url = \"{bad}\""))).expect("loads");
            let err = cfg.validate().expect_err(bad);
            assert!(err.contains("upstream.anthropic.base_url"), "{bad}: {err}");
        }
        for good in ["https://api.anthropic.com", "http://127.0.0.1:8080", "https://proxy.internal/anthropic"] {
            let cfg = tp_load(&anthropic_toml(&format!("base_url = \"{good}\""))).expect("loads");
            cfg.validate().unwrap_or_else(|e| panic!("{good}: {e}"));
        }
    }

    #[test]
    fn an_invalid_default_version_fails() {
        let cfg = tp_load(&anthropic_toml("default_version = \"2023\\n06\"")).expect("loads");
        let err = cfg.validate().expect_err("bad header value");
        assert!(err.contains("upstream.anthropic.default_version"), "{err}");
    }

    #[test]
    fn messages_without_the_anthropic_upstream_fails() {
        let cfg = tp_load(&format!("{}\n[messages]\nenabled = true\n", valid_toml())).expect("loads");
        let err = cfg.validate().expect_err("no upstream");
        assert!(err.contains("upstream.anthropic"), "{err}");
        let ok = tp_load(&format!("{}\n[messages]\nenabled = true\n", anthropic_toml(""))).expect("loads");
        assert!(ok.validate().is_ok(), "{:?}", ok.validate());
    }

    #[test]
    fn messages_validation_names_the_key() {
        for (body, key) in [
            ("max_request_bytes = 0", "messages.max_request_bytes"),
            ("max_record_bytes = 0", "messages.max_record_bytes"),
            ("max_held_bytes = 0", "messages.max_held_bytes"),
            ("max_hold_secs = 0", "messages.max_hold_secs"),
            ("max_record_bytes = 9000000\nmax_held_bytes = 8000000", "messages.max_record_bytes"),
            ("max_hold_secs = 86401", "messages.max_hold_secs"),
        ] {
            let cfg = tp_load(&format!("{}\n[messages]\n{body}\n", anthropic_toml(""))).expect("loads");
            let err = cfg.validate().expect_err(body);
            assert!(err.contains(key), "{body}: {err}");
        }
        let ok = tp_load(&format!("{}\n[messages]\nmax_hold_secs = 86400\n", anthropic_toml(""))).expect("loads");
        assert!(ok.validate().is_ok());
        let cfg = tp_load(&anthropic_toml("non_stream_timeout_secs = 0")).expect("loads");
        let err = cfg.validate().expect_err("zero timeout");
        assert!(err.contains("upstream.anthropic.non_stream_timeout_secs"), "{err}");
    }

    #[test]
    fn the_anthropic_ca_path_follows_the_openai_rule() {
        for body in ["extra_ca_bundle_path = \"\"", "extra_ca_bundle_path = \"/etc/ca.pem\\n\""] {
            let cfg = tp_load(&anthropic_toml(body)).expect("loads");
            let err = cfg.validate().expect_err(body);
            assert!(err.contains("upstream.anthropic.extra_ca_bundle_path"), "{err}");
        }
        let cfg = tp_load(&anthropic_toml("extra_ca_bundle_path = \"/etc/ca.pem\"")).expect("loads");
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn an_unknown_messages_key_fails() {
        let err = tp_load(&format!("{}\n[messages]\nenabeld = true\n", anthropic_toml(""))).expect_err("typo");
        assert!(err.contains("enabeld"), "{err}");
        let err = tp_load(&anthropic_toml("bogus = 1")).expect_err("typo");
        assert!(err.contains("bogus"), "{err}");
    }

    #[test]
    fn the_detector_default_covers_every_route_cap() {
        let cfg = tp_load(&tp_enabled_toml("")).expect("loads");
        assert_eq!(cfg.default_detector_scan_bytes(), 16_777_216);
        assert_eq!(cfg.tool_policy.detector_settings(cfg.default_detector_scan_bytes()).max_scan_bytes, 16_777_216);
        let on = tp_load(&format!("{}\n[upstream.anthropic]\napi_key = \"k\"\n[messages]\nenabled = true\n", tp_enabled_toml(""))).expect("loads");
        assert_eq!(on.default_detector_scan_bytes(), 33_554_432);
        assert_eq!(on.tool_policy.detector_settings(on.default_detector_scan_bytes()).max_scan_bytes, 33_554_432);
        // `[messages]` off does not count its cap, even with a larger value.
        let off = tp_load(&format!("{}\n[messages]\nmax_request_bytes = 67108864\n", tp_enabled_toml(""))).expect("loads");
        assert_eq!(off.default_detector_scan_bytes(), 16_777_216);
        // An explicit value wins.
        let explicit = tp_load(&format!(
            "{}\n[upstream.anthropic]\napi_key = \"k\"\n[messages]\nenabled = true\n[tool_policy.detectors]\nmax_scan_bytes = 100000\n",
            tp_enabled_toml("")
        ))
        .expect("loads");
        assert_eq!(explicit.tool_policy.detector_settings(explicit.default_detector_scan_bytes()).max_scan_bytes, 100_000);
    }

    // --- SMA-755 `[decision_log]` ---------------------------------------------------------------

    const DL_KEY: &str = "0123456789abcdef0123456789abcdef";

    fn dl_full_body() -> String {
        format!(
            "enabled = true\nredis_url = \"redis://:hunter2@redis.internal:6379\"\ndigest_key = \"{DL_KEY}\"\ndigest_key_id = \"2026-11\"\n\n[decision_log.nats]\nurl = \"nats://:natspw@nats.internal:4222\"\n"
        )
    }

    fn dl_toml(body: &str) -> String {
        format!("{}\n[decision_log]\n{body}\n", tp_enabled_toml(""))
    }

    fn dl_validate(body: &str) -> Result<(), String> {
        tp_load(&dl_toml(body))?.validate()
    }

    #[test]
    fn the_decision_log_is_off_by_default() {
        let cfg = tp_load(valid_toml()).expect("loads");
        assert!(!cfg.decision_log.enabled);
        assert_eq!(cfg.decision_log.key_prefix, "paigasus:gateway");
        assert_eq!(cfg.decision_log.stream_max_len, 100_000);
        assert_eq!(cfg.decision_log.queue_capacity, 4096);
        assert!(cfg.decision_log.digest_key().is_none());
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn enabled_without_the_tool_policy_is_refused() {
        let cfg = tp_load(&format!("{}\n[decision_log]\n{}", valid_toml(), dl_full_body())).expect("loads");
        let err = cfg.validate().unwrap_err();
        assert!(err.contains("decision_log.enabled") && err.contains("tool_policy"), "{err}");
    }

    #[test]
    fn enabled_needs_redis_url_digest_key_key_id_and_nats() {
        assert!(dl_validate(&dl_full_body()).is_ok());
        let nats = "[decision_log.nats]\nurl = \"nats://n:4222\"";
        let cases = [
            ("decision_log.redis_url", format!("enabled = true\ndigest_key = \"{DL_KEY}\"\ndigest_key_id = \"k\"\n{nats}")),
            ("decision_log.digest_key ", format!("enabled = true\nredis_url = \"redis://r:6379\"\ndigest_key_id = \"k\"\n{nats}")),
            (
                "decision_log.digest_key_id",
                format!("enabled = true\nredis_url = \"redis://r:6379\"\ndigest_key = \"{DL_KEY}\"\n{nats}"),
            ),
            (
                "decision_log.nats",
                format!("enabled = true\nredis_url = \"redis://r:6379\"\ndigest_key = \"{DL_KEY}\"\ndigest_key_id = \"k\""),
            ),
        ];
        for (key, body) in cases {
            let err = dl_validate(&body).unwrap_err();
            assert!(err.contains(key), "{key}: {err}");
        }
    }

    #[test]
    fn a_digest_key_under_32_bytes_is_refused() {
        let err = dl_validate(&dl_full_body().replace(DL_KEY, "short")).unwrap_err();
        assert!(err.contains("decision_log.digest_key"), "{err}");
        let cfg = tp_load(&dl_toml(&dl_full_body())).expect("loads");
        assert!(cfg.decision_log.digest_key().is_some());
    }

    #[test]
    fn zero_capacity_and_zero_max_len_are_refused() {
        let err = dl_validate("queue_capacity = 0").unwrap_err();
        assert!(err.contains("decision_log.queue_capacity"), "{err}");
        let err = dl_validate("stream_max_len = 0").unwrap_err();
        assert!(err.contains("decision_log.stream_max_len"), "{err}");
    }

    #[test]
    fn an_empty_or_padded_key_prefix_is_refused() {
        for v in ["", " ", " paigasus", "paigasus "] {
            let err = dl_validate(&format!("key_prefix = {v:?}")).unwrap_err();
            assert!(err.contains("decision_log.key_prefix"), "{v:?}: {err}");
        }
        assert!(dl_validate("key_prefix = \"tenant-a:gateway\"").is_ok());
    }

    #[test]
    fn a_misspelt_decision_log_key_fails_extraction() {
        assert!(tp_load(&dl_toml("stream_maxlen = 1")).is_err());
        assert!(tp_load(&dl_toml("[decision_log.nats]\nurl = \"nats://n:4222\"\ncredentials_file = \"x\"")).is_err());
    }

    #[test]
    fn the_decision_log_secrets_do_not_serialize_and_debug_is_redacted() {
        let cfg = tp_load(&dl_toml(&dl_full_body())).expect("loads");
        let serialized = serde_json::to_string(&cfg).expect("serializes");
        let debug = format!("{cfg:?}");
        for text in [&serialized, &debug] {
            for secret in ["hunter2", "redis.internal", DL_KEY, "natspw", "nats.internal"] {
                assert!(!text.contains(secret), "{secret} leaked: {text}");
            }
        }
    }
}
