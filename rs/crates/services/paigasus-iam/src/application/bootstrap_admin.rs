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
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{AuditEntry, AuditLog, AuditOutcome, DomainEvent, EventType, Outbox, PolicyGenBumper, UnitOfWork};
use paigasus_iam_core::{Clock, IdGenerator, Issuer, PrincipalId, RoleGrant, RoleGrantStore};
use std::collections::HashSet;
use std::sync::Arc;

use metrics::counter;
use paigasus_observability::names;

use crate::config::BootstrapAdmin;

/// The system role key a bootstrap-admin identity is seeded — the same key
/// `authz::roles::system_roles()` defines for `platform_admin` (Root-only scope, spec §3.2).
const PLATFORM_ADMIN_ROLE_KEY: &str = "platform_admin";

/// Seeds a `platform_admin`@`Root` [`RoleGrant`] for a configured bootstrap-admin identity,
/// on first authentication. Built once in `AppState::new` from `cfg.authz.bootstrap_admins`.
/// SMA-468: the grant, its outbox event and its audit row commit together on one
/// `UnitOfWork`-scoped transaction (`grants.grant_in`/`outbox.enqueue`/`audit.record`, then
/// `tx.commit()`) — the same shape `RoleService::grant` uses — and only once that commit
/// succeeds does the seeder run an awaited, best-effort post-commit `gen_bumper.bump()` on its
/// own `PolicyGenBumper`, exactly as `RoleService::grant` does. That explicit bump is required
/// precisely because `RoleGrantStore::grant_in` (unlike the `grant` wrapper this replaced)
/// does NOT bump the policy generation counter itself — without it a freshly seeded admin
/// would be denied until the policy snapshot's TTL backstop.
#[derive(Clone)]
pub struct BootstrapAdminSeeder<I, C> {
    /// The configured `(issuer, subject)` set, pre-parsed once at construction time so the
    /// hot path (`ensure_platform_admin`, called on every authenticated request) is a pure
    /// in-memory lookup — no `Issuer::parse` per request. `Arc`-wrapped so cloning
    /// `BootstrapAdminSeeder` (mirroring `AppState`'s cheap-`Clone` posture) never copies the
    /// set itself.
    admins: Arc<HashSet<(Issuer, String)>>,
    grants: Arc<dyn RoleGrantStore>,
    uow: Arc<dyn UnitOfWork>,
    outbox: Arc<dyn Outbox>,
    audit: Arc<dyn AuditLog>,
    gen_bumper: Arc<dyn PolicyGenBumper>,
    ids: I,
    clock: C,
}

/// Named-field constructor input, mirroring `RoleServiceDeps` (`application/roles.rs:120`)
/// and for the same reason: with eight dependencies — four of them `Arc<dyn …>` — positional
/// arguments let a reordering silently swap two same-typed values past the compiler.
pub struct BootstrapAdminSeederDeps<I, C> {
    pub admins_config: Vec<BootstrapAdmin>,
    pub grants: Arc<dyn RoleGrantStore>,
    /// SMA-468: the seed's grant, audit row and outbox event commit in ONE transaction, so
    /// the seeder owns a `UnitOfWork` rather than leaning on `RoleGrantStore::grant`'s
    /// internal one-shot wrapper.
    pub uow: Arc<dyn UnitOfWork>,
    pub outbox: Arc<dyn Outbox>,
    pub audit: Arc<dyn AuditLog>,
    /// SMA-468 D5: `grant_in` does NOT bump `policy_gen` (only the `grant` wrapper does), so
    /// the seeder must bump post-commit itself or a freshly seeded admin is denied until the
    /// snapshot's TTL backstop.
    pub gen_bumper: Arc<dyn PolicyGenBumper>,
    pub ids: I,
    pub clock: C,
}

/// Why a seed attempt failed. Deliberately a local enum rather than funnelling through
/// `TenancyError`: `From<AuthzError> for TenancyError` collapses `Backend` into
/// `TenancyError::Internal`, whose `Display` is the constant `"internal server error"`
/// (`application/error.rs`). That would destroy the one diagnostic explaining WHY the
/// bootstrap admin was never seeded — the Postgres constraint name in the source error
/// (SMA-468 D7).
#[derive(Debug, thiserror::Error)]
enum SeedError {
    #[error(transparent)]
    Repository(#[from] paigasus_iam_core::RepositoryError),
    #[error(transparent)]
    Authz(#[from] paigasus_iam_core::AuthzError),
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
    pub fn new(deps: BootstrapAdminSeederDeps<I, C>) -> Self {
        let admins = deps
            .admins_config
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
            grants: deps.grants,
            uow: deps.uow,
            outbox: deps.outbox,
            audit: deps.audit,
            gen_bumper: deps.gen_bumper,
            ids: deps.ids,
            clock: deps.clock,
        }
    }

    /// The write half of a seed: the grant, its outbox event and its audit row commit in ONE
    /// transaction, then an awaited best-effort `policy_gen` bump post-commit — the same
    /// shape as `RoleService::grant` (`application/roles.rs:244-252`), which this cannot
    /// reuse because that method authorizes the caller first and bootstrap exists precisely
    /// to precede any authority.
    async fn seed_grant(&self, grant: &RoleGrant, issuer: &Issuer) -> Result<(), SeedError> {
        let corr = self.ids.new_correlation_id();
        let event = DomainEvent {
            id: self.ids.new_event_id(),
            event_type: EventType::RoleGranted,
            schema_version: 1,
            aggregate_prn: grant.principal.canonical(),
            // SMA-468 D2: no principal authorized this — operator configuration did.
            actor_prn: None,
            occurred_at: grant.created_at,
            // SMA-468 D4: PII-minimal — this crosses the outbox to an external broker, so it
            // carries neither the issuer nor the IdP subject.
            payload: serde_json::json!({
                "grant_id": grant.id,
                "role_key": grant.role_key,
                "scope": grant.scope.canonical_prn(),
                "source": "bootstrap_admins",
            }),
            correlation_id: Some(corr),
        };
        let entry = AuditEntry {
            id: self.ids.new_audit_id(),
            occurred_at: grant.created_at,
            actor_prn: None,
            action: "GrantRole".into(),
            resource_prn: Some(root_prn().canonical()),
            outcome: AuditOutcome::Committed,
            determining_policies: vec![],
            // SMA-468 D4: `principal_prn` is the ONLY field naming the grantee, since the
            // actor is null and `resource_prn` is the scope. The `issuer` gives provenance;
            // the IdP `subject` is deliberately absent (append-only table, erasure).
            detail: serde_json::json!({
                "principal_prn": grant.principal.canonical(),
                "grant_id": grant.id,
                "role_key": grant.role_key,
                "scope": grant.scope.canonical_prn(),
                "source": "bootstrap_admins",
                "issuer": issuer.as_str(),
            }),
            correlation_id: Some(corr),
        };

        let tx = self.uow.begin().await?;
        self.grants.grant_in(&*tx, grant).await?;
        self.outbox.enqueue(&*tx, &event).await?;
        self.audit.record(&*tx, &entry).await?;
        tx.commit().await?;

        // SMA-468 D5: `grant_in` does NOT bump (only `RoleGrantStore::grant` did), so this is
        // load-bearing, not polish — without it a freshly seeded admin is denied until the
        // policy snapshot's TTL backstop (~31s at defaults).
        self.gen_bumper.bump().await;
        Ok(())
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
                counter!(names::IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL, "stage" => "list").increment(1);
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
        if let Err(e) = self.seed_grant(&grant, issuer).await {
            counter!(names::IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL, "stage" => "txn").increment(1);
            tracing::warn!(
                principal = %principal.canonical(),
                error = %e,
                "bootstrap-admin seeding: failed to persist the platform_admin grant with its audit row; will retry on the next authentication. If this persists the bootstrap admin is NEVER seeded (lockout) — seed it manually and record the matching audit row"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::fakes::{FakeAuditLog, FakeOutbox, FakePolicyGenBumper, FakeUnitOfWork};
    use crate::application::fakes::{FixedClock, InMemoryRoleGrants, SeqIds};
    use paigasus_iam_core::authz::model::root_prn;
    use paigasus_iam_core::{AuditOutcome, AuthzError};
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

    /// Everything a test needs to assert on: the seeder plus the backing stores of every
    /// fake it writes through.
    struct Harness {
        seeder: BootstrapAdminSeeder<SeqIds, FixedClock>,
        grants: GrantsBacking,
        events: Arc<std::sync::Mutex<Vec<paigasus_iam_core::DomainEvent>>>,
        entries: Arc<std::sync::Mutex<Vec<paigasus_iam_core::AuditEntry>>>,
        bumps: FakePolicyGenBumper,
    }

    fn seeder(configured: &[BootstrapAdmin]) -> Harness {
        let grants = InMemoryRoleGrants::default();
        let grants_backing = grants.0.clone();
        let outbox = FakeOutbox::default();
        let events = outbox.0.clone();
        let audit = FakeAuditLog::default();
        let entries = audit.0.clone();
        let bumps = FakePolicyGenBumper::default();
        let seeder = BootstrapAdminSeeder::new(BootstrapAdminSeederDeps {
            admins_config: configured.to_vec(),
            grants: Arc::new(grants),
            uow: Arc::new(FakeUnitOfWork),
            outbox: Arc::new(outbox),
            audit: Arc::new(audit),
            gen_bumper: Arc::new(bumps.clone()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        });
        Harness {
            seeder,
            grants: grants_backing,
            events,
            entries,
            bumps,
        }
    }

    #[tokio::test]
    async fn non_configured_identity_never_touches_the_store() {
        let h = seeder(&[]);
        h.seeder.ensure_platform_admin(&principal(1), &issuer("https://idp.example.com"), "sub-1").await;
        assert!(h.grants.lock().unwrap().is_empty(), "a non-bootstrap identity must not get a grant");
    }

    #[tokio::test]
    async fn configured_identity_gets_a_platform_admin_root_grant() {
        let h = seeder(&[BootstrapAdmin {
            issuer: "https://idp.example.com".to_string(),
            subject: "sub-admin".to_string(),
        }]);
        let p = principal(1);
        h.seeder.ensure_platform_admin(&p, &issuer("https://idp.example.com"), "sub-admin").await;

        let grants = h.grants.lock().unwrap();
        assert_eq!(grants.len(), 1);
        let grant = grants.values().next().unwrap();
        assert_eq!(grant.principal, p);
        assert_eq!(grant.role_key, "platform_admin");
        assert_eq!(grant.scope, GrantScope::Root);
        assert_eq!(grant.linked_policy_id, format!("grant:{}", grant.id));
    }

    #[tokio::test]
    async fn a_second_authentication_does_not_create_a_duplicate_grant() {
        let h = seeder(&[BootstrapAdmin {
            issuer: "https://idp.example.com".to_string(),
            subject: "sub-admin".to_string(),
        }]);
        let p = principal(1);
        let iss = issuer("https://idp.example.com");
        h.seeder.ensure_platform_admin(&p, &iss, "sub-admin").await;
        h.seeder.ensure_platform_admin(&p, &iss, "sub-admin").await;

        assert_eq!(h.grants.lock().unwrap().len(), 1, "idempotent: a second authentication must not duplicate the grant");
    }

    #[tokio::test]
    async fn an_existing_platform_admin_grant_is_left_untouched() {
        // Even if the grant was seeded some other way (e.g. an operator-run `psql` seed
        // ahead of Task 21b landing), `ensure_platform_admin` must not insert a second one.
        let h = seeder(&[BootstrapAdmin {
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
        h.grants.lock().unwrap().insert(pre_existing.id, pre_existing.clone());

        h.seeder.ensure_platform_admin(&p, &issuer("https://idp.example.com"), "sub-admin").await;

        let grants = h.grants.lock().unwrap();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants.get(&pre_existing.id), Some(&pre_existing));
    }

    #[tokio::test]
    async fn a_matching_issuer_with_a_different_subject_is_not_seeded() {
        let h = seeder(&[BootstrapAdmin {
            issuer: "https://idp.example.com".to_string(),
            subject: "sub-admin".to_string(),
        }]);
        h.seeder.ensure_platform_admin(&principal(1), &issuer("https://idp.example.com"), "sub-other").await;
        assert!(h.grants.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_matching_subject_with_a_different_issuer_is_not_seeded() {
        let h = seeder(&[BootstrapAdmin {
            issuer: "https://idp.example.com".to_string(),
            subject: "sub-admin".to_string(),
        }]);
        h.seeder.ensure_platform_admin(&principal(1), &issuer("https://other-idp.example.com"), "sub-admin").await;
        assert!(h.grants.lock().unwrap().is_empty());
    }

    #[test]
    fn an_unparseable_configured_issuer_is_skipped_not_a_construction_error() {
        // `IamConfig::validate` rejects this at boot in production; this proves the fallback
        // (skip + warn) doesn't panic when it's reached anyway (e.g. a hand-built config in a
        // test that bypasses `validate`).
        let h = seeder(&[BootstrapAdmin {
            issuer: "not-a-valid-issuer".to_string(),
            subject: "sub-admin".to_string(),
        }]);
        assert!(h.seeder.admins.is_empty());
    }

    /// A `RoleGrantStore` that fails on the FIRST write step. Deliberately errors on
    /// `grant_in` rather than on the audit write: `FakeUnitOfWork` has no real transaction
    /// (`fakes.rs:893-903` — the fakes ignore `tx` and mutate immediately), so a failure on
    /// the LAST step would leave the grant already in the map and could not prove rollback.
    /// True atomicity is proven against Postgres in `tests/authz_bootstrap_admin.rs`.
    #[derive(Default)]
    struct FailingGrants;

    #[async_trait::async_trait]
    impl RoleGrantStore for FailingGrants {
        async fn list_by_principal(&self, _p: &PrincipalId) -> Result<Vec<RoleGrant>, AuthzError> {
            Ok(Vec::new())
        }
        async fn grant_in(&self, _tx: &dyn paigasus_iam_core::Transaction, _g: &RoleGrant) -> Result<(), AuthzError> {
            Err(AuthzError::Backend(Box::new(std::io::Error::other("simulated mid-txn store failure"))))
        }
        async fn grant(&self, _g: &RoleGrant) -> Result<(), AuthzError> {
            unimplemented!("the seeder only uses grant_in")
        }
        async fn revoke(&self, _id: Uuid) -> Result<(), AuthzError> {
            unimplemented!("the seeder never revokes")
        }
        async fn revoke_in(&self, _tx: &dyn paigasus_iam_core::Transaction, _id: Uuid) -> Result<bool, AuthzError> {
            unimplemented!("the seeder never revokes")
        }
        async fn list_all(&self) -> Result<Vec<RoleGrant>, AuthzError> {
            Ok(Vec::new())
        }
        async fn find(&self, _id: Uuid) -> Result<Option<RoleGrant>, AuthzError> {
            unimplemented!("the seeder never looks up by id")
        }
    }

    fn admin_cfg() -> Vec<BootstrapAdmin> {
        vec![BootstrapAdmin {
            issuer: "https://idp.example.com".to_string(),
            subject: "sub-admin".to_string(),
        }]
    }

    /// Test 1 — the audit row is correct and, crucially, SELF-DESCRIBING. With
    /// `actor_prn: None` and `resource_prn` set to the SCOPE, `principal_prn` in `detail` is
    /// the only thing naming who actually became platform admin (SMA-468 D4).
    #[tokio::test]
    async fn the_seeded_grant_writes_a_self_describing_audit_row() {
        let h = seeder(&admin_cfg());
        let p = principal(1);
        h.seeder.ensure_platform_admin(&p, &issuer("https://idp.example.com"), "sub-admin").await;

        let entries = h.entries.lock().unwrap();
        assert_eq!(entries.len(), 1, "a seeded grant must write exactly one audit entry");
        let e = &entries[0];
        assert_eq!(e.action, "GrantRole", "SMA-468 D3: reuse the standard action so the row appears in the standard query");
        assert_eq!(e.actor_prn, None, "SMA-468 D2: no principal authorized this — configuration did");
        assert_eq!(e.outcome, AuditOutcome::Committed);
        assert_eq!(e.resource_prn.as_deref(), Some(root_prn().canonical().as_str()));
        assert_eq!(
            e.detail["principal_prn"],
            serde_json::json!(p.canonical()),
            "SMA-468 D4: with a null actor this is the ONLY field naming the grantee"
        );
        assert_eq!(e.detail["source"], serde_json::json!("bootstrap_admins"));
        assert_eq!(e.detail["issuer"], serde_json::json!("https://idp.example.com"));
        assert_eq!(e.detail["role_key"], serde_json::json!("platform_admin"));
    }

    /// Test 2 — the IdP `subject` must appear in NEITHER artifact (SMA-468 D4). `audit_log`
    /// is append-only and designed to outlive the rows it describes, so an external
    /// identifier written here cannot be removed under an erasure request. Asserted over the
    /// serialized forms so a nested placement cannot slip through.
    #[tokio::test]
    async fn neither_artifact_carries_the_idp_subject() {
        let h = seeder(&admin_cfg());
        h.seeder.ensure_platform_admin(&principal(1), &issuer("https://idp.example.com"), "sub-admin").await;

        let detail = h.entries.lock().unwrap()[0].detail.to_string();
        let payload = h.events.lock().unwrap()[0].payload.to_string();
        assert!(!detail.contains("sub-admin"), "the IdP subject must not reach audit_log: {detail}");
        assert!(!payload.contains("sub-admin"), "the IdP subject must not cross the outbox boundary: {payload}");
    }

    /// Test 3 — control flow: a failure on the FIRST write step must stop everything after
    /// it. This is what the in-memory fakes can honestly prove; see `FailingGrants`.
    #[tokio::test]
    async fn a_failed_grant_write_stops_the_event_the_audit_row_and_the_bump() {
        let bumps = FakePolicyGenBumper::default();
        let outbox = FakeOutbox::default();
        let events = outbox.0.clone();
        let audit = FakeAuditLog::default();
        let entries = audit.0.clone();
        let seeder = BootstrapAdminSeeder::new(BootstrapAdminSeederDeps {
            admins_config: admin_cfg(),
            grants: Arc::new(FailingGrants),
            uow: Arc::new(FakeUnitOfWork),
            outbox: Arc::new(outbox),
            audit: Arc::new(audit),
            gen_bumper: Arc::new(bumps.clone()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        });

        seeder.ensure_platform_admin(&principal(1), &issuer("https://idp.example.com"), "sub-admin").await;

        assert!(events.lock().unwrap().is_empty(), "no event may be enqueued once the grant write failed");
        assert!(entries.lock().unwrap().is_empty(), "no audit row may be written once the grant write failed");
        assert_eq!(bumps.calls(), 0, "SMA-468 D5: the post-commit bump must not run for a transaction that never committed");
    }

    /// Test 4 — the D5 regression guard. `grant_in` does NOT bump `policy_gen` (only the
    /// `RoleGrantStore::grant` wrapper this replaced did), so without an explicit post-commit
    /// bump a freshly seeded admin is denied until the snapshot's ~31s TTL backstop.
    #[tokio::test]
    async fn a_successful_seed_bumps_policy_gen_exactly_once() {
        let h = seeder(&admin_cfg());
        h.seeder.ensure_platform_admin(&principal(1), &issuer("https://idp.example.com"), "sub-admin").await;
        assert_eq!(h.bumps.calls(), 1, "SMA-468 D5: a seeded grant must invalidate the policy snapshot immediately");
    }

    /// Test 5 — idempotence now covers the new artifacts too, not just the grant row.
    #[tokio::test]
    async fn a_second_authentication_writes_no_second_audit_row_or_event() {
        let h = seeder(&admin_cfg());
        let p = principal(1);
        let iss = issuer("https://idp.example.com");
        h.seeder.ensure_platform_admin(&p, &iss, "sub-admin").await;
        h.seeder.ensure_platform_admin(&p, &iss, "sub-admin").await;

        assert_eq!(h.grants.lock().unwrap().len(), 1);
        assert_eq!(h.entries.lock().unwrap().len(), 1, "idempotent: the second authentication must not re-audit");
        assert_eq!(h.events.lock().unwrap().len(), 1, "idempotent: the second authentication must not re-emit");
        assert_eq!(h.bumps.calls(), 1, "idempotent: the second authentication must not re-bump");
    }

    /// Test 6 — the fast path is untouched: a non-configured identity produces nothing at all.
    #[tokio::test]
    async fn a_non_configured_identity_writes_no_audit_row_or_event() {
        let h = seeder(&[]);
        h.seeder.ensure_platform_admin(&principal(1), &issuer("https://idp.example.com"), "sub-1").await;
        assert!(h.grants.lock().unwrap().is_empty());
        assert!(h.entries.lock().unwrap().is_empty());
        assert!(h.events.lock().unwrap().is_empty());
        assert_eq!(h.bumps.calls(), 0);
    }
}
