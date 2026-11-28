// SPDX-License-Identifier: Apache-2.0

//! The gateway model adapter (SMA-851): the Helikon Anthropic provider behind the model
//! registry. It calls the `/v1/messages` route of the AI Gateway with the key of the agent host.
//!
//! The key is never in a log line, a `Debug` output, a serialized value or an error text. An
//! error names the path of a file, never the content of it.

use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use paigasus_helikon::core::{CancellationToken, Model, ModelCapabilities, ModelError, ModelEvent, ModelRequest};
use paigasus_helikon::runtime_tokio::{RetryPolicy as HelikonRetryPolicy, RetryingModel};
use paigasus_helikon_providers_anthropic::{AnthropicModel, BuildError};
use reqwest::header::HeaderValue;

use super::classify::GatewayBackOff;
use super::model_registry::{ModelRegistry, SharedModel};
use crate::config::{GatewayConfig, IN_RUN_ATTEMPTS, ModelConfig};

/// The longest `retry-after` that a run waits for. A longer value ends the run with a host fault.
pub const IN_RUN_RETRY_AFTER_MAX: Duration = Duration::from_secs(5);
/// The time limit of the boot probe.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
/// The `anthropic-version` header value of the probe.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

/// The key. `Debug` and `Serialize` write `<redacted>`.
#[derive(Clone)]
pub struct GatewayKey(String);

impl GatewayKey {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for GatewayKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("GatewayKey(<redacted>)")
    }
}

impl serde::Serialize for GatewayKey {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str("<redacted>")
    }
}

/// An error of the gateway adapter. Every text names the path, never the content.
#[derive(Debug, thiserror::Error)]
pub enum GatewayModelError {
    #[error("cannot read the gateway key file \"{path}\"")]
    KeyRead { path: String },
    #[error("the gateway key file \"{path}\" is empty")]
    KeyEmpty { path: String },
    #[error("the gateway key file \"{path}\" is not UTF-8 text")]
    KeyNotUtf8 { path: String },
    #[error("the gateway key file \"{path}\" starts with a byte order mark")]
    KeyBom { path: String },
    #[error("the gateway key file \"{path}\" has a space inside the key")]
    KeyInnerWhitespace { path: String },
    #[error("the gateway key file \"{path}\" holds a key that is not valid in an HTTP header")]
    KeyNotHeaderValue { path: String },
    #[error("cannot read the gateway CA file \"{path}\"")]
    CaRead { path: String },
    #[error("the gateway CA file \"{path}\" holds no valid PEM certificate")]
    CaParse { path: String },
    #[error("the gateway CA file \"{path}\" holds no certificate")]
    CaEmpty { path: String },
    #[error("cannot build the HTTP client of the gateway")]
    Client,
    #[error("cannot build the model \"{name}\": {source}")]
    Model {
        name: String,
        #[source]
        source: BuildError,
    },
}

/// Read the key file. The key has no byte order mark, no space at the start or end, and no
/// space inside. A trailing newline is removed.
pub fn read_key(path: &str) -> Result<GatewayKey, GatewayModelError> {
    let path_owned = || path.to_owned();
    let bytes = std::fs::read(path).map_err(|_| GatewayModelError::KeyRead { path: path_owned() })?;
    let text = String::from_utf8(bytes).map_err(|_| GatewayModelError::KeyNotUtf8 { path: path_owned() })?;
    if text.starts_with('\u{feff}') {
        return Err(GatewayModelError::KeyBom { path: path_owned() });
    }
    let key = text.trim();
    if key.is_empty() {
        return Err(GatewayModelError::KeyEmpty { path: path_owned() });
    }
    if key.chars().any(char::is_whitespace) {
        return Err(GatewayModelError::KeyInnerWhitespace { path: path_owned() });
    }
    if HeaderValue::from_str(key).is_err() {
        return Err(GatewayModelError::KeyNotHeaderValue { path: path_owned() });
    }
    Ok(GatewayKey(key.to_owned()))
}

/// The gateway answered with a redirect. The send fails, so the provider reports a transport
/// error, which is a host fault.
#[derive(Debug, thiserror::Error)]
#[error("the gateway answered with a redirect; the agent host follows no redirect")]
struct RedirectRefused;

/// Build the HTTP client of the gateway. It installs the `ring` crypto provider first. The
/// client follows no redirect, uses no proxy, and uses rustls. A `ca_file` REPLACES the
/// platform roots.
pub fn gateway_client(gateway: &GatewayConfig) -> Result<reqwest::Client, GatewayModelError> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut builder = reqwest::Client::builder()
        .tls_backend_rustls()
        .redirect(reqwest::redirect::Policy::custom(|attempt| attempt.error(RedirectRefused)))
        .no_proxy()
        .connect_timeout(Duration::from_millis(gateway.connect_timeout_ms))
        .read_timeout(Duration::from_millis(gateway.read_timeout_ms));
    if let Some(path) = &gateway.ca_file {
        let pem = std::fs::read(path).map_err(|_| GatewayModelError::CaRead { path: path.clone() })?;
        let certs = reqwest::Certificate::from_pem_bundle(&pem).map_err(|_| GatewayModelError::CaParse { path: path.clone() })?;
        if certs.is_empty() {
            return Err(GatewayModelError::CaEmpty { path: path.clone() });
        }
        builder = builder.tls_certs_only(certs);
    }
    builder.build().map_err(|_| GatewayModelError::Client)
}

/// Wraps the provider inside `RetryingModel`. A `RateLimited` error with a `retry_after_ms`
/// above `IN_RUN_RETRY_AFTER_MAX` becomes `ModelError::Other(GatewayBackOff)`, from `invoke` and
/// from each stream item, so that the run ends at once and the worker backs off. Every other
/// result passes unchanged. Every method of `Model` goes to the inner model.
pub struct RetryAfterCap<M>(pub M);

fn cap(error: ModelError) -> ModelError {
    match error {
        ModelError::RateLimited { retry_after_ms: Some(ms) } if Duration::from_millis(ms) > IN_RUN_RETRY_AFTER_MAX => ModelError::Other(anyhow::Error::new(GatewayBackOff { retry_after_ms: ms })),
        other => other,
    }
}

#[async_trait::async_trait]
impl<M: Model> Model for RetryAfterCap<M> {
    async fn invoke(&self, request: ModelRequest, cancel: CancellationToken) -> Result<BoxStream<'static, Result<ModelEvent, ModelError>>, ModelError> {
        let stream = self.0.invoke(request, cancel).await.map_err(cap)?;
        Ok(stream.map(|item| item.map_err(cap)).boxed())
    }

    fn capabilities(&self) -> ModelCapabilities {
        self.0.capabilities()
    }

    fn provider(&self) -> &str {
        self.0.provider()
    }

    fn model(&self) -> &str {
        self.0.model()
    }
}

/// Register one gateway model for each `[[models]]` entry. It reads the key once and builds the
/// HTTP client once. It returns the probe that the caller starts after the boot.
pub fn register(models: &mut ModelRegistry, gateway: &GatewayConfig, entries: &[ModelConfig]) -> anyhow::Result<GatewayProbe> {
    let key = read_key(&gateway.api_key_file)?;
    let client = gateway_client(gateway)?;
    for entry in entries {
        // Without a provider TLS feature, `build()` needs `http_client`: the client is always passed.
        let model = AnthropicModel::messages(&entry.model)
            .base_url(&gateway.base_url)
            .api_key(key.as_str())
            .http_client(client.clone())
            .native_tool_types(false)
            .build()
            .map_err(|source| GatewayModelError::Model { name: entry.name.clone(), source })?;
        let policy = HelikonRetryPolicy::new()
            .max_attempts(IN_RUN_ATTEMPTS)
            .base_delay(Duration::from_millis(500))
            .max_delay(Duration::from_secs(5));
        let shared = SharedModel::new(RetryingModel::new(RetryAfterCap(model), policy));
        models.register_with_limit(&entry.name, shared, entry.max_output_tokens);
    }
    Ok(GatewayProbe {
        client,
        base_url: gateway.base_url.clone(),
        key,
    })
}

/// The boot probe: one `GET /v1/models` with the key of the agent host.
#[derive(Debug)]
pub struct GatewayProbe {
    client: reqwest::Client,
    base_url: String,
    key: GatewayKey,
}

/// The result of the probe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeOutcome {
    Passed(u16),
    CredentialRejected(u16),
    RouteMissing,
    Unexpected(u16),
    Unreachable,
}

impl GatewayProbe {
    /// Send the request and log one line. The line never holds the key.
    pub async fn run(self) -> ProbeOutcome {
        let url = format!("{}/v1/models", self.base_url.trim_end_matches('/'));
        let Ok(mut key) = HeaderValue::from_str(self.key.as_str()) else {
            // `read_key` checked this value, so this does not happen.
            tracing::warn!("the gateway probe failed: the key is not a header value");
            return ProbeOutcome::Unreachable;
        };
        key.set_sensitive(true);
        let sent = self
            .client
            .get(url)
            .header("x-api-key", key)
            .header("anthropic-version", ANTHROPIC_VERSION)
            .timeout(PROBE_TIMEOUT)
            .send()
            .await;
        match sent {
            Ok(response) => {
                let status = response.status().as_u16();
                match status {
                    200..=299 => {
                        tracing::info!(status, "the gateway probe passed");
                        ProbeOutcome::Passed(status)
                    }
                    401 | 403 => {
                        tracing::warn!(status, "the gateway probe failed: the gateway rejected the agent host credential");
                        ProbeOutcome::CredentialRejected(status)
                    }
                    404 => {
                        tracing::warn!("the gateway probe failed: the route is off or missing; set [messages] enabled = true on the gateway");
                        ProbeOutcome::RouteMissing
                    }
                    _ => {
                        tracing::warn!(status, "the gateway probe failed: unexpected status");
                        ProbeOutcome::Unexpected(status)
                    }
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "the gateway probe failed: the gateway is not reachable");
                ProbeOutcome::Unreachable
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::extract::State;
    use axum::http::{HeaderMap, StatusCode};
    use axum::routing::get;
    use futures::stream;

    use super::*;

    const KEY: &str = "sk-gw-1";

    fn write(dir: &tempfile::TempDir, name: &str, bytes: &[u8]) -> String {
        let path = dir.path().join(name);
        std::fs::write(&path, bytes).unwrap();
        path.to_str().unwrap().to_owned()
    }

    fn gateway(base_url: &str, key_file: &str, ca_file: Option<&str>) -> GatewayConfig {
        GatewayConfig {
            base_url: base_url.to_owned(),
            api_key_file: key_file.to_owned(),
            ca_file: ca_file.map(str::to_owned),
            connect_timeout_ms: 1000,
            read_timeout_ms: 5000,
        }
    }

    fn entry(name: &str, limit: u32) -> ModelConfig {
        serde_json::from_value(serde_json::json!({"name": name, "provider": "anthropic", "model": "claude-x", "max_output_tokens": limit})).unwrap()
    }

    #[test]
    fn the_key_file_rules_hold() {
        let dir = tempfile::tempdir().unwrap();
        for content in ["sk-gw-1\n", "sk-gw-1\r\n"] {
            let path = write(&dir, "ok", content.as_bytes());
            assert_eq!(read_key(&path).unwrap().as_str(), KEY);
        }
        type Check = fn(&GatewayModelError) -> bool;
        let cases: Vec<(&str, Vec<u8>, Check)> = vec![
            ("empty", b"".to_vec(), |e| matches!(e, GatewayModelError::KeyEmpty { .. })),
            ("blank", b"  \n".to_vec(), |e| matches!(e, GatewayModelError::KeyEmpty { .. })),
            ("bom", "\u{feff}sk-gw-1".as_bytes().to_vec(), |e| matches!(e, GatewayModelError::KeyBom { .. })),
            ("inner", b"sk gw".to_vec(), |e| matches!(e, GatewayModelError::KeyInnerWhitespace { .. })),
            ("utf8", vec![0xff, 0xfe], |e| matches!(e, GatewayModelError::KeyNotUtf8 { .. })),
            ("header", "sk-\u{7f}".as_bytes().to_vec(), |e| matches!(e, GatewayModelError::KeyNotHeaderValue { .. })),
        ];
        for (name, content, check) in cases {
            let path = write(&dir, name, &content);
            let err = read_key(&path).expect_err(name);
            assert!(check(&err), "{name}: {err:?}");
            for text in [err.to_string(), format!("{err:?}")] {
                assert!(text.contains(&path), "{name}: the text names the path");
                assert!(!text.contains(KEY), "{name}: the text holds the content");
            }
        }
        let missing = dir.path().join("missing").to_str().unwrap().to_owned();
        let err = read_key(&missing).expect_err("missing");
        assert!(matches!(err, GatewayModelError::KeyRead { .. }));
        assert!(err.to_string().contains(&missing));
    }

    #[test]
    fn the_key_debug_and_serialize_are_redacted() {
        let dir = tempfile::tempdir().unwrap();
        let key = read_key(&write(&dir, "k", KEY.as_bytes())).unwrap();
        let debug = format!("{key:?}");
        let json = serde_json::to_string(&key).unwrap();
        assert!(debug.contains("<redacted>") && !debug.contains(KEY), "{debug}");
        assert_eq!(json, "\"<redacted>\"");
    }

    #[test]
    fn the_ca_file_rules_hold() {
        let dir = tempfile::tempdir().unwrap();
        let key = write(&dir, "k", KEY.as_bytes());
        let cert = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let good = write(&dir, "good.pem", cert.cert.pem().as_bytes());
        assert!(gateway_client(&gateway("https://gw.example", &key, Some(&good))).is_ok());
        assert!(gateway_client(&gateway("https://gw.example", &key, None)).is_ok());
        let missing = dir.path().join("none.pem").to_str().unwrap().to_owned();
        assert!(matches!(gateway_client(&gateway("https://gw.example", &key, Some(&missing))), Err(GatewayModelError::CaRead { .. })));
        let junk = write(&dir, "junk.pem", b"x");
        assert!(matches!(
            gateway_client(&gateway("https://gw.example", &key, Some(&junk))),
            Err(GatewayModelError::CaParse { .. } | GatewayModelError::CaEmpty { .. })
        ));
        let empty = write(&dir, "empty.pem", b"");
        assert!(matches!(gateway_client(&gateway("https://gw.example", &key, Some(&empty))), Err(GatewayModelError::CaEmpty { .. })));
    }

    #[test]
    fn register_registers_each_entry_with_its_limit() {
        let dir = tempfile::tempdir().unwrap();
        let key = write(&dir, "k", KEY.as_bytes());
        let mut models = ModelRegistry::default();
        let probe = register(&mut models, &gateway("http://127.0.0.1:1", &key, None), &[entry("a", 16000), entry("b", 800)]).unwrap();
        assert!(models.model("a").is_some());
        assert!(models.model("b").is_some());
        assert!(models.model("c").is_none());
        assert_eq!(models.max_output_tokens("a"), Some(16000));
        assert_eq!(models.max_output_tokens("b"), Some(800));
        let shared = models.model("a").unwrap();
        assert_eq!(shared.provider(), "anthropic");
        assert_eq!(shared.model(), "claude-x");
        drop(probe);
    }

    #[test]
    fn no_built_value_or_boot_error_shows_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let key_path = write(&dir, "k", KEY.as_bytes());
        let mut models = ModelRegistry::default();
        let probe = register(&mut models, &gateway("http://127.0.0.1:1", &key_path, None), &[entry("a", 10)]).unwrap();
        assert!(!format!("{probe:?}").contains(KEY));
        let client = gateway_client(&gateway("http://127.0.0.1:1", &key_path, None)).unwrap();
        let builder = AnthropicModel::messages("claude-x").base_url("http://127.0.0.1:1").api_key(KEY).http_client(client);
        assert!(!format!("{builder:?}").contains(KEY));
        let model = builder.build().unwrap();
        assert!(!format!("{model:?}").contains(KEY));
        // A `Model { .. }` error: a base URL that does not parse makes the provider refuse the build.
        let err = AnthropicModel::messages("claude-x").base_url("not a url").api_key(KEY).build().map(|_| ()).unwrap_err();
        let boot = GatewayModelError::Model { name: "a".to_owned(), source: err };
        assert!(!format!("{boot} {boot:?}").contains(KEY));
    }

    /// A model whose stream has one scripted item.
    struct Scripted(Mutex<Option<ModelError>>);

    #[async_trait::async_trait]
    impl Model for Scripted {
        async fn invoke(&self, _request: ModelRequest, _cancel: CancellationToken) -> Result<BoxStream<'static, Result<ModelEvent, ModelError>>, ModelError> {
            let err = self.0.lock().unwrap().take().expect("one call");
            Ok(stream::iter(vec![Err(err)]).boxed())
        }

        fn capabilities(&self) -> ModelCapabilities {
            ModelCapabilities::default()
        }

        fn provider(&self) -> &str {
            "scripted-provider"
        }

        fn model(&self) -> &str {
            "scripted-model"
        }
    }

    async fn first_error(error: ModelError) -> ModelError {
        let model = RetryAfterCap(Scripted(Mutex::new(Some(error))));
        let mut s = model.invoke(ModelRequest::default(), CancellationToken::new()).await.expect("a stream");
        s.next().await.expect("one item").expect_err("an error")
    }

    #[tokio::test]
    async fn a_long_retry_after_leaves_the_run_at_once() {
        match first_error(ModelError::RateLimited { retry_after_ms: Some(120_000) }).await {
            ModelError::Other(e) => assert_eq!(e.downcast_ref::<GatewayBackOff>(), Some(&GatewayBackOff { retry_after_ms: 120_000 })),
            other => panic!("expected Other, got {other:?}"),
        }
        assert!(matches!(
            first_error(ModelError::RateLimited { retry_after_ms: Some(1_000) }).await,
            ModelError::RateLimited { retry_after_ms: Some(1_000) }
        ));
        assert!(matches!(
            first_error(ModelError::RateLimited { retry_after_ms: None }).await,
            ModelError::RateLimited { retry_after_ms: None }
        ));
        // The same mapping on an `invoke` error.
        struct Failing;
        #[async_trait::async_trait]
        impl Model for Failing {
            async fn invoke(&self, _r: ModelRequest, _c: CancellationToken) -> Result<BoxStream<'static, Result<ModelEvent, ModelError>>, ModelError> {
                Err(ModelError::RateLimited { retry_after_ms: Some(60_000) })
            }
            fn capabilities(&self) -> ModelCapabilities {
                ModelCapabilities::default()
            }
        }
        let err = RetryAfterCap(Failing).invoke(ModelRequest::default(), CancellationToken::new()).await.err().expect("an error");
        assert!(matches!(err, ModelError::Other(_)));
        let wrapped = RetryAfterCap(Scripted(Mutex::new(None)));
        assert_eq!(wrapped.provider(), "scripted-provider");
        assert_eq!(wrapped.model(), "scripted-model");
    }

    type Seen = Arc<Mutex<Vec<HeaderMap>>>;

    async fn fake(status: StatusCode) -> (String, Seen) {
        let seen: Seen = Arc::default();
        let app = Router::new()
            .route(
                "/v1/models",
                get(move |State(seen): State<Seen>, headers: HeaderMap| async move {
                    seen.lock().unwrap().push(headers);
                    status
                }),
            )
            .with_state(seen.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}"), seen)
    }

    fn probe(base_url: &str) -> GatewayProbe {
        let dir = tempfile::tempdir().unwrap();
        let key = write(&dir, "k", KEY.as_bytes());
        let mut models = ModelRegistry::default();
        register(&mut models, &gateway(base_url, &key, None), &[]).unwrap()
    }

    #[tokio::test]
    async fn the_probe_sends_the_key_and_the_version_and_logs_the_outcome() {
        let (logs, _guard) = paigasus_logging::test_support::capture_logs();
        let (url, seen) = fake(StatusCode::UNAUTHORIZED).await;
        assert_eq!(probe(&url).run().await, ProbeOutcome::CredentialRejected(401));
        {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 1);
            assert_eq!(seen[0].get("x-api-key").unwrap(), KEY);
            assert_eq!(seen[0].get("anthropic-version").unwrap(), ANTHROPIC_VERSION);
        }
        let text = logs.text();
        assert!(text.contains("the gateway rejected the agent host credential"), "{text}");
        assert!(!text.contains(KEY));

        let (url, _seen) = fake(StatusCode::OK).await;
        assert_eq!(probe(&format!("{url}/")).run().await, ProbeOutcome::Passed(200));
        assert!(logs.text().contains("the gateway probe passed"));
        assert_eq!(logs.text().matches("probe failed").count(), 1, "only the first probe failed");

        let (url, _seen) = fake(StatusCode::NOT_FOUND).await;
        assert_eq!(probe(&url).run().await, ProbeOutcome::RouteMissing);
        let (url, _seen) = fake(StatusCode::BAD_GATEWAY).await;
        assert_eq!(probe(&url).run().await, ProbeOutcome::Unexpected(502));

        let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let closed_url = format!("http://{}", closed.local_addr().unwrap());
        drop(closed);
        assert_eq!(probe(&closed_url).run().await, ProbeOutcome::Unreachable);
        assert!(!logs.text().contains(KEY));
    }
}
