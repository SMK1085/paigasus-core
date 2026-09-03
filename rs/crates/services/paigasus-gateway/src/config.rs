// SPDX-License-Identifier: Apache-2.0

//! Service configuration via figment: built-in defaults < `gateway.toml` < `GATEWAY_*` env.

use figment::providers::{Env, Format, Serialized, Toml};
use figment::{Figment, error::Error as FigmentError};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

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
}

/// The IAM gRPC client endpoint G4 dials (`Introspect`/authorization calls). `tls` governs
/// how that channel is secured — see [`IamTlsConfig`]'s doc for the D8 rationale.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct IamClientConfig {
    pub grpc_addr: String,
    pub tls: IamTlsConfig,
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
    /// compiled-in Mozilla roots, the image's own store, AND every certificate here. The
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
/// loopback such as `::1`). Deliberately a small hand-rolled parser rather than pulling in a
/// `url` crate dependency for a single boot-time check: `grpc_addr` is `scheme://host[:port]`,
/// optionally with a trailing path; strip the scheme, strip any path, then split the host from
/// an optional port (bracketed for IPv6 literals).
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

    let after_scheme = grpc_addr.split("://").last().unwrap_or(grpc_addr);
    let host_port = after_scheme.split('/').next().unwrap_or(after_scheme);
    let host = if let Some(rest) = host_port.strip_prefix('[') {
        // IPv6 literal, e.g. "[::1]:9090" or "[::1]" -> "::1" (brackets stripped so the
        // `Ipv6Addr` parse below succeeds).
        rest.split(']').next().unwrap_or(rest)
    } else {
        host_port.rsplit_once(':').map_or(host_port, |(h, _)| h)
    };

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

        Ok(())
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
}
