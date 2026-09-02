// SPDX-License-Identifier: Apache-2.0

//! Schema-level tests for m0002 — asserts the exact constraint/index names the D7 error
//! mapping depends on, and that the `ck_membership_one_target` CHECK actually rejects a
//! membership row with more than one target set.

mod support;

use sea_orm::{ConnectionTrait, DbBackend, Statement};

#[tokio::test]
async fn tenancy_schema_has_named_constraints_and_indexes() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let constraint_names = [
        "uq_organization_prn",
        "uq_organization_slug",
        "uq_team_prn",
        "uq_team_org_slug",
        "uq_team_id_org",
        "uq_project_prn",
        "uq_project_team_slug",
        "fk_project_team",
        "fk_team_org",
        "fk_membership_principal",
        "fk_membership_org",
        "fk_membership_team",
        "fk_membership_project",
        "ck_membership_one_target",
    ];
    for n in constraint_names {
        let row = db
            .query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres, "SELECT 1 AS one FROM pg_constraint WHERE conname = $1", [n.into()]))
            .await
            .unwrap();
        assert!(row.is_some(), "missing constraint {n}");
    }

    for n in [
        "uq_membership_principal_org",
        "uq_membership_principal_team",
        "uq_membership_principal_project",
        "ix_team_org",
        "ix_project_team",
        "ix_project_org",
        "ix_membership_principal",
        "ix_membership_org",
        "ix_membership_team",
        "ix_membership_project",
    ] {
        let row = db
            .query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres, "SELECT 1 AS one FROM pg_indexes WHERE indexname = $1", [n.into()]))
            .await
            .unwrap();
        assert!(row.is_some(), "missing index {n}");
    }
}

/// AC — `ck_membership_one_target` rejects a row that sets more than one of
/// org_id/team_id/project_id.
#[tokio::test]
async fn membership_check_rejects_multi_target_rows() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    // Seed one `principal` row (raw SQL — this test only needs a valid FK target, not the
    // domain layer) and one `organization` row.
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"INSERT INTO "principal" (id, prn, kind, status, created_at, updated_at)
           VALUES ('11111111-1111-1111-1111-111111111111', 'prn:pgs:iam:::principal/11111111-1111-1111-1111-111111111111', 'user', 'active', now(), now())"#,
        [],
    ))
    .await
    .unwrap();

    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        r#"INSERT INTO "organization" (id, prn, slug, name, status, created_at, updated_at)
           VALUES ('22222222-2222-2222-2222-222222222222', 'prn:pgs:iam:::organization/22222222-2222-2222-2222-222222222222', 'acme', 'Acme', 'active', now(), now())"#,
        [],
    ))
    .await
    .unwrap();

    // A membership row with BOTH org_id and team_id set must fail `ck_membership_one_target`.
    // `team_id` below is an arbitrary uuid, not a real `team` row — Postgres evaluates CHECK
    // constraints (part of ExecConstraints, before the row is written) ahead of the FK's
    // AFTER-ROW trigger, so the CHECK violation fires first regardless.
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            r#"INSERT INTO "membership" (id, principal_id, org_id, team_id, created_at)
               VALUES ('33333333-3333-3333-3333-333333333333', '11111111-1111-1111-1111-111111111111',
                       '22222222-2222-2222-2222-222222222222', '44444444-4444-4444-4444-444444444444', now())"#,
            [],
        ))
        .await;

    let err = result.expect_err("insert with both org_id and team_id set must fail");
    assert!(err.to_string().contains("ck_membership_one_target"), "unexpected error: {err}");
}
