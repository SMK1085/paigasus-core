// SPDX-License-Identifier: Apache-2.0

//! End-to-end gRPC coverage for `ServiceAccountService` (7 RPCs) + `AuthnService.
//! IntrospectApiKey` (SMA-445 Task 21). Drives the real `grpc::router(AppState::new(db, &cfg),
//! ..)` over an ephemeral `TcpListener` (mirrors `tests/grpc_tenancy.rs`/`tests/grpc_authn.rs`)
//! against an ephemeral Postgres (Docker; see `tests/support/mod.rs`).
//!
//! Two things this file specifically proves:
//! 1. `grpc_issue_and_introspect_parity` — a service account created and issued a key entirely
//!    over gRPC introspects (also over gRPC) to that same service account's principal PRN.
//! 2. `management_rpcs_not_exempt` — every `ServiceAccountService` RPC (`CreateServiceAccount`/
//!    `IssueApiKey` stand in for the other five) requires a bearer, while `IntrospectApiKey`
//!    (added to the `AuthLayer`'s `is_exempt` set alongside token `Introspect`) does not.
//!
//! `service_account_and_api_key_lifecycle_over_grpc` additionally exercises every one of the 7
//! `ServiceAccountService` RPCs once each, catching any wire-shape/authorization-ordering bug
//! a narrower test could miss (`ArchiveServiceAccount` returns an EMPTY response, like
//! `DetachMembership` — see `grpc/service_accounts.rs`'s module docs — so the test re-`get`s
//! the row afterward to confirm the archive itself succeeded), AND covers the CodeRabbit
//! finding on the SMA-445 PR that `ServiceAccount.status` must be populated on the wire:
//! `"active"` right after create/get/list, then `"disabled"` on both `GetServiceAccount` and
//! `ListServiceAccounts` once the account is archived — HTTP/gRPC parity with
//! `tests/http_service_accounts.rs::create_get_list_archive_lifecycle_over_http`.

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use paigasus_iam::adapters::grpc;
use paigasus_iam::adapters::http::AppState;
use paigasus_proto::paigasus::iam::v1::authn_service_client::AuthnServiceClient;
use paigasus_proto::paigasus::iam::v1::service_account_service_client::ServiceAccountServiceClient;
use paigasus_proto::paigasus::iam::v1::{
    ArchiveServiceAccountRequest, CreateServiceAccountRequest, GetServiceAccountRequest, IntrospectApiKeyRequest, IssueApiKeyRequest, ListApiKeysRequest, ListServiceAccountsRequest,
    RevokeApiKeyRequest,
};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tonic::Code;
use tonic::transport::Channel;

/// Spawns the full `grpc::router` (health + tenancy + authn + authz + service-accounts, all
/// wrapped by the bearer layer) on an ephemeral port; `abort()` the returned handle when done.
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

#[tokio::test]
async fn grpc_issue_and_introspect_parity() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-sa-tester", Some("grpc-sa-tester@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &token).await;
    let owner = support::seed_org_ref(&state.db).await;
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;
    let mut sa_client = ServiceAccountServiceClient::new(ch.clone());
    let mut authn = AuthnServiceClient::new(ch);

    let created = sa_client
        .create_service_account(authed(
            CreateServiceAccountRequest {
                owner_prn: owner.canonical(),
                name: "ci-bot".to_string(),
            },
            &token,
        ))
        .await
        .unwrap()
        .into_inner()
        .service_account
        .expect("service_account");
    let sa_prn = created.prn.clone();

    let issued = sa_client
        .issue_api_key(authed(
            IssueApiKeyRequest {
                service_account_prn: sa_prn.clone(),
                scope_prn: owner.canonical(),
                expires_at: None,
                scope_actions: Vec::new(),
                scope_roles: Vec::new(),
            },
            &token,
        ))
        .await
        .unwrap()
        .into_inner();
    assert!(issued.token.starts_with("pgs_sk_"), "{}", issued.token);
    let api_key = issued.api_key.expect("api_key");
    assert_eq!(api_key.service_account_prn, sa_prn);
    // The one-time issue response never carries a bare secret/hash field either -- `ApiKey`
    // structurally has neither (module docs), so there is nothing beyond `token` to leak.

    // IntrospectApiKey WITHOUT a bearer (the credential travels in the request body, not a
    // metadata entry) resolves the token back to the SA's own principal PRN -- the gRPC-issued
    // key really does authenticate over the gRPC introspection path.
    let ctx = authn.introspect_api_key(IntrospectApiKeyRequest { token: issued.token }).await.unwrap().into_inner();
    assert_eq!(ctx.principal_prn, sa_prn);
    assert_eq!(ctx.status, "active");
    assert!(!ctx.key_id.is_empty(), "{ctx:?}");
    assert!(ctx.memberships.is_empty());
    assert!(ctx.role_grants.is_empty());

    server.abort();
}

#[tokio::test]
async fn management_rpcs_not_exempt() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-sa-exempt-tester", Some("grpc-sa-exempt-tester@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &token).await;
    let owner = support::seed_org_ref(&state.db).await;
    let (addr, server) = spawn_server(state).await;
    let ch = channel(addr).await;
    let mut sa_client = ServiceAccountServiceClient::new(ch.clone());
    let mut authn = AuthnServiceClient::new(ch);

    // Seed a real SA + key over an AUTHENTICATED call, so the exempt half of this test below has
    // a genuinely valid plaintext token to present.
    let created = sa_client
        .create_service_account(authed(
            CreateServiceAccountRequest {
                owner_prn: owner.canonical(),
                name: "ci-bot".to_string(),
            },
            &token,
        ))
        .await
        .unwrap()
        .into_inner()
        .service_account
        .expect("service_account");
    let issued = sa_client
        .issue_api_key(authed(
            IssueApiKeyRequest {
                service_account_prn: created.prn.clone(),
                scope_prn: owner.canonical(),
                expires_at: None,
                scope_actions: Vec::new(),
                scope_roles: Vec::new(),
            },
            &token,
        ))
        .await
        .unwrap()
        .into_inner();

    // `CreateServiceAccount` with NO bearer -> `Unauthenticated`: the `AuthLayer`'s `:path`
    // exemption gate does not cover `ServiceAccountService` at all, so this never even reaches
    // the handler.
    let err = sa_client
        .create_service_account(CreateServiceAccountRequest {
            owner_prn: owner.canonical(),
            name: "another".to_string(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated, "{err:?}");

    // `IssueApiKey` with NO bearer -> `Unauthenticated`, same reason (a second management RPC,
    // proving this isn't specific to `CreateServiceAccount`).
    let err = sa_client
        .issue_api_key(IssueApiKeyRequest {
            service_account_prn: created.prn.clone(),
            scope_prn: owner.canonical(),
            expires_at: None,
            scope_actions: Vec::new(),
            scope_roles: Vec::new(),
        })
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated, "{err:?}");

    // `IntrospectApiKey` with NO bearer -> ALLOWED: it reaches the handler (proving the
    // `is_exempt` entry added alongside token `Introspect`) and resolves the valid key to the
    // SA's own principal -- a genuine success, not merely "not Unauthenticated for some other
    // reason" (e.g. a malformed-token rejection is ALSO `Unauthenticated`, from the handler
    // itself, so only a real success proves the exemption here).
    let ctx = authn.introspect_api_key(IntrospectApiKeyRequest { token: issued.token }).await.unwrap().into_inner();
    assert_eq!(ctx.principal_prn, created.prn);

    server.abort();
}

#[tokio::test]
async fn service_account_and_api_key_lifecycle_over_grpc() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-sa-lifecycle", Some("grpc-sa-lifecycle@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &token).await;
    let owner = support::seed_org_ref(&state.db).await;
    let (addr, server) = spawn_server(state).await;
    let mut sa_client = ServiceAccountServiceClient::new(channel(addr).await);

    let created = sa_client
        .create_service_account(authed(
            CreateServiceAccountRequest {
                owner_prn: owner.canonical(),
                name: "ci-bot".to_string(),
            },
            &token,
        ))
        .await
        .unwrap()
        .into_inner()
        .service_account
        .expect("service_account");
    assert_eq!(created.owner_prn, owner.canonical());
    assert_eq!(created.name, "ci-bot");
    // CodeRabbit finding on the SMA-445 PR: `ServiceAccount.status` must be populated, not left
    // empty — a freshly created SA's principal is `active` (D16).
    assert_eq!(created.status, "active", "{created:?}");

    let got = sa_client
        .get_service_account(authed(GetServiceAccountRequest { prn: created.prn.clone() }, &token))
        .await
        .unwrap()
        .into_inner()
        .service_account
        .expect("service_account");
    assert_eq!(got.prn, created.prn);
    assert_eq!(got.status, "active", "{got:?}");

    let listed = sa_client
        .list_service_accounts(authed(
            ListServiceAccountsRequest {
                owner_prn: owner.canonical(),
                limit: 0,
                offset: 0,
            },
            &token,
        ))
        .await
        .unwrap()
        .into_inner()
        .service_accounts;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].prn, created.prn);
    assert_eq!(listed[0].status, "active", "{listed:?}");

    let issued = sa_client
        .issue_api_key(authed(
            IssueApiKeyRequest {
                service_account_prn: created.prn.clone(),
                scope_prn: owner.canonical(),
                expires_at: None,
                scope_actions: Vec::new(),
                scope_roles: Vec::new(),
            },
            &token,
        ))
        .await
        .unwrap()
        .into_inner();
    let key_id = issued.api_key.expect("api_key").id;

    let listed_keys = sa_client
        .list_api_keys(authed(
            ListApiKeysRequest {
                service_account_prn: created.prn.clone(),
                limit: 0,
                offset: 0,
            },
            &token,
        ))
        .await
        .unwrap()
        .into_inner()
        .api_keys;
    assert_eq!(listed_keys.len(), 1);
    assert_eq!(listed_keys[0].id, key_id);

    sa_client.revoke_api_key(authed(RevokeApiKeyRequest { id: key_id.clone() }, &token)).await.unwrap();

    // Archiving disables the underlying principal (D16). The response is EMPTY
    // (`ArchiveServiceAccountResponse {}`, like `DetachMembership`) -- archive authorizes ONLY
    // `ArchiveServiceAccount`, matching the HTTP `DELETE`'s 204 semantics (see `grpc/
    // service_accounts.rs`'s module docs). A subsequent `GetServiceAccount` still resolves the
    // (now-disabled) row, proving the archive itself succeeded -- and its `status` now reads
    // `disabled`, not a stale `active` (CodeRabbit finding on the SMA-445 PR).
    sa_client
        .archive_service_account(authed(ArchiveServiceAccountRequest { prn: created.prn.clone() }, &token))
        .await
        .unwrap();
    let after = sa_client
        .get_service_account(authed(GetServiceAccountRequest { prn: created.prn.clone() }, &token))
        .await
        .unwrap()
        .into_inner()
        .service_account
        .expect("service_account");
    assert_eq!(after.prn, created.prn);
    assert_eq!(after.owner_prn, owner.canonical());
    assert_eq!(after.status, "disabled", "{after:?}");

    // The list surface agrees: HTTP/gRPC parity on the archived status.
    let listed_after = sa_client
        .list_service_accounts(authed(
            ListServiceAccountsRequest {
                owner_prn: owner.canonical(),
                limit: 0,
                offset: 0,
            },
            &token,
        ))
        .await
        .unwrap()
        .into_inner()
        .service_accounts;
    assert_eq!(listed_after.len(), 1);
    assert_eq!(listed_after[0].status, "disabled", "{listed_after:?}");

    server.abort();
}
