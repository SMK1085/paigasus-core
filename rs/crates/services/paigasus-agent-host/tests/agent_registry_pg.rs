// SPDX-License-Identifier: Apache-2.0

//! The Postgres agent registry against a real database (SMA-852, spec §6.3 and §6.4): create,
//! list, update with a version, archive, the inbox binding and the races between them.

mod support;

use std::time::Duration;

use paigasus_agent_host::adapters::persistence::agent_registry::PgAgentRegistry;
use paigasus_agent_host_core::ids::{AgentId, InboxId, PrincipalId};
use paigasus_agent_host_core::ports::AgentRegistry;
use paigasus_agent_host_core::registry::{AgentFields, PageRequest, RegistryError};
use sea_orm::{ConnectionTrait, DatabaseConnection, TransactionTrait};
use support::inbox::migrated_postgres;
use support::timer::{insert_follow_up_row, timer_state};
use uuid::Uuid;

fn u(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn agent_id(n: u128) -> AgentId {
    AgentId::new(u(1), u(n))
}

fn fields(name: &str) -> AgentFields {
    AgentFields {
        name: name.to_string(),
        model_id: "scripted".to_string(),
        instructions: "be brief".to_string(),
        tool_names: vec!["a".to_string(), "b".to_string()],
    }
}

fn page(size: u32, after: Option<Uuid>) -> PageRequest {
    PageRequest { size, after }
}

async fn scalar_text(db: &DatabaseConnection, sql: &str) -> String {
    db.query_one_raw(sea_orm::Statement::from_string(sea_orm::DatabaseBackend::Postgres, sql.to_string()))
        .await
        .expect("query")
        .expect("row")
        .try_get::<String>("", "v")
        .expect("text column v")
}

#[tokio::test]
async fn create_then_get_returns_version_1_and_no_archived_at() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db);
    let created = reg.create(&agent_id(10), &fields("one")).await.expect("create");
    assert_eq!(created.version, 1);
    assert_eq!(created.archived_at, None);
    assert_eq!(created.definition.id, agent_id(10));
    assert_eq!(created.definition.name, "one");
    assert_eq!(created.definition.model_id, "scripted");
    assert_eq!(created.definition.instructions, "be brief");
    assert_eq!(created.definition.tool_names, vec!["a", "b"]);
    let got = reg.get(u(10)).await.expect("get").expect("some");
    assert_eq!(got, created);
    assert!(reg.get(u(99)).await.expect("get").is_none());
}

#[tokio::test]
async fn list_pages_by_id_and_stops_with_no_next_after() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db);
    for n in 10..15 {
        reg.create(&agent_id(n), &fields(&format!("a{n}"))).await.expect("create");
    }
    let p1 = reg.list(u(1), false, page(2, None)).await.expect("p1");
    assert_eq!(p1.items.iter().map(|a| a.definition.id.uuid()).collect::<Vec<_>>(), vec![u(10), u(11)]);
    assert_eq!(p1.next_after, Some(u(11)));
    let p2 = reg.list(u(1), false, page(2, p1.next_after)).await.expect("p2");
    assert_eq!(p2.items.iter().map(|a| a.definition.id.uuid()).collect::<Vec<_>>(), vec![u(12), u(13)]);
    assert_eq!(p2.next_after, Some(u(13)));
    let p3 = reg.list(u(1), false, page(2, p2.next_after)).await.expect("p3");
    assert_eq!(p3.items.iter().map(|a| a.definition.id.uuid()).collect::<Vec<_>>(), vec![u(14)]);
    assert_eq!(p3.next_after, None);
    // A page of exactly the remaining size has no next page.
    let exact = reg.list(u(1), false, page(5, None)).await.expect("exact");
    assert_eq!(exact.items.len(), 5);
    assert_eq!(exact.next_after, None);
    // Another org sees nothing.
    assert!(reg.list(u(2), false, page(5, None)).await.expect("other org").items.is_empty());
}

#[tokio::test]
async fn list_leaves_out_archived_agents_unless_asked() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db);
    reg.create(&agent_id(10), &fields("a")).await.expect("create");
    reg.create(&agent_id(11), &fields("b")).await.expect("create");
    reg.archive(&agent_id(10)).await.expect("archive");
    let active = reg.list(u(1), false, page(10, None)).await.expect("list");
    assert_eq!(active.items.len(), 1);
    assert_eq!(active.items[0].definition.id, agent_id(11));
    let all = reg.list(u(1), true, page(10, None)).await.expect("list all");
    assert_eq!(all.items.len(), 2);
    assert!(all.items[0].archived_at.is_some());
}

#[tokio::test]
async fn update_with_the_right_version_bumps_the_version_and_updated_at() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db);
    let created = reg.create(&agent_id(10), &fields("old")).await.expect("create");
    let mut new_fields = fields("new");
    new_fields.instructions = "other".to_string();
    new_fields.tool_names = vec![];
    let updated = reg.update(&agent_id(10), 1, &new_fields).await.expect("update");
    assert_eq!(updated.version, 2);
    assert_eq!(updated.definition.name, "new");
    assert_eq!(updated.definition.instructions, "other");
    assert!(updated.definition.tool_names.is_empty());
    assert_eq!(updated.created_at, created.created_at);
    assert!(updated.updated_at > created.updated_at);
    assert_eq!(reg.get(u(10)).await.expect("get").expect("some"), updated);
}

#[tokio::test]
async fn update_with_a_wrong_version_is_version_mismatch_with_the_current_version() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db);
    reg.create(&agent_id(10), &fields("a")).await.expect("create");
    reg.update(&agent_id(10), 1, &fields("b")).await.expect("update");
    let err = reg.update(&agent_id(10), 1, &fields("c")).await.expect_err("stale");
    assert_eq!(err, RegistryError::VersionMismatch { current: 2 });
    assert_eq!(reg.get(u(10)).await.expect("get").expect("some").definition.name, "b");
}

#[tokio::test]
async fn update_of_an_archived_agent_is_archived() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db);
    reg.create(&agent_id(10), &fields("a")).await.expect("create");
    reg.archive(&agent_id(10)).await.expect("archive");
    let err = reg.update(&agent_id(10), 1, &fields("b")).await.expect_err("archived");
    assert_eq!(err, RegistryError::Archived);
}

#[tokio::test]
async fn update_to_a_name_of_another_active_agent_is_name_taken() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db);
    reg.create(&agent_id(10), &fields("a")).await.expect("create");
    reg.create(&agent_id(11), &fields("b")).await.expect("create");
    let err = reg.update(&agent_id(11), 1, &fields("a")).await.expect_err("taken");
    assert_eq!(err, RegistryError::NameTaken);
    let unchanged = reg.get(u(11)).await.expect("get").expect("some");
    assert_eq!(unchanged.version, 1);
    assert_eq!(unchanged.definition.name, "b");
}

#[tokio::test]
async fn archive_twice_returns_the_same_archived_at() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db);
    reg.create(&agent_id(10), &fields("a")).await.expect("create");
    let first = reg.archive(&agent_id(10)).await.expect("archive");
    assert!(first.archived_at.is_some());
    tokio::time::sleep(Duration::from_millis(20)).await;
    let second = reg.archive(&agent_id(10)).await.expect("archive again");
    assert_eq!(second.archived_at, first.archived_at);
    assert_eq!(reg.archive(&agent_id(99)).await.expect_err("unknown"), RegistryError::NotFound);
}

#[tokio::test]
async fn archive_cancels_the_active_timers_of_the_agents_inboxes_only() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db.clone());
    reg.create(&agent_id(10), &fields("a")).await.expect("create");
    reg.create(&agent_id(11), &fields("b")).await.expect("create");
    reg.create_inbox(&InboxId::new(u(1), u(20)), &agent_id(10), &PrincipalId::from_uuid(u(30))).await.expect("bind");
    reg.create_inbox(&InboxId::new(u(1), u(21)), &agent_id(11), &PrincipalId::from_uuid(u(30))).await.expect("bind");
    insert_follow_up_row(&db, u(20), u(40), "agent", "now() + interval '1 hour'", "n").await;
    insert_follow_up_row(&db, u(21), u(41), "agent", "now() + interval '1 hour'", "n").await;
    reg.archive(&agent_id(10)).await.expect("archive");
    assert_eq!(timer_state(&db, u(40)).await.status, "cancelled");
    assert_eq!(timer_state(&db, u(41)).await.status, "active");
}

#[tokio::test]
async fn an_archived_name_is_free_again() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db);
    reg.create(&agent_id(10), &fields("a")).await.expect("create");
    assert_eq!(reg.create(&agent_id(11), &fields("a")).await.expect_err("taken"), RegistryError::NameTaken);
    reg.archive(&agent_id(10)).await.expect("archive");
    reg.create(&agent_id(11), &fields("a")).await.expect("name is free");
}

#[tokio::test]
async fn names_that_differ_only_by_case_are_two_names() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db);
    reg.create(&agent_id(10), &fields("Alpha")).await.expect("create");
    reg.create(&agent_id(11), &fields("alpha")).await.expect("a second name");
}

#[tokio::test]
async fn create_inbox_then_get_inbox() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db);
    reg.create(&agent_id(10), &fields("a")).await.expect("create");
    let inbox_id = InboxId::new(u(1), u(20));
    let principal = PrincipalId::from_uuid(u(30));
    let made = reg.create_inbox(&inbox_id, &agent_id(10), &principal).await.expect("bind");
    assert_eq!(made.inbox.id(), &inbox_id);
    assert_eq!(made.inbox.agent(), &agent_id(10));
    assert_eq!(made.inbox.principal(), &principal);
    assert_eq!(made.agent_archived_at, None);
    let got = reg.get_inbox(u(20)).await.expect("get").expect("some");
    assert_eq!(got, made);
    assert!(reg.get_inbox(u(99)).await.expect("get").is_none());
    let listed = reg.list_inboxes(&agent_id(10), page(10, None)).await.expect("list");
    assert_eq!(listed.items, vec![made]);
    assert_eq!(listed.next_after, None);
}

#[tokio::test]
async fn a_second_bind_is_inbox_exists_with_the_existing_id() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db.clone());
    reg.create(&agent_id(10), &fields("a")).await.expect("create");
    let principal = PrincipalId::from_uuid(u(30));
    reg.create_inbox(&InboxId::new(u(1), u(20)), &agent_id(10), &principal).await.expect("bind");
    let err = reg.create_inbox(&InboxId::new(u(1), u(21)), &agent_id(10), &principal).await.expect_err("second");
    assert_eq!(err, RegistryError::InboxExists(InboxId::new(u(1), u(20))));
    assert!(reg.get_inbox(u(21)).await.expect("get").is_none());
    // The connection is not left in an aborted transaction.
    reg.create_inbox(&InboxId::new(u(1), u(22)), &agent_id(10), &PrincipalId::from_uuid(u(31)))
        .await
        .expect("another principal");
}

#[tokio::test]
async fn a_bind_to_an_archived_agent_is_archived() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db);
    reg.create(&agent_id(10), &fields("a")).await.expect("create");
    reg.archive(&agent_id(10)).await.expect("archive");
    let err = reg.create_inbox(&InboxId::new(u(1), u(20)), &agent_id(10), &PrincipalId::from_uuid(u(30))).await.expect_err("archived");
    assert_eq!(err, RegistryError::Archived);
    let err = reg
        .create_inbox(&InboxId::new(u(1), u(20)), &agent_id(99), &PrincipalId::from_uuid(u(30)))
        .await
        .expect_err("unknown agent");
    assert_eq!(err, RegistryError::NotFound);
}

#[tokio::test]
async fn a_bind_with_an_inbox_id_of_another_org_is_invalid_org() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db.clone());
    reg.create(&agent_id(10), &fields("a")).await.expect("create");
    let err = reg.create_inbox(&InboxId::new(u(2), u(20)), &agent_id(10), &PrincipalId::from_uuid(u(30))).await.expect_err("org");
    assert_eq!(err, RegistryError::Invalid("org"));
    assert_eq!(support::scalar_i64(&db, "SELECT count(*) AS n FROM inbox").await, 0);
}

#[tokio::test]
async fn an_agent_id_of_another_org_is_not_found() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db);
    reg.create(&agent_id(10), &fields("a")).await.expect("create");
    let other = AgentId::new(u(2), u(10));
    assert_eq!(reg.update(&other, 1, &fields("b")).await.expect_err("update"), RegistryError::NotFound);
    assert_eq!(reg.archive(&other).await.expect_err("archive"), RegistryError::NotFound);
    assert_eq!(
        reg.create_inbox(&InboxId::new(u(2), u(20)), &other, &PrincipalId::from_uuid(u(30))).await.expect_err("bind"),
        RegistryError::NotFound
    );
    assert!(reg.list_inboxes(&other, page(10, None)).await.expect("list").items.is_empty());
    // The agent of org 1 is untouched.
    let kept = reg.get(u(10)).await.expect("get").expect("some");
    assert_eq!(kept.version, 1);
    assert_eq!(kept.archived_at, None);
}

#[tokio::test]
async fn get_inbox_of_an_archived_agent_sets_agent_archived_at() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db);
    reg.create(&agent_id(10), &fields("a")).await.expect("create");
    reg.create_inbox(&InboxId::new(u(1), u(20)), &agent_id(10), &PrincipalId::from_uuid(u(30))).await.expect("bind");
    let archived = reg.archive(&agent_id(10)).await.expect("archive");
    let got = reg.get_inbox(u(20)).await.expect("get").expect("some");
    assert_eq!(got.agent_archived_at, archived.archived_at);
    assert!(got.agent_archived_at.is_some());
}

#[tokio::test]
async fn list_inboxes_of_an_archived_agent_returns_its_inboxes() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db);
    reg.create(&agent_id(10), &fields("a")).await.expect("create");
    for (inbox, principal) in [(20, 30), (21, 31), (22, 32)] {
        reg.create_inbox(&InboxId::new(u(1), u(inbox)), &agent_id(10), &PrincipalId::from_uuid(u(principal)))
            .await
            .expect("bind");
    }
    let archived = reg.archive(&agent_id(10)).await.expect("archive");
    let p1 = reg.list_inboxes(&agent_id(10), page(2, None)).await.expect("list");
    assert_eq!(p1.items.iter().map(|r| r.inbox.id().uuid()).collect::<Vec<_>>(), vec![u(20), u(21)]);
    assert_eq!(p1.next_after, Some(u(21)));
    assert!(p1.items.iter().all(|r| r.agent_archived_at == archived.archived_at));
    let p2 = reg.list_inboxes(&agent_id(10), page(2, p1.next_after)).await.expect("list");
    assert_eq!(p2.items.len(), 1);
    assert_eq!(p2.next_after, None);
}

#[tokio::test]
async fn get_of_an_archived_agent_returns_it_with_archived_at_set() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db);
    reg.create(&agent_id(10), &fields("a")).await.expect("create");
    let archived = reg.archive(&agent_id(10)).await.expect("archive");
    let got = reg.get(u(10)).await.expect("get").expect("some");
    assert!(got.archived_at.is_some());
    assert_eq!(got, archived);
}

#[tokio::test]
async fn active_models_lists_only_the_active_agents() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db);
    reg.create(&agent_id(10), &fields("a")).await.expect("create");
    reg.create(&agent_id(11), &fields("b")).await.expect("create");
    reg.archive(&agent_id(10)).await.expect("archive");
    assert_eq!(reg.active_models().await.expect("models"), vec![(agent_id(11), "scripted".to_string())]);
}

#[tokio::test]
async fn two_creates_with_one_name_on_two_connections_exactly_one_succeeds() {
    let Some((_node, url, db)) = migrated_postgres().await else { return };
    let a = PgAgentRegistry::new(support::connect_pinned(&url).await);
    let b = PgAgentRegistry::new(support::connect_pinned(&url).await);
    let (id_a, id_b, fa, fb) = (agent_id(10), agent_id(11), fields("same"), fields("same"));
    let (ra, rb) = tokio::join!(a.create(&id_a, &fa), b.create(&id_b, &fb));
    let oks = [&ra, &rb].iter().filter(|r| r.is_ok()).count();
    assert_eq!(oks, 1, "{ra:?} / {rb:?}");
    let err = ra.err().or(rb.err()).expect("one error");
    assert_eq!(err, RegistryError::NameTaken);
    assert_eq!(support::scalar_i64(&db, "SELECT count(*) AS n FROM agent").await, 1);
}

#[tokio::test]
async fn two_updates_with_one_version_on_two_connections_exactly_one_succeeds() {
    let Some((_node, url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db);
    reg.create(&agent_id(10), &fields("a")).await.expect("create");
    let a = PgAgentRegistry::new(support::connect_pinned(&url).await);
    let b = PgAgentRegistry::new(support::connect_pinned(&url).await);
    let (id, fa, fb) = (agent_id(10), fields("from-a"), fields("from-b"));
    let (ra, rb) = tokio::join!(a.update(&id, 1, &fa), b.update(&id, 1, &fb));
    let oks = [&ra, &rb].iter().filter(|r| r.is_ok()).count();
    assert_eq!(oks, 1, "{ra:?} / {rb:?}");
    let err = ra.as_ref().err().or(rb.as_ref().err()).expect("one error");
    assert_eq!(err, &RegistryError::VersionMismatch { current: 2 });
    assert_eq!(reg.get(u(10)).await.expect("get").expect("some").version, 2);
}

#[tokio::test]
async fn a_bind_and_an_archive_that_race_never_leave_an_inbox_after_the_archive() {
    let Some((_node, url, db)) = migrated_postgres().await else { return };
    let reg = PgAgentRegistry::new(db.clone());
    reg.create(&agent_id(10), &fields("a")).await.expect("create");

    // Order 1: the archive holds its row lock. The bind must wait for it (FOR SHARE), then see
    // the archive and return `Archived`.
    let conn_a = support::connect_pinned(&url).await;
    let conn_b = PgAgentRegistry::new(support::connect_pinned(&url).await);
    let tx = conn_a.begin().await.expect("begin");
    tx.execute_unprepared(&format!("UPDATE agent SET archived_at = now() WHERE id = '{}'", u(10)))
        .await
        .expect("archive update");
    let bind = tokio::spawn(async move { conn_b.create_inbox(&InboxId::new(u(1), u(20)), &agent_id(10), &PrincipalId::from_uuid(u(30))).await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!bind.is_finished(), "the bind must wait on the row lock of the archive");
    tx.commit().await.expect("commit archive");
    let result = tokio::time::timeout(Duration::from_secs(10), bind).await.expect("the bind finishes").expect("join");
    assert_eq!(result.expect_err("archived"), RegistryError::Archived);
    assert_eq!(support::scalar_i64(&db, "SELECT count(*) AS n FROM inbox").await, 0);

    // Order 2: the bind commits first. A later archive succeeds and the inbox exists.
    reg.create(&agent_id(11), &fields("b")).await.expect("create");
    let conn_a = support::connect_pinned(&url).await;
    let conn_b = PgAgentRegistry::new(support::connect_pinned(&url).await);
    conn_b
        .create_inbox(&InboxId::new(u(1), u(21)), &agent_id(11), &PrincipalId::from_uuid(u(31)))
        .await
        .expect("bind first");
    let tx = conn_a.begin().await.expect("begin");
    tx.execute_unprepared(&format!("UPDATE agent SET archived_at = now() WHERE id = '{}'", u(11)))
        .await
        .expect("archive update");
    tx.commit().await.expect("commit archive");
    let inbox = reg.get_inbox(u(21)).await.expect("get").expect("the inbox exists");
    assert!(inbox.agent_archived_at.is_some());
    assert_eq!(scalar_text(&db, &format!("SELECT agent_id::text AS v FROM inbox WHERE id = '{}'", u(21))).await, u(11).to_string());
}
