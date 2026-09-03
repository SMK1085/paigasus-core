// SPDX-License-Identifier: Apache-2.0

//! The outbound OpenAI egress client.
//!
//! [`OpenAiClient`] forwards a caller's chat-completion request to the OpenAI upstream and returns
//! the upstream's status plus either the full body (non-stream) or an UNBUFFERED byte stream
//! (stream). It is the sole holder of the real OpenAI API key.
//!
//! ## Split timeouts (D6 / §5)
//! A single global `.timeout()` would cap a legitimately long stream, so the client instead
//! splits the budget across the three phases the caller configures:
//! - **connect** — [`reqwest::ClientBuilder::connect_timeout`]: bounds the TCP+TLS handshake.
//! - **idle (between bytes)** — [`reqwest::ClientBuilder::read_timeout`]: the maximum gap between
//!   successive reads. It bounds a *stalled* stream without killing a long *active* one, and
//!   applies to both paths.
//! - **first byte** — applied as a per-request `.timeout()` on the NON-stream request only (a
//!   non-stream completion should return within it). The stream path deliberately sets NO overall
//!   `.timeout()`, relying on connect + idle instead.
//!
//! ## Header & secret hygiene (§5) — load-bearing
//! Every upstream request is built FRESH from a curated header set (`Authorization`,
//! `Content-Type`, `Accept`) — the caller's inbound headers are never forwarded (the client is
//! only ever handed the request `body`, never the caller's headers). The real key is exposed via
//! [`ExposeSecret::expose_secret`] ONLY at the instant the `Authorization` value is built, is held
//! as a [`SecretString`] otherwise, and never appears in `Debug`/log renders.
//!
//! ## Cancel-on-drop
//! reqwest cancels the in-flight upstream request when the [`reqwest::Response`] (and the byte
//! stream borrowed from it) is dropped. The stream path returns that stream boxed but otherwise
//! un-held, so when axum drops the response body on client disconnect (G7/G8) the upstream request
//! is cancelled — do not stash the response anywhere that outlives the returned stream.

use std::time::Duration;

use bytes::Bytes;
use futures::StreamExt;
use reqwest::StatusCode;
use reqwest::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE};
use secrecy::{ExposeSecret, SecretString};

use crate::config::OpenAiConfig;

/// The boxed, UNBUFFERED chunk stream the [`ChatResponse::Stream`] path yields — each item is a
/// raw upstream body chunk exactly as it arrived off the socket (no line-buffering, no
/// re-framing). `'static + Send` so it can be handed to axum's `Body::from_stream` and outlive
/// this call. Dropping it cancels the upstream request (cancel-on-drop).
pub type OpenAiByteStream = futures::stream::BoxStream<'static, Result<Bytes, reqwest::Error>>;

/// The upstream response, shaped so G7 can handle both paths. A NON-2xx upstream is NOT an error —
/// it arrives here as [`ChatResponse::Full`] with the upstream status + body so G7 forwards
/// OpenAI's own error envelope verbatim.
pub enum ChatResponse {
    /// Non-stream: the upstream status and the fully-buffered response body.
    Full {
        /// The upstream HTTP status, forwarded verbatim by G7.
        status: StatusCode,
        /// The complete upstream response body.
        body: Bytes,
    },
    /// Stream: the upstream status and an UNBUFFERED chunk stream (SSE), forwarded as-is.
    Stream {
        /// The upstream HTTP status (the SSE stream begins after the response head).
        status: StatusCode,
        /// The unbuffered upstream byte stream; dropping it cancels the request.
        stream: OpenAiByteStream,
    },
}

impl std::fmt::Debug for ChatResponse {
    // Manual `Debug`: `OpenAiByteStream` is not `Debug`, and we would not want to consume/print
    // the streamed body anyway. Only the discriminant + status are shown.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChatResponse::Full { status, body } => f.debug_struct("ChatResponse::Full").field("status", status).field("body_len", &body.len()).finish(),
            ChatResponse::Stream { status, .. } => f.debug_struct("ChatResponse::Stream").field("status", status).finish_non_exhaustive(),
        }
    }
}

/// Errors from the OpenAI egress client — reqwest send/connect/read failures ONLY. A non-2xx
/// upstream is deliberately NOT an error (it is returned as a [`ChatResponse::Full`] so G7
/// forwards it verbatim). G7 maps these to HTTP: connect/timeout are upstream-unreachable/slow
/// (→ 502/504), a build failure is a boot-time fault, and a bare transport error is a bad gateway.
#[derive(Debug, thiserror::Error)]
pub enum OpenAiError {
    /// The `reqwest::Client` could not be constructed (TLS backend init, invalid builder config) —
    /// a boot-time fault, surfaced when G7 builds the client at startup. Native-roots support
    /// (SMA-558 D1) made an empty/unparseable platform trust store a newly reachable cause here,
    /// mirroring `paigasus-iam/src/adapters/oidc/jwks.rs`'s equivalent hint.
    #[error("failed to build the OpenAI HTTP client — this can also mean the platform trust store contains no parseable certificates")]
    Build(#[source] reqwest::Error),
    /// Failed to establish the connection to the upstream (DNS / TCP / TLS handshake).
    #[error("failed to connect to the OpenAI upstream")]
    Connect(#[source] reqwest::Error),
    /// A configured timeout fired (connect, first-byte, or idle-between-bytes).
    #[error("the OpenAI upstream request timed out")]
    Timeout(#[source] reqwest::Error),
    /// Any other transport-level failure talking to the upstream.
    #[error("transport error talking to the OpenAI upstream")]
    Transport(#[source] reqwest::Error),
    /// The configured CA bundle could not be read, parsed, or contained no certificates — a
    /// boot-time fault like `Build`, never a request-time one, so G7's status mapping is
    /// unaffected.
    #[error("failed to load upstream.openai.extra_ca_bundle_path {path:?}")]
    CaBundle {
        path: String,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

impl OpenAiError {
    /// Classify a request-time `reqwest::Error` (from `send`/`bytes`) into connect / timeout /
    /// transport so G7 can map each to the right HTTP status. NOT a `From` impl on purpose — a
    /// client-*build* error must not be silently classified as a transport error.
    fn from_request(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            OpenAiError::Timeout(e)
        } else if e.is_connect() {
            OpenAiError::Connect(e)
        } else {
            OpenAiError::Transport(e)
        }
    }
}

/// The `source` of a `CaBundle` error raised when the bundle's certificates parse as PEM but not
/// as DER. Holds the original `reqwest::Error` as its own `source`, so anyhow renders the whole
/// chain and callers keep the ability to downcast — a pre-rendered `String` would discard both.
#[derive(thiserror::Error)]
#[error(
    "contains a structurally invalid certificate: it decodes as base64 but is not valid DER. \
     A control client built without it succeeded, so the platform trust store is not the cause"
)]
struct InvalidBundleCertificate {
    #[source]
    source: reqwest::Error,
}

impl std::fmt::Debug for InvalidBundleCertificate {
    // Manual, not derived: a derived Debug would print only the struct's field, never this
    // type's own `#[error(...)]` message — and this struct is boxed as a trait object elsewhere
    // (`OpenAiError::CaBundle`'s derived `Debug` dispatches to whatever `Debug` this type has), so
    // a bare `{err:?}` on that outer error needs this type's message + cause chain reachable
    // without going through anyhow.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&describe_error(self))
    }
}

/// What a failed `reqwest::Client` build can be attributed to. Mirrors
/// `paigasus-iam/src/adapters/oidc/jwks.rs`'s copy; see SMA-558 D7 for why the two services'
/// bundle handling is duplicated rather than extracted.
///
/// Only three combinations are reachable, and the type says so.
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
/// Success proves the store did not cause THIS failure — not that it is healthy, since reqwest
/// errors on the native store only when `valid_count == 0 && invalid_count > 0`, so an absent or
/// empty store builds fine. Failure names BOTH, because reqwest adds user roots first and
/// `?`-returns on the first bad one before reaching the native store block: with both broken, the
/// real build dies on the bundle while the control dies on the store.
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
/// IAM has its own copy at `adapters/error_chain.rs` (SMA-558 D7: duplicated, not extracted).
fn describe_error(err: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![err.to_string()];
    let mut source = err.source();
    while let Some(e) = source {
        parts.push(e.to_string());
        source = e.source();
    }
    parts.join(": ")
}

/// The client's non-TLS options, in ONE place so the control build differs from the real one by
/// exactly the added anchors (SMA-570 D6). `ClientBuilder` is not `Clone`, hence a function.
fn base_builder(connect_timeout: Duration, stream_idle_timeout: Duration) -> reqwest::ClientBuilder {
    reqwest::Client::builder().connect_timeout(connect_timeout).read_timeout(stream_idle_timeout)
}

/// The outbound OpenAI client: a shared `reqwest::Client` (connection-pooled; cheap to clone),
/// the upstream base URL, the real API key, and the first-byte budget applied per non-stream
/// request.
///
/// `Debug` is derived: `SecretString`'s own `Debug` redacts the key, so the derived output never
/// contains it (locked in by [`tests::debug_never_leaks_the_api_key`]).
#[derive(Clone, Debug)]
pub struct OpenAiClient {
    http: reqwest::Client,
    base_url: String,
    api_key: SecretString,
    /// Overall deadline for a NON-stream completion (not applied to the stream path).
    first_byte_timeout: Duration,
}

impl OpenAiClient {
    /// Build the client from the OpenAI config and the three timeout budgets (threaded in
    /// explicitly by G7 from `GatewayConfig`, since [`OpenAiConfig`] alone does not carry them).
    ///
    /// `connect_timeout` and `stream_idle_timeout` (the read/between-bytes gap) are baked into the
    /// underlying `reqwest::Client`; `first_byte_timeout` is stored and applied per non-stream
    /// request. NO global client `.timeout()` is set (it would cap a legitimate long stream).
    pub fn new(cfg: &OpenAiConfig, connect_timeout: Duration, first_byte_timeout: Duration, stream_idle_timeout: Duration) -> Result<Self, OpenAiError> {
        Self::new_with_control_build(cfg, connect_timeout, first_byte_timeout, stream_idle_timeout, || {
            base_builder(connect_timeout, stream_idle_timeout).build().is_ok()
        })
    }

    /// `new` with the control build injected, so both attribution arms are reachable in tests
    /// without mutating `SSL_CERT_FILE` (`unsafe` in edition 2024) or depending on the host's
    /// trust store.
    pub(crate) fn new_with_control_build(
        cfg: &OpenAiConfig,
        connect_timeout: Duration,
        first_byte_timeout: Duration,
        stream_idle_timeout: Duration,
        control_build_ok: impl FnOnce() -> bool,
    ) -> Result<Self, OpenAiError> {
        let mut builder = base_builder(connect_timeout, stream_idle_timeout);

        if let Some(path) = cfg.extra_ca_bundle_path.as_deref() {
            let ca_bundle = |source: Box<dyn std::error::Error + Send + Sync>| OpenAiError::CaBundle { path: path.to_string(), source };

            let pem = std::fs::read(path).map_err(|e| ca_bundle(Box::new(e)))?;
            // `from_pem_bundle`, NOT `from_pem`: a bundle may carry more than one ROOT and
            // `from_pem` reads only the first. Not an invitation to add intermediates — see the
            // ROOTS ONLY note on the config field. Mirrors
            // `paigasus-iam/src/adapters/oidc/jwks.rs`'s copy; see SMA-558 D7 for why the two are
            // duplicated rather than extracted.
            let certs = reqwest::Certificate::from_pem_bundle(&pem).map_err(|e| ca_bundle(Box::new(e)))?;
            // Ok(vec![]) is what a DER .crt, a key-only PEM or an empty file parses to, so
            // without this the likeliest operator mistake boots green having added nothing.
            if certs.is_empty() {
                return Err(ca_bundle(
                    "contained no PEM certificates — a DER file, a key-only PEM or an empty file parses as an empty bundle".into(),
                ));
            }
            tracing::info!(path = %path, count = certs.len(), "loaded extra upstream trust anchors from upstream.openai.extra_ca_bundle_path");
            for cert in certs {
                builder = builder.add_root_certificate(cert);
            }
        }

        let http = builder.build().map_err(|e| match attribute_build_failure(cfg.extra_ca_bundle_path.as_deref(), control_build_ok) {
            Attribution::NoBundle => OpenAiError::Build(e),
            Attribution::Bundle { path } => {
                tracing::error!(path = %path, attribution = "bundle", "OpenAI HTTP client build failed");
                OpenAiError::CaBundle {
                    path: path.to_string(),
                    source: Box::new(InvalidBundleCertificate { source: e }),
                }
            }
            Attribution::BundleAndStore { path } => {
                tracing::error!(path = %path, attribution = "bundle_and_store", "OpenAI HTTP client build failed");
                let chain = describe_error(&e);
                OpenAiError::CaBundle {
                    path: path.to_string(),
                    source: format!(
                        "the platform trust store also contains no parseable certificates, which is the more \
                         likely cause — fix that first, then re-verify this bundle ({chain})"
                    )
                    .into(),
                }
            }
        })?;
        Ok(Self {
            http,
            // Trim a trailing slash so `{base_url}/v1/chat/completions` never doubles up.
            base_url: cfg.base_url.trim_end_matches('/').to_owned(),
            api_key: cfg.api_key.clone(),
            first_byte_timeout,
        })
    }

    /// Forward a chat-completion request upstream. `body` is the caller's ORIGINAL raw request
    /// bytes (byte-lossless passthrough); `stream` selects the path (G7 reads it off the parsed
    /// [`ChatCompletionRequest`](crate::adapters::http::dto::ChatCompletionRequest)).
    ///
    /// The request is built FRESH with only the curated headers below — the caller's inbound
    /// headers are never present here to forward. The real key is exposed solely to build the
    /// `Authorization` value.
    pub async fn chat_completion(&self, body: Bytes, stream: bool) -> Result<ChatResponse, OpenAiError> {
        let url = format!("{}/v1/chat/completions", self.base_url);
        // `Accept` advertises the path: SSE for stream, JSON otherwise.
        let accept = if stream { "text/event-stream" } else { "application/json" };

        let mut request = self
            .http
            .post(url)
            // Real key exposed ONLY here, to build the header value; dropped immediately after.
            .header(AUTHORIZATION, format!("Bearer {}", self.api_key.expose_secret()))
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, accept)
            .body(body);

        // First-byte budget bounds a non-stream completion; the stream path relies on
        // connect + idle timeouts only (a global timeout would kill a long active stream).
        if !stream {
            request = request.timeout(self.first_byte_timeout);
        }

        let response = request.send().await.map_err(OpenAiError::from_request)?;
        let status = response.status();

        if stream {
            // UNBUFFERED: hand back the raw chunk stream as-is — no `.collect()`, no line-buffering.
            // Boxing erases the opaque `bytes_stream()` type but does not buffer; dropping the
            // boxed stream drops the response and cancels the upstream request (cancel-on-drop).
            Ok(ChatResponse::Stream {
                status,
                stream: response.bytes_stream().boxed(),
            })
        } else {
            let body = response.bytes().await.map_err(OpenAiError::from_request)?;
            Ok(ChatResponse::Full { status, body })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_client(api_key: &str) -> OpenAiClient {
        client_with_bundle(api_key, None).expect("client builds")
    }

    fn client_with_bundle(api_key: &str, extra_ca_bundle_path: Option<String>) -> Result<OpenAiClient, OpenAiError> {
        let cfg = OpenAiConfig {
            base_url: "https://api.openai.com/".to_string(),
            api_key: SecretString::from(api_key.to_string()),
            extra_ca_bundle_path,
        };
        OpenAiClient::new(&cfg, Duration::from_secs(10), Duration::from_secs(30), Duration::from_secs(300))
    }

    fn tmp_file_with(contents: &[u8]) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().expect("temp file");
        f.write_all(contents).expect("write");
        f.flush().expect("flush");
        f
    }

    #[test]
    fn debug_never_leaks_the_api_key() {
        // Secret hygiene (§5): the real key must never surface in a `Debug`/log render. The field
        // is a `SecretString`, whose own `Debug` redacts — this locks that in against a future
        // refactor that might swap the field type.
        let secret = "sk-super-secret-real-key-abc123";
        let client = test_client(secret);
        let rendered = format!("{client:?}");
        assert!(!rendered.contains(secret), "the API key must never appear in Debug output: {rendered}");
    }

    #[test]
    fn base_url_trailing_slash_is_normalized() {
        // A configured trailing slash must not produce `//v1/...`.
        let client = test_client("sk-x");
        assert_eq!(client.base_url, "https://api.openai.com");
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
        client_with_bundle("sk-x", Some(f.path().to_str().unwrap().to_string())).expect("a valid bundle must build");
    }

    #[test]
    fn missing_ca_bundle_path_is_a_build_error() {
        let err = client_with_bundle("sk-x", Some("/nonexistent/paigasus-sma558/ca.pem".to_string())).expect_err("a nonexistent bundle path must fail");
        assert!(matches!(err, OpenAiError::CaBundle { .. }), "expected CaBundle, got {err:?}");
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
        let err = client_with_bundle("sk-x", Some(f.path().to_str().unwrap().to_string())).expect_err("a certificate-free bundle must fail");
        assert!(matches!(err, OpenAiError::CaBundle { .. }), "expected CaBundle, got {err:?}");
        assert!(format!("{err:?}").contains("no PEM certificates"), "the error must say the bundle was empty: {err:?}");
    }

    #[test]
    fn undecodable_ca_bundle_is_a_build_error() {
        let f = tmp_file_with(b"-----BEGIN CERTIFICATE-----\n!!!not base64!!!\n-----END CERTIFICATE-----\n");
        let err = client_with_bundle("sk-x", Some(f.path().to_str().unwrap().to_string())).expect_err("an undecodable bundle must fail");
        assert!(matches!(err, OpenAiError::CaBundle { .. }), "expected CaBundle, got {err:?}");
        assert!(format!("{err:?}").contains("invalid certificate encoding"), "the error must say the bundle failed to decode: {err:?}");
    }

    // ---- build-failure attribution (SMA-570) --------------------------------------------------
    // `AAAAAAAA` is valid base64 (six zero bytes) but not valid DER, so unlike the
    // `!!!not base64!!!` fixture above it PASSES from_pem_bundle and fails later in
    // builder.build() — a different reqwest code path, hence its own tests.
    const INVALID_DER_PEM: &[u8] = b"-----BEGIN CERTIFICATE-----\nAAAAAAAA\n-----END CERTIFICATE-----\n";

    fn client_with_bundle_and_control(extra_ca_bundle_path: Option<String>, control_build_ok: bool) -> Result<OpenAiClient, OpenAiError> {
        let cfg = OpenAiConfig {
            base_url: "https://api.openai.com/".to_string(),
            api_key: SecretString::from("sk-x".to_string()),
            extra_ca_bundle_path,
        };
        OpenAiClient::new_with_control_build(&cfg, Duration::from_secs(10), Duration::from_secs(30), Duration::from_secs(300), || control_build_ok)
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
        // AC4. The no-bundle path still returns OpenAiError::Build, whose #[error] attribute is
        // untouched by SMA-570. Minted from a real failed build so the variant is exercised, not
        // just quoted.
        let bad = reqwest::Certificate::from_pem_bundle(INVALID_DER_PEM).expect("parses as one cert");
        let mut b = reqwest::Client::builder();
        for c in bad {
            b = b.add_root_certificate(c);
        }
        let reqwest_err = b.build().expect_err("an invalid-DER anchor must fail the build");

        assert_eq!(
            OpenAiError::Build(reqwest_err).to_string(),
            "failed to build the OpenAI HTTP client — this can also mean the platform trust store contains no parseable certificates"
        );
    }

    #[test]
    fn der_invalid_ca_bundle_names_the_config_key() {
        let f = tmp_file_with(INVALID_DER_PEM);
        let err = client_with_bundle_and_control(Some(f.path().to_str().unwrap().to_string()), true).expect_err("a structurally invalid certificate must fail the build");

        let rendered = format!("{err:?}");
        assert!(matches!(err, OpenAiError::CaBundle { .. }), "expected CaBundle, got {err:?}");
        assert!(err.to_string().contains("upstream.openai.extra_ca_bundle_path"), "the error must name the config key: {err}");
        assert!(rendered.contains("not valid DER"), "the error must say what is wrong with it: {rendered}");
    }

    #[test]
    fn der_invalid_ca_bundle_with_a_broken_store_names_both() {
        let f = tmp_file_with(INVALID_DER_PEM);
        let err = client_with_bundle_and_control(Some(f.path().to_str().unwrap().to_string()), false).expect_err("a structurally invalid certificate must fail the build");

        let rendered = format!("{err:?}");
        assert!(matches!(err, OpenAiError::CaBundle { .. }), "expected CaBundle, got {err:?}");
        assert!(rendered.contains("platform trust store"), "the store is the primary fault: {rendered}");
        assert!(rendered.contains("fix that first"), "the operator needs an order of operations: {rendered}");
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
}
