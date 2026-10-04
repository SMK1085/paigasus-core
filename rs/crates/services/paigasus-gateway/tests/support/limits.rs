// SPDX-License-Identifier: Apache-2.0

//! Fixtures for the gateway limit tests (SMA-677 spec § 5.4): a fake IAM with a chosen scope,
//! canonical tenancy PRNs, a fixed clock, a recording store and a failing store, real-shaped
//! OpenAI chunks, and a Prometheus text reader that parses NUMBERS.

use std::collections::HashMap;
use std::num::NonZeroU64;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::http::{HeaderMap, Request, StatusCode, header};
use secrecy::SecretString;
use tonic::Status;
use tower::ServiceExt;
use uuid::Uuid;

use paigasus_gateway::adapters::http::{AppState, router};
use paigasus_gateway::adapters::iam::{CallerCredential, DpopContext, Iam, IamError};
use paigasus_gateway::adapters::limits::MemoryLimitStore;
use paigasus_gateway::adapters::openai::OpenAiClient;
use paigasus_gateway::application::limits::Limits;
use paigasus_gateway::config::OpenAiConfig;
use paigasus_gateway::domain::limits::{BudgetPeriod, Clock, LimitDecision, LimitPolicy, LimitRules, LimitStore, LimitStoreError, LimitTicket, OrgOverride, UnavailableKind};
use paigasus_gateway::service_info::Capabilities;
use paigasus_proto::paigasus::iam::v1::{IntrospectApiKeyResponse, IntrospectResponse};

pub const CALLER_KEY: &str = "sk-caller-secret";
pub const ORG_A: &str = "0190a100-0000-7000-8000-0000000000a1";
pub const ORG_A_PRN: &str = "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1";
pub const PROJECT_IN_A: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000a1:project/0190a1c3-0000-7000-8000-0000000000d4";
/// The pre-SMA-677 fixture scope: it does not parse as a tenancy PRN, so it names no org (D2).
pub const UNSCOPED: &str = "prn:paigasus:iam:default:scope/team-a";
pub const PRINCIPAL_1: &str = "prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000001";
pub const PRINCIPAL_2: &str = "prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000002";
pub const NOON: &str = "2026-10-02T12:00:00Z";
pub const NON_STREAM_BODY: &str = r#"{"model":"gpt-4o-mini","messages":[{"role":"user","content":"hi"}]}"#;
pub const STREAM_BODY: &str = r#"{"model":"gpt-4o-mini","stream":true,"messages":[{"role":"user","content":"hi"}]}"#;
/// A non-stream answer that reports 5 tokens.
pub const USAGE_BODY: &str = r#"{"id":"chatcmpl-1","object":"chat.completion","created":1790942400,"model":"gpt-4o-mini","choices":[{"index":0,"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}}"#;

/// An active API-key caller with a chosen principal and scope.
pub struct ScopedIam {
    principal: String,
    scope: String,
}

impl ScopedIam {
    pub fn arc(principal: &str, scope: &str) -> Arc<dyn Iam> {
        Arc::new(ScopedIam {
            principal: principal.to_owned(),
            scope: scope.to_owned(),
        })
    }
}

#[async_trait::async_trait]
impl Iam for ScopedIam {
    async fn introspect_api_key(&self, _token: &str) -> Result<IntrospectApiKeyResponse, IamError> {
        Ok(IntrospectApiKeyResponse {
            principal_prn: self.principal.clone(),
            status: "active".to_owned(),
            key_id: "key-limits".to_owned(),
            expires_at: None,
            memberships: Vec::new(),
            role_grants: Vec::new(),
            scope_prn: self.scope.clone(),
        })
    }

    async fn is_authorized_self(&self, _caller: &CallerCredential, _principal_prn: &str, _action: &str, _resource_prn: &str) -> Result<bool, IamError> {
        Ok(true)
    }

    async fn introspect_token(&self, _token: &str, _dpop: Option<DpopContext>) -> Result<IntrospectResponse, IamError> {
        Err(IamError::Rpc(Status::unauthenticated("not a token")))
    }
}

pub fn at(rfc3339: &str) -> SystemTime {
    SystemTime::from(chrono::DateTime::parse_from_rfc3339(rfc3339).expect("a fixture instant"))
}

pub struct FixedClock(pub SystemTime);

impl Clock for FixedClock {
    fn now(&self) -> SystemTime {
        self.0
    }
}

/// The memory store, plus a record of every `check_and_admit` and every `charge`.
pub struct RecordingStore {
    inner: MemoryLimitStore,
    checks: AtomicUsize,
    charges: Mutex<Vec<u64>>,
}

impl RecordingStore {
    pub fn new() -> Arc<Self> {
        Arc::new(RecordingStore {
            inner: MemoryLimitStore::new(),
            checks: AtomicUsize::new(0),
            charges: Mutex::new(Vec::new()),
        })
    }

    pub fn checks(&self) -> usize {
        self.checks.load(Ordering::SeqCst)
    }

    pub fn charges(&self) -> Vec<u64> {
        self.charges.lock().expect("not poisoned").clone()
    }
}

#[async_trait::async_trait]
impl LimitStore for RecordingStore {
    async fn check_and_admit(&self, principal: &str, org: Option<Uuid>, policy: &LimitPolicy, now: SystemTime) -> Result<LimitDecision, LimitStoreError> {
        self.checks.fetch_add(1, Ordering::SeqCst);
        self.inner.check_and_admit(principal, org, policy, now).await
    }

    fn charge(&self, ticket: LimitTicket, tokens: u64, now: SystemTime) {
        self.charges.lock().expect("not poisoned").push(tokens);
        self.inner.charge(ticket, tokens, now);
    }
}

/// A store whose every check fails with `kind` (A12), and that counts charges.
pub struct FailingStore {
    kind: UnavailableKind,
    charges: AtomicUsize,
}

impl FailingStore {
    pub fn new(kind: UnavailableKind) -> Arc<Self> {
        Arc::new(FailingStore { kind, charges: AtomicUsize::new(0) })
    }

    pub fn charges(&self) -> usize {
        self.charges.load(Ordering::SeqCst)
    }
}

#[async_trait::async_trait]
impl LimitStore for FailingStore {
    async fn check_and_admit(&self, _principal: &str, _org: Option<Uuid>, _policy: &LimitPolicy, _now: SystemTime) -> Result<LimitDecision, LimitStoreError> {
        Err(LimitStoreError::Unavailable {
            kind: self.kind,
            detail: "test failure".to_owned(),
        })
    }

    fn charge(&self, _ticket: LimitTicket, _tokens: u64, _now: SystemTime) {
        self.charges.fetch_add(1, Ordering::SeqCst);
    }
}

/// Table defaults only, monthly budget.
pub fn rules(principal: Option<u64>, org: Option<u64>, tokens: Option<u64>) -> LimitRules {
    LimitRules {
        principal_requests_per_minute: principal.and_then(NonZeroU64::new),
        org_requests_per_minute: org.and_then(NonZeroU64::new),
        tokens_per_period: tokens.and_then(NonZeroU64::new),
        budget_period: BudgetPeriod::Monthly,
        overrides: HashMap::new(),
    }
}

/// `rules` plus one exempt org.
pub fn rules_with_exempt(rules: LimitRules, org: &str) -> LimitRules {
    let id = Uuid::try_parse(org).expect("a fixture uuid");
    LimitRules {
        overrides: HashMap::from([(
            id,
            OrgOverride {
                exempt: true,
                ..OrgOverride::default()
            },
        )]),
        ..rules
    }
}

/// The limits service over `store`, with a clock fixed at `NOON`.
pub fn limits(rules: LimitRules, store: Arc<dyn LimitStore>) -> Arc<Limits> {
    Arc::new(Limits::new(rules, store, Arc::new(FixedClock(at(NOON)))))
}

pub fn openai(base_url: &str, first_byte: Duration) -> OpenAiClient {
    let cfg = OpenAiConfig {
        base_url: base_url.to_owned(),
        api_key: SecretString::from("sk-real-openai-key".to_owned()),
        extra_ca_bundle_path: None,
    };
    OpenAiClient::new(&cfg, Duration::from_secs(10), first_byte, Duration::from_secs(300)).expect("client builds")
}

pub fn app(iam: Arc<dyn Iam>, base_url: &str, limits: Option<Arc<Limits>>) -> Router {
    app_with(iam, base_url, limits, Duration::from_secs(30), true)
}

pub fn app_with(iam: Arc<dyn Iam>, base_url: &str, limits: Option<Arc<Limits>>, first_byte: Duration, chat_stream: bool) -> Router {
    router(AppState {
        iam,
        openai: Arc::new(openai(base_url, first_byte)),
        max_request_bytes: 1_048_576,
        capabilities: Capabilities { chat_stream },
        limits,
        dpop_enabled: false,
    })
}

pub fn post(body: &str) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri("/v1/chat/completions")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {CALLER_KEY}"))
        .body(Body::from(body.to_owned()))
        .expect("build request")
}

/// One answer, read whole.
pub struct Sent {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

impl Sent {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("a JSON body")
    }

    pub fn header(&self, name: &str) -> &str {
        self.headers.get(name).and_then(|v| v.to_str().ok()).unwrap_or_default()
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

pub async fn send(app: &Router, body: &str) -> Sent {
    let resp = app.clone().oneshot(post(body)).await.expect("the router answers");
    let status = resp.status();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("the body reads");
    Sent { status, headers, body }
}

/// A real-shaped OpenAI chunk record (spec § 5.4), with `\n\n` after it.
pub fn chunk(content: &str, usage_null: bool) -> String {
    let usage = if usage_null { ",\"usage\":null" } else { "" };
    format!(
        "data: {{\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"created\":1790942400,\"model\":\"gpt-4o-mini\",\"system_fingerprint\":\"fp_1\",\"choices\":[{{\"index\":0,\"delta\":{{\"content\":\"{content}\"}},\"finish_reason\":null}}]{usage}}}\n\n"
    )
}

/// The final usage record of an `include_usage` stream: empty `choices`.
pub fn usage_chunk(total: u64) -> String {
    format!("data: {{\"id\":\"chatcmpl-1\",\"object\":\"chat.completion.chunk\",\"choices\":[],\"usage\":{{\"prompt_tokens\":2,\"completion_tokens\":3,\"total_tokens\":{total}}}}}\n\n")
}

/// The numeric value of the series `name` with exactly `labels`, parsed from Prometheus text.
/// Never `contains()` on a name or a `# TYPE` line (memory `prometheus-type-line-vacuous-assertion`).
pub fn sample(rendered: &str, name: &str, labels: &[(&str, &str)]) -> Option<f64> {
    let want: Vec<String> = labels.iter().map(|(k, v)| format!("{k}=\"{v}\"")).collect();
    rendered.lines().filter(|line| !line.starts_with('#')).find_map(|line| {
        let (series, value) = line.rsplit_once(' ')?;
        let (metric, label_text) = match series.split_once('{') {
            Some((metric, rest)) => (metric, rest.strip_suffix('}')?),
            None => (series, ""),
        };
        let have: Vec<&str> = if label_text.is_empty() { Vec::new() } else { label_text.split(',').collect() };
        (metric == name && have.len() == want.len() && want.iter().all(|w| have.contains(&w.as_str())))
            .then(|| value.parse().ok())
            .flatten()
    })
}
