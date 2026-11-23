// SPDX-License-Identifier: Apache-2.0

//! `ConfigAgentResolver` against a real Postgres (spec §6.2).

mod support;

use paigasus_agent_host::adapters::agents::ConfigAgentResolver;
use paigasus_agent_host_core::agent::AgentDefinition;
use paigasus_agent_host_core::ids::{AgentId, InboxId, PrincipalId};
use paigasus_agent_host_core::ports::{AgentResolver, RepositoryError};
use support::inbox::{insert_inbox, migrated_postgres};
use uuid::Uuid;

fn u(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn definition(org: u128, agent: u128) -> AgentDefinition {
    AgentDefinition {
        id: AgentId::new(u(org), u(agent)),
        name: "triage".to_owned(),
        model_id: "scripted".to_owned(),
        instructions: "Be brief.".to_owned(),
        tool_names: vec![],
    }
}

#[tokio::test]
async fn an_inbox_resolves_to_its_configured_agent() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let def = definition(1, 3);
    let resolver = ConfigAgentResolver::new(db, vec![def.clone()]);
    assert_eq!(resolver.served(), vec![def.id.clone()]);
    let resolved = resolver.resolve(&InboxId::new(u(1), u(2))).await.expect("resolves");
    assert_eq!(resolved.definition, def);
    assert_eq!(resolved.principal, PrincipalId::from_uuid(u(4)));
}

#[tokio::test]
async fn an_agent_of_another_org_with_the_same_uuid_does_not_match() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let resolver = ConfigAgentResolver::new(db, vec![definition(9, 3)]);
    assert_eq!(resolver.resolve(&InboxId::new(u(1), u(2))).await, Err(RepositoryError::NotFound));
}

#[tokio::test]
async fn an_inbox_id_with_the_wrong_org_does_not_load_the_row() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    // The inbox row is in org 1. The caller names it under org 9, and org 9 has the agent.
    insert_inbox(&db, u(1), u(2), u(3), u(4)).await;
    let resolver = ConfigAgentResolver::new(db, vec![definition(9, 3)]);
    assert_eq!(resolver.resolve(&InboxId::new(u(9), u(2))).await, Err(RepositoryError::NotFound));
}

#[tokio::test]
async fn an_unknown_inbox_is_not_found() {
    let Some((_node, _url, db)) = migrated_postgres().await else { return };
    let resolver = ConfigAgentResolver::new(db, vec![definition(1, 3)]);
    assert_eq!(resolver.resolve(&InboxId::new(u(1), u(99))).await, Err(RepositoryError::NotFound));
}
