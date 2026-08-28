// SPDX-License-Identifier: Apache-2.0

//! SMA-571: the boot router with a FULL slot — a real `Serving` built from a real `AppState`,
//! not a stub. Docker-gated because `AppState::new` reconciles system policies into Postgres and
//! compiles a policy snapshot out of it.
//!
//! Without this file, production's composition (`boot_http_router` → fallback → the real
//! `app_routes` under `TraceLayer`/`TimeoutLayer`) would be exercised by nothing: every existing
//! suite drives `http::router` instead. `the_delegated_grpc_path_completes_a_real_authenticated_rpc`
//! is the gRPC half of that same claim — spec §6.2(a) asked for "an authenticated app route AND
//! an authenticated RPC through the boot router" and the HTTP-only tests above it were the first
//! draft's whole answer to that (SMA-571 final review).

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use paigasus_iam::adapters::boot::{BootSlot, Serving, boot_grpc_routes, boot_http_router};
use paigasus_iam::adapters::persistence::entities::event_outbox;
use paigasus_proto::paigasus::common::v1::GetServiceInfoRequest;
use paigasus_proto::paigasus::common::v1::service_info_service_client::ServiceInfoServiceClient;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use std::time::Duration;
use testcontainers_modules::postgres::Postgres;
use testcontainers_modules::testcontainers::ContainerAsync;
use tower::ServiceExt;

/// Brief defect fixed here: the original helper bound the container to `_node` and never
/// returned it, so it was dropped — and its Postgres container torn down — the instant this
/// function returned, before any caller ever touched the slot. That is invisible on the FIRST
/// DB query a caller makes (the pool already holds an open connection), but every query after it
/// hangs until the pool's acquire timeout and then fails — measured here: a raw `SELECT 1` right
/// after `app_with_state` succeeds, the very next one (a few synchronous lines later, no
/// `.await` on anything DB-related in between) returns `ConnectionAcquire(Timeout)`. The
/// container is now threaded all the way out and held by the caller for the test's whole body.
async fn slot_and_router() -> Option<(ContainerAsync<Postgres>, BootSlot, axum::Router, paigasus_iam::adapters::http::AppState)> {
    let (node, db) = support::start_migrated_postgres().await?;
    let (_app, state, _idp) = support::app_with_state(db).await;
    let (reporter, _health) = paigasus_iam::adapters::grpc::health_service().await;
    let slot = BootSlot::new(reporter);
    let router = boot_http_router(slot.clone(), None);
    Some((node, slot, router, state))
}

/// AC 4's first clause, and the failure mode that would make every other test here pass while the
/// feature did nothing: the router must read the slot PER REQUEST, not capture its contents when
/// it was built. Same router value on both sides of the install.
#[tokio::test]
async fn the_swap_takes_effect_on_an_already_built_router() {
    let Some((_node, slot, router, state)) = slot_and_router().await else {
        return;
    };

    let resp = router.clone().oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE, "empty slot => migrating");

    slot.install(Serving::new(state, Duration::from_secs(30)).await).await.expect("first install");

    let resp = router.clone().oneshot(Request::builder().uri("/readyz").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "the SAME router value must now see the installed slot");

    // Real delegation: an app route now reaches the real `app_routes`, so an unauthenticated call
    // is a 401 from the bearer layer — not the 503 it was a moment ago, and not a 404.
    let resp = router.oneshot(Request::builder().uri("/v1/organizations").body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED, "503 -> 401 is what proves delegation happened");
}

/// AC 4's third clause: `OnceLock::get` is taken ONCE at dispatch, so a request whose handling
/// started before an `install` must resolve against the value it started with, never tearing
/// across the swap.
///
/// **Honest scope (SMA-571 Task 5 review round 1).** `deferred_fallback`'s `None` arm
/// (`CorrelationLayer` + `migrating_response`) has no genuine async wait of its own — no I/O, no
/// timer, no lock — so it resolves within a handful of scheduler ticks. Verified by mutation:
/// injecting `tokio::time::sleep(Duration::from_millis(200))` before `slot.get()` in
/// `deferred_fallback` makes this test FAIL as expected (the request observes the post-install
/// state and gets 401, not 503); injecting a single `tokio::task::yield_now().await` in the SAME
/// spot does NOT — one scheduler tick is not enough real delay to matter at any timescale a human
/// would call a "race". So under UNMUTATED code, the spawned request below has almost certainly
/// already read the slot and computed its response well before the `sleep`/`install()` below run:
/// this test does not exercise, and is not proof of, a genuine tight race. What it actually
/// guards is a regression that inserts real async work (a network call, a lock, a sleep) between
/// dispatch and the `slot.get()` read, which would let a concurrent `install()` win and delegate a
/// request that should have resolved pre-swap. Read this test's name and this doc as "no
/// regression introduces a real delay before the slot read" — never as "concurrent access to the
/// slot is race-free".
///
/// **Why the `sleep` stays, deliberately, rather than a "clean" primitive.** All this test needs
/// to know is whether the spawned request has already performed its (unobservable, synchronous,
/// checkpoint-free) `slot.get()` read before `install()` runs — and that read has no external
/// signal in production code today, so no `Notify`/`Barrier`/channel can observe it without one
/// being added. Adding a synthetic signal/slow-path INTO production code, purely to make a test
/// deterministic, is a worse outcome than the coverage gap it would close, so that is ruled out.
/// The one primitive that IS available without touching production code — awaiting the spawned
/// `JoinHandle` BEFORE calling `install()` — was tried and rejected: it turns the ORDER (fully
/// resolve, then install) into a structural guarantee rather than a race, which makes the
/// assertion pass UNCONDITIONALLY regardless of implementation. Verified: with that ordering, the
/// SAME 200ms mutation above no longer fails the test at all (`install()` never runs until the
/// spawned request has already returned, so the response can only ever be the pre-swap value). A
/// vacuous test is strictly worse than a timing-dependent one that can still fail, so the
/// wall-clock `sleep` — the only mechanism left that keeps this test able to fail — stays.
///
/// **The flake direction, not just the vacuity direction (SMA-571 final review).** Everything
/// above reasons about the test going vacuously green; the fixed 50ms head start can also fail
/// SPURIOUSLY under load, on unmutated code, if the spawned task simply hasn't been scheduled by
/// the time it elapses — nothing here guarantees the runtime picks up `in_flight` within 50ms of
/// `spawn`, only that it eventually does. `nextest`'s default retry budget (`rs/.config/
/// nextest.toml`) masks that: an occasional scheduler-starved run reports FLAKY rather than
/// failing outright, which is an acceptable cost for keeping this test non-vacuous, but it means
/// a persistent failure here is the signal to look at, not an isolated flaky one.
#[tokio::test]
async fn a_request_dispatched_before_install_completes_against_its_pre_swap_value() {
    let Some((_node, slot, router, state)) = slot_and_router().await else {
        return;
    };

    let in_flight = tokio::spawn({
        let router = router.clone();
        async move { router.oneshot(Request::builder().uri("/v1/organizations").body(Body::empty()).unwrap()).await.unwrap() }
    });
    // Best-effort head start, not a synchronization guarantee — see this test's doc for why no
    // clean primitive can observe the unmutated handler's checkpoint-free slot read.
    tokio::time::sleep(Duration::from_millis(50)).await;
    slot.install(Serving::new(state, Duration::from_secs(30)).await).await.expect("install");

    let resp = in_flight.await.expect("in-flight task");
    assert_eq!(
        resp.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "a request dispatched before the install must finish as 503 migrating, never tear across the swap"
    );
}

/// D6: `OnceLock` makes a double install a visible error rather than a silent replace.
#[tokio::test]
async fn a_second_install_is_rejected() {
    let Some((_node, slot, _router, state)) = slot_and_router().await else {
        return;
    };
    slot.install(Serving::new(state.clone(), Duration::from_secs(30)).await).await.expect("first install");
    assert!(slot.install(Serving::new(state, Duration::from_secs(30)).await).await.is_err());
}

/// SMA-571 review round 1 carry-over — the point of this file.
///
/// `boot_http_router` originally wrapped the WHOLE router (healthz/readyz/fallback) in a second
/// `CorrelationLayer`, on top of the one `app_routes` already carries internally. Two NESTED
/// `tokio::task_local!` scopes disagree: the handler (and anything it persists) observes the
/// INNERMOST scope, because nested `task_local!` scopes shadow — but `CorrelationLayer::call`
/// unconditionally overwrites the RESPONSE headers with its OWN, independently-minted ids on the
/// way out, after the inner future resolves. Net effect: a caller's `paigasus-request-id`/
/// `paigasus-correlation-id` would never match what was actually logged/persisted — silently
/// breaking the SMA-504 cross-service traceability contract this header exists to provide. That
/// was fixed (`boot.rs::deferred_fallback`'s doc), but nothing drove the FULL slot's delegated
/// path with a real `AppState` to see it — every other suite exercises `http::router` directly,
/// which never had the second layer to begin with.
///
/// **Chosen form.** No existing route reflects `current_ids()` straight into a response header
/// (the brief's first-choice construction), but one already OBSERVES it in a way this test can
/// read back: `CreateUser::execute` (`application/create_user.rs`) stamps its `DomainEvent` with
/// `id_gen.new_correlation_id()`, which is exactly `paigasus_observability::current_ids()`
/// (`adapters/id.rs`), and that event is persisted verbatim on the `event_outbox` row. So this
/// drives a REAL, platform_admin-authorized `POST /v1/users` through the FULL slot's delegated
/// path and compares the id the CLIENT received on the wire against the id `CreateUser`'s
/// handler actually saw and persisted.
///
/// Deliberately sends NO inbound `paigasus-correlation-id` header: `adopt_or_mint` reads the
/// SAME, unmodified request at every layer (a `CorrelationLayer` only ever writes RESPONSE
/// headers, never the request it forwards), so with no header to adopt, a double application
/// mints two INDEPENDENT ids — one the client sees, a different one the handler persists. A
/// single application can only ever produce one id, so they are equal only when there is exactly
/// one `CorrelationLayer` in play — under the bug this assertion fails; under the fix it cannot.
///
/// **What this would not catch:** a regression confined to `request_id` while `correlation_id`
/// handling stayed correct (no production route persists `request_id` anywhere to compare
/// against — this repo's audit/outbox rows only carry `correlation_id`); a double application on
/// the gRPC delegation path (`deferred_grpc_fallback`/`Serving.grpc`, which is a completely
/// separate code path from the HTTP one under test here); or a double application on the
/// empty-slot arm (`deferred_fallback`'s `None` branch, which has its own, deliberately single,
/// `CorrelationLayer` application and is covered instead by `boot_deferred.rs`).
#[tokio::test]
async fn the_delegated_path_applies_correlation_layer_exactly_once() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    // Cloned BEFORE `app_with_state` consumes `db` — mirrors `tests/http_users.rs`'s identical
    // setup — so the `event_outbox` read-back below can query the same database independently.
    let query_db = db.clone();
    let (_app, state, idp) = support::app_with_state(db).await;

    let admin_token = idp.bearer("boot-correlation-admin", Some("boot-correlation-admin@example.com"), "paigasus", 3600);
    support::provision_platform_admin(&state, &admin_token).await;

    let (reporter, _health) = paigasus_iam::adapters::grpc::health_service().await;
    let slot = BootSlot::new(reporter);
    let router = boot_http_router(slot.clone(), None);
    slot.install(Serving::new(state, Duration::from_secs(30)).await).await.expect("install");

    let resp = support::send_raw(
        &router,
        "POST",
        "/v1/users",
        Some(serde_json::json!({"email": "boot-correlation@example.com", "display_name": "Boot Correlation"})),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::CREATED, "the delegated path must reach the real, platform_admin-authorized handler");
    let observed_on_wire = resp.headers()["paigasus-correlation-id"].to_str().unwrap().to_string();
    let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024).await.expect("body");
    let body: serde_json::Value = serde_json::from_slice(&bytes).expect("json body");
    let principal_prn = body["principal_prn"].as_str().expect("principal_prn").to_string();

    let row = event_outbox::Entity::find()
        .filter(event_outbox::Column::AggregatePrn.eq(principal_prn))
        .one(&query_db)
        .await
        .expect("query event_outbox")
        .expect("CreateUser enqueues an event_outbox row for the new principal");
    let persisted = row.correlation_id.expect("CreateUser always sets a correlation_id");

    assert_eq!(
        observed_on_wire,
        persisted.to_string(),
        "the id the client received must be the SAME id CreateUser's handler actually observed via \
         current_ids() and persisted — a second CorrelationLayer on the delegated path would make these diverge"
    );
}

/// I1 (SMA-571 final review): drives a REAL, AUTHENTICATED gRPC RPC through the production
/// delegation chain in `deferred_grpc_fallback`'s `Some` arm — `req.map(tonic::body::Body::new)
/// -> AuthEnforce -> Routes -> .map(axum::body::Body::new)` — with a genuine wire-encoded request
/// and a genuine wire-decoded response, over a real TCP connection so trailers are exercised too.
///
/// Before this test, the two RPCs `boot_lifecycle_pg.rs` drives both terminate at an ERROR
/// status (`UNAVAILABLE` from the empty-slot arm, `UNAUTHENTICATED` from `AuthEnforce` rejecting
/// a missing bearer) and both are trailers-only `Status::into_http` responses with no message
/// body — so nothing anywhere proved a SUCCESSFUL unary RPC's response body and trailers survive
/// this mapping. A defect here would break every unary gRPC response in production for the life
/// of the process, with the whole suite green.
///
/// `ServiceInfoService.GetServiceInfo` is the cheapest target: always mounted (SMA-505,
/// regardless of any capability flag) and needs no tenancy/authorization setup beyond a
/// resolvable bearer — mirrors `tests/grpc_service_info.rs`'s own posture for the same RPC.
///
/// The decode is the point: a `grpc-status: 0` assertion alone would not prove the response
/// MESSAGE survived `AuthEnforce::call`'s body mapping — only decoding `service_info` and
/// checking a real field does.
#[tokio::test]
async fn the_delegated_grpc_path_completes_a_real_authenticated_rpc() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (_app, state, idp) = support::app_with_state(db).await;
    let token = idp.bearer("boot-grpc-svcinfo", Some("boot-grpc-svcinfo@example.com"), "paigasus", 3600);
    support::provision(&state, &token).await;

    let (reporter, health) = paigasus_iam::adapters::grpc::health_service().await;
    let slot = BootSlot::new(reporter);
    let routes = boot_grpc_routes(slot.clone(), health);
    slot.install(Serving::new(state, Duration::from_secs(30)).await).await.expect("install");

    // A real TCP server, mirroring `main.rs`'s own `serve_with_incoming(routes.prepare(), ..)`
    // call (minus the `CorrelationLayer`/`timeout` wrapping, which is orthogonal to the
    // body/trailer mapping under test here) — so the generated tonic client stub does the wire
    // encode/decode, exactly as a real caller would, rather than this test hand-rolling gRPC
    // framing over a bare `oneshot`.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local_addr");
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let server = tokio::spawn(async move {
        tonic::transport::Server::builder().serve_with_incoming(routes.prepare(), incoming).await.expect("boot grpc server");
    });

    let channel = tonic::transport::Endpoint::new(format!("http://{addr}")).expect("endpoint").connect().await.expect("connect");
    let mut client = ServiceInfoServiceClient::new(channel);
    let mut req = tonic::Request::new(GetServiceInfoRequest {});
    support::grpc_bearer(&mut req, &token);
    let resp = client
        .get_service_info(req)
        .await
        .expect("an authenticated GetServiceInfo must succeed through the boot router's delegated path")
        .into_inner();

    let info = resp
        .service_info
        .expect("service_info must always be populated, never None — this is the decode that proves the mapping");
    assert_eq!(info.service, "iam", "the decoded response body must be the REAL ServiceInfo, not an artifact of a broken mapping");
    assert!(!info.version.is_empty(), "version must be a non-empty string decoded off the wire");

    server.abort();
}
