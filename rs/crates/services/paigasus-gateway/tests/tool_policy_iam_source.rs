// SPDX-License-Identifier: Apache-2.0
//! SMA-775: the IAM policy source against an in-process fake `ToolPolicyService` on a loopback
//! port. A tower layer records the path and the `authorization` metadata of every call that
//! reaches the server, also a call to a service that the fake does not implement.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

use bytes::Bytes;
use paigasus_gateway::adapters::iam::{CallerCredential, Iam, IamClient};
use paigasus_gateway::adapters::policy_source::iam::IamPolicySource;
use paigasus_gateway::application::tool_policy::{CacheSettings, Lookup, PolicyCache, RequestToolPolicy};
use paigasus_gateway::config::{GatewayConfig, IamClientConfig, IamTlsConfig, ToolPolicySettings};
use paigasus_gateway::domain::tool_policy::{PolicySource, SourceError};
use paigasus_gateway::runtime::build_tool_policy;
use paigasus_gateway_core::{BlockCode, Decision, DecisionContext, Format, OnUnavailable, PolicyBasis, TenantMode, decide_calls, parse_body, parse_request};
use paigasus_proto::paigasus::iam::v1 as pb;
use paigasus_proto::paigasus::iam::v1::tool_policy_service_server::{ToolPolicyService, ToolPolicyServiceServer};
use tonic::codegen::http;
use tonic::transport::{Endpoint, Server};
use tonic::{Request, Response, Status};
use uuid::Uuid;

const ORG: &str = "0190a100-0000-7000-8000-0000000000a1";
const ORG_PRN: &str = "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1";
const PRINCIPAL: &str = "prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-000000000004";
const TENANT_PATH: &str = "/paigasus.iam.v1.ToolPolicyService/GetCompiledToolPolicies";
const FLOOR_PATH: &str = "/paigasus.iam.v1.ToolPolicyService/GetCompiledFloorPolicies";
const IS_AUTHORIZED_PATH: &str = "/paigasus.iam.v1.AuthorizationService/IsAuthorized";
const FLOOR_RM: &str = "@id(\"floor-rm\") forbid(principal, action == Pgs::Gateway::Action::\"rm\", resource);";

fn org() -> Uuid {
    Uuid::try_parse(ORG).expect("uuid")
}

// --- the recording layer -----------------------------------------------------------------------

type Log = Arc<Mutex<Vec<(String, Option<String>)>>>;

#[derive(Clone)]
struct RecordLayer(Log);

impl<S> tower::Layer<S> for RecordLayer {
    type Service = Record<S>;
    fn layer(&self, inner: S) -> Record<S> {
        Record { inner, log: Arc::clone(&self.0) }
    }
}

#[derive(Clone)]
struct Record<S> {
    inner: S,
    log: Log,
}

impl<S, B> tower::Service<http::Request<B>> for Record<S>
where
    S: tower::Service<http::Request<B>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<B>) -> Self::Future {
        let auth = req.headers().get("authorization").map(|v| v.to_str().unwrap_or("<not ascii>").to_owned());
        self.log.lock().unwrap().push((req.uri().path().to_owned(), auth));
        self.inner.call(req)
    }
}

// --- the fake service --------------------------------------------------------------------------

#[derive(Default)]
struct Script {
    tenant: Mutex<Option<Result<pb::GetCompiledToolPoliciesResponse, Status>>>,
    floor: Mutex<Option<Result<pb::GetCompiledFloorPoliciesResponse, Status>>>,
    org_prns: Mutex<Vec<String>>,
}

#[derive(Clone, Default)]
struct Fake(Arc<Script>);

fn unimplemented<T>() -> Result<Response<T>, Status> {
    Err(Status::unimplemented("not part of this test"))
}

#[tonic::async_trait]
impl ToolPolicyService for Fake {
    async fn put_tool(&self, _: Request<pb::PutToolRequest>) -> Result<Response<pb::PutToolResponse>, Status> {
        unimplemented()
    }
    async fn delete_tool(&self, _: Request<pb::DeleteToolRequest>) -> Result<Response<pb::DeleteToolResponse>, Status> {
        unimplemented()
    }
    async fn list_tools(&self, _: Request<pb::ListToolsRequest>) -> Result<Response<pb::ListToolsResponse>, Status> {
        unimplemented()
    }
    async fn import_tools(&self, _: Request<pb::ImportToolsRequest>) -> Result<Response<pb::ImportToolsResponse>, Status> {
        unimplemented()
    }
    async fn put_tool_policy_set(&self, _: Request<pb::PutToolPolicySetRequest>) -> Result<Response<pb::PutToolPolicySetResponse>, Status> {
        unimplemented()
    }
    async fn get_tool_policy_set(&self, _: Request<pb::GetToolPolicySetRequest>) -> Result<Response<pb::GetToolPolicySetResponse>, Status> {
        unimplemented()
    }
    async fn put_floor_policy_set(&self, _: Request<pb::PutFloorPolicySetRequest>) -> Result<Response<pb::PutFloorPolicySetResponse>, Status> {
        unimplemented()
    }
    async fn get_floor_policy_set(&self, _: Request<pb::GetFloorPolicySetRequest>) -> Result<Response<pb::GetFloorPolicySetResponse>, Status> {
        unimplemented()
    }
    async fn get_tool_policy_settings(&self, _: Request<pb::GetToolPolicySettingsRequest>) -> Result<Response<pb::GetToolPolicySettingsResponse>, Status> {
        unimplemented()
    }
    async fn put_tool_policy_settings(&self, _: Request<pb::PutToolPolicySettingsRequest>) -> Result<Response<pb::PutToolPolicySettingsResponse>, Status> {
        unimplemented()
    }

    async fn get_compiled_tool_policies(&self, request: Request<pb::GetCompiledToolPoliciesRequest>) -> Result<Response<pb::GetCompiledToolPoliciesResponse>, Status> {
        self.0.org_prns.lock().unwrap().push(request.into_inner().org_prn);
        self.0.tenant.lock().unwrap().clone().unwrap_or_else(|| Err(Status::internal("no script"))).map(Response::new)
    }

    async fn get_compiled_floor_policies(&self, _: Request<pb::GetCompiledFloorPoliciesRequest>) -> Result<Response<pb::GetCompiledFloorPoliciesResponse>, Status> {
        self.0.floor.lock().unwrap().clone().unwrap_or_else(|| Err(Status::internal("no script"))).map(Response::new)
    }

    async fn list_held_tool_policy_versions(&self, _: Request<pb::ListHeldToolPolicyVersionsRequest>) -> Result<Response<pb::ListHeldToolPolicyVersionsResponse>, Status> {
        unimplemented()
    }

    async fn accept_held_tool_policy_version(&self, _: Request<pb::AcceptHeldToolPolicyVersionRequest>) -> Result<Response<pb::AcceptHeldToolPolicyVersionResponse>, Status> {
        unimplemented()
    }

    async fn discard_held_tool_policy_version(&self, _: Request<pb::DiscardHeldToolPolicyVersionRequest>) -> Result<Response<pb::DiscardHeldToolPolicyVersionResponse>, Status> {
        unimplemented()
    }

    async fn run_tool_policy_recompile(&self, _: Request<pb::RunToolPolicyRecompileRequest>) -> Result<Response<pb::RunToolPolicyRecompileResponse>, Status> {
        unimplemented()
    }
}

/// A running fake: its address, the script and the call log.
struct Server_ {
    addr: std::net::SocketAddr,
    fake: Fake,
    log: Log,
}

async fn start() -> Server_ {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let fake = Fake::default();
    let log: Log = Arc::default();
    let svc = ToolPolicyServiceServer::new(fake.clone());
    let layer = RecordLayer(Arc::clone(&log));
    tokio::spawn(async move {
        Server::builder()
            .layer(layer)
            .add_service(svc)
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .expect("serve");
    });
    Server_ { addr, fake, log }
}

impl Server_ {
    fn channel(&self) -> tonic::transport::Channel {
        Endpoint::from_shared(format!("http://{}", self.addr)).expect("uri").connect_lazy()
    }

    fn source(&self, credential_file: &std::path::Path) -> IamPolicySource {
        IamPolicySource::new(self.channel(), credential_file)
    }

    fn calls(&self) -> Vec<(String, Option<String>)> {
        self.log.lock().unwrap().clone()
    }

    fn script_tenant(&self, r: Result<pb::GetCompiledToolPoliciesResponse, Status>) {
        *self.fake.0.tenant.lock().unwrap() = Some(r);
    }

    fn script_floor(&self, r: Result<pb::GetCompiledFloorPoliciesResponse, Status>) {
        *self.fake.0.floor.lock().unwrap() = Some(r);
    }
}

fn tenant_response(version: u64, mode: pb::ToolPolicyMode, text: &str) -> pb::GetCompiledToolPoliciesResponse {
    pb::GetCompiledToolPoliciesResponse {
        set: Some(pb::CompiledToolPolicySet {
            kind: pb::ToolPolicySetKind::Tenant as i32,
            org_id: ORG.to_owned(),
            version,
            cedar_policies: text.to_owned(),
            ..Default::default()
        }),
        settings: Some(pb::ToolPolicySettings {
            mode: mode as i32,
            fail_mode: pb::ToolPolicyFailMode::Closed as i32,
            ..Default::default()
        }),
        floor_version: 1,
    }
}

fn floor_response(version: u64, text: &str) -> pb::GetCompiledFloorPoliciesResponse {
    pb::GetCompiledFloorPoliciesResponse {
        set: Some(pb::CompiledToolPolicySet {
            kind: pb::ToolPolicySetKind::Floor as i32,
            version,
            cedar_policies: text.to_owned(),
            ..Default::default()
        }),
    }
}

fn credential(content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("gateway-iam-key");
    std::fs::write(&path, content).expect("write");
    (dir, path)
}

// --- the tests ---------------------------------------------------------------------------------

#[tokio::test]
async fn the_fetch_sends_the_service_credential_as_bearer() {
    let s = start().await;
    s.script_tenant(Ok(tenant_response(2, pb::ToolPolicyMode::Enforce, "")));
    s.script_floor(Ok(floor_response(1, FLOOR_RM)));
    let (_dir, path) = credential("SERVICE-KEY");
    let source = s.source(&path);

    let tenant = source.tenant(org()).await.expect("tenant");
    assert_eq!(tenant.version, "t2:enforce:closed");
    let floor = source.floor().await.expect("floor");
    assert_eq!(floor.version, "f1");

    assert_eq!(
        s.calls(),
        [
            (TENANT_PATH.to_owned(), Some("Bearer SERVICE-KEY".to_owned())),
            (FLOOR_PATH.to_owned(), Some("Bearer SERVICE-KEY".to_owned()))
        ]
    );
    assert_eq!(*s.fake.0.org_prns.lock().unwrap(), [ORG_PRN]);
}

#[tokio::test]
async fn the_service_credential_is_not_sent_on_the_is_authorized_self_query() {
    let s = start().await;
    s.script_tenant(Ok(tenant_response(1, pb::ToolPolicyMode::Off, "")));
    let (_dir, path) = credential("SERVICE-KEY");
    s.source(&path).tenant(org()).await.expect("tenant");

    // The self-query client of the gateway, on the same server. The fake has no
    // `AuthorizationService`, so the call fails; the layer has recorded its metadata already.
    let iam = IamClient::connect(&IamClientConfig {
        grpc_addr: format!("http://{}", s.addr),
        tls: IamTlsConfig::LoopbackInsecure,
        service_credential_file: Some(path.to_string_lossy().into_owned()),
    })
    .await
    .expect("connect");
    let caller = CallerCredential::ApiKey("CALLER-KEY".to_owned());
    let _ = iam.is_authorized_self(&caller, PRINCIPAL, "gateway.chat.invoke", ORG_PRN).await;

    let calls = s.calls();
    let self_query: Vec<_> = calls.iter().filter(|(p, _)| p == IS_AUTHORIZED_PATH).collect();
    assert_eq!(self_query.len(), 1, "{calls:?}");
    assert_eq!(self_query[0].1.as_deref(), Some("Bearer CALLER-KEY"));
    assert!(
        calls.iter().filter(|(p, _)| p != TENANT_PATH).all(|(_, a)| !a.as_deref().unwrap_or("").contains("SERVICE-KEY")),
        "{calls:?}"
    );
}

#[tokio::test]
async fn a_rotated_credential_is_used_on_the_next_fetch() {
    let s = start().await;
    s.script_floor(Ok(floor_response(1, "")));
    let (_dir, path) = credential("OLD-KEY");
    let source = s.source(&path);
    source.floor().await.expect("first");
    std::fs::write(&path, "NEW-KEY").expect("rotate");
    source.floor().await.expect("second");
    let auth: Vec<_> = s.calls().into_iter().map(|(_, a)| a).collect();
    assert_eq!(auth, [Some("Bearer OLD-KEY".to_owned()), Some("Bearer NEW-KEY".to_owned())]);
}

#[tokio::test]
async fn a_trailing_newline_in_the_credential_file_is_trimmed() {
    let s = start().await;
    s.script_floor(Ok(floor_response(1, "")));
    let (_dir, path) = credential("SERVICE-KEY\r\n");
    s.source(&path).floor().await.expect("floor");
    assert_eq!(s.calls()[0].1.as_deref(), Some("Bearer SERVICE-KEY"));
}

#[tokio::test]
async fn a_missing_credential_file_is_an_io_error_without_the_path_content() {
    let s = start().await;
    s.script_floor(Ok(floor_response(1, "")));
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("no-such-secret-name");
    let err = s.source(&missing).floor().await.expect_err("no file");
    let text = err.to_string();
    assert!(matches!(&err, SourceError::Io(m) if m.starts_with("service credential file: ")), "{text}");
    assert!(!text.contains("no-such-secret-name"), "{text}");

    // An empty file and a file of whitespace are errors too, and no call reaches IAM.
    for content in ["", " \n"] {
        let (_d, path) = credential(content);
        let err = s.source(&path).floor().await.expect_err("empty");
        assert!(matches!(&err, SourceError::Io(m) if m == "service credential file: empty"), "{err}");
    }
    assert!(s.calls().is_empty(), "no request without a credential: {:?}", s.calls());
}

#[tokio::test]
async fn an_oversized_credential_file_is_an_io_error_without_its_content() {
    let s = start().await;
    s.script_floor(Ok(floor_response(1, "")));
    let (_d, big) = credential(&"SECRETSECRET".repeat(5 * 1024 / 12 + 1));
    let err = s.source(&big).floor().await.expect_err("too large");
    assert!(matches!(&err, SourceError::Io(m) if m.starts_with("service credential file: too large")), "{err}");
    assert!(!err.to_string().contains("SECRET"), "{err}");
    assert!(s.calls().is_empty());
    // A file of exactly 4096 bytes is accepted.
    let (_d2, edge) = credential(&"K".repeat(4096));
    s.source(&edge).floor().await.expect("4096 bytes is within the cap");
}

#[tokio::test]
async fn not_found_maps_to_not_found_and_other_codes_to_io() {
    let s = start().await;
    let (_dir, path) = credential("K");
    let source = s.source(&path);

    s.script_tenant(Err(Status::not_found("x")));
    s.script_floor(Err(Status::not_found("x")));
    assert!(matches!(source.tenant(org()).await, Err(SourceError::NotFound(w)) if w == "tenant"));
    assert!(matches!(source.floor().await, Err(SourceError::NotFound(w)) if w == "floor"));

    for (status, code) in [
        (Status::permission_denied("secret detail"), "PermissionDenied"),
        (Status::unavailable("x"), "Unavailable"),
        (Status::failed_precondition("capability-disabled"), "FailedPrecondition"),
    ] {
        s.script_tenant(Err(status));
        let err = source.tenant(org()).await.expect_err("an error");
        assert!(matches!(&err, SourceError::Io(m) if m == &format!("iam {code}")), "{err}");
        assert!(!err.to_string().contains("secret detail"), "{err}");
    }
}

fn settings() -> ToolPolicySettings {
    ToolPolicySettings {
        enabled: true,
        default_mode: TenantMode::Enforce,
        on_policy_unavailable: OnUnavailable::Closed,
        refresh: Duration::from_secs(30),
        max_stale: Duration::from_secs(300),
        idle_evict: Duration::from_secs(3600),
        load_timeout: Duration::from_secs(30),
        orgs: HashMap::new(),
        settings_from_source: true,
    }
}

#[tokio::test]
async fn an_org_in_mode_off_still_gets_the_floor() {
    let s = start().await;
    s.script_floor(Ok(floor_response(1, FLOOR_RM)));
    s.script_tenant(Ok(tenant_response(1, pb::ToolPolicyMode::Off, "@id(\"p\") permit(principal, action, resource);")));
    let (_dir, path) = credential("K");
    let cfg = settings();
    let cache = PolicyCache::new(Arc::new(s.source(&path)), CacheSettings::from_settings(&cfg));
    let policy = RequestToolPolicy::new(cache, cfg, Arc::new(paigasus_gateway::application::decision_log::DecisionRecorder::tracing()));

    let body = Bytes::from_static(br#"{"model":"m","messages":[{"role":"user","content":"hi"}],"tools":[{"type":"function","function":{"name":"rm","parameters":{}}},{"type":"function","function":{"name":"ls","parameters":{}}}]}"#);
    let turn = parse_request(Format::OpenAiChat, &body);
    let ctx = DecisionContext::new(PRINCIPAL, ORG_PRN, Some("req-1".into()));
    let out = policy.apply(&ctx, "m", Format::OpenAiChat, &turn, body).await.expect("apply");

    let v: serde_json::Value = serde_json::from_slice(&out.body).expect("json");
    let names: Vec<&str> = v["tools"].as_array().expect("tools").iter().map(|t| t["function"]["name"].as_str().expect("name")).collect();
    assert_eq!(names, ["ls"], "the floor removes rm although the tenant mode is off");
    assert_eq!(out.snapshot.mode, TenantMode::Off);
}

/// R7 end to end: the real IAM source and the policy cache. Only the mode changes at IAM, and
/// the set version stays the same. The cache keeps a stored set when the version token is
/// unchanged, so the next load applies the new mode only if the token covers the settings.
#[tokio::test]
async fn a_mode_change_alone_at_iam_applies_on_the_next_load() {
    const FORBID_X: &str = r#"@id("p") permit(principal, action, resource); @id("t") forbid(principal, action == Pgs::Gateway::Action::"x", resource);"#;
    let s = start().await;
    s.script_floor(Ok(floor_response(1, FLOOR_RM)));
    s.script_tenant(Ok(tenant_response(3, pb::ToolPolicyMode::LogOnly, FORBID_X)));
    let (_dir, path) = credential("K");
    let cfg = settings();
    let cache = PolicyCache::new(Arc::new(s.source(&path)), CacheSettings::from_settings(&cfg));
    let policy = RequestToolPolicy::new(Arc::clone(&cache), cfg, Arc::new(paigasus_gateway::application::decision_log::DecisionRecorder::tracing()));

    let body = Bytes::from_static(br#"{"model":"m","messages":[{"role":"user","content":"hi"}],"tools":[{"type":"function","function":{"name":"x","parameters":{}}},{"type":"function","function":{"name":"y","parameters":{}}}]}"#);
    let ctx = DecisionContext::new(PRINCIPAL, ORG_PRN, Some("req-1".into()));
    let names = |out: &[u8]| -> Vec<String> {
        let v: serde_json::Value = serde_json::from_slice(out).expect("json");
        v["tools"].as_array().expect("tools").iter().map(|t| t["function"]["name"].as_str().expect("name").to_owned()).collect()
    };

    let first = policy
        .apply(&ctx, "m", Format::OpenAiChat, &parse_request(Format::OpenAiChat, &body), body.clone())
        .await
        .expect("apply");
    assert_eq!(first.snapshot.mode, TenantMode::LogOnly);
    assert_eq!(names(&first.body), ["x", "y"], "log_only removes nothing");

    // The mode changes at IAM; the set version stays 3.
    s.script_tenant(Ok(tenant_response(3, pb::ToolPolicyMode::Enforce, FORBID_X)));
    cache.invalidate_all();
    let second = policy
        .apply(&ctx, "m", Format::OpenAiChat, &parse_request(Format::OpenAiChat, &body), body.clone())
        .await
        .expect("apply");
    assert_eq!(second.snapshot.mode, TenantMode::Enforce, "the new mode applies on the next load");
    assert_eq!(names(&second.body), ["y"], "enforce removes x");
}

// `figment::Jail::expect_with` fixes its closure's `Err` type to `figment::Error`.
#[allow(clippy::result_large_err)]
#[tokio::test]
async fn an_iam_that_is_down_at_boot_does_not_stop_the_gateway() {
    let (_dir, path) = credential("K");
    let toml = format!(
        "[upstream.openai]\napi_key = \"sk-test-key\"\n[iam]\ngrpc_addr = \"http://127.0.0.1:1\"\nservice_credential_file = {:?}\n[iam.tls]\nmode = \"loopback_insecure\"\n[tool_interception]\nenabled = true\n[tool_policy]\nenabled = true\non_policy_unavailable = \"closed\"\nsource = {{ kind = \"iam\" }}\n",
        path.to_string_lossy()
    );
    let mut config = None;
    figment::Jail::expect_with(|jail| {
        jail.create_file("gateway.toml", &toml)?;
        let cfg: GatewayConfig = GatewayConfig::figment().extract()?;
        cfg.validate().expect("validates");
        config = Some(cfg);
        Ok(())
    });
    let runtime = build_tool_policy(&config.expect("config"), Arc::new(paigasus_gateway::application::decision_log::DecisionRecorder::tracing()), None).expect("the gateway builds with IAM down");
    let (cache, _) = runtime.policy.expect("the tool policy is on");
    assert!(matches!(cache.floor(OnUnavailable::Closed).await, Lookup::Unavailable));
}

// --- SMA-753: a set with history conditions loads; its slots are still empty ---------------------
//
// The cache maps and validates the conditions at load (PR 2), but no code fills a slot yet. The
// kernel builds each Cedar request with no schema (`decide.rs`), so a policy that reads
// `context.<slot>` errors. An errored forbid denies and an errored permit does not allow (SMA-752
// D26). So only the history policies fail closed, and the other policies of the set still decide.

/// The window limit that these answers report.
const MAX_WINDOW_MS: u64 = 86_400_000;

/// A valid `formerly` condition on `slot`: a call of `rm` in the session within one minute.
fn slot_condition(slot: &str, policy_id: &str) -> pb::HistoryCondition {
    let tool = pb::HistoryPredicate {
        node: Some(pb::history_predicate::Node::Tool(pb::ToolMatch {
            name_key: "rm".to_owned(),
            any_catalog_tool: false,
        })),
    };
    pb::HistoryCondition {
        slot: slot.to_owned(),
        policy_id: policy_id.to_owned(),
        scope: pb::HistoryScope::Session as i32,
        window_ms: 60_000,
        shape: Some(pb::history_condition::Shape::Formerly(pb::FormerlyShape { predicate: Some(tool) })),
    }
}

/// A tenant answer of IAM with one history condition on the slot `h0`.
fn tenant_with_slot(mode: pb::ToolPolicyMode, text: &str) -> pb::GetCompiledToolPoliciesResponse {
    let mut r = tenant_response(5, mode, text);
    let set = r.set.as_mut().expect("set");
    set.conditions.push(slot_condition("h0", "t"));
    set.max_window_ms = MAX_WINDOW_MS;
    r
}

/// A floor answer of IAM with one history condition on the slot `fh0`. A tenant slot must not
/// equal a floor slot, so the floor uses its own name.
fn floor_with_slot(text: &str) -> pb::GetCompiledFloorPoliciesResponse {
    let mut r = floor_response(2, text);
    let set = r.set.as_mut().expect("set");
    set.conditions.push(slot_condition("fh0", "floor-h"));
    set.max_window_ms = MAX_WINDOW_MS;
    r
}

/// The outcome of [`decide_through_iam`].
struct Decided {
    /// The function tool names that go upstream.
    upstream: Vec<String>,
    mode: TenantMode,
    /// The decision on each call, in order.
    decisions: Vec<Decision>,
    /// The basis on each call, in order.
    bases: Vec<PolicyBasis>,
    /// The `(determining, errored)` policy ids on each call, in order.
    refs: Vec<(Vec<String>, Vec<String>)>,
}

/// One request with the function tools `tools` through the real IAM source and the policy cache,
/// then the kernel decision on one call to each of `calls`.
async fn decide_through_iam(s: &Server_, tools: &[&str], calls: &[&str]) -> Decided {
    let (_dir, path) = credential("K");
    let cfg = settings();
    let cache = PolicyCache::new(Arc::new(s.source(&path)), CacheSettings::from_settings(&cfg));
    let policy = RequestToolPolicy::new(cache, cfg, Arc::new(paigasus_gateway::application::decision_log::DecisionRecorder::tracing()));

    let defs: Vec<serde_json::Value> = tools.iter().map(|n| serde_json::json!({"type":"function","function":{"name":n,"parameters":{}}})).collect();
    let body = Bytes::from(serde_json::json!({"model":"m","messages":[{"role":"user","content":"hi"}],"tools":defs}).to_string());
    let turn = parse_request(Format::OpenAiChat, &body);
    let ctx = DecisionContext::new(PRINCIPAL, ORG_PRN, Some("req-1".into()));
    let out = policy.apply(&ctx, "m", Format::OpenAiChat, &turn, body).await.expect("apply");
    let v: serde_json::Value = serde_json::from_slice(&out.body).expect("json");
    let upstream = v["tools"].as_array().expect("tools").iter().map(|t| t["function"]["name"].as_str().expect("name").to_owned()).collect();

    let tool_calls: Vec<serde_json::Value> = calls
        .iter()
        .enumerate()
        .map(|(i, n)| serde_json::json!({"id":format!("c{i}"),"type":"function","function":{"name":n,"arguments":"{}"}}))
        .collect();
    let answer = serde_json::json!({"object":"chat.completion","choices":[{"index":0,"message":{"role":"assistant","tool_calls":tool_calls},"finish_reason":"tool_calls"}]}).to_string();
    let parsed = parse_body(Format::OpenAiChat, answer.as_bytes(), 1 << 20).expect("answer").calls;
    let outcomes = decide_calls(&out.snapshot, &turn, &parsed, &[], &std::time::Instant::now);
    let ids = |r: &[paigasus_kernel::tool_policy::PolicyRef]| r.iter().map(|p| p.id.clone()).collect::<Vec<_>>();
    Decided {
        upstream,
        mode: out.snapshot.mode,
        decisions: outcomes.iter().map(|o| o.decision).collect(),
        bases: outcomes.iter().map(|o| o.basis).collect(),
        refs: outcomes
            .iter()
            .map(|o| o.cedar.as_ref().map_or_else(Default::default, |c| (ids(&c.determining), ids(&c.errored))))
            .collect(),
    }
}

const BLOCK: Decision = Decision::Block { code: BlockCode::Policy };
const OBSERVE: Decision = Decision::Observe { would: BlockCode::Policy };

#[tokio::test]
async fn a_tenant_set_with_a_history_condition_loads_and_only_its_slot_policies_fail_closed() {
    // `ls` has its own permit. `y` is allowed only by a permit that reads the slot. `rm` has a
    // permit, and a forbid that reads the slot.
    const TENANT_SLOTS: &str = r#"@id("ls") permit(principal, action == Pgs::Gateway::Action::"ls", resource);
        @id("y") permit(principal, action == Pgs::Gateway::Action::"y", resource) when { context.h0 };
        @id("rm-ok") permit(principal, action == Pgs::Gateway::Action::"rm", resource);
        @id("t") forbid(principal, action == Pgs::Gateway::Action::"rm", resource) when { context.h0 };"#;
    for (mode, want_mode, slot_effect) in [(pb::ToolPolicyMode::Enforce, TenantMode::Enforce, BLOCK), (pb::ToolPolicyMode::LogOnly, TenantMode::LogOnly, OBSERVE)] {
        let s = start().await;
        s.script_floor(Ok(floor_response(1, "")));
        s.script_tenant(Ok(tenant_with_slot(mode, TENANT_SLOTS)));
        let d = decide_through_iam(&s, &["ls", "y", "rm"], &["ls", "y", "rm"]).await;
        assert_eq!(d.mode, want_mode);
        // The set loads: its own permit allows `ls`, not a fail mode.
        assert_eq!(d.decisions[0], Decision::Allow, "{want_mode:?}: an unrelated policy still allows its call");
        assert_eq!(d.decisions[1], slot_effect, "{want_mode:?}: a permit that reads a slot does not allow");
        assert_eq!(d.decisions[2], slot_effect, "{want_mode:?}: a forbid that reads a slot denies");
        // How an operator sees these decisions (runbook section 10): the slot forbid has the basis
        // `errored_forbid` and is named in `errored`; a call that only a slot permit could allow
        // has the basis `policy` and no determining policy.
        assert_eq!(d.bases, [PolicyBasis::Policy, PolicyBasis::Policy, PolicyBasis::ErroredForbid], "{want_mode:?}");
        assert!(d.refs[1].0.is_empty(), "{want_mode:?}: {:?}", d.refs[1]);
        assert!(d.refs[2].1.contains(&"t".to_owned()), "{want_mode:?}: {:?}", d.refs[2]);
        // D6: a forbid that reads a slot is never in the optimistic set, so the request side keeps `rm`.
        assert!(d.upstream.iter().any(|n| n == "rm"), "{want_mode:?}: {:?}", d.upstream);
    }
}

#[tokio::test]
async fn a_floor_with_a_history_condition_loads_and_its_slot_forbid_denies_in_every_mode() {
    const FLOOR_SLOTS: &str = r#"@id("floor-h") forbid(principal, action == Pgs::Gateway::Action::"rm", resource) when { context.fh0 };
        @id("floor-del") forbid(principal, action == Pgs::Gateway::Action::"del", resource);"#;
    const PERMIT: &str = r#"@id("p") permit(principal, action, resource);"#;
    for mode in [pb::ToolPolicyMode::Enforce, pb::ToolPolicyMode::LogOnly, pb::ToolPolicyMode::Off] {
        let s = start().await;
        s.script_floor(Ok(floor_with_slot(FLOOR_SLOTS)));
        s.script_tenant(Ok(tenant_response(1, mode, PERMIT)));
        let d = decide_through_iam(&s, &["ls", "rm", "del"], &["ls", "rm", "del"]).await;
        assert_eq!(d.decisions, [Decision::Allow, BLOCK, BLOCK], "{mode:?}: D33, the floor denies in every mode");
        // The floor forbid with no slot still removes `del` on the request side. The slot forbid is
        // not in the optimistic set, so `rm` stays.
        assert_eq!(d.upstream, ["ls", "rm"], "{mode:?}");
    }
}

#[tokio::test]
async fn a_negated_slot_forbid_also_denies() {
    // `unless` on a temporal block compiles to `when { !context.<slot> }`. The negation of an
    // empty slot errors too, so the forbid denies. It must never allow.
    const TENANT_NOT: &str = r#"@id("p") permit(principal, action, resource);
        @id("t") forbid(principal, action == Pgs::Gateway::Action::"rm", resource) when { !context.h0 };"#;
    const FLOOR_NOT: &str = r#"@id("floor-h") forbid(principal, action == Pgs::Gateway::Action::"del", resource) when { !context.fh0 };"#;
    let s = start().await;
    s.script_floor(Ok(floor_with_slot(FLOOR_NOT)));
    s.script_tenant(Ok(tenant_with_slot(pb::ToolPolicyMode::Enforce, TENANT_NOT)));
    let d = decide_through_iam(&s, &["ls", "rm", "del"], &["ls", "rm", "del"]).await;
    assert_eq!(d.decisions, [Decision::Allow, BLOCK, BLOCK]);
    assert_eq!(d.bases, [PolicyBasis::Policy, PolicyBasis::ErroredForbid, PolicyBasis::ErroredForbid]);
}
