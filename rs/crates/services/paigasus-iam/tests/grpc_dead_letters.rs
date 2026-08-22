// SPDX-License-Identifier: Apache-2.0

//! End-to-end gRPC coverage for `OutboxService` (SMA-501): the operator break-glass surface
//! over parked `event_outbox` rows. All four RPCs (`ListDeadLetters`, `ReplayDeadLetter`,
//! `BulkReplayDeadLetters`, `DiscardDeadLetter`) are Root-only, enforced INSIDE
//! `DeadLetterService` itself (not the Cedar schema), so a non-Root caller gets
//! `PermissionDenied` with nothing about the dead-letter contents in the response — and every
//! RPC is bearer-enforced (`OutboxService` carries no `is_exempt` entry), so an unauthenticated
//! caller gets `Unauthenticated` before ever reaching the handler.
//!
//! Mirrors `tests/http_dead_letters.rs`'s scenarios (HTTP/gRPC parity is this issue's
//! acceptance criterion) and `tests/grpc_users.rs`/`tests/grpc_authz.rs`'s harness: the real
//! `grpc::router(AppState::new(db, &cfg), ..)` over an ephemeral `TcpListener`, against an
//! ephemeral Postgres (Docker) + the HTTPS mock IdP.
//!
//! The suite's most important test is [`a_present_but_invalid_parked_from_is_rejected_not_ignored`]
//! (design D10): on `BulkReplayDeadLetters`, a `parked_from` bound that is PRESENT but
//! unrepresentable must be rejected outright, never silently treated as "unfiltered" — a
//! dropped time bound there turns a narrowly-scoped replay into "replay everything up to
//! `max_rows`". No unit test can prove the second half (that the would-be-matched rows are
//! still parked after the rejection); only a real store round trip can.

mod support;

use std::net::SocketAddr;
use std::time::Duration;

use chrono::Utc;
use paigasus_iam::adapters::grpc;
use paigasus_iam::adapters::http::AppState;
use paigasus_iam::adapters::persistence::entities::event_outbox;
use paigasus_proto::paigasus::iam::v1::outbox_service_client::OutboxServiceClient;
use paigasus_proto::paigasus::iam::v1::{BulkReplayDeadLettersRequest, DiscardDeadLetterRequest, ListDeadLettersRequest, ReplayDeadLetterRequest};
use sea_orm::{ActiveModelTrait, DatabaseConnection, EntityTrait, Set};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tonic::Code;
use tonic::transport::Channel;
use uuid::Uuid;

/// Seeds a parked `event_outbox` row directly — the dead-letter state. Copied from
/// `tests/http_dead_letters.rs::seed_parked` (NOT `tests/dead_letters_pg.rs::seed_parked_with_details`,
/// a differently-shaped helper in a different file) — each `tests/*.rs` binary compiles its own
/// copy of `mod support` and its fixtures, and duplicating a private seeder across suites is
/// this crate's established posture (`relay_nudge_pg.rs`'s module doc, re:
/// `dead_letters_pg.rs::seed_parked`, documents doing exactly this).
async fn seed_parked(db: &DatabaseConnection, id: u128, event_type: &str) -> Uuid {
    let uuid = Uuid::from_u128(id);
    event_outbox::ActiveModel {
        id: Set(uuid),
        occurred_at: Set(Utc::now()),
        event_type: Set(event_type.to_string()),
        schema_version: Set(1),
        aggregate_prn: Set("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa".to_string()),
        actor_prn: Set(Some("prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000bb".to_string())),
        payload: Set(serde_json::json!({"kind": "user"}).to_string()),
        correlation_id: Set(Some(Uuid::from_u128(999_999))),
        published_at: Set(None),
        attempts: Set(5),
        parked: Set(true),
        parked_at: Set(Some(Utc::now() - chrono::Duration::days(1))),
        last_error: Set(Some("backend error: transport closed".to_string())),
    }
    .insert(db)
    .await
    .unwrap();
    uuid
}

/// Spawns the full `grpc::router` (health, tenancy, authn, authz, service-account,
/// service-info, users, outbox — all wrapped by the bearer layer) on an ephemeral port;
/// `abort()` the returned handle when the test finishes.
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

/// Wraps a request message in a `tonic::Request` carrying an `authorization: Bearer <token>`
/// metadata entry.
fn authed<T>(msg: T, token: &str) -> tonic::Request<T> {
    let mut req = tonic::Request::new(msg);
    support::grpc_bearer(&mut req, token);
    req
}

fn list_req() -> ListDeadLettersRequest {
    ListDeadLettersRequest {
        event_type: String::new(),
        parked_from: None,
        parked_to: None,
        cursor: String::new(),
        limit: 0,
    }
}

fn bulk_req(max_rows: u64) -> BulkReplayDeadLettersRequest {
    BulkReplayDeadLettersRequest {
        event_type: String::new(),
        parked_from: None,
        parked_to: None,
        max_rows,
    }
}

/// A marker planted in the seeded row's `event_type` — distinctive enough that if a future
/// change ever let dead-letter contents leak into an error message (e.g. a `PermissionDenied`
/// that echoed back what it denied access to), this string would show up in the assertion
/// failure rather than the test passing by coincidence.
const CONTENT_MARKER: &str = "sma-501-secret-event-type-must-never-leak";

/// For all four RPCs: an ordinary, non-Root principal (JIT-provisioned via `support::provision`,
/// no `platform_admin` grant) gets `PermissionDenied`, and the error carries nothing about the
/// dead-letter contents — `DeadLetterService` authorizes at `root_prn()` for every operation, so
/// a caller with no grant there is denied before any handler-specific logic runs.
#[tokio::test]
async fn a_non_root_caller_is_permission_denied() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let seed_db = db.clone();
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-dlq-nonadmin", Some("grpc-dlq-nonadmin@example.com"), "paigasus", 3600);
    // An ORDINARY principal: JIT-provisioned, deliberately NOT `support::provision_platform_admin`.
    support::provision(&state, &token).await;
    let id = seed_parked(&seed_db, 1, CONTENT_MARKER).await;
    let (addr, server) = spawn_server(state).await;
    let mut client = OutboxServiceClient::new(channel(addr).await);

    let err = client.list_dead_letters(authed(list_req(), &token)).await.unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied, "{err:?}");
    assert!(!err.message().contains(CONTENT_MARKER), "{err:?}");

    let err = client.replay_dead_letter(authed(ReplayDeadLetterRequest { id: id.to_string() }, &token)).await.unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied, "{err:?}");
    assert!(!err.message().contains(CONTENT_MARKER), "{err:?}");

    let err = client.discard_dead_letter(authed(DiscardDeadLetterRequest { id: id.to_string() }, &token)).await.unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied, "{err:?}");
    assert!(!err.message().contains(CONTENT_MARKER), "{err:?}");

    let err = client.bulk_replay_dead_letters(authed(bulk_req(10), &token)).await.unwrap_err();
    assert_eq!(err.code(), Code::PermissionDenied, "{err:?}");
    assert!(!err.message().contains(CONTENT_MARKER), "{err:?}");

    server.abort();
}

/// Asserts field values, not just a count, so a broken projection fails here — mirrors
/// `tests/http_dead_letters.rs::list_returns_parked_rows_for_a_platform_admin`.
#[tokio::test]
async fn list_returns_seeded_parked_rows() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let seed_db = db.clone();
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-dlq-list-admin", Some("grpc-dlq-list-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &token).await;
    let id = seed_parked(&seed_db, 1, "iam.principal.created").await;
    let (addr, server) = spawn_server(state).await;
    let mut client = OutboxServiceClient::new(channel(addr).await);

    let resp = client.list_dead_letters(authed(list_req(), &token)).await.unwrap().into_inner();
    assert_eq!(resp.entries.len(), 1);
    let entry = &resp.entries[0];
    assert_eq!(entry.id, id.to_string());
    assert_eq!(entry.event_type, "iam.principal.created");
    assert_eq!(entry.schema_version, 1);
    assert_eq!(entry.aggregate_prn, "prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000aa");
    assert_eq!(entry.actor_prn, "prn:pgs:iam:::principal/00000000-0000-0000-0000-0000000000bb");
    assert_eq!(entry.correlation_id, Uuid::from_u128(999_999).to_string());
    assert_eq!(entry.attempts, 5);
    assert_eq!(entry.last_error, "backend error: transport closed");
    assert!(entry.parked_at.is_some(), "{entry:?}");
    // `payload` is the raw stored TEXT, emitted as a JSON STRING — never re-parsed into a
    // structured field (mirrors `http_dead_letters.rs`'s identical assertion).
    assert_eq!(entry.payload, serde_json::json!({"kind": "user"}).to_string());

    server.abort();
}

/// Replaying an id un-parks it (a real, existing row); a second replay of the same id is
/// `NotFound` — the row is simply no longer parked, matching the documented
/// success-after-timeout signal on the HTTP surface.
#[tokio::test]
async fn replay_is_not_idempotent_and_the_second_call_is_not_found() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let seed_db = db.clone();
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-dlq-replay-admin", Some("grpc-dlq-replay-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &token).await;
    let id = seed_parked(&seed_db, 1, "iam.principal.created").await;
    let (addr, server) = spawn_server(state).await;
    let mut client = OutboxServiceClient::new(channel(addr).await);

    let resp = client.replay_dead_letter(authed(ReplayDeadLetterRequest { id: id.to_string() }, &token)).await.unwrap().into_inner();
    assert_eq!(resp.entry.expect("entry").id, id.to_string());

    let err = client.replay_dead_letter(authed(ReplayDeadLetterRequest { id: id.to_string() }, &token)).await.unwrap_err();
    assert_eq!(err.code(), Code::NotFound, "{err:?}");

    server.abort();
}

/// Discarding a row removes it from a subsequent list — a discarded dead letter is gone
/// forever.
#[tokio::test]
async fn discard_removes_the_row_from_a_subsequent_list() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let seed_db = db.clone();
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-dlq-discard-admin", Some("grpc-dlq-discard-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &token).await;
    let id = seed_parked(&seed_db, 1, "iam.principal.created").await;
    let (addr, server) = spawn_server(state).await;
    let mut client = OutboxServiceClient::new(channel(addr).await);

    let resp = client.discard_dead_letter(authed(DiscardDeadLetterRequest { id: id.to_string() }, &token)).await.unwrap().into_inner();
    assert_eq!(resp.entry.expect("entry").id, id.to_string());

    let resp = client.list_dead_letters(authed(list_req(), &token)).await.unwrap().into_inner();
    assert!(resp.entries.is_empty(), "the discarded row must be gone: {:?}", resp.entries);

    server.abort();
}

/// The D5 pin, over the wire: an absent (proto3-collapses-to-zero) `max_rows` is rejected
/// before any store access — never defaulted to anything usable, since the explicit row budget
/// is the guard on blast radius.
#[tokio::test]
async fn bulk_replay_without_max_rows_is_invalid_argument() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let seed_db = db.clone();
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-dlq-bulk-invalid-admin", Some("grpc-dlq-bulk-invalid-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &token).await;
    let id = seed_parked(&seed_db, 1, "iam.principal.created").await;
    let (addr, server) = spawn_server(state).await;
    let mut client = OutboxServiceClient::new(channel(addr).await);

    let err = client.bulk_replay_dead_letters(authed(bulk_req(0), &token)).await.unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument, "{err:?}");

    // Validation happened before any store access — the seeded row must still be parked.
    let row = event_outbox::Entity::find_by_id(id).one(&seed_db).await.unwrap().unwrap();
    assert!(row.parked, "an invalid bulk-replay request must never touch the store");

    server.abort();
}

/// A valid `max_rows` replays every currently-matching parked row and reports the count.
#[tokio::test]
async fn bulk_replay_with_max_rows_replays_matching_rows() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let seed_db = db.clone();
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-dlq-bulk-admin", Some("grpc-dlq-bulk-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &token).await;
    let id1 = seed_parked(&seed_db, 1, "iam.principal.created").await;
    let id2 = seed_parked(&seed_db, 2, "iam.principal.created").await;
    let (addr, server) = spawn_server(state).await;
    let mut client = OutboxServiceClient::new(channel(addr).await);

    let resp = client.bulk_replay_dead_letters(authed(bulk_req(10), &token)).await.unwrap().into_inner();
    assert_eq!(resp.replayed, 2, "must match the seeded parked count");

    for id in [id1, id2] {
        let row = event_outbox::Entity::find_by_id(id).one(&seed_db).await.unwrap().unwrap();
        assert!(!row.parked, "a replayed row must no longer be parked: {id}");
    }

    server.abort();
}

/// **The D10 pin, over the wire — the most important test in this suite.** A `parked_from`
/// bound that is PRESENT but unrepresentable (`nanos: -1` is out of `Timestamp`'s valid
/// `[0, 999_999_999]` range) must be rejected outright, never silently treated as unfiltered:
/// on `BulkReplayDeadLetters`, a dropped time bound would turn a narrowly-scoped replay into
/// "replay everything up to `max_rows`". Seeds rows an UNFILTERED bulk replay WOULD match,
/// sends the invalid bound alongside a valid `max_rows`, asserts `InvalidArgument`, and then —
/// the half a unit test cannot prove — asserts those rows are STILL PARKED, proving the request
/// was rejected rather than silently widened.
#[tokio::test]
async fn a_present_but_invalid_parked_from_is_rejected_not_ignored() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let seed_db = db.clone();
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let token = idp.bearer("grpc-dlq-bulk-badts-admin", Some("grpc-dlq-bulk-badts-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &token).await;
    // Rows an UNFILTERED bulk replay would match.
    let id1 = seed_parked(&seed_db, 1, "iam.principal.created").await;
    let id2 = seed_parked(&seed_db, 2, "iam.principal.created").await;
    let (addr, server) = spawn_server(state).await;
    let mut client = OutboxServiceClient::new(channel(addr).await);

    let req = BulkReplayDeadLettersRequest {
        parked_from: Some(prost_types::Timestamp { seconds: 0, nanos: -1 }),
        ..bulk_req(10)
    };
    let err = client.bulk_replay_dead_letters(authed(req, &token)).await.unwrap_err();
    assert_eq!(err.code(), Code::InvalidArgument, "{err:?}");

    // The half a unit test cannot prove: the rows an unfiltered replay would have matched are
    // still parked. A regression that silently treated the invalid bound as "unfiltered" would
    // have replayed both.
    for id in [id1, id2] {
        let row = event_outbox::Entity::find_by_id(id).one(&seed_db).await.unwrap().unwrap();
        assert!(row.parked, "a rejected bulk-replay request must never touch the store: {id}");
    }

    server.abort();
}

/// Every one of the four `OutboxService` RPCs requires a bearer — `OutboxService` carries no
/// `is_exempt` allowlist entry, so an unauthenticated call never even reaches the handler.
/// Modelled on `tests/api_keys_grpc.rs::management_rpcs_not_exempt`.
#[tokio::test]
async fn outbox_rpcs_not_exempt() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let state = AppState::new(db, &support::test_config(&idp)).await.unwrap();
    let (addr, server) = spawn_server(state).await;
    let mut client = OutboxServiceClient::new(channel(addr).await);
    let some_id = Uuid::from_u128(1).to_string();

    let err = client.list_dead_letters(list_req()).await.unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated, "{err:?}");

    let err = client.replay_dead_letter(ReplayDeadLetterRequest { id: some_id.clone() }).await.unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated, "{err:?}");

    let err = client.discard_dead_letter(DiscardDeadLetterRequest { id: some_id }).await.unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated, "{err:?}");

    let err = client.bulk_replay_dead_letters(bulk_req(10)).await.unwrap_err();
    assert_eq!(err.code(), Code::Unauthenticated, "{err:?}");

    server.abort();
}
