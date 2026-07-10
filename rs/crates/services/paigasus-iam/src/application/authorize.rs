// SPDX-License-Identifier: Apache-2.0

//! `Authorize`: the application-layer wrapper around the `Authorizer` port (ADR-0013) that
//! every authz-aware use case (`RoleService`, `PolicyService`, and the `IsAuthorized` query
//! handlers) calls through, rather than reaching for the shared `Arc<dyn Authorizer>`
//! directly. `check` is the enforcement entry point: it collapses a `Decision` into a
//! `Result`, mapping `Effect::Deny` onto the wire-stable `TenancyError::Forbidden` (SMA-444
//! task-16 brief: the 403 body never carries the denying policy id) while a genuine
//! evaluation/backend failure from the authorizer itself surfaces as `TenancyError::Internal`
//! via `From<AuthzError>` — a deny and an error are never conflated. `decide` is a thin
//! passthrough for callers that need the raw `Decision` (including `determining_policies`)
//! rather than a collapsed allow/deny `Result`. `decide_gated` layers the `IsAuthorized`
//! self/admin exposure rule on top of `decide` — see its own doc for the rule itself; both
//! `adapters::http::authz::is_authorized` and `adapters::grpc::authz::AuthzGrpc::is_authorized`
//! (SMA-444 Task 18/19) call it exclusively, so the two transports can never diverge.

use crate::application::error::TenancyError;
use paigasus_iam_core::{AccessRequest, Action, Authorizer, AuthzError, Decision, Effect, RequestContext};
use paigasus_kernel::Prn;
use std::sync::Arc;

/// Wraps the shared `Arc<dyn Authorizer>` (the same handle `AppState.authz` holds) behind the
/// two operations application services need: enforce (`check`) and inspect (`decide`).
/// `Clone` is cheap (an `Arc` clone) — every service that embeds an `Authorize` derives
/// `Clone` too, mirroring `MembershipService`/`OrganizationService`'s posture.
#[derive(Clone)]
pub struct Authorize {
    authorizer: Arc<dyn Authorizer>,
}

impl Authorize {
    #[must_use]
    pub fn new(authorizer: Arc<dyn Authorizer>) -> Self {
        Self { authorizer }
    }

    /// Builds an `AccessRequest` with an empty `RequestContext` (no Task 17 use case needs
    /// context attributes) and enforces it: `Effect::Allow` -> `Ok(())`, `Effect::Deny` ->
    /// `TenancyError::Forbidden`. An `AuthzError` from the authorizer itself (evaluation or
    /// backend failure, not a denial) propagates via `?`/`From<AuthzError>` as
    /// `TenancyError::Internal` — never as `Forbidden`, so callers can't mistake "the policy
    /// engine broke" for "the policy engine said no."
    pub async fn check(&self, actor: &Prn, action: Action, resource: &Prn) -> Result<(), TenancyError> {
        let req = AccessRequest {
            principal: actor.clone(),
            action,
            resource: resource.clone(),
            context: RequestContext::empty(),
        };
        let decision = self.authorizer.is_authorized(&req).await?;
        match decision.effect {
            Effect::Allow => Ok(()),
            Effect::Deny => Err(TenancyError::Forbidden),
        }
    }

    /// Passthrough for callers that need the raw `Decision`, not collapsed into a
    /// `Result<(), _>`. `decide_gated` is what the `IsAuthorized` query handlers actually
    /// call; this stays public for `decide_gated`'s own self-query path and for tests.
    pub async fn decide(&self, req: &AccessRequest) -> Result<Decision, AuthzError> {
        self.authorizer.is_authorized(req).await
    }

    /// The `IsAuthorized` self/admin exposure rule (spec §9.2, SMA-444 Task 18/19), shared by
    /// every transport so they can never diverge: `actor` may always ask about themselves
    /// (`req.principal == actor`) and gets back the raw `Decision`. Asking about a DIFFERENT
    /// principal requires `actor` to already hold `Action::ListRoleGrants` AT `req.resource`
    /// — i.e. to already administer roles there. If that check denies,
    /// `TenancyError::Forbidden` propagates BEFORE `req` is ever decided for the probed
    /// principal — a caller who wasn't permitted to ask the question never sees a `Decision`
    /// (whose `determining_policies` can carry `grant:<uuid>` ids) or even the `allowed` bit.
    /// `adapters::http::authz::is_authorized` and `adapters::grpc::authz::AuthzGrpc::
    /// is_authorized` both call ONLY this (never `decide` directly) to decide a wire
    /// `IsAuthorized` request.
    pub async fn decide_gated(&self, actor: &Prn, req: &AccessRequest) -> Result<Decision, TenancyError> {
        if req.principal.canonical() != actor.canonical() {
            self.check(actor, Action::ListRoleGrants, &req.resource).await?;
        }
        Ok(self.decide(req).await?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::FakeAuthorizer;
    use paigasus_iam_core::authz::model::root_prn;
    use uuid::Uuid;

    fn principal(n: u128) -> Prn {
        Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap()
    }

    #[tokio::test]
    async fn check_allow_is_ok() {
        let fake = FakeAuthorizer::default();
        let resource = root_prn();
        fake.allow(Action::ListOrganizations, &resource);
        let authorize = Authorize::new(Arc::new(fake));

        assert!(authorize.check(&principal(1), Action::ListOrganizations, &resource).await.is_ok());
    }

    #[tokio::test]
    async fn check_deny_maps_to_forbidden_not_an_error() {
        let authorize = Authorize::new(Arc::new(FakeAuthorizer::default()));
        let resource = root_prn();

        assert_eq!(authorize.check(&principal(1), Action::ListOrganizations, &resource).await.unwrap_err(), TenancyError::Forbidden);
    }

    #[tokio::test]
    async fn decide_passes_through_the_raw_decision() {
        let fake = FakeAuthorizer::default();
        let resource = root_prn();
        fake.allow(Action::PutPolicy, &resource);
        let authorize = Authorize::new(Arc::new(fake));

        let req = AccessRequest {
            principal: principal(1),
            action: Action::PutPolicy,
            resource,
            context: RequestContext::empty(),
        };
        let decision = authorize.decide(&req).await.unwrap();
        assert_eq!(decision.effect, Effect::Allow);
    }

    #[tokio::test]
    async fn decide_gated_self_query_never_consults_the_authorizer_for_the_gate() {
        // Self is always visible — the default-deny fake never grants `ListRoleGrants`, so a
        // gate check here would fail; the raw (denied) `Decision` for the query itself is
        // still returned, not an error.
        let authorize = Authorize::new(Arc::new(FakeAuthorizer::default()));
        let actor = principal(1);
        let req = AccessRequest {
            principal: actor.clone(),
            action: Action::ListOrganizations,
            resource: root_prn(),
            context: RequestContext::empty(),
        };

        let decision = authorize.decide_gated(&actor, &req).await.unwrap();
        assert_eq!(decision.effect, Effect::Deny);
    }

    #[tokio::test]
    async fn decide_gated_non_self_denies_without_list_role_grants_at_the_resource() {
        let authorize = Authorize::new(Arc::new(FakeAuthorizer::default()));
        let actor = principal(1);
        let other = principal(2);
        let req = AccessRequest {
            principal: other,
            action: Action::ListOrganizations,
            resource: root_prn(),
            context: RequestContext::empty(),
        };

        assert_eq!(authorize.decide_gated(&actor, &req).await.unwrap_err(), TenancyError::Forbidden);
    }

    #[tokio::test]
    async fn decide_gated_non_self_succeeds_once_the_actor_holds_list_role_grants() {
        let fake = FakeAuthorizer::default();
        let resource = root_prn();
        fake.allow(Action::ListRoleGrants, &resource);
        fake.allow(Action::ListOrganizations, &resource);
        let authorize = Authorize::new(Arc::new(fake));
        let actor = principal(1);
        let other = principal(2);
        let req = AccessRequest {
            principal: other,
            action: Action::ListOrganizations,
            resource,
            context: RequestContext::empty(),
        };

        let decision = authorize.decide_gated(&actor, &req).await.unwrap();
        assert_eq!(decision.effect, Effect::Allow);
    }
}
