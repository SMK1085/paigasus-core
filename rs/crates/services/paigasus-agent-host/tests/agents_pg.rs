// SPDX-License-Identifier: Apache-2.0

//! `PgAgentResolver` against a real Postgres (SMA-852 spec §6.2, §6.8).

mod support;

use paigasus_agent_host::adapters::agents::PgAgentResolver;
use paigasus_agent_host_core::agent::AgentDefinition;
use paigasus_agent_host_core::ids::{AgentId, InboxId, PrincipalId};
use paigasus_agent_host_core::ports::{AgentResolver, RepositoryError};
use sea_orm::ConnectionTrait;
use support::inbox::{insert_agent, insert_inbox, migrated_postgres};
use uuid::Uuid;

fn u(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

#[tokio::test]
async fn an_inbox_of_an_active_agent_resolves_to_its_row() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_agent(&db, u(1), u(3), "scripted", "Be brief.", &["memory"]).await;
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let resolved = PgAgentResolver::new(db).resolve(&InboxId::new(u(1), u(2))).await.expect("resolves");
    assert_eq!(
        resolved.definition,
        AgentDefinition {
            id: AgentId::new(u(1), u(3)),
            name: u(3).to_string(),
            model_id: "scripted".to_owned(),
            instructions: "Be brief.".to_owned(),
            tool_names: vec!["memory".to_owned()],
        }
    );
    assert_eq!(resolved.principal, PrincipalId::from_uuid(u(4)));
}

#[tokio::test]
async fn an_inbox_of_an_archived_agent_is_not_found() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let resolver = PgAgentResolver::new(db.clone());
    resolver.resolve(&InboxId::new(u(1), u(2))).await.expect("the active agent resolves first");
    db.execute_unprepared(&format!("UPDATE agent SET archived_at = now() WHERE id = '{}'", u(3))).await.expect("archive");
    assert_eq!(resolver.resolve(&InboxId::new(u(1), u(2))).await, Err(RepositoryError::NotFound));
}

#[tokio::test]
async fn an_inbox_id_with_another_org_is_not_found() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    // The inbox row is in org 1. The caller names it under org 9.
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let resolver = PgAgentResolver::new(db);
    assert_eq!(resolver.resolve(&InboxId::new(u(9), u(2))).await, Err(RepositoryError::NotFound));
}

#[tokio::test]
async fn an_unknown_inbox_is_not_found() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_agent(&db, u(1), u(3), "scripted", "test", &[]).await;
    let resolver = PgAgentResolver::new(db);
    assert_eq!(resolver.resolve(&InboxId::new(u(1), u(99))).await, Err(RepositoryError::NotFound));
}
