// SPDX-License-Identifier: Apache-2.0

//! In-memory fakes of the agent registry ports for the transport tests (REST and gRPC): an
//! `AgentRegistry` that keeps rows in memory and records each call, an `AgentRegistry` that fails
//! the test on any call, a fixed `RunCapabilities` and a counting `IdSource`.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use jiff::Timestamp;
use paigasus_agent_host_core::agent::AgentDefinition;
use paigasus_agent_host_core::ids::{AgentId, InboxId, PrincipalId};
use paigasus_agent_host_core::inbox::Inbox;
use paigasus_agent_host_core::ports::{AgentRegistry, IdSource, RunCapabilities};
use paigasus_agent_host_core::registry::{AgentFields, AgentRecord, InboxRecord, Page, PageRequest, RegistryError};
use uuid::Uuid;

/// An `AgentRegistry` over two vectors. It records the name of each call.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
#[derive(Default)]
pub struct MemoryRegistry {
    agents: Mutex<Vec<AgentRecord>>,
    inboxes: Mutex<Vec<InboxRecord>>,
    calls: Mutex<Vec<&'static str>>,
}

#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
impl MemoryRegistry {
    fn call(&self, name: &'static str) {
        self.calls.lock().unwrap().push(name);
    }

    /// The names of the calls, in order.
    pub fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }

    /// Store an active agent directly, with no rule. The time is the epoch.
    pub fn seed_agent(&self, org: Uuid, id: Uuid, name: &str) -> AgentId {
        let agent = AgentId::new(org, id);
        self.agents.lock().unwrap().push(AgentRecord {
            definition: AgentDefinition {
                id: agent.clone(),
                name: name.to_owned(),
                model_id: "m1".to_owned(),
                instructions: "Be brief.".to_owned(),
                tool_names: vec![],
            },
            version: 1,
            created_at: Timestamp::UNIX_EPOCH,
            updated_at: Timestamp::UNIX_EPOCH,
            archived_at: None,
        });
        agent
    }

    /// Store an inbox directly, with no rule.
    pub fn seed_inbox(&self, agent: &AgentId, id: Uuid, principal: Uuid) -> InboxId {
        let inbox = InboxId::new(agent.org(), id);
        self.inboxes.lock().unwrap().push(InboxRecord {
            inbox: Inbox::new(inbox.clone(), agent.clone(), PrincipalId::from_uuid(principal), Timestamp::UNIX_EPOCH).expect("inbox"),
            agent_archived_at: None,
        });
        inbox
    }
}

#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
fn page_of<T: Clone>(mut rows: Vec<T>, page: PageRequest, id_of: impl Fn(&T) -> Uuid) -> Page<T> {
    rows.sort_by_key(&id_of);
    let rows: Vec<T> = rows.into_iter().filter(|r| page.after.is_none_or(|a| id_of(r) > a)).collect();
    let size = page.size as usize;
    let next_after = (rows.len() > size).then(|| id_of(&rows[size - 1]));
    Page {
        items: rows.into_iter().take(size).collect(),
        next_after,
    }
}

#[async_trait]
impl AgentRegistry for MemoryRegistry {
    async fn create(&self, id: &AgentId, fields: &AgentFields) -> Result<AgentRecord, RegistryError> {
        self.call("create");
        let mut agents = self.agents.lock().unwrap();
        if agents.iter().any(|a| a.definition.id.org() == id.org() && a.archived_at.is_none() && a.definition.name == fields.name) {
            return Err(RegistryError::NameTaken);
        }
        let record = AgentRecord {
            definition: AgentDefinition {
                id: id.clone(),
                name: fields.name.clone(),
                model_id: fields.model_id.clone(),
                instructions: fields.instructions.clone(),
                tool_names: fields.tool_names.clone(),
            },
            version: 1,
            created_at: Timestamp::UNIX_EPOCH,
            updated_at: Timestamp::UNIX_EPOCH,
            archived_at: None,
        };
        agents.push(record.clone());
        Ok(record)
    }

    async fn get(&self, id: Uuid) -> Result<Option<AgentRecord>, RegistryError> {
        self.call("get");
        Ok(self.agents.lock().unwrap().iter().find(|a| a.definition.id.uuid() == id).cloned())
    }

    async fn list(&self, org: Uuid, include_archived: bool, page: PageRequest) -> Result<Page<AgentRecord>, RegistryError> {
        self.call("list");
        let rows = self
            .agents
            .lock()
            .unwrap()
            .iter()
            .filter(|a| a.definition.id.org() == org && (include_archived || a.archived_at.is_none()))
            .cloned()
            .collect();
        Ok(page_of(rows, page, |a| a.definition.id.uuid()))
    }

    async fn update(&self, id: &AgentId, version: u64, fields: &AgentFields) -> Result<AgentRecord, RegistryError> {
        self.call("update");
        let mut agents = self.agents.lock().unwrap();
        let row = agents.iter_mut().find(|a| &a.definition.id == id).ok_or(RegistryError::NotFound)?;
        if row.archived_at.is_some() {
            return Err(RegistryError::Archived);
        }
        if row.version != version {
            return Err(RegistryError::VersionMismatch { current: row.version });
        }
        row.definition.name = fields.name.clone();
        row.definition.model_id = fields.model_id.clone();
        row.definition.instructions = fields.instructions.clone();
        row.definition.tool_names = fields.tool_names.clone();
        row.version += 1;
        Ok(row.clone())
    }

    async fn archive(&self, id: &AgentId) -> Result<AgentRecord, RegistryError> {
        self.call("archive");
        let mut agents = self.agents.lock().unwrap();
        let row = agents.iter_mut().find(|a| &a.definition.id == id).ok_or(RegistryError::NotFound)?;
        row.archived_at.get_or_insert(Timestamp::UNIX_EPOCH);
        Ok(row.clone())
    }

    async fn create_inbox(&self, id: &InboxId, agent: &AgentId, principal: &PrincipalId) -> Result<InboxRecord, RegistryError> {
        self.call("create_inbox");
        let mut inboxes = self.inboxes.lock().unwrap();
        if let Some(existing) = inboxes.iter().find(|i| i.inbox.agent() == agent && i.inbox.principal() == principal) {
            return Err(RegistryError::InboxExists(existing.inbox.id().clone()));
        }
        let record = InboxRecord {
            inbox: Inbox::new(id.clone(), agent.clone(), principal.clone(), Timestamp::UNIX_EPOCH).map_err(|e| RegistryError::Store(e.to_string()))?,
            agent_archived_at: None,
        };
        inboxes.push(record.clone());
        Ok(record)
    }

    async fn get_inbox(&self, id: Uuid) -> Result<Option<InboxRecord>, RegistryError> {
        self.call("get_inbox");
        Ok(self.inboxes.lock().unwrap().iter().find(|i| i.inbox.id().uuid() == id).cloned())
    }

    async fn list_inboxes(&self, agent: &AgentId, page: PageRequest) -> Result<Page<InboxRecord>, RegistryError> {
        self.call("list_inboxes");
        let rows = self.inboxes.lock().unwrap().iter().filter(|i| i.inbox.agent() == agent).cloned().collect();
        Ok(page_of(rows, page, |i| i.inbox.id().uuid()))
    }
}

/// An `AgentRegistry` that panics on every call. It proves that a request reached no registry.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub struct UnreachableRegistry;

#[async_trait]
impl AgentRegistry for UnreachableRegistry {
    async fn create(&self, _: &AgentId, _: &AgentFields) -> Result<AgentRecord, RegistryError> {
        panic!("the registry must not be called: create");
    }
    async fn get(&self, _: Uuid) -> Result<Option<AgentRecord>, RegistryError> {
        panic!("the registry must not be called: get");
    }
    async fn list(&self, _: Uuid, _: bool, _: PageRequest) -> Result<Page<AgentRecord>, RegistryError> {
        panic!("the registry must not be called: list");
    }
    async fn update(&self, _: &AgentId, _: u64, _: &AgentFields) -> Result<AgentRecord, RegistryError> {
        panic!("the registry must not be called: update");
    }
    async fn archive(&self, _: &AgentId) -> Result<AgentRecord, RegistryError> {
        panic!("the registry must not be called: archive");
    }
    async fn create_inbox(&self, _: &InboxId, _: &AgentId, _: &PrincipalId) -> Result<InboxRecord, RegistryError> {
        panic!("the registry must not be called: create_inbox");
    }
    async fn get_inbox(&self, _: Uuid) -> Result<Option<InboxRecord>, RegistryError> {
        panic!("the registry must not be called: get_inbox");
    }
    async fn list_inboxes(&self, _: &AgentId, _: PageRequest) -> Result<Page<InboxRecord>, RegistryError> {
        panic!("the registry must not be called: list_inboxes");
    }
}

/// The capabilities of the tests: the model `m1` and the tool `memory`.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub struct FixedCapabilities;

impl RunCapabilities for FixedCapabilities {
    fn has_model(&self, model: &str) -> bool {
        model == "m1"
    }

    fn has_tool(&self, tool: &str) -> bool {
        tool == "memory"
    }

    fn model_ids(&self) -> Vec<String> {
        vec!["m1".to_owned()]
    }
}

/// Increasing ids from `0x1000`.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
#[derive(Default)]
pub struct CountingIds(AtomicU64);

impl IdSource for CountingIds {
    fn new_uuid(&self) -> Uuid {
        Uuid::from_u128(0x1000 + u128::from(self.0.fetch_add(1, Ordering::SeqCst)))
    }
}
