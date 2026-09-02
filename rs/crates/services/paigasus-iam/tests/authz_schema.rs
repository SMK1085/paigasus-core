// SPDX-License-Identifier: Apache-2.0

//! Schema-level tests for m0004 (ADR-0013, design §6.1): asserts the exact constraint/index
//! names the D7 error mapping and the future `PgPolicyStore`/`PgRoleGrantStore` adapters
//! depend on, round-trips a `policy` + `role` + `role_grant` row (both org-scoped and the
//! Root sentinel) through real Postgres via the SeaORM entities, and asserts
//! `ck_role_grant_scope` rejects a row whose `scope_kind` doesn't match its non-null
//! `scope_*_id`.

mod support;

use chrono::{SubsecRound, Utc};
use paigasus_iam::adapters::persistence::entities::{policy, role, role_grant};
use sea_orm::{ActiveModelTrait, ActiveValue::NotSet, ConnectionTrait, DbBackend, EntityTrait, Set, Statement};
use uuid::Uuid;

/// The `principal` row every test below grants roles to.
const PRINCIPAL_ID: &str = "11111111-1111-1111-1111-111111111111";
/// The `organization` row the org-scoped grant targets.
const ORG_ID: &str = "22222222-2222-2222-2222-222222222222";
/// The synthetic Root sentinel's nil-UUID + canonical PRN (design D4;
/// `paigasus_iam_core::authz::model::root_prn`/`ROOT_ENTITY`, pinned here independently so this
/// schema test doesn't need to depend on `paigasus-iam-core`).
const ROOT_PRN: &str = "prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000";

/// Seeds a `principal` and an `organization` row (raw SQL — this test only needs valid FK
/// targets, not the domain layer), mirroring `tenancy_schema.rs`'s seeding. The UUIDs are
/// fixed test constants inlined directly into the statement text (not bind parameters) —
/// bound `text`-typed parameters need an explicit cast against a `uuid` column, whereas an
/// inline literal is coerced from Postgres's "unknown"-typed constant the same way
/// `tenancy_schema.rs`'s seeding does.
async fn seed_principal_and_org(db: &sea_orm::DatabaseConnection) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r#"INSERT INTO "principal" (id, prn, kind, status, created_at, updated_at)
               VALUES ('{PRINCIPAL_ID}', 'prn:pgs:iam:::principal/{PRINCIPAL_ID}', 'user', 'active', now(), now())"#
        ),
        [],
    ))
    .await
    .unwrap();

    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        format!(
            r#"INSERT INTO "organization" (id, prn, slug, name, status, created_at, updated_at)
               VALUES ('{ORG_ID}', 'prn:pgs:iam:::organization/{ORG_ID}', 'acme', 'Acme', 'active', now(), now())"#
        ),
        [],
    ))
    .await
    .unwrap();
}

#[tokio::test]
async fn authz_schema_has_named_constraints_and_indexes() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let constraint_names = [
        "ck_policy_kind",
        "fk_role_template",
        "fk_role_grant_principal",
        "fk_role_grant_role",
        "fk_role_grant_org",
        "fk_role_grant_team",
        "fk_role_grant_project",
        "ck_role_grant_scope_kind",
        "uq_role_grant_principal_role_scope",
        "uq_role_grant_linked_policy",
        "ck_role_grant_scope",
    ];
    for n in constraint_names {
        let row = db
            .query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres, "SELECT 1 AS one FROM pg_constraint WHERE conname = $1", [n.into()]))
            .await
            .unwrap();
        assert!(row.is_some(), "missing constraint {n}");
    }

    for n in ["ix_role_grant_principal", "ix_role_grant_org", "ix_role_grant_team", "ix_role_grant_project"] {
        let row = db
            .query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres, "SELECT 1 AS one FROM pg_indexes WHERE indexname = $1", [n.into()]))
            .await
            .unwrap();
        assert!(row.is_some(), "missing index {n}");
    }
}

/// AC — a `policy` (template) + `role` referencing it + two `role_grant` rows (one
/// organization-scoped, one the Root sentinel) round-trip through real Postgres via the
/// SeaORM entities.
#[tokio::test]
async fn policy_role_role_grant_round_trip_through_postgres() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    seed_principal_and_org(&db).await;

    let now = Utc::now().trunc_subsecs(6);
    let principal_id = Uuid::parse_str(PRINCIPAL_ID).unwrap();
    let org_id = Uuid::parse_str(ORG_ID).unwrap();

    // --- policy (a template) ---------------------------------------------------------
    let policy_am = policy::ActiveModel {
        policy_id: Set("test_role_template".to_string()),
        kind: Set("template".to_string()),
        source: Set("permit(principal == ?principal, action, resource in ?resource);".to_string()),
        description: Set(Some("test-only template".to_string())),
        system: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
        content_fingerprint: NotSet,
        starter_revision: NotSet,
    };
    policy_am.insert(&db).await.unwrap();

    let got_policy = policy::Entity::find_by_id("test_role_template").one(&db).await.unwrap().expect("policy row present");
    assert_eq!(got_policy.kind, "template");
    assert_eq!(got_policy.source, "permit(principal == ?principal, action, resource in ?resource);");
    assert!(!got_policy.system);

    // --- role (references the template above) -----------------------------------------
    let role_am = role::ActiveModel {
        key: Set("test_role".to_string()),
        template_id: Set("test_role_template".to_string()),
        scope_kinds: Set(r#"["organization"]"#.to_string()),
        description: Set(Some("test-only role".to_string())),
        system: Set(false),
        created_at: Set(now),
    };
    role_am.insert(&db).await.unwrap();

    let got_role = role::Entity::find_by_id("test_role").one(&db).await.unwrap().expect("role row present");
    assert_eq!(got_role.template_id, "test_role_template");
    assert_eq!(got_role.scope_kinds, r#"["organization"]"#);

    // --- role_grant: organization-scoped -----------------------------------------------
    let org_grant_id = Uuid::from_u128(1);
    let org_prn = "prn:pgs:iam:::organization/22222222-2222-2222-2222-222222222222".to_string();
    let org_grant_am = role_grant::ActiveModel {
        id: Set(org_grant_id),
        principal_id: Set(principal_id),
        role_key: Set("test_role".to_string()),
        scope_kind: Set("organization".to_string()),
        scope_node_prn: Set(org_prn.clone()),
        scope_org_id: Set(Some(org_id)),
        scope_team_id: Set(None),
        scope_project_id: Set(None),
        linked_policy_id: Set("grant:org-scope".to_string()),
        created_at: Set(now),
    };
    org_grant_am.insert(&db).await.unwrap();

    let got_org_grant = role_grant::Entity::find_by_id(org_grant_id).one(&db).await.unwrap().expect("org-scoped role_grant row present");
    assert_eq!(got_org_grant.scope_kind, "organization");
    assert_eq!(got_org_grant.scope_node_prn, org_prn);
    assert_eq!(got_org_grant.scope_org_id, Some(org_id));
    assert_eq!(got_org_grant.scope_team_id, None);
    assert_eq!(got_org_grant.scope_project_id, None);

    // --- role_grant: the Root sentinel (all scope_*_id NULL) ---------------------------
    let root_grant_id = Uuid::from_u128(2);
    let root_grant_am = role_grant::ActiveModel {
        id: Set(root_grant_id),
        principal_id: Set(principal_id),
        role_key: Set("test_role".to_string()),
        scope_kind: Set("root".to_string()),
        scope_node_prn: Set(ROOT_PRN.to_string()),
        scope_org_id: Set(None),
        scope_team_id: Set(None),
        scope_project_id: Set(None),
        linked_policy_id: Set("grant:root-scope".to_string()),
        created_at: Set(now),
    };
    root_grant_am.insert(&db).await.unwrap();

    let got_root_grant = role_grant::Entity::find_by_id(root_grant_id).one(&db).await.unwrap().expect("root-scoped role_grant row present");
    assert_eq!(got_root_grant.scope_kind, "root");
    assert_eq!(got_root_grant.scope_node_prn, ROOT_PRN);
    assert_eq!(got_root_grant.scope_org_id, None);
    assert_eq!(got_root_grant.scope_team_id, None);
    assert_eq!(got_root_grant.scope_project_id, None);
}

/// AC — `ck_role_grant_scope` rejects a row whose `scope_kind` doesn't match its non-null
/// `scope_*_id`: `organization` with `scope_org_id` NULL, and `root` with `scope_org_id` set.
#[tokio::test]
async fn role_grant_check_rejects_scope_kind_mismatch() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    seed_principal_and_org(&db).await;

    let policy_am = policy::ActiveModel {
        policy_id: Set("mismatch_template".to_string()),
        kind: Set("template".to_string()),
        source: Set("permit(principal == ?principal, action, resource in ?resource);".to_string()),
        description: Set(None),
        system: Set(false),
        created_at: Set(Utc::now().trunc_subsecs(6)),
        updated_at: Set(Utc::now().trunc_subsecs(6)),
        content_fingerprint: NotSet,
        starter_revision: NotSet,
    };
    policy_am.insert(&db).await.unwrap();

    let role_am = role::ActiveModel {
        key: Set("mismatch_role".to_string()),
        template_id: Set("mismatch_template".to_string()),
        scope_kinds: Set(r#"["organization","root"]"#.to_string()),
        description: Set(None),
        system: Set(false),
        created_at: Set(Utc::now().trunc_subsecs(6)),
    };
    role_am.insert(&db).await.unwrap();

    // Case A: `scope_kind = 'organization'` but `scope_org_id` is NULL. UUIDs are inlined
    // literals (not bind params) for the same reason as `seed_principal_and_org` — a bound
    // `text` parameter against a `uuid` column needs an explicit cast; an inline literal is
    // coerced from Postgres's "unknown"-typed constant.
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                r#"INSERT INTO "role_grant"
                       (id, principal_id, role_key, scope_kind, scope_node_prn, scope_org_id, scope_team_id, scope_project_id, linked_policy_id, created_at)
                   VALUES ('33333333-3333-3333-3333-333333333333', '{PRINCIPAL_ID}', 'mismatch_role', 'organization',
                           'prn:pgs:iam:::organization/{ORG_ID}', NULL, NULL, NULL,
                           'grant:bad-org-null', now())"#
            ),
            [],
        ))
        .await;
    let err = result.expect_err("organization scope_kind with NULL scope_org_id must be rejected");
    assert!(err.to_string().contains("ck_role_grant_scope"), "unexpected error: {err}");

    // Case B: `scope_kind = 'root'` but `scope_org_id` is set (non-NULL).
    let result = db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            format!(
                r#"INSERT INTO "role_grant"
                       (id, principal_id, role_key, scope_kind, scope_node_prn, scope_org_id, scope_team_id, scope_project_id, linked_policy_id, created_at)
                   VALUES ('44444444-4444-4444-4444-444444444444', '{PRINCIPAL_ID}', 'mismatch_role', 'root', '{ROOT_PRN}', '{ORG_ID}', NULL, NULL,
                           'grant:bad-root-org-set', now())"#
            ),
            [],
        ))
        .await;
    let err = result.expect_err("root scope_kind with non-NULL scope_org_id must be rejected");
    assert!(err.to_string().contains("ck_role_grant_scope"), "unexpected error: {err}");
}
