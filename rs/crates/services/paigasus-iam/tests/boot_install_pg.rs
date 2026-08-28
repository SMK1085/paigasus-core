// SPDX-License-Identifier: Apache-2.0

//! SMA-571: the boot router with a FULL slot — a real `Serving` built from a real `AppState`,
//! not a stub. Docker-gated because `AppState::new` reconciles system policies into Postgres and
//! compiles a policy snapshot out of it.
//!
//! Without this file, production's composition (`boot_http_router` → fallback → the real
//! `app_routes` under `TraceLayer`/`TimeoutLayer`) would be exercised by nothing: every existing
//! suite drives `http::router` instead.

mod support;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use paigasus_iam::adapters::boot::{BootSlot, Serving, boot_http_router};
use paigasus_iam::adapters::persistence::entities::event_outbox;
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

/// AC 4's third clause. `OnceLock::get` is taken once at dispatch, so a request that started
/// before the install completes against the value it started with.
#[tokio::test]
async fn a_request_in_flight_across_the_install_completes_against_its_pre_swap_value() {
    let Some((_node, slot, router, state)) = slot_and_router().await else {
        return;
    };

    let in_flight = tokio::spawn({
        let router = router.clone();
        async move { router.oneshot(Request::builder().uri("/v1/organizations").body(Body::empty()).unwrap()).await.unwrap() }
    });
    // Let the request reach dispatch before the slot changes under it.
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
