// SPDX-License-Identifier: Apache-2.0

//! SMA-699: the scope-filtered `RoleGrantQuery::find` and its index
//! (`ix_role_grant_scope_node_prn_principal_id`, m0012). Three parts: m0012's migration round
//! trip; the results of every filter shape against the domain oracle
//! `RoleGrantFilter::matches`; and the plans of the three real query shapes (spec "The real
//! query shapes"), with a custom and a generic plan.

mod support;

use chrono::{SubsecRound, Utc};
use paigasus_iam::adapters::authz::Generations;
use paigasus_iam::adapters::persistence::entities::{policy, role};
use paigasus_iam::adapters::persistence::pg_role_grants::find_statement;
use paigasus_iam::adapters::persistence::{Migrator, PgRoleGrantStore};
use paigasus_iam_core::{GrantScope, OrganizationId, PrincipalId, PrincipalKind, ProjectId, RoleGrant, RoleGrantFilter, RoleGrantQuery, RoleGrantStore, TeamId, TenancyNodeRef};
use paigasus_kernel::Prn;
use sea_orm::{ActiveModelTrait, ActiveValue::NotSet, ConnectionTrait, DatabaseConnection, DbBackend, Set, Statement, TransactionTrait};
use sea_orm_migration::MigratorTrait;
use std::collections::HashMap;
use uuid::Uuid;

const INDEX: &str = "ix_role_grant_scope_node_prn_principal_id";

/// m0012 is the 12th migration. Pinning the `up` count (not counting `down` steps from the tip)
/// keeps `down(Some(1))` equal to m0012's `down` when later migrations land (the
/// `audit_log_partition_pg.rs` precedent).
const M0012: u32 = 12;

/// `None`: no such index. `Some(valid)`: the index exists, with `pg_index.indisvalid`.
async fn index_validity(db: &DatabaseConnection) -> Option<bool> {
    let row = db
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            format!("SELECT i.indisvalid FROM pg_index i WHERE i.indexrelid = to_regclass('public.{INDEX}')"),
        ))
        .await
        .unwrap()?;
    Some(row.try_get::<bool>("", "indisvalid").unwrap())
}

#[tokio::test]
async fn m0012_up_down_up_keeps_a_valid_index() {
    let Some((_pg, db)) = support::start_raw_postgres().await else { return };
    Migrator::up(&db, Some(M0012)).await.expect("m0001..m0012 must apply");
    assert_eq!(index_validity(&db).await, Some(true), "after up");

    Migrator::down(&db, Some(1)).await.expect("m0012 down must succeed");
    assert_eq!(index_validity(&db).await, None, "after down");

    Migrator::up(&db, Some(1)).await.expect("m0012 must apply again");
    assert_eq!(index_validity(&db).await, Some(true), "after the second up");
}

/// E5: `CREATE INDEX IF NOT EXISTS` skips an INVALID index with the same name (what a failed
/// out-of-band `CREATE INDEX CONCURRENTLY` leaves), and the planner ignores an INVALID index.
/// m0012 must fail with an operator message, not pass.
#[tokio::test]
async fn m0012_refuses_an_invalid_leftover_index() {
    let Some((_pg, db)) = support::start_raw_postgres().await else { return };
    Migrator::up(&db, Some(M0012 - 1)).await.expect("m0001..m0011 must apply");
    db.execute_unprepared(&format!(r#"CREATE INDEX {INDEX} ON "role_grant" (scope_node_prn, principal_id, id);"#))
        .await
        .unwrap();
    db.execute_unprepared(&format!("UPDATE pg_index SET indisvalid = false WHERE indexrelid = '{INDEX}'::regclass;"))
        .await
        .unwrap();
    assert_eq!(index_validity(&db).await, Some(false), "sanity: the leftover index is INVALID");

    let err = Migrator::up(&db, Some(1)).await.expect_err("m0012 must refuse an INVALID index");
    let msg = err.to_string();
    assert!(msg.contains(INDEX) && msg.contains("INVALID"), "the error must name the index and the cause: {msg}");
}

/// `CREATE INDEX IF NOT EXISTS` also skips a VALID index with this name but other columns (an
/// operator build with a wrong definition). m0012 must refuse it, not pass.
#[tokio::test]
async fn m0012_refuses_a_valid_index_with_other_columns() {
    let Some((_pg, db)) = support::start_raw_postgres().await else { return };
    Migrator::up(&db, Some(M0012 - 1)).await.expect("m0001..m0011 must apply");
    db.execute_unprepared(&format!(r#"CREATE INDEX {INDEX} ON "role_grant" (linked_policy_id);"#)).await.unwrap();

    let err = Migrator::up(&db, Some(1)).await.expect_err("m0012 must refuse a valid index with the wrong definition");
    let msg = err.to_string();
    assert!(msg.contains(INDEX) && msg.contains("linked_policy_id"), "the error must name the index and the found definition: {msg}");
}

fn pid(u: Uuid) -> PrincipalId {
    PrincipalId::from_prn(Prn::build("iam", "", None, "principal", u).unwrap())
}

async fn exec(db: &DatabaseConnection, sql: String) {
    db.execute_raw(Statement::from_string(DbBackend::Postgres, sql)).await.unwrap();
}

async fn seed_principal(db: &DatabaseConnection, u: Uuid, kind: PrincipalKind) {
    exec(
        db,
        format!(
            r#"INSERT INTO "principal" (id, prn, kind, status, created_at, updated_at)
               VALUES ('{u}', 'prn:pgs:iam:::principal/{u}', '{}', 'active', now(), now())"#,
            kind.as_str()
        ),
    )
    .await;
}

async fn seed_org(db: &DatabaseConnection, org: &OrganizationId, slug: &str) {
    exec(
        db,
        format!(
            r#"INSERT INTO "organization" (id, prn, slug, name, status, created_at, updated_at)
               VALUES ('{}', '{}', '{slug}', '{slug}', 'active', now(), now())"#,
            org.uuid(),
            org.canonical()
        ),
    )
    .await;
}

/// A `policy` template + a `role` row: the `fk_role_grant_role` target (copied from
/// `authz_role_grants.rs::seed_role`; each test binary compiles its own helpers).
async fn seed_role(db: &DatabaseConnection, role_key: &str) {
    let now = Utc::now().trunc_subsecs(6);
    let template_id = format!("{role_key}_template");
    policy::ActiveModel {
        policy_id: Set(template_id.clone()),
        kind: Set("template".to_string()),
        source: Set("permit(principal == ?principal, action, resource in ?resource);".to_string()),
        description: Set(None),
        system: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
        content_fingerprint: NotSet,
        starter_revision: NotSet,
    }
    .insert(db)
    .await
    .unwrap();
    role::ActiveModel {
        key: Set(role_key.to_string()),
        template_id: Set(template_id),
        scope_kinds: Set(r#"["root","organization","team","project"]"#.to_string()),
        description: Set(None),
        system: Set(false),
        created_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

/// Every filter shape `RoleGrantFilter::new` accepts, over values from a fixed grant set, gives
/// the rows the domain oracle `RoleGrantFilter::matches` selects, in `(principal_id, id)` order.
/// The candidate scopes include Root, a project, and a team PRN with a forged org slot (D5: it
/// matches no row). Paging: `limit` 1 gives the first oracle row; an `offset` past the end gives
/// no row.
#[tokio::test]
async fn find_matches_the_domain_oracle_for_every_filter_shape() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    let (u1, u2, s1) = (Uuid::from_u128(0x11), Uuid::from_u128(0x12), Uuid::from_u128(0x13));
    let kinds: HashMap<Uuid, PrincipalKind> = HashMap::from([(u1, PrincipalKind::User), (u2, PrincipalKind::User), (s1, PrincipalKind::ServiceAccount)]);
    for (u, k) in &kinds {
        seed_principal(&db, *u, *k).await;
    }
    let (o1, o2) = (OrganizationId::from_uuid(Uuid::from_u128(0x21)), OrganizationId::from_uuid(Uuid::from_u128(0x22)));
    seed_org(&db, &o1, "one").await;
    seed_org(&db, &o2, "two").await;
    let team = TeamId::from_parts(o1.uuid(), Uuid::from_u128(0x31));
    exec(
        &db,
        format!(
            r#"INSERT INTO "team" (id, org_id, prn, slug, name, status, created_at, updated_at)
               VALUES ('{}', '{}', '{}', 'core', 'Core', 'active', now(), now())"#,
            team.uuid(),
            o1.uuid(),
            team.canonical()
        ),
    )
    .await;
    let project = ProjectId::from_parts(o1.uuid(), Uuid::from_u128(0x41));
    exec(
        &db,
        format!(
            r#"INSERT INTO "project" (id, team_id, org_id, prn, slug, name, status, created_at, updated_at)
               VALUES ('{}', '{}', '{}', '{}', 'svc', 'Svc', 'active', now(), now())"#,
            project.uuid(),
            team.uuid(),
            o1.uuid(),
            project.canonical()
        ),
    )
    .await;
    seed_role(&db, "gateway_user").await;
    seed_role(&db, "org_admin").await;

    let at_o1 = GrantScope::Node(TenancyNodeRef::Organization(o1.clone()));
    let at_o2 = GrantScope::Node(TenancyNodeRef::Organization(o2.clone()));
    let at_team = GrantScope::Node(TenancyNodeRef::Team(team.clone()));
    let at_project = GrantScope::Node(TenancyNodeRef::Project(project.clone()));
    let forged_team = GrantScope::Node(TenancyNodeRef::Team(TeamId::from_parts(o2.uuid(), team.uuid())));

    let store = PgRoleGrantStore::new(db.clone(), Generations::memory());
    let mut grants: Vec<RoleGrant> = Vec::new();
    for (n, (who, role_key, scope)) in [
        (u2, "gateway_user", at_o1.clone()),
        (u1, "gateway_user", at_o1.clone()),
        (s1, "gateway_user", at_o1.clone()),
        (u1, "org_admin", at_o1.clone()),
        (u1, "gateway_user", at_o2.clone()),
        (u1, "gateway_user", at_team.clone()),
        (u2, "org_admin", at_project.clone()),
        (u1, "org_admin", GrantScope::Root),
        (s1, "gateway_user", GrantScope::Root),
    ]
    .into_iter()
    .enumerate()
    {
        let id = Uuid::from_u128(0x1000 + n as u128);
        let g = RoleGrant {
            id,
            principal: pid(who),
            role_key: role_key.to_string(),
            scope,
            linked_policy_id: format!("grant:{id}"),
            created_at: now,
        };
        store.grant(&g).await.unwrap();
        grants.push(g);
    }

    let mut checked = 0;
    for principal in [None, Some(pid(u1)), Some(pid(s1))] {
        for scope in [
            None,
            Some(at_o1.clone()),
            Some(at_team.clone()),
            Some(at_project.clone()),
            Some(GrantScope::Root),
            Some(forged_team.clone()),
        ] {
            for role_key in [None, Some("gateway_user".to_string())] {
                for kind in [None, Some(PrincipalKind::User), Some(PrincipalKind::ServiceAccount)] {
                    let Some(f) = RoleGrantFilter::new(principal.clone(), scope.clone(), role_key.clone(), kind) else {
                        continue;
                    };
                    let mut expected: Vec<RoleGrant> = grants.iter().filter(|g| f.matches(g, kinds.get(&g.principal.uuid()).copied())).cloned().collect();
                    expected.sort_by_key(|g| (g.principal.uuid(), g.id));

                    assert_eq!(RoleGrantQuery::find(&store, &f, 200, 0).await.unwrap(), expected, "{f:?}");
                    assert_eq!(
                        RoleGrantQuery::find(&store, &f, 1, 0).await.unwrap(),
                        expected.iter().take(1).cloned().collect::<Vec<_>>(),
                        "limit 1: {f:?}"
                    );
                    assert!(RoleGrantQuery::find(&store, &f, 200, expected.len() as u64 + 1).await.unwrap().is_empty(), "offset past the end: {f:?}");
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, 102, "every accepted shape: 3 x 6 x 2 x 3, minus the 6 with no principal and no scope");

    let forged = RoleGrantFilter::new(None, Some(forged_team), None, None).unwrap();
    assert!(RoleGrantQuery::find(&store, &forged, 200, 0).await.unwrap().is_empty(), "D5: a forged org slot matches no row");
}

/// Bulk seed for the plan tests (spec "Tests / Seed"). 10,007 principals (every 5th a service
/// account), 2,000 orgs, 25,000 org grants over 4 roles. 10,007 is prime, so
/// (principal, role, org) never repeats within 25,000 rows and `uq_role_grant_principal_role_scope`
/// holds. The target org (`md5('o1')`) gets 12 grants (n = 2000, 4000, ..., 24000): under 0.1% of the table. Every table stays
/// under 30,000 rows, so `ANALYZE` reads it whole and the statistics are the same on every run.
/// Autovacuum is off, so no background `ANALYZE` changes them during the test.
async fn seed_bulk(db: &DatabaseConnection) -> OrganizationId {
    for r in ["r0", "r1", "r2", "r3"] {
        seed_role(db, r).await;
    }
    exec(
        db,
        r#"INSERT INTO "principal" (id, prn, kind, status, created_at, updated_at)
           SELECT md5('p' || n)::uuid, 'prn:pgs:iam:::principal/' || md5('p' || n)::uuid,
                  CASE WHEN n % 5 = 0 THEN 'service_account' ELSE 'user' END, 'active', now(), now()
             FROM generate_series(1, 10007) n"#
            .to_string(),
    )
    .await;
    exec(
        db,
        r#"INSERT INTO "organization" (id, prn, slug, name, status, created_at, updated_at)
           SELECT md5('o' || n)::uuid, 'prn:pgs:iam:::organization/' || md5('o' || n)::uuid,
                  'org-' || n, 'Org ' || n, 'active', now(), now()
             FROM generate_series(1, 2000) n"#
            .to_string(),
    )
    .await;
    exec(
        db,
        r#"INSERT INTO "role_grant" (id, principal_id, role_key, scope_kind, scope_node_prn, scope_org_id,
                                     scope_team_id, scope_project_id, linked_policy_id, created_at)
           SELECT md5('g' || n)::uuid, md5('p' || (1 + n % 10007))::uuid, 'r' || (n % 4), 'organization',
                  'prn:pgs:iam:::organization/' || md5('o' || (1 + n % 2000))::uuid, md5('o' || (1 + n % 2000))::uuid,
                  NULL, NULL, 'grant:' || md5('g' || n)::uuid, now()
             FROM generate_series(1, 25000) n"#
            .to_string(),
    )
    .await;
    exec(db, r#"ALTER TABLE "role_grant" SET (autovacuum_enabled = false)"#.to_string()).await;
    exec(db, r#"ALTER TABLE "principal" SET (autovacuum_enabled = false)"#.to_string()).await;
    exec(db, r#"ANALYZE "role_grant""#.to_string()).await;
    exec(db, r#"ANALYZE "principal""#.to_string()).await;

    let target = db
        .query_one_raw(Statement::from_string(DbBackend::Postgres, "SELECT md5('o1')::uuid AS id".to_string()))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Uuid>("", "id")
        .unwrap();
    let org = OrganizationId::from_uuid(target);
    // Sanity: the seeded PRN text is the canonical form the service writes and queries with.
    let stored: String = db
        .query_one_raw(Statement::from_string(DbBackend::Postgres, format!(r#"SELECT prn FROM "organization" WHERE id = '{target}'"#)))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "prn")
        .unwrap();
    assert_eq!(stored, org.canonical(), "the seed must write canonical org PRNs");
    org
}

/// One query shape for `EXPLAIN EXECUTE`: the filter, and the Postgres type and SQL literal of
/// every bound value in `find_statement` order (principal, scope, role, kind, limit, offset).
struct Shape {
    name: &'static str,
    filter: RoleGrantFilter,
    limit: u64,
    offset: u64,
    types: Vec<&'static str>,
    literals: Vec<String>,
}

/// The custom plan and the generic plan of `sql`, through a named prepared statement:
/// `plan_cache_mode` controls only the plan cache, which a plain `EXPLAIN` with bound values
/// never goes through. `drop_index` drops the new index first (control 1); the transaction
/// rolls back, so the index comes back. Returns `(custom, generic)`.
async fn plans(db: &DatabaseConnection, sql: &str, types: &[&str], literals: &[String], drop_index: bool) -> (String, String) {
    let txn = db.begin().await.unwrap();
    if drop_index {
        txn.execute_unprepared(&format!("DROP INDEX {INDEX};")).await.unwrap();
    }
    txn.execute_unprepared(&format!("PREPARE sma699_q({}) AS {sql}", types.join(", "))).await.unwrap();
    let mut out = Vec::new();
    for mode in ["force_custom_plan", "force_generic_plan"] {
        txn.execute_unprepared(&format!("SET LOCAL plan_cache_mode = {mode};")).await.unwrap();
        let rows = txn
            .query_all_raw(Statement::from_string(DbBackend::Postgres, format!("EXPLAIN EXECUTE sma699_q({})", literals.join(", "))))
            .await
            .unwrap();
        out.push(rows.iter().map(|r| r.try_get::<String>("", "QUERY PLAN").unwrap()).collect::<Vec<_>>().join("\n"));
    }
    txn.execute_unprepared("DEALLOCATE sma699_q;").await.unwrap();
    txn.rollback().await.unwrap();
    let generic = out.pop().unwrap();
    let custom = out.pop().unwrap();
    (custom, generic)
}

fn shapes(org: &OrganizationId) -> Vec<Shape> {
    let scope = GrantScope::Node(TenancyNodeRef::Organization(org.clone()));
    let prn = format!("'{}'", org.canonical());
    // A principal with a grant at the target org: n = 2000 gives org 1 + 2000 % 2000 = 1 and
    // principal 1 + 2000 % 10007 = 2001, role r0 (2000 % 4 = 0), kind user (2001 % 5 != 0).
    let principal_sql = "md5('p2001')::uuid";
    vec![
        Shape {
            name: "A offset 0 (org page)",
            filter: RoleGrantFilter::new(None, Some(scope.clone()), None, Some(PrincipalKind::User)).unwrap(),
            limit: 200,
            offset: 0,
            types: vec!["text", "text", "bigint", "bigint"],
            literals: vec![prn.clone(), "'user'".into(), "200".into(), "0".into()],
        },
        Shape {
            name: "A offset 800 (org page, page 5)",
            filter: RoleGrantFilter::new(None, Some(scope.clone()), None, Some(PrincipalKind::User)).unwrap(),
            limit: 200,
            offset: 800,
            types: vec!["text", "text", "bigint", "bigint"],
            literals: vec![prn.clone(), "'user'".into(), "200".into(), "800".into()],
        },
        Shape {
            name: "B (revoke check)",
            filter: RoleGrantFilter::new(Some(pid(Uuid::nil())), Some(scope.clone()), Some("r0".into()), Some(PrincipalKind::User)).unwrap(),
            limit: 50,
            offset: 0,
            types: vec!["uuid", "text", "text", "text", "bigint", "bigint"],
            literals: vec![principal_sql.into(), prn.clone(), "'r0'".into(), "'user'".into(), "50".into(), "0".into()],
        },
        Shape {
            name: "C (grant pre-check)",
            filter: RoleGrantFilter::new(Some(pid(Uuid::nil())), Some(scope), Some("r0".into()), None).unwrap(),
            limit: 1,
            offset: 0,
            types: vec!["uuid", "text", "text", "bigint", "bigint"],
            literals: vec![principal_sql.into(), prn, "'r0'".into(), "1".into(), "0".into()],
        },
    ]
}

/// SMA-699 acceptance. Shape A uses the new index with a custom and a generic plan; shapes B
/// and C never scan `role_grant` in full. The test pins the index name for shape A (the outbox
/// precedent does not): the seed rules fix the table shape, and the index is the object under
/// test. The prepared statement's text is `find_statement`'s own SQL; only the principal value
/// comes from the `EXECUTE` literals (the filter's own principal is a placeholder with the same
/// shape). Plans print with `--no-capture` for the PR.
#[tokio::test]
async fn the_real_query_shapes_use_an_index_in_custom_and_generic_plans() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let org = seed_bulk(&db).await;

    let store = PgRoleGrantStore::new(db.clone(), Generations::memory());
    let org_page = RoleGrantFilter::new(None, Some(GrantScope::Node(TenancyNodeRef::Organization(org.clone()))), None, Some(PrincipalKind::User)).unwrap();
    let rows = RoleGrantQuery::find(&store, &org_page, 200, 0).await.unwrap();
    // Sanity: the target org holds exactly 12 grants (the 12 target-org principals 2001, 4001,
    // 6001, 8001, 10001, 1994, 3994, 5994, 7994, 9994, 1987, 3987 are all users), and every row's
    // scope is the target org itself.
    assert_eq!(rows.len(), 12, "sanity: the target org must hold exactly 12 grants, got {}", rows.len());
    for row in &rows {
        assert_eq!(row.scope.canonical_prn(), org.canonical(), "every row's scope must be the target org: {row:?}");
    }

    for s in shapes(&org) {
        let stmt = find_statement(&s.filter, s.limit, s.offset);
        assert_eq!(
            stmt.values.as_ref().map_or(0, |v| v.0.len()),
            s.types.len(),
            "{}: the hand-typed type list must match find_statement's bound value count",
            s.name
        );
        assert_eq!(
            stmt.values.as_ref().map_or(0, |v| v.0.len()),
            s.literals.len(),
            "{}: the hand-typed literal list must match find_statement's bound value count",
            s.name
        );
        let (custom, generic) = plans(&db, &stmt.sql, &s.types, &s.literals, false).await;
        eprintln!("=== {} / custom plan ===\n{custom}\n=== {} / generic plan ===\n{generic}\n", s.name, s.name);

        assert!(!custom.contains("$1"), "{}: the custom plan must bind the values:\n{custom}", s.name);
        assert!(generic.contains("$1"), "{}: the generic plan must show parameters:\n{generic}", s.name);
        for (mode, plan) in [("custom", &custom), ("generic", &generic)] {
            assert!(!plan.contains("Seq Scan on role_grant"), "{} / {mode}: no full scan of role_grant:\n{plan}", s.name);
            if s.name.starts_with('A') {
                assert!(plan.contains(INDEX), "{} / {mode}: shape A must use {INDEX}:\n{plan}", s.name);
            }
        }
    }
}

/// Control 1: without the new index, shape A's custom plan scans `role_grant` in full — this is
/// the proof that the seed is large enough to make the index matter, so the positive assertion
/// above can fail. The generic plan takes a different full read: MEASURED, it falls back to a
/// full scan of `uq_role_grant_principal_role_scope` (m0004), whose leading column
/// `principal_id` is unbound in shape A, so `Index Cond: (scope_node_prn = $1)` still reads the
/// whole index — the generic plan cannot see the real `LIMIT` value (it is itself a bound
/// parameter under `force_generic_plan`) and prefers the path that already matches `ORDER BY
/// g.principal_id, g.id` over a plan with a lower total cost. Both `Seq Scan on role_grant` and
/// a full scan of `uq_role_grant_principal_role_scope` are full reads of `role_grant`, so either
/// one proves the same thing for the generic half: with no index, the generic plan has no cheap
/// path to the target rows. This is measured on PostgreSQL 16 (16-alpine), which has no B-tree
/// skip scan; a skip scan (PostgreSQL 18+) can change this.
#[tokio::test]
async fn control_without_the_index_the_org_page_query_scans_role_grant() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let org = seed_bulk(&db).await;
    let a = shapes(&org).into_iter().next().unwrap();
    let stmt = find_statement(&a.filter, a.limit, a.offset);
    assert_eq!(
        stmt.values.as_ref().map_or(0, |v| v.0.len()),
        a.types.len(),
        "control 1: the hand-typed type list must match find_statement's bound value count"
    );
    assert_eq!(
        stmt.values.as_ref().map_or(0, |v| v.0.len()),
        a.literals.len(),
        "control 1: the hand-typed literal list must match find_statement's bound value count"
    );
    let (custom, generic) = plans(&db, &stmt.sql, &a.types, &a.literals, true).await;
    eprintln!("=== control 1 / custom ===\n{custom}\n=== control 1 / generic ===\n{generic}");
    assert!(!custom.contains(INDEX), "the index was dropped:\n{custom}");
    assert!(!generic.contains(INDEX), "the index was dropped:\n{generic}");
    assert!(custom.contains("Seq Scan on role_grant"), "with no index, the custom plan must scan role_grant:\n{custom}");
    assert!(
        generic.contains("Seq Scan on role_grant") || generic.contains("Index Scan using uq_role_grant_principal_role_scope"),
        "with no index, the generic plan must take some full read of role_grant:\n{generic}"
    );
}

/// The SMA-676 statement, kept as a fixture for control 2 only.
const OLD_IS_NULL_OR_SQL: &str = r#"
SELECT g.id, g.principal_id, g.role_key, g.scope_kind, g.scope_node_prn, g.scope_org_id,
       g.scope_team_id, g.scope_project_id, g.linked_policy_id, g.created_at
  FROM "role_grant" g JOIN "principal" pr ON pr.id = g.principal_id
 WHERE ($1::uuid IS NULL OR g.principal_id = $1)
   AND ($2::text IS NULL OR g.scope_node_prn = $2)
   AND ($3::boolean IS FALSE OR g.scope_kind = 'root')
   AND ($4::text IS NULL OR g.role_key = $4)
   AND ($5::text IS NULL OR pr.kind = $5)
 ORDER BY g.principal_id, g.id
 LIMIT $6 OFFSET $7"#;

/// Control 2: with the index in place, the old `IS NULL OR` statement cannot use it in a
/// generic plan. This proves that the builder (spec E2) is needed, not only the index.
#[tokio::test]
async fn control_the_old_is_null_or_statement_cannot_use_the_index_in_a_generic_plan() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let org = seed_bulk(&db).await;
    let types = ["uuid", "text", "boolean", "text", "text", "bigint", "bigint"];
    let literals = [
        "NULL".to_string(),
        format!("'{}'", org.canonical()),
        "false".into(),
        "NULL".into(),
        "'user'".into(),
        "200".into(),
        "0".into(),
    ];
    let (custom, generic) = plans(&db, OLD_IS_NULL_OR_SQL, &types, &literals, false).await;
    eprintln!("=== control 2 / custom ===\n{custom}\n=== control 2 / generic ===\n{generic}");
    assert!(generic.contains("$2"), "sanity: a generic plan:\n{generic}");
    assert!(!generic.contains(INDEX), "the IS NULL OR form must not reach the index in a generic plan:\n{generic}");
}
