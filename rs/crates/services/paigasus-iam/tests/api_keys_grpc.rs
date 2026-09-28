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
use paigasus_iam::adapters::persistence::entities::{api_key, audit_log, event_outbox, service_account};
use paigasus_iam_core::{EventType, TeamId};
use paigasus_kernel::Prn;
use paigasus_proto::paigasus::iam::v1::authn_service_client::AuthnServiceClient;
use paigasus_proto::paigasus::iam::v1::service_account_service_client::ServiceAccountServiceClient;
use paigasus_proto::paigasus::iam::v1::{
    ArchiveServiceAccountRequest, CreateServiceAccountRequest, GetServiceAccountRequest, IntrospectApiKeyRequest, IssueApiKeyRequest, ListApiKeysRequest, ListServiceAccountsRequest,
    RevokeApiKeyRequest,
};
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tonic::Code;
use tonic::transport::Channel;
use uuid::Uuid;

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

/// Reads `ErrorInfo.reason` off a `tonic::Status`. Every IAM status carries one (SMA-504).
fn reason(err: &tonic::Status) -> String {
    let details = tonic_types::StatusExt::get_error_details(err);
    details.error_info().expect("every IAM status carries ErrorInfo").reason.clone()
}

/// SMA-646: `iam.principal.created` outbox rows whose payload names `name`. Filtered by type and
/// payload, never a total (spec §4.2).
async fn created_events(db: &DatabaseConnection, name: &str) -> usize {
    event_outbox::Entity::find()
        .filter(event_outbox::Column::EventType.eq(EventType::PrincipalCreated.as_wire()))
        .all(db)
        .await
        .expect("read event_outbox")
        .into_iter()
        .filter(|row| serde_json::from_str::<serde_json::Value>(&row.payload).expect("payload is json")["name"] == name)
        .count()
}

/// SMA-646: `service_account` rows with this name.
async fn accounts_named(db: &DatabaseConnection, name: &str) -> u64 {
    service_account::Entity::find()
        .filter(service_account::Column::Name.eq(name))
        .count(db)
        .await
        .expect("count service_account")
}

/// SMA-646: `iam.api_key.issued` outbox rows for one service account.
async fn issued_events(db: &DatabaseConnection, sa_prn: &str) -> u64 {
    event_outbox::Entity::find()
        .filter(event_outbox::Column::EventType.eq(EventType::ApiKeyIssued.as_wire()))
        .filter(event_outbox::Column::AggregatePrn.eq(sa_prn))
        .count(db)
        .await
        .expect("count event_outbox")
}

/// SMA-646: `IssueApiKey` audit rows. Their `resource_prn` is the SA's stored owner.
async fn issue_audits(db: &DatabaseConnection, owner_prn: &str) -> u64 {
    audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq("IssueApiKey"))
        .filter(audit_log::Column::ResourcePrn.eq(owner_prn))
        .count(db)
        .await
        .expect("count audit_log")
}

/// SMA-646: `api_key` rows of one service account.
async fn keys_of(db: &DatabaseConnection, sa_prn: &str) -> u64 {
    let sa_uuid = Prn::parse(sa_prn).expect("sa prn").resource_id();
    api_key::Entity::find().filter(api_key::Column::ServiceAccountId.eq(sa_uuid)).count(db).await.expect("count api_key")
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

    // SMA-583: `expires_at` goes through the same `parse_opt_ts` helper as the audit filters.
    // A present-but-unrepresentable value is a client error — the ONLY test of this path, and
    // what makes the refactor's "no behaviour change" claim checkable.
    let err = sa_client
        .issue_api_key(authed(
            IssueApiKeyRequest {
                service_account_prn: sa_prn.clone(),
                scope_prn: owner.canonical(),
                expires_at: Some(prost_types::Timestamp { seconds: 0, nanos: -1 }),
                scope_actions: Vec::new(),
                scope_roles: Vec::new(),
            },
            &token,
        ))
        .await
        .unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument, "an unrepresentable expires_at must be rejected: {err:?}");
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
    // SMA-633 D2: the API-key path reports no grants by decision, not by omission. It runs on
    // the gateway's per-request path and nothing reads the field, so it does not pay for the
    // query. Do not "fix" this to match the OIDC path without reading D2 first.
    assert!(ctx.role_grants.is_empty());
    // SMA-446: introspection surfaces the key's tenancy `scope_prn` — the scope the key was
    // issued for (`owner`), matching the issued `ApiKey.scope_prn` (D11 — the gateway authorizes
    // `InvokeModel` against it).
    assert_eq!(ctx.scope_prn, owner.canonical(), "introspect must echo the issued key's scope_prn: {ctx:?}");
    assert_eq!(ctx.scope_prn, api_key.scope_prn, "introspect scope_prn must match the issued ApiKey's scope_prn");

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

/// SMA-646 I1 (spec §4.2, AC 1-4, 7): `CreateServiceAccount`/`ListServiceAccounts` confirm the
/// owner PRN against storage, against real Postgres and real Cedar.
#[tokio::test]
async fn a_forged_owner_prn_never_creates_a_service_account() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db.clone(), &support::test_config(&idp)).await.unwrap();
    let admin = idp.bearer("i1-admin", Some("i1-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin).await;
    let stranger = idp.bearer("i1-stranger", Some("i1-stranger@example.com"), "paigasus", 3600);
    support::provision(&state, &stranger).await;
    let org = support::seed_org_ref(&state.db).await;
    let team = support::seed_team_ref(&state.db, &org).await;
    let (addr, server) = spawn_server(state).await;
    let mut client = ServiceAccountServiceClient::new(channel(addr).await);

    let absent_org = Uuid::from_u128(0x0f46).as_hyphenated().to_string();
    let forged_slot = support::prn_with_org(&team.canonical(), &absent_org);
    let forged_region = support::prn_with_region(&team.canonical(), "eu-west-1");
    let create = |owner_prn: String, name: &str| CreateServiceAccountRequest { owner_prn, name: name.to_string() };
    let list = |owner_prn: String| ListServiceAccountsRequest { owner_prn, limit: 0, offset: 0 };

    // B1: a forged org slot and a forged region answer prn-mismatch; nothing is written.
    for (label, forged) in [("org slot", forged_slot.clone()), ("region", forged_region.clone())] {
        let err = client.create_service_account(authed(create(forged.clone(), "i1-forged"), &admin)).await.unwrap_err();
        assert_eq!(err.code(), Code::InvalidArgument, "create {label}: {err:?}");
        assert_eq!(reason(&err), "prn-mismatch", "create {label}");
        // B2: List with the forged PRN answers the same.
        let err = client.list_service_accounts(authed(list(forged), &admin)).await.unwrap_err();
        assert_eq!(err.code(), Code::InvalidArgument, "list {label}: {err:?}");
        assert_eq!(reason(&err), "prn-mismatch", "list {label}");
    }
    assert_eq!(accounts_named(&db, "i1-forged").await, 0, "a refused create must write no service_account row");
    assert_eq!(created_events(&db, "i1-forged").await, 0, "a refused create must enqueue no iam.principal.created event");
    let listed = client.list_service_accounts(authed(list(team.canonical()), &admin)).await.unwrap().into_inner().service_accounts;
    assert!(listed.iter().all(|sa| sa.name != "i1-forged"), "{listed:?}");

    // B5: an unknown team uuid answers not-found.
    let unknown = TeamId::from_parts(org.resource_uuid(), Uuid::from_u128(0x0f47)).canonical();
    let err = client.create_service_account(authed(create(unknown, "i1-unknown"), &admin)).await.unwrap_err();
    assert_eq!(err.code(), Code::NotFound, "{err:?}");

    // B6 / AC 4: an ungranted caller gets permission-denied for the forged and the correct PRN
    // alike, on Create and List (real Cedar).
    for (label, prn) in [("correct", team.canonical()), ("forged", forged_slot.clone())] {
        let err = client.create_service_account(authed(create(prn.clone(), "i1-stranger"), &stranger)).await.unwrap_err();
        assert_eq!(err.code(), Code::PermissionDenied, "create {label}: {err:?}");
        assert_eq!(reason(&err), "forbidden", "create {label}");
        let err = client.list_service_accounts(authed(list(prn), &stranger)).await.unwrap_err();
        assert_eq!(err.code(), Code::PermissionDenied, "list {label}: {err:?}");
        assert_eq!(reason(&err), "forbidden", "list {label}");
    }

    // Positive control (AC 3): the correct PRN with upper-cased uuids creates the account, the
    // filtered outbox count moves by one, and Create, Get and List agree on the STORED owner.
    let created = client
        .create_service_account(authed(create(support::prn_upper_uuids(&team.canonical()), "i1-ok"), &admin))
        .await
        .unwrap()
        .into_inner()
        .service_account
        .expect("service_account");
    assert_eq!(created.owner_prn, team.canonical());
    assert_eq!(created_events(&db, "i1-ok").await, 1, "the positive control must be visible to the filtered count");
    let got = client
        .get_service_account(authed(GetServiceAccountRequest { prn: created.prn.clone() }, &admin))
        .await
        .unwrap()
        .into_inner()
        .service_account
        .expect("service_account");
    assert_eq!(got.owner_prn, created.owner_prn);
    let listed = client.list_service_accounts(authed(list(team.canonical()), &admin)).await.unwrap().into_inner().service_accounts;
    let entry = listed.iter().find(|sa| sa.prn == created.prn).expect("the created account is listed");
    assert_eq!(entry.owner_prn, created.owner_prn);

    server.abort();
}

/// SMA-646 I3 (spec §4.2, AC 11-14): `IssueApiKey` confirms the scope PRN against storage, and
/// authorizes `IssueApiKey` at the stored scope (K4), against real Postgres and real Cedar.
#[tokio::test]
async fn a_forged_scope_prn_never_issues_an_api_key() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db.clone(), &support::test_config(&idp)).await.unwrap();
    let admin = idp.bearer("i3-admin", Some("i3-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin).await;
    let org_a = support::seed_org_ref(&state.db).await;
    let team_a = support::seed_team_ref(&state.db, &org_a).await;
    let org_b = support::seed_org_ref(&state.db).await;
    let team_b = support::seed_team_ref(&state.db, &org_b).await;
    // The actor holds org_admin at organization A only.
    let actor = idp.bearer("i3-actor", Some("i3-actor@example.com"), "paigasus", 3600);
    let actor_prn = support::provision(&state, &actor).await;
    support::seed_org_admin(&state, &actor_prn, &org_a.canonical()).await;
    let (addr, server) = spawn_server(state).await;
    let mut client = ServiceAccountServiceClient::new(channel(addr).await);

    // The SA is owned by team A (created by the admin with the correct PRN). It has no grants,
    // so D15 is empty and the only checks are the owner check and the new scope check.
    let sa = client
        .create_service_account(authed(
            CreateServiceAccountRequest {
                owner_prn: team_a.canonical(),
                name: "i3-bot".to_string(),
            },
            &admin,
        ))
        .await
        .unwrap()
        .into_inner()
        .service_account
        .expect("service_account");
    let issue = |scope_prn: String| IssueApiKeyRequest {
        service_account_prn: sa.prn.clone(),
        scope_prn,
        expires_at: None,
        scope_actions: Vec::new(),
        scope_roles: Vec::new(),
    };
    let events_before = issued_events(&db, &sa.prn).await;
    let audits_before = issue_audits(&db, &team_a.canonical()).await;

    // K1: a forged org slot and a forged region answer prn-mismatch; nothing is written.
    let absent_org = Uuid::from_u128(0x0f48).as_hyphenated().to_string();
    for (label, forged) in [
        ("org slot", support::prn_with_org(&team_a.canonical(), &absent_org)),
        ("region", support::prn_with_region(&team_a.canonical(), "eu-west-1")),
    ] {
        let err = client.issue_api_key(authed(issue(forged), &actor)).await.unwrap_err();
        assert_eq!(err.code(), Code::InvalidArgument, "{label}: {err:?}");
        assert_eq!(reason(&err), "prn-mismatch", "{label}");
    }
    assert_eq!(keys_of(&db, &sa.prn).await, 0, "a refused issue must write no api_key row");
    assert_eq!(issued_events(&db, &sa.prn).await, events_before, "a refused issue must enqueue no iam.api_key.issued event");
    assert_eq!(issue_audits(&db, &team_a.canonical()).await, audits_before, "a refused issue must write no IssueApiKey audit row");

    // K3: an unknown team uuid as scope answers not-found.
    let unknown = TeamId::from_parts(org_a.resource_uuid(), Uuid::from_u128(0x0f49)).canonical();
    let err = client.issue_api_key(authed(issue(unknown), &actor)).await.unwrap_err();
    assert_eq!(err.code(), Code::NotFound, "{err:?}");

    // K4: a scope node in organization B, where the actor has no IssueApiKey, answers
    // permission-denied for the correct and the forged PRN alike.
    let org_a_slot = org_a.resource_uuid().as_hyphenated().to_string();
    for (label, scope) in [("correct", team_b.canonical()), ("forged", support::prn_with_org(&team_b.canonical(), &org_a_slot))] {
        let err = client.issue_api_key(authed(issue(scope), &actor)).await.unwrap_err();
        assert_eq!(err.code(), Code::PermissionDenied, "{label}: {err:?}");
        assert_eq!(reason(&err), "forbidden", "{label}");
    }
    assert_eq!(keys_of(&db, &sa.prn).await, 0);

    // Positive control (AC 12, Review Focus 3): the actor's org-level grant covers team A as a
    // scope (Cedar `resource in`). Upper-cased uuids still succeed, the answer carries the
    // STORED scope, the filtered outbox count moves by one, and ListApiKeys agrees.
    let issued = client.issue_api_key(authed(issue(support::prn_upper_uuids(&team_a.canonical())), &actor)).await.unwrap().into_inner();
    let key = issued.api_key.expect("api_key");
    assert_eq!(key.scope_prn, team_a.canonical());
    assert_eq!(issued_events(&db, &sa.prn).await, events_before + 1, "the positive control must be visible to the filtered count");
    let listed = client
        .list_api_keys(authed(
            ListApiKeysRequest {
                service_account_prn: sa.prn.clone(),
                limit: 0,
                offset: 0,
            },
            &actor,
        ))
        .await
        .unwrap()
        .into_inner()
        .api_keys;
    let entry = listed.iter().find(|k| k.id == key.id).expect("the issued key is listed");
    assert_eq!(entry.scope_prn, key.scope_prn);

    server.abort();
}
