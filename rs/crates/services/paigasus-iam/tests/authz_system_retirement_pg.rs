// SPDX-License-Identifier: Apache-2.0

//! Postgres-level behaviour of the SMA-481 retirement path: the FK ordering the schema forces,
//! the locks that make the checks trustworthy, and the deletes themselves.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note —
//! same gating pattern as `tests/roundtrip.rs`/`tests/authz_bootstrap.rs`.

mod support;

use chrono::Utc;
use paigasus_iam::adapters::authz::{Generations, GenerationsPolicyGenBumper};
use paigasus_iam::adapters::clock::SystemClock;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::entities::{policy, principal, role, role_grant};
use paigasus_iam::adapters::persistence::{PgAuditLog, PgOutbox, PgPolicyStore, PgRoleGrantStore, PgSystemRoleReconciler, PgSystemRowRetirer};
use paigasus_iam::application::authorize::Authorize;
use paigasus_iam::application::error::TenancyError;
use paigasus_iam::application::system_retirement::{SystemRetirementDeps, SystemRetirementService};
use paigasus_iam_core::authz::engine::{CompiledPolicies, PolicyEngine};
use paigasus_iam_core::authz::model::{ContextValue, EntitySlice, NodeKind, PolicyKind, ROOT_ENTITY, SliceEntity, root_prn};
use paigasus_iam_core::authz::reconcile::StarterPolicyOutcome;
use paigasus_iam_core::authz::roles::{FORBID_ARCHIVED_WRITES_ID, STARTER_POLICY_REVISION};
use paigasus_iam_core::{
    AccessRequest, Action, Authorizer, AuthzError, Decision, Effect, GrantScope, PolicyDocument, PolicyStore, PrincipalId, RequestContext, RetireOutcome, Role, RoleGrant, RoleGrantStore,
    SystemPolicyReconciler, SystemRoleReconciler, SystemRowRetirer,
};
use paigasus_kernel::Prn;
use sea_orm::{ActiveModelTrait, ActiveValue::NotSet, ConnectionTrait, Database, DatabaseConnection, DbBackend, EntityTrait, Set, Statement};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ContainerAsync;
use uuid::Uuid;

/// The revision a REAL orphan carries: strictly below this binary's own. An orphan's
/// `starter_revision` is, by construction, the revision of the last binary that still DEFINED
/// its id — dropping an id from `starter_policies()` changes the starter content hash, which
/// `starter_policy_content_is_pinned_to_the_declared_revision` forces you to answer with a
/// `STARTER_POLICY_REVISION` bump, and `reconcile_policies` only iterates code-defined docs, so
/// nothing ever restamps the orphan afterwards. `saturating_sub` only for a hypothetical
/// revision `0`; today it is `2`.
const ORPHAN_REVISION: u32 = STARTER_POLICY_REVISION.saturating_sub(1);

/// Seeds a system-owned template + role at a NON-code-defined id: the `policy` row first, then
/// the `role` row that references it — `fk_role_template` requires that order, the same order
/// retirement must undo in reverse. Direct SeaORM inserts on purpose: there is deliberately no
/// supported path that writes a `role` row for a key the code catalog does not define — that
/// absence IS the bug SMA-481 exists for.
///
/// Stamped at [`ORPHAN_REVISION`] — BELOW this binary's — because that is the only value a
/// naturally-orphaned row can hold. An earlier fixture stamped it at `STARTER_POLICY_REVISION`
/// to get past `retire`'s D11 guard; that modelled a state which cannot occur, and hid the fact
/// that the guard was counting the orphan itself and so refused every real orphan forever. The
/// evidence `retire` actually reads is the converged STARTER set — see [`converge_starter_set`],
/// which every test calling `retire` seeds through the real boot path.
async fn seed_orphan_chain(db: &DatabaseConnection, id: &str) {
    seed_orphan_chain_at(db, id, Some(ORPHAN_REVISION)).await;
}

/// [`seed_orphan_chain`] with the orphan's `starter_revision` forced — `None` models a
/// pre-m0010 row, which is just as realistic an orphan as an older-but-present revision.
async fn seed_orphan_chain_at(db: &DatabaseConnection, id: &str, revision: Option<u32>) {
    let now = Utc::now();
    policy::ActiveModel {
        policy_id: Set(id.to_string()),
        kind: Set("template".to_string()),
        source: Set("permit(principal == ?principal, action, resource in ?resource);".to_string()),
        description: Set(None),
        system: Set(true),
        created_at: Set(now),
        updated_at: Set(now),
        content_fingerprint: NotSet,
        starter_revision: Set(revision.map(|r| i32::try_from(r).expect("test revision fits i32"))),
    }
    .insert(db)
    .await
    .unwrap();
    role::ActiveModel {
        key: Set(id.to_string()),
        template_id: Set(id.to_string()),
        scope_kinds: Set(r#"["organization"]"#.to_string()),
        description: Set(None),
        system: Set(true),
        created_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

/// Seeds the REAL starter policy set + system roles through the REAL boot convergence path
/// (`bootstrap::reconcile_starter`, wired exactly as `tests/authz_bootstrap.rs` wires it), which
/// stamps every starter row at this binary's `STARTER_POLICY_REVISION`.
///
/// This — not the orphan's own revision — is what makes the fleet look converged to D11's guard.
/// Running the real path rather than hand-rolling rows means the fixture cannot drift from what
/// boot actually writes: if `reconcile_system` ever stopped stamping the revision, these tests
/// would go red rather than keep passing against a hand-built lie.
async fn converge_starter_set(db: &DatabaseConnection) {
    use paigasus_iam::application::bootstrap::{ReconcileStarterDeps, reconcile_starter};

    let gens = Generations::memory();
    reconcile_starter(&ReconcileStarterDeps {
        policies: Arc::new(PgPolicyStore::new(db.clone(), gens.clone())),
        roles: Arc::new(PgSystemRoleReconciler::new(db.clone())),
        audit: Arc::new(PgAuditLog::new(db.clone())),
        ids: KernelIdGenerator,
        clock: SystemClock,
    })
    .await
    .unwrap();
}

/// Seeds one grant of `role_key` at the synthetic Root scope per entry in `ids`, inserted in
/// the GIVEN order — the caller controls insertion order deliberately (fix round 1: a fixture
/// that happens to insert in ascending-id order makes an "ordered by id" assertion
/// self-satisfying, since a plain heap scan can return rows in insertion order by accident
/// whether or not `ORDER BY id` is actually in the query). Each id doubles as both the grant's
/// own `id` and its principal's `id`. Distinct PRINCIPALS (rather than distinct scopes) are
/// what makes each grant a genuinely separate row under `uq_role_grant_principal_role_scope`
/// (principal_id, role_key, scope_node_prn) — every grant here shares the same role and the
/// same Root scope on purpose.
async fn seed_grants(db: &DatabaseConnection, role_key: &str, ids: &[Uuid]) {
    let now = Utc::now();
    for &id in ids {
        principal::ActiveModel {
            id: Set(id),
            prn: Set(format!("prn:pgs:iam:::principal/{id}")),
            kind: Set("user".to_string()),
            status: Set("active".to_string()),
            created_at: Set(now),
            updated_at: Set(now),
        }
        .insert(db)
        .await
        .unwrap();

        role_grant::ActiveModel {
            id: Set(id),
            principal_id: Set(id),
            role_key: Set(role_key.to_string()),
            scope_kind: Set("root".to_string()),
            scope_node_prn: Set(GrantScope::Root.canonical_prn()),
            scope_org_id: Set(None),
            scope_team_id: Set(None),
            scope_project_id: Set(None),
            linked_policy_id: Set(format!("grant:{id}")),
            created_at: Set(now),
        }
        .insert(db)
        .await
        .unwrap();
    }
}

/// Seeds a bare system-owned `policy` row at `id` with `starter_revision` forced to `revision`
/// (`None` simulates a pre-m0010 row) — `min_starter_revision`'s fixture, deliberately without
/// the role-row half of `seed_orphan_chain`: this test only exercises the advisory revision
/// read, never a delete. Callers pass a CODE-DEFINED starter id when they want the row to be
/// seen at all: `min_starter_revision` reads `STARTER_POLICY_IDS`, not `system = true`.
async fn seed_system_policy_with_revision(db: &DatabaseConnection, id: &str, revision: Option<u32>) {
    let now = Utc::now();
    policy::ActiveModel {
        policy_id: Set(id.to_string()),
        kind: Set("static".to_string()),
        source: Set("forbid(principal, action, resource);".to_string()),
        description: Set(None),
        system: Set(true),
        created_at: Set(now),
        updated_at: Set(now),
        content_fingerprint: NotSet,
        starter_revision: Set(revision.map(|r| i32::try_from(r).expect("test revision fits i32"))),
    }
    .insert(db)
    .await
    .unwrap();
}

/// The static-policy acknowledgement path (D4), at the Postgres level. Every other test in this
/// file retires a TEMPLATE seeded by `seed_orphan_chain` and passes `ack = true`, so until now
/// `NeedsAcknowledgement` and the acknowledged static retirement were covered only by the
/// service's unit tests against fakes — and the static path is the DANGEROUS half of D4, the one
/// where deleting the row genuinely changes decisions fleet-wide rather than removing an inert
/// template.
///
/// It also exercises a distinct row shape against the real adapter: a `policy` row with no
/// `role` row, so `lock_role_in` returns `None` and `delete_role_in` is never called. A fake
/// cannot prove that shape survives the FKs.
#[tokio::test]
async fn a_static_orphan_needs_acknowledgement_at_the_postgres_level_then_retires_without_a_role() {
    let Some((_c, db)) = support::start_migrated_postgres().await else { return };
    converge_starter_set(&db).await;
    seed_system_policy_with_revision(&db, "legacy_forbid", Some(ORPHAN_REVISION)).await;

    // Unacknowledged: refuses, hands back the content that would be destroyed, writes nothing.
    match retire(&db, "legacy_forbid", false).await.unwrap() {
        RetireOutcome::NeedsAcknowledgement { policy_id, kind, source, .. } => {
            assert_eq!(policy_id, "legacy_forbid");
            assert_eq!(kind, PolicyKind::Static, "a static row must not be mistaken for a template");
            assert_eq!(source, "forbid(principal, action, resource);", "the refusal must preview what would be lost");
        }
        other => panic!("a static policy without acknowledgement must refuse, got {other:?}"),
    }
    assert!(policy_row(&db, "legacy_forbid").await.is_some(), "an unacknowledged refusal must delete nothing");

    // Acknowledged: retires, and reports that no role row was involved.
    let outcome = retire(&db, "legacy_forbid", true).await.unwrap();
    assert_eq!(
        outcome,
        RetireOutcome::Retired {
            policy_id: "legacy_forbid".to_string(),
            kind: PolicyKind::Static,
            role_deleted: false,
        },
        "a static orphan has no role row, so role_deleted must be false"
    );
    assert!(policy_row(&db, "legacy_forbid").await.is_none());
    assert!(role_row(&db, "legacy_forbid").await.is_none(), "no role row should ever have existed");
}

#[tokio::test]
async fn the_fk_ordering_is_real_and_the_retirer_respects_it() {
    let Some((_c, db)) = support::start_migrated_postgres().await else { return };
    seed_orphan_chain(&db, "legacy_auditor").await;
    let retirer = PgSystemRowRetirer::new(db.clone());

    // Deleting the policy while the role row still references it must fail: fk_role_template.
    let tx = retirer.begin_retirement(Duration::from_secs(5)).await.unwrap();
    retirer
        .delete_policy_in(&*tx, "legacy_auditor")
        .await
        .expect_err("fk_role_template must block a policy delete while its role row survives");
    drop(tx); // no commit -> rollback

    // role first, then policy — the only order the schema permits.
    let tx = retirer.begin_retirement(Duration::from_secs(5)).await.unwrap();
    assert!(retirer.delete_role_in(&*tx, "legacy_auditor").await.unwrap());
    assert!(retirer.delete_policy_in(&*tx, "legacy_auditor").await.unwrap());
    tx.commit().await.unwrap();

    // The brief's snippet nests `retirer.begin_retirement(..).await.unwrap()` directly inside
    // the `lock_policy_in(&*.., ..)` argument list, which does not compile: the `Box<dyn
    // Transaction>` temporary is only kept alive for the enclosing STATEMENT, but `&*tx` needs
    // to borrow through it for the `.await` that follows within the same expression — a
    // temporary dropped while still borrowed. Binding it to a local first (as every other test
    // in this file already does with `tx`) gives the temporary a place to live for the borrow's
    // duration.
    let verify_tx = retirer.begin_retirement(Duration::from_secs(5)).await.unwrap();
    assert!(retirer.lock_policy_in(&*verify_tx, "legacy_auditor").await.unwrap().is_none());
}

/// Smoke-checks `lock_role_in`'s two read outcomes against a real row: it reads the right row
/// (`key`/`system` round-trip) and correctly reports absence once the row is gone. This does
/// **not** prove the row is actually locked `FOR UPDATE` — nothing here would catch a
/// regression that dropped `.lock_exclusive()` from the query, since a single connection can't
/// observe its own lock. Proving the lock itself holds needs two connections racing on the same
/// key, which is Task 9's `a_concurrent_grant_blocks_then_reports_unknown_role`.
#[tokio::test]
async fn lock_role_in_reads_the_row_and_then_reports_its_absence() {
    let Some((_c, db)) = support::start_migrated_postgres().await else { return };
    seed_orphan_chain(&db, "legacy_auditor").await;
    let retirer = PgSystemRowRetirer::new(db.clone());

    let tx = retirer.begin_retirement(Duration::from_secs(5)).await.unwrap();
    let found = retirer.lock_role_in(&*tx, "legacy_auditor").await.unwrap().expect("the seeded role row must be found");
    assert_eq!(found.key, "legacy_auditor");
    assert!(found.system, "the seeded role row is system-owned");

    assert!(retirer.delete_role_in(&*tx, "legacy_auditor").await.unwrap());
    assert!(retirer.lock_role_in(&*tx, "legacy_auditor").await.unwrap().is_none(), "the same transaction must see its own delete");
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn surviving_grants_are_capped_and_report_the_true_total() {
    let Some((_c, db)) = support::start_migrated_postgres().await else { return };
    seed_orphan_chain(&db, "legacy_auditor").await;
    // Deliberately inserted OUT of ascending-id order (5, 2, 4, 1, 3): if this seeded 1..=5 in
    // order, sorting the returned page and comparing it to itself would pass whether or not
    // `.order_by_asc(role_grant::Column::Id)` is even in the query — a heap scan can return rows
    // in insertion order by accident. Shuffling the insertion order is what makes the assertion
    // below actually exercise the ORDER BY clause: only a real ascending sort returns the two
    // SMALLEST ids (1, 2) first, not the first two inserted (5, 2).
    let ids = [Uuid::from_u128(5), Uuid::from_u128(2), Uuid::from_u128(4), Uuid::from_u128(1), Uuid::from_u128(3)];
    seed_grants(&db, "legacy_auditor", &ids).await;
    let retirer = PgSystemRowRetirer::new(db.clone());

    let tx = retirer.begin_retirement(Duration::from_secs(5)).await.unwrap();
    let survivors = retirer.surviving_grants_in(&*tx, "legacy_auditor", 2).await.unwrap();
    assert_eq!(survivors.grants.len(), 2, "the page is capped");
    assert_eq!(survivors.total, 5, "the total is the truth, not the page size");
    assert!(survivors.truncated(2));

    let got: Vec<String> = survivors.grants.iter().map(|g| g.id.clone()).collect();
    let want: Vec<String> = [Uuid::from_u128(1), Uuid::from_u128(2)].into_iter().map(|u| u.to_string()).collect();
    assert_eq!(got, want, "ordered by id ascending — the two SMALLEST ids, not the first two inserted");
}

#[tokio::test]
async fn min_starter_revision_reports_null_as_none() {
    let Some((_c, db)) = support::start_migrated_postgres().await else { return };
    // A pre-m0010 STARTER row: a code-defined id with a NULL starter_revision. The id must be
    // a real starter id — a non-starter row is invisible to this read, so seeding one would
    // make the assertion pass for the empty-set reason instead of the NULL reason.
    seed_system_policy_with_revision(&db, FORBID_ARCHIVED_WRITES_ID, None).await;
    let retirer = PgSystemRowRetirer::new(db.clone());
    assert_eq!(retirer.min_starter_revision().await.unwrap(), None, "a NULL revision is unprovable, not zero");
}

/// The empty set must NOT read as "converged" — an unseeded database (or one whose boot
/// convergence never ran) is the absence of evidence, and `min()` over nothing is `None`
/// deliberately, not incidentally. Without this, a database with no starter rows would silently
/// permit every retirement.
#[tokio::test]
async fn min_starter_revision_reports_an_unseeded_database_as_none() {
    let Some((_c, db)) = support::start_migrated_postgres().await else { return };
    let retirer = PgSystemRowRetirer::new(db.clone());
    assert_eq!(retirer.min_starter_revision().await.unwrap(), None, "no starter rows at all is unprovable, not converged");
}

/// THE regression guard on the query's filter. `min_starter_revision` must read only the
/// CODE-DEFINED starter ids: filtering on `system = true` instead also matches the orphan being
/// retired, whose revision is always older by construction — which made the D11 guard refuse
/// every real orphan, forever. Restoring `.filter(policy::Column::System.eq(true))` turns the
/// `STARTER_POLICY_REVISION` below into `ORPHAN_REVISION` and reds this.
#[tokio::test]
async fn min_starter_revision_ignores_the_orphan_and_reads_only_the_starter_set() {
    let Some((_c, db)) = support::start_migrated_postgres().await else { return };
    converge_starter_set(&db).await;
    seed_orphan_chain(&db, "legacy_auditor").await;
    seed_orphan_chain_at(&db, "legacy_null", None).await;

    let retirer = PgSystemRowRetirer::new(db.clone());
    assert_eq!(
        retirer.min_starter_revision().await.unwrap(),
        Some(STARTER_POLICY_REVISION),
        "two system-owned orphans (one older, one NULL) must not drag the converged starter set's minimum down"
    );
}

/// Seeds a system-owned `policy` row with `starter_revision` forced to a value
/// `Option<u32>`-typed helpers (`seed_system_policy_with_revision`) can't represent on purpose
/// — a raw negative `i32`, only reachable via a hand edit (every value this service itself
/// writes is cast up from a `u32`).
async fn seed_system_policy_with_raw_revision(db: &DatabaseConnection, id: &str, revision: i32) {
    let now = Utc::now();
    policy::ActiveModel {
        policy_id: Set(id.to_string()),
        kind: Set("static".to_string()),
        source: Set("forbid(principal, action, resource);".to_string()),
        description: Set(None),
        system: Set(true),
        created_at: Set(now),
        updated_at: Set(now),
        content_fingerprint: NotSet,
        starter_revision: Set(Some(revision)),
    }
    .insert(db)
    .await
    .unwrap();
}

/// Fix round 1: `min_starter_revision` used to coerce a negative `starter_revision` to `0`
/// (`u32::try_from(r).unwrap_or(0)`) instead of surfacing it. Reading it as `0` — "oldest
/// possible" — would defer retirement behind a row that is actually just corrupt, not
/// genuinely old, so a negative value must error loudly instead of guessing a default.
#[tokio::test]
async fn min_starter_revision_rejects_a_negative_value_instead_of_coercing_it() {
    let Some((_c, db)) = support::start_migrated_postgres().await else { return };
    // A code-defined starter id, for the same reason `min_starter_revision_reports_null_as_none`
    // needs one: a non-starter row is invisible to the read and would prove nothing.
    seed_system_policy_with_raw_revision(&db, FORBID_ARCHIVED_WRITES_ID, -1).await;
    let retirer = PgSystemRowRetirer::new(db.clone());
    let err = retirer.min_starter_revision().await.expect_err("a negative starter_revision must surface as an error, not coerce to 0");
    assert!(matches!(err, AuthzError::Backend(_)), "unexpected error variant: {err:?}");
}

// ---------------------------------------------------------------------------------------------
// Task 9: the decision change, the locks, and the fleet-skew failure mode.
//
// Everything above proves the retirer's own row-level mechanics. These prove the thing the
// issue is actually about: a retired role's grant stops conferring permission, the row locks
// genuinely block a racing writer (not merely "read the right row"), and the one documented
// gap (D11 fleet skew) stays pinned rather than silently regressing.
// ---------------------------------------------------------------------------------------------

/// A Root-authorized actor PRN for [`retire`] — mirrors `system_retirement.rs`'s own unit-test
/// `actor()` fixture.
fn actor() -> Prn {
    Prn::build("iam", "", None, "principal", Uuid::from_u128(1)).unwrap()
}

/// A second, wholly independent connection (own pool, own physical session) to the SAME
/// container `support::start_migrated_postgres` already stood up — needed for every
/// concurrency test below, which must race a REAL second session against a lock the first
/// session holds open. Mirrors `tests/outbox_retention_concurrency_pg.rs`'s own "own pool, own
/// physical session, not one borrowed from db's own pool" hold-open technique.
async fn second_connection(container: &ContainerAsync<Postgres>) -> DatabaseConnection {
    let port = container.get_host_port_ipv4(5432).await.unwrap();
    Database::connect(format!("postgres://postgres:postgres@127.0.0.1:{port}/postgres")).await.unwrap()
}

/// Seeds ONE grant of `role_key` at `scope` through the real `PgRoleGrantStore::grant` (not a
/// raw insert): reusing the actual store means the scope-column mapping isn't reinvented here,
/// and the returned `RoleGrant` is byte-for-byte what a caller can feed straight into
/// `PolicyEngine::compile` without re-reading the row back. Requires the grant's `role_key` to
/// already have a `role` row (`fk_role_grant_role`) — callers seed that first, typically via
/// `seed_orphan_chain`.
async fn seed_grant(db: &DatabaseConnection, role_key: &str, scope: GrantScope) -> RoleGrant {
    let principal_id = Uuid::from_u128(0xA11CE);
    let now = Utc::now();
    principal::ActiveModel {
        id: Set(principal_id),
        prn: Set(format!("prn:pgs:iam:::principal/{principal_id}")),
        kind: Set("user".to_string()),
        status: Set("active".to_string()),
        created_at: Set(now),
        updated_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();

    let grant_id = Uuid::from_u128(0xBEEF);
    let grant = RoleGrant {
        id: grant_id,
        principal: PrincipalId::from_prn(Prn::build("iam", "", None, "principal", principal_id).unwrap()),
        role_key: role_key.to_string(),
        scope,
        linked_policy_id: format!("grant:{grant_id}"),
        created_at: now,
    };
    PgRoleGrantStore::new(db.clone(), Generations::memory()).grant(&grant).await.unwrap();
    grant
}

/// Revokes a grant by id through the real `PgRoleGrantStore::revoke` — the runbook's own
/// remedy for a `Blocked` retirement.
async fn revoke_grant(db: &DatabaseConnection, id: Uuid) {
    PgRoleGrantStore::new(db.clone(), Generations::memory()).revoke(id).await.unwrap();
}

/// `seed_orphan_chain` immediately followed by removing its `role` row — isolating that LOCKING
/// THE POLICY ROW ALONE (not a role-row lock) is what must block a concurrent role insert
/// referencing it (`fk_role_template`), the reason `SystemRetirementService::retire` locks the
/// policy row, the FK PARENT, first (D6/§3.2). Reuses `seed_orphan_chain` rather than
/// re-deriving the policy row's shape.
async fn seed_policy_only(db: &DatabaseConnection, id: &str) {
    seed_orphan_chain(db, id).await;
    role::Entity::delete_by_id(id.to_string()).exec(db).await.unwrap();
}

/// Every stored [`PolicyDocument`] via the real `PgPolicyStore::list_all` — what a live compile
/// would actually read.
async fn load_all_policies(db: &DatabaseConnection) -> Vec<PolicyDocument> {
    PgPolicyStore::new(db.clone(), Generations::memory()).list_all().await.unwrap()
}

/// The stored `policy` row at `id`, if any.
async fn policy_row(db: &DatabaseConnection, id: &str) -> Option<policy::Model> {
    policy::Entity::find_by_id(id.to_string()).one(db).await.unwrap()
}

/// The stored `role` row at `key`, if any.
async fn role_row(db: &DatabaseConnection, key: &str) -> Option<role::Model> {
    role::Entity::find_by_id(key.to_string()).one(db).await.unwrap()
}

/// The `PolicyDocument` an older binary's code catalog would still hand `reconcile_system` for
/// `id` — the exact template shape `seed_orphan_chain` gives its policy row, so the fleet-skew
/// test reconciles the identical document a retired row used to hold.
fn orphan_doc(id: &str) -> PolicyDocument {
    let now = Utc::now();
    PolicyDocument {
        policy_id: id.to_string(),
        kind: PolicyKind::Template,
        source: "permit(principal == ?principal, action, resource in ?resource);".to_string(),
        description: String::new(),
        system: true,
        created_at: now,
        updated_at: now,
    }
}

/// A trivial `Authorizer` that unconditionally allows every request. `system_retirement.rs`'s
/// own unit tests already cover the Root-only enforcement itself (`FakeAuthorizer`, `#[cfg(test)]`
/// there and unreachable from an integration-test binary); these Postgres tests need only a
/// caller who passes that check so they can exercise the actual row-level behaviour.
struct AllowAllAuthorizer;

#[async_trait::async_trait]
impl Authorizer for AllowAllAuthorizer {
    async fn is_authorized(&self, _req: &AccessRequest) -> Result<Decision, AuthzError> {
        Ok(Decision {
            effect: Effect::Allow,
            determining_policies: Vec::new(),
        })
    }
}

/// Drives one `SystemRetirementService::retire` call over REAL Postgres-backed adapters
/// (`PgSystemRowRetirer`/`PgOutbox`/`PgAuditLog`/`GenerationsPolicyGenBumper`), the same wiring
/// `AppState::new` uses (`adapters/http/mod.rs`) — except `authorize` is an
/// [`AllowAllAuthorizer`].
async fn retire(db: &DatabaseConnection, id: &str, ack: bool) -> Result<RetireOutcome, TenancyError> {
    let svc = SystemRetirementService::new(SystemRetirementDeps {
        retirer: Arc::new(PgSystemRowRetirer::new(db.clone())),
        outbox: Arc::new(PgOutbox::new()),
        audit: Arc::new(PgAuditLog::new(db.clone())),
        gen_bumper: Arc::new(GenerationsPolicyGenBumper::new(Generations::memory())),
        ids: Arc::new(KernelIdGenerator),
        clock: Arc::new(SystemClock),
        authorize: Authorize::new(Arc::new(AllowAllAuthorizer)),
    });
    svc.retire(&actor(), id, ack).await
}

/// Builds the minimal `EntitySlice` (`Root` + `grant`'s principal) and drives one `action`
/// decision at `Root` for `grant`'s principal against `policies` — copies the `Request`/
/// `EntitySlice` construction `authz::engine`'s own tests use (its `slice`/`principal_prn`
/// helpers), simplified to Root scope since `Action::ListAuditLog`'s resource IS Root, so no
/// org/team/project ancestor chain is needed.
fn decide(policies: &CompiledPolicies, grant: &RoleGrant, action: Action) -> Decision {
    let principal_prn = grant.principal.prn().clone();
    let principal_uid = paigasus_kernel::to_cedar_uid(&principal_prn);

    let slice = EntitySlice {
        entities: vec![
            SliceEntity {
                uid: (ROOT_ENTITY.0.to_string(), ROOT_ENTITY.1.to_string()),
                parents: vec![],
                attrs: BTreeMap::new(),
            },
            SliceEntity {
                uid: (principal_uid.entity_type, principal_uid.entity_id),
                parents: vec![],
                attrs: BTreeMap::from([
                    ("kind".to_string(), ContextValue::Str("user".to_string())),
                    ("status".to_string(), ContextValue::Str("active".to_string())),
                ]),
            },
        ],
    };
    let req = AccessRequest {
        principal: principal_prn,
        action,
        resource: root_prn(),
        context: RequestContext::empty(),
    };
    PolicyEngine::decide(&policies.policy_set, &slice, &req)
}

/// THE test. Everything else in this file checks rows; this checks a DECISION — the one thing
/// that actually demonstrates the bug in SMA-481 is fixed. The "before" assertion is not
/// optional: without it, a fixture that never granted anything in the first place would make
/// the "after" assertion pass vacuously, proving nothing.
#[tokio::test]
async fn a_retired_role_s_grant_stops_conferring_permission() {
    let Some((_c, db)) = support::start_migrated_postgres().await else { return };
    converge_starter_set(&db).await;
    seed_orphan_chain(&db, "legacy_auditor").await;
    let grant = seed_grant(&db, "legacy_auditor", GrantScope::Root).await;

    let before = PolicyEngine::compile(&load_all_policies(&db).await, std::slice::from_ref(&grant)).expect("compile succeeds");
    assert_eq!(
        decide(&before, &grant, Action::ListAuditLog).effect,
        Effect::Allow,
        "the fixture must actually grant, or the after-assertion proves nothing"
    );

    // Zero surviving grants, so the template retirement below is not blocked (D4/D5) — the
    // runbook's own remedy, applied here so the retirement itself can succeed.
    revoke_grant(&db, grant.id).await;
    let outcome = retire(&db, "legacy_auditor", true).await.expect("no grants survive after the revoke above");
    assert!(outcome.is_retired(), "expected the template to actually retire once its grant is revoked, got {outcome:?}");

    // The SAME in-memory `grant` value, recompiled against the policies AFTER retirement: the
    // template row is gone, so `PolicyEngine::compile` silently skips linking it (its own doc:
    // "a grant naming an absent template is silently skipped") — that is the actual mechanism
    // that stops the grant from conferring permission, not merely that the grant row itself was
    // revoked.
    let after = PolicyEngine::compile(&load_all_policies(&db).await, std::slice::from_ref(&grant)).expect("compile succeeds");
    assert_eq!(decide(&after, &grant, Action::ListAuditLog).effect, Effect::Deny, "the template is gone, so the grant links nothing");
}

/// D6, both halves. The lock blocks the concurrent insert AND the caller that loses the race
/// gets a mapped error (`AuthzError::UnknownRole`), not a raw `Backend`/500. Asserting only the
/// blocking would go green against an unmapped-error regression.
#[tokio::test]
async fn a_concurrent_grant_blocks_then_reports_unknown_role() {
    let Some((c, db_a)) = support::start_migrated_postgres().await else { return };
    seed_orphan_chain(&db_a, "legacy_auditor").await;
    let db_b = second_connection(&c).await;

    let retirer = PgSystemRowRetirer::new(db_a.clone());
    let tx = retirer.begin_retirement(Duration::from_secs(5)).await.unwrap();
    // Locks the role row FOR UPDATE — D6's own mechanism, exercised directly (rather than
    // through the full `retire()` service) so this test can race a SECOND connection against
    // the held lock before the transaction ever commits.
    retirer.lock_role_in(&*tx, "legacy_auditor").await.unwrap().expect("seeded role row must be found");

    // A concurrent grant from a SEPARATE connection/session — simulating a replica on an older
    // binary that still defines "legacy_auditor" and would happily grant it.
    let principal_id = Uuid::from_u128(0xC0FFEE);
    principal::ActiveModel {
        id: Set(principal_id),
        prn: Set(format!("prn:pgs:iam:::principal/{principal_id}")),
        kind: Set("user".to_string()),
        status: Set("active".to_string()),
        created_at: Set(Utc::now()),
        updated_at: Set(Utc::now()),
    }
    .insert(&db_b)
    .await
    .unwrap();
    let grant_id = Uuid::from_u128(0xF00D);
    let racing_grant = RoleGrant {
        id: grant_id,
        principal: PrincipalId::from_prn(Prn::build("iam", "", None, "principal", principal_id).unwrap()),
        role_key: "legacy_auditor".to_string(),
        scope: GrantScope::Root,
        linked_policy_id: format!("grant:{grant_id}"),
        created_at: Utc::now(),
    };
    let grant_store = PgRoleGrantStore::new(db_b.clone(), Generations::memory());
    let mut handle = tokio::spawn(async move { grant_store.grant(&racing_grant).await });

    // Prove the grant actually BLOCKS: actively polling it (not merely sleeping then checking
    // `is_finished`) within a bounded window must itself time out while the role row's lock is
    // still held.
    tokio::time::timeout(Duration::from_millis(500), &mut handle)
        .await
        .expect_err("the grant must block behind the role row's FOR UPDATE lock, not complete while it is held");

    // Release the lock: delete the role (and its parent policy) and commit.
    assert!(retirer.delete_role_in(&*tx, "legacy_auditor").await.unwrap());
    assert!(retirer.delete_policy_in(&*tx, "legacy_auditor").await.unwrap());
    tx.commit().await.unwrap();

    let result = tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("the grant never resumed after the retirement transaction committed — the FOR UPDATE lock is not being released")
        .unwrap();
    match result {
        Err(AuthzError::UnknownRole(role_key)) => assert_eq!(role_key, "legacy_auditor"),
        other => panic!("expected UnknownRole once the role row is gone, got {other:?}"),
    }
}

/// The policy row is the FK PARENT of the role row, so it is locked first — otherwise an older
/// replica's `reconcile_role` INSERT slips in when no role row exists to lock, and the policy
/// delete fails on `fk_role_template` with an unmapped error.
#[tokio::test]
async fn locking_the_policy_row_blocks_a_concurrent_role_insert() {
    let Some((c, db_a)) = support::start_migrated_postgres().await else { return };
    // Only the policy row: no role row exists yet, so `lock_role_in` would find nothing to
    // lock — isolating that the POLICY lock alone is what must block the concurrent insert.
    seed_policy_only(&db_a, "legacy_auditor").await;
    let db_b = second_connection(&c).await;

    let retirer = PgSystemRowRetirer::new(db_a.clone());
    let tx = retirer.begin_retirement(Duration::from_secs(5)).await.unwrap();
    retirer.lock_policy_in(&*tx, "legacy_auditor").await.unwrap().expect("seeded policy row must be found");

    // Simulate an older replica's boot-time `reconcile_role`, inserting the role row this
    // policy's id would back (`template_id == key`, the linkage convention `authz::engine`
    // documents).
    let role_def = Role {
        key: "legacy_auditor".to_string(),
        template_id: "legacy_auditor".to_string(),
        scope_kinds: vec![NodeKind::Organization],
        description: String::new(),
        system: true,
    };
    let reconciler = PgSystemRoleReconciler::new(db_b.clone());
    let mut handle = tokio::spawn(async move { reconciler.reconcile_role(&role_def).await });

    tokio::time::timeout(Duration::from_millis(500), &mut handle)
        .await
        .expect_err("the role INSERT must block behind the policy row's FOR UPDATE lock — this is why retirement locks the policy row, the FK parent, first");

    assert!(retirer.delete_policy_in(&*tx, "legacy_auditor").await.unwrap());
    tx.commit().await.unwrap();

    let result = tokio::time::timeout(Duration::from_secs(10), handle)
        .await
        .expect("the role insert never resumed after the policy row's lock was released")
        .unwrap();
    assert!(result.is_err(), "with the policy row gone, the blocked role insert must fail on fk_role_template, not silently succeed");
}

/// `lock_timeout` bounds the wait rather than hanging: this runs on an operator's request.
#[tokio::test]
async fn a_contended_row_times_out_rather_than_hanging() {
    let Some((c, db_a)) = support::start_migrated_postgres().await else { return };
    seed_orphan_chain(&db_a, "legacy_auditor").await;
    let db_b = second_connection(&c).await;

    // Connection A holds the policy row locked, uncommitted, for the rest of this test.
    let retirer_a = PgSystemRowRetirer::new(db_a.clone());
    let holder = retirer_a.begin_retirement(Duration::from_secs(30)).await.unwrap();
    retirer_a.lock_policy_in(&*holder, "legacy_auditor").await.unwrap().expect("seeded policy row must be found");

    // Connection B asks for the SAME row under a SHORT lock_timeout: an operator-triggered
    // request must fail with an error rather than hang indefinitely. `tokio::time::timeout`
    // around the call turns a regression that dropped the `SET LOCAL lock_timeout` guard into a
    // clean, attributable failure instead of a hung test suite (mirrors
    // `tests/outbox_retention_concurrency_pg.rs`'s own idiom).
    let retirer_b = PgSystemRowRetirer::new(db_b.clone());
    let contended = retirer_b.begin_retirement(Duration::from_millis(300)).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), retirer_b.lock_policy_in(&*contended, "legacy_auditor"))
        .await
        .expect("lock_policy_in hung past 10s instead of surfacing its own lock_timeout");
    let err = result.expect_err("a row locked by another session must surface lock_timeout as an error, not succeed");
    assert!(matches!(err, AuthzError::Backend(_)), "unexpected error variant: {err:?}");

    drop(holder); // never committed -> rolls back, releasing the lock.
}

/// D11's known failure mode, pinned deliberately: it is documented, accepted behaviour (the
/// runbook tells operators to wait for fleet convergence and retry), NOT a bug to fix here.
/// `classify_starter_policy` classifies an absent row as `Absent` BEFORE the revision guard ever
/// runs (`authz::reconcile`'s own module doc, step 1 of its classification order), so a replica
/// whose code catalog still defines a retired id re-seeds the deleted row unconditionally — no
/// in-band mechanism can bind a binary older than the mechanism itself. Pinning this is what
/// stops it from being silently (re)discovered in production.
#[tokio::test]
async fn a_binary_that_still_defines_the_id_re_seeds_it_after_retirement() {
    let Some((_c, db)) = support::start_migrated_postgres().await else { return };
    converge_starter_set(&db).await;
    seed_orphan_chain(&db, "legacy_auditor").await;
    let first = retire(&db, "legacy_auditor", true).await.unwrap();
    assert!(first.is_retired(), "the setup retirement itself must actually retire, got {first:?}");
    assert!(policy_row(&db, "legacy_auditor").await.is_none(), "retirement must have removed the row");

    // Simulate the older replica: reconcile the id as though its own code catalog still
    // defines it.
    let reconciler = PgPolicyStore::new(db.clone(), Generations::memory());
    let outcome = reconciler.reconcile_system(&orphan_doc("legacy_auditor"), STARTER_POLICY_REVISION).await.unwrap();
    assert_eq!(outcome, StarterPolicyOutcome::Absent, "classify_starter_policy must see an absent row and reseed unconditionally");

    assert!(
        policy_row(&db, "legacy_auditor").await.is_some(),
        "Absent is classified BEFORE the revision guard, so an older binary re-seeds unconditionally — the documented D11 failure mode, pinned here so it cannot regress silently into a surprise"
    );
}

/// Retirement is not an idempotent DELETE: a second call means the operator's model of the
/// system is wrong and they should be told, not silently congratulated.
#[tokio::test]
async fn a_repeated_retirement_is_not_found_and_writes_nothing() {
    let Some((_c, db)) = support::start_migrated_postgres().await else { return };
    converge_starter_set(&db).await;
    // A second orphan that is NEVER retired here: it pins that retirement touches ONLY the id
    // it was given. (It used to exist to keep `min_starter_revision` non-vacuous after
    // "legacy_auditor" was gone — that was an artifact of the old `system = true` filter; the
    // converged starter set above is now what supplies the convergence evidence.)
    seed_orphan_chain(&db, "anchor_role").await;
    seed_orphan_chain(&db, "legacy_auditor").await;

    let first = retire(&db, "legacy_auditor", true).await.expect("first retirement succeeds");
    assert!(first.is_retired(), "the first retirement must actually retire, got {first:?}");
    assert!(policy_row(&db, "legacy_auditor").await.is_none());

    let err = retire(&db, "legacy_auditor", true).await.expect_err("a second retirement of an already-gone id must not be idempotent");
    assert!(matches!(err, TenancyError::NotFound), "retirement is not an idempotent DELETE: got {err:?}");
    assert!(policy_row(&db, "anchor_role").await.is_some(), "the untouched anchor row must survive");
}

/// End to end, the way the runbook reads: blocked while a grant survives, revoke, then retire
/// succeeds and both rows are gone.
#[tokio::test]
async fn grant_then_retire_then_revoke_then_retire() {
    let Some((_c, db)) = support::start_migrated_postgres().await else { return };
    converge_starter_set(&db).await;
    seed_orphan_chain(&db, "legacy_auditor").await;
    let grant = seed_grant(&db, "legacy_auditor", GrantScope::Root).await;

    // A surviving grant blocks retirement — writes nothing (D4/D5).
    match retire(&db, "legacy_auditor", true).await.unwrap() {
        RetireOutcome::Blocked { role_key, total, .. } => {
            assert_eq!(role_key, "legacy_auditor");
            assert_eq!(total, 1);
        }
        other => panic!("expected Blocked while the grant survives, got {other:?}"),
    }
    assert!(policy_row(&db, "legacy_auditor").await.is_some(), "a blocked retirement must write nothing");

    // The runbook's remedy: revoke the surviving grant, then retry.
    revoke_grant(&db, grant.id).await;

    let outcome = retire(&db, "legacy_auditor", true).await.expect("retirement succeeds once no grants survive");
    assert!(outcome.is_retired());
    assert!(policy_row(&db, "legacy_auditor").await.is_none(), "the policy row must be gone");
    assert!(role_row(&db, "legacy_auditor").await.is_none(), "the role row must be gone");
}

/// THE regression test for the bug this file's fixtures used to hide: a REALISTICALLY stamped
/// orphan — one whose `starter_revision` is below the running binary's, or NULL, the only two
/// values a naturally-orphaned row can hold — must retire successfully against a converged
/// starter set. The old `min_starter_revision` filtered `system = true`, which matched the
/// orphan itself, so D11's guard measured the very row being retired and refused every real
/// orphan forever. Restore that filter and both cases below fail with `FleetNotConverged`.
///
/// The old fixture could not catch this: it stamped the orphan at `STARTER_POLICY_REVISION`, a
/// value no naturally-orphaned row can hold, which is exactly what made the broken guard pass.
#[tokio::test]
async fn a_realistically_stamped_orphan_retires_against_a_converged_starter_set() {
    let Some((_c, db)) = support::start_migrated_postgres().await else { return };
    converge_starter_set(&db).await;
    // An orphan left behind by an older binary (revision below this one's) …
    seed_orphan_chain_at(&db, "legacy_auditor", Some(ORPHAN_REVISION)).await;
    // … and a pre-m0010 orphan, which carries no revision at all.
    seed_orphan_chain_at(&db, "legacy_prehistoric", None).await;

    let outcome = retire(&db, "legacy_auditor", true).await.expect("an orphan below the current revision must still be retirable");
    assert!(outcome.is_retired(), "expected the older-revision orphan to retire, got {outcome:?}");
    assert!(policy_row(&db, "legacy_auditor").await.is_none());

    let outcome = retire(&db, "legacy_prehistoric", true).await.expect("a NULL-revision (pre-m0010) orphan must still be retirable");
    assert!(outcome.is_retired(), "expected the NULL-revision orphan to retire, got {outcome:?}");
    assert!(policy_row(&db, "legacy_prehistoric").await.is_none());
}

/// The other half of the guard, which must NOT be lost while fixing the above: a genuinely
/// unconverged fleet still refuses. Here the skew is where D11 actually looks for it — a STARTER
/// policy row (a still-code-defined id) last written by an older binary — so the retirement of a
/// perfectly ordinary orphan is deferred.
#[tokio::test]
async fn a_starter_row_below_the_current_revision_still_refuses() {
    let Some((_c, db)) = support::start_migrated_postgres().await else { return };
    converge_starter_set(&db).await;
    seed_orphan_chain(&db, "legacy_auditor").await;

    // Simulate an older replica having last written one starter row.
    db.execute(Statement::from_string(
        DbBackend::Postgres,
        format!(r#"UPDATE "policy" SET starter_revision = {ORPHAN_REVISION} WHERE policy_id = '{FORBID_ARCHIVED_WRITES_ID}'"#),
    ))
    .await
    .unwrap();

    let err = retire(&db, "legacy_auditor", true)
        .await
        .expect_err("a starter row below the current revision must defer the retirement");
    assert!(matches!(err, TenancyError::FleetNotConverged), "expected FleetNotConverged, got {err:?}");
    assert!(policy_row(&db, "legacy_auditor").await.is_some(), "a refused retirement must write nothing");
}

/// `Duration::ZERO` must not disable the lock timeout. Postgres reads `lock_timeout = '0'` as
/// "wait forever" — the exact inverse of the intent on this privileged path — so the adapter
/// clamps it to 1ms. Without the clamp this test hangs on the contended row until the outer
/// `tokio::time::timeout` fires, which is the failure it exists to make attributable.
#[tokio::test]
async fn a_zero_lock_timeout_fails_fast_rather_than_disabling_the_timeout() {
    let Some((c, db_a)) = support::start_migrated_postgres().await else { return };
    seed_orphan_chain(&db_a, "legacy_auditor").await;
    let db_b = second_connection(&c).await;

    let retirer_a = PgSystemRowRetirer::new(db_a.clone());
    let holder = retirer_a.begin_retirement(Duration::from_secs(30)).await.unwrap();
    retirer_a.lock_policy_in(&*holder, "legacy_auditor").await.unwrap().expect("seeded policy row must be found");

    let retirer_b = PgSystemRowRetirer::new(db_b.clone());
    let contended = retirer_b.begin_retirement(Duration::ZERO).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10), retirer_b.lock_policy_in(&*contended, "legacy_auditor"))
        .await
        .expect("a zero lock_timeout was rendered as '0ms', which Postgres reads as NO timeout — the call hung instead of failing");
    let err = result.expect_err("a contended row under a zero (clamped to 1ms) lock_timeout must error, not succeed");
    assert!(matches!(err, AuthzError::Backend(_)), "unexpected error variant: {err:?}");

    drop(holder);
}
