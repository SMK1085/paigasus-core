// SPDX-License-Identifier: Apache-2.0

//! Cold-start bootstrap-admin seeding (SMA-444 Task 21b, spec D9 / challenge M4): a fresh
//! deployment has NO `platform_admin` grant, so nobody can create organizations or grant
//! roles — total lockout. `authz.bootstrap_admins` (Task 21, `config::BootstrapAdmin`)
//! configures a set of `(issuer, subject)` OIDC identities that should be JIT-granted
//! `platform_admin`@`Root` the first time they authenticate — principals are minted
//! server-side only on first login, so a bootstrap admin can't be pre-seeded any other way.
//!
//! [`BootstrapAdminSeeder::ensure_platform_admin`] is called by BOTH the HTTP bearer
//! middleware (`adapters::http::auth_middleware::require_bearer`) and the gRPC enforcement
//! layer (`adapters::grpc::authn::AuthEnforce`), immediately after a successful
//! `AuthenticateToken::resolve(.., Provisioning::Enabled)` and before the resolved principal
//! is handed to the protected handler. It is deliberately NEVER called from the read-only
//! `introspect` path (`Provisioning::Disabled`) — D10 requires introspect to have no side
//! effects, and a role-grant insert is exactly the kind of side effect that guarantee rules
//! out.

use paigasus_iam_core::authz::model::GrantScope;
use paigasus_iam_core::{Clock, IdGenerator, Issuer, PrincipalId, RoleGrant, RoleGrantStore};
use std::collections::HashSet;
use std::sync::Arc;

use crate::config::BootstrapAdmin;

/// The system role key a bootstrap-admin identity is seeded — the same key
/// `authz::roles::system_roles()` defines for `platform_admin` (Root-only scope, spec §3.2).
const PLATFORM_ADMIN_ROLE_KEY: &str = "platform_admin";

/// Seeds a `platform_admin`@`Root` [`RoleGrant`] for a configured bootstrap-admin identity,
/// on first authentication. Built once in `AppState::new` from `cfg.authz.bootstrap_admins`;
/// `AppState` clones hold the SAME `Arc<dyn RoleGrantStore>` the rest of the composition root
/// shares, so a seeded grant bumps the identical `policy_gen` counter `CedarAuthorizer` polls
/// (mirrors `RoleService::grant`'s wiring, and `PgRoleGrantStore::grant`'s own doc contract:
/// "inserts the grant row and bumps the policy generation counter").
#[derive(Clone)]
pub struct BootstrapAdminSeeder<I, C> {
    /// The configured `(issuer, subject)` set, pre-parsed once at construction time so the
    /// hot path (`ensure_platform_admin`, called on every authenticated request) is a pure
    /// in-memory lookup — no `Issuer::parse` per request. `Arc`-wrapped so cloning
    /// `BootstrapAdminSeeder` (mirroring `AppState`'s cheap-`Clone` posture) never copies the
    /// set itself.
    admins: Arc<HashSet<(Issuer, String)>>,
    grants: Arc<dyn RoleGrantStore>,
    ids: I,
    clock: C,
}

impl<I, C> BootstrapAdminSeeder<I, C>
where
    I: IdGenerator,
    C: Clock,
{
    /// `IamConfig::validate` already rejects any `bootstrap_admins` entry with an unparseable
    /// issuer at boot time, so a parse failure reaching here is a wiring defect (validate
    /// skipped, or a hand-built config in a test) rather than an operator error — skip the
    /// entry with a loud warning instead of panicking the composition root over it.
    #[must_use]
    pub fn new(configured: &[BootstrapAdmin], grants: Arc<dyn RoleGrantStore>, ids: I, clock: C) -> Self {
        let admins = configured
            .iter()
            .filter_map(|admin| match Issuer::parse(&admin.issuer) {
                Ok(issuer) => Some((issuer, admin.subject.clone())),
                Err(e) => {
                    tracing::warn!(
                        issuer = %admin.issuer,
                        error = %e,
                        "authz.bootstrap_admins entry has an unparseable issuer (IamConfig::validate should have rejected this at boot) — skipping"
                    );
                    None
                }
            })
            .collect();
        Self {
            admins: Arc::new(admins),
            grants,
            ids,
            clock,
        }
    }

    /// No-op (and no store round-trip) unless `(issuer, subject)` is in the configured
    /// bootstrap set — every ordinary, non-bootstrap authentication pays nothing beyond one
    /// `HashSet` lookup. When it IS configured: idempotent — a `platform_admin`@`Root` grant
    /// already held by `principal` is left alone; otherwise one is minted and persisted.
    ///
    /// BEST-EFFORT: any store error is logged and swallowed, never propagated — a seeding
    /// hiccup must not fail the request that triggered it. The identity self-heals on its
    /// next authentication, since the check-then-seed sequence is itself idempotent.
    pub async fn ensure_platform_admin(&self, principal: &PrincipalId, issuer: &Issuer, subject: &str) {
        if !self.admins.contains(&(issuer.clone(), subject.to_string())) {
            return;
        }

        let existing = match self.grants.list_by_principal(principal).await {
            Ok(grants) => grants,
            Err(e) => {
                tracing::warn!(
                    principal = %principal.canonical(),
                    error = %e,
                    "bootstrap-admin seeding: failed to list existing role grants; will retry on the next authentication"
                );
                return;
            }
        };
        if existing.iter().any(|g| g.role_key == PLATFORM_ADMIN_ROLE_KEY && g.scope == GrantScope::Root) {
            return;
        }

        let id = self.ids.new_membership_id();
        let grant = RoleGrant {
            id,
            principal: principal.clone(),
            role_key: PLATFORM_ADMIN_ROLE_KEY.to_string(),
            scope: GrantScope::Root,
            linked_policy_id: format!("grant:{id}"),
            created_at: self.clock.now(),
        };
        if let Err(e) = self.grants.grant(&grant).await {
            tracing::warn!(
                principal = %principal.canonical(),
                error = %e,
                "bootstrap-admin seeding: failed to persist the platform_admin grant; will retry on the next authentication"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FixedClock, InMemoryRoleGrants, SeqIds};
    use paigasus_kernel::Prn;
    use uuid::Uuid;

    fn principal(n: u128) -> PrincipalId {
        PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(n)).unwrap())
    }

    fn issuer(raw: &str) -> Issuer {
        Issuer::parse(raw).unwrap()
    }

    /// `InMemoryRoleGrants`'s own backing-map type (`fakes.rs`'s `pub Arc<Mutex<HashMap<Uuid,
    /// RoleGrant>>>` field) — named here so `seeder`'s return type stays readable.
    type GrantsBacking = Arc<std::sync::Mutex<std::collections::HashMap<Uuid, RoleGrant>>>;

    fn seeder(configured: &[BootstrapAdmin]) -> (BootstrapAdminSeeder<SeqIds, FixedClock>, GrantsBacking) {
        let grants = InMemoryRoleGrants::default();
        let backing = grants.0.clone();
        (BootstrapAdminSeeder::new(configured, Arc::new(grants), SeqIds::default(), FixedClock::default()), backing)
    }

    #[tokio::test]
    async fn non_configured_identity_never_touches_the_store() {
        let (seeder, backing) = seeder(&[]);
        seeder.ensure_platform_admin(&principal(1), &issuer("https://idp.example.com"), "sub-1").await;
        assert!(backing.lock().unwrap().is_empty(), "a non-bootstrap identity must not get a grant");
    }

    #[tokio::test]
    async fn configured_identity_gets_a_platform_admin_root_grant() {
        let (seeder, backing) = seeder(&[BootstrapAdmin {
            issuer: "https://idp.example.com".to_string(),
            subject: "sub-admin".to_string(),
        }]);
        let p = principal(1);
        seeder.ensure_platform_admin(&p, &issuer("https://idp.example.com"), "sub-admin").await;

        let grants = backing.lock().unwrap();
        assert_eq!(grants.len(), 1);
        let grant = grants.values().next().unwrap();
        assert_eq!(grant.principal, p);
        assert_eq!(grant.role_key, "platform_admin");
        assert_eq!(grant.scope, GrantScope::Root);
        assert_eq!(grant.linked_policy_id, format!("grant:{}", grant.id));
    }

    #[tokio::test]
    async fn a_second_authentication_does_not_create_a_duplicate_grant() {
        let (seeder, backing) = seeder(&[BootstrapAdmin {
            issuer: "https://idp.example.com".to_string(),
            subject: "sub-admin".to_string(),
        }]);
        let p = principal(1);
        let iss = issuer("https://idp.example.com");
        seeder.ensure_platform_admin(&p, &iss, "sub-admin").await;
        seeder.ensure_platform_admin(&p, &iss, "sub-admin").await;

        assert_eq!(backing.lock().unwrap().len(), 1, "idempotent: a second authentication must not duplicate the grant");
    }

    #[tokio::test]
    async fn an_existing_platform_admin_grant_is_left_untouched() {
        // Even if the grant was seeded some other way (e.g. an operator-run `psql` seed
        // ahead of Task 21b landing), `ensure_platform_admin` must not insert a second one.
        let (seeder, backing) = seeder(&[BootstrapAdmin {
            issuer: "https://idp.example.com".to_string(),
            subject: "sub-admin".to_string(),
        }]);
        let p = principal(1);
        let pre_existing = RoleGrant {
            id: Uuid::from_u128(999),
            principal: p.clone(),
            role_key: "platform_admin".to_string(),
            scope: GrantScope::Root,
            linked_policy_id: "grant:pre-existing".to_string(),
            created_at: chrono::Utc::now(),
        };
        backing.lock().unwrap().insert(pre_existing.id, pre_existing.clone());

        seeder.ensure_platform_admin(&p, &issuer("https://idp.example.com"), "sub-admin").await;

        let grants = backing.lock().unwrap();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants.get(&pre_existing.id), Some(&pre_existing));
    }

    #[tokio::test]
    async fn a_matching_issuer_with_a_different_subject_is_not_seeded() {
        let (seeder, backing) = seeder(&[BootstrapAdmin {
            issuer: "https://idp.example.com".to_string(),
            subject: "sub-admin".to_string(),
        }]);
        seeder.ensure_platform_admin(&principal(1), &issuer("https://idp.example.com"), "sub-other").await;
        assert!(backing.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_matching_subject_with_a_different_issuer_is_not_seeded() {
        let (seeder, backing) = seeder(&[BootstrapAdmin {
            issuer: "https://idp.example.com".to_string(),
            subject: "sub-admin".to_string(),
        }]);
        seeder.ensure_platform_admin(&principal(1), &issuer("https://other-idp.example.com"), "sub-admin").await;
        assert!(backing.lock().unwrap().is_empty());
    }

    #[test]
    fn an_unparseable_configured_issuer_is_skipped_not_a_construction_error() {
        // `IamConfig::validate` rejects this at boot in production; this proves the fallback
        // (skip + warn) doesn't panic when it's reached anyway (e.g. a hand-built config in a
        // test that bypasses `validate`).
        let (seeder, _backing) = seeder(&[BootstrapAdmin {
            issuer: "not-a-valid-issuer".to_string(),
            subject: "sub-admin".to_string(),
        }]);
        assert!(seeder.admins.is_empty());
    }
}
