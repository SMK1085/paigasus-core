// SPDX-License-Identifier: Apache-2.0

//! The gRPC `AgentService` (SMA-852 spec §6.5). Each RPC authenticates the caller through the IAM
//! link of `AgentAdmin` (the code of ingress, not a copy), maps the request and calls
//! `AgentAdmin`, which owns the authorization and the rules.
//!
//! The order of a call: the slot (before boot installs the admin, `UNAVAILABLE`), the
//! authentication (before any registry read), the PRN fields, then `AgentAdmin`.

use std::future::Future;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use jiff::Timestamp;
use paigasus_agent_host_core::ids::{AgentId, InboxId, PrincipalId};
use paigasus_agent_host_core::registry::{AgentFields, AgentRecord, InboxRecord};
use paigasus_proto::paigasus::agents::v1::agent_service_server::AgentService;
use paigasus_proto::paigasus::agents::v1::{
    Agent, ArchiveAgentRequest, ArchiveAgentResponse, CreateAgentRequest, CreateAgentResponse, CreateInboxRequest, CreateInboxResponse, GetAgentRequest, GetAgentResponse, GetInboxRequest,
    GetInboxResponse, Inbox, ListAgentsRequest, ListAgentsResponse, ListInboxesRequest, ListInboxesResponse, UpdateAgentRequest, UpdateAgentResponse,
};
use tonic::{Request, Response, Status};
use uuid::Uuid;

use super::status::{from_api_error, migrating, to_status};
use crate::adapters::http::agents::dto::{org_prn, parse_org_prn};
use crate::adapters::http::auth::{authenticate, bearer_token};
use crate::application::agents::{AdminError, AgentAdmin, ResourceKey};
use crate::application::auth::Caller;

/// The service name of the metrics.
const SERVICE: &str = "AgentService";

/// Holds the admin once boot installs it. Before that, each RPC is `UNAVAILABLE` with the
/// `service-migrating` reason, as the HTTP boot slot answers 503. A clone shares the slot.
#[derive(Clone, Default)]
pub struct AgentGrpcSlot(Arc<OnceLock<Arc<AgentAdmin>>>);

impl AgentGrpcSlot {
    /// Fills the slot. This is the only way to become ready. `install` has one call site, so a
    /// second call is a wiring defect.
    pub fn install(&self, admin: Arc<AgentAdmin>) -> anyhow::Result<()> {
        self.0.set(admin).map_err(|_| anyhow::anyhow!("the gRPC agent slot was already installed"))
    }

    /// The common path of an RPC: the slot, the authentication, then `f` with the admin, the
    /// caller and the message. It records the call in the metrics.
    async fn call<T, R, F, Fut>(&self, method: &'static str, request: Request<T>, f: F) -> Result<Response<R>, Status>
    where
        F: FnOnce(Arc<AgentAdmin>, Caller, T) -> Fut,
        Fut: Future<Output = Result<R, AdminError>>,
    {
        let started = Instant::now();
        let result = self.guarded(request, f).await;
        paigasus_observability::record_grpc(SERVICE, method, started, &result);
        result
    }

    async fn guarded<T, R, F, Fut>(&self, request: Request<T>, f: F) -> Result<Response<R>, Status>
    where
        F: FnOnce(Arc<AgentAdmin>, Caller, T) -> Fut,
        Fut: Future<Output = Result<R, AdminError>>,
    {
        let admin = Arc::clone(self.0.get().ok_or_else(migrating)?);
        let token = bearer_token(&request.metadata().clone().into_headers()).map_err(|e| to_status(&from_api_error(e)))?;
        let caller = authenticate(admin.iam().as_ref(), &token).await.map_err(|e| to_status(&from_api_error(e)))?;
        f(admin, caller, request.into_inner()).await.map(Response::new).map_err(|e| to_status(&e))
    }
}

fn invalid(field: &'static str) -> AdminError {
    AdminError::InvalidField(field)
}

/// The org of an org PRN. The error names the field of the request.
fn org_of(raw: &str) -> Result<Uuid, AdminError> {
    parse_org_prn(raw).map_err(|_| invalid("org_prn"))
}

/// The key of an agent PRN. The PRN names the org, so a row of another org is an unknown id.
fn agent_key(raw: &str) -> Result<ResourceKey, AdminError> {
    let agent = AgentId::parse(raw).map_err(|_| invalid("agent_prn"))?;
    Ok(ResourceKey {
        id: agent.uuid(),
        org: Some(agent.org()),
    })
}

/// The key of an inbox PRN.
fn inbox_key(raw: &str) -> Result<ResourceKey, AdminError> {
    let inbox = InboxId::parse(raw).map_err(|_| invalid("inbox_prn"))?;
    Ok(ResourceKey {
        id: inbox.uuid(),
        org: Some(inbox.org()),
    })
}

fn fields(name: String, model: String, instructions: String, tools: Vec<String>) -> AgentFields {
    AgentFields {
        name,
        model_id: model,
        instructions,
        tool_names: tools,
    }
}

/// Sets a protobuf timestamp field. The type is named by the field that it fills, so this crate
/// needs no dependency on `prost-types`.
macro_rules! set_time {
    ($slot:expr, $time:expr) => {{
        let time: Timestamp = $time;
        $slot = Some(Default::default());
        if let Some(t) = $slot.as_mut() {
            t.seconds = time.as_second();
            t.nanos = time.subsec_nanosecond();
        }
    }};
}

fn agent_message(record: &AgentRecord) -> Agent {
    let def = &record.definition;
    let mut agent = Agent {
        agent_prn: def.id.as_prn().canonical(),
        org_prn: org_prn(def.id.org()),
        name: def.name.clone(),
        model: def.model_id.clone(),
        instructions: def.instructions.clone(),
        tools: def.tool_names.clone(),
        version: i64::try_from(record.version).unwrap_or(i64::MAX),
        create_time: None,
        update_time: None,
        archive_time: None,
    };
    set_time!(agent.create_time, record.created_at);
    set_time!(agent.update_time, record.updated_at);
    if let Some(at) = record.archived_at {
        set_time!(agent.archive_time, at);
    }
    agent
}

fn inbox_message(record: &InboxRecord) -> Inbox {
    let inbox = &record.inbox;
    let mut out = Inbox {
        inbox_prn: inbox.id().as_prn().canonical(),
        agent_prn: inbox.agent().as_prn().canonical(),
        principal_prn: inbox.principal().as_prn().canonical(),
        create_time: None,
        agent_archive_time: None,
    };
    set_time!(out.create_time, inbox.created_at());
    if let Some(at) = record.agent_archived_at {
        set_time!(out.agent_archive_time, at);
    }
    out
}

/// The next page token: empty on the last page.
fn token(next: Option<String>) -> String {
    next.unwrap_or_default()
}

#[tonic::async_trait]
impl AgentService for AgentGrpcSlot {
    async fn create_agent(&self, request: Request<CreateAgentRequest>) -> Result<Response<CreateAgentResponse>, Status> {
        self.call("CreateAgent", request, |admin, caller, m| async move {
            let org = org_of(&m.org_prn)?;
            let record = admin.create(&caller, org, fields(m.name, m.model, m.instructions, m.tools)).await?;
            Ok(CreateAgentResponse { agent: Some(agent_message(&record)) })
        })
        .await
    }

    async fn get_agent(&self, request: Request<GetAgentRequest>) -> Result<Response<GetAgentResponse>, Status> {
        self.call("GetAgent", request, |admin, caller, m| async move {
            let record = admin.get(&caller, agent_key(&m.agent_prn)?).await?;
            Ok(GetAgentResponse { agent: Some(agent_message(&record)) })
        })
        .await
    }

    async fn list_agents(&self, request: Request<ListAgentsRequest>) -> Result<Response<ListAgentsResponse>, Status> {
        self.call("ListAgents", request, |admin, caller, m| async move {
            let org = org_of(&m.org_prn)?;
            let (items, next) = admin.list(&caller, org, m.include_archived, m.page_size, &m.page_token).await?;
            Ok(ListAgentsResponse {
                agents: items.iter().map(agent_message).collect(),
                next_page_token: token(next),
            })
        })
        .await
    }

    async fn update_agent(&self, request: Request<UpdateAgentRequest>) -> Result<Response<UpdateAgentResponse>, Status> {
        self.call("UpdateAgent", request, |admin, caller, m| async move {
            let key = agent_key(&m.agent_prn)?;
            let version = u64::try_from(m.version).map_err(|_| invalid("version"))?;
            let record = admin.update(&caller, key, version, fields(m.name, m.model, m.instructions, m.tools)).await?;
            Ok(UpdateAgentResponse { agent: Some(agent_message(&record)) })
        })
        .await
    }

    async fn archive_agent(&self, request: Request<ArchiveAgentRequest>) -> Result<Response<ArchiveAgentResponse>, Status> {
        self.call("ArchiveAgent", request, |admin, caller, m| async move {
            let record = admin.archive(&caller, agent_key(&m.agent_prn)?).await?;
            Ok(ArchiveAgentResponse { agent: Some(agent_message(&record)) })
        })
        .await
    }

    async fn create_inbox(&self, request: Request<CreateInboxRequest>) -> Result<Response<CreateInboxResponse>, Status> {
        self.call("CreateInbox", request, |admin, caller, m| async move {
            let key = agent_key(&m.agent_prn)?;
            let principal = PrincipalId::parse(&m.principal_prn).map_err(|_| invalid("principal_prn"))?;
            let record = admin.create_inbox(&caller, key, principal).await?;
            Ok(CreateInboxResponse { inbox: Some(inbox_message(&record)) })
        })
        .await
    }

    async fn get_inbox(&self, request: Request<GetInboxRequest>) -> Result<Response<GetInboxResponse>, Status> {
        self.call("GetInbox", request, |admin, caller, m| async move {
            let record = admin.get_inbox(&caller, inbox_key(&m.inbox_prn)?).await?;
            Ok(GetInboxResponse { inbox: Some(inbox_message(&record)) })
        })
        .await
    }

    async fn list_inboxes(&self, request: Request<ListInboxesRequest>) -> Result<Response<ListInboxesResponse>, Status> {
        self.call("ListInboxes", request, |admin, caller, m| async move {
            let (items, next) = admin.list_inboxes(&caller, agent_key(&m.agent_prn)?, m.page_size, &m.page_token).await?;
            Ok(ListInboxesResponse {
                inboxes: items.iter().map(inbox_message).collect(),
                next_page_token: token(next),
            })
        })
        .await
    }
}
