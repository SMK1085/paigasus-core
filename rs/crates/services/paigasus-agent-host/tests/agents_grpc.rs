// SPDX-License-Identifier: Apache-2.0

//! The gRPC `AgentService` (SMA-852 spec §6.5, §6.6). Every test goes through a real tonic server
//! on a loopback port, with an in-memory registry and a stub IAM, so no test needs Docker.

mod support;

use std::sync::Arc;

use paigasus_agent_host::adapters::grpc::AgentGrpcSlot;
use paigasus_agent_host::adapters::iam::CallerCredential;
use paigasus_agent_host::application::agents::AgentAdmin;
use paigasus_agent_host_core::ids::{AgentId, InboxId};
use paigasus_agent_host_core::ports::AgentRegistry;
use paigasus_proto::paigasus::agents::v1::agent_service_client::AgentServiceClient;
use paigasus_proto::paigasus::agents::v1::agent_service_server::AgentServiceServer;
use paigasus_proto::paigasus::agents::v1::{ArchiveAgentRequest, CreateAgentRequest, CreateInboxRequest, GetAgentRequest, GetInboxRequest, ListAgentsRequest, ListInboxesRequest, UpdateAgentRequest};
use paigasus_proto::paigasus::common::v1::ErrorReason;
use support::memory_registry::{CountingIds, FixedCapabilities, MemoryRegistry, UnreachableRegistry};
use support::stub_iam::{StubIam, TOKEN, org_prn, principal_prn};
use tonic::metadata::MetadataValue;
use tonic::transport::Channel;
use tonic::{Code, Request, Status};
use tonic_types::StatusExt;
use uuid::Uuid;

const BOTH: &[&str] = &["ManageAgents", "ReadAgents"];
const READ_ONLY: &[&str] = &["ReadAgents"];

fn u(n: u128) -> Uuid {
    Uuid::from_u128(n)
}

fn org() -> Uuid {
    u(0x500)
}

fn other_org() -> Uuid {
    u(0x600)
}

fn caller() -> Uuid {
    u(0x10)
}

fn agent_uuid() -> Uuid {
    u(0x700)
}

fn inbox_uuid() -> Uuid {
    u(0x800)
}

fn key_iam() -> StubIam {
    StubIam::api_key(caller())
}

/// A registry with the agent and its inbox in `org`.
fn seeded(org: Uuid) -> Arc<MemoryRegistry> {
    let registry = Arc::new(MemoryRegistry::default());
    let agent = registry.seed_agent(org, agent_uuid(), "seeded");
    registry.seed_inbox(&agent, inbox_uuid(), u(0x30));
    registry
}

/// A tonic server on a loopback port and a client of it. `install` false leaves the slot empty.
async fn serve(registry: Arc<dyn AgentRegistry>, iam: &Arc<StubIam>, install: bool) -> AgentServiceClient<Channel> {
    let slot = AgentGrpcSlot::default();
    if install {
        slot.install(Arc::new(AgentAdmin::new(registry, Arc::new(FixedCapabilities), iam.clone(), Arc::new(CountingIds::default()))))
            .unwrap();
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .layer(paigasus_observability::CorrelationLayer)
            .add_service(AgentServiceServer::new(slot))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    AgentServiceClient::connect(format!("http://{addr}")).await.unwrap()
}

/// A request that carries the bearer token, or none.
fn req<T>(message: T, token: Option<&str>) -> Request<T> {
    let mut request = Request::new(message);
    if let Some(token) = token {
        request.metadata_mut().insert("authorization", MetadataValue::try_from(format!("Bearer {token}")).unwrap());
    }
    request
}

fn agent_prn(org: Uuid) -> String {
    AgentId::new(org, agent_uuid()).as_prn().canonical()
}

fn inbox_prn(org: Uuid) -> String {
    InboxId::new(org, inbox_uuid()).as_prn().canonical()
}

/// One RPC, with its IAM action.
#[derive(Clone, Copy, Debug)]
struct Rpc {
    name: &'static str,
    action: &'static str,
    /// `true` when the RPC names a row by PRN. Create and list name the org.
    by_id: bool,
}

const RPCS: [Rpc; 8] = [
    Rpc {
        name: "CreateAgent",
        action: "ManageAgents",
        by_id: false,
    },
    Rpc {
        name: "GetAgent",
        action: "ReadAgents",
        by_id: true,
    },
    Rpc {
        name: "ListAgents",
        action: "ReadAgents",
        by_id: false,
    },
    Rpc {
        name: "UpdateAgent",
        action: "ManageAgents",
        by_id: true,
    },
    Rpc {
        name: "ArchiveAgent",
        action: "ManageAgents",
        by_id: true,
    },
    Rpc {
        name: "CreateInbox",
        action: "ManageAgents",
        by_id: true,
    },
    Rpc {
        name: "GetInbox",
        action: "ReadAgents",
        by_id: true,
    },
    Rpc {
        name: "ListInboxes",
        action: "ReadAgents",
        by_id: true,
    },
];

/// Call `rpc`. The PRNs of the by-id calls carry `prn_org`; the org of create and list is
/// `org()`.
async fn call(client: &mut AgentServiceClient<Channel>, rpc: Rpc, prn_org: Uuid, token: Option<&str>) -> Result<(), Status> {
    let fields = |version: i64| UpdateAgentRequest {
        agent_prn: agent_prn(prn_org),
        version,
        name: "renamed".to_owned(),
        model: "m1".to_owned(),
        instructions: "Be brief.".to_owned(),
        tools: vec!["memory".to_owned()],
    };
    match rpc.name {
        "CreateAgent" => client
            .create_agent(req(
                CreateAgentRequest {
                    org_prn: org_prn(org()),
                    name: "helper".to_owned(),
                    model: "m1".to_owned(),
                    instructions: "Be brief.".to_owned(),
                    tools: vec!["memory".to_owned()],
                },
                token,
            ))
            .await
            .map(drop),
        "GetAgent" => client.get_agent(req(GetAgentRequest { agent_prn: agent_prn(prn_org) }, token)).await.map(drop),
        "ListAgents" => client
            .list_agents(req(
                ListAgentsRequest {
                    org_prn: org_prn(org()),
                    ..Default::default()
                },
                token,
            ))
            .await
            .map(drop),
        "UpdateAgent" => client.update_agent(req(fields(1), token)).await.map(drop),
        "ArchiveAgent" => client.archive_agent(req(ArchiveAgentRequest { agent_prn: agent_prn(prn_org) }, token)).await.map(drop),
        "CreateInbox" => client
            .create_inbox(req(
                CreateInboxRequest {
                    agent_prn: agent_prn(prn_org),
                    principal_prn: principal_prn(u(0x20)),
                },
                token,
            ))
            .await
            .map(drop),
        "GetInbox" => client.get_inbox(req(GetInboxRequest { inbox_prn: inbox_prn(prn_org) }, token)).await.map(drop),
        "ListInboxes" => client
            .list_inboxes(req(
                ListInboxesRequest {
                    agent_prn: agent_prn(prn_org),
                    ..Default::default()
                },
                token,
            ))
            .await
            .map(drop),
        other => panic!("unknown rpc {other}"),
    }
}

fn query(action: &str, org: Uuid) -> (CallerCredential, String, String, String) {
    (CallerCredential::ApiKey(TOKEN.to_owned()), principal_prn(caller()), action.to_owned(), org_prn(org))
}

/// The reason, the domain and the metadata of an error `Status`.
fn info(status: &Status) -> (String, String, std::collections::HashMap<String, String>) {
    let details = status.get_error_details();
    let info = details.error_info().expect("an ErrorInfo");
    (info.reason.clone(), info.domain.clone(), info.metadata.clone())
}

fn reason(r: ErrorReason) -> String {
    r.as_wire_reason().expect("a declared reason")
}

async fn run(rpc: Rpc, prn_org: Uuid, registry: Arc<dyn AgentRegistry>, iam: StubIam) -> (Result<(), Status>, Arc<StubIam>) {
    let iam = Arc::new(iam);
    let mut client = serve(registry, &iam, true).await;
    let result = call(&mut client, rpc, prn_org, Some(TOKEN)).await;
    (result, iam)
}

#[tokio::test]
async fn each_rpc_self_queries_the_expected_action_on_the_org() {
    for rpc in RPCS {
        let want = vec![query(rpc.action, org())];

        let (result, iam) = run(rpc, org(), seeded(org()), key_iam().granting(org(), BOTH)).await;
        result.unwrap_or_else(|e| panic!("{}: allowed: {e}", rpc.name));
        assert_eq!(iam.self_query_log(), want, "{}: allowed", rpc.name);

        let (result, iam) = run(rpc, org(), seeded(org()), key_iam().allowing(false)).await;
        let status = result.expect_err(rpc.name);
        assert_eq!(status.code(), Code::PermissionDenied, "{}: denied", rpc.name);
        assert_eq!(info(&status).0, reason(ErrorReason::Forbidden), "{}", rpc.name);
        assert_eq!(status.message(), "the caller may not perform this action", "{}", rpc.name);
        assert_eq!(iam.self_query_log(), want, "{}: denied", rpc.name);

        if rpc.action == "ManageAgents" {
            let (result, iam) = run(rpc, org(), seeded(org()), key_iam().granting(org(), READ_ONLY)).await;
            assert_eq!(result.expect_err(rpc.name).code(), Code::PermissionDenied, "{}: read-only grant", rpc.name);
            assert_eq!(iam.self_query_log(), want, "{}: read-only grant", rpc.name);
        }
    }
}

#[tokio::test]
async fn an_api_key_of_another_org_is_permission_denied_and_unavailable_during_an_outage() {
    for rpc in RPCS {
        let scope = org_prn(other_org());
        let want = vec![query(rpc.action, org())];

        // The probe says yes, and the answer is still PERMISSION_DENIED.
        let (result, iam) = run(rpc, org(), seeded(org()), key_iam().with_key_scope(&scope)).await;
        assert_eq!(result.expect_err(rpc.name).code(), Code::PermissionDenied, "{}", rpc.name);
        assert_eq!(iam.self_query_log(), want, "{}", rpc.name);

        let (result, iam) = run(rpc, org(), seeded(org()), key_iam().with_key_scope(&scope).self_query_outage()).await;
        let status = result.expect_err(rpc.name);
        assert_eq!(status.code(), Code::Unavailable, "{}", rpc.name);
        assert_eq!(info(&status).0, reason(ErrorReason::IamUnavailable), "{}", rpc.name);
        assert_eq!(iam.self_query_log(), want, "{}", rpc.name);
    }
}

#[tokio::test]
async fn an_unknown_id_is_permission_denied_after_a_probe_against_the_nil_org() {
    for rpc in RPCS.into_iter().filter(|r| r.by_id) {
        let want = vec![query(rpc.action, Uuid::nil())];

        let (result, iam) = run(rpc, org(), Arc::new(MemoryRegistry::default()), key_iam()).await;
        assert_eq!(result.expect_err(rpc.name).code(), Code::PermissionDenied, "{}", rpc.name);
        assert_eq!(iam.self_query_log(), want, "{}", rpc.name);

        let (result, iam) = run(rpc, org(), Arc::new(MemoryRegistry::default()), key_iam().self_query_outage()).await;
        assert_eq!(result.expect_err(rpc.name).code(), Code::Unavailable, "{}", rpc.name);
        assert_eq!(iam.self_query_log(), want, "{}", rpc.name);
    }
}

#[tokio::test]
async fn a_prn_whose_uuid_exists_in_another_org_is_permission_denied_after_a_nil_org_probe() {
    // The row is in `org()`. The PRN names the same uuid under `other_org()`. The plan decision:
    // that is an unknown id, and the probe runs against the nil org.
    for rpc in RPCS.into_iter().filter(|r| r.by_id) {
        let want = vec![query(rpc.action, Uuid::nil())];

        let (result, iam) = run(rpc, other_org(), seeded(org()), key_iam().granting(org(), BOTH)).await;
        let status = result.expect_err(rpc.name);
        assert_eq!(status.code(), Code::PermissionDenied, "{}", rpc.name);
        assert_eq!(info(&status).0, reason(ErrorReason::Forbidden), "{}", rpc.name);
        assert_eq!(iam.self_query_log(), want, "{}", rpc.name);

        let (result, iam) = run(rpc, other_org(), seeded(org()), key_iam().granting(org(), BOTH).self_query_outage()).await;
        assert_eq!(result.expect_err(rpc.name).code(), Code::Unavailable, "{}", rpc.name);
        assert_eq!(iam.self_query_log(), want, "{}", rpc.name);
    }
}

#[tokio::test]
async fn a_missing_authorization_metadata_is_unauthenticated_and_reads_nothing() {
    for rpc in RPCS {
        // No metadata at all.
        let iam = Arc::new(key_iam());
        let mut client = serve(Arc::new(UnreachableRegistry), &iam, true).await;
        let status = call(&mut client, rpc, org(), None).await.expect_err(rpc.name);
        assert_eq!(status.code(), Code::Unauthenticated, "{}", rpc.name);
        assert_eq!(info(&status).0, reason(ErrorReason::InvalidToken), "{}", rpc.name);
        assert_eq!(status.message(), "the bearer credential is missing or not valid", "{}", rpc.name);
        assert!(iam.calls().is_empty(), "{}: IAM was called", rpc.name);

        // A token that IAM refuses.
        let iam = Arc::new(StubIam::unprovisioned());
        let mut client = serve(Arc::new(UnreachableRegistry), &iam, true).await;
        let status = call(&mut client, rpc, org(), Some(TOKEN)).await.expect_err(rpc.name);
        assert_eq!(status.code(), Code::Unauthenticated, "{}", rpc.name);
        assert_eq!(iam.self_queries(), 0, "{}", rpc.name);
    }
}

#[tokio::test]
async fn before_install_every_rpc_is_unavailable_service_migrating() {
    for rpc in RPCS {
        let iam = Arc::new(key_iam());
        let mut client = serve(Arc::new(UnreachableRegistry), &iam, false).await;
        let status = call(&mut client, rpc, org(), Some(TOKEN)).await.expect_err(rpc.name);
        assert_eq!(status.code(), Code::Unavailable, "{}", rpc.name);
        let (reason_text, domain, _) = info(&status);
        assert_eq!(reason_text, reason(ErrorReason::ServiceMigrating), "{}", rpc.name);
        assert_eq!(domain, *paigasus_proto::error::AGENTS_DOMAIN, "{}", rpc.name);
        assert!(iam.calls().is_empty(), "{}: IAM was called", rpc.name);
    }
}

#[tokio::test]
async fn create_agent_wires_to_agent_admin_and_returns_the_agent() {
    let iam = Arc::new(key_iam().granting(org(), BOTH));
    let mut client = serve(Arc::new(MemoryRegistry::default()), &iam, true).await;
    let created = client
        .create_agent(req(
            CreateAgentRequest {
                org_prn: org_prn(org()),
                name: "helper".to_owned(),
                model: "m1".to_owned(),
                instructions: "Be brief.".to_owned(),
                tools: vec!["memory".to_owned()],
            },
            Some(TOKEN),
        ))
        .await
        .unwrap()
        .into_inner()
        .agent
        .unwrap();
    assert_eq!(created.agent_prn, AgentId::new(org(), u(0x1000)).as_prn().canonical());
    assert_eq!(created.org_prn, org_prn(org()));
    assert_eq!((created.name.as_str(), created.model.as_str(), created.instructions.as_str()), ("helper", "m1", "Be brief."));
    assert_eq!(created.tools, vec!["memory".to_owned()]);
    assert_eq!(created.version, 1);
    assert_eq!(created.create_time.unwrap().seconds, 0);
    assert!(created.archive_time.is_none());
}

#[tokio::test]
async fn get_agent_wires_to_agent_admin() {
    let iam = Arc::new(key_iam().granting(org(), BOTH));
    let mut client = serve(seeded(org()), &iam, true).await;
    let agent = client
        .get_agent(req(GetAgentRequest { agent_prn: agent_prn(org()) }, Some(TOKEN)))
        .await
        .unwrap()
        .into_inner()
        .agent
        .unwrap();
    assert_eq!(agent.agent_prn, agent_prn(org()));
    assert_eq!(agent.name, "seeded");
}

#[tokio::test]
async fn list_agents_wires_and_returns_the_next_page_token() {
    let registry = Arc::new(MemoryRegistry::default());
    for n in 1..=3 {
        registry.seed_agent(org(), u(0x700 + n), &format!("a{n}"));
    }
    let iam = Arc::new(key_iam().granting(org(), BOTH));
    let mut client = serve(registry, &iam, true).await;
    let list = |size: u32, token: &str| ListAgentsRequest {
        org_prn: org_prn(org()),
        include_archived: false,
        page_size: size,
        page_token: token.to_owned(),
    };
    let first = client.list_agents(req(list(2, ""), Some(TOKEN))).await.unwrap().into_inner();
    assert_eq!(first.agents.len(), 2);
    assert!(!first.next_page_token.is_empty());
    let second = client.list_agents(req(list(2, &first.next_page_token), Some(TOKEN))).await.unwrap().into_inner();
    assert_eq!(second.agents.len(), 1);
    assert!(second.next_page_token.is_empty());

    let status = client.list_agents(req(list(2, "junk"), Some(TOKEN))).await.unwrap_err();
    assert_eq!(status.code(), Code::InvalidArgument);
    assert_eq!(info(&status).0, reason(ErrorReason::InvalidCursor));
    assert_eq!(status.message(), "the page token is not valid for this list");
    let status = client.list_agents(req(list(101, ""), Some(TOKEN))).await.unwrap_err();
    assert_eq!(info(&status).0, reason(ErrorReason::InvalidPagination));
}

#[tokio::test]
async fn update_agent_wires_and_a_stale_version_is_aborted_with_the_version_metadata() {
    let iam = Arc::new(key_iam().granting(org(), BOTH));
    let mut client = serve(seeded(org()), &iam, true).await;
    let update = |version: i64| UpdateAgentRequest {
        agent_prn: agent_prn(org()),
        version,
        name: "renamed".to_owned(),
        model: "m1".to_owned(),
        instructions: "i".to_owned(),
        tools: vec![],
    };
    let status = client.update_agent(req(update(5), Some(TOKEN))).await.unwrap_err();
    assert_eq!(status.code(), Code::Aborted);
    let (reason_text, domain, metadata) = info(&status);
    assert_eq!(reason_text, reason(ErrorReason::AgentVersionMismatch));
    assert_eq!(domain, *paigasus_proto::error::AGENTS_DOMAIN);
    assert_eq!(metadata.get("version").map(String::as_str), Some("1"));

    let updated = client.update_agent(req(update(1), Some(TOKEN))).await.unwrap().into_inner().agent.unwrap();
    assert_eq!((updated.name.as_str(), updated.version), ("renamed", 2));

    let status = client.update_agent(req(update(-1), Some(TOKEN))).await.unwrap_err();
    assert_eq!(status.code(), Code::InvalidArgument);
    assert_eq!(info(&status).2.get("field").map(String::as_str), Some("version"));
}

#[tokio::test]
async fn archive_agent_wires_and_sets_archive_time() {
    let iam = Arc::new(key_iam().granting(org(), BOTH));
    let mut client = serve(seeded(org()), &iam, true).await;
    let archived = client
        .archive_agent(req(ArchiveAgentRequest { agent_prn: agent_prn(org()) }, Some(TOKEN)))
        .await
        .unwrap()
        .into_inner()
        .agent
        .unwrap();
    assert!(archived.archive_time.is_some());

    let update = UpdateAgentRequest {
        agent_prn: agent_prn(org()),
        version: 1,
        name: "n".to_owned(),
        model: "m1".to_owned(),
        instructions: "i".to_owned(),
        tools: vec![],
    };
    let status = client.update_agent(req(update, Some(TOKEN))).await.unwrap_err();
    assert_eq!(status.code(), Code::FailedPrecondition);
    assert_eq!(status.message(), "the agent is archived");
}

#[tokio::test]
async fn create_inbox_wires_and_a_second_bind_is_already_exists_with_the_inbox_metadata() {
    let iam = Arc::new(key_iam().granting(org(), BOTH));
    let mut client = serve(seeded(org()), &iam, true).await;
    let bind = |principal: Uuid| CreateInboxRequest {
        agent_prn: agent_prn(org()),
        principal_prn: principal_prn(principal),
    };
    let inbox = client.create_inbox(req(bind(u(0x20)), Some(TOKEN))).await.unwrap().into_inner().inbox.unwrap();
    assert_eq!(inbox.inbox_prn, InboxId::new(org(), u(0x1000)).as_prn().canonical());
    assert_eq!(inbox.agent_prn, agent_prn(org()));
    assert_eq!(inbox.principal_prn, principal_prn(u(0x20)));

    let status = client.create_inbox(req(bind(u(0x30)), Some(TOKEN))).await.unwrap_err();
    assert_eq!(status.code(), Code::AlreadyExists);
    assert_eq!(info(&status).2.get("inbox").map(String::as_str), Some(inbox_prn(org()).as_str()));
    assert!(!status.message().contains(&inbox_prn(org())));

    let status = client
        .create_inbox(req(
            CreateInboxRequest {
                agent_prn: agent_prn(org()),
                principal_prn: "nope".to_owned(),
            },
            Some(TOKEN),
        ))
        .await
        .unwrap_err();
    assert_eq!(status.code(), Code::InvalidArgument);
    assert_eq!(info(&status).2.get("field").map(String::as_str), Some("principal_prn"));
}

#[tokio::test]
async fn get_inbox_wires_to_agent_admin() {
    let iam = Arc::new(key_iam().granting(org(), BOTH));
    let mut client = serve(seeded(org()), &iam, true).await;
    let inbox = client
        .get_inbox(req(GetInboxRequest { inbox_prn: inbox_prn(org()) }, Some(TOKEN)))
        .await
        .unwrap()
        .into_inner()
        .inbox
        .unwrap();
    assert_eq!(inbox.inbox_prn, inbox_prn(org()));
    assert_eq!(inbox.principal_prn, principal_prn(u(0x30)));
}

#[tokio::test]
async fn list_inboxes_wires_to_agent_admin() {
    let iam = Arc::new(key_iam().granting(org(), BOTH));
    let mut client = serve(seeded(org()), &iam, true).await;
    let page = client
        .list_inboxes(req(
            ListInboxesRequest {
                agent_prn: agent_prn(org()),
                ..Default::default()
            },
            Some(TOKEN),
        ))
        .await
        .unwrap()
        .into_inner();
    assert_eq!(page.inboxes.len(), 1);
    assert!(page.next_page_token.is_empty());
}

#[tokio::test]
async fn a_bad_prn_field_is_invalid_argument_with_the_field_name() {
    let iam = Arc::new(key_iam().granting(org(), BOTH));
    let mut client = serve(seeded(org()), &iam, true).await;
    let status = client.get_agent(req(GetAgentRequest { agent_prn: "nope".to_owned() }, Some(TOKEN))).await.unwrap_err();
    assert_eq!(status.code(), Code::InvalidArgument);
    assert_eq!(info(&status).0, reason(ErrorReason::InvalidAgentField));
    assert_eq!(info(&status).2.get("field").map(String::as_str), Some("agent_prn"));
    let status = client.get_inbox(req(GetInboxRequest { inbox_prn: "nope".to_owned() }, Some(TOKEN))).await.unwrap_err();
    assert_eq!(info(&status).2.get("field").map(String::as_str), Some("inbox_prn"));
}
