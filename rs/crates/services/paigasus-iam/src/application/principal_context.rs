// SPDX-License-Identifier: Apache-2.0

//! The membership half of a `PrincipalContext`, in ONE place.
//!
//! `AuthenticateToken::introspect` and `AuthenticateApiKey::introspect` used to run
//! byte-identical paging loops, each with its own `MEMBERSHIP_PAGE_SIZE` constant, and the
//! api-key copy's own comment admitted the duplication. SMA-632 needed the same loop a third
//! time (for `WhoAmI`), so the loop moved here instead.
//!
//! D13's rule is unchanged and now enforceable: this function is the only place that fetches a
//! principal's memberships for an authn context.

use paigasus_iam_core::{AuthnError, MembershipRecord, MembershipRepository, PrincipalId, RepositoryError};

/// `list_by_principal` page size for introspection's membership assembly (§6.1).
const MEMBERSHIP_PAGE_SIZE: u64 = 200;

/// Wraps any `RepositoryError` as `AuthnError::Backend` — the catch-all for repository
/// failures the authn use cases don't specifically interpret (§6.2 rule 4).
fn backend(err: RepositoryError) -> AuthnError {
    AuthnError::Backend(Box::new(err))
}

/// Every membership row for `principal`, paged internally.
///
/// Pages until a short page arrives. A page of exactly `MEMBERSHIP_PAGE_SIZE` rows is followed
/// by another request, so a principal with exactly 200 memberships costs two calls — that is the
/// cost of not being able to distinguish "full page, more to come" from "full page, that's all".
pub(crate) async fn load_all_memberships<M>(memberships: &M, principal: &PrincipalId) -> Result<Vec<MembershipRecord>, AuthnError>
where
    M: MembershipRepository,
{
    let mut all = Vec::new();
    let mut offset = 0u64;
    loop {
        let page = memberships.list_by_principal(principal.uuid(), MEMBERSHIP_PAGE_SIZE, offset).await.map_err(backend)?;
        let page_len = page.len() as u64;
        all.extend(page);
        if page_len < MEMBERSHIP_PAGE_SIZE {
            break;
        }
        offset += MEMBERSHIP_PAGE_SIZE;
    }
    Ok(all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::InMemoryMembershipRepository;
    use paigasus_kernel::Prn;
    use uuid::Uuid;

    /// Mirrors the other application-layer test fakes' `principal_id(n)` helper
    /// (`authenticate_token.rs`, `authenticate_api_key.rs`): `PrincipalId` has no
    /// `from_uuid` constructor, only `from_prn`.
    fn principal_id(n: u128) -> PrincipalId {
        PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap())
    }

    /// Builds a repository holding `count` memberships for one principal, and returns both.
    fn with_memberships(count: usize) -> (InMemoryMembershipRepository, PrincipalId) {
        let repo = InMemoryMembershipRepository::default();
        let principal = principal_id(1);
        repo.seed_for(&principal, count);
        (repo, principal)
    }

    #[tokio::test]
    async fn returns_nothing_for_a_principal_with_no_memberships() {
        let (repo, principal) = with_memberships(0);
        assert_eq!(load_all_memberships(&repo, &principal).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn returns_a_single_membership() {
        let (repo, principal) = with_memberships(1);
        assert_eq!(load_all_memberships(&repo, &principal).await.unwrap().len(), 1);
    }

    /// Exactly one full page. The loop cannot tell this from "more to come", so it asks again
    /// and gets an empty second page. Both facts are asserted: the count, and the two calls.
    #[tokio::test]
    async fn returns_exactly_one_full_page_and_asks_once_more() {
        let (repo, principal) = with_memberships(200);
        assert_eq!(load_all_memberships(&repo, &principal).await.unwrap().len(), 200);
        assert_eq!(repo.list_call_count(), 2);
    }

    /// The case a single un-paged call would get wrong: 201 rows behind a 200-row page size.
    /// With the loop deleted this returns 200 and the test fails.
    #[tokio::test]
    async fn pages_past_the_first_page() {
        let (repo, principal) = with_memberships(201);
        assert_eq!(load_all_memberships(&repo, &principal).await.unwrap().len(), 201);
        assert_eq!(repo.list_call_count(), 2);
    }
}
