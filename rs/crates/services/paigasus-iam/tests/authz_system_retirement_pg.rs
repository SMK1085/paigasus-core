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
use paigasus_iam_core::{GrantScope, SystemRowRetirer};
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

/// Seeds `n` distinct principals, each granted `role_key` at the synthetic Root scope. Distinct
/// PRINCIPALS (rather than distinct scopes) are what makes each grant a genuinely separate row
/// under `uq_role_grant_principal_role_scope` (principal_id, role_key, scope_node_prn) — every
/// grant here shares the same role and the same Root scope on purpose. Ids are minted from the
/// loop index (never 0, so `Uuid::from_u128` never collides with `Uuid::nil()`), deterministic
/// without needing a real id generator.
async fn seed_grants(db: &DatabaseConnection, role_key: &str, n: u32) {
    let now = Utc::now();
    for i in 0..n {
        let principal_id = Uuid::from_u128(u128::from(i) + 1);
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

        let grant_id = principal_id;
        role_grant::ActiveModel {
            id: Set(grant_id),
            principal_id: Set(principal_id),
            role_key: Set(role_key.to_string()),
            scope_kind: Set("root".to_string()),
            scope_node_prn: Set(GrantScope::Root.canonical_prn()),
            scope_org_id: Set(None),
            scope_team_id: Set(None),
            scope_project_id: Set(None),
            linked_policy_id: Set(format!("grant:{grant_id}")),
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

#[tokio::test]
async fn surviving_grants_are_capped_and_report_the_true_total() {
    let Some((_c, db)) = support::start_migrated_postgres().await else { return };
    seed_orphan_chain(&db, "legacy_auditor").await;
    seed_grants(&db, "legacy_auditor", 5).await;
    let retirer = PgSystemRowRetirer::new(db.clone());

    let tx = retirer.begin_retirement(Duration::from_secs(5)).await.unwrap();
    let survivors = retirer.surviving_grants_in(&*tx, "legacy_auditor", 2).await.unwrap();
    assert_eq!(survivors.grants.len(), 2, "the page is capped");
    assert_eq!(survivors.total, 5, "the total is the truth, not the page size");
    assert!(survivors.truncated(2));

    let ids: Vec<&str> = survivors.grants.iter().map(|g| g.id.as_str()).collect();
    let mut sorted = ids.clone();
    sorted.sort_unstable();
    assert_eq!(ids, sorted, "ordered by id so a refusal lists them deterministically");
}

#[tokio::test]
async fn min_starter_revision_reports_null_as_none() {
    let Some((_c, db)) = support::start_migrated_postgres().await else { return };
    // A pre-m0010 row: system-owned with a NULL starter_revision.
    seed_system_policy_with_revision(&db, "legacy_forbid", None).await;
    let retirer = PgSystemRowRetirer::new(db.clone());
    assert_eq!(retirer.min_starter_revision().await.unwrap(), None, "a NULL revision is unprovable, not zero");
}
