// SPDX-License-Identifier: Apache-2.0

//! The HTTP adapter of a webhook detector (SMA-856 § 6.3 to § 6.5). One [`WebhookDetector`] calls
//! one provider. The wire protocol is in `domain::webhook_protocol`.
//!
//! Every failure of a call is one `detector_error` finding of that call, with an adapter reason
//! (§ 6.4). `detect` never returns `Err` and spawns no task (A8). Nothing in a log event, a metric
//! label or a `Debug` holds a secret, a body, an argument, a nonce, a signature or a URL path (A4).
//! The `Display` of a `reqwest::Error` holds the URL, so no code here ever prints one.

use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, UNIX_EPOCH};

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use metrics::counter;
use paigasus_kernel::detect::{Finding, Severity};
use paigasus_observability::names;
use reqwest::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_TYPE, HeaderValue};
use secrecy::{ExposeSecret, SecretString};
use tokio::sync::Semaphore;
use tokio::time::{Instant, timeout_at};

use crate::adapters::upstream::{ExtraCaError, build_extra_ca_client};
use crate::application::detectors::count_webhook_failure;
use crate::config::{ProviderSecrets, WebhookConfig};
use crate::domain::detectors::{ADAPTER, DetectBatch, Detector, DetectorError, WebhookFailure, failure_finding};
use crate::domain::limits::Clock;
use crate::domain::webhook_protocol::{
    HEADER_NONCE, HEADER_SIGNATURE, HEADER_VERSION, MAX_RESPONSE_BYTES, PROTOCOL_VERSION, content_type_ok, hex_lower, parse_answer, request_mac, request_signature_header, verify_response,
};

/// The number of calls of one batch that run at the same time (D1).
const CONCURRENCY: usize = 8;
/// The adapter returns this long before the cut of the pipeline (D24).
const DEADLINE_MARGIN: Duration = Duration::from_millis(10);
/// One warn per provider and reason in this window.
const WARN_WINDOW: Duration = Duration::from_secs(10);
/// How long an idle connection stays in the pool (D26).
const POOL_IDLE: Duration = Duration::from_secs(90);

/// The source of the 16 random bytes of a request nonce.
pub trait NonceSource: Send + Sync {
    /// 16 fresh random bytes. `None` when the source fails: the call then gives `transport` and
    /// sends nothing (PD8).
    fn nonce16(&self) -> Option<[u8; 16]>;
}

/// The nonce source of production: the entropy of the operating system.
#[derive(Debug, Clone, Copy, Default)]
pub struct OsNonce;

impl NonceSource for OsNonce {
    fn nonce16(&self) -> Option<[u8; 16]> {
        use rand::TryRng;
        let mut bytes = [0u8; 16];
        rand::rngs::SysRng.try_fill_bytes(&mut bytes).ok()?;
        Some(bytes)
    }
}

/// The provider name and the reason of a warn.
type WarnKey = (String, &'static str);

/// Lets one warn through per provider and reason in each window and counts the others. The time is
/// an input, so a test needs no clock.
#[derive(Default)]
struct WarnLimiter {
    state: Mutex<HashMap<WarnKey, (Instant, u64)>>,
}

impl WarnLimiter {
    /// `Some(suppressed)` when the caller may warn now, with the number of warns that the limiter
    /// held back since the last one. `None` when the warn is held back.
    fn admit(&self, provider: &str, reason: WebhookFailure, now: Instant) -> Option<u64> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        match state.get_mut(&(provider.to_owned(), reason.as_str())) {
            None => {
                state.insert((provider.to_owned(), reason.as_str()), (now, 0));
                Some(0)
            }
            Some((last, suppressed)) if now.saturating_duration_since(*last) >= WARN_WINDOW => {
                let held = *suppressed;
                *last = now;
                *suppressed = 0;
                Some(held)
            }
            Some((_, suppressed)) => {
                *suppressed += 1;
                None
            }
        }
    }
}

/// One webhook provider. It holds the client, the secrets and the in-flight bound of the provider.
pub struct WebhookDetector {
    name: String,
    url: reqwest::Url,
    host: String,
    secret: SecretString,
    previous_secret: Option<SecretString>,
    timeout: Duration,
    failure_severity: Severity,
    kinds: BTreeSet<String>,
    max_request_bytes: usize,
    max_in_flight: usize,
    in_flight: Semaphore,
    client: reqwest::Client,
    clock: Arc<dyn Clock>,
    nonce: Arc<dyn NonceSource>,
    warn: WarnLimiter,
}

impl WebhookDetector {
    /// Build the detector of one configured provider.
    ///
    /// # Errors
    /// [`ExtraCaError`] when the CA bundle of the provider is unusable or the client does not build.
    pub fn from_config(cfg: &WebhookConfig, secrets: ProviderSecrets, clock: Arc<dyn Clock>, nonce: Arc<dyn NonceSource>) -> Result<Self, ExtraCaError> {
        let label = format!("tool_policy.detectors.webhook[{}].extra_ca_bundle_path", cfg.name);
        let timeout = Duration::from_millis(cfg.timeout_ms);
        let max_in_flight = cfg.max_in_flight;
        // PD9: the connect timeout is the provider timeout. The call bound covers the rest. The
        // workspace has no `http2` feature, so the client speaks HTTP/1.1.
        let client = build_extra_ca_client(&label, cfg.extra_ca_bundle_path.as_deref(), timeout, |b| {
            // N10: no egress proxy for a provider. reqwest reads HTTP_PROXY, HTTPS_PROXY and NO_PROXY by
            // default; `no_proxy` turns that off for the webhook clients only.
            b.no_proxy().pool_max_idle_per_host(max_in_flight).pool_idle_timeout(POOL_IDLE)
        })?;
        let url = cfg.parsed_url();
        Ok(Self {
            name: cfg.name.clone(),
            host: url.host_str().unwrap_or_default().to_owned(),
            url,
            secret: secrets.current,
            previous_secret: secrets.previous,
            timeout,
            failure_severity: cfg.failure_severity.into(),
            kinds: cfg.kind_set(),
            max_request_bytes: cfg.max_request_bytes,
            max_in_flight,
            in_flight: Semaphore::new(max_in_flight),
            client,
            clock,
            nonce,
            warn: WarnLimiter::default(),
        })
    }

    /// One call: the bound, the size check, the permit, then the exchange.
    async fn call(&self, batch: &DetectBatch, index: usize) -> Result<Vec<Finding>, WebhookFailure> {
        let now = Instant::now();
        // Less than the margin left, or no cut at all: send nothing.
        let cut = batch.deadline.checked_sub(DEADLINE_MARGIN).filter(|cut| *cut > now).ok_or(WebhookFailure::Deadline)?;
        let own = now + self.timeout;
        let (bound, elapsed) = if own < cut { (own, WebhookFailure::Timeout) } else { (cut, WebhookFailure::Deadline) };
        let body = batch
            .webhook_bodies
            .as_ref()
            .and_then(|b| b.get(index)) // Cannot happen: the pipeline builds one body per call. `transport` keeps the failure finding
            // valid for the P7 check, and the counter records the call once.
            .ok_or(WebhookFailure::Transport)?;
        if body.len() > self.max_request_bytes {
            return Err(WebhookFailure::RequestTooLarge);
        }
        let _permit = timeout_at(bound, self.in_flight.acquire())
            .await
            .map_err(|_| WebhookFailure::Overloaded)?
            // Cannot happen: the semaphore is never closed. `transport` keeps P7 happy (see above).
            .map_err(|_| WebhookFailure::Transport)?;
        timeout_at(bound, self.exchange(body)).await.map_err(|_| elapsed)?
    }

    /// Send the request and check the response in the order of § 6.3.
    async fn exchange(&self, body: &Bytes) -> Result<Vec<Finding>, WebhookFailure> {
        let nonce = hex_lower(&self.nonce.nonce16().ok_or(WebhookFailure::Transport)?);
        let t = self.clock.now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
        let mac = request_mac(self.secret.expose_secret().as_bytes(), &self.name, t, &nonce, body);
        let mut signature = // Cannot happen: the value is ASCII. `transport` keeps P7 happy (see `call`).
        HeaderValue::from_str(&request_signature_header(t, &mac)).map_err(|_| WebhookFailure::Transport)?;
        signature.set_sensitive(true);
        let mut response = self
            .client
            .post(self.url.clone())
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT_ENCODING, "identity")
            .header(HEADER_VERSION, PROTOCOL_VERSION)
            .header(HEADER_NONCE, nonce.as_str())
            .header(HEADER_SIGNATURE, signature)
            .body(body.clone())
            .send()
            .await
            // Never print the error: its `Display` holds the URL.
            .map_err(|e| if e.is_timeout() { WebhookFailure::Timeout } else { WebhookFailure::Transport })?;

        // 1. The status. A redirect is not followed, so it is a status too.
        if response.status() != reqwest::StatusCode::OK {
            return Err(WebhookFailure::Status);
        }
        let headers = response.headers().clone();
        // 2. The encoding.
        if headers.get_all(CONTENT_ENCODING).iter().any(|v| !v.as_bytes().eq_ignore_ascii_case(b"identity")) {
            return Err(WebhookFailure::Malformed);
        }
        // 3. The body, cut at the limit while it is read. The limit is inclusive.
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| WebhookFailure::Transport)? {
            if buf.len() + chunk.len() > MAX_RESPONSE_BYTES {
                return Err(WebhookFailure::ResponseTooLarge);
            }
            buf.extend_from_slice(&chunk);
        }
        // 4. Exactly one signature header.
        let mut values = headers.get_all(HEADER_SIGNATURE).iter();
        let (Some(header), None) = (values.next(), values.next()) else {
            return Err(WebhookFailure::Signature);
        };
        let header = header.to_str().map_err(|_| WebhookFailure::Signature)?;
        // 5. The signature, with the current and then the previous secret.
        let mut secrets: Vec<&[u8]> = vec![self.secret.expose_secret().as_bytes()];
        if let Some(previous) = &self.previous_secret {
            secrets.push(previous.expose_secret().as_bytes());
        }
        verify_response(&secrets, &self.name, &nonce, &buf, header)?;
        // 6. The content type.
        if !headers.get(CONTENT_TYPE).and_then(|v| v.to_str().ok()).is_some_and(content_type_ok) {
            return Err(WebhookFailure::Malformed);
        }
        // 7. The JSON and the field checks.
        parse_answer(&self.name, &buf, &self.kinds)
    }

    /// The findings of one subject: the answer, or the failure finding of the call.
    async fn run_one(&self, batch: &DetectBatch, index: usize) -> Vec<Finding> {
        if !batch.subjects[index].is_call() {
            return Vec::new();
        }
        match self.call(batch, index).await {
            Ok(found) => found,
            Err(reason) => {
                count_webhook_failure(&self.name, reason);
                if let Some(suppressed) = self.warn.admit(&self.name, reason, Instant::now()) {
                    tracing::warn!(detector = %self.name, reason = reason.as_str(), request_id = ?batch.request_id, suppressed, "a webhook call failed");
                }
                vec![failure_finding(&self.name, self.failure_severity, reason)]
            }
        }
    }
}

impl fmt::Debug for WebhookDetector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebhookDetector")
            .field("name", &self.name)
            .field("host", &self.host)
            .field("timeout", &self.timeout)
            .field("max_in_flight", &self.max_in_flight)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl Detector for WebhookDetector {
    fn name(&self) -> &str {
        &self.name
    }

    fn failure_severity(&self) -> Severity {
        self.failure_severity
    }

    fn kinds(&self) -> Option<&BTreeSet<String>> {
        Some(&self.kinds)
    }

    async fn detect(&self, batch: Arc<DetectBatch>) -> Result<Vec<Vec<Finding>>, DetectorError> {
        let n = batch.subjects.len();
        let batch = &*batch;
        let done: Vec<(usize, Vec<Finding>)> = futures::stream::iter(0..n)
            .map(|i| async move { (i, self.run_one(batch, i).await) })
            .buffer_unordered(CONCURRENCY)
            .collect()
            .await;
        let mut out = vec![Vec::new(); n];
        for (i, found) in done {
            out[i] = found;
        }
        Ok(out)
    }
}

/// Set the failure counter of every adapter reason to zero for each provider, so that `increase()`
/// sees the first failure (the first sample of a new series is 1).
pub fn prime_webhook_failure_metrics(providers: &[&str]) {
    for provider in providers {
        for reason in ADAPTER {
            counter!(names::GATEWAY_TOOL_POLICY_WEBHOOK_FAILURES_TOTAL, "provider" => (*provider).to_owned(), "reason" => reason.as_str()).absolute(0);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::SystemTime;

    use axum::Router;
    use axum::body::Body;
    use axum::http::{HeaderMap, Response};
    use metrics_util::debugging::{DebugValue, DebuggingRecorder, Snapshotter};
    use paigasus_kernel::tool_policy::{Declared, ToolInput, ToolInvocation};
    use paigasus_logging::test_support::capture_logs;
    use serde_json::json;
    use tokio::sync::{Barrier, Notify};
    use uuid::Uuid;

    use super::*;
    use crate::domain::detectors::{Caller, Subject};
    use crate::domain::webhook_protocol::{call_body, response_mac};

    const SECRET: &[u8] = b"paigasus-webhook-example-secret-0001";
    const OLD_SECRET: &[u8] = b"paigasus-webhook-example-secret-OLD1";
    const NAME: &str = "acme_dlp";
    const T: u64 = 1_764_300_000;
    const NONCE: &str = "00112233445566778899aabbccddeeff";
    const BODY: &str = r#"{"version":1,"request_id":"req_example","subject":"prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000004","org":"6f1c0000-0000-4000-8000-000000000001","session":"9a2e0000-0000-4000-8000-000000000002","tool":{"name":"send_email","declared":"yes"},"args":{"to":"x@y.example","body":"hi"},"args_text":null,"findings":[]}"#;
    const RESPONSE: &str = r#"{"findings":[{"kind":"pii_email","severity":"medium","version":"2026.11"}]}"#;
    const REQUEST_MAC: &str = "4929f346b96d703a5ab953c6e8981444573fa64e66795b8a5d61ba680c5ba878";

    struct FixedClock;
    impl Clock for FixedClock {
        fn now(&self) -> SystemTime {
            UNIX_EPOCH + Duration::from_secs(T)
        }
    }

    struct FixedNonce;
    impl NonceSource for FixedNonce {
        fn nonce16(&self) -> Option<[u8; 16]> {
            Some(core::array::from_fn(|i| u8::try_from(i * 0x11).unwrap()))
        }
    }

    struct NoNonce;
    impl NonceSource for NoNonce {
        fn nonce16(&self) -> Option<[u8; 16]> {
            None
        }
    }

    #[derive(Clone)]
    struct Recorded {
        headers: HeaderMap,
        body: Bytes,
    }

    struct Mock {
        url: String,
        hits: Arc<AtomicUsize>,
        seen: Arc<Mutex<Vec<Recorded>>>,
    }

    impl Mock {
        fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }
    }

    /// An axum mock on a loopback port. `script` gets the arrival index and the request.
    async fn mock<F, Fut>(script: F) -> Mock
    where
        F: Fn(usize, Recorded) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Response<Body>> + Send + 'static,
    {
        let hits = Arc::new(AtomicUsize::new(0));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let script = Arc::new(script);
        let (h, s) = (hits.clone(), seen.clone());
        let app = Router::new().fallback(move |req: axum::extract::Request| {
            let (hits, seen, script) = (h.clone(), s.clone(), script.clone());
            async move {
                let (parts, body) = req.into_parts();
                let body = axum::body::to_bytes(body, usize::MAX).await.unwrap();
                let rec = Recorded { headers: parts.headers, body };
                let i = hits.fetch_add(1, Ordering::SeqCst);
                seen.lock().unwrap().push(rec.clone());
                script(i, rec).await
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        Mock {
            url: format!("http://{addr}/hook/path-canary"),
            hits,
            seen,
        }
    }

    async fn mock_sync(f: impl Fn(Recorded) -> Response<Body> + Send + Sync + 'static) -> Mock {
        mock(move |_, rec| std::future::ready(f(rec))).await
    }

    fn reply(status: u16, headers: &[(&str, &str)], body: impl Into<Body>) -> Response<Body> {
        let mut b = Response::builder().status(status);
        for (k, v) in headers {
            b = b.header(*k, *v);
        }
        b.body(body.into()).unwrap()
    }

    fn signed_by(secret: &[u8], body: &str) -> Response<Body> {
        let mac = response_mac(secret, NAME, NONCE, body.as_bytes());
        reply(200, &[("content-type", "application/json"), ("paigasus-signature", &format!("v1={mac}"))], body.to_owned())
    }

    fn signed(body: &str) -> Response<Body> {
        signed_by(SECRET, body)
    }

    fn config(url: &str, timeout_ms: u64, max_request_bytes: usize, max_in_flight: usize) -> WebhookConfig {
        serde_json::from_value(json!({
            "name": NAME, "url": url, "orgs": "*", "secret_env": "ACME_SECRET",
            "timeout_ms": timeout_ms, "kinds": ["pii_email"],
            "max_request_bytes": max_request_bytes, "max_in_flight": max_in_flight,
        }))
        .unwrap()
    }

    fn secrets(previous: bool) -> ProviderSecrets {
        ProviderSecrets {
            current: SecretString::from(String::from_utf8(SECRET.to_vec()).unwrap()),
            previous: previous.then(|| SecretString::from(String::from_utf8(OLD_SECRET.to_vec()).unwrap())),
        }
    }

    fn detector(url: &str, timeout_ms: u64) -> WebhookDetector {
        WebhookDetector::from_config(&config(url, timeout_ms, 1_048_576, 64), secrets(false), Arc::new(FixedClock), Arc::new(FixedNonce)).unwrap()
    }

    fn far() -> Instant {
        Instant::now() + Duration::from_secs(60)
    }

    fn batch_of(inputs: Vec<ToolInput>, deadline: Instant) -> Arc<DetectBatch> {
        let caller = Caller {
            principal: Arc::from("prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000004"),
            session: Some(Uuid::parse_str("9a2e0000-0000-4000-8000-000000000002").unwrap()),
            opt_in: Arc::from(Vec::<String>::new()),
        };
        let subjects = inputs
            .into_iter()
            .enumerate()
            .map(|(index, input)| Subject::Call {
                index,
                invocation: ToolInvocation::new("send_email", input, Declared::Yes),
                schema: None,
            })
            .collect();
        let mut batch = DetectBatch::for_calls(
            Some(Uuid::parse_str("6f1c0000-0000-4000-8000-000000000001").unwrap()),
            Some("req_example".to_owned()),
            subjects,
            None,
            caller,
        );
        let bodies: Vec<Bytes> = batch.subjects.iter().map(|s| call_body(&batch, s, &[]).unwrap()).collect();
        batch.webhook_bodies = Some(Arc::from(bodies));
        batch.deadline = deadline;
        Arc::new(batch)
    }

    fn example_input() -> ToolInput {
        ToolInput::Json(r#"{"to":"x@y.example","body":"hi"}"#.to_owned())
    }

    fn one_call(deadline: Instant) -> Arc<DetectBatch> {
        batch_of(vec![example_input()], deadline)
    }

    fn failure(reason: WebhookFailure) -> Vec<Vec<Finding>> {
        vec![vec![failure_finding(NAME, Severity::High, reason)]]
    }

    fn counter_of(snapshotter: &Snapshotter) -> BTreeMap<(String, String), u64> {
        let mut out = BTreeMap::new();
        for (key, _, _, value) in snapshotter.snapshot().into_vec() {
            let key = key.key();
            if key.name() != names::GATEWAY_TOOL_POLICY_WEBHOOK_FAILURES_TOTAL {
                continue;
            }
            let label = |k: &str| key.labels().find(|l| l.key() == k).map(|l| l.value().to_owned()).unwrap();
            if let DebugValue::Counter(c) = value {
                out.insert((label("provider"), label("reason")), c);
            }
        }
        out
    }

    #[tokio::test]
    async fn a_request_carries_the_signed_headers_and_the_worked_example_body() {
        let m = mock_sync(|_| signed(RESPONSE)).await;
        let found = detector(&m.url, 2000).detect(one_call(far())).await.unwrap();
        assert_eq!(found, vec![vec![Finding::new(NAME, "pii_email", Severity::Medium, "2026.11")]]);
        let seen = m.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        let h = &seen[0].headers;
        let get = |k: &str| h.get(k).unwrap().to_str().unwrap().to_owned();
        assert_eq!(get("content-type"), "application/json");
        assert_eq!(get("accept-encoding"), "identity");
        assert_eq!(get("paigasus-webhook-version"), "1");
        assert_eq!(get("paigasus-nonce"), NONCE);
        assert_eq!(get("paigasus-signature"), format!("t={T},v1={REQUEST_MAC}"));
        assert_eq!(seen[0].body, BODY.as_bytes());
    }

    #[tokio::test]
    async fn each_response_failure_gives_its_reason() {
        let other = mock_sync(|_| reply(200, &[], "other")).await;
        let location = other.url.clone();
        let padded = format!("{}{}", r#"{"findings":[]}"#, " ".repeat(MAX_RESPONSE_BYTES + 1 - 15));
        assert_eq!(padded.len(), MAX_RESPONSE_BYTES + 1);
        let wrong_secret = b"paigasus-webhook-WRONG-secret-0001";
        type Case = (&'static str, WebhookFailure, Box<dyn Fn() -> Response<Body> + Send + Sync>);
        let cases: Vec<Case> = vec![
            ("500", WebhookFailure::Status, Box::new(|| reply(500, &[], "x"))),
            ("302", WebhookFailure::Status, Box::new(move || reply(302, &[("location", location.as_str())], ""))),
            (
                "gzip",
                WebhookFailure::Malformed,
                Box::new(|| {
                    let mut r = signed(r#"{"findings":[]}"#);
                    r.headers_mut().insert("content-encoding", "gzip".parse().unwrap());
                    r
                }),
            ),
            ("65537 bytes", WebhookFailure::ResponseTooLarge, Box::new(move || signed(&padded))),
            (
                "no signature",
                WebhookFailure::Signature,
                Box::new(|| reply(200, &[("content-type", "application/json")], r#"{"findings":[]}"#)),
            ),
            (
                "two signatures",
                WebhookFailure::Signature,
                Box::new(|| {
                    let mut r = signed(r#"{"findings":[]}"#);
                    let again = r.headers().get("paigasus-signature").unwrap().clone();
                    r.headers_mut().append("paigasus-signature", again);
                    r
                }),
            ),
            ("wrong secret", WebhookFailure::Signature, Box::new(move || signed_by(wrong_secret, r#"{"findings":[]}"#))),
            (
                "text/plain",
                WebhookFailure::Malformed,
                Box::new(|| {
                    let mut r = signed(r#"{"findings":[]}"#);
                    r.headers_mut().insert("content-type", "text/plain".parse().unwrap());
                    r
                }),
            ),
            ("invalid json", WebhookFailure::Malformed, Box::new(|| signed("not json"))),
            (
                "undeclared kind",
                WebhookFailure::Malformed,
                Box::new(|| signed(r#"{"findings":[{"kind":"pii_iban","severity":"low"}]}"#)),
            ),
        ];
        for (label, reason, make) in cases {
            let recorder = DebuggingRecorder::new();
            let snapshotter = recorder.snapshotter();
            let _local = metrics::set_default_local_recorder(&recorder);
            let m = mock_sync(move |_| make()).await;
            let found = detector(&m.url, 2000).detect(one_call(far())).await.unwrap();
            assert_eq!(found, failure(reason), "{label}");
            assert_eq!(counter_of(&snapshotter), BTreeMap::from([((NAME.to_owned(), reason.as_str().to_owned()), 1)]), "{label}");
        }
        assert_eq!(other.hits(), 0, "a redirect is not followed");

        // A closed port.
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/hook", listener.local_addr().unwrap());
        drop(listener);
        let found = detector(&url, 2000).detect(one_call(far())).await.unwrap();
        assert_eq!(found, failure(WebhookFailure::Transport));
        assert_eq!(counter_of(&snapshotter).get(&(NAME.to_owned(), "transport".to_owned())), Some(&1));
    }

    #[tokio::test]
    async fn the_signature_is_checked_before_the_content_type() {
        let unsigned_text = || reply(200, &[("content-type", "text/plain")], r#"{"findings":[]}"#);
        let m = mock_sync(move |_| unsigned_text()).await;
        assert_eq!(detector(&m.url, 2000).detect(one_call(far())).await.unwrap(), failure(WebhookFailure::Signature));
        let m = mock_sync(|_| {
            let mut r = signed_by(b"paigasus-webhook-WRONG-secret-0001", r#"{"findings":[]}"#);
            r.headers_mut().insert("content-type", "text/plain".parse().unwrap());
            r
        })
        .await;
        assert_eq!(detector(&m.url, 2000).detect(one_call(far())).await.unwrap(), failure(WebhookFailure::Signature));
    }

    #[tokio::test]
    async fn an_explicit_identity_encoding_passes() {
        let m = mock_sync(|_| {
            let mut r = signed(RESPONSE);
            r.headers_mut().insert("content-encoding", "identity".parse().unwrap());
            r
        })
        .await;
        let found = detector(&m.url, 2000).detect(one_call(far())).await.unwrap();
        assert_eq!(found, vec![vec![Finding::new(NAME, "pii_email", Severity::Medium, "2026.11")]]);
    }

    #[tokio::test]
    async fn a_nonce_failure_sends_nothing_and_gives_transport() {
        let m = mock_sync(|_| signed(RESPONSE)).await;
        let d = WebhookDetector::from_config(&config(&m.url, 2000, 1_048_576, 64), secrets(false), Arc::new(FixedClock), Arc::new(NoNonce)).unwrap();
        assert_eq!(d.detect(one_call(far())).await.unwrap(), failure(WebhookFailure::Transport));
        assert_eq!(m.hits(), 0);
    }

    #[tokio::test]
    async fn a_response_signed_with_the_previous_secret_is_accepted() {
        let m = mock_sync(|_| signed_by(OLD_SECRET, RESPONSE)).await;
        let with_previous = WebhookDetector::from_config(&config(&m.url, 2000, 1_048_576, 64), secrets(true), Arc::new(FixedClock), Arc::new(FixedNonce)).unwrap();
        assert_eq!(
            with_previous.detect(one_call(far())).await.unwrap(),
            vec![vec![Finding::new(NAME, "pii_email", Severity::Medium, "2026.11")]]
        );
        // Without the previous secret the same answer fails.
        assert_eq!(detector(&m.url, 2000).detect(one_call(far())).await.unwrap(), failure(WebhookFailure::Signature));
    }

    #[tokio::test]
    async fn the_size_limits_are_inclusive() {
        // A signed answer of exactly 65536 bytes is read.
        let exact = format!("{}{}", r#"{"findings":[]}"#, " ".repeat(MAX_RESPONSE_BYTES - 15));
        assert_eq!(exact.len(), MAX_RESPONSE_BYTES);
        let m = mock_sync(move |_| signed(&exact)).await;
        assert_eq!(detector(&m.url, 2000).detect(one_call(far())).await.unwrap(), vec![vec![]]);
        // The request limit: BODY.len() passes, BODY.len() - 1 sends nothing.
        let m = mock_sync(|_| signed(RESPONSE)).await;
        let at = |max| WebhookDetector::from_config(&config(&m.url, 2000, max, 64), secrets(false), Arc::new(FixedClock), Arc::new(FixedNonce)).unwrap();
        assert_eq!(at(BODY.len()).detect(one_call(far())).await.unwrap()[0].len(), 1);
        assert_eq!(m.hits(), 1);
        assert_eq!(at(BODY.len() - 1).detect(one_call(far())).await.unwrap(), failure(WebhookFailure::RequestTooLarge));
        assert_eq!(m.hits(), 1, "no request for an oversized body");
    }

    #[tokio::test]
    async fn the_provider_timeout_and_the_deadline_give_different_reasons() {
        let m = mock(|_, _| async {
            tokio::time::sleep(Duration::from_secs(1)).await;
            signed(RESPONSE)
        })
        .await;
        assert_eq!(detector(&m.url, 100).detect(one_call(far())).await.unwrap(), failure(WebhookFailure::Timeout));
        let deadline = Instant::now() + Duration::from_millis(150);
        assert_eq!(detector(&m.url, 1900).detect(one_call(deadline)).await.unwrap(), failure(WebhookFailure::Deadline));
    }

    #[tokio::test]
    async fn a_cut_inside_the_10_ms_margin_sends_nothing() {
        let m = mock_sync(|_| signed(RESPONSE)).await;
        let d = detector(&m.url, 2000);
        let now = Instant::now();
        let past = now.checked_sub(Duration::from_secs(1)).unwrap_or(now);
        for deadline in [now + Duration::from_millis(5), now, past] {
            let batch = batch_of(vec![example_input(), example_input()], deadline);
            let found = d.detect(batch).await.unwrap();
            assert_eq!(found, vec![failure(WebhookFailure::Deadline).remove(0); 2]);
        }
        assert_eq!(m.hits(), 0);
    }

    #[tokio::test]
    async fn a_slow_first_call_does_not_hold_the_others() {
        let release = Arc::new(Barrier::new(2));
        let done = Arc::new(AtomicUsize::new(0));
        let (r, dn) = (release.clone(), done.clone());
        let m = mock(move |_, rec| {
            let (r, dn) = (r.clone(), dn.clone());
            async move {
                let n = serde_json::from_slice::<serde_json::Value>(&rec.body).unwrap()["args"]["n"].as_u64().unwrap();
                if n == 0 {
                    r.wait().await;
                } else {
                    dn.fetch_add(1, Ordering::SeqCst);
                }
                signed(&format!(r#"{{"findings":[{{"kind":"pii_email","severity":"low","version":"{n}"}}]}}"#))
            }
        })
        .await;
        let batch = batch_of((0..10).map(|n| ToolInput::Json(format!(r#"{{"n":{n}}}"#))).collect(), far());
        let d = detector(&m.url, 5000);
        let releaser = async {
            while done.load(Ordering::SeqCst) < 9 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            release.wait().await;
        };
        let (found, ()) = tokio::join!(d.detect(batch), releaser);
        let found = found.unwrap();
        assert_eq!(found.len(), 10);
        for (n, f) in found.iter().enumerate() {
            assert_eq!(f, &vec![Finding::new(NAME, "pii_email", Severity::Low, n.to_string())], "call {n}");
        }
    }

    #[tokio::test]
    async fn no_free_permit_within_the_bound_is_overloaded() {
        let (started, release) = (Arc::new(Notify::new()), Arc::new(Notify::new()));
        let (s, r) = (started.clone(), release.clone());
        let m = mock(move |_, _| {
            let (s, r) = (s.clone(), r.clone());
            async move {
                s.notify_one();
                r.notified().await;
                signed(RESPONSE)
            }
        })
        .await;
        let d = WebhookDetector::from_config(&config(&m.url, 5000, 1_048_576, 1), secrets(false), Arc::new(FixedClock), Arc::new(FixedNonce)).unwrap();
        let a = d.detect(one_call(far()));
        let b = async {
            started.notified().await;
            let found = d.detect(one_call(Instant::now() + Duration::from_millis(110))).await;
            release.notify_one();
            found
        };
        let (a, b) = tokio::join!(a, b);
        assert_eq!(b.unwrap(), failure(WebhookFailure::Overloaded));
        assert_eq!(a.unwrap(), vec![vec![Finding::new(NAME, "pii_email", Severity::Medium, "2026.11")]]);
        assert_eq!(m.hits(), 1);
    }

    #[test]
    fn the_warn_limiter_lets_one_through_per_window_and_counts_the_rest() {
        let limiter = WarnLimiter::default();
        let t0 = Instant::now();
        let s = Duration::from_secs;
        assert_eq!(limiter.admit(NAME, WebhookFailure::Status, t0), Some(0));
        assert_eq!(limiter.admit(NAME, WebhookFailure::Status, t0 + s(1)), None);
        assert_eq!(limiter.admit(NAME, WebhookFailure::Status, t0 + s(9)), None);
        // Another reason and another provider have their own window.
        assert_eq!(limiter.admit(NAME, WebhookFailure::Signature, t0 + s(1)), Some(0));
        assert_eq!(limiter.admit("other_dlp", WebhookFailure::Status, t0 + s(1)), Some(0));
        assert_eq!(limiter.admit(NAME, WebhookFailure::Status, t0 + s(10)), Some(2));
        assert_eq!(limiter.admit(NAME, WebhookFailure::Status, t0 + s(11)), None);
        assert_eq!(limiter.admit(NAME, WebhookFailure::Status, t0 + s(20)), Some(1));
    }

    #[tokio::test]
    async fn the_warn_is_rate_limited_and_holds_no_content() {
        let (logs, _guard) = capture_logs();
        let m = mock_sync(|_| reply(500, &[], "body-canary")).await;
        let inputs = (0..3).map(|_| ToolInput::Json(r#"{"canary":"ARG-CANARY"}"#.to_owned())).collect();
        let found = detector(&m.url, 2000).detect(batch_of(inputs, far())).await.unwrap();
        assert_eq!(found, vec![failure(WebhookFailure::Status).remove(0); 3]);
        let text = logs.text();
        let warns: Vec<&str> = text.lines().filter(|l| l.contains("a webhook call failed")).collect();
        assert_eq!(warns.len(), 1, "{text}");
        assert!(
            warns[0].contains("detector=acme_dlp") && warns[0].contains("reason=\"status\"") && warns[0].contains("suppressed=0"),
            "{text}"
        );
        let mac = response_mac(SECRET, NAME, NONCE, b"x");
        for canary in ["path-canary", "ARG-CANARY", "body-canary", NONCE, REQUEST_MAC, mac.as_str(), "paigasus-webhook-example-secret"] {
            assert!(!text.contains(canary), "{canary} leaked: {text}");
        }
        // The capture also holds the TRACE lines of hyper, which name the host and the port. The
        // events of the adapter itself name no address.
        assert!(!warns[0].contains("127.0.0.1"), "{text}");
    }

    #[test]
    fn debug_holds_no_secret_body_path_or_argument() {
        let d = detector("http://127.0.0.1:9/hook/path-canary", 300);
        let text = format!("{d:?}");
        assert!(text.contains("acme_dlp") && text.contains("127.0.0.1"), "{text}");
        for canary in ["path-canary", "paigasus-webhook-example-secret", "ACME_SECRET"] {
            assert!(!text.contains(canary), "{text}");
        }
    }

    #[test]
    fn prime_sets_every_adapter_reason_to_zero() {
        let recorder = DebuggingRecorder::new();
        let snapshotter = recorder.snapshotter();
        let _local = metrics::set_default_local_recorder(&recorder);
        prime_webhook_failure_metrics(&["acme_dlp", "other_dlp"]);
        let series = counter_of(&snapshotter);
        assert_eq!(series.len(), 18);
        assert!(series.values().all(|v| *v == 0));
        assert!(series.contains_key(&("other_dlp".to_owned(), "response_too_large".to_owned())));
    }
}
