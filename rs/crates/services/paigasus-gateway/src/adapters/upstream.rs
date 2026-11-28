// SPDX-License-Identifier: Apache-2.0

//! The upstream HTTP-client helper shared by the OpenAI and the Anthropic egress clients
//! (SMA-750 D2).
//!
//! It holds the response and error types, the `reqwest::Client` builder with the extra CA bundle
//! (SMA-558, SMA-570) and the request-time error classification. The config key and the client
//! name in every error and log text come from [`Provider`].
//!
//! ## No redirects (D16)
//! The base builder sets `redirect(Policy::none())`. A `3xx` answer reaches the caller as a status,
//! never as a followed request. The gateway holds a real provider key, so a redirect to a third
//! host must never carry that key. The clients turn a `3xx` into [`UpstreamError::Redirect`], and
//! `Location` is never forwarded.

use std::time::Duration;

use bytes::Bytes;
use reqwest::StatusCode;
use reqwest::header::{HeaderMap, HeaderValue};

/// The upstream provider a client talks to. It names the client and the config key in texts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    OpenAi,
    Anthropic,
}

impl Provider {
    /// The config key of the provider's extra CA bundle.
    pub fn ca_key(self) -> &'static str {
        match self {
            Provider::OpenAi => "upstream.openai.extra_ca_bundle_path",
            Provider::Anthropic => "upstream.anthropic.extra_ca_bundle_path",
        }
    }
}

impl std::fmt::Display for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Provider::OpenAi => "OpenAI",
            Provider::Anthropic => "Anthropic",
        })
    }
}

/// The upstream response headers that the gateway passes on (SMA-750 D8). The other upstream
/// headers are dropped, and `location` is never kept.
#[derive(Debug, Clone, Default)]
pub struct ResponseHead {
    pub request_id: Option<HeaderValue>,
    pub retry_after: Option<HeaderValue>,
    pub should_retry: Option<HeaderValue>,
    /// The upstream `content-type`. `apply` never writes it.
    pub content_type: Option<HeaderValue>,
}

impl ResponseHead {
    /// Read the D8 headers from an upstream response.
    pub fn from_headers(headers: &HeaderMap) -> Self {
        Self {
            request_id: headers.get("request-id").cloned(),
            retry_after: headers.get(reqwest::header::RETRY_AFTER).cloned(),
            should_retry: headers.get("x-should-retry").cloned(),
            content_type: headers.get(reqwest::header::CONTENT_TYPE).cloned(),
        }
    }

    /// Insert `request-id`, `retry-after` and `x-should-retry` when present. Never `content-type`.
    pub fn apply(&self, headers: &mut HeaderMap) {
        if let Some(v) = &self.request_id {
            headers.insert("request-id", v.clone());
        }
        if let Some(v) = &self.retry_after {
            headers.insert(reqwest::header::RETRY_AFTER, v.clone());
        }
        if let Some(v) = &self.should_retry {
            headers.insert("x-should-retry", v.clone());
        }
    }
}

/// The boxed, UNBUFFERED chunk stream the [`UpstreamResponse::Stream`] path yields. Each item is a
/// raw upstream body chunk as it arrived off the socket. `'static + Send` so axum can own it.
/// Dropping it cancels the upstream request (cancel-on-drop).
pub type UpstreamByteStream = futures::stream::BoxStream<'static, Result<Bytes, StreamReadError>>;

/// An error item of an [`UpstreamByteStream`]. Every relay treats each variant the same way: it
/// sends its terminal event and ends the stream. So a caller sees no difference between them.
#[derive(Debug, thiserror::Error)]
pub enum StreamReadError {
    /// reqwest failed to read the body. This includes its own read timeout.
    #[error("failed to read the upstream stream body")]
    Read(#[from] reqwest::Error),
    /// No chunk arrived within the stream idle timeout (SMA-750 § 5).
    #[error("no upstream stream chunk arrived within {0:?}")]
    Idle(Duration),
}

/// The upstream response. A NON-2xx upstream is NOT an error: it arrives with the upstream status
/// and body, so the caller forwards the provider's own error envelope verbatim. For a non-stream
/// request it is [`UpstreamResponse::Full`]. For a `stream:true` request it is
/// [`UpstreamResponse::Stream`], also for a `4xx`/`5xx`. A caller must check the status in
/// either variant. A `3xx` is the exception (D16): see [`UpstreamError`].
pub enum UpstreamResponse {
    /// Non-stream: the upstream status and the fully-buffered response body.
    Full {
        /// The upstream HTTP status.
        status: StatusCode,
        /// The D8 response headers and the upstream `content-type`.
        head: ResponseHead,
        /// The complete upstream response body.
        body: Bytes,
    },
    /// Stream: the upstream status and an UNBUFFERED chunk stream (SSE), forwarded as-is.
    Stream {
        /// The upstream HTTP status.
        status: StatusCode,
        /// The D8 response headers and the upstream `content-type`.
        head: ResponseHead,
        /// The unbuffered upstream byte stream; dropping it cancels the request.
        stream: UpstreamByteStream,
    },
}

impl std::fmt::Debug for UpstreamResponse {
    // Manual `Debug`: the stream is not `Debug`, and printing it would consume the body.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UpstreamResponse::Full { status, body, .. } => f.debug_struct("UpstreamResponse::Full").field("status", status).field("body_len", &body.len()).finish(),
            UpstreamResponse::Stream { status, .. } => f.debug_struct("UpstreamResponse::Stream").field("status", status).finish_non_exhaustive(),
        }
    }
}

/// Errors from an egress client: reqwest build/send/connect/read failures, a refused redirect and
/// a CA-bundle fault. A non-2xx upstream is NOT an error.
#[derive(Debug, thiserror::Error)]
pub enum UpstreamError {
    /// The `reqwest::Client` could not be built (TLS backend init, invalid builder config). An
    /// empty or unparseable platform trust store is a reachable cause (SMA-558 D1).
    #[error("failed to build the {provider} HTTP client — this can also mean the platform trust store contains no parseable certificates")]
    Build { provider: Provider, source: reqwest::Error },
    /// A configured header value is not valid for an HTTP header. The message never holds the value.
    #[error("the {provider} {setting} is not a valid HTTP header value")]
    InvalidHeaderValue { provider: Provider, setting: &'static str },
    /// Failed to establish the connection to the upstream (DNS / TCP / TLS handshake).
    #[error("failed to connect to the {provider} upstream")]
    Connect { provider: Provider, source: reqwest::Error },
    /// A configured timeout fired (connect, first-byte, or idle-between-bytes).
    #[error("the {provider} upstream request timed out")]
    Timeout { provider: Provider, source: reqwest::Error },
    /// Any other transport-level failure talking to the upstream.
    #[error("transport error talking to the {provider} upstream")]
    Transport { provider: Provider, source: reqwest::Error },
    /// The upstream answered `3xx`. The gateway never follows a redirect (D16).
    #[error("the {provider} upstream answered with a redirect ({status}), which the gateway does not follow")]
    Redirect { provider: Provider, status: StatusCode },
    /// The configured CA bundle could not be read, parsed, or contained no certificates. A
    /// boot-time fault like `Build`.
    #[error("failed to load {} {path:?}", .provider.ca_key())]
    CaBundle {
        provider: Provider,
        path: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// Classify a request-time `reqwest::Error` (from `send`/`bytes`) into timeout / connect /
/// transport. NOT a `From` impl on purpose: a client-*build* error must not be silently classified
/// as a transport error.
pub fn classify(provider: Provider, e: reqwest::Error) -> UpstreamError {
    if e.is_timeout() {
        UpstreamError::Timeout { provider, source: e }
    } else if e.is_connect() {
        UpstreamError::Connect { provider, source: e }
    } else {
        UpstreamError::Transport { provider, source: e }
    }
}

/// The `source` of a `CaBundle` error raised when the bundle's certificates parse as PEM but not
/// as DER. Holds the original `reqwest::Error` as its own `source`, so anyhow renders the whole
/// chain and callers keep the ability to downcast.
#[derive(Debug, thiserror::Error)]
#[error(
    "contains a structurally invalid certificate: it decodes as base64 but is not valid DER. \
     A control client built without it succeeded, so the platform trust store is not the cause"
)]
struct InvalidBundleCertificate {
    #[source]
    source: reqwest::Error,
}

/// The `source` of a `CaBundle` error raised when the control build ALSO failed, so the platform
/// trust store is broken too. Typed for the same reason as [`InvalidBundleCertificate`].
#[derive(Debug, thiserror::Error)]
#[error(
    "the platform trust store also contains no parseable certificates, which is the more likely \
     cause — fix that first, then re-verify this bundle"
)]
struct BundleAndStoreUnusable {
    #[source]
    source: reqwest::Error,
}

/// What a failed `reqwest::Client` build can be attributed to. Mirrors
/// `paigasus-iam/src/adapters/oidc/jwks.rs`'s copy; see SMA-558 D7.
#[derive(Debug)]
enum Attribution<'a> {
    NoBundle,
    Bundle { path: &'a str },
    BundleAndStore { path: &'a str },
}

/// Decides what a `build()` failure should be blamed on (SMA-570 D1). `control_build_ok` builds a
/// client with the SAME options but no added anchors, and is called ONLY when a bundle is
/// configured.
///
/// Success proves the store did not cause THIS failure. Failure names BOTH, because the added
/// anchors are loaded first and a bad one fails the build before the store is consulted.
fn attribute_build_failure<'a>(bundle: Option<&'a str>, control_build_ok: impl FnOnce() -> bool) -> Attribution<'a> {
    match bundle {
        None => Attribution::NoBundle,
        Some(path) => {
            if control_build_ok() {
                Attribution::Bundle { path }
            } else {
                Attribution::BundleAndStore { path }
            }
        }
    }
}

/// Renders `err` and its full `source()` chain as `"outer: middle: inner"`. reqwest's build error
/// `Display` is the bare string `"builder error"`, so the cause is only reachable this way.
fn describe_error(err: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(e) = source {
        parts.push(e.to_string());
        source = e.source();
    }
    parts.join(": ")
}

/// The client's non-TLS options shared by every egress client, in ONE place so the control build
/// differs from the real one by exactly the added anchors (SMA-570 D6). `ClientBuilder` is not
/// `Clone`, hence a function.
fn plain_builder(connect_timeout: Duration) -> reqwest::ClientBuilder {
    // reqwest 0.13 `rustls-no-provider` panics at `build()` unless a provider is installed. Every
    // build goes through here (SMA-809). `Err` means one is already installed: fine.
    let _ = rustls::crypto::ring::default_provider().install_default();
    // D16: never follow a redirect; the clients turn a `3xx` into `UpstreamError::Redirect`.
    reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).connect_timeout(connect_timeout)
}

/// The options of an upstream client: [`plain_builder`] and the idle read timeout.
fn base_builder(connect_timeout: Duration, stream_idle_timeout: Duration) -> reqwest::ClientBuilder {
    plain_builder(connect_timeout).read_timeout(stream_idle_timeout)
}

fn log_loaded(provider: Provider, path: &str, count: usize) {
    match provider {
        Provider::OpenAi => tracing::info!(path = %path, count, "loaded extra upstream trust anchors from upstream.openai.extra_ca_bundle_path"),
        Provider::Anthropic => tracing::info!(path = %path, count, "loaded extra upstream trust anchors from upstream.anthropic.extra_ca_bundle_path"),
    }
}

fn log_build_failed(provider: Provider, path: &str, attribution: &str, cause: &str) {
    match provider {
        Provider::OpenAi => tracing::error!(path = %path, attribution, cause = %cause, "OpenAI HTTP client build failed"),
        Provider::Anthropic => tracing::error!(path = %path, attribution, cause = %cause, "Anthropic HTTP client build failed"),
    }
}

/// Why [`build_extra_ca_client`] failed. A boot-time fault. `label` names the config key that holds
/// the bundle path (for a webhook: `tool_policy.detectors.webhook[<name>].extra_ca_bundle_path`).
#[derive(Debug, thiserror::Error)]
pub enum ExtraCaError {
    /// The `reqwest::Client` could not be built, with no bundle configured. An empty or
    /// unparseable platform trust store is a reachable cause (SMA-558 D1).
    #[error("failed to build the HTTP client for {label} — this can also mean the platform trust store contains no parseable certificates")]
    Build { label: String, source: reqwest::Error },
    /// The bundle could not be read, parsed or built into a client, or it held no certificates.
    #[error("failed to load {label} {path:?}")]
    CaBundle {
        label: String,
        path: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

/// Build a `reqwest::Client` with no redirects, a connect timeout, the options of `configure` and
/// the optional extra CA bundle. The upstream clients and the webhook clients share it. `label` is
/// the config key of the bundle path, for the error text.
///
/// # Errors
/// [`ExtraCaError`] when the bundle is unreadable, malformed or empty, or when the build fails.
pub fn build_extra_ca_client(
    label: &str,
    extra_ca_bundle_path: Option<&str>,
    connect_timeout: Duration,
    configure: impl Fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder,
) -> Result<reqwest::Client, ExtraCaError> {
    build_extra_ca_client_with_control(label, extra_ca_bundle_path, connect_timeout, &configure, || configure(plain_builder(connect_timeout)).build().is_ok()).map(|(client, _loaded)| client)
}

/// [`build_extra_ca_client`] with the control build injected, so both attribution arms are
/// reachable in tests without mutating `SSL_CERT_FILE` (`unsafe` in edition 2024) or depending on
/// the host's trust store. It also returns the number of added anchors (0 with no bundle).
pub(crate) fn build_extra_ca_client_with_control(
    label: &str,
    extra_ca_bundle_path: Option<&str>,
    connect_timeout: Duration,
    configure: impl Fn(reqwest::ClientBuilder) -> reqwest::ClientBuilder,
    control_build_ok: impl FnOnce() -> bool,
) -> Result<(reqwest::Client, usize), ExtraCaError> {
    let mut builder = configure(plain_builder(connect_timeout));
    let mut loaded = 0;

    if let Some(path) = extra_ca_bundle_path {
        let ca_bundle = |source: Box<dyn std::error::Error + Send + Sync>| ExtraCaError::CaBundle {
            label: label.to_string(),
            path: path.to_string(),
            source,
        };

        let pem = std::fs::read(path).map_err(|e| ca_bundle(Box::new(e)))?;
        // `from_pem_bundle`, NOT `from_pem`: a bundle may carry more than one ROOT and `from_pem`
        // reads only the first. Roots only; see the config field docs.
        let certs = reqwest::Certificate::from_pem_bundle(&pem).map_err(|e| ca_bundle(Box::new(e)))?;
        // Ok(vec![]) is what a DER .crt, a key-only PEM or an empty file parses to, so without
        // this the likeliest operator mistake boots green having added nothing.
        if certs.is_empty() {
            return Err(ca_bundle(
                "contained no PEM certificates — a DER file, a key-only PEM or an empty file parses as an empty bundle".into(),
            ));
        }
        loaded = certs.len();
        for cert in certs {
            builder = builder.add_root_certificate(cert);
        }
    }

    match builder.build() {
        Ok(client) => Ok((client, loaded)),
        Err(e) => Err(match attribute_build_failure(extra_ca_bundle_path, control_build_ok) {
            Attribution::NoBundle => ExtraCaError::Build { label: label.to_string(), source: e },
            Attribution::Bundle { path } => ExtraCaError::CaBundle {
                label: label.to_string(),
                path: path.to_string(),
                source: Box::new(InvalidBundleCertificate { source: e }),
            },
            Attribution::BundleAndStore { path } => ExtraCaError::CaBundle {
                label: label.to_string(),
                path: path.to_string(),
                source: Box::new(BundleAndStoreUnusable { source: e }),
            },
        }),
    }
}

/// Build the shared `reqwest::Client` for `provider`: no redirects, split timeouts (connect and
/// idle between bytes; the first-byte budget is per request) and the optional extra CA bundle.
/// NO global `.timeout()` is set, because it would cap a legitimate long stream.
pub fn build_client(provider: Provider, extra_ca_bundle_path: Option<&str>, connect_timeout: Duration, read_timeout: Duration) -> Result<reqwest::Client, UpstreamError> {
    build_client_with_control(provider, extra_ca_bundle_path, connect_timeout, read_timeout, || {
        base_builder(connect_timeout, read_timeout).build().is_ok()
    })
}

/// `build_client` with the control build injected (see [`build_extra_ca_client_with_control`]).
pub(crate) fn build_client_with_control(
    provider: Provider,
    extra_ca_bundle_path: Option<&str>,
    connect_timeout: Duration,
    read_timeout: Duration,
    control_build_ok: impl FnOnce() -> bool,
) -> Result<reqwest::Client, UpstreamError> {
    let built = build_extra_ca_client_with_control(provider.ca_key(), extra_ca_bundle_path, connect_timeout, |b| b.read_timeout(read_timeout), control_build_ok);
    match built {
        Ok((client, loaded)) => {
            if let Some(path) = extra_ca_bundle_path {
                log_loaded(provider, path, loaded);
            }
            Ok(client)
        }
        Err(ExtraCaError::Build { source, .. }) => Err(UpstreamError::Build { provider, source }),
        Err(ExtraCaError::CaBundle { path, source, .. }) => {
            // `cause` carries the rendered chain: reqwest's build-error `Display` is the bare
            // string "builder error", and a log aggregator indexes this field.
            if let Some(e) = source.downcast_ref::<InvalidBundleCertificate>() {
                log_build_failed(provider, &path, "bundle", &describe_error(&e.source));
            } else if let Some(e) = source.downcast_ref::<BundleAndStoreUnusable>() {
                log_build_failed(provider, &path, "bundle_and_store", &describe_error(&e.source));
            }
            Err(UpstreamError::CaBundle { provider, path, source })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const TEN: Duration = Duration::from_secs(10);
    const THREE_HUNDRED: Duration = Duration::from_secs(300);

    fn client_with_bundle(extra_ca_bundle_path: Option<String>) -> Result<reqwest::Client, UpstreamError> {
        build_client(Provider::OpenAi, extra_ca_bundle_path.as_deref(), TEN, THREE_HUNDRED)
    }

    fn tmp_file_with(contents: &[u8]) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().expect("temp file");
        f.write_all(contents).expect("write");
        f.flush().expect("flush");
        f
    }

    // ---- extra_ca_bundle_path plumbing (SMA-558 D6) ------------------------------------------
    // These prove the gateway's OWN wiring: the config field reaches reqwest and each failure
    // mode maps to CaBundle. They deliberately prove NOTHING about whether a handshake against a
    // private-CA upstream succeeds — `reqwest::Client` exposes no trust-store accessor, so no
    // test here could observe that. IAM's `tests/authn_private_ca.rs` proves it once for the
    // shared reqwest mechanism.

    #[test]
    fn valid_ca_bundle_builds_the_client() {
        let f = tmp_file_with(test_ca_pem().as_bytes());
        client_with_bundle(Some(f.path().to_str().unwrap().to_string())).expect("a valid bundle must build");
    }

    #[test]
    fn missing_ca_bundle_path_is_a_build_error() {
        let err = client_with_bundle(Some("/nonexistent/paigasus-sma558/ca.pem".to_string())).expect_err("a nonexistent bundle path must fail");
        assert!(matches!(err, UpstreamError::CaBundle { .. }), "expected CaBundle, got {err:?}");
        assert!(
            format!("{err:?}").contains("NotFound"),
            "the error must be attributable to a missing file, not another CaBundle cause: {err:?}"
        );
    }

    #[test]
    fn certificate_free_ca_bundle_is_a_build_error() {
        // `from_pem_bundle` returns Ok(vec![]) for a file with no CERTIFICATE section, so only an
        // explicit is_empty() check catches this.
        let f = tmp_file_with(b"-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIA==\n-----END PRIVATE KEY-----\n");
        let err = client_with_bundle(Some(f.path().to_str().unwrap().to_string())).expect_err("a certificate-free bundle must fail");
        assert!(matches!(err, UpstreamError::CaBundle { .. }), "expected CaBundle, got {err:?}");
        assert!(format!("{err:?}").contains("no PEM certificates"), "the error must say the bundle was empty: {err:?}");
    }

    #[test]
    fn undecodable_ca_bundle_is_a_build_error() {
        let f = tmp_file_with(b"-----BEGIN CERTIFICATE-----\n!!!not base64!!!\n-----END CERTIFICATE-----\n");
        let err = client_with_bundle(Some(f.path().to_str().unwrap().to_string())).expect_err("an undecodable bundle must fail");
        assert!(matches!(err, UpstreamError::CaBundle { .. }), "expected CaBundle, got {err:?}");
        assert!(format!("{err:?}").contains("invalid certificate encoding"), "the error must say the bundle failed to decode: {err:?}");
    }

    // ---- build-failure attribution (SMA-570) --------------------------------------------------
    // `AAAAAAAA` is valid base64 (six zero bytes) but not valid DER, so unlike the
    // `!!!not base64!!!` fixture above it PASSES from_pem_bundle and fails later in
    // builder.build() — a different reqwest code path, hence its own tests.
    const INVALID_DER_PEM: &[u8] = b"-----BEGIN CERTIFICATE-----\nAAAAAAAA\n-----END CERTIFICATE-----\n";

    fn client_with_bundle_and_control(extra_ca_bundle_path: Option<String>, control_build_ok: bool) -> Result<reqwest::Client, UpstreamError> {
        build_client_with_control(Provider::OpenAi, extra_ca_bundle_path.as_deref(), TEN, THREE_HUNDRED, || control_build_ok)
    }

    #[test]
    fn attribution_without_a_bundle_never_runs_the_control_build() {
        let probed = std::cell::Cell::new(false);
        let attribution = attribute_build_failure(None, || {
            probed.set(true);
            true
        });

        assert!(matches!(attribution, Attribution::NoBundle));
        assert!(!probed.get(), "a bundle-less failure must not pay for a second load_native_certs()");
    }

    #[test]
    fn attribution_blames_the_bundle_when_the_control_build_succeeds() {
        let attribution = attribute_build_failure(Some("/etc/paigasus/corp-ca.pem"), || true);
        assert!(matches!(attribution, Attribution::Bundle { path } if path == "/etc/paigasus/corp-ca.pem"));
    }

    #[test]
    fn attribution_names_both_when_the_control_build_also_fails() {
        let attribution = attribute_build_failure(Some("/etc/paigasus/corp-ca.pem"), || false);
        assert!(matches!(attribution, Attribution::BundleAndStore { path } if path == "/etc/paigasus/corp-ca.pem"));
    }

    #[test]
    fn the_build_variant_message_is_byte_unchanged() {
        // AC4. The no-bundle path still returns UpstreamError::Build, whose #[error] attribute is
        // untouched by SMA-570. Minted from a real failed build so the variant is exercised, not
        // just quoted.
        let bad = reqwest::Certificate::from_pem_bundle(INVALID_DER_PEM).expect("parses as one cert");
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut b = reqwest::Client::builder();
        for c in bad {
            b = b.add_root_certificate(c);
        }
        let reqwest_err = b.build().expect_err("an invalid-DER anchor must fail the build");

        assert_eq!(
            UpstreamError::Build {
                provider: Provider::OpenAi,
                source: reqwest_err
            }
            .to_string(),
            "failed to build the OpenAI HTTP client — this can also mean the platform trust store contains no parseable certificates"
        );
    }

    #[test]
    fn der_invalid_ca_bundle_names_the_config_key() {
        let f = tmp_file_with(INVALID_DER_PEM);
        let err = client_with_bundle_and_control(Some(f.path().to_str().unwrap().to_string()), true).expect_err("a structurally invalid certificate must fail the build");

        // Renders the chain the way anyhow's Debug does at the boot call site (main.rs), which is
        // what an operator actually reads — a bare `{err:?}` would show `InvalidBundleCertificate`'s
        // derived struct fields, never its `#[error(...)]` message.
        let rendered = describe_error(&err);
        assert!(matches!(err, UpstreamError::CaBundle { .. }), "expected CaBundle, got {err:?}");
        assert!(err.to_string().contains("upstream.openai.extra_ca_bundle_path"), "the error must name the config key: {err}");
        assert!(rendered.contains("not valid DER"), "the error must say what is wrong with it: {rendered}");
    }

    #[test]
    fn der_invalid_ca_bundle_with_a_broken_store_names_both() {
        let f = tmp_file_with(INVALID_DER_PEM);
        let err = client_with_bundle_and_control(Some(f.path().to_str().unwrap().to_string()), false).expect_err("a structurally invalid certificate must fail the build");

        let rendered = describe_error(&err);
        assert!(matches!(err, UpstreamError::CaBundle { .. }), "expected CaBundle, got {err:?}");
        assert!(rendered.contains("platform trust store"), "the store is the primary fault: {rendered}");
        assert!(rendered.contains("upstream.openai.extra_ca_bundle_path"), "the bundle must still be named: {rendered}");
        assert!(rendered.contains("fix that first"), "the operator needs an order of operations: {rendered}");
    }

    // ---- the webhook clients (SMA-856 § 7.5, PD16) ---------------------------------------------
    // Like the tests above, these prove the build and the error attribution, not trust itself.
    const WEBHOOK_LABEL: &str = "tool_policy.detectors.webhook[acme_dlp].extra_ca_bundle_path";

    fn webhook_client(path: Option<&str>) -> Result<reqwest::Client, ExtraCaError> {
        build_extra_ca_client(WEBHOOK_LABEL, path, TEN, |b| b.pool_max_idle_per_host(4).pool_idle_timeout(Duration::from_secs(90)))
    }

    #[test]
    fn a_webhook_client_with_an_rcgen_bundle_builds() {
        let f = tmp_file_with(test_ca_pem().as_bytes());
        webhook_client(Some(f.path().to_str().unwrap())).expect("a valid bundle must build");
        webhook_client(None).expect("no bundle builds");
    }

    #[test]
    fn a_bad_webhook_bundle_names_the_provider_and_the_path() {
        let key_only = tmp_file_with(b"-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIA==\n-----END PRIVATE KEY-----\n");
        let undecodable = tmp_file_with(b"-----BEGIN CERTIFICATE-----\n!!!not base64!!!\n-----END CERTIFICATE-----\n");
        let missing = "/nonexistent/paigasus-sma856/ca.pem".to_owned();
        let paths = [missing, key_only.path().to_str().unwrap().to_owned(), undecodable.path().to_str().unwrap().to_owned()];
        for path in &paths {
            let err = webhook_client(Some(path)).expect_err("a bad bundle must fail");
            assert!(matches!(err, ExtraCaError::CaBundle { .. }), "{err:?}");
            let text = err.to_string();
            assert!(text.contains(WEBHOOK_LABEL) && text.contains(&format!("{path:?}")), "{text}");
        }
    }

    #[test]
    fn the_webhook_bundle_attribution_follows_the_control_build() {
        let f = tmp_file_with(INVALID_DER_PEM);
        let path = f.path().to_str().unwrap();
        let build = |control: bool| build_extra_ca_client_with_control(WEBHOOK_LABEL, Some(path), TEN, |b| b, || control).map(|(c, _)| c);
        let only_bundle = build(true).expect_err("an invalid anchor must fail the build");
        let rendered = describe_error(&only_bundle);
        assert!(matches!(only_bundle, ExtraCaError::CaBundle { .. }), "{only_bundle:?}");
        assert!(
            rendered.contains("not valid DER") && rendered.contains(WEBHOOK_LABEL) && rendered.contains(&format!("{path:?}")),
            "{rendered}"
        );
        let both = build(false).expect_err("an invalid anchor must fail the build");
        let rendered = describe_error(&both);
        assert!(
            rendered.contains("platform trust store") && rendered.contains("fix that first") && rendered.contains(WEBHOOK_LABEL),
            "{rendered}"
        );
    }

    /// A throwaway CA certificate, minted fresh per test run. Used ONLY to prove the bundle path
    /// parses and reaches the builder — nothing here is ever trusted by a real handshake, and no
    /// gateway test starts a TLS listener (SMA-558 D6). Minted rather than committed so there is
    /// no fixture to rot, and none for a future reader to mistake for a real trust anchor.
    fn test_ca_pem() -> String {
        let mut params = rcgen::CertificateParams::new(Vec::new()).expect("ca params");
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        params.distinguished_name.push(rcgen::DnType::CommonName, "paigasus-gateway-test-ca");
        let key = rcgen::KeyPair::generate().expect("ca keypair");
        params.self_signed(&key).expect("self-signed ca").pem()
    }

    async fn dead_port_error() -> reqwest::Error {
        let _ = rustls::crypto::ring::default_provider().install_default();
        reqwest::Client::new()
            .get("http://127.0.0.1:1")
            .timeout(Duration::from_millis(50))
            .send()
            .await
            .expect_err("port 1 refuses")
    }

    #[tokio::test]
    async fn the_openai_error_texts_are_byte_unchanged() {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let p = Provider::OpenAi;
        let bad = reqwest::Certificate::from_pem_bundle(INVALID_DER_PEM).expect("parses as one cert");
        let mut b = reqwest::Client::builder();
        for c in bad {
            b = b.add_root_certificate(c);
        }
        let build_err = b.build().expect_err("an invalid-DER anchor must fail the build");
        assert_eq!(
            UpstreamError::Build { provider: p, source: build_err }.to_string(),
            "failed to build the OpenAI HTTP client — this can also mean the platform trust store contains no parseable certificates"
        );
        assert_eq!(
            UpstreamError::Connect {
                provider: p,
                source: dead_port_error().await
            }
            .to_string(),
            "failed to connect to the OpenAI upstream"
        );
        assert_eq!(
            UpstreamError::Timeout {
                provider: p,
                source: dead_port_error().await
            }
            .to_string(),
            "the OpenAI upstream request timed out"
        );
        assert_eq!(
            UpstreamError::Transport {
                provider: p,
                source: dead_port_error().await
            }
            .to_string(),
            "transport error talking to the OpenAI upstream"
        );
        assert_eq!(
            UpstreamError::CaBundle {
                provider: p,
                path: "/x/ca.pem".to_string(),
                source: "boom".into()
            }
            .to_string(),
            "failed to load upstream.openai.extra_ca_bundle_path \"/x/ca.pem\""
        );
        assert_eq!(
            UpstreamError::Redirect {
                provider: Provider::Anthropic,
                status: StatusCode::TEMPORARY_REDIRECT
            }
            .to_string(),
            "the Anthropic upstream answered with a redirect (307 Temporary Redirect), which the gateway does not follow"
        );
    }

    #[test]
    fn the_anthropic_ca_errors_name_the_anthropic_key() {
        let key = "upstream.anthropic.extra_ca_bundle_path";
        let missing = build_client(Provider::Anthropic, Some("/nonexistent/paigasus-sma750/ca.pem"), TEN, THREE_HUNDRED).expect_err("missing path");
        assert!(matches!(missing, UpstreamError::CaBundle { provider: Provider::Anthropic, .. }));
        let rendered = describe_error(&missing);
        assert!(rendered.contains(key) && !rendered.contains("upstream.openai"), "{rendered}");

        let f = tmp_file_with(INVALID_DER_PEM);
        let path = f.path().to_str().unwrap();
        for control in [true, false] {
            let err = build_client_with_control(Provider::Anthropic, Some(path), TEN, THREE_HUNDRED, || control).expect_err("invalid DER");
            let rendered = describe_error(&err);
            assert!(rendered.contains(key), "{rendered}");
            assert!(!rendered.contains("upstream.openai"), "{rendered}");
        }
    }

    #[tokio::test]
    async fn a_redirect_is_not_followed() {
        use axum::Router;
        use axum::http::{HeaderMap, HeaderValue, StatusCode as AxStatus};
        use axum::routing::any;

        let hits = Arc::new(AtomicUsize::new(0));
        let second = {
            let hits = hits.clone();
            Router::new().fallback(any(move || {
                let hits = hits.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    "second"
                }
            }))
        };
        let l2 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let second_addr = l2.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l2, second).await.unwrap() });

        let location = format!("http://{second_addr}/elsewhere");
        let first = Router::new().fallback(any(move || {
            let location = location.clone();
            async move {
                let mut h = HeaderMap::new();
                h.insert("location", HeaderValue::from_str(&location).unwrap());
                (AxStatus::TEMPORARY_REDIRECT, h)
            }
        }));
        let l1 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let first_addr = l1.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(l1, first).await.unwrap() });

        let client = build_client(Provider::OpenAi, None, TEN, THREE_HUNDRED).unwrap();
        let resp = client.post(format!("http://{first_addr}/v1/x")).body("{}").send().await.unwrap();
        assert_eq!(resp.status(), StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(hits.load(Ordering::SeqCst), 0, "the second host must get no request");
    }
}
