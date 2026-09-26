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
use paigasus_iam::adapters::persistence::{Migrator, PgRoleGrantStore};
use paigasus_iam_core::{GrantScope, OrganizationId, PrincipalId, PrincipalKind, ProjectId, RoleGrant, RoleGrantFilter, RoleGrantQuery, RoleGrantStore, TeamId, TenancyNodeRef};
use paigasus_kernel::Prn;
use sea_orm::{ActiveModelTrait, ActiveValue::NotSet, ConnectionTrait, DatabaseConnection, DbBackend, Set, Statement};
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
            format!("SELECT i.indisvalid FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid WHERE c.relname = '{INDEX}'"),
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
