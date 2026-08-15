// SPDX-License-Identifier: Apache-2.0

//! SMA-505: `ServiceInfoService.GetServiceInfo` over gRPC, plus the spec § 6.5 transport-
//! agreement assertion — the HTTP body and the RPC response must describe the same build.
//!
//! Docker-gated exactly like `tests/grpc_audit.rs`; see `tests/http_service_info.rs`'s module
//! doc for why a skipped daemon still leaves AC 3 proven.

mod support;

use std::collections::HashSet;
use std::net::SocketAddr;
use std::time::Duration;

use axum::http::StatusCode;
use paigasus_iam::adapters::grpc;
use paigasus_iam::adapters::http::{AppState, router as http_router};
use paigasus_iam_core::authz::model::root_prn;
use paigasus_proto::paigasus::common::v1::GetServiceInfoRequest;
use paigasus_proto::paigasus::common::v1::service_info_service_client::ServiceInfoServiceClient;
use paigasus_proto::paigasus::iam::v1::audit_service_client::AuditServiceClient;
use paigasus_proto::paigasus::iam::v1::authorization_service_client::AuthorizationServiceClient;
use paigasus_proto::paigasus::iam::v1::service_account_service_client::ServiceAccountServiceClient;
use paigasus_proto::paigasus::iam::v1::{IsAuthorizedRequest, ListApiKeysRequest, ListAuditEntriesRequest, ListPoliciesRequest, ListServiceAccountsRequest};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tonic::Code;
use tonic::transport::Channel;

/// Spawns the full `grpc::router` on an ephemeral port; `abort()` the returned handle when the
/// test finishes. Mirrors `tests/grpc_audit.rs::spawn_server` exactly.
async fn spawn_server(state: AppState) -> (SocketAddr, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let router = grpc::router(state, Duration::from_secs(5)).await;
    let server = tokio::spawn(async move {
        router.serve_with_incoming(incoming).await.unwrap();
    });
    (addr, server)
}

async fn channel(addr: SocketAddr) -> Channel {
    tonic::transport::Endpoint::new(format!("http://{addr}")).unwrap().connect().await.unwrap()
}

/// Builds a `tonic::Request` carrying an `authorization: Bearer <token>` metadata entry.
fn authed<T>(msg: T, token: &str) -> tonic::Request<T> {
    let mut req = tonic::Request::new(msg);
    support::grpc_bearer(&mut req, token);
    req
}

/// A default (unfiltered) `ListAuditEntriesRequest` — mirrors `tests/grpc_audit.rs::
/// default_request`.
fn default_audit_request() -> ListAuditEntriesRequest {
    ListAuditEntriesRequest {
        actor_prn: String::new(),
        resource_prn: String::new(),
        action: String::new(),
        outcome: String::new(),
        from: None,
        to: None,
        cursor: String::new(),
        limit: 0,
    }
}

#[tokio::test]
async fn get_service_info_requires_a_bearer() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;

    let mut client = ServiceInfoServiceClient::new(ch);
    // No `authorization` metadata at all — proves the RPC's `:path` was not added to
    // `grpc::authn::is_exempt`.
    let err = client.get_service_info(tonic::Request::new(GetServiceInfoRequest {})).await.unwrap_err();

    assert_eq!(err.code(), Code::Unauthenticated, "{err:?}");

    server.abort();
}

#[tokio::test]
async fn get_service_info_reports_the_enabled_capabilities() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-svcinfo-reader", Some("grpc-svcinfo-reader@example.com"), "paigasus", 3600);
    support::provision(&state, &token).await;
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;

    let mut client = ServiceInfoServiceClient::new(ch);
    let resp = client.get_service_info(authed(GetServiceInfoRequest {}, &token)).await.unwrap().into_inner();

    let info = resp.service_info.expect("service_info must always be populated, never None");
    assert_eq!(info.service, "iam");
    assert!(!info.version.is_empty(), "version must be a non-empty string");
    let caps: HashSet<String> = info.capabilities.into_iter().collect();
    assert_eq!(caps, HashSet::from(["iam.authz.cedar".to_string(), "iam.apikeys".to_string(), "iam.audit".to_string()]));

    server.abort();
}

#[tokio::test]
async fn the_grpc_and_http_transports_describe_the_same_build() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    // ONE AppState, served over both transports — the whole point of this test.
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-http-parity", Some("grpc-http-parity@example.com"), "paigasus", 3600);
    support::provision(&state, &token).await;

    let http = http_router(state.clone());
    let (status, http_body) = support::send(&http, "GET", "/v1/service-info", None, Some(token.as_str())).await;
    assert_eq!(status, StatusCode::OK, "{http_body}");

    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;
    let mut client = ServiceInfoServiceClient::new(ch);
    let grpc_info = client
        .get_service_info(authed(GetServiceInfoRequest {}, &token))
        .await
        .unwrap()
        .into_inner()
        .service_info
        .expect("service_info must always be populated, never None");

    assert_eq!(http_body["service"], grpc_info.service, "service must agree across transports");
    assert_eq!(http_body["version"], grpc_info.version, "version must agree across transports");

    let http_caps: HashSet<String> = http_body["capabilities"]
        .as_array()
        .expect("capabilities must be an array")
        .iter()
        .map(|v| v.as_str().expect("capability keys are strings").to_string())
        .collect();
    let grpc_caps: HashSet<String> = grpc_info.capabilities.into_iter().collect();
    assert_eq!(http_caps, grpc_caps, "the capability SET must agree across transports");

    server.abort();
}

#[tokio::test]
async fn a_disabled_capabilitys_rpc_returns_unimplemented() {
    // `iam.audit` off: `AuditService` is not registered at all, so `ListAuditEntries` is
    // `Unimplemented` regardless of the caller's own permissions.
    {
        let Some((_node, db)) = support::start_migrated_postgres().await else {
            return;
        };
        let idp = support::start_mock_idp().await;
        let mut cfg = support::test_config(&idp);
        cfg.audit.query_enabled = false;
        let state = AppState::new(db, &cfg).await.unwrap();
        let token = idp.bearer("grpc-svcinfo-audit-off", Some("grpc-svcinfo-audit-off@example.com"), "paigasus", 3600);
        support::provision(&state, &token).await;
        let (addr, server) = spawn_server(state).await;
        let ch = channel(addr).await;

        let mut audit = AuditServiceClient::new(ch);
        let err = audit.list_audit_entries(authed(default_audit_request(), &token)).await.unwrap_err();
        assert_eq!(err.code(), Code::Unimplemented, "{err:?}");

        server.abort();
    }

    // `iam.authz.cedar` off: the six administration RPCs (`ListPolicies` stands in) are
    // `Unimplemented`, but `IsAuthorized` — the gateway's per-request primitive — is not.
    {
        let Some((_node, db)) = support::start_migrated_postgres().await else {
            return;
        };
        let idp = support::start_mock_idp().await;
        let mut cfg = support::test_config(&idp);
        cfg.authz.admin_enabled = false;
        let state = AppState::new(db, &cfg).await.unwrap();
        let token = idp.bearer("grpc-svcinfo-authz-off", Some("grpc-svcinfo-authz-off@example.com"), "paigasus", 3600);
        let principal_prn = support::provision(&state, &token).await;
        let (addr, server) = spawn_server(state).await;
        let ch = channel(addr).await;

        let mut authz = AuthorizationServiceClient::new(ch);
        let err = authz.list_policies(authed(ListPoliciesRequest { limit: 0, offset: 0 }, &token)).await.unwrap_err();
        assert_eq!(err.code(), Code::Unimplemented, "{err:?}");

        // Mirrors `tests/grpc_authz.rs`'s known-good self-query shape: a normal (denied)
        // decision, never `Unimplemented` — proving the RPC stayed mounted.
        let is_authorized = authz
            .is_authorized(authed(
                IsAuthorizedRequest {
                    principal_prn: principal_prn.clone(),
                    action: "ListOrganizations".to_string(),
                    resource_prn: root_prn().canonical(),
                    context: Default::default(),
                },
                &token,
            ))
            .await;
        match is_authorized {
            Ok(_) => {}
            Err(e) => assert_ne!(e.code(), Code::Unimplemented, "IsAuthorized must stay mounted: {e:?}"),
        }

        server.abort();
    }

    // `iam.apikeys` off: the three key RPCs (`ListApiKeys` stands in) are `Unimplemented`, but
    // `ListServiceAccounts` — tenancy management, not an API-key concern — is not.
    {
        let Some((_node, db)) = support::start_migrated_postgres().await else {
            return;
        };
        let idp = support::start_mock_idp().await;
        let mut cfg = support::test_config(&idp);
        cfg.api_keys.management_enabled = false;
        let state = AppState::new(db, &cfg).await.unwrap();
        let token = idp.bearer("grpc-svcinfo-apikeys-off", Some("grpc-svcinfo-apikeys-off@example.com"), "paigasus", 3600);
        support::provision(&state, &token).await;
        let (addr, server) = spawn_server(state).await;
        let ch = channel(addr).await;

        let mut sa = ServiceAccountServiceClient::new(ch);
        let err = sa
            .list_api_keys(authed(
                ListApiKeysRequest {
                    service_account_prn: String::new(),
                    limit: 0,
                    offset: 0,
                },
                &token,
            ))
            .await
            .unwrap_err();
        assert_eq!(err.code(), Code::Unimplemented, "{err:?}");

        let list_service_accounts = sa
            .list_service_accounts(authed(
                ListServiceAccountsRequest {
                    owner_prn: String::new(),
                    limit: 0,
                    offset: 0,
                },
                &token,
            ))
            .await;
        match list_service_accounts {
            Ok(_) => {}
            Err(e) => assert_ne!(e.code(), Code::Unimplemented, "ListServiceAccounts must stay mounted: {e:?}"),
        }

        server.abort();
    }
}
