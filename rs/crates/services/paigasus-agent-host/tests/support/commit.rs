// SPDX-License-Identifier: Apache-2.0

//! Helpers for the tests of the run commit.

use std::sync::Arc;

use paigasus_agent_host::adapters::memory_commit::MemoryCommit;
use paigasus_agent_host::adapters::persistence::memory::PgMemoryStore;
use paigasus_agent_host::adapters::persistence::run_commit::PgRunCommitter;
use paigasus_agent_host_core::claim::Claim;
use paigasus_agent_host_core::fault::NoFaults;
use paigasus_agent_host_core::ids::{AgentId, InboxId, PrincipalId};
use paigasus_agent_host_core::memory::{MemoryCaps, MemoryChangeSet, MemoryCommitSet, MemoryScope};
use paigasus_agent_host_core::run::{RunClass, RunRecord, RunUsage};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, Statement};
use uuid::Uuid;

/// The memory scope of one agent and one principal of an organization.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub fn scope_of(org: Uuid, agent: Uuid, principal: Uuid) -> MemoryScope {
    MemoryScope::new(AgentId::new(org, agent), PrincipalId::from_uuid(principal))
}

/// The memory scope of an inbox: `(org_id, agent_id, principal_id)` of its row. A run commit
/// refuses every other scope.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub async fn inbox_scope(db: &DatabaseConnection, inbox: &InboxId) -> MemoryScope {
    let sql = format!("SELECT org_id, agent_id, principal_id FROM inbox WHERE id = '{}'", inbox.uuid());
    let row = db
        .query_one_raw(Statement::from_string(DatabaseBackend::Postgres, sql))
        .await
        .expect("read the inbox")
        .expect("the inbox row");
    let get = |column: &str| row.try_get::<Uuid>("", column).expect("uuid column");
    scope_of(get("org_id"), get("agent_id"), get("principal_id"))
}

/// A `Completed` record of the claim: no output, no usage, an empty session and an empty memory
/// set of `scope`. Every claimed event passed the screen.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub fn completed_record(claim: &Claim, scope: MemoryScope) -> RunRecord {
    RunRecord {
        run: claim.run.clone(),
        token: claim.lease_token,
        passed: claim.events.iter().map(|e| e.id).collect(),
        class: RunClass::Completed {
            output: None,
            usage: RunUsage::default(),
        },
        session: Vec::new(),
        nonce: "0123456789abcdef0123456789abcdef".to_string(),
        close_thread: false,
        memory: MemoryCommitSet::new(scope, MemoryChangeSet::new()),
    }
}

/// A committer on `db` with the in-transaction memory store, the default caps and no fault.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub fn committer(db: &DatabaseConnection) -> Arc<PgRunCommitter> {
    Arc::new(PgRunCommitter::new(
        db.clone(),
        MemoryCommit::InTransaction(PgMemoryStore::new(db.clone())),
        MemoryCaps::default(),
        Arc::new(NoFaults),
    ))
}
