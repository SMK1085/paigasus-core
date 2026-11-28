// SPDX-License-Identifier: Apache-2.0

//! The unit tests of `AgentAdmin`, with in-module fakes of the IAM port and the registry port.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use jiff::Timestamp;
use paigasus_agent_host_core::agent::AgentDefinition;
use paigasus_agent_host_core::inbox::Inbox;
use paigasus_agent_host_core::registry::{Page, RegistryError};
use paigasus_proto::paigasus::iam::v1::{IntrospectApiKeyResponse, IntrospectResponse};
use tonic::Status;

use super::page_token::encode;
use super::*;
use crate::adapters::iam::{CallerCredential, IamError};
use crate::application::auth::CallerKind;

const TOKEN: &str = "caller-secret";

fn u(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn org() -> Uuid {
    u(0x500)
}

fn other_org() -> Uuid {
    u(0x600)
}

fn caller_id() -> Uuid {
    u(0x10)
}

fn principal_prn(id: Uuid) -> String {
    format!("prn:pgs:iam:::principal/{id}")
}

fn org_prn(id: Uuid) -> String {
    format!("prn:pgs:iam:::organization/{id}")
}

// ---- the fakes ------------------------------------------------------------------------------

type SelfQuery = (CallerCredential, String, String, String);

/// How the fake IAM answers a self-query.
#[derive(Clone, Copy)]
enum Answer {
    /// `true` for the listed actions, on every org.
    Grants(&'static [&'static str]),
    /// `Unavailable` for every self-query.
    Outage,
}

/// A recording fake of the `Iam` port: the shape of `FakeIam` in `adapters::http::auth`. It
/// records each self-query as `(credential, principal PRN, action, org PRN)`.
struct RecordingIam {
    answer: Answer,
    self_queries: Mutex<Vec<SelfQuery>>,
}

impl RecordingIam {
    fn new(answer: Answer) -> Self {
        Self {
            answer,
            self_queries: Mutex::new(Vec::new()),
        }
    }

    fn self_queries(&self) -> Vec<SelfQuery> {
        self.self_queries.lock().unwrap().clone()
    }

    fn clear(&self) {
        self.self_queries.lock().unwrap().clear();
    }
}

#[async_trait]
impl Iam for RecordingIam {
    async fn introspect_api_key(&self, _token: &str) -> Result<IntrospectApiKeyResponse, IamError> {
        panic!("AgentAdmin must not authenticate");
    }

    async fn introspect_token(&self, _token: &str) -> Result<IntrospectResponse, IamError> {
        panic!("AgentAdmin must not authenticate");
    }

    async fn is_authorized_self(&self, caller: &CallerCredential, principal_prn: &str, action: &str, resource_prn: &str) -> Result<bool, IamError> {
        self.self_queries
            .lock()
            .unwrap()
            .push((caller.clone(), principal_prn.to_owned(), action.to_owned(), resource_prn.to_owned()));
        match self.answer {
            Answer::Grants(actions) => Ok(actions.contains(&action)),
            Answer::Outage => Err(IamError::Rpc(Status::unavailable("fake"))),
        }
    }
}

/// An in-memory fake of the `AgentRegistry` port. It records the name of each call.
#[derive(Default)]
struct MemRegistry {
    agents: Mutex<Vec<AgentRecord>>,
    inboxes: Mutex<Vec<InboxRecord>>,
    calls: Mutex<Vec<&'static str>>,
}

impl MemRegistry {
    fn call(&self, name: &'static str) {
        self.calls.lock().unwrap().push(name);
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }

    fn writes(&self) -> Vec<&'static str> {
        self.calls().into_iter().filter(|c| !matches!(*c, "get" | "get_inbox" | "list" | "list_inboxes")).collect()
    }

    /// Store an agent directly, with no rule.
    fn seed_agent(&self, org: Uuid, id: Uuid, name: &str) -> AgentId {
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
    fn seed_inbox(&self, agent: &AgentId, id: Uuid, principal: Uuid) -> InboxId {
        let inbox = InboxId::new(agent.org(), id);
        self.inboxes.lock().unwrap().push(InboxRecord {
            inbox: Inbox::new(inbox.clone(), agent.clone(), PrincipalId::from_uuid(principal), Timestamp::UNIX_EPOCH).expect("inbox"),
            agent_archived_at: None,
        });
        inbox
    }
}

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
impl AgentRegistry for MemRegistry {
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

/// The capabilities of the tests: the model `m1` and the tool `memory`.
struct FakeCapabilities;

impl RunCapabilities for FakeCapabilities {
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
#[derive(Default)]
struct CountingIds(AtomicU64);

impl IdSource for CountingIds {
    fn new_uuid(&self) -> Uuid {
        u(0x1000 + u128::from(self.0.fetch_add(1, Ordering::SeqCst)))
    }
}

struct Harness {
    admin: AgentAdmin,
    iam: Arc<RecordingIam>,
    registry: Arc<MemRegistry>,
}

fn harness(answer: Answer) -> Harness {
    let iam = Arc::new(RecordingIam::new(answer));
    let registry = Arc::new(MemRegistry::default());
    let admin = AgentAdmin::new(registry.clone(), Arc::new(FakeCapabilities), iam.clone(), Arc::new(CountingIds::default()));
    Harness { admin, iam, registry }
}

const BOTH: &[&str] = &["ManageAgents", "ReadAgents"];
const READ_ONLY: &[&str] = &["ReadAgents"];

/// An OIDC caller: no key scope rule applies.
fn user() -> Caller {
    Caller {
        principal: PrincipalId::from_uuid(caller_id()),
        credential: CallerCredential::Bearer(TOKEN.to_owned()),
        kind: CallerKind::Oidc { member_orgs: vec![org()] },
    }
}

/// An API key caller scoped to `scope_prn`.
fn key_of(scope_prn: String) -> Caller {
    Caller {
        principal: PrincipalId::from_uuid(caller_id()),
        credential: CallerCredential::ApiKey(TOKEN.to_owned()),
        kind: CallerKind::ApiKey { scope_prn },
    }
}

fn query(credential: CallerCredential, action: &str, org: Uuid) -> SelfQuery {
    (credential, principal_prn(caller_id()), action.to_owned(), org_prn(org))
}

fn bearer() -> CallerCredential {
    CallerCredential::Bearer(TOKEN.to_owned())
}

fn fields() -> AgentFields {
    AgentFields {
        name: "triage".to_owned(),
        model_id: "m1".to_owned(),
        instructions: "Be brief.".to_owned(),
        tool_names: vec!["memory".to_owned()],
    }
}

fn named(name: &str) -> AgentFields {
    AgentFields { name: name.to_owned(), ..fields() }
}

fn key(id: Uuid) -> ResourceKey {
    ResourceKey { id, org: None }
}

// ---- authorization (spec §6.5) --------------------------------------------------------------

#[tokio::test]
async fn create_self_queries_manage_agents_on_the_request_org() {
    let h = harness(Answer::Grants(BOTH));
    let record = h.admin.create(&user(), org(), fields()).await.expect("created");
    assert_eq!(h.iam.self_queries(), vec![query(bearer(), "ManageAgents", org())]);
    assert_eq!(record.definition.id, AgentId::new(org(), u(0x1000)), "the server makes the id in the request org");
    assert_eq!(record.definition.name, "triage");
    assert_eq!(h.registry.calls(), vec!["create"]);
}

#[tokio::test]
async fn list_self_queries_read_agents_on_the_request_org() {
    let h = harness(Answer::Grants(BOTH));
    h.registry.seed_agent(org(), u(0x2000), "a");
    h.registry.seed_agent(other_org(), u(0x2001), "b");
    let (items, next) = h.admin.list(&user(), org(), false, 0, "").await.expect("listed");
    assert_eq!(h.iam.self_queries(), vec![query(bearer(), "ReadAgents", org())]);
    assert_eq!(items.len(), 1);
    assert_eq!(next, None);
}

#[tokio::test]
async fn get_of_an_unknown_id_probes_the_nil_org_with_read_agents_and_is_forbidden() {
    // The probe says yes: the answer is still Forbidden.
    let h = harness(Answer::Grants(BOTH));
    assert_eq!(h.admin.get(&user(), key(u(0x999))).await, Err(AdminError::Forbidden));
    assert_eq!(h.iam.self_queries(), vec![query(bearer(), "ReadAgents", Uuid::nil())]);

    h.iam.clear();
    assert_eq!(h.admin.get_inbox(&user(), key(u(0x999))).await, Err(AdminError::Forbidden));
    assert_eq!(h.iam.self_queries(), vec![query(bearer(), "ReadAgents", Uuid::nil())]);

    // An unknown agent never gives an empty page of inboxes.
    h.iam.clear();
    assert_eq!(h.admin.list_inboxes(&user(), key(u(0x999)), 0, "").await, Err(AdminError::Forbidden));
    assert_eq!(h.iam.self_queries(), vec![query(bearer(), "ReadAgents", Uuid::nil())]);

    // A write probes with the write action.
    for call in ["update", "archive", "create_inbox"] {
        h.iam.clear();
        let got = match call {
            "update" => h.admin.update(&user(), key(u(0x999)), 1, fields()).await.map(|_| ()),
            "archive" => h.admin.archive(&user(), key(u(0x999))).await.map(|_| ()),
            _ => h.admin.create_inbox(&user(), key(u(0x999)), PrincipalId::from_uuid(u(0x77))).await.map(|_| ()),
        };
        assert_eq!(got, Err(AdminError::Forbidden), "{call}");
        assert_eq!(h.iam.self_queries(), vec![query(bearer(), "ManageAgents", Uuid::nil())], "{call}");
    }
    assert!(h.registry.writes().is_empty(), "{:?}", h.registry.calls());
}

#[tokio::test]
async fn get_of_an_unknown_id_during_an_outage_is_iam_unavailable() {
    let h = harness(Answer::Outage);
    let agent = h.registry.seed_agent(org(), u(0x2000), "a");
    assert_eq!(h.admin.get(&user(), key(u(0x999))).await, Err(AdminError::IamUnavailable));
    // The same answer as for an agent that exists.
    assert_eq!(h.admin.get(&user(), key(agent.uuid())).await, Err(AdminError::IamUnavailable));
    assert_eq!(h.admin.update(&user(), key(u(0x999)), 1, fields()).await, Err(AdminError::IamUnavailable));
}

#[tokio::test]
async fn get_with_an_agent_key_of_another_org_probes_like_an_unknown_id() {
    let h = harness(Answer::Grants(BOTH));
    let agent = h.registry.seed_agent(org(), u(0x2000), "a");
    let inbox = h.registry.seed_inbox(&agent, u(0x3000), u(0x77));
    let wrong = |id| ResourceKey { id, org: Some(other_org()) };

    assert_eq!(h.admin.get(&user(), wrong(agent.uuid())).await, Err(AdminError::Forbidden));
    assert_eq!(h.iam.self_queries(), vec![query(bearer(), "ReadAgents", Uuid::nil())]);

    h.iam.clear();
    assert_eq!(h.admin.get_inbox(&user(), wrong(inbox.uuid())).await, Err(AdminError::Forbidden));
    assert_eq!(h.iam.self_queries(), vec![query(bearer(), "ReadAgents", Uuid::nil())]);

    h.iam.clear();
    assert_eq!(h.admin.list_inboxes(&user(), wrong(agent.uuid()), 0, "").await, Err(AdminError::Forbidden));
    assert_eq!(h.iam.self_queries(), vec![query(bearer(), "ReadAgents", Uuid::nil())]);

    h.iam.clear();
    assert_eq!(h.admin.archive(&user(), wrong(agent.uuid())).await, Err(AdminError::Forbidden));
    assert_eq!(h.iam.self_queries(), vec![query(bearer(), "ManageAgents", Uuid::nil())]);
    assert!(h.registry.writes().is_empty(), "{:?}", h.registry.calls());

    // The control: the right org passes, and the self-query names the row org.
    h.iam.clear();
    let right = ResourceKey { id: agent.uuid(), org: Some(org()) };
    assert_eq!(h.admin.get(&user(), right).await.expect("found").definition.id, agent);
    assert_eq!(h.iam.self_queries(), vec![query(bearer(), "ReadAgents", org())]);
    let right_inbox = ResourceKey { id: inbox.uuid(), org: Some(org()) };
    assert_eq!(h.admin.get_inbox(&user(), right_inbox).await.expect("found").inbox.id(), &inbox);
}

#[tokio::test]
async fn a_key_scoped_to_another_org_is_forbidden_after_a_probe() {
    let h = harness(Answer::Grants(BOTH));
    let agent = h.registry.seed_agent(org(), u(0x2000), "a");
    let caller = key_of(org_prn(other_org()));
    let key_cred = CallerCredential::ApiKey(TOKEN.to_owned());

    assert_eq!(h.admin.get(&caller, key(agent.uuid())).await, Err(AdminError::Forbidden));
    assert_eq!(h.iam.self_queries(), vec![query(key_cred.clone(), "ReadAgents", org())], "the probe runs against the target org");

    h.iam.clear();
    assert_eq!(h.admin.create(&caller, org(), fields()).await, Err(AdminError::Forbidden));
    assert_eq!(h.iam.self_queries(), vec![query(key_cred.clone(), "ManageAgents", org())]);
    assert!(h.registry.writes().is_empty(), "{:?}", h.registry.calls());

    // The controls: a key of the org and a Root key pass.
    for scope in [org_prn(org()), "prn:pgs:iam:::root/00000000-0000-0000-0000-000000000000".to_owned()] {
        assert!(h.admin.get(&key_of(scope.clone()), key(agent.uuid())).await.is_ok(), "{scope}");
    }
}

#[tokio::test]
async fn a_key_scoped_to_another_org_during_an_outage_is_iam_unavailable() {
    let h = harness(Answer::Outage);
    let agent = h.registry.seed_agent(org(), u(0x2000), "a");
    let caller = key_of(org_prn(other_org()));
    assert_eq!(h.admin.get(&caller, key(agent.uuid())).await, Err(AdminError::IamUnavailable));
    assert_eq!(h.admin.create(&caller, org(), fields()).await, Err(AdminError::IamUnavailable));
    assert_eq!(h.admin.list(&caller, org(), false, 0, "").await, Err(AdminError::IamUnavailable));
}

#[tokio::test]
async fn read_agents_without_manage_agents_is_forbidden_on_every_write() {
    let h = harness(Answer::Grants(READ_ONLY));
    let agent = h.registry.seed_agent(org(), u(0x2000), "a");
    assert_eq!(h.admin.create(&user(), org(), fields()).await, Err(AdminError::Forbidden));
    assert_eq!(h.admin.update(&user(), key(agent.uuid()), 1, fields()).await, Err(AdminError::Forbidden));
    assert_eq!(h.admin.archive(&user(), key(agent.uuid())).await, Err(AdminError::Forbidden));
    assert_eq!(h.admin.create_inbox(&user(), key(agent.uuid()), PrincipalId::from_uuid(u(0x77))).await, Err(AdminError::Forbidden));
    assert!(h.registry.writes().is_empty(), "{:?}", h.registry.calls());
    let actions: Vec<String> = h.iam.self_queries().into_iter().map(|q| q.2).collect();
    assert_eq!(actions, ["ManageAgents"; 4]);
    // The read right still reads.
    assert!(h.admin.get(&user(), key(agent.uuid())).await.is_ok());
}

#[tokio::test]
async fn a_refused_caller_gets_forbidden_also_for_invalid_fields() {
    let h = harness(Answer::Grants(READ_ONLY));
    let agent = h.registry.seed_agent(org(), u(0x2000), "a");
    let bad = [
        named(""),
        AgentFields {
            model_id: "nope".to_owned(),
            ..fields()
        },
        AgentFields {
            tool_names: vec!["shell".to_owned()],
            ..fields()
        },
    ];
    for f in bad {
        assert_eq!(h.admin.create(&user(), org(), f.clone()).await, Err(AdminError::Forbidden), "{f:?}");
        assert_eq!(h.admin.update(&user(), key(agent.uuid()), 1, f.clone()).await, Err(AdminError::Forbidden), "{f:?}");
    }
    // A refused list gets Forbidden also for a page size or a token that is not valid.
    let refused = harness(Answer::Grants(&[]));
    assert_eq!(refused.admin.list(&user(), org(), false, 101, "garbage").await, Err(AdminError::Forbidden));
}

// ---- the field rules ------------------------------------------------------------------------

#[tokio::test]
async fn each_name_rule_is_invalid_field_name() {
    let h = harness(Answer::Grants(BOTH));
    let agent = h.registry.seed_agent(org(), u(0x2000), "a");
    for name in [
        String::new(),
        "a".repeat(65),
        "é".repeat(65),
        "\u{7}".to_owned(),
        "\0".to_owned(),
        "a\u{85}b".to_owned(),
        " a".to_owned(),
        "a ".to_owned(),
        "a\n".to_owned(),
        "\u{3000}a".to_owned(),
    ] {
        assert_eq!(h.admin.create(&user(), org(), named(&name)).await, Err(AdminError::InvalidField("name")), "{name:?}");
        assert_eq!(h.admin.update(&user(), key(agent.uuid()), 1, named(&name)).await, Err(AdminError::InvalidField("name")), "{name:?}");
    }
    assert!(h.registry.writes().is_empty(), "{:?}", h.registry.calls());
}

#[tokio::test]
async fn the_name_limits_are_inclusive() {
    let h = harness(Answer::Grants(BOTH));
    for name in ["a".to_owned(), "b".repeat(64), "é".repeat(64), "a b".to_owned()] {
        let record = h.admin.create(&user(), org(), named(&name)).await.unwrap_or_else(|e| panic!("{name:?}: {e:?}"));
        assert_eq!(record.definition.name, name);
    }
}

#[tokio::test]
async fn instructions_over_32768_bytes_or_empty_or_with_nul_are_invalid() {
    let h = harness(Answer::Grants(BOTH));
    let with = |instructions: String| AgentFields { instructions, ..fields() };
    // 16385 two-byte chars are 32770 bytes: the limit counts bytes, not chars.
    for instructions in [String::new(), "a".repeat(32_769), "é".repeat(16_385), "a\0b".to_owned()] {
        let len = instructions.len();
        assert_eq!(h.admin.create(&user(), org(), with(instructions)).await, Err(AdminError::InvalidField("instructions")), "{len} bytes");
    }
    for (i, instructions) in ["a".repeat(32_768), "é".repeat(16_384), "x".to_owned()].into_iter().enumerate() {
        let f = AgentFields {
            name: format!("n{i}"),
            ..with(instructions)
        };
        assert!(h.admin.create(&user(), org(), f).await.is_ok(), "row {i}");
    }
}

#[tokio::test]
async fn a_duplicate_tool_is_invalid_field_tools() {
    let h = harness(Answer::Grants(BOTH));
    for tools in [vec!["memory", "memory"], vec!["shell", "shell"]] {
        let f = AgentFields {
            tool_names: tools.iter().map(|t| (*t).to_owned()).collect(),
            ..fields()
        };
        assert_eq!(h.admin.create(&user(), org(), f).await, Err(AdminError::InvalidField("tools")), "{tools:?}");
    }
    // No tool at all is allowed.
    let f = AgentFields { tool_names: vec![], ..fields() };
    assert!(h.admin.create(&user(), org(), f).await.is_ok());
}

#[tokio::test]
async fn an_unknown_model_is_unknown_model_with_the_name() {
    let h = harness(Answer::Grants(BOTH));
    let f = AgentFields {
        model_id: "gpt-x".to_owned(),
        ..fields()
    };
    assert_eq!(h.admin.create(&user(), org(), f.clone()).await, Err(AdminError::UnknownModel("gpt-x".to_owned())));
    // The field rules come first.
    let f = AgentFields { name: String::new(), ..f };
    assert_eq!(h.admin.create(&user(), org(), f).await, Err(AdminError::InvalidField("name")));
    let agent = h.registry.seed_agent(org(), u(0x2000), "a");
    let f = AgentFields {
        model_id: "gpt-x".to_owned(),
        ..fields()
    };
    assert_eq!(h.admin.update(&user(), key(agent.uuid()), 1, f).await, Err(AdminError::UnknownModel("gpt-x".to_owned())));
    assert!(h.registry.writes().is_empty(), "{:?}", h.registry.calls());
}

#[tokio::test]
async fn an_unknown_tool_is_unknown_tool_with_the_name() {
    let h = harness(Answer::Grants(BOTH));
    let f = AgentFields {
        tool_names: vec!["memory".to_owned(), "shell".to_owned(), "web".to_owned()],
        ..fields()
    };
    assert_eq!(
        h.admin.create(&user(), org(), f.clone()).await,
        Err(AdminError::UnknownTool("shell".to_owned())),
        "the first unknown tool"
    );
    // The model comes before the tools.
    let f = AgentFields { model_id: "gpt-x".to_owned(), ..f };
    assert_eq!(h.admin.create(&user(), org(), f).await, Err(AdminError::UnknownModel("gpt-x".to_owned())));
    assert!(h.registry.writes().is_empty(), "{:?}", h.registry.calls());
}

// ---- pages ----------------------------------------------------------------------------------

#[tokio::test]
async fn page_size_0_is_50_and_101_is_invalid_pagination() {
    let h = harness(Answer::Grants(BOTH));
    let agent = h.registry.seed_agent(org(), u(0x2000), "a0");
    for n in 1..60 {
        h.registry.seed_agent(org(), u(0x2000 + n), &format!("a{n}"));
        h.registry.seed_inbox(&agent, u(0x3000 + n), u(0x4000 + n));
    }
    h.registry.seed_inbox(&agent, u(0x3000), u(0x4000));

    let (items, next) = h.admin.list(&user(), org(), false, 0, "").await.expect("listed");
    assert_eq!(items.len(), 50);
    assert!(next.is_some());
    let (items, next) = h.admin.list(&user(), org(), false, 100, "").await.expect("listed");
    assert_eq!((items.len(), next), (60, None));
    let (items, _) = h.admin.list(&user(), org(), false, 1, "").await.expect("listed");
    assert_eq!(items.len(), 1);
    let (items, _) = h.admin.list_inboxes(&user(), key(agent.uuid()), 0, "").await.expect("listed");
    assert_eq!(items.len(), 50);

    for size in [101, u32::MAX] {
        assert_eq!(h.admin.list(&user(), org(), false, size, "").await, Err(AdminError::InvalidPagination), "{size}");
        assert_eq!(h.admin.list_inboxes(&user(), key(agent.uuid()), size, "").await, Err(AdminError::InvalidPagination), "{size}");
    }
}

#[tokio::test]
async fn a_page_token_of_another_org_or_another_filter_is_invalid_page_token() {
    let h = harness(Answer::Grants(BOTH));
    let agent = h.registry.seed_agent(org(), u(0x2000), "a");
    let other = h.registry.seed_agent(org(), u(0x2001), "b");
    let token = encode(org(), false, u(0x2000));
    assert!(h.admin.list(&user(), org(), false, 0, &token).await.is_ok(), "the control");
    assert_eq!(h.admin.list(&user(), other_org(), false, 0, &token).await, Err(AdminError::InvalidPageToken));
    assert_eq!(h.admin.list(&user(), org(), true, 0, &token).await, Err(AdminError::InvalidPageToken));
    // An agent list token is not an inbox list token, and an inbox token is per agent.
    assert_eq!(h.admin.list_inboxes(&user(), key(agent.uuid()), 0, &token).await, Err(AdminError::InvalidPageToken));
    let inbox_token = encode(agent.uuid(), false, u(0x3000));
    assert!(h.admin.list_inboxes(&user(), key(agent.uuid()), 0, &inbox_token).await.is_ok(), "the control");
    assert_eq!(h.admin.list_inboxes(&user(), key(other.uuid()), 0, &inbox_token).await, Err(AdminError::InvalidPageToken));
    let archived_flag = encode(agent.uuid(), true, u(0x3000));
    assert_eq!(h.admin.list_inboxes(&user(), key(agent.uuid()), 0, &archived_flag).await, Err(AdminError::InvalidPageToken));
}

#[tokio::test]
async fn a_garbled_page_token_is_invalid_page_token() {
    let h = harness(Answer::Grants(BOTH));
    let agent = h.registry.seed_agent(org(), u(0x2000), "a");
    for token in ["garbage", "v1", "v1...", "v1.x.0.y"] {
        assert_eq!(h.admin.list(&user(), org(), false, 0, token).await, Err(AdminError::InvalidPageToken), "{token:?}");
        assert_eq!(h.admin.list_inboxes(&user(), key(agent.uuid()), 0, token).await, Err(AdminError::InvalidPageToken), "{token:?}");
    }
    assert!(!h.registry.calls().contains(&"list"), "a bad token reaches no list");
}

#[tokio::test]
async fn the_next_page_token_round_trips() {
    let h = harness(Answer::Grants(BOTH));
    let agent = h.registry.seed_agent(org(), u(0x2000), "a0");
    for n in 1..5 {
        h.registry.seed_agent(org(), u(0x2000 + n), &format!("a{n}"));
    }
    for n in 0..3 {
        h.registry.seed_inbox(&agent, u(0x3000 + n), u(0x4000 + n));
    }

    let mut seen = Vec::new();
    let mut token = String::new();
    let mut pages = 0;
    loop {
        let (items, next) = h.admin.list(&user(), org(), false, 2, &token).await.expect("listed");
        pages += 1;
        seen.extend(items.iter().map(|a| a.definition.id.uuid()));
        match next {
            Some(next) => token = next,
            None => break,
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(seen, (0..5).map(|n| u(0x2000 + n)).collect::<Vec<_>>());

    let (first, next) = h.admin.list_inboxes(&user(), key(agent.uuid()), 2, "").await.expect("listed");
    let next = next.expect("a second page");
    assert_eq!(next, encode(agent.uuid(), false, u(0x3001)));
    let (second, last) = h.admin.list_inboxes(&user(), key(agent.uuid()), 2, &next).await.expect("listed");
    assert_eq!(first.len() + second.len(), 3);
    assert_eq!(second[0].inbox.id().uuid(), u(0x3002));
    assert_eq!(last, None);
}

// ---- the registry outcomes ------------------------------------------------------------------

#[tokio::test]
async fn the_writes_pass_the_registry_refusals_through() {
    let h = harness(Answer::Grants(BOTH));
    let created = h.admin.create(&user(), org(), fields()).await.expect("created");
    let id = key(created.definition.id.uuid());
    assert_eq!(h.admin.create(&user(), org(), fields()).await, Err(AdminError::NameTaken));
    assert_eq!(h.admin.update(&user(), id, 9, fields()).await, Err(AdminError::VersionMismatch { current: 1 }));
    let updated = h.admin.update(&user(), id, 1, named("renamed")).await.expect("updated");
    assert_eq!((updated.version, updated.definition.name.as_str()), (2, "renamed"));

    let principal = PrincipalId::from_uuid(u(0x77));
    let inbox = h.admin.create_inbox(&user(), id, principal.clone()).await.expect("bound");
    assert_eq!(
        inbox.inbox.id(),
        &InboxId::new(org(), u(0x1002)),
        "the server makes the inbox id in the agent org (the refused create used 0x1001)"
    );
    assert_eq!(inbox.inbox.agent(), &created.definition.id);
    assert_eq!(h.admin.create_inbox(&user(), id, principal).await, Err(AdminError::InboxExists(inbox.inbox.id().clone())));

    let archived = h.admin.archive(&user(), id).await.expect("archived");
    assert!(archived.archived_at.is_some());
    assert_eq!(h.admin.update(&user(), id, 2, fields()).await, Err(AdminError::Archived));
}
