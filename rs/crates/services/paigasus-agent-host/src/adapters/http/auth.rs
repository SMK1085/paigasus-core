// SPDX-License-Identifier: Apache-2.0

//! Authentication and authorisation of the event ingress through IAM (spec §6.2, §6.3).
//!
//! The functions here follow `bearer_iam_auth` and `require_authenticated` of the gateway
//! (`paigasus-gateway/src/adapters/http/auth.rs`), without the DPoP scheme and without the
//! `paigasus-org` header. The org comes from the inbox.
//!
//! ## Authentication (§6.2)
//! The credential is `Authorization: Bearer <secret>`. Every other scheme, `DPoP` included, is
//! `401 invalid-token`. The host tries `IntrospectApiKey` first and then `Introspect`. It does not
//! branch on the key prefix: the prefix is an IAM setting that the host cannot see.
//!
//! ## Authorisation (§6.3), in this order
//! 1. **Key scope.** The `scope_prn` of an API key is Root or exactly the org of the inbox.
//! 2. **Owner path.** The caller is the inbox principal. An API key is then allowed. An OIDC user
//!    is allowed only when the inbox org is in the memberships of `Introspect`.
//! 3. **Non-owner path.** The self-query: `IsAuthorized` with the caller's own credential, the
//!    caller's own principal PRN, `WriteInboxEvent`, and the org PRN of the inbox.
//!
//! Every PRN comparison compares UUIDs, never strings. There is no cache.
//!
//! IAM calls per request: API key owner 1, API key non-owner 2, OIDC owner 2, OIDC non-owner 3.

use std::sync::LazyLock;

use axum::http::{HeaderMap, header};
use paigasus_agent_host_core::ids::PrincipalId;
use paigasus_agent_host_core::inbox::Inbox;
use paigasus_kernel::resource_name::Prn;
use paigasus_proto::paigasus::common::v1::ErrorReason;
use paigasus_proto::paigasus::iam::v1::{IntrospectApiKeyResponse, IntrospectResponse};
use tonic::{Code, Status};
use tonic_types::StatusExt;
use uuid::Uuid;

use super::error::ApiError;
use crate::adapters::iam::{CallerCredential, Iam, IamError};

/// The IAM action of a write into an inbox. It is the wire form of `Action::WriteInboxEvent` in
/// `paigasus-iam-core`. The host cannot import that enum across the gRPC boundary.
pub const WRITE_INBOX_EVENT_ACTION: &str = "WriteInboxEvent";

/// The `status` of an active key or token in an IAM introspect answer.
const ACTIVE: &str = "active";
/// The PRN parts of the IAM tenancy nodes.
const IAM_SERVICE: &str = "iam";
const ORGANIZATION: &str = "organization";
const ROOT: &str = "root";

/// What the credential is, with the data that authorisation needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallerKind {
    /// An API key. `scope_prn` is the scope the key was issued under.
    ApiKey { scope_prn: String },
    /// An OIDC user. `member_orgs` are the orgs of the memberships of `Introspect`.
    Oidc { member_orgs: Vec<Uuid> },
}

/// An authenticated caller. `credential` is the caller's own credential, for the self-query.
/// `Debug` prints no secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caller {
    pub principal: PrincipalId,
    pub credential: CallerCredential,
    pub kind: CallerKind,
}

/// Read the `Authorization` header. The parse is the gateway's: split on the first space, an
/// ASCII-case-insensitive `Bearer` scheme, and a trimmed token that is not empty. `DPoP` and every
/// other scheme give [`ApiError::InvalidToken`], with no challenge.
pub fn bearer_token(headers: &HeaderMap) -> Result<String, ApiError> {
    let value = headers.get(header::AUTHORIZATION).and_then(|v| v.to_str().ok()).ok_or(ApiError::InvalidToken)?;
    let (scheme, token) = value.split_once(' ').ok_or(ApiError::InvalidToken)?;
    let token = token.trim();
    if !scheme.eq_ignore_ascii_case("Bearer") || token.is_empty() {
        return Err(ApiError::InvalidToken);
    }
    Ok(token.to_owned())
}

/// Authenticate a write request: the key leg, then the token leg (§6.2).
///
/// An inactive key or a key-leg refusal falls through to the token leg. An unprovisioned OIDC
/// identity is refused here: it holds no grant.
pub async fn authenticate(iam: &dyn Iam, token: &str) -> Result<Caller, ApiError> {
    let key_inconclusive = match iam.introspect_api_key(token).await {
        Ok(resp) if resp.status == ACTIVE => return api_key_caller(token, resp),
        // IAM answered "not active": a verdict, not an outage.
        Ok(_) => false,
        Err(err) => introspect_error(err) == ApiError::IamUnavailable,
    };
    match iam.introspect_token(token).await {
        Ok(resp) if resp.status == ACTIVE => oidc_caller(token, resp),
        Ok(_) => Err(preserve_outage(key_inconclusive, ApiError::InvalidToken)),
        Err(err) => Err(preserve_outage(key_inconclusive, introspect_error(err))),
    }
}

/// Authenticate a discovery request (`GET /v1/service-info`, spec §5.3). There is no
/// authorisation. Like the gateway discovery route, it also accepts a token that IAM answers with
/// `identity-not-provisioned`: the console calls this route first. The other reasons of
/// `PermissionDenied`, and a `PermissionDenied` with no `ErrorInfo`, are refused.
pub async fn authenticate_discovery(iam: &dyn Iam, token: &str) -> Result<(), ApiError> {
    let key_inconclusive = match iam.introspect_api_key(token).await {
        Ok(resp) if resp.status == ACTIVE => return Ok(()),
        Ok(_) => false,
        Err(err) => introspect_error(err) == ApiError::IamUnavailable,
    };
    match iam.introspect_token(token).await {
        Ok(resp) if resp.status == ACTIVE => Ok(()),
        Ok(_) => Err(preserve_outage(key_inconclusive, ApiError::InvalidToken)),
        Err(IamError::Rpc(ref status)) if is_identity_not_provisioned(status) => Ok(()),
        Err(err) => Err(preserve_outage(key_inconclusive, introspect_error(err))),
    }
}

/// The org PRN and the principal PRN of a self-query for `org`.
fn self_query_prns(caller: &Caller, org: Uuid) -> (String, String) {
    let org_prn = Prn::build(IAM_SERVICE, "", None, ORGANIZATION, org).expect("static organization PRN parts are valid").canonical();
    (org_prn, caller.principal.as_prn().canonical())
}

/// Authorise a write into `inbox` (§6.3 steps 1 to 3, in that order).
///
/// A refusal that needs no IAM answer (a key out of scope) still runs the self-query, and then
/// returns [`ApiError::Forbidden`] whatever the answer is. Only an IAM outage changes it, to
/// [`ApiError::IamUnavailable`]. So an outage gives the same answer for an inbox that exists, an
/// inbox of another org and an inbox that does not exist (see [`deny_unknown_inbox`]).
pub async fn authorize_write(iam: &dyn Iam, caller: &Caller, inbox: &Inbox) -> Result<(), ApiError> {
    let org = inbox.org();

    // 1. The key scope: Root, or exactly the org of the inbox.
    if let CallerKind::ApiKey { scope_prn } = &caller.kind
        && !key_scope_allows(scope_prn, org)
    {
        return Err(refusal_after_probe(iam, caller, org).await);
    }

    // 2. The owner path. A UUID comparison: `PrincipalId::parse` made `caller.principal`.
    if caller.principal.uuid() == inbox.principal().uuid() {
        return match &caller.kind {
            CallerKind::ApiKey { .. } => Ok(()),
            CallerKind::Oidc { member_orgs } if member_orgs.contains(&org) => Ok(()),
            CallerKind::Oidc { .. } => Err(ApiError::Forbidden),
        };
    }

    // 3. The non-owner path: the self-query against the org of the inbox.
    let (org_prn, principal_prn) = self_query_prns(caller, org);
    match iam.is_authorized_self(&caller.credential, &principal_prn, WRITE_INBOX_EVENT_ACTION, &org_prn).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(ApiError::Forbidden),
        Err(err) => {
            let mapped = authz_error(err);
            if mapped == ApiError::Internal {
                // An exposure-gate denial of a self-query must not occur. Log it.
                tracing::error!(principal_prn = %principal_prn, "the self-query IsAuthorized returned an unexpected error; possible broken self-query");
            }
            Err(mapped)
        }
    }
}

/// The answer for an inbox that does not exist. It must look like a non-owner caller on an
/// inbox that exists: the same IAM calls, and the same answer during an IAM outage. So this runs
/// the self-query against a nil org and returns [`ApiError::Forbidden`], or
/// [`ApiError::IamUnavailable`] when IAM is unavailable. Without it, an outage is `503` for an
/// inbox that exists and `403` for one that does not, and a caller learns which inboxes exist.
pub async fn deny_unknown_inbox(iam: &dyn Iam, caller: &Caller) -> ApiError {
    refusal_after_probe(iam, caller, Uuid::nil()).await
}

/// Run a self-query for its outage only. The verdict and every other error are ignored: the
/// refusal is already decided. A probe can fail in ways that a real self-query cannot (a key
/// of another org), and these failures must not change the answer.
async fn refusal_after_probe(iam: &dyn Iam, caller: &Caller, org: Uuid) -> ApiError {
    let (org_prn, principal_prn) = self_query_prns(caller, org);
    let outage = match iam.is_authorized_self(&caller.credential, &principal_prn, WRITE_INBOX_EVENT_ACTION, &org_prn).await {
        Err(err) => authz_error(err) == ApiError::IamUnavailable,
        Ok(_) => false,
    };
    if outage { ApiError::IamUnavailable } else { ApiError::Forbidden }
}

/// The caller of an active API key.
fn api_key_caller(token: &str, resp: IntrospectApiKeyResponse) -> Result<Caller, ApiError> {
    Ok(Caller {
        principal: parse_principal(&resp.principal_prn)?,
        credential: CallerCredential::ApiKey(token.to_owned()),
        kind: CallerKind::ApiKey { scope_prn: resp.scope_prn },
    })
}

/// The caller of an active OIDC token.
fn oidc_caller(token: &str, resp: IntrospectResponse) -> Result<Caller, ApiError> {
    let member_orgs = resp.memberships.iter().filter_map(|m| org_of(&m.node_prn)).collect();
    Ok(Caller {
        principal: parse_principal(&resp.principal_prn)?,
        credential: CallerCredential::Bearer(token.to_owned()),
        kind: CallerKind::Oidc { member_orgs },
    })
}

/// Parse the principal PRN that IAM returned. A PRN that does not parse is an IAM fault, so it is
/// [`ApiError::Internal`] with an `error` log.
fn parse_principal(raw: &str) -> Result<PrincipalId, ApiError> {
    PrincipalId::parse(raw).map_err(|e| {
        tracing::error!(principal_prn = %raw, error = %e, "IAM returned a principal PRN that does not parse");
        ApiError::Internal
    })
}

/// §6.3 step 1: the scope of a key is the Root node, or exactly the organization `org`. A team,
/// a project, another org, and a scope that does not parse are all refused. UUIDs are compared.
fn key_scope_allows(scope_prn: &str, org: Uuid) -> bool {
    let Ok(prn) = Prn::parse(scope_prn) else {
        return false;
    };
    if prn.service() != IAM_SERVICE || !prn.region().is_empty() || prn.org().is_some() {
        return false;
    }
    match prn.resource_type() {
        ROOT => prn.resource_id().is_nil(),
        ORGANIZATION => prn.resource_id() == org,
        _ => false,
    }
}

/// The org a tenancy PRN belongs to, or `None` when the PRN names no org. A copy of the gateway
/// `org_of` (`paigasus-gateway/src/domain/mod.rs`).
fn org_of(raw: &str) -> Option<Uuid> {
    let prn = Prn::parse(raw).ok()?;
    if prn.service() != IAM_SERVICE {
        return None;
    }
    match prn.resource_type() {
        ORGANIZATION => Some(prn.resource_id()),
        "team" | "project" => prn.org(),
        _ => None,
    }
}

/// The reasons IAM sends on `PermissionDenied`. `as_wire_reason` allocates, so each is a
/// `LazyLock` (the gateway pattern). `expect`: a reason that the registry stops to declare must
/// fail loudly, not make each comparison false.
static IDENTITY_NOT_PROVISIONED: LazyLock<String> = LazyLock::new(|| ErrorReason::IdentityNotProvisioned.as_wire_reason().expect("a declared reason is never the sentinel"));
static PRINCIPAL_INACTIVE: LazyLock<String> = LazyLock::new(|| ErrorReason::PrincipalInactive.as_wire_reason().expect("a declared reason is never the sentinel"));
static PROVISIONING_FAILED: LazyLock<String> = LazyLock::new(|| ErrorReason::ProvisioningFailed.as_wire_reason().expect("a declared reason is never the sentinel"));

/// The `ErrorInfo` reason of an IAM `Status`, read on the IAM domain only, never from the
/// message. A `PermissionDenied` with no `ErrorInfo` is version skew, so it writes one warning.
fn iam_reason(status: &Status) -> Option<String> {
    let details = status.get_error_details();
    let Some(info) = details.error_info() else {
        tracing::warn!("IAM returned a PermissionDenied with no ErrorInfo; rolling-upgrade skew?");
        return None;
    };
    (info.domain == *paigasus_proto::error::IAM_DOMAIN).then(|| info.reason.clone())
}

/// Is this `Status` "validated, but not yet provisioned"? The reason decides. A `Status` with no
/// `ErrorInfo` fails closed.
fn is_identity_not_provisioned(status: &Status) -> bool {
    status.code() == Code::PermissionDenied && iam_reason(status).as_deref() == Some(IDENTITY_NOT_PROVISIONED.as_str())
}

/// Keep an IAM outage visible across the two legs. When the key leg reached no verdict, a 401 of
/// the token leg is not trustworthy: the token leg refuses every API key. So the answer is the
/// outage (503, retryable). Only `InvalidToken` is widened.
fn preserve_outage(key_inconclusive: bool, mapped: ApiError) -> ApiError {
    match mapped {
        ApiError::InvalidToken if key_inconclusive => ApiError::IamUnavailable,
        other => other,
    }
}

/// Map an error of an introspect call (either leg). `PermissionDenied` here is an inactive or
/// unprovisioned principal: a 401, not a 403. Every other code is an IAM failure (503).
fn introspect_error(err: IamError) -> ApiError {
    match err {
        IamError::Connect(_) => ApiError::IamUnavailable,
        IamError::Rpc(status) => match status.code() {
            Code::Unauthenticated | Code::PermissionDenied => ApiError::InvalidToken,
            _ => ApiError::IamUnavailable,
        },
    }
}

/// Map an error of the self-query. `PermissionDenied` is read by its reason:
/// `principal-inactive` and `provisioning-failed` are IAM refusing the caller's credential (401).
/// Any other `PermissionDenied` means that IAM denied a self-query, which is a fault (500).
fn authz_error(err: IamError) -> ApiError {
    match err {
        IamError::Connect(_) => ApiError::IamUnavailable,
        IamError::Rpc(status) => match status.code() {
            Code::PermissionDenied => match iam_reason(&status).as_deref() {
                Some(reason) if reason == PRINCIPAL_INACTIVE.as_str() || reason == PROVISIONING_FAILED.as_str() => ApiError::InvalidToken,
                _ => ApiError::Internal,
            },
            Code::Unauthenticated => ApiError::InvalidToken,
            _ => ApiError::IamUnavailable,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    use axum::http::HeaderValue;
    use axum::http::header::AUTHORIZATION;
    use jiff::Timestamp;
    use paigasus_agent_host_core::ids::{AgentId, InboxId};
    use paigasus_proto::paigasus::common::v1::ErrorReason;
    use paigasus_proto::paigasus::iam::v1::{IntrospectApiKeyResponse, IntrospectResponse, Membership};
    use tonic::{Code, Status};
    use tonic_types::{ErrorDetails, StatusExt};

    fn u(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }

    const TOKEN: &str = "caller-secret";

    fn org() -> Uuid {
        u(0x500)
    }

    fn other_org() -> Uuid {
        u(0x600)
    }

    fn owner() -> Uuid {
        u(0x10)
    }

    fn stranger() -> Uuid {
        u(0x11)
    }

    fn principal_prn(id: Uuid) -> String {
        format!("prn:pgs:iam:::principal/{id}")
    }

    fn org_prn(id: Uuid) -> String {
        format!("prn:pgs:iam:::organization/{id}")
    }

    fn inbox() -> Inbox {
        inbox_of(owner())
    }

    fn inbox_of(principal: Uuid) -> Inbox {
        Inbox::new(InboxId::new(org(), u(1)), AgentId::new(org(), u(2)), PrincipalId::from_uuid(principal), Timestamp::UNIX_EPOCH).expect("inbox")
    }

    /// One answer of a fake IAM call: a message, or a `Status` with an optional `ErrorInfo`.
    #[derive(Clone)]
    enum Leg<T> {
        Ok(T),
        Rpc(Code, Option<ErrorReason>),
    }

    fn status(code: Code, reason: Option<ErrorReason>) -> Status {
        match reason {
            None => Status::new(code, "fake"),
            Some(reason) => Status::with_error_details(
                code,
                "fake",
                ErrorDetails::with_error_info(
                    reason.as_wire_reason().expect("a declared reason"),
                    &*paigasus_proto::error::IAM_DOMAIN,
                    std::collections::HashMap::new(),
                ),
            ),
        }
    }

    impl<T: Clone> Leg<T> {
        fn answer(&self) -> Result<T, IamError> {
            match self {
                Leg::Ok(value) => Ok(value.clone()),
                Leg::Rpc(code, reason) => Err(IamError::Rpc(status(*code, *reason))),
            }
        }
    }

    type SelfQuery = (CallerCredential, String, String, String);

    /// A minimal in-module fake of the `Iam` port (ruling R13). It records each call by name, and
    /// the arguments of each self-query.
    struct FakeIam {
        key: Leg<IntrospectApiKeyResponse>,
        token: Leg<IntrospectResponse>,
        allowed: Leg<bool>,
        calls: Mutex<Vec<&'static str>>,
        self_queries: Mutex<Vec<SelfQuery>>,
    }

    impl FakeIam {
        fn new(key: Leg<IntrospectApiKeyResponse>, token: Leg<IntrospectResponse>, allowed: Leg<bool>) -> Self {
            FakeIam {
                key,
                token,
                allowed,
                calls: Mutex::new(Vec::new()),
                self_queries: Mutex::new(Vec::new()),
            }
        }

        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().unwrap().clone()
        }

        fn self_queries(&self) -> Vec<SelfQuery> {
            self.self_queries.lock().unwrap().clone()
        }
    }

    #[async_trait::async_trait]
    impl Iam for FakeIam {
        async fn introspect_api_key(&self, token: &str) -> Result<IntrospectApiKeyResponse, IamError> {
            assert_eq!(token, TOKEN);
            self.calls.lock().unwrap().push("introspect_api_key");
            self.key.answer()
        }

        async fn introspect_token(&self, token: &str) -> Result<IntrospectResponse, IamError> {
            assert_eq!(token, TOKEN);
            self.calls.lock().unwrap().push("introspect_token");
            self.token.answer()
        }

        async fn is_authorized_self(&self, caller: &CallerCredential, principal_prn: &str, action: &str, resource_prn: &str) -> Result<bool, IamError> {
            self.calls.lock().unwrap().push("is_authorized_self");
            self.self_queries
                .lock()
                .unwrap()
                .push((caller.clone(), principal_prn.to_owned(), action.to_owned(), resource_prn.to_owned()));
            self.allowed.answer()
        }
    }

    fn key(principal: &str, scope_prn: &str) -> Leg<IntrospectApiKeyResponse> {
        Leg::Ok(IntrospectApiKeyResponse {
            status: "active".to_owned(),
            principal_prn: principal.to_owned(),
            scope_prn: scope_prn.to_owned(),
            key_id: "key-1".to_owned(),
            ..Default::default()
        })
    }

    fn user(principal: &str, member_of: &[String]) -> Leg<IntrospectResponse> {
        Leg::Ok(IntrospectResponse {
            principal_prn: principal.to_owned(),
            status: "active".to_owned(),
            memberships: member_of
                .iter()
                .map(|node| Membership {
                    principal_prn: principal.to_owned(),
                    node_prn: node.clone(),
                    ..Default::default()
                })
                .collect(),
            ..Default::default()
        })
    }

    /// What IAM answers on the key leg for a token that is not an API key.
    fn not_a_key() -> Leg<IntrospectApiKeyResponse> {
        Leg::Rpc(Code::Unauthenticated, Some(ErrorReason::InvalidToken))
    }

    /// What IAM answers on the token leg for an API key.
    fn not_a_token() -> Leg<IntrospectResponse> {
        Leg::Rpc(Code::Unauthenticated, Some(ErrorReason::InvalidToken))
    }

    /// A self-query that must not run.
    fn no_self_query() -> Leg<bool> {
        Leg::Rpc(Code::Internal, None)
    }

    async fn write(iam: &FakeIam) -> Result<(), ApiError> {
        let caller = authenticate(iam, TOKEN).await?;
        authorize_write(iam, &caller, &inbox()).await
    }

    fn headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(AUTHORIZATION, HeaderValue::from_str(value).unwrap());
        headers
    }

    // ---- the Authorization header (Review Focus 5) ------------------------------------------

    #[test]
    fn bearer_token_reads_a_case_insensitive_scheme_and_trims() {
        assert_eq!(bearer_token(&headers("bearer  key ")), Ok("key".to_owned()));
        assert_eq!(bearer_token(&headers("BEARER key")), Ok("key".to_owned()));
        assert_eq!(bearer_token(&headers("Bearer key")), Ok("key".to_owned()));
    }

    #[test]
    fn dpop_and_other_schemes_are_invalid_token() {
        for value in ["DPoP x", "dpop x", "Basic x", "Bearer ", "Bearer", "x"] {
            assert_eq!(bearer_token(&headers(value)), Err(ApiError::InvalidToken), "{value:?}");
        }
        assert_eq!(bearer_token(&HeaderMap::new()), Err(ApiError::InvalidToken), "no header");
    }

    // ---- the §6.2 table: the IAM calls per credential ---------------------------------------

    #[tokio::test]
    async fn an_api_key_owner_costs_one_call() {
        let iam = FakeIam::new(key(&principal_prn(owner()), &org_prn(org())), not_a_token(), no_self_query());
        assert_eq!(write(&iam).await, Ok(()));
        assert_eq!(iam.calls(), vec!["introspect_api_key"]);
    }

    #[tokio::test]
    async fn an_api_key_non_owner_costs_two_calls() {
        let iam = FakeIam::new(key(&principal_prn(stranger()), &org_prn(org())), not_a_token(), Leg::Ok(true));
        assert_eq!(write(&iam).await, Ok(()));
        assert_eq!(iam.calls(), vec!["introspect_api_key", "is_authorized_self"]);
    }

    #[tokio::test]
    async fn an_oidc_owner_costs_two_calls() {
        let iam = FakeIam::new(not_a_key(), user(&principal_prn(owner()), &[org_prn(org())]), no_self_query());
        assert_eq!(write(&iam).await, Ok(()));
        assert_eq!(iam.calls(), vec!["introspect_api_key", "introspect_token"]);
    }

    #[tokio::test]
    async fn an_oidc_non_owner_costs_three_calls() {
        let iam = FakeIam::new(not_a_key(), user(&principal_prn(stranger()), &[org_prn(org())]), Leg::Ok(true));
        assert_eq!(write(&iam).await, Ok(()));
        assert_eq!(iam.calls(), vec!["introspect_api_key", "introspect_token", "is_authorized_self"]);
    }

    #[tokio::test]
    async fn the_self_query_uses_the_callers_own_credential_and_the_org() {
        let iam = FakeIam::new(not_a_key(), user(&principal_prn(stranger()), &[org_prn(org())]), Leg::Ok(true));
        assert_eq!(write(&iam).await, Ok(()));
        assert_eq!(
            iam.self_queries(),
            vec![(CallerCredential::Bearer(TOKEN.to_owned()), principal_prn(stranger()), "WriteInboxEvent".to_owned(), org_prn(org()))]
        );

        // The API-key form of the same rule.
        let iam = FakeIam::new(key(&principal_prn(stranger()), &org_prn(org())), not_a_token(), Leg::Ok(true));
        assert_eq!(write(&iam).await, Ok(()));
        assert_eq!(
            iam.self_queries(),
            vec![(CallerCredential::ApiKey(TOKEN.to_owned()), principal_prn(stranger()), "WriteInboxEvent".to_owned(), org_prn(org()))]
        );
    }

    #[tokio::test]
    async fn a_refused_non_owner_is_forbidden() {
        let iam = FakeIam::new(key(&principal_prn(stranger()), &org_prn(org())), not_a_token(), Leg::Ok(false));
        assert_eq!(write(&iam).await, Err(ApiError::Forbidden));
        let iam = FakeIam::new(not_a_key(), user(&principal_prn(stranger()), &[org_prn(org())]), Leg::Ok(false));
        assert_eq!(write(&iam).await, Err(ApiError::Forbidden));
    }

    // ---- §6.3 step 1: the key scope ---------------------------------------------------------

    /// The table of `key_scope_allows`: each refused row trips one sub-guard.
    #[test]
    fn key_scope_allows_table() {
        let o = org();
        let rows: Vec<(String, bool)> = vec![
            ("prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000".to_string(), true),
            (format!("prn:pgs:iam:::organization/{o}"), true),
            (format!("prn:pgs:iam:::organization/{}", o.to_string().to_uppercase()), true),
            // Wrong service.
            (format!("prn:pgs:agents:::organization/{o}"), false),
            // A region.
            (format!("prn:pgs:iam:eu::organization/{o}"), false),
            // An org segment.
            (format!("prn:pgs:iam::{o}:organization/{o}"), false),
            // Root with an id that is not nil.
            (format!("prn:pgs:iam:::root/{}", u(0x77)), false),
            // Another org.
            (format!("prn:pgs:iam:::organization/{}", u(0x600)), false),
            (String::new(), false),
            ("garbage".to_string(), false),
        ];
        for (scope, want) in rows {
            assert_eq!(key_scope_allows(&scope, o), want, "scope {scope:?}");
        }
    }

    #[tokio::test]
    async fn a_key_scoped_to_another_org_is_forbidden() {
        for principal in [stranger(), owner()] {
            let iam = FakeIam::new(key(&principal_prn(principal), &org_prn(other_org())), not_a_token(), Leg::Ok(true));
            assert_eq!(write(&iam).await, Err(ApiError::Forbidden));
            assert_eq!(iam.calls(), vec!["introspect_api_key", "is_authorized_self"], "a key out of scope runs a probe self-query");
        }
    }

    // The probe ignores the verdict: IAM that says yes does not open the door for a key out of scope.
    #[tokio::test]
    async fn a_key_out_of_scope_stays_forbidden_when_the_probe_says_yes_or_fails() {
        for probe in [
            Leg::Ok(true),
            Leg::Ok(false),
            Leg::Rpc(Code::PermissionDenied, Some(ErrorReason::Forbidden)),
            Leg::Rpc(Code::Unauthenticated, None),
        ] {
            let iam = FakeIam::new(key(&principal_prn(stranger()), &org_prn(other_org())), not_a_token(), probe);
            assert_eq!(write(&iam).await, Err(ApiError::Forbidden));
        }
    }

    // An outage gives the same answer for a key out of scope, a non-owner and an unknown inbox.
    #[tokio::test]
    async fn an_outage_of_the_self_query_is_503_on_every_refusal_path() {
        let outage = || Leg::Rpc(Code::Unavailable, None);
        let out_of_scope = FakeIam::new(key(&principal_prn(stranger()), &org_prn(other_org())), not_a_token(), outage());
        assert_eq!(write(&out_of_scope).await, Err(ApiError::IamUnavailable));
        let non_owner = FakeIam::new(key(&principal_prn(stranger()), &org_prn(org())), not_a_token(), outage());
        assert_eq!(write(&non_owner).await, Err(ApiError::IamUnavailable));
        let unknown = FakeIam::new(key(&principal_prn(stranger()), &org_prn(org())), not_a_token(), outage());
        let caller = authenticate(&unknown, TOKEN).await.expect("caller");
        assert_eq!(deny_unknown_inbox(&unknown, &caller).await, ApiError::IamUnavailable);
        assert_eq!(unknown.calls(), non_owner.calls());
    }

    #[tokio::test]
    async fn an_unknown_inbox_is_forbidden_when_iam_answers() {
        for verdict in [true, false] {
            let iam = FakeIam::new(key(&principal_prn(stranger()), &org_prn(org())), not_a_token(), Leg::Ok(verdict));
            let caller = authenticate(&iam, TOKEN).await.expect("caller");
            assert_eq!(deny_unknown_inbox(&iam, &caller).await, ApiError::Forbidden, "verdict {verdict}");
        }
    }

    #[tokio::test]
    async fn a_key_scoped_to_a_team_or_project_is_forbidden() {
        let team = format!("prn:pgs:iam::{}:team/{}", org(), u(0x700));
        let project = format!("prn:pgs:iam::{}:project/{}", org(), u(0x701));
        for scope in [team, project] {
            for principal in [owner(), stranger()] {
                let iam = FakeIam::new(key(&principal_prn(principal), &scope), not_a_token(), Leg::Ok(true));
                assert_eq!(write(&iam).await, Err(ApiError::Forbidden), "scope {scope}, principal {principal}");
                assert_eq!(iam.calls(), vec!["introspect_api_key", "is_authorized_self"]);
            }
        }
    }

    #[tokio::test]
    async fn a_root_key_passes_the_scope_rule() {
        let root = "prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000";
        let iam = FakeIam::new(key(&principal_prn(stranger()), root), not_a_token(), Leg::Ok(true));
        assert_eq!(write(&iam).await, Ok(()));
        assert_eq!(iam.calls(), vec!["introspect_api_key", "is_authorized_self"]);
    }

    // ---- §6.3 step 2: the owner path --------------------------------------------------------

    #[tokio::test]
    async fn an_oidc_owner_without_membership_is_forbidden() {
        let iam = FakeIam::new(not_a_key(), user(&principal_prn(owner()), &[org_prn(other_org())]), Leg::Ok(true));
        assert_eq!(write(&iam).await, Err(ApiError::Forbidden));
        assert_eq!(iam.calls().len(), 2, "{:?}", iam.calls());
    }

    #[tokio::test]
    async fn the_owner_check_compares_uuids() {
        // The owner UUID needs hex letters, or its upper-case form is the same string. So this
        // test uses u(0xabc10), not u(0x10).
        let owner = u(0xabc10);
        let upper = principal_prn(owner).replace(&owner.to_string(), &owner.to_string().to_uppercase());
        assert_ne!(upper, principal_prn(owner), "the fixture must differ as a string");
        let iam = FakeIam::new(key(&upper, &org_prn(org())), not_a_token(), no_self_query());
        let caller = authenticate(&iam, TOKEN).await.expect("authenticated");
        assert_eq!(authorize_write(&iam, &caller, &inbox_of(owner)).await, Ok(()));
        assert_eq!(iam.calls(), vec!["introspect_api_key"], "the owner path makes one call");
    }

    // ---- the two legs -----------------------------------------------------------------------

    #[tokio::test]
    async fn an_inactive_key_falls_through_to_the_token_leg() {
        let revoked = Leg::Ok(IntrospectApiKeyResponse {
            status: "revoked".to_owned(),
            principal_prn: principal_prn(owner()),
            scope_prn: org_prn(org()),
            ..Default::default()
        });
        let iam = FakeIam::new(revoked, user(&principal_prn(owner()), &[org_prn(org())]), no_self_query());
        assert_eq!(write(&iam).await, Ok(()));
        assert_eq!(iam.calls(), vec!["introspect_api_key", "introspect_token"]);

        // With the token leg also refused, the caller is not authenticated.
        let revoked = Leg::Ok(IntrospectApiKeyResponse {
            status: "revoked".to_owned(),
            ..Default::default()
        });
        let iam = FakeIam::new(revoked, not_a_token(), no_self_query());
        assert_eq!(authenticate(&iam, TOKEN).await.map(|_| ()), Err(ApiError::InvalidToken));
    }

    #[tokio::test]
    async fn an_iam_outage_on_both_legs_is_iam_unavailable() {
        let iam = FakeIam::new(Leg::Rpc(Code::Unavailable, None), Leg::Rpc(Code::Unavailable, None), no_self_query());
        assert_eq!(authenticate(&iam, TOKEN).await.map(|_| ()), Err(ApiError::IamUnavailable));

        // preserve_outage: a key leg with no verdict turns a token-leg 401 into a 503.
        let iam = FakeIam::new(Leg::Rpc(Code::Unavailable, None), not_a_token(), no_self_query());
        assert_eq!(authenticate(&iam, TOKEN).await.map(|_| ()), Err(ApiError::IamUnavailable));

        // The control: a key leg with a verdict keeps the 401.
        let iam = FakeIam::new(not_a_key(), not_a_token(), no_self_query());
        assert_eq!(authenticate(&iam, TOKEN).await.map(|_| ()), Err(ApiError::InvalidToken));
    }

    #[tokio::test]
    async fn a_principal_that_is_not_a_principal_prn_is_internal() {
        let iam = FakeIam::new(key("not-a-prn", &org_prn(org())), not_a_token(), no_self_query());
        assert_eq!(authenticate(&iam, TOKEN).await.map(|_| ()), Err(ApiError::Internal));
    }

    #[tokio::test]
    async fn self_query_errors_map_like_the_gateway() {
        let cases = [
            (Leg::Rpc(Code::Unavailable, None), ApiError::IamUnavailable),
            (Leg::Rpc(Code::Unauthenticated, Some(ErrorReason::InvalidToken)), ApiError::InvalidToken),
            (Leg::Rpc(Code::PermissionDenied, Some(ErrorReason::PrincipalInactive)), ApiError::InvalidToken),
            (Leg::Rpc(Code::PermissionDenied, Some(ErrorReason::Forbidden)), ApiError::Internal),
            (Leg::Rpc(Code::PermissionDenied, None), ApiError::Internal),
        ];
        for (answer, expected) in cases {
            let iam = FakeIam::new(key(&principal_prn(stranger()), &org_prn(org())), not_a_token(), answer);
            assert_eq!(write(&iam).await, Err(expected));
        }
    }

    // ---- discovery (spec §5.3) ----------------------------------------------------------------

    #[tokio::test]
    async fn discovery_accepts_identity_not_provisioned() {
        let iam = FakeIam::new(not_a_key(), Leg::Rpc(Code::PermissionDenied, Some(ErrorReason::IdentityNotProvisioned)), no_self_query());
        assert_eq!(authenticate_discovery(&iam, TOKEN).await, Ok(()));
        assert_eq!(iam.calls(), vec!["introspect_api_key", "introspect_token"], "discovery never authorizes");

        // The write path does not accept it.
        assert_eq!(authenticate(&iam, TOKEN).await.map(|_| ()), Err(ApiError::InvalidToken));
    }

    #[tokio::test]
    async fn discovery_refuses_principal_inactive() {
        let iam = FakeIam::new(not_a_key(), Leg::Rpc(Code::PermissionDenied, Some(ErrorReason::PrincipalInactive)), no_self_query());
        assert_eq!(authenticate_discovery(&iam, TOKEN).await, Err(ApiError::InvalidToken));
        // A PermissionDenied with no ErrorInfo fails closed too.
        let iam = FakeIam::new(not_a_key(), Leg::Rpc(Code::PermissionDenied, None), no_self_query());
        assert_eq!(authenticate_discovery(&iam, TOKEN).await, Err(ApiError::InvalidToken));
    }

    #[tokio::test]
    async fn discovery_accepts_an_active_key_or_token_with_no_self_query() {
        let iam = FakeIam::new(key(&principal_prn(stranger()), &org_prn(other_org())), not_a_token(), no_self_query());
        assert_eq!(authenticate_discovery(&iam, TOKEN).await, Ok(()));
        assert_eq!(iam.calls(), vec!["introspect_api_key"]);
        let iam = FakeIam::new(not_a_key(), user(&principal_prn(stranger()), &[]), no_self_query());
        assert_eq!(authenticate_discovery(&iam, TOKEN).await, Ok(()));
        let iam = FakeIam::new(Leg::Rpc(Code::Unavailable, None), not_a_token(), no_self_query());
        assert_eq!(authenticate_discovery(&iam, TOKEN).await, Err(ApiError::IamUnavailable));
    }

    #[test]
    fn org_of_reads_org_team_and_project() {
        assert_eq!(org_of(&org_prn(org())), Some(org()));
        assert_eq!(org_of(&format!("prn:pgs:iam::{}:team/{}", org(), u(1))), Some(org()));
        assert_eq!(org_of(&format!("prn:pgs:iam::{}:project/{}", org(), u(1))), Some(org()));
        assert_eq!(org_of(&principal_prn(owner())), None);
        assert_eq!(org_of(&format!("prn:pgs:agents:::organization/{}", org())), None);
        assert_eq!(org_of("garbage"), None);
    }
}
