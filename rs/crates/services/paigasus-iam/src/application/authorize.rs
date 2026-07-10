// SPDX-License-Identifier: Apache-2.0

//! `Authorize`: the application-layer wrapper around the `Authorizer` port (ADR-0013) that
//! every authz-aware use case (`RoleService`, `PolicyService`, and later Task 18/19's
//! `IsAuthorized` query handler) calls through, rather than reaching for the shared
//! `Arc<dyn Authorizer>` directly. `check` is the enforcement entry point: it collapses a
//! `Decision` into a `Result`, mapping `Effect::Deny` onto the wire-stable
//! `TenancyError::Forbidden` (SMA-444 task-16 brief: the 403 body never carries the denying
//! policy id) while a genuine evaluation/backend failure from the authorizer itself surfaces
//! as `TenancyError::Internal` via `From<AuthzError>` — a deny and an error are never
//! conflated. `decide` is a thin passthrough for callers (the `IsAuthorized` query) that need
//! the raw `Decision` (including `determining_policies`) rather than a collapsed
//! allow/deny `Result`.

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

    /// Passthrough for the `IsAuthorized` query handler (Task 18/19 adds the self/admin
    /// exposure rule on top of this): the raw `Decision`, not collapsed into a `Result<(), _>`.
    pub async fn decide(&self, req: &AccessRequest) -> Result<Decision, AuthzError> {
        self.authorizer.is_authorized(req).await
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
}
