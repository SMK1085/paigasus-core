// SPDX-License-Identifier: Apache-2.0

//! Postgres-level behaviour of the SMA-481 retirement path: the FK ordering the schema forces,
//! the locks that make the checks trustworthy, and the deletes themselves.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker
//! daemon is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note —
//! same gating pattern as `tests/roundtrip.rs`/`tests/authz_bootstrap.rs`.

mod support;

use chrono::Utc;
use paigasus_iam::adapters::persistence::PgSystemRowRetirer;
use paigasus_iam::adapters::persistence::entities::{policy, principal, role, role_grant};
use paigasus_iam_core::{AuthzError, GrantScope, SystemRowRetirer};
use sea_orm::{ActiveModelTrait, ActiveValue::NotSet, DatabaseConnection, Set};
use std::time::Duration;
use uuid::Uuid;

/// Seeds a system-owned template + role at a NON-code-defined id: the `policy` row first, then
/// the `role` row that references it — `fk_role_template` requires that order, the same order
/// retirement must undo in reverse. Direct SeaORM inserts on purpose: there is deliberately no
/// supported path that writes a `role` row for a key the code catalog does not define — that
/// absence IS the bug SMA-481 exists for.
async fn seed_orphan_chain(db: &DatabaseConnection, id: &str) {
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
        starter_revision: NotSet,
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
/// read, never a delete.
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
    // A pre-m0010 row: system-owned with a NULL starter_revision.
    seed_system_policy_with_revision(&db, "legacy_forbid", None).await;
    let retirer = PgSystemRowRetirer::new(db.clone());
    assert_eq!(retirer.min_starter_revision().await.unwrap(), None, "a NULL revision is unprovable, not zero");
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
    seed_system_policy_with_raw_revision(&db, "legacy_forbid", -1).await;
    let retirer = PgSystemRowRetirer::new(db.clone());
    let err = retirer.min_starter_revision().await.expect_err("a negative starter_revision must surface as an error, not coerce to 0");
    assert!(matches!(err, AuthzError::Backend(_)), "unexpected error variant: {err:?}");
}
