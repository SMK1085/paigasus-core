// SPDX-License-Identifier: Apache-2.0

//! The authenticated caller and the org authorization through IAM. Ingress and the agent API
//! share this code (SMA-852 spec §6.5). It holds no transport type: `adapters::http::auth`
//! authenticates the caller and maps an [`AuthError`] to its HTTP answer.
//!
//! The steps of [`authorize_org`] (spec §6.5 steps 3 and 4):
//! 1. **Key scope.** The `scope_prn` of an API key is Root or exactly the target org. Out of
//!    scope runs the probe ([`refusal_after_probe`]) and then refuses.
//! 2. **The self-query.** `IsAuthorized` with the caller's own credential, the caller's own
//!    principal PRN, the action, and the org PRN. There is no owner path.
//!
//! [`deny_unknown`] is step 2 of §6.5: the probe against the nil org, for an id with no row.

use std::sync::LazyLock;

use paigasus_agent_host_core::ids::PrincipalId;
use paigasus_kernel::resource_name::Prn;
use paigasus_proto::paigasus::common::v1::ErrorReason;
use tonic::{Code, Status};
use tonic_types::StatusExt;
use uuid::Uuid;

use crate::adapters::iam::{CallerCredential, Iam, IamError};

/// The IAM action of a write to the agent registry: create, update, archive and bind. It is the
/// wire form of `Action::ManageAgents` in `paigasus-iam-core`.
pub const MANAGE_AGENTS_ACTION: &str = "ManageAgents";
/// The IAM action of a read of the agent registry. It is the wire form of `Action::ReadAgents`.
pub const READ_AGENTS_ACTION: &str = "ReadAgents";

/// The PRN parts of the IAM tenancy nodes.
pub(crate) const IAM_SERVICE: &str = "iam";
pub(crate) const ORGANIZATION: &str = "organization";
pub(crate) const ROOT: &str = "root";

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

/// A refusal of the authorization. The transport maps each variant to its answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    /// IAM refused the caller's credential.
    InvalidToken,
    /// The caller may not do this.
    Forbidden,
    /// IAM cannot be reached. The client can retry.
    IamUnavailable,
    /// An unexpected fault. The detail stays in the logs.
    Internal,
}

/// Spec §6.5 steps 3 and 4: the key scope (the probe on a refusal), then the self-query with
/// `action` against `org`. There is no owner path.
pub(crate) async fn authorize_org(iam: &dyn Iam, caller: &Caller, org: Uuid, action: &'static str) -> Result<(), AuthError> {
    if let CallerKind::ApiKey { scope_prn } = &caller.kind
        && !key_scope_allows(scope_prn, org)
    {
        return Err(refusal_after_probe(iam, caller, org, action).await);
    }
    self_query(iam, caller, org, action).await
}

/// Spec §6.5 step 2: the answer for an id with no row. It runs the self-query with `action`
/// against the nil org and returns [`AuthError::Forbidden`], or [`AuthError::IamUnavailable`]
/// during an outage. So an unknown id costs the same IAM calls as a refused caller on a row that
/// exists, and an outage gives the same answer for both.
pub(crate) async fn deny_unknown(iam: &dyn Iam, caller: &Caller, action: &'static str) -> AuthError {
    refusal_after_probe(iam, caller, Uuid::nil(), action).await
}

/// The self-query `IsAuthorized(caller principal, action, org PRN)` with the caller's own
/// credential. Denied gives [`AuthError::Forbidden`]. An error maps through [`authz_error`].
pub(crate) async fn self_query(iam: &dyn Iam, caller: &Caller, org: Uuid, action: &'static str) -> Result<(), AuthError> {
    let (org_prn, principal_prn) = self_query_prns(caller, org);
    match iam.is_authorized_self(&caller.credential, &principal_prn, action, &org_prn).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(AuthError::Forbidden),
        Err(err) => {
            let mapped = authz_error(err);
            if mapped == AuthError::Internal {
                // An exposure-gate denial of a self-query must not occur. Log it.
                tracing::error!(principal_prn = %principal_prn, "the self-query IsAuthorized returned an unexpected error; possible broken self-query");
            }
            Err(mapped)
        }
    }
}

/// Run a self-query with `action` for its outage only. The verdict and every other error are
/// ignored: the refusal is already decided. A probe can fail in ways that a real self-query
/// cannot (a key of another org), and these failures must not change the answer.
pub(crate) async fn refusal_after_probe(iam: &dyn Iam, caller: &Caller, org: Uuid, action: &'static str) -> AuthError {
    let (org_prn, principal_prn) = self_query_prns(caller, org);
    let outage = match iam.is_authorized_self(&caller.credential, &principal_prn, action, &org_prn).await {
        Err(err) => authz_error(err) == AuthError::IamUnavailable,
        Ok(_) => false,
    };
    if outage { AuthError::IamUnavailable } else { AuthError::Forbidden }
}

/// The org PRN and the principal PRN of a self-query for `org`.
fn self_query_prns(caller: &Caller, org: Uuid) -> (String, String) {
    let org_prn = Prn::build(IAM_SERVICE, "", None, ORGANIZATION, org).expect("static organization PRN parts are valid").canonical();
    (org_prn, caller.principal.as_prn().canonical())
}

/// The scope of a key is the Root node, or exactly the organization `org`. A team, a project,
/// another org, and a scope that does not parse are all refused. UUIDs are compared.
#[must_use]
pub(crate) fn key_scope_allows(scope_prn: &str, org: Uuid) -> bool {
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

/// The reasons IAM sends on `PermissionDenied`. `as_wire_reason` allocates, so each is a
/// `LazyLock` (the gateway pattern). `expect`: a reason that the registry stops to declare must
/// fail loudly, not make each comparison false.
static PRINCIPAL_INACTIVE: LazyLock<String> = LazyLock::new(|| ErrorReason::PrincipalInactive.as_wire_reason().expect("a declared reason is never the sentinel"));
static PROVISIONING_FAILED: LazyLock<String> = LazyLock::new(|| ErrorReason::ProvisioningFailed.as_wire_reason().expect("a declared reason is never the sentinel"));

/// The `ErrorInfo` reason of an IAM `Status`, read on the IAM domain only, never from the
/// message. A `PermissionDenied` with no `ErrorInfo` is version skew, so it writes one warning.
#[must_use]
pub(crate) fn iam_reason(status: &Status) -> Option<String> {
    let details = status.get_error_details();
    let Some(info) = details.error_info() else {
        tracing::warn!("IAM returned a PermissionDenied with no ErrorInfo; rolling-upgrade skew?");
        return None;
    };
    (info.domain == *paigasus_proto::error::IAM_DOMAIN).then(|| info.reason.clone())
}

/// Map an error of the self-query. `PermissionDenied` is read by its reason:
/// `principal-inactive` and `provisioning-failed` are IAM refusing the caller's credential (401).
/// Any other `PermissionDenied` means that IAM denied a self-query, which is a fault (500).
fn authz_error(err: IamError) -> AuthError {
    match err {
        IamError::Connect(_) => AuthError::IamUnavailable,
        IamError::Rpc(status) => match status.code() {
            Code::PermissionDenied => match iam_reason(&status).as_deref() {
                Some(reason) if reason == PRINCIPAL_INACTIVE.as_str() || reason == PROVISIONING_FAILED.as_str() => AuthError::InvalidToken,
                _ => AuthError::Internal,
            },
            Code::Unauthenticated => AuthError::InvalidToken,
            _ => AuthError::IamUnavailable,
        },
    }
}
