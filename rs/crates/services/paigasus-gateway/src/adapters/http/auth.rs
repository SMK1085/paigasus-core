// SPDX-License-Identifier: Apache-2.0

//! The gateway's authentication + authorization middleware — the security crux of the M0 slice.
//!
//! [`require_iam_auth`] runs BEFORE the chat handler on every protected request and performs the
//! full pipeline: extract the bearer credential, introspect it against IAM, then run the D9
//! *self-query* authorization check, and finally attach the resolved [`CallerContext`] to the
//! request extensions. Any failure short-circuits with a [`GatewayError`] rendered through the
//! OpenAI error envelope.
//!
//! ## The self-query invariant (D9 — the whole point)
//! The authorization call ([`Iam::is_authorized_self`]) is made with the caller's OWN bearer as
//! the credential AND the caller's OWN service-account PRN (the one IAM's introspect response just
//! returned) as the queried principal. Because IAM resolves that bearer to the same principal the
//! request names, it sees a principal asking about *itself* and applies no cross-principal
//! exposure gate. The middleware sources both from the SAME inbound request, so it can never query
//! a principal other than the authenticated caller — a unit test proves the recorded authz args
//! are exactly `(caller's key, introspected SA, "InvokeModel", scope)`.
//!
//! ## Two different IAM-error → HTTP mappings
//! The introspect and authz calls map gRPC status codes DIFFERENTLY, and they diverge on
//! `PermissionDenied`: on introspect it means an inactive principal (a client-auth failure → 401);
//! on the authz call — which we ALWAYS make as a self-query — it means IAM's exposure gate denied
//! us, i.e. a plumbing/self-query bug, not a client denial (→ 500). See [`introspect_error`] and
//! [`authz_error`].

use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use metrics::{counter, histogram};
use paigasus_observability::names;
use tonic::Code;

use super::error::GatewayError;
use crate::adapters::iam::{Iam, IamError};
use crate::domain::CallerContext;

/// The wire action string the gateway authorizes every chat request against. Hardcoded because the
/// gateway cannot import iam-core's `Action` enum across the gRPC boundary — it sends the literal
/// wire form (`Action::InvokeModel.as_wire() == "InvokeModel"`, verified in the integration facts).
const INVOKE_MODEL_ACTION: &str = "InvokeModel";

/// Authenticate + authorize a request before it reaches the protected handler. Wired by G7 via
/// `from_fn_with_state(app_state.iam.clone(), require_iam_auth)`; the middleware's state
/// (`Arc<dyn Iam>`) is independent of the handler's `AppState`, so this depends only on the IAM
/// port. On success the request carries a [`CallerContext`] extension; on any failure it returns
/// the mapped [`GatewayError`] as an OpenAI-envelope response.
pub async fn require_iam_auth(State(iam): State<Arc<dyn Iam>>, mut req: Request, next: Next) -> Response {
    // 1. Bearer — the ONLY accepted credential source (no cookies, no query params).
    let Some(key) = bearer(req.headers()) else {
        return GatewayError::MissingBearer.into_response();
    };

    // 2. Introspect the caller-presented key. An error Status maps per `introspect_error`; a
    //    success carrying a non-active principal or an empty scope is rejected here. The IAM-call
    //    metric records the RPC's own outcome (ok/denied/unavailable/error) — NOT the later
    //    active-status/scope validation below, which is a separate, non-metric decision.
    let introspect_started = Instant::now();
    let resp = match iam.introspect_api_key(&key).await {
        Ok(resp) => {
            record_iam_call("introspect", "ok", introspect_started);
            resp
        }
        Err(err) => {
            record_iam_call("introspect", iam_result(&err), introspect_started);
            return introspect_error(err).into_response();
        }
    };
    if resp.status != "active" {
        // Belt-and-braces: IAM returns an error Status for a bad key, but a success-with-
        // non-active-status is still a client-auth failure, not a valid caller.
        return GatewayError::InvalidCredential.into_response();
    }
    if resp.scope_prn.is_empty() {
        // A missing scope is a plumbing bug (introspect should always return one), surfaced as a
        // distinct 500 diagnostic rather than a silent deny. Log so it's visible in prod — the
        // response body stays generic (see `GatewayError::Internal`'s doc).
        tracing::error!(
            principal_prn = %resp.principal_prn,
            key_id = %resp.key_id,
            "introspect returned an empty scope_prn — IAM plumbing bug (SMA-446 D11)"
        );
        return GatewayError::MissingScope.into_response();
    }
    let principal_prn = resp.principal_prn;
    let scope_prn = resp.scope_prn;
    let key_id = resp.key_id;

    // 3. Self-query authorization (D9): the caller's OWN key as the bearer AND the caller's OWN SA
    //    PRN (from step 2) as the queried principal, resource = the introspected scope. Never a
    //    different principal — that is exactly what makes this a self-query.
    let authz_started = Instant::now();
    match iam.is_authorized_self(&key, &principal_prn, INVOKE_MODEL_ACTION, &scope_prn).await {
        Ok(true) => record_iam_call("authorize", "ok", authz_started),
        Ok(false) => {
            record_iam_call("authorize", "denied", authz_started);
            return GatewayError::AuthzDenied.into_response();
        }
        Err(err) => {
            record_iam_call("authorize", iam_result(&err), authz_started);
            let mapped = authz_error(err);
            if mapped == GatewayError::Internal {
                // A `PermissionDenied` here means IAM's exposure gate denied a self-query, which
                // should be impossible — log it, since this is the single most important thing to
                // see (a broken D9 self-query). The response body stays generic.
                tracing::error!(
                    principal_prn = %principal_prn,
                    key_id = %key_id,
                    "self-query IsAuthorized returned an unexpected error mapped to 500 — possible broken self-query (SMA-446 D9)"
                );
            }
            return mapped.into_response();
        }
    }

    // 4. Attach the resolved caller identity and proceed to the handler.
    req.extensions_mut().insert(CallerContext { principal_prn, scope_prn, key_id });
    next.run(req).await
}

/// Authenticate a capability-discovery request. Unlike [`require_iam_auth`] this performs NO
/// authorization: discovery must not be gated on a permission, or a caller who legitimately
/// cannot invoke models could never learn that streaming exists — and ADR-0020 D4 forbids
/// provisioning a service credential for the console.
///
/// ## Why both introspections are tried, rather than branching on the token prefix
/// IAM's API-key prefix is an operator knob (`api_keys.key_prefix`), and the gateway has no
/// visibility of its value. Branching on a hardcoded `pgs_sk_` would silently route every
/// service-account key to the OIDC path — and reject it — for any operator who changed that
/// setting, with no boot error. Mirroring the prefix into `GatewayConfig` would instead create a
/// must-match-or-break coupling between two services' configs. Trying both costs one extra RPC
/// on the OIDC path of a low-frequency, client-cached call, and cannot drift.
///
/// ## Why an unprovisioned identity is accepted
/// IAM's `Introspect` resolves with `Provisioning::Disabled`, so a VALIDATED token whose
/// `(issuer, subject)` has no local principal comes back `PermissionDenied`. IAM's own HTTP
/// middleware JIT-provisions instead, so rejecting here would make gateway discovery succeed or
/// fail purely on whether the console happened to call IAM first — breaking exactly the lazy
/// in-user-request flow ADR-0020 D4 specifies. The descriptor is byte-identical for every
/// caller and exposes no per-principal data, so accepting widens nothing. This relaxation is
/// scoped to THIS middleware; `require_iam_auth` is unchanged.
///
/// ### Why accepting the WHOLE `PermissionDenied` code is safe today (and when it stops being so)
/// IAM's gRPC layer collapses THREE `AuthnError` variants onto `Code::PermissionDenied`, with no
/// structured reason on the wire to tell them apart (`paigasus-iam`'s
/// `adapters/grpc/convert.rs:56-58`): `IdentityNotProvisioned` (the case this relaxation targets),
/// `ProvisioningFailed`, and `PrincipalInactive`. Accepting the whole code is correct only because
/// the other two are UNREACHABLE via `Introspect` as the code stands today (verified against
/// `paigasus-iam`'s source, not assumed):
/// - `ProvisioningFailed` can only be returned by `jit_provision`
///   (`application/authenticate_token.rs`), which `AuthenticateToken::resolve` calls ONLY on the
///   `Provisioning::Enabled` arm (`:106-112`, `self.jit_provision(&claims).await?`). The
///   `Provisioning::Disabled` arm `Introspect` always uses returns `IdentityNotProvisioned`
///   immediately instead (`:104-105`) — `jit_provision` is never reached.
/// - `PrincipalInactive` requires resolving to a principal whose `PrincipalStatus` is `Disabled`
///   (`:127-129`). Production code sets that status in exactly one call site,
///   `ServiceAccountService::archive` (`application/service_accounts.rs:205,222`) — which disables
///   a SERVICE-ACCOUNT principal. Service accounts authenticate by API key, never by OIDC token,
///   so they have no `(issuer, subject)` row in `external_identity` for `resolve`'s
///   `find_by_issuer_subject` lookup to ever match. A disabled SA can therefore never be the
///   principal an OIDC `Introspect` call resolves to.
///
/// **If either of those becomes reachable through `Introspect`** — a future "disable a user"
/// use case, or any change that lets `ProvisioningFailed` surface from a read-only resolve — **this
/// match arm must narrow**, not stay a blanket `PermissionDenied` accept. Matching on the `Status`
/// MESSAGE STRING instead is deliberately not done: those strings
/// (`adapters/grpc/convert.rs:56-58`) are bare literals with no test pinning their text, so an IAM
/// copy-edit would silently flip this middleware to rejecting every unprovisioned caller. The
/// durable fix — a structured `ErrorInfo` reason on IAM's authn gRPC surface — is IAM-side and out
/// of scope for this task; tracked as a follow-up.
pub async fn require_authenticated(State(iam): State<Arc<dyn Iam>>, req: Request, next: Next) -> Response {
    let Some(token) = bearer(req.headers()) else {
        return GatewayError::MissingBearer.into_response();
    };

    let started = Instant::now();
    // Whether the API-key leg failed to reach a VERDICT, as opposed to reaching a rejection.
    // An outage here must not be laundered into a `401` by the fallback below: an API key is
    // never a valid JWT, so the OIDC leg answers `Unauthenticated` for an API-key caller no
    // matter how valid the key is. Without this flag the ORDINARY outcome of a transient
    // API-key-leg failure is a definitive-looking `401` telling the client to stop retrying —
    // during exactly the outage it should be backing off through.
    let mut api_key_inconclusive = false;
    match iam.introspect_api_key(&token).await {
        Ok(resp) if resp.status == "active" => {
            record_iam_call("introspect", "ok", started);
            return next.run(req).await;
        }
        // IAM answered, and the answer was "not active" — a verdict, not an outage.
        Ok(_) => record_iam_call("introspect", "denied", started),
        Err(err) => {
            // `IamError` is not `Clone`, so compute the bounded metric label before
            // `introspect_error` consumes `err`.
            let label = iam_result(&err);
            // Reuse `introspect_error`'s own mapping as the predicate rather than re-listing
            // status codes here: everything it calls `IamUnavailable` is a failure to reach a
            // verdict, and everything else (`Unauthenticated`/`PermissionDenied`) is a real
            // rejection. One definition, so the two cannot drift apart.
            api_key_inconclusive = introspect_error(err) == GatewayError::IamUnavailable;
            record_iam_call("introspect", label, started);
        }
    }

    let started = Instant::now();
    match iam.introspect_token(&token).await {
        Ok(resp) if resp.status == "active" => {
            record_iam_call("introspect_token", "ok", started);
            next.run(req).await
        }
        // Belt-and-braces, symmetric with the API-key leg above: a success carrying a non-active
        // status is a rejected credential, not a pass. Currently a dead branch in production (IAM
        // resolve fails closed on anything but `Active`, so a success response never carries a
        // non-active status — see `PrincipalStatus` in `paigasus-iam-core`), but keeping this
        // fail-closed rather than trusting the field is unset costs nothing and guards exactly the
        // kind of future IAM change the doc comment above calls out.
        Ok(_) => {
            record_iam_call("introspect_token", "denied", started);
            preserve_outage(api_key_inconclusive, GatewayError::InvalidCredential).into_response()
        }
        // A validated-but-unprovisioned identity — see the doc comment above. Recorded as
        // "denied" rather than "ok": IAM did reject the RPC, and conflating it with success would
        // hide a genuine provisioning problem from the dashboard.
        Err(IamError::Rpc(ref status)) if status.code() == Code::PermissionDenied => {
            record_iam_call("introspect_token", "denied", started);
            next.run(req).await
        }
        Err(err) => {
            // `IamError` is not `Clone` (and does not get a `Clone` impl just for this): compute
            // the bounded metric label BEFORE `introspect_error` consumes `err`.
            let label = iam_result(&err);
            let mapped = introspect_error(err);
            record_iam_call("introspect_token", label, started);
            preserve_outage(api_key_inconclusive, mapped).into_response()
        }
    }
}

/// Keep an IAM outage visible across [`require_authenticated`]'s two-leg fallback.
///
/// When the API-key leg never reached a verdict, a `401` from the OIDC leg is not trustworthy:
/// the caller may hold a perfectly valid API key that IAM was simply unreachable to check, and
/// the OIDC leg rejects every API key anyway. Report the outage (`503`, retryable) instead of a
/// credential rejection (`401`, permanent) so a client backs off rather than giving up.
///
/// Only ever widens `InvalidCredential` to `IamUnavailable`. An accepted caller is untouched,
/// and an outcome that is already `IamUnavailable` is unchanged.
fn preserve_outage(api_key_inconclusive: bool, mapped: GatewayError) -> GatewayError {
    match mapped {
        GatewayError::InvalidCredential if api_key_inconclusive => GatewayError::IamUnavailable,
        other => other,
    }
}

/// Record an outbound IAM call's outcome for `gateway_iam_calls_total`/`_duration_seconds`.
/// `operation` is `"introspect"` (API-key path, both middlewares), `"introspect_token"` (OIDC
/// token path, [`require_authenticated`] only), or `"authorize"` ([`require_iam_auth`] only);
/// `result` is the bounded label [`iam_result`]/the call sites above produce
/// (`"ok"`/`"denied"`/`"unavailable"`/`"error"`) — never a raw gRPC status string (bounded-
/// cardinality labels only, see the global constraints).
fn record_iam_call(operation: &'static str, result: &'static str, started: Instant) {
    counter!(names::GATEWAY_IAM_CALLS_TOTAL, "operation" => operation, "result" => result).increment(1);
    histogram!(names::GATEWAY_IAM_CALL_DURATION_SECONDS, "operation" => operation).record(started.elapsed().as_secs_f64());
}

/// Map an [`IamError`] to a bounded `result` label for [`record_iam_call`] — never the raw gRPC
/// status/message text. `Unauthenticated` is the one code that maps to `"denied"` here (a
/// rejected credential); every other application-level `Status` (including `PermissionDenied`,
/// which has case-specific meaning per call site — see [`introspect_error`]/[`authz_error`])
/// collapses to `"error"`, distinct from the transport/backend `"unavailable"` bucket.
fn iam_result(err: &IamError) -> &'static str {
    match err {
        IamError::Connect(_) => "unavailable",
        IamError::Rpc(status) if matches!(status.code(), Code::Unavailable | Code::DeadlineExceeded) => "unavailable",
        IamError::Rpc(status) if status.code() == Code::Unauthenticated => "denied",
        IamError::Rpc(_) => "error",
    }
}

/// Extract a bearer credential from an `Authorization` header, independent of the iam crate.
/// Matches IAM's own parser (`adapters/auth.rs::bearer_from_headers`): split on the first space,
/// ASCII-case-insensitive `Bearer` scheme, trim the token, and require it non-empty. Any deviation
/// (absent header, non-UTF-8 value, wrong scheme, empty token) yields `None`.
fn bearer(headers: &HeaderMap) -> Option<String> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    if !scheme.eq_ignore_ascii_case("Bearer") {
        return None;
    }
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    Some(token.to_owned())
}

/// Map an [`IamError`] from the **introspect** call to a [`GatewayError`]. `PermissionDenied` here
/// is an inactive principal on the API-key path — a client-auth failure (401), NOT a 403. Transport
/// / backend codes are retryable (503); a connect-time failure is likewise 503.
fn introspect_error(err: IamError) -> GatewayError {
    match err {
        IamError::Connect(_) => GatewayError::IamUnavailable,
        IamError::Rpc(status) => match status.code() {
            Code::Unauthenticated => GatewayError::InvalidCredential,
            // Inactive principal on the API-key introspect path — a client-auth failure, not a 403.
            Code::PermissionDenied => GatewayError::InvalidCredential,
            Code::Unavailable | Code::DeadlineExceeded | Code::Internal => GatewayError::IamUnavailable,
            _ => GatewayError::IamUnavailable,
        },
    }
}

/// Map an [`IamError`] from the **self-query authz** call to a [`GatewayError`]. Diverges from
/// [`introspect_error`] on `PermissionDenied`: because we ALWAYS self-query, IAM's exposure gate
/// denying us is a plumbing/self-query bug, not a client 403 → 500 (spec §4.3). `Unauthenticated`
/// is still a rejected credential (401); transport/backend/connect codes are retryable (503).
fn authz_error(err: IamError) -> GatewayError {
    match err {
        IamError::Connect(_) => GatewayError::IamUnavailable,
        IamError::Rpc(status) => match status.code() {
            // We only ever self-query, so an exposure-gate denial is our bug, not the caller's.
            Code::PermissionDenied => GatewayError::Internal,
            Code::Unauthenticated => GatewayError::InvalidCredential,
            Code::Unavailable | Code::DeadlineExceeded | Code::Internal => GatewayError::IamUnavailable,
            _ => GatewayError::IamUnavailable,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::Body;
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::middleware::from_fn_with_state;
    use axum::routing::get;
    use paigasus_proto::paigasus::iam::v1::{IntrospectApiKeyResponse, IntrospectResponse};
    use std::sync::Mutex;
    use tower::ServiceExt; // for `oneshot`

    const CALLER_KEY: &str = "sk-caller-secret";
    const CALLER_SA: &str = "prn:paigasus:iam:default:sa/gw-caller";
    const CALLER_SCOPE: &str = "prn:paigasus:iam:default:scope/team-a";
    const CALLER_KEY_ID: &str = "key-abc123";

    /// What the introspect call should return for a test case.
    enum IntrospectOutcome {
        Ok(IntrospectApiKeyResponse),
        /// An IAM gRPC error Status with this code.
        Rpc(Code),
        /// A channel/connect-time failure.
        Connect,
    }

    /// What the OIDC-token introspect (`Iam::introspect_token`) call should return for a test
    /// case — a separate outcome type from [`IntrospectOutcome`] because `IntrospectResponse`
    /// (issuer/subject) is a different message from `IntrospectApiKeyResponse`
    /// (scope_prn/key_id). Exercised only by [`require_authenticated`]'s tests.
    enum TokenIntrospectOutcome {
        Ok(IntrospectResponse),
        /// An IAM gRPC error Status with this code.
        Rpc(Code),
        /// A channel/connect-time failure.
        Connect,
    }

    /// What the self-query authz call should return for a test case.
    enum AuthzOutcome {
        Ok(bool),
        Rpc(Code),
        Connect,
        /// The middleware under test must never reach `is_authorized_self` at all — a hit is a
        /// test bug (or a real regression), not a scenario to model, so it panics loudly. Used by
        /// every `require_authenticated` test: that middleware performs NO authorization.
        Unreachable,
    }

    /// The args recorded from a call to `is_authorized_self` — the self-query proof.
    #[derive(Debug, Clone)]
    struct RecordedAuthz {
        caller_key: String,
        principal_prn: String,
        action: String,
        resource_prn: String,
    }

    /// A canned, recording `Iam` for the decision-table tests. No live IAM. Records the args the
    /// middleware passes to `is_authorized_self` so the security-critical self-query test can
    /// assert them.
    struct FakeIam {
        introspect: IntrospectOutcome,
        /// The `introspect_token` outcome — `None` unless a `require_authenticated` test
        /// configures one via [`FakeIam::with_token_introspect`]; calling `introspect_token`
        /// without configuring it is a test-setup bug, so it panics with a clear message rather
        /// than silently returning something.
        token_introspect: Option<TokenIntrospectOutcome>,
        authz: AuthzOutcome,
        recorded: Arc<Mutex<Option<RecordedAuthz>>>,
    }

    impl FakeIam {
        fn new(introspect: IntrospectOutcome, authz: AuthzOutcome) -> Self {
            Self {
                introspect,
                token_introspect: None,
                authz,
                recorded: Arc::new(Mutex::new(None)),
            }
        }

        /// Configure the `introspect_token` outcome — used by the `require_authenticated` tests
        /// to drive the OIDC-token introspection path (a separate call from `introspect_api_key`).
        fn with_token_introspect(mut self, outcome: TokenIntrospectOutcome) -> Self {
            self.token_introspect = Some(outcome);
            self
        }
    }

    #[async_trait::async_trait]
    impl Iam for FakeIam {
        async fn introspect_api_key(&self, _token: &str) -> Result<IntrospectApiKeyResponse, IamError> {
            match &self.introspect {
                IntrospectOutcome::Ok(resp) => Ok(resp.clone()),
                IntrospectOutcome::Rpc(code) => Err(IamError::Rpc(tonic::Status::new(*code, ""))),
                IntrospectOutcome::Connect => Err(IamError::Connect("test connect failure".to_owned())),
            }
        }

        async fn is_authorized_self(&self, caller_key: &str, principal_prn: &str, action: &str, resource_prn: &str) -> Result<bool, IamError> {
            *self.recorded.lock().unwrap() = Some(RecordedAuthz {
                caller_key: caller_key.to_owned(),
                principal_prn: principal_prn.to_owned(),
                action: action.to_owned(),
                resource_prn: resource_prn.to_owned(),
            });
            match &self.authz {
                AuthzOutcome::Ok(allowed) => Ok(*allowed),
                AuthzOutcome::Rpc(code) => Err(IamError::Rpc(tonic::Status::new(*code, ""))),
                AuthzOutcome::Connect => Err(IamError::Connect("test connect failure".to_owned())),
                AuthzOutcome::Unreachable => panic!("is_authorized_self must not be called by require_authenticated — it performs no authorization"),
            }
        }

        async fn introspect_token(&self, _token: &str) -> Result<IntrospectResponse, IamError> {
            match self
                .token_introspect
                .as_ref()
                .expect("test did not configure a token-introspect outcome via FakeIam::with_token_introspect")
            {
                TokenIntrospectOutcome::Ok(resp) => Ok(resp.clone()),
                TokenIntrospectOutcome::Rpc(code) => Err(IamError::Rpc(tonic::Status::new(*code, ""))),
                TokenIntrospectOutcome::Connect => Err(IamError::Connect("test connect failure".to_owned())),
            }
        }
    }

    /// An `active` introspect response for the canonical caller (SA + scope + key_id).
    fn active_response() -> IntrospectApiKeyResponse {
        IntrospectApiKeyResponse {
            principal_prn: CALLER_SA.to_owned(),
            status: "active".to_owned(),
            key_id: CALLER_KEY_ID.to_owned(),
            expires_at: None,
            memberships: Vec::new(),
            role_grants: Vec::new(),
            scope_prn: CALLER_SCOPE.to_owned(),
        }
    }

    /// An API-key introspection that SUCCEEDED but reports a non-active principal — IAM reached
    /// a verdict, so it is a rejection rather than an outage. Distinguishing the two is what
    /// `preserve_outage` turns on.
    fn inactive_response() -> IntrospectApiKeyResponse {
        IntrospectApiKeyResponse {
            status: "inactive".to_owned(),
            ..active_response()
        }
    }

    /// The probe handler: proves the inner handler sees the `CallerContext` the middleware attached
    /// by echoing its three fields.
    async fn probe(axum::Extension(ctx): axum::Extension<CallerContext>) -> String {
        format!("{}|{}|{}", ctx.principal_prn, ctx.scope_prn, ctx.key_id)
    }

    fn build_app(fake: FakeIam) -> Router {
        Router::new().route("/x", get(probe)).layer(from_fn_with_state(Arc::new(fake) as Arc<dyn Iam>, require_iam_auth))
    }

    fn req_no_auth() -> HttpRequest<Body> {
        HttpRequest::builder().uri("/x").body(Body::empty()).unwrap()
    }

    fn req_with_auth(value: &str) -> HttpRequest<Body> {
        HttpRequest::builder().uri("/x").header(header::AUTHORIZATION, value).body(Body::empty()).unwrap()
    }

    async fn status_of(fake: FakeIam, req: HttpRequest<Body>) -> StatusCode {
        build_app(fake).oneshot(req).await.unwrap().status()
    }

    async fn body_json(resp: Response) -> serde_json::Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    /// A fake whose introspect returns an active caller and whose authz allows — used where the
    /// interesting variable is elsewhere (e.g. the bearer parse, which runs before IAM).
    fn happy_fake() -> FakeIam {
        FakeIam::new(IntrospectOutcome::Ok(active_response()), AuthzOutcome::Ok(true))
    }

    // ---- `require_authenticated` (SMA-505 discovery auth) test helpers -----------------------

    const CONSOLE_TOKEN: &str = "console-oidc-token";
    const CONSOLE_PRINCIPAL: &str = "prn:paigasus:iam:default:user/console-user";

    /// An `active` OIDC-token introspect response for the console-user case (the happy path of
    /// [`Iam::introspect_token`]).
    fn active_token_response() -> IntrospectResponse {
        IntrospectResponse {
            principal_prn: CONSOLE_PRINCIPAL.to_owned(),
            status: "active".to_owned(),
            issuer: "https://issuer.example.com".to_owned(),
            subject: "console-user".to_owned(),
            expires_at: None,
            memberships: Vec::new(),
            role_grants: Vec::new(),
        }
    }

    /// The discovery-path probe handler. Unlike [`probe`], `require_authenticated` attaches no
    /// `CallerContext` (it authenticates without resolving or authorizing an identity), so this
    /// just proves the request reached the handler.
    async fn discovery_probe() -> StatusCode {
        StatusCode::OK
    }

    fn build_discovery_app(fake: FakeIam) -> Router {
        Router::new()
            .route("/x", get(discovery_probe))
            .layer(from_fn_with_state(Arc::new(fake) as Arc<dyn Iam>, require_authenticated))
    }

    async fn discovery_status_of(fake: FakeIam, req: HttpRequest<Body>) -> StatusCode {
        build_discovery_app(fake).oneshot(req).await.unwrap().status()
    }

    /// A fake whose API-key introspect succeeds and whose authz panics if reached at all —
    /// `require_authenticated` must never authorize. Used where the interesting variable is
    /// elsewhere (e.g. the bearer parse, which runs before IAM).
    fn discovery_happy_fake() -> FakeIam {
        FakeIam::new(IntrospectOutcome::Ok(active_response()), AuthzOutcome::Unreachable)
    }

    // ---- `iam_result` bounded-label mapping --------------------------------------------------

    #[test]
    fn iam_result_maps_errors_to_bounded_labels() {
        let cases: &[(IamError, &str)] = &[
            (IamError::Connect("boom".to_owned()), "unavailable"),
            (IamError::Rpc(tonic::Status::new(Code::Unavailable, "")), "unavailable"),
            (IamError::Rpc(tonic::Status::new(Code::DeadlineExceeded, "")), "unavailable"),
            (IamError::Rpc(tonic::Status::new(Code::Unauthenticated, "")), "denied"),
            (IamError::Rpc(tonic::Status::new(Code::PermissionDenied, "")), "error"),
            (IamError::Rpc(tonic::Status::new(Code::Internal, "")), "error"),
            (IamError::Rpc(tonic::Status::new(Code::NotFound, "")), "error"),
        ];
        for (err, want) in cases {
            assert_eq!(iam_result(err), *want, "iam_result({err:?}) should map to {want:?}");
        }
    }

    // ---- bearer extraction / missing-credential rows ----------------------------------------

    #[tokio::test]
    async fn missing_authorization_header_returns_401() {
        assert_eq!(status_of(happy_fake(), req_no_auth()).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn empty_bearer_token_returns_401() {
        assert_eq!(status_of(happy_fake(), req_with_auth("Bearer   ")).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn non_bearer_scheme_returns_401() {
        assert_eq!(status_of(happy_fake(), req_with_auth("Basic dXNlcjpwYXNz")).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn bearer_scheme_is_case_insensitive() {
        // A lowercase scheme must still authenticate (matches IAM's ASCII-case-insensitive parse).
        assert_eq!(status_of(happy_fake(), req_with_auth("bearer sk-caller-secret")).await, StatusCode::OK);
    }

    // ---- introspect rows ---------------------------------------------------------------------

    #[tokio::test]
    async fn introspect_unauthenticated_returns_401() {
        let fake = FakeIam::new(IntrospectOutcome::Rpc(Code::Unauthenticated), AuthzOutcome::Ok(true));
        assert_eq!(status_of(fake, req_with_auth("Bearer bad-key")).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn introspect_permission_denied_returns_401() {
        // Inactive principal on the API-key path is a client-auth failure (401), NOT a 403.
        let fake = FakeIam::new(IntrospectOutcome::Rpc(Code::PermissionDenied), AuthzOutcome::Ok(true));
        assert_eq!(status_of(fake, req_with_auth("Bearer inactive-key")).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn introspect_unavailable_returns_503() {
        let fake = FakeIam::new(IntrospectOutcome::Rpc(Code::Unavailable), AuthzOutcome::Ok(true));
        assert_eq!(status_of(fake, req_with_auth("Bearer any-key")).await, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn introspect_connect_failure_returns_503() {
        let fake = FakeIam::new(IntrospectOutcome::Connect, AuthzOutcome::Ok(true));
        assert_eq!(status_of(fake, req_with_auth("Bearer any-key")).await, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn introspect_success_with_non_active_status_returns_401() {
        let resp = IntrospectApiKeyResponse {
            status: "disabled".to_owned(),
            ..active_response()
        };
        let fake = FakeIam::new(IntrospectOutcome::Ok(resp), AuthzOutcome::Ok(true));
        assert_eq!(status_of(fake, req_with_auth("Bearer disabled-key")).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn introspect_success_with_empty_scope_returns_500() {
        let resp = IntrospectApiKeyResponse {
            scope_prn: String::new(),
            ..active_response()
        };
        let fake = FakeIam::new(IntrospectOutcome::Ok(resp), AuthzOutcome::Ok(true));
        assert_eq!(status_of(fake, req_with_auth("Bearer scopeless-key")).await, StatusCode::INTERNAL_SERVER_ERROR);
    }

    // ---- authz rows --------------------------------------------------------------------------

    #[tokio::test]
    async fn authz_denied_returns_403() {
        let fake = FakeIam::new(IntrospectOutcome::Ok(active_response()), AuthzOutcome::Ok(false));
        assert_eq!(status_of(fake, req_with_auth("Bearer sk-caller-secret")).await, StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn authz_permission_denied_returns_500() {
        // We ALWAYS self-query, so an exposure-gate denial is a plumbing bug, not a client 403.
        let fake = FakeIam::new(IntrospectOutcome::Ok(active_response()), AuthzOutcome::Rpc(Code::PermissionDenied));
        assert_eq!(status_of(fake, req_with_auth("Bearer sk-caller-secret")).await, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn authz_unavailable_returns_503() {
        let fake = FakeIam::new(IntrospectOutcome::Ok(active_response()), AuthzOutcome::Rpc(Code::Unavailable));
        assert_eq!(status_of(fake, req_with_auth("Bearer sk-caller-secret")).await, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn authz_connect_failure_returns_503() {
        let fake = FakeIam::new(IntrospectOutcome::Ok(active_response()), AuthzOutcome::Connect);
        assert_eq!(status_of(fake, req_with_auth("Bearer sk-caller-secret")).await, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn authz_unauthenticated_returns_401() {
        let fake = FakeIam::new(IntrospectOutcome::Ok(active_response()), AuthzOutcome::Rpc(Code::Unauthenticated));
        assert_eq!(status_of(fake, req_with_auth("Bearer sk-caller-secret")).await, StatusCode::UNAUTHORIZED);
    }

    // ---- happy path: reaches the handler with the CallerContext ------------------------------

    #[tokio::test]
    async fn happy_path_reaches_handler_with_caller_context() {
        let resp = build_app(happy_fake()).oneshot(req_with_auth("Bearer sk-caller-secret")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let echoed = String::from_utf8(bytes.to_vec()).unwrap();
        // The inner handler saw the CallerContext the middleware attached.
        assert_eq!(echoed, format!("{CALLER_SA}|{CALLER_SCOPE}|{CALLER_KEY_ID}"));
    }

    // ---- THE self-query assertion (security-critical) ----------------------------------------

    #[tokio::test]
    async fn self_query_uses_caller_key_and_introspected_principal() {
        let fake = FakeIam::new(IntrospectOutcome::Ok(active_response()), AuthzOutcome::Ok(true));
        // Capture the recorder before the fake is erased into `Arc<dyn Iam>`.
        let recorded = fake.recorded.clone();

        let app = Router::new().route("/x", get(probe)).layer(from_fn_with_state(Arc::new(fake) as Arc<dyn Iam>, require_iam_auth));
        let resp = app.oneshot(req_with_auth(&format!("Bearer {CALLER_KEY}"))).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let rec = recorded.lock().unwrap().take().expect("is_authorized_self must have been called on the happy path");
        // A self-query, never cross-principal: the caller's OWN bearer + the introspected SA.
        assert_eq!(rec.caller_key, CALLER_KEY, "authz must present the caller's OWN bearer");
        assert_eq!(rec.principal_prn, CALLER_SA, "authz must query the introspected caller SA, never a different principal");
        assert_eq!(rec.action, INVOKE_MODEL_ACTION);
        assert_eq!(rec.resource_prn, CALLER_SCOPE, "resource must be the introspected scope_prn");
    }

    // ---- error bodies carry the OpenAI envelope shape ----------------------------------------

    #[tokio::test]
    async fn error_responses_carry_the_openai_envelope() {
        let fake = FakeIam::new(IntrospectOutcome::Ok(active_response()), AuthzOutcome::Ok(false));
        let resp = build_app(fake).oneshot(req_with_auth("Bearer sk-caller-secret")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        let body = body_json(resp).await;
        let err = body.get("error").expect("OpenAI envelope has a top-level `error` object");
        assert_eq!(err["type"], "invalid_request_error", "SDKs branch on error.type");
        assert_eq!(err["code"], "insufficient-permissions");
        assert!(err["param"].is_null());
        assert!(err["message"].as_str().is_some_and(|m| !m.is_empty()));
    }

    // ---- `require_authenticated` (SMA-505 discovery auth) -------------------------------------

    /// AC 2: an API key works on the discovery path, and NO authorization call is made — the
    /// fake's `is_authorized_self` panics, so reaching it fails the test loudly.
    #[tokio::test]
    async fn require_authenticated_accepts_an_api_key_without_authorizing() {
        assert_eq!(discovery_status_of(discovery_happy_fake(), req_with_auth(&format!("Bearer {CALLER_KEY}"))).await, StatusCode::OK);
    }

    /// AC 2 + ADR-0020 D4: a console user's OIDC token works. The API-key introspect is tried
    /// first and fails; the token introspect then succeeds.
    #[tokio::test]
    async fn require_authenticated_accepts_an_oidc_token() {
        let fake = FakeIam::new(IntrospectOutcome::Rpc(Code::Unauthenticated), AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Ok(active_token_response()));
        assert_eq!(discovery_status_of(fake, req_with_auth(&format!("Bearer {CONSOLE_TOKEN}"))).await, StatusCode::OK);
    }

    /// D5's deliberate relaxation, and the one most likely to be "fixed" back into a 401 by a
    /// later reader. IAM returns `PermissionDenied` for a VALIDATED token whose identity has no
    /// local principal; on the discovery path that still counts as authenticated, because the
    /// descriptor is byte-identical for every caller and carries no per-principal data.
    #[tokio::test]
    async fn require_authenticated_accepts_a_validated_but_unprovisioned_identity() {
        let fake = FakeIam::new(IntrospectOutcome::Rpc(Code::Unauthenticated), AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Rpc(Code::PermissionDenied));
        assert_eq!(discovery_status_of(fake, req_with_auth("Bearer validated-but-unprovisioned-token")).await, StatusCode::OK);
    }

    /// The relaxation must NOT leak onto the chat path.
    #[tokio::test]
    async fn require_iam_auth_still_rejects_an_unprovisioned_identity() {
        // The same IAM outcome D5 accepts on the discovery path (a VALIDATED credential IAM
        // surfaces as `PermissionDenied`) must still 401 here — `require_iam_auth` is untouched
        // by this task's relaxation and never even calls `introspect_token`.
        let fake = FakeIam::new(IntrospectOutcome::Rpc(Code::PermissionDenied), AuthzOutcome::Unreachable);
        assert_eq!(status_of(fake, req_with_auth("Bearer validated-but-unprovisioned-token")).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn require_authenticated_rejects_a_missing_bearer_with_401() {
        assert_eq!(discovery_status_of(discovery_happy_fake(), req_no_auth()).await, StatusCode::UNAUTHORIZED);
    }

    /// Both introspections failing with a transport error is an IAM outage → 503, and the call
    /// is recorded so the `result="unavailable"` alert can see it.
    #[tokio::test]
    async fn require_authenticated_maps_an_unreachable_iam_to_503() {
        let fake = FakeIam::new(IntrospectOutcome::Connect, AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Connect);
        assert_eq!(discovery_status_of(fake, req_with_auth("Bearer any-token")).await, StatusCode::SERVICE_UNAVAILABLE);
    }

    /// Review finding (Important 1): the most important negative case for a new authentication
    /// middleware was missing — neither leg accepts, and neither is a transport failure. Both
    /// introspections come back a plain `Unauthenticated` (a garbage credential that is neither a
    /// valid API key nor a valid OIDC token), so the `Err(err) => introspect_error(err)` catch-all
    /// must map it to `401`, not silently let it through.
    #[tokio::test]
    async fn require_authenticated_rejects_an_invalid_credential_with_401() {
        let fake = FakeIam::new(IntrospectOutcome::Rpc(Code::Unauthenticated), AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Rpc(Code::Unauthenticated));
        assert_eq!(discovery_status_of(fake, req_with_auth("Bearer garbage")).await, StatusCode::UNAUTHORIZED);
    }

    /// CodeRabbit, PR 124: an IAM outage on the API-key leg must survive the OIDC fallback.
    ///
    /// This is not a corner case — it is the ORDINARY shape of a transient failure for an
    /// API-key caller. An API key is never a valid JWT, so `introspect_token` answers
    /// `Unauthenticated` regardless of how valid the key is; without `preserve_outage` the
    /// middleware would report `401` (permanent, stop retrying) for a caller whose credential
    /// IAM merely failed to check. Every retryable API-key-leg failure is covered, because
    /// `introspect_error` maps each of them to `IamUnavailable`.
    #[tokio::test]
    async fn an_inconclusive_api_key_leg_reports_the_outage_rather_than_a_401() {
        for outcome in [
            IntrospectOutcome::Connect,
            IntrospectOutcome::Rpc(Code::Unavailable),
            IntrospectOutcome::Rpc(Code::DeadlineExceeded),
            IntrospectOutcome::Rpc(Code::Internal),
        ] {
            let fake = FakeIam::new(outcome, AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Rpc(Code::Unauthenticated));
            assert_eq!(
                discovery_status_of(fake, req_with_auth("Bearer pgs_sk_a-real-key-iam-could-not-check")).await,
                StatusCode::SERVICE_UNAVAILABLE,
                "a retryable API-key-leg failure must not become a 401 via the OIDC fallback"
            );
        }
    }

    /// The control for the test above, and what stops it from being a blanket "always 503":
    /// when the API-key leg reaches a real VERDICT, the `401` must survive. Without this pair,
    /// `preserve_outage` could widen every rejection to `503` and both tests would still pass.
    #[tokio::test]
    async fn a_conclusive_api_key_rejection_still_yields_401_not_503() {
        for outcome in [
            IntrospectOutcome::Rpc(Code::Unauthenticated),
            IntrospectOutcome::Rpc(Code::PermissionDenied),
            IntrospectOutcome::Ok(inactive_response()),
        ] {
            let fake = FakeIam::new(outcome, AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Rpc(Code::Unauthenticated));
            assert_eq!(
                discovery_status_of(fake, req_with_auth("Bearer garbage")).await,
                StatusCode::UNAUTHORIZED,
                "a definitive API-key rejection must stay a 401 — the outage widening is not unconditional"
            );
        }
    }

    /// Review finding (Minor 3): pins the symmetric active-status check added to the
    /// `introspect_token` leg. A dead branch in production today (IAM's `resolve` fails closed on
    /// anything but `Active`, so a success response never carries a non-active status), but the
    /// belt-and-braces check exists precisely so a future IAM change can't silently let a
    /// non-active OIDC identity through — this test proves that guard actually rejects, not just
    /// that it compiles.
    #[tokio::test]
    async fn require_authenticated_rejects_a_non_active_oidc_token_with_401() {
        let non_active = IntrospectResponse {
            status: "disabled".to_owned(),
            ..active_token_response()
        };
        let fake = FakeIam::new(IntrospectOutcome::Rpc(Code::Unauthenticated), AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Ok(non_active));
        assert_eq!(discovery_status_of(fake, req_with_auth(&format!("Bearer {CONSOLE_TOKEN}"))).await, StatusCode::UNAUTHORIZED);
    }
}
