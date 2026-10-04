// SPDX-License-Identifier: Apache-2.0

//! The gateway's authentication + authorization middleware — the security crux of the M0 slice.
//!
//! [`require_iam_auth`] runs BEFORE the chat handler on every protected request. It accepts two
//! credentials, tried in this order — the same order as [`require_authenticated`], so the two
//! middlewares cannot drift (SMA-635 D1):
//!
//! 1. **An API key** (a service account). `IntrospectApiKey` answers `active`: the key's own
//!    `scope_prn` is the scope. A `paigasus-org` header is ignored, with one warning (D5): a key is
//!    issued under one scope, and the header must not let its holder choose another resource.
//! 2. **An OIDC access token** (a console user, or a machine client with an OIDC token).
//!    `Introspect` answers `active`: the scope is ONE organization, from the `paigasus-org`
//!    header, or inferred when the user reaches exactly one organization (D2, D3). CONSEQUENCE OF
//!    D3: a client that sends no header works while its user has one organization, and starts to
//!    get `400 org-required` when the user joins a second one. A client that knows the
//!    organization must send the header.
//!
//! Both paths then run the D9 *self-query* and attach the resolved [`CallerContext`]. Any failure
//! short-circuits with a [`GatewayError`] rendered through the OpenAI error envelope.
//!
//! A user turn costs three IAM RPCs (`IntrospectApiKey`, `Introspect`, `IsAuthorized`); an
//! API-key turn costs two. There is no cache (SMA-635 spec §4.1).
//!
//! ## The DPoP scheme (SMA-700)
//! With gateway DPoP on, `Authorization: DPoP <token>` plus one `DPoP` header skips the API-key leg
//! and sends the proof, the method and the path to `Introspect`. The self-query then sends the
//! same token and proof as IAM's `IsAuthorized` follow-up. Every 401 carries one
//! `WWW-Authenticate: DPoP` line (`dpop_challenge`).
//!
//! ## The self-query invariant (D9 — the whole point)
//! The authorization call ([`Iam::is_authorized_self`]) is made with the caller's OWN bearer as
//! the credential AND the caller's OWN principal PRN (the one IAM's introspect response just
//! returned) as the queried principal. Because IAM resolves that bearer to the same principal the
//! request names, it sees a principal asking about *itself* and applies no cross-principal
//! exposure gate. The middleware sources both from the SAME inbound request, so it can never query
//! a principal other than the authenticated caller — unit tests prove the recorded authz args for
//! both credentials.
//!
//! ## Two different IAM-error → HTTP mappings
//! The introspect and authz calls map gRPC status codes DIFFERENTLY, and they diverge on
//! `PermissionDenied`: on introspect it means an inactive or unprovisioned principal (a
//! client-auth failure → 401). On the authz call it is read by its `ErrorInfo` reason:
//! `principal-inactive` and `provisioning-failed` are IAM's bearer enforcement rejecting an OIDC
//! caller (→ 401); anything else means IAM's exposure gate denied our self-query, a plumbing bug
//! (→ 500). See [`introspect_error`] and [`authz_error`].

use std::sync::{Arc, LazyLock};
use std::time::Instant;

use axum::extract::{OriginalUri, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use metrics::{counter, histogram};
use paigasus_observability::names;
use tonic::{Code, Status};
use tonic_types::StatusExt;

use super::error::GatewayError;
use crate::adapters::iam::{CallerCredential, DpopContext, Iam, IamError};
use crate::domain::{CallerContext, Credential, OrgHeader, resolve_org};
use paigasus_proto::paigasus::iam::v1::{IntrospectApiKeyResponse, IntrospectResponse};

/// The wire action string the gateway authorizes every chat request against. Hardcoded because the
/// gateway cannot import iam-core's `Action` enum across the gRPC boundary — it sends the literal
/// wire form (`Action::InvokeModel.as_wire() == "InvokeModel"`, verified in the integration facts).
const INVOKE_MODEL_ACTION: &str = "InvokeModel";

/// The request header an OIDC caller names its organization with (SMA-635 D2): one organization
/// UUID in the 36-character form.
pub const ORG_HEADER: &str = "paigasus-org";

/// The auth middlewares' state (SMA-700 § 4.10): the IAM port and the DPoP switch. Independent of
/// the handler's `AppState`, as before.
#[derive(Clone)]
pub struct AuthState {
    pub iam: Arc<dyn Iam>,
    pub dpop_enabled: bool,
}

/// Which `Authorization` scheme the client used, for the challenge (SMA-700 § 4.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SchemeUsed {
    /// No usable credential (absent, unknown scheme, empty token, or `DPoP` while DPoP is off).
    None,
    Bearer,
    Dpop,
}

impl SchemeUsed {
    fn of(credentials: Option<&Credentials>) -> Self {
        match credentials {
            None => SchemeUsed::None,
            Some(Credentials::Bearer(_)) => SchemeUsed::Bearer,
            Some(Credentials::Dpop { .. }) => SchemeUsed::Dpop,
        }
    }
}

const CHALLENGE_INVALID_PROOF: &str = "DPoP error=\"invalid_dpop_proof\", algs=\"ES256 RS256\"";
const CHALLENGE_INVALID_TOKEN: &str = "DPoP error=\"invalid_token\", algs=\"ES256 RS256\"";
const CHALLENGE_BARE: &str = "DPoP algs=\"ES256 RS256\"";

/// The `WWW-Authenticate` value of a refusal (SMA-700 § 4.10), or `None`. Only when gateway DPoP
/// is on, and only on a 401. The error attribute goes on the scheme the client used (RFC 6750
/// § 3): a Bearer client or one with no credential gets the bare DPoP challenge. With DPoP off the
/// gateway sends no challenge, as before (D11). `algs` is D8's list.
pub fn dpop_challenge(dpop_enabled: bool, scheme: SchemeUsed, err: &GatewayError) -> Option<HeaderValue> {
    if !dpop_enabled || err.status() != StatusCode::UNAUTHORIZED {
        return None;
    }
    let value = match (scheme, err) {
        (SchemeUsed::Dpop, GatewayError::InvalidDpopProof) => CHALLENGE_INVALID_PROOF,
        (SchemeUsed::Dpop, _) => CHALLENGE_INVALID_TOKEN,
        (SchemeUsed::Bearer | SchemeUsed::None, _) => CHALLENGE_BARE,
    };
    Some(HeaderValue::from_static(value))
}

/// Render a refusal, with the challenge appended after `into_response` (`IntoResponse` does not
/// change; the gateway sends one line, through `append`).
fn reject(dpop_enabled: bool, scheme: SchemeUsed, err: GatewayError) -> Response {
    let challenge = dpop_challenge(dpop_enabled, scheme, &err);
    let mut response = err.into_response();
    if let Some(value) = challenge {
        response.headers_mut().append(header::WWW_AUTHENTICATE, value);
    }
    response
}

/// The proof value of a DPoP request. A missing or invalid `DPoP` header is a 401 at once, with
/// no IAM call (§ 4.10).
fn proof_value(proof: ProofHeader) -> Result<String, GatewayError> {
    match proof {
        ProofHeader::One(proof) => Ok(proof),
        ProofHeader::Missing | ProofHeader::Invalid => Err(GatewayError::InvalidDpopProof),
    }
}

/// The DPoP context that IAM checks (§ 4.10, D5): the proof, the method, and the path as the
/// gateway received it, with no query (`OriginalUri`, so a nested router cannot shorten it).
fn dpop_context(req: &Request, proof: &str) -> DpopContext {
    let uri = req.extensions().get::<OriginalUri>().map_or(req.uri(), |original| &original.0);
    DpopContext {
        proof: proof.to_owned(),
        method: req.method().as_str().to_owned(),
        path: uri.path().to_owned(),
    }
}

/// Authenticate + authorize a request before it reaches the protected handler. Wired via
/// `from_fn_with_state(AuthState { iam, dpop_enabled }, require_iam_auth)`. On success the request
/// carries a [`CallerContext`] extension; on any failure it returns the mapped [`GatewayError`],
/// with the DPoP challenge when gateway DPoP is on.
pub async fn require_iam_auth(State(auth): State<AuthState>, req: Request, next: Next) -> Response {
    let credentials = credentials(req.headers(), auth.dpop_enabled);
    let scheme = SchemeUsed::of(credentials.as_ref());
    match iam_auth(auth.iam.as_ref(), credentials, req, next).await {
        Ok(response) => response,
        Err(err) => reject(auth.dpop_enabled, scheme, err),
    }
}

async fn iam_auth(iam: &dyn Iam, credentials: Option<Credentials>, req: Request, next: Next) -> Result<Response, GatewayError> {
    match credentials {
        None => Err(GatewayError::MissingBearer),
        Some(Credentials::Bearer(token)) => bearer_iam_auth(iam, token, req, next).await,
        Some(Credentials::Dpop { token, proof }) => {
            // D15: no API-key leg on the DPoP scheme; API keys are not bound to a key, and the
            // proof goes to IAM on the token leg only.
            let proof = proof_value(proof)?;
            let context = dpop_context(&req, &proof);
            let started = Instant::now();
            let user = match iam.introspect_token(&token, Some(context)).await {
                Ok(resp) if resp.status == "active" => {
                    record_iam_call("introspect_token", "ok", started);
                    resp
                }
                Ok(_) => {
                    record_iam_call("introspect_token", "denied", started);
                    return Err(GatewayError::InvalidCredential);
                }
                Err(err) => {
                    record_iam_call("introspect_token", iam_result(&err), started);
                    return Err(introspect_error(err));
                }
            };
            // IAM's IsAuthorized follow-up: the same token and the same proof bytes (§ 4.8).
            oidc_caller(iam, CallerCredential::Dpop { token, proof }, user, req, next).await
        }
    }
}

/// The Bearer scheme: the API-key leg, then the OIDC leg (SMA-635), unchanged.
async fn bearer_iam_auth(iam: &dyn Iam, token: String, req: Request, next: Next) -> Result<Response, GatewayError> {
    // 2. The API-key leg. An `active` key continues on the unchanged API-key path. Anything else
    //    falls through to the OIDC leg; `api_key_inconclusive` records whether this leg failed to
    //    reach a VERDICT, with the same rule `require_authenticated` uses.
    let started = Instant::now();
    let mut api_key_inconclusive = false;
    match iam.introspect_api_key(&token).await {
        Ok(resp) if resp.status == "active" => {
            record_iam_call("introspect", "ok", started);
            return api_key_caller(iam, &token, resp, req, next).await;
        }
        // IAM answered, and the answer was "not active" — a verdict, not an outage.
        Ok(_) => record_iam_call("introspect", "denied", started),
        Err(err) => {
            let label = iam_result(&err);
            api_key_inconclusive = introspect_error(err) == GatewayError::IamUnavailable;
            record_iam_call("introspect", label, started);
        }
    }

    // 3. The OIDC leg. Unlike `require_authenticated`, `identity-not-provisioned` is REJECTED
    //    here: a user with no provisioned identity has no grants. `introspect_error` maps that
    //    `PermissionDenied` to a 401, and `preserve_outage` widens it to a 503 when the key leg
    //    never reached a verdict.
    let started = Instant::now();
    let user = match iam.introspect_token(&token, None).await {
        Ok(resp) if resp.status == "active" => {
            record_iam_call("introspect_token", "ok", started);
            resp
        }
        Ok(_) => {
            record_iam_call("introspect_token", "denied", started);
            return Err(preserve_outage(api_key_inconclusive, GatewayError::InvalidCredential));
        }
        Err(err) => {
            let label = iam_result(&err);
            let mapped = introspect_error(err);
            record_iam_call("introspect_token", label, started);
            return Err(preserve_outage(api_key_inconclusive, mapped));
        }
    };
    oidc_caller(iam, CallerCredential::Bearer(token), user, req, next).await
}

/// Steps 4-6 of the OIDC leg, shared by the Bearer and the DPoP scheme: the organization, the
/// self-query, the caller context.
async fn oidc_caller(iam: &dyn Iam, caller: CallerCredential, user: IntrospectResponse, mut req: Request, next: Next) -> Result<Response, GatewayError> {
    // 4. The organization (spec §4.2). Every membership node and every grant scope is evidence
    //    for inference; the header, when present, wins and is checked by IAM in step 5.
    let org_prn = {
        let node_prns: Vec<&str> = user
            .memberships
            .iter()
            .map(|m| m.node_prn.as_str())
            .chain(user.role_grants.iter().map(|g| g.scope_prn.as_str()))
            .collect();
        org_header(req.headers()).and_then(|header| resolve_org(header, &node_prns).map_err(GatewayError::from))?.canonical()
    };
    let principal_prn = user.principal_prn;

    // 5. The self-query against the ORG (D4): an org UUID that does not exist is a Deny, not an
    //    error, so the answer does not show whether the org exists.
    authorize_self(iam, &caller, &principal_prn, &org_prn, None).await?;

    // 6. Attach the resolved caller and proceed.
    req.extensions_mut().insert(CallerContext {
        principal_prn,
        scope_prn: org_prn,
        credential: Credential::Oidc,
    });
    Ok(next.run(req).await)
}

/// The API-key path, unchanged since SMA-446 apart from the D5 warning: the key's own
/// `scope_prn` is the scope, and a `paigasus-org` header is never read for it.
async fn api_key_caller(iam: &dyn Iam, token: &str, resp: IntrospectApiKeyResponse, mut req: Request, next: Next) -> Result<Response, GatewayError> {
    if resp.scope_prn.is_empty() {
        // A missing scope is a plumbing bug (introspect should always return one), surfaced as a
        // distinct 500 diagnostic rather than a silent deny. The response body stays generic.
        tracing::error!(
            principal_prn = %resp.principal_prn,
            key_id = %resp.key_id,
            "introspect returned an empty scope_prn — IAM plumbing bug (SMA-446 D11)"
        );
        return Err(GatewayError::MissingScope);
    }
    if req.headers().contains_key(ORG_HEADER) {
        // D5. The header VALUE is never logged: it is caller input.
        tracing::warn!(key_id = %resp.key_id, "paigasus-org ignored for an API key");
    }
    authorize_self(iam, &CallerCredential::ApiKey(token.to_owned()), &resp.principal_prn, &resp.scope_prn, Some(&resp.key_id)).await?;
    req.extensions_mut().insert(CallerContext {
        principal_prn: resp.principal_prn,
        scope_prn: resp.scope_prn,
        credential: Credential::ApiKey { key_id: resp.key_id },
    });
    Ok(next.run(req).await)
}

/// The D9 self-query, shared by every credential: the caller's OWN credential, the caller's OWN
/// introspected principal, `InvokeModel`, and the resolved scope.
async fn authorize_self(iam: &dyn Iam, caller: &CallerCredential, principal_prn: &str, scope_prn: &str, key_id: Option<&str>) -> Result<(), GatewayError> {
    let started = Instant::now();
    match iam.is_authorized_self(caller, principal_prn, INVOKE_MODEL_ACTION, scope_prn).await {
        Ok(true) => {
            record_iam_call("authorize", "ok", started);
            Ok(())
        }
        Ok(false) => {
            record_iam_call("authorize", "denied", started);
            Err(GatewayError::AuthzDenied)
        }
        Err(err) => {
            record_iam_call("authorize", iam_result(&err), started);
            let mapped = authz_error(err);
            if mapped == GatewayError::Internal {
                // An exposure-gate denial of a self-query should be impossible — log it, since this
                // is the single most important thing to see (a broken D9 self-query). `key_id` is
                // logged plainly for an API key; an OIDC caller has none, so the field reads `-`
                // rather than the `Some(..)`/`None` Debug wrapper.
                tracing::error!(
                    principal_prn = %principal_prn,
                    key_id = %key_id.unwrap_or("-"),
                    "self-query IsAuthorized returned an unexpected error mapped to 500 — possible broken self-query (SMA-446 D9)"
                );
            }
            Err(mapped)
        }
    }
}

/// Read the `paigasus-org` header into an [`OrgHeader`]. A value that is not visible ASCII is an
/// invalid header at once (`HeaderValue::to_str` fails on an obs-text byte).
fn org_header(headers: &HeaderMap) -> Result<OrgHeader<'_>, GatewayError> {
    let mut values = headers.get_all(ORG_HEADER).iter();
    let Some(first) = values.next() else {
        return Ok(OrgHeader::Absent);
    };
    if values.next().is_some() {
        return Ok(OrgHeader::Many);
    }
    first.to_str().map(OrgHeader::One).map_err(|_| GatewayError::InvalidOrgHeader)
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
/// caller, and exposes no per-principal data, so accepting widens nothing. This relaxation is
/// scoped to THIS middleware: [`require_iam_auth`] also tries the OIDC leg (SMA-635), but REJECTS
/// an unprovisioned identity, because a user with no provisioned identity holds no grant.
///
/// ### Which `PermissionDenied` is accepted
/// Exactly one: IAM's `identity-not-provisioned`, read from `ErrorInfo` (SMA-504). The other two
/// reasons that share `PermissionDenied` — `provisioning-failed` and `principal-inactive` — are
/// rejected, as is a `Status` carrying no details at all. This replaces the blanket
/// code-only accept, which was correct only by reachability accident.
pub async fn require_authenticated(State(auth): State<AuthState>, req: Request, next: Next) -> Response {
    let credentials = credentials(req.headers(), auth.dpop_enabled);
    let scheme = SchemeUsed::of(credentials.as_ref());
    match authenticated(auth.iam.as_ref(), credentials, req, next).await {
        Ok(response) => response,
        Err(err) => reject(auth.dpop_enabled, scheme, err),
    }
}

async fn authenticated(iam: &dyn Iam, credentials: Option<Credentials>, req: Request, next: Next) -> Result<Response, GatewayError> {
    match credentials {
        None => Err(GatewayError::MissingBearer),
        Some(Credentials::Bearer(token)) => bearer_authenticated(iam, &token, req, next).await,
        // D15: no API-key leg. The `identity-not-provisioned` rule stays: IAM answers it only
        // after the proof check passed (D18).
        Some(Credentials::Dpop { token, proof }) => {
            let proof = proof_value(proof)?;
            let context = dpop_context(&req, &proof);
            token_authenticated(iam, &token, Some(context), false, req, next).await
        }
    }
}

async fn bearer_authenticated(iam: &dyn Iam, token: &str, req: Request, next: Next) -> Result<Response, GatewayError> {
    let started = Instant::now();
    // Whether the API-key leg failed to reach a VERDICT, as opposed to reaching a rejection.
    // An outage here must not be laundered into a `401` by the fallback below: an API key is
    // never a valid JWT, so the OIDC leg answers `Unauthenticated` for an API-key caller no
    // matter how valid the key is. Without this flag the ORDINARY outcome of a transient
    // API-key-leg failure is a definitive-looking `401` telling the client to stop retrying —
    // during exactly the outage it should be backing off through.
    let mut api_key_inconclusive = false;
    match iam.introspect_api_key(token).await {
        Ok(resp) if resp.status == "active" => {
            record_iam_call("introspect", "ok", started);
            return Ok(next.run(req).await);
        }
        // IAM answered, and the answer was "not active" — a verdict, not an outage.
        Ok(_) => record_iam_call("introspect", "denied", started),
        Err(err) => {
            // `IamError` is not `Clone`, so compute the bounded metric label before
            // `introspect_error` consumes `err`. Reuse `introspect_error`'s own mapping as the
            // predicate: everything it calls `IamUnavailable` is a failure to reach a verdict.
            let label = iam_result(&err);
            api_key_inconclusive = introspect_error(err) == GatewayError::IamUnavailable;
            record_iam_call("introspect", label, started);
        }
    }
    token_authenticated(iam, token, None, api_key_inconclusive, req, next).await
}

/// The token leg of discovery: an active identity, or a VALIDATED but unprovisioned one (see the
/// doc of [`require_authenticated`]), reaches the handler.
async fn token_authenticated(iam: &dyn Iam, token: &str, dpop: Option<DpopContext>, api_key_inconclusive: bool, req: Request, next: Next) -> Result<Response, GatewayError> {
    let started = Instant::now();
    match iam.introspect_token(token, dpop).await {
        Ok(resp) if resp.status == "active" => {
            record_iam_call("introspect_token", "ok", started);
            Ok(next.run(req).await)
        }
        // Belt-and-braces: a success carrying a non-active status is a rejected credential, not a
        // pass. Keeping this fail-closed costs nothing and guards a future IAM change.
        Ok(_) => {
            record_iam_call("introspect_token", "denied", started);
            Err(preserve_outage(api_key_inconclusive, GatewayError::InvalidCredential))
        }
        // A validated-but-unprovisioned identity. Recorded as "denied" rather than "ok": IAM did
        // reject the RPC, and conflating it with success would hide a provisioning problem.
        Err(IamError::Rpc(ref status)) if is_identity_not_provisioned(status) => {
            record_iam_call("introspect_token", "denied", started);
            Ok(next.run(req).await)
        }
        Err(err) => {
            let label = iam_result(&err);
            let mapped = introspect_error(err);
            record_iam_call("introspect_token", label, started);
            Err(preserve_outage(api_key_inconclusive, mapped))
        }
    }
}

/// The reasons IAM sends on `PermissionDenied`, hoisted into `LazyLock`s because `as_wire_reason`
/// allocates. `LazyLock<String>` + `.expect(...)`, matching the registry-derived statics in
/// `paigasus-iam`'s `adapters/grpc/convert.rs:33-49`: if the registry stopped declaring a reason,
/// an `Option` would make every comparison below silently false (review finding #8).
static IDENTITY_NOT_PROVISIONED: LazyLock<String> = LazyLock::new(|| {
    paigasus_proto::paigasus::common::v1::ErrorReason::IdentityNotProvisioned
        .as_wire_reason()
        .expect("a declared reason is never the sentinel")
});
static PRINCIPAL_INACTIVE: LazyLock<String> = LazyLock::new(|| {
    paigasus_proto::paigasus::common::v1::ErrorReason::PrincipalInactive
        .as_wire_reason()
        .expect("a declared reason is never the sentinel")
});
static PROVISIONING_FAILED: LazyLock<String> = LazyLock::new(|| {
    paigasus_proto::paigasus::common::v1::ErrorReason::ProvisioningFailed
        .as_wire_reason()
        .expect("a declared reason is never the sentinel")
});

static INVALID_DPOP_PROOF: LazyLock<String> = LazyLock::new(|| {
    paigasus_proto::paigasus::common::v1::ErrorReason::InvalidDpopProof
        .as_wire_reason()
        .expect("a declared reason is never the sentinel")
});
static DPOP_QUOTA_EXCEEDED: LazyLock<String> = LazyLock::new(|| {
    paigasus_proto::paigasus::common::v1::ErrorReason::DpopQuotaExceeded
        .as_wire_reason()
        .expect("a declared reason is never the sentinel")
});

/// The `ErrorInfo` reason of an IAM `Status`, read only on the IAM domain — never the message
/// string — and with no log line. For `Unauthenticated` and `ResourceExhausted` (SMA-700), where a
/// status with no `ErrorInfo` is ordinary and must not write the SMA-504 skew warning.
fn iam_reason_quiet(status: &Status) -> Option<String> {
    let details = status.get_error_details();
    let info = details.error_info()?;
    (info.domain == *paigasus_proto::error::IAM_DOMAIN).then(|| info.reason.clone())
}

/// [`iam_reason_quiet`] for a `PermissionDenied`: a status with no `ErrorInfo` there is version
/// skew (IAM must roll before the gateway, SMA-504), so it writes one warning.
fn iam_reason(status: &Status) -> Option<String> {
    if status.get_error_details().error_info().is_none() {
        tracing::warn!("IAM returned a PermissionDenied with no ErrorInfo — rolling-upgrade skew? (SMA-504)");
        return None;
    }
    iam_reason_quiet(status)
}

/// IAM's DPoP quota refusal (SMA-700 D10): `ResourceExhausted` with `dpop-quota-exceeded`.
fn is_dpop_quota(status: &Status) -> bool {
    status.code() == Code::ResourceExhausted && iam_reason_quiet(status).as_deref() == Some(DPOP_QUOTA_EXCEEDED.as_str())
}

/// The `RetryInfo` delay in whole seconds, at least 1; 1 when IAM sent none.
fn retry_after_secs(status: &Status) -> u32 {
    status
        .get_error_details()
        .retry_info()
        .and_then(|info| info.retry_delay)
        .map_or(1, |delay| u32::try_from(delay.as_secs()).unwrap_or(u32::MAX).max(1))
}

/// An IAM `Unauthenticated`: a refused DPoP proof keeps its own code; every other one is the
/// credential rejection it was before.
fn unauthenticated_error(status: &Status) -> GatewayError {
    if iam_reason_quiet(status).as_deref() == Some(INVALID_DPOP_PROOF.as_str()) {
        GatewayError::InvalidDpopProof
    } else {
        GatewayError::InvalidCredential
    }
}

/// An IAM `ResourceExhausted`: the DPoP quota is the gateway's own 429 `rate-limited` with
/// `Retry-After` (D10); anything else stays an IAM failure.
fn resource_exhausted_error(status: &Status) -> GatewayError {
    if is_dpop_quota(status) {
        GatewayError::RateLimited {
            retry_after_secs: retry_after_secs(status),
        }
    } else {
        GatewayError::IamUnavailable
    }
}

/// Is this IAM `Status` specifically "validated, but not yet provisioned"? SMA-504 discharges
/// ADR-0020 D4's tripwire: the reason decides, and a `Status` with no `ErrorInfo` fails CLOSED.
fn is_identity_not_provisioned(status: &Status) -> bool {
    status.code() == Code::PermissionDenied && iam_reason(status).as_deref() == Some(IDENTITY_NOT_PROVISIONED.as_str())
}

/// Keep an IAM outage visible across the two-leg fallback, in BOTH [`require_authenticated`] and
/// [`require_iam_auth`] (SMA-635) — the same rule serves each middleware's own key-leg/OIDC-leg
/// pair.
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
/// `operation` is `"introspect"` (the API-key leg, both middlewares), `"introspect_token"` (the
/// OIDC leg, both middlewares since SMA-635), or `"authorize"` (the self-query,
/// [`require_iam_auth`] only); `result` is the bounded label [`iam_result`]/the call sites above
/// produce (`"ok"`/`"denied"`/`"unavailable"`/`"error"`) — never a raw gRPC status string.
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
        // SMA-700 D10: a quota hit is a verdict about one client, not an outage.
        IamError::Rpc(status) if is_dpop_quota(status) => "denied",
        IamError::Rpc(_) => "error",
    }
}

/// The DPoP proof request header (RFC 9449 § 4.1), in lower case as `HeaderMap` stores it.
pub const DPOP_HEADER: &str = "dpop";

/// The `DPoP` header of a DPoP request. `Debug` prints no proof.
#[derive(Clone, PartialEq, Eq)]
pub enum ProofHeader {
    One(String),
    Missing,
    /// Two or more `DPoP` headers, a value that is not visible ASCII, or an empty value.
    Invalid,
}

impl std::fmt::Debug for ProofHeader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ProofHeader::One(_) => "One(..)",
            ProofHeader::Missing => "Missing",
            ProofHeader::Invalid => "Invalid",
        })
    }
}

/// The credential of a request. `Debug` prints no secret.
#[derive(Clone, PartialEq, Eq)]
pub enum Credentials {
    Bearer(String),
    Dpop { token: String, proof: ProofHeader },
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Credentials::Bearer(_) => f.write_str("Bearer(..)"),
            Credentials::Dpop { proof, .. } => write!(f, "Dpop {{ token: .., proof: {proof:?} }}"),
        }
    }
}

/// Parse the `Authorization` header (SMA-700 § 4.10, decision P7). Matches IAM's own parser
/// (`adapters/auth.rs`): split on the first space, an ASCII-case-insensitive scheme, the token
/// trimmed and not empty. `Bearer` ignores a `DPoP` header. The `DPoP` scheme counts only when
/// gateway DPoP is on; else it is `None`, as today (D11).
pub fn credentials(headers: &HeaderMap, dpop_enabled: bool) -> Option<Credentials> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    if scheme.eq_ignore_ascii_case("Bearer") {
        return Some(Credentials::Bearer(token.to_owned()));
    }
    if dpop_enabled && scheme.eq_ignore_ascii_case("DPoP") {
        return Some(Credentials::Dpop {
            token: token.to_owned(),
            proof: proof_header(headers),
        });
    }
    None
}

fn proof_header(headers: &HeaderMap) -> ProofHeader {
    let mut values = headers.get_all(DPOP_HEADER).iter();
    let Some(first) = values.next() else {
        return ProofHeader::Missing;
    };
    if values.next().is_some() {
        return ProofHeader::Invalid;
    }
    match first.to_str() {
        Ok(proof) if !proof.is_empty() => ProofHeader::One(proof.to_owned()),
        _ => ProofHeader::Invalid,
    }
}

/// Map an [`IamError`] from the **introspect** call (either leg: `introspect_api_key` or
/// `introspect_token`, SMA-635) to a [`GatewayError`]. `PermissionDenied` here is an inactive or
/// unprovisioned principal — a client-auth failure (401), NOT a 403. Transport / backend codes are
/// retryable (503); a connect-time failure is likewise 503.
fn introspect_error(err: IamError) -> GatewayError {
    match err {
        IamError::Connect(_) => GatewayError::IamUnavailable,
        IamError::Rpc(status) => match status.code() {
            Code::Unauthenticated => unauthenticated_error(&status),
            // Inactive/unprovisioned principal on either introspect leg — a client-auth failure,
            // not a 403.
            Code::PermissionDenied => GatewayError::InvalidCredential,
            Code::ResourceExhausted => resource_exhausted_error(&status),
            Code::Unavailable | Code::DeadlineExceeded | Code::Internal => GatewayError::IamUnavailable,
            _ => GatewayError::IamUnavailable,
        },
    }
}

/// Map an [`IamError`] from the **self-query authz** call to a [`GatewayError`]. Diverges from
/// [`introspect_error`] on `PermissionDenied`, which is read by its IAM reason (SMA-635 spec §4.1
/// step 5): `principal-inactive` and `provisioning-failed` are IAM's bearer enforcement rejecting
/// an OIDC caller (`authn.rs:217-241`) → 401. Any other `PermissionDenied` — `forbidden`, or no
/// `ErrorInfo` at all — means IAM's exposure gate denied a self-query, our bug → 500.
fn authz_error(err: IamError) -> GatewayError {
    match err {
        IamError::Connect(_) => GatewayError::IamUnavailable,
        IamError::Rpc(status) => match status.code() {
            Code::PermissionDenied => match iam_reason(&status).as_deref() {
                Some(reason) if reason == PRINCIPAL_INACTIVE.as_str() || reason == PROVISIONING_FAILED.as_str() => GatewayError::InvalidCredential,
                _ => GatewayError::Internal,
            },
            Code::Unauthenticated => unauthenticated_error(&status),
            Code::ResourceExhausted => resource_exhausted_error(&status),
            Code::Unavailable | Code::DeadlineExceeded | Code::Internal => GatewayError::IamUnavailable,
            _ => GatewayError::IamUnavailable,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapters::iam::DpopContext;
    use axum::Router;
    use axum::body::Body;
    use axum::http::HeaderValue;
    use axum::http::{Request as HttpRequest, StatusCode};
    use axum::middleware::from_fn_with_state;
    use axum::routing::get;
    // TRACE (the default of `capture_logs`), not INFO: row 9 asserts that the `paigasus-org`
    // header value is never logged, and only TRACE makes that check see every level.
    use paigasus_logging::test_support::capture_logs;
    use paigasus_proto::paigasus::iam::v1::{IntrospectApiKeyResponse, IntrospectResponse, Membership, RoleGrantRef};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tower::ServiceExt; // for `oneshot`

    const CALLER_KEY: &str = "sk-caller-secret";
    const CALLER_SA: &str = "prn:paigasus:iam:default:sa/gw-caller";
    const CALLER_SCOPE: &str = "prn:paigasus:iam:default:scope/team-a";
    const CALLER_KEY_ID: &str = "key-abc123";

    /// What the introspect call should return for a test case.
    // `ErrorDetails` is a large, all-`Option` richer-error struct; boxing it would push a `.clone()`
    // and an `as_ref()`/deref step onto every match site below for a fixture that is never
    // allocated at scale — test-only, so the size lint is silenced rather than worked around.
    #[allow(clippy::large_enum_variant)]
    enum IntrospectOutcome {
        Ok(IntrospectApiKeyResponse),
        /// An IAM gRPC error Status with this code, optionally carrying richer-error details.
        /// SMA-504: `require_authenticated` now branches on `ErrorInfo`, so a test that wants the
        /// unprovisioned-identity path must supply the reason IAM really sends.
        Rpc(Code, Option<tonic_types::ErrorDetails>),
        /// A channel/connect-time failure.
        Connect,
    }

    /// What the OIDC-token introspect (`Iam::introspect_token`) call should return for a test
    /// case — a separate outcome type from [`IntrospectOutcome`] because `IntrospectResponse`
    /// (issuer/subject) is a different message from `IntrospectApiKeyResponse`
    /// (scope_prn/key_id). Exercised only by [`require_authenticated`]'s tests.
    #[allow(clippy::large_enum_variant)] // see IntrospectOutcome's comment
    enum TokenIntrospectOutcome {
        Ok(IntrospectResponse),
        /// An IAM gRPC error Status with this code, optionally carrying richer-error details.
        /// SMA-504: `require_authenticated` now branches on `ErrorInfo`, so a test that wants the
        /// unprovisioned-identity path must supply the reason IAM really sends.
        Rpc(Code, Option<tonic_types::ErrorDetails>),
        /// A channel/connect-time failure.
        Connect,
    }

    /// What the self-query authz call should return for a test case.
    #[allow(clippy::large_enum_variant)] // see IntrospectOutcome's comment
    enum AuthzOutcome {
        Ok(bool),
        /// An IAM gRPC error Status with this code, optionally carrying richer-error details.
        /// SMA-504: `require_authenticated` now branches on `ErrorInfo`, so a test that wants the
        /// unprovisioned-identity path must supply the reason IAM really sends.
        Rpc(Code, Option<tonic_types::ErrorDetails>),
        Connect,
        /// The middleware under test must never reach `is_authorized_self` at all — a hit is a
        /// test bug (or a real regression), not a scenario to model, so it panics loudly. Used by
        /// every `require_authenticated` test: that middleware performs NO authorization.
        Unreachable,
    }

    /// The args recorded from a call to `is_authorized_self` — the self-query proof.
    #[derive(Debug, Clone)]
    struct RecordedAuthz {
        caller: CallerCredential,
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
        /// SMA-700 D15: how often the API-key leg ran.
        api_key_calls: Arc<AtomicUsize>,
        /// SMA-700: the `dpop` argument of the last `introspect_token` call.
        token_dpop: Arc<Mutex<Option<Option<DpopContext>>>>,
    }

    impl FakeIam {
        fn new(introspect: IntrospectOutcome, authz: AuthzOutcome) -> Self {
            Self {
                introspect,
                token_introspect: None,
                authz,
                recorded: Arc::new(Mutex::new(None)),
                api_key_calls: Arc::new(AtomicUsize::new(0)),
                token_dpop: Arc::new(Mutex::new(None)),
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
            self.api_key_calls.fetch_add(1, Ordering::SeqCst);
            match &self.introspect {
                IntrospectOutcome::Ok(resp) => Ok(resp.clone()),
                IntrospectOutcome::Rpc(code, details) => Err(IamError::Rpc(match details {
                    Some(d) => tonic::Status::with_error_details(*code, "", d.clone()),
                    None => tonic::Status::new(*code, ""),
                })),
                IntrospectOutcome::Connect => Err(IamError::Connect("test connect failure".to_owned())),
            }
        }

        async fn is_authorized_self(&self, caller: &CallerCredential, principal_prn: &str, action: &str, resource_prn: &str) -> Result<bool, IamError> {
            *self.recorded.lock().unwrap() = Some(RecordedAuthz {
                caller: caller.clone(),
                principal_prn: principal_prn.to_owned(),
                action: action.to_owned(),
                resource_prn: resource_prn.to_owned(),
            });
            match &self.authz {
                AuthzOutcome::Ok(allowed) => Ok(*allowed),
                AuthzOutcome::Rpc(code, details) => Err(IamError::Rpc(match details {
                    Some(d) => tonic::Status::with_error_details(*code, "", d.clone()),
                    None => tonic::Status::new(*code, ""),
                })),
                AuthzOutcome::Connect => Err(IamError::Connect("test connect failure".to_owned())),
                AuthzOutcome::Unreachable => panic!("is_authorized_self must not be called by require_authenticated — it performs no authorization"),
            }
        }

        async fn introspect_token(&self, _token: &str, dpop: Option<DpopContext>) -> Result<IntrospectResponse, IamError> {
            *self.token_dpop.lock().unwrap() = Some(dpop);
            match self
                .token_introspect
                .as_ref()
                .expect("test did not configure a token-introspect outcome via FakeIam::with_token_introspect")
            {
                TokenIntrospectOutcome::Ok(resp) => Ok(resp.clone()),
                TokenIntrospectOutcome::Rpc(code, details) => Err(IamError::Rpc(match details {
                    Some(d) => tonic::Status::with_error_details(*code, "", d.clone()),
                    None => tonic::Status::new(*code, ""),
                })),
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
    /// by echoing its three parts. An API key echoes its `key_id`; an OIDC token echoes `oidc`.
    async fn probe(axum::Extension(ctx): axum::Extension<CallerContext>) -> String {
        let credential = match &ctx.credential {
            Credential::ApiKey { key_id } => key_id.clone(),
            Credential::Oidc => "oidc".to_owned(),
        };
        format!("{}|{}|{}", ctx.principal_prn, ctx.scope_prn, credential)
    }

    fn build_app(fake: FakeIam) -> Router {
        build_app_with(fake, false)
    }

    fn build_app_with(fake: FakeIam, dpop_enabled: bool) -> Router {
        let state = AuthState { iam: Arc::new(fake), dpop_enabled };
        Router::new().route("/x", get(probe)).layer(from_fn_with_state(state, require_iam_auth))
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
        build_discovery_app_with(fake, false)
    }

    fn build_discovery_app_with(fake: FakeIam, dpop_enabled: bool) -> Router {
        let state = AuthState { iam: Arc::new(fake), dpop_enabled };
        Router::new().route("/x", get(discovery_probe)).layer(from_fn_with_state(state, require_authenticated))
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

    /// The `ErrorInfo` details IAM attaches for a given canonical reason, on the IAM domain —
    /// what `authn_status` (Task 4) actually puts on the wire.
    fn reason_details(reason: paigasus_proto::paigasus::common::v1::ErrorReason) -> tonic_types::ErrorDetails {
        tonic_types::ErrorDetails::with_error_info(
            reason.as_wire_reason().expect("a declared reason"),
            &*paigasus_proto::error::IAM_DOMAIN,
            std::collections::HashMap::new(),
        )
    }

    /// The details IAM attaches to a validated-but-unprovisioned identity — the ONE
    /// `PermissionDenied` reason [`is_identity_not_provisioned`] accepts.
    fn identity_not_provisioned_details() -> tonic_types::ErrorDetails {
        reason_details(paigasus_proto::paigasus::common::v1::ErrorReason::IdentityNotProvisioned)
    }

    // ---- SMA-635: the OIDC leg of `require_iam_auth` ------------------------------------------

    const USER_TOKEN: &str = "user-oidc-access-token";
    const USER_PRN: &str = "prn:pgs:iam:::principal/0190a1e5-0000-7000-8000-0000000000e0";
    const ORG_A: &str = "0190a100-0000-7000-8000-0000000000a1";
    const ORG_A_PRN: &str = "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000a1";
    const ORG_B: &str = "0190a100-0000-7000-8000-0000000000b2";
    const ORG_B_PRN: &str = "prn:pgs:iam:::organization/0190a100-0000-7000-8000-0000000000b2";
    const TEAM_IN_A_PRN: &str = "prn:pgs:iam::0190a100-0000-7000-8000-0000000000a1:team/0190a1b2-0000-7000-8000-0000000000c3";

    /// What real IAM sends for a bearer that is not a JWT (`paigasus-iam` `convert.rs:141`):
    /// `Unauthenticated` with the reason `invalid-token`. Every key-leg failure row now reaches the
    /// token leg, and this is the answer that leg gets for an API key or garbage.
    fn rejected_token() -> TokenIntrospectOutcome {
        TokenIntrospectOutcome::Rpc(Code::Unauthenticated, Some(reason_details(paigasus_proto::paigasus::common::v1::ErrorReason::InvalidToken)))
    }

    /// The key-leg answer for an OIDC access token: it is not an API key.
    fn rejected_key() -> IntrospectOutcome {
        IntrospectOutcome::Rpc(Code::Unauthenticated, Some(reason_details(paigasus_proto::paigasus::common::v1::ErrorReason::InvalidToken)))
    }

    /// An active user whose memberships and `gateway_user` grants sit on the given PRNs.
    fn user_response(memberships: &[&str], grants: &[&str]) -> IntrospectResponse {
        IntrospectResponse {
            principal_prn: USER_PRN.to_owned(),
            status: "active".to_owned(),
            issuer: "https://issuer.example.com".to_owned(),
            subject: "user-1".to_owned(),
            expires_at: None,
            memberships: memberships
                .iter()
                .map(|prn| Membership {
                    principal_prn: USER_PRN.to_owned(),
                    node_prn: (*prn).to_owned(),
                    ..Default::default()
                })
                .collect(),
            role_grants: grants
                .iter()
                .map(|prn| RoleGrantRef {
                    scope_prn: (*prn).to_owned(),
                    role_key: "gateway_user".to_owned(),
                })
                .collect(),
        }
    }

    fn user_fake(user: IntrospectResponse, authz: AuthzOutcome) -> FakeIam {
        FakeIam::new(rejected_key(), authz).with_token_introspect(TokenIntrospectOutcome::Ok(user))
    }

    /// A request with `Bearer <token>` and one `paigasus-org` header per entry of `orgs` (raw
    /// bytes, so a test can send an obs-text byte).
    fn req_with_orgs(token: &str, orgs: &[&[u8]]) -> HttpRequest<Body> {
        let mut builder = HttpRequest::builder().uri("/x").header(header::AUTHORIZATION, format!("Bearer {token}"));
        for org in orgs {
            builder = builder.header(ORG_HEADER, HeaderValue::from_bytes(org).expect("a valid header value"));
        }
        builder.body(Body::empty()).unwrap()
    }

    async fn run(fake: FakeIam, req: HttpRequest<Body>) -> (StatusCode, String) {
        let resp = build_app(fake).oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    // ---- `iam_result` bounded-label mapping --------------------------------------------------

    #[test]
    fn iam_result_maps_errors_to_bounded_labels() {
        let cases: &[(IamError, &str)] = &[
            (IamError::Connect("boom".to_owned()), "unavailable"),
            (IamError::Rpc(tonic::Status::new(Code::Unavailable, "")), "unavailable"),
            (IamError::Rpc(tonic::Status::new(Code::DeadlineExceeded, "")), "unavailable"),
            (IamError::Rpc(tonic::Status::new(Code::Unauthenticated, "")), "denied"),
            (
                IamError::Rpc(tonic::Status::with_error_details(Code::ResourceExhausted, "", quota_details(std::time::Duration::from_secs(1)))),
                "denied",
            ),
            (IamError::Rpc(tonic::Status::new(Code::ResourceExhausted, "")), "error"),
            (IamError::Rpc(tonic::Status::new(Code::PermissionDenied, "")), "error"),
            (IamError::Rpc(tonic::Status::new(Code::Internal, "")), "error"),
            (IamError::Rpc(tonic::Status::new(Code::NotFound, "")), "error"),
        ];
        for (err, want) in cases {
            assert_eq!(iam_result(err), *want, "iam_result({err:?}) should map to {want:?}");
        }
    }

    // ---- SMA-700: the DPoP flow ------------------------------------------------------------------

    const PROOF: &str = "proof.header.sig";
    const CHALLENGE_PROOF: &str = "DPoP error=\"invalid_dpop_proof\", algs=\"ES256 RS256\"";
    const CHALLENGE_TOKEN: &str = "DPoP error=\"invalid_token\", algs=\"ES256 RS256\"";
    const CHALLENGE_BARE: &str = "DPoP algs=\"ES256 RS256\"";

    fn dpop_req(uri: &str, proofs: &[&str]) -> HttpRequest<Body> {
        let mut builder = HttpRequest::builder().uri(uri).header(header::AUTHORIZATION, format!("DPoP {USER_TOKEN}"));
        for proof in proofs {
            builder = builder.header(DPOP_HEADER, *proof);
        }
        builder.body(Body::empty()).unwrap()
    }

    fn challenges(resp: &Response) -> Vec<String> {
        resp.headers().get_all(header::WWW_AUTHENTICATE).iter().map(|v| v.to_str().unwrap().to_owned()).collect()
    }

    /// A fake whose IAM must never be reached: the key leg answers a connect failure only if it
    /// runs, and `introspect_token` is unconfigured (it panics).
    fn untouched_fake() -> FakeIam {
        FakeIam::new(IntrospectOutcome::Connect, AuthzOutcome::Unreachable)
    }

    #[tokio::test]
    async fn a_dpop_request_forwards_its_context_skips_the_key_leg_and_self_queries_with_dpop() {
        let fake = user_fake(user_response(&[ORG_A_PRN], &[]), AuthzOutcome::Ok(true));
        let (api_key_calls, token_dpop, recorded) = (fake.api_key_calls.clone(), fake.token_dpop.clone(), fake.recorded.clone());
        let resp = build_app_with(fake, true).oneshot(dpop_req("/x", &[PROOF])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(api_key_calls.load(Ordering::SeqCst), 0, "D15: the DPoP scheme skips the API-key leg");
        assert_eq!(
            token_dpop.lock().unwrap().clone().expect("introspect_token ran"),
            Some(DpopContext {
                proof: PROOF.into(),
                method: "GET".into(),
                path: "/x".into()
            })
        );
        let rec = recorded.lock().unwrap().take().expect("is_authorized_self ran");
        assert_eq!(
            rec.caller,
            CallerCredential::Dpop {
                token: USER_TOKEN.into(),
                proof: PROOF.into()
            }
        );
        assert_eq!((rec.principal_prn.as_str(), rec.resource_prn.as_str()), (USER_PRN, ORG_A_PRN));
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(String::from_utf8(body.to_vec()).unwrap(), format!("{USER_PRN}|{ORG_A_PRN}|oidc"));
    }

    #[tokio::test]
    async fn a_query_string_is_not_forwarded_in_the_path() {
        // Review Focus 3: IAM refuses a path with `?`, so the gateway must send the path alone.
        let fake = user_fake(user_response(&[ORG_A_PRN], &[]), AuthzOutcome::Ok(true));
        let token_dpop = fake.token_dpop.clone();
        let resp = build_app_with(fake, true).oneshot(dpop_req("/x?trace=1", &[PROOF])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(token_dpop.lock().unwrap().clone().flatten().expect("a context").path, "/x");
    }

    #[tokio::test]
    async fn a_missing_or_doubled_proof_is_401_with_no_iam_call() {
        for proofs in [&[][..], &[PROOF, PROOF][..]] {
            let fake = untouched_fake();
            let api_key_calls = fake.api_key_calls.clone();
            let resp = build_app_with(fake, true).oneshot(dpop_req("/x", proofs)).await.unwrap();
            assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "{proofs:?}");
            assert_eq!(challenges(&resp), vec![CHALLENGE_PROOF.to_owned()]);
            assert_eq!(api_key_calls.load(Ordering::SeqCst), 0);
            let want = GatewayError::InvalidDpopProof.into_response();
            let want = body_json(want).await["error"]["code"].clone();
            assert_eq!(body_json(resp).await["error"]["code"], want);
        }
    }

    #[tokio::test]
    async fn iam_invalid_dpop_proof_is_401_with_the_proof_challenge() {
        let fake = FakeIam::new(rejected_key(), AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Rpc(
            Code::Unauthenticated,
            Some(reason_details(paigasus_proto::paigasus::common::v1::ErrorReason::InvalidDpopProof)),
        ));
        let resp = build_app_with(fake, true).oneshot(dpop_req("/x", &[PROOF])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(challenges(&resp), vec![CHALLENGE_PROOF.to_owned()]);
        let want = body_json(GatewayError::InvalidDpopProof.into_response()).await;
        assert_eq!(body_json(resp).await, want);
    }

    #[tokio::test]
    async fn another_401_on_the_dpop_scheme_gets_the_invalid_token_challenge() {
        let fake = FakeIam::new(rejected_key(), AuthzOutcome::Unreachable).with_token_introspect(rejected_token());
        let resp = build_app_with(fake, true).oneshot(dpop_req("/x", &[PROOF])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(challenges(&resp), vec![CHALLENGE_TOKEN.to_owned()]);
        let want = body_json(GatewayError::InvalidCredential.into_response()).await;
        assert_eq!(body_json(resp).await, want);
    }

    #[tokio::test]
    async fn a_bearer_or_absent_credential_gets_the_bare_challenge_when_dpop_is_on() {
        // RFC 6750 § 3: the error attribute belongs to the scheme the client used.
        let resp = build_app_with(untouched_fake(), true).oneshot(req_no_auth()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(challenges(&resp), vec![CHALLENGE_BARE.to_owned()]);
        let fake = FakeIam::new(rejected_key(), AuthzOutcome::Unreachable).with_token_introspect(rejected_token());
        let resp = build_app_with(fake, true).oneshot(req_with_auth("Bearer some-token")).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(challenges(&resp), vec![CHALLENGE_BARE.to_owned()]);
        // A 403 gets no challenge.
        let resp = build_app_with(user_fake(user_response(&[ORG_A_PRN], &[]), AuthzOutcome::Ok(false)), true)
            .oneshot(dpop_req("/x", &[PROOF]))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::FORBIDDEN);
        assert!(challenges(&resp).is_empty());
    }

    #[tokio::test]
    async fn dpop_off_answers_as_today_with_no_challenge() {
        // D11: the DPoP scheme is ignored (a missing bearer), no IAM call, no WWW-Authenticate.
        let fake = untouched_fake();
        let api_key_calls = fake.api_key_calls.clone();
        let resp = build_app_with(fake, false).oneshot(dpop_req("/x", &[PROOF])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(challenges(&resp).is_empty());
        assert_eq!(api_key_calls.load(Ordering::SeqCst), 0);
        let want = body_json(GatewayError::MissingBearer.into_response()).await;
        assert_eq!(body_json(resp).await, want);
        let fake = FakeIam::new(rejected_key(), AuthzOutcome::Unreachable).with_token_introspect(rejected_token());
        let resp = build_app_with(fake, false).oneshot(req_with_auth("Bearer some-token")).await.unwrap();
        assert!(challenges(&resp).is_empty(), "no challenge for Bearer either while off");
    }

    /// The headers of a response, minus `content-length` (the router adds it to a served response).
    fn headers_sans_length(resp: &Response) -> HeaderMap {
        let mut headers = resp.headers().clone();
        headers.remove(header::CONTENT_LENGTH);
        headers
    }

    /// With DPoP off, the full response (status, every header, body) equals the bare
    /// `GatewayError` response: the middleware adds nothing.
    #[tokio::test]
    async fn dpop_off_responses_equal_the_bare_error_response() {
        let cases: [(HttpRequest<Body>, GatewayError); 2] = [(req_no_auth(), GatewayError::MissingBearer), (dpop_req("/x", &[PROOF]), GatewayError::MissingBearer)];
        for (req, err) in cases {
            let resp = build_app_with(untouched_fake(), false).oneshot(req).await.unwrap();
            let want = err.into_response();
            assert_eq!(resp.status(), want.status());
            assert_eq!(headers_sans_length(&resp), headers_sans_length(&want));
            assert_eq!(body_json(resp).await, body_json(want).await);
        }
        let fake = FakeIam::new(rejected_key(), AuthzOutcome::Unreachable).with_token_introspect(rejected_token());
        let resp = build_app_with(fake, false).oneshot(req_with_auth("Bearer some-token")).await.unwrap();
        let want = GatewayError::InvalidCredential.into_response();
        assert_eq!(resp.status(), want.status());
        assert_eq!(headers_sans_length(&resp), headers_sans_length(&want));
        assert_eq!(body_json(resp).await, body_json(want).await);
    }

    fn quota_details(retry: std::time::Duration) -> tonic_types::ErrorDetails {
        let mut details = reason_details(paigasus_proto::paigasus::common::v1::ErrorReason::DpopQuotaExceeded);
        details.set_retry_info(Some(retry));
        details
    }

    #[tokio::test]
    async fn an_iam_dpop_quota_refusal_is_429_with_retry_after_labelled_denied_and_no_warning() {
        let handle = paigasus_observability::init("test-gateway-dpop-quota");
        let (logs, _guard) = capture_logs();
        let fake =
            FakeIam::new(rejected_key(), AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Rpc(Code::ResourceExhausted, Some(quota_details(std::time::Duration::from_secs(7)))));
        let resp = build_app_with(fake, true).oneshot(dpop_req("/x", &[PROOF])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(resp.headers()["retry-after"], "7");
        assert!(challenges(&resp).is_empty(), "a 429 gets no challenge");
        let want = body_json(GatewayError::RateLimited { retry_after_secs: 7 }.into_response()).await;
        assert_eq!(body_json(resp).await, want);
        let out = handle.render();
        assert!(
            out.lines()
                .any(|l| l.starts_with("gateway_iam_calls_total") && l.contains(r#"operation="introspect_token""#) && l.contains(r#"result="denied""#)),
            "D10: a quota hit is a verdict, not an outage:\n{out}"
        );
        assert!(!logs.text().contains("PermissionDenied with no ErrorInfo"), "{}", logs.text());
    }

    #[tokio::test]
    async fn reading_a_reason_on_a_detail_less_401_writes_no_warning() {
        let (logs, _guard) = capture_logs();
        let fake = FakeIam::new(rejected_key(), AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Rpc(Code::Unauthenticated, None));
        let resp = build_app_with(fake, true).oneshot(dpop_req("/x", &[PROOF])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert!(!logs.text().contains("PermissionDenied with no ErrorInfo"), "{}", logs.text());
    }

    #[tokio::test]
    async fn a_detail_less_resource_exhausted_is_an_iam_failure_and_writes_no_warning() {
        // Not the DPoP quota: no `ErrorInfo` at all. It stays a 503, and reading the reason must
        // not write the SMA-504 skew warning (that one is for `PermissionDenied` only).
        let (logs, _guard) = capture_logs();
        let fake = FakeIam::new(rejected_key(), AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Rpc(Code::ResourceExhausted, None));
        let resp = build_app_with(fake, true).oneshot(dpop_req("/x", &[PROOF])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(!logs.text().contains("PermissionDenied with no ErrorInfo"), "{}", logs.text());
    }

    #[test]
    fn retry_after_is_whole_seconds_and_at_least_one() {
        let status = |retry: Option<std::time::Duration>| {
            let mut details = reason_details(paigasus_proto::paigasus::common::v1::ErrorReason::DpopQuotaExceeded);
            details.set_retry_info(retry);
            tonic::Status::with_error_details(Code::ResourceExhausted, "", details)
        };
        assert_eq!(retry_after_secs(&status(None)), 1, "no RetryInfo");
        assert_eq!(retry_after_secs(&status(Some(std::time::Duration::from_millis(500)))), 1, "a delay under one second");
        assert_eq!(retry_after_secs(&status(Some(std::time::Duration::from_secs(0)))), 1, "a zero delay");
        assert_eq!(retry_after_secs(&status(Some(std::time::Duration::from_secs(61)))), 61);
    }

    #[test]
    fn the_authz_leg_keeps_an_invalid_dpop_proof_apart_from_a_rejected_credential() {
        let proof = tonic::Status::with_error_details(Code::Unauthenticated, "", reason_details(paigasus_proto::paigasus::common::v1::ErrorReason::InvalidDpopProof));
        assert_eq!(authz_error(IamError::Rpc(proof)), GatewayError::InvalidDpopProof);
        let plain = tonic::Status::new(Code::Unauthenticated, "");
        assert_eq!(authz_error(IamError::Rpc(plain)), GatewayError::InvalidCredential);
    }

    #[test]
    fn the_dpop_context_path_is_the_original_uri_not_the_nested_one() {
        // A nested router strips its prefix from `req.uri()`; `OriginalUri` keeps the path that the
        // client signed. No production route is nested today, so this builds the extension by hand.
        let req = Request::builder()
            .uri("/stripped")
            .extension(OriginalUri("/v1/stripped?x=1".parse().unwrap()))
            .body(axum::body::Body::empty())
            .unwrap();
        assert_eq!(dpop_context(&req, PROOF).path, "/v1/stripped");
        let plain = Request::builder().uri("/plain?y=2").body(axum::body::Body::empty()).unwrap();
        assert_eq!(dpop_context(&plain, PROOF).path, "/plain");
    }

    #[tokio::test]
    async fn discovery_on_dpop_forwards_the_context_and_never_authorizes() {
        let fake = FakeIam::new(IntrospectOutcome::Connect, AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Ok(active_token_response()));
        let (api_key_calls, token_dpop) = (fake.api_key_calls.clone(), fake.token_dpop.clone());
        let resp = build_discovery_app_with(fake, true).oneshot(dpop_req("/x", &[PROOF])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(api_key_calls.load(Ordering::SeqCst), 0);
        let ctx = token_dpop.lock().unwrap().clone().flatten().expect("a context");
        assert_eq!((ctx.proof.as_str(), ctx.method.as_str(), ctx.path.as_str()), (PROOF, "GET", "/x"));
    }

    #[tokio::test]
    async fn discovery_on_dpop_keeps_its_unprovisioned_rule_and_maps_a_bad_proof() {
        // IAM answers identity-not-provisioned only after the proof check passed (D18).
        let fake =
            FakeIam::new(IntrospectOutcome::Connect, AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Rpc(Code::PermissionDenied, Some(identity_not_provisioned_details())));
        assert_eq!(build_discovery_app_with(fake, true).oneshot(dpop_req("/x", &[PROOF])).await.unwrap().status(), StatusCode::OK);
        let fake = FakeIam::new(IntrospectOutcome::Connect, AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Rpc(
            Code::Unauthenticated,
            Some(reason_details(paigasus_proto::paigasus::common::v1::ErrorReason::InvalidDpopProof)),
        ));
        let resp = build_discovery_app_with(fake, true).oneshot(dpop_req("/x", &[PROOF])).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(challenges(&resp), vec![CHALLENGE_PROOF.to_owned()]);
    }

    #[test]
    fn the_dpop_challenge_table() {
        use SchemeUsed::{Bearer, Dpop, None as NoScheme};
        let cases = [
            (true, Dpop, GatewayError::InvalidDpopProof, Some(CHALLENGE_PROOF)),
            (true, Dpop, GatewayError::InvalidCredential, Some(CHALLENGE_TOKEN)),
            (true, Dpop, GatewayError::MissingBearer, Some(CHALLENGE_TOKEN)),
            (true, Bearer, GatewayError::InvalidCredential, Some(CHALLENGE_BARE)),
            (true, NoScheme, GatewayError::MissingBearer, Some(CHALLENGE_BARE)),
            (true, Dpop, GatewayError::AuthzDenied, None),
            (true, Dpop, GatewayError::RateLimited { retry_after_secs: 3 }, None),
            (true, Dpop, GatewayError::IamUnavailable, None),
            (false, Dpop, GatewayError::InvalidDpopProof, None),
            (false, Bearer, GatewayError::InvalidCredential, None),
        ];
        for (on, scheme, err, want) in cases {
            assert_eq!(
                dpop_challenge(on, scheme, &err).map(|v| v.to_str().unwrap().to_owned()),
                want.map(str::to_owned),
                "{on} {scheme:?} {err:?}"
            );
        }
    }

    // ---- SMA-700: the credentials parser -----------------------------------------------------

    fn headers_of(authorization: Option<&str>, proofs: &[&[u8]]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(value) = authorization {
            headers.insert(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        }
        for proof in proofs {
            headers.append(DPOP_HEADER, HeaderValue::from_bytes(proof).unwrap());
        }
        headers
    }

    #[test]
    fn the_parser_reads_both_schemes_in_any_case() {
        assert_eq!(credentials(&headers_of(Some("Bearer t"), &[]), true), Some(Credentials::Bearer("t".into())));
        assert_eq!(credentials(&headers_of(Some("bearer t"), &[]), false), Some(Credentials::Bearer("t".into())));
        for scheme in ["DPoP", "dpop", "DPOP"] {
            assert_eq!(
                credentials(&headers_of(Some(&format!("{scheme} t")), &[b"a.b.c"]), true),
                Some(Credentials::Dpop {
                    token: "t".into(),
                    proof: ProofHeader::One("a.b.c".into())
                }),
                "{scheme}"
            );
        }
    }

    #[test]
    fn the_proof_header_is_one_visible_ascii_value() {
        let dpop = |proofs: &[&[u8]]| credentials(&headers_of(Some("DPoP t"), proofs), true);
        assert_eq!(
            dpop(&[]),
            Some(Credentials::Dpop {
                token: "t".into(),
                proof: ProofHeader::Missing
            })
        );
        assert_eq!(
            dpop(&[b"a.b.c", b"d.e.f"]),
            Some(Credentials::Dpop {
                token: "t".into(),
                proof: ProofHeader::Invalid
            }),
            "two headers"
        );
        assert_eq!(
            dpop(&[b"a.\xffb.c"]),
            Some(Credentials::Dpop {
                token: "t".into(),
                proof: ProofHeader::Invalid
            }),
            "not visible ASCII"
        );
        assert_eq!(
            dpop(&[b""]),
            Some(Credentials::Dpop {
                token: "t".into(),
                proof: ProofHeader::Invalid
            }),
            "empty"
        );
    }

    #[test]
    fn the_bearer_scheme_ignores_a_dpop_header_and_dpop_off_ignores_the_scheme() {
        assert_eq!(credentials(&headers_of(Some("Bearer t"), &[b"a.b.c"]), true), Some(Credentials::Bearer("t".into())));
        assert_eq!(credentials(&headers_of(Some("DPoP t"), &[b"a.b.c"]), false), None, "D11: DPoP off is as today");
        assert_eq!(credentials(&headers_of(Some("DPoP "), &[b"a.b.c"]), true), None, "an empty token");
        assert_eq!(credentials(&headers_of(Some("Basic t"), &[]), true), None);
        assert_eq!(credentials(&headers_of(None, &[b"a.b.c"]), true), None);
    }

    #[test]
    fn credentials_debug_prints_no_secret() {
        let printed = format!(
            "{:?} {:?}",
            Credentials::Bearer("tok-secret".into()),
            Credentials::Dpop {
                token: "tok-secret".into(),
                proof: ProofHeader::One("proof-secret".into())
            }
        );
        assert!(!printed.contains("secret"), "{printed}");
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
        let fake = FakeIam::new(IntrospectOutcome::Rpc(Code::Unauthenticated, None), AuthzOutcome::Ok(true)).with_token_introspect(rejected_token());
        assert_eq!(status_of(fake, req_with_auth("Bearer bad-key")).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn introspect_permission_denied_returns_401() {
        // Inactive principal on the API-key path is a client-auth failure (401), NOT a 403.
        let fake = FakeIam::new(IntrospectOutcome::Rpc(Code::PermissionDenied, None), AuthzOutcome::Ok(true)).with_token_introspect(rejected_token());
        assert_eq!(status_of(fake, req_with_auth("Bearer inactive-key")).await, StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn introspect_unavailable_returns_503() {
        let fake = FakeIam::new(IntrospectOutcome::Rpc(Code::Unavailable, None), AuthzOutcome::Ok(true)).with_token_introspect(rejected_token());
        assert_eq!(status_of(fake, req_with_auth("Bearer any-key")).await, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn introspect_connect_failure_returns_503() {
        let fake = FakeIam::new(IntrospectOutcome::Connect, AuthzOutcome::Ok(true)).with_token_introspect(rejected_token());
        assert_eq!(status_of(fake, req_with_auth("Bearer any-key")).await, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn introspect_success_with_non_active_status_returns_401() {
        let resp = IntrospectApiKeyResponse {
            status: "disabled".to_owned(),
            ..active_response()
        };
        let fake = FakeIam::new(IntrospectOutcome::Ok(resp), AuthzOutcome::Ok(true)).with_token_introspect(rejected_token());
        assert_eq!(status_of(fake, req_with_auth("Bearer disabled-key")).await, StatusCode::UNAUTHORIZED);
    }

    /// SMA-635 review: a key-leg `Ok` answer with a non-`active` status (a disabled key) is a
    /// VERDICT, not an outage — it must record `introspect` with result `denied`, not `ok`. Read
    /// via the rendered Prometheus exposition (not the `iam_result` mapping function, which never
    /// sees this arm) so the assertion fails if the `Ok(_) => record_iam_call("introspect", "denied", ..)`
    /// arm were changed back to `"ok"`.
    #[tokio::test]
    async fn a_disabled_api_key_records_introspect_denied() {
        let handle = paigasus_observability::init("test-gateway-auth-disabled-key-metric");
        let resp = IntrospectApiKeyResponse {
            status: "disabled".to_owned(),
            ..active_response()
        };
        let fake = FakeIam::new(IntrospectOutcome::Ok(resp), AuthzOutcome::Unreachable).with_token_introspect(rejected_token());
        let _ = status_of(fake, req_with_auth("Bearer disabled-key")).await;
        let out = handle.render();
        assert!(
            out.lines()
                .any(|l| l.starts_with("gateway_iam_calls_total") && l.contains(r#"operation="introspect""#) && l.contains(r#"result="denied""#)),
            "expected a denied-result introspect call recorded:\n{out}"
        );
        assert!(
            !out.lines()
                .any(|l| l.starts_with("gateway_iam_calls_total") && l.contains(r#"operation="introspect""#) && l.contains(r#"result="ok""#)),
            "a disabled key must never record introspect as ok:\n{out}"
        );
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
        let fake = FakeIam::new(IntrospectOutcome::Ok(active_response()), AuthzOutcome::Rpc(Code::PermissionDenied, None));
        assert_eq!(status_of(fake, req_with_auth("Bearer sk-caller-secret")).await, StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[tokio::test]
    async fn authz_unavailable_returns_503() {
        let fake = FakeIam::new(IntrospectOutcome::Ok(active_response()), AuthzOutcome::Rpc(Code::Unavailable, None));
        assert_eq!(status_of(fake, req_with_auth("Bearer sk-caller-secret")).await, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn authz_connect_failure_returns_503() {
        let fake = FakeIam::new(IntrospectOutcome::Ok(active_response()), AuthzOutcome::Connect);
        assert_eq!(status_of(fake, req_with_auth("Bearer sk-caller-secret")).await, StatusCode::SERVICE_UNAVAILABLE);
    }

    #[tokio::test]
    async fn authz_unauthenticated_returns_401() {
        let fake = FakeIam::new(IntrospectOutcome::Ok(active_response()), AuthzOutcome::Rpc(Code::Unauthenticated, None));
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

        let app = build_app(fake);
        let resp = app.oneshot(req_with_auth(&format!("Bearer {CALLER_KEY}"))).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        let rec = recorded.lock().unwrap().take().expect("is_authorized_self must have been called on the happy path");
        // A self-query, never cross-principal: the caller's OWN bearer + the introspected SA.
        assert_eq!(rec.caller, CallerCredential::ApiKey(CALLER_KEY.to_owned()), "authz must present the caller's OWN key");
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
        let fake = FakeIam::new(IntrospectOutcome::Rpc(Code::Unauthenticated, None), AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Ok(active_token_response()));
        assert_eq!(discovery_status_of(fake, req_with_auth(&format!("Bearer {CONSOLE_TOKEN}"))).await, StatusCode::OK);
    }

    /// D5's deliberate relaxation, and the one most likely to be "fixed" back into a 401 by a
    /// later reader. IAM returns `PermissionDenied` for a VALIDATED token whose identity has no
    /// local principal; on the discovery path that still counts as authenticated, because the
    /// descriptor is byte-identical for every caller and carries no per-principal data. SMA-504:
    /// the accept is now narrow (`ErrorInfo` reason `identity-not-provisioned`), so this fake must
    /// supply exactly that detail to keep passing — this test drives `require_authenticated` all
    /// the way to `next.run(req).await`, i.e. the handler is still reached.
    #[tokio::test]
    async fn require_authenticated_accepts_a_validated_but_unprovisioned_identity() {
        let fake = FakeIam::new(IntrospectOutcome::Rpc(Code::Unauthenticated, None), AuthzOutcome::Unreachable)
            .with_token_introspect(TokenIntrospectOutcome::Rpc(Code::PermissionDenied, Some(identity_not_provisioned_details())));
        assert_eq!(discovery_status_of(fake, req_with_auth("Bearer validated-but-unprovisioned-token")).await, StatusCode::OK);
    }

    /// SMA-504 AC 2: the narrow accepts ONLY `identity-not-provisioned`. `principal-inactive` and
    /// `provisioning-failed` share `PermissionDenied` and were previously accepted along with it
    /// — the blanket accept ADR-0020 D4's tripwire comment warned about.
    #[tokio::test]
    async fn require_authenticated_rejects_other_permission_denied_reasons() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;

        for reason in [ErrorReason::PrincipalInactive, ErrorReason::ProvisioningFailed] {
            let fake = FakeIam::new(IntrospectOutcome::Rpc(Code::Unauthenticated, None), AuthzOutcome::Unreachable)
                .with_token_introspect(TokenIntrospectOutcome::Rpc(Code::PermissionDenied, Some(reason_details(reason))));
            assert_eq!(
                discovery_status_of(fake, req_with_auth("Bearer token")).await,
                StatusCode::UNAUTHORIZED,
                "{reason:?} must not ride in on the identity-not-provisioned relaxation"
            );
        }
    }

    /// A detail-less `PermissionDenied` fails closed. Two outcomes, both correct: a 401 when the
    /// API-key leg reached a verdict, and a 503 when it did not — `preserve_outage` still widens
    /// an inconclusive leg, so "fails closed" is not unconditionally a 401.
    #[tokio::test]
    async fn a_detail_less_permission_denied_fails_closed() {
        let conclusive = FakeIam::new(IntrospectOutcome::Rpc(Code::Unauthenticated, None), AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Rpc(Code::PermissionDenied, None));
        assert_eq!(discovery_status_of(conclusive, req_with_auth("Bearer token")).await, StatusCode::UNAUTHORIZED);

        let inconclusive = FakeIam::new(IntrospectOutcome::Connect, AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Rpc(Code::PermissionDenied, None));
        assert_eq!(
            discovery_status_of(inconclusive, req_with_auth("Bearer token")).await,
            StatusCode::SERVICE_UNAVAILABLE,
            "an outage on the API-key leg still wins over a rejection on the OIDC leg"
        );
    }

    /// The predicate checks the DOMAIN, not just the reason string — otherwise any service that
    /// happened to emit the same `identity-not-provisioned` reason on its own `ErrorInfo` could
    /// forge IAM's relaxation.
    #[tokio::test]
    async fn require_authenticated_rejects_the_reason_from_a_foreign_domain() {
        let foreign_domain_details = tonic_types::ErrorDetails::with_error_info(
            paigasus_proto::paigasus::common::v1::ErrorReason::IdentityNotProvisioned.as_wire_reason().expect("a declared reason"),
            "not-iam.paigasus.io",
            std::collections::HashMap::new(),
        );
        let fake = FakeIam::new(IntrospectOutcome::Rpc(Code::Unauthenticated, None), AuthzOutcome::Unreachable)
            .with_token_introspect(TokenIntrospectOutcome::Rpc(Code::PermissionDenied, Some(foreign_domain_details)));
        assert_eq!(
            discovery_status_of(fake, req_with_auth("Bearer token")).await,
            StatusCode::UNAUTHORIZED,
            "the right reason on the WRONG domain must not be accepted"
        );
    }

    /// The discovery relaxation must NOT leak onto the chat path. Real IAM sends
    /// `identity-not-provisioned` on the TOKEN leg (`convert.rs:142`), so that is where this fake
    /// puts it. A user with no provisioned identity has no grants (SMA-635 spec §4.1 step 3).
    /// Two forms: a conclusive key leg gives 401; an inconclusive key leg gives 503, because
    /// `preserve_outage` keeps an IAM outage visible.
    #[tokio::test]
    async fn require_iam_auth_still_rejects_an_unprovisioned_identity() {
        let conclusive = FakeIam::new(rejected_key(), AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Rpc(Code::PermissionDenied, Some(identity_not_provisioned_details())));
        assert_eq!(status_of(conclusive, req_with_auth("Bearer validated-but-unprovisioned-token")).await, StatusCode::UNAUTHORIZED);

        let inconclusive =
            FakeIam::new(IntrospectOutcome::Connect, AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Rpc(Code::PermissionDenied, Some(identity_not_provisioned_details())));
        assert_eq!(
            status_of(inconclusive, req_with_auth("Bearer validated-but-unprovisioned-token")).await,
            StatusCode::SERVICE_UNAVAILABLE
        );
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
        let fake = FakeIam::new(IntrospectOutcome::Rpc(Code::Unauthenticated, None), AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Rpc(Code::Unauthenticated, None));
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
            IntrospectOutcome::Rpc(Code::Unavailable, None),
            IntrospectOutcome::Rpc(Code::DeadlineExceeded, None),
            IntrospectOutcome::Rpc(Code::Internal, None),
        ] {
            let fake = FakeIam::new(outcome, AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Rpc(Code::Unauthenticated, None));
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
            IntrospectOutcome::Rpc(Code::Unauthenticated, None),
            IntrospectOutcome::Rpc(Code::PermissionDenied, None),
            IntrospectOutcome::Ok(inactive_response()),
        ] {
            let fake = FakeIam::new(outcome, AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Rpc(Code::Unauthenticated, None));
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
        let fake = FakeIam::new(IntrospectOutcome::Rpc(Code::Unauthenticated, None), AuthzOutcome::Unreachable).with_token_introspect(TokenIntrospectOutcome::Ok(non_active));
        assert_eq!(discovery_status_of(fake, req_with_auth(&format!("Bearer {CONSOLE_TOKEN}"))).await, StatusCode::UNAUTHORIZED);
    }

    // ---- SMA-635 rows 1-11 ------------------------------------------------------------------

    /// Row 1: an OIDC bearer with a valid header reaches the handler as `Credential::Oidc`, with
    /// the org PRN as the scope.
    #[tokio::test]
    async fn an_oidc_bearer_with_a_valid_org_header_reaches_the_handler() {
        let fake = user_fake(user_response(&[ORG_A_PRN], &[]), AuthzOutcome::Ok(true));
        let (status, body) = run(fake, req_with_orgs(USER_TOKEN, &[ORG_A.as_bytes()])).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, format!("{USER_PRN}|{ORG_A_PRN}|oidc"));
    }

    /// Row 2: no header, one org reached twice (a membership on the org, a `gateway_user` grant
    /// on a team of the same org): the org is inferred.
    #[tokio::test]
    async fn without_a_header_one_org_reached_twice_is_inferred() {
        let fake = user_fake(user_response(&[ORG_A_PRN], &[TEAM_IN_A_PRN]), AuthzOutcome::Ok(true));
        let (status, body) = run(fake, req_with_orgs(USER_TOKEN, &[])).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, format!("{USER_PRN}|{ORG_A_PRN}|oidc"));
    }

    /// Rows 3 and 4: no header, and zero orgs or two orgs: `400 org-required`, and no
    /// authorization call.
    #[tokio::test]
    async fn without_a_header_zero_or_two_orgs_is_org_required() {
        for (label, memberships) in [("zero orgs", vec![]), ("two orgs", vec![ORG_A_PRN, ORG_B_PRN])] {
            let fake = user_fake(user_response(&memberships, &[]), AuthzOutcome::Unreachable);
            let (status, body) = run(fake, req_with_orgs(USER_TOKEN, &[])).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{label}");
            let json: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(json["error"]["code"], "org-required", "{label}");
            assert_eq!(json["error"]["param"], "paigasus-org", "{label}");
        }
    }

    /// Row 5 and Review Focus 1: a header that is not a UUID, a simple-form UUID, two headers, and
    /// a value with an obs-text byte (valid as a `HeaderValue`, but `to_str()` fails) each give
    /// `400 invalid-org-header` BEFORE any authorization call — never a 500 or a panic.
    #[tokio::test]
    async fn an_invalid_org_header_is_400_before_any_authorization() {
        let cases: [(&str, Vec<&[u8]>); 4] = [
            ("not a uuid", vec![b"acme".as_slice()]),
            ("simple form", vec![b"0190a1000000700080000000000000a1".as_slice()]),
            ("two headers", vec![ORG_A.as_bytes(), ORG_A.as_bytes()]),
            ("obs-text byte", vec![b"0190a100-0000-7000-8000-0000000000a\x80".as_slice()]),
        ];
        for (label, orgs) in cases {
            let fake = user_fake(user_response(&[ORG_A_PRN], &[]), AuthzOutcome::Unreachable);
            let (status, body) = run(fake, req_with_orgs(USER_TOKEN, &orgs)).await;
            assert_eq!(status, StatusCode::BAD_REQUEST, "{label}");
            let json: serde_json::Value = serde_json::from_str(&body).unwrap();
            assert_eq!(json["error"]["code"], "invalid-org-header", "{label}");
            assert_eq!(json["error"]["param"], "paigasus-org", "{label}");
        }
    }

    /// Row 6: IAM denies the self-query: `403 insufficient-permissions`.
    #[tokio::test]
    async fn an_oidc_authz_deny_is_403() {
        let fake = user_fake(user_response(&[ORG_A_PRN], &[]), AuthzOutcome::Ok(false));
        let (status, body) = run(fake, req_with_orgs(USER_TOKEN, &[ORG_A.as_bytes()])).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["error"]["code"], "insufficient-permissions");
    }

    /// Row 7 — THE self-query proof for OIDC (D9): the user's OWN token, the INTROSPECTED user PRN,
    /// `InvokeModel`, and the org PRN built from the header. The user reaches BOTH `ORG_A` and
    /// `ORG_B` (inference alone would be ambiguous and give `org-required`), and the header names
    /// `ORG_B` — proving the header WINS over inference, not merely that it is accepted when it
    /// happens to agree with the only inferable org.
    #[tokio::test]
    async fn the_oidc_self_query_uses_the_users_token_principal_and_the_org_prn() {
        let fake = user_fake(user_response(&[ORG_A_PRN, ORG_B_PRN], &[]), AuthzOutcome::Ok(true));
        let recorded = fake.recorded.clone();
        let (status, _) = run(fake, req_with_orgs(USER_TOKEN, &[ORG_B.as_bytes()])).await;
        assert_eq!(status, StatusCode::OK);
        let rec = recorded.lock().unwrap().take().expect("is_authorized_self was called");
        assert_eq!(rec.caller, CallerCredential::Bearer(USER_TOKEN.to_owned()));
        assert_eq!(rec.principal_prn, USER_PRN);
        assert_eq!(rec.action, INVOKE_MODEL_ACTION);
        assert_eq!(
            rec.resource_prn, ORG_B_PRN,
            "the header must win over inference, which alone would be ambiguous between ORG_A and ORG_B"
        );
    }

    /// Review Focus 2: an upper-case header queries the LOWER-case canonical org PRN.
    #[tokio::test]
    async fn an_uppercase_org_header_queries_the_lowercase_org_prn() {
        let fake = user_fake(user_response(&[ORG_A_PRN], &[]), AuthzOutcome::Ok(true));
        let recorded = fake.recorded.clone();
        let upper = ORG_A.to_uppercase();
        let (status, _) = run(fake, req_with_orgs(USER_TOKEN, &[upper.as_bytes()])).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(recorded.lock().unwrap().take().expect("called").resource_prn, ORG_A_PRN);
    }

    /// Row 8: the key leg is inconclusive and the OIDC leg rejects: 503, not 401.
    #[tokio::test]
    async fn an_inconclusive_key_leg_and_a_rejected_token_is_503() {
        let fake = FakeIam::new(IntrospectOutcome::Connect, AuthzOutcome::Unreachable).with_token_introspect(rejected_token());
        assert_eq!(status_of(fake, req_with_orgs(USER_TOKEN, &[ORG_A.as_bytes()])).await, StatusCode::SERVICE_UNAVAILABLE);
    }

    /// Row 9 (D5): an API key with a header for ANOTHER org keeps the key's own scope, never calls
    /// `introspect_token` (the fake panics if it does: no token outcome is configured), and logs
    /// one warning that never holds the header value.
    #[tokio::test]
    async fn an_api_key_ignores_the_org_header_and_warns_once() {
        let (logs, _guard) = capture_logs();
        let fake = happy_fake();
        let recorded = fake.recorded.clone();
        let (status, body) = run(fake, req_with_orgs(CALLER_KEY, &[ORG_B.as_bytes()])).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body, format!("{CALLER_SA}|{CALLER_SCOPE}|{CALLER_KEY_ID}"));
        assert_eq!(recorded.lock().unwrap().take().expect("called").resource_prn, CALLER_SCOPE);
        let text = logs.text();
        assert_eq!(text.matches("paigasus-org ignored for an API key").count(), 1, "{text}");
        assert!(text.contains(CALLER_KEY_ID), "the warning names the key: {text}");
        assert!(!text.contains(ORG_B), "the header value is never logged: {text}");
    }

    /// Row 10 (D4): a team-only `gateway_user` grant with a header for its org: the gateway asks
    /// IAM about the ORG PRN, never the team. The fake denies, so the answer is 403.
    #[tokio::test]
    async fn a_team_only_grant_is_authorized_against_the_org_prn() {
        let fake = user_fake(user_response(&[], &[TEAM_IN_A_PRN]), AuthzOutcome::Ok(false));
        let recorded = fake.recorded.clone();
        let (status, _) = run(fake, req_with_orgs(USER_TOKEN, &[ORG_A.as_bytes()])).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        assert_eq!(recorded.lock().unwrap().take().expect("called").resource_prn, ORG_A_PRN);
    }

    /// Row 11: `AuthEnforce` can reject an OIDC bearer on `IsAuthorized` with `principal-inactive`
    /// or `provisioning-failed` (`authn.rs:217-241`): 401. `forbidden` keeps the 500 "possible
    /// broken self-query" mapping, and so does a detail-less `PermissionDenied`.
    #[tokio::test]
    async fn authz_permission_denied_maps_by_iam_reason() {
        use paigasus_proto::paigasus::common::v1::ErrorReason;

        for (reason, want) in [
            (ErrorReason::PrincipalInactive, StatusCode::UNAUTHORIZED),
            (ErrorReason::ProvisioningFailed, StatusCode::UNAUTHORIZED),
            (ErrorReason::Forbidden, StatusCode::INTERNAL_SERVER_ERROR),
        ] {
            let fake = user_fake(user_response(&[ORG_A_PRN], &[]), AuthzOutcome::Rpc(Code::PermissionDenied, Some(reason_details(reason))));
            assert_eq!(status_of(fake, req_with_orgs(USER_TOKEN, &[ORG_A.as_bytes()])).await, want, "{reason:?}");
        }
        let detail_less = user_fake(user_response(&[ORG_A_PRN], &[]), AuthzOutcome::Rpc(Code::PermissionDenied, None));
        assert_eq!(status_of(detail_less, req_with_orgs(USER_TOKEN, &[ORG_A.as_bytes()])).await, StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// A non-active user introspection is a rejected credential (401), symmetric with the key leg.
    #[tokio::test]
    async fn a_non_active_oidc_user_is_401() {
        let user = IntrospectResponse {
            status: "disabled".to_owned(),
            ..user_response(&[ORG_A_PRN], &[])
        };
        let fake = user_fake(user, AuthzOutcome::Unreachable);
        assert_eq!(status_of(fake, req_with_orgs(USER_TOKEN, &[ORG_A.as_bytes()])).await, StatusCode::UNAUTHORIZED);
    }
}
