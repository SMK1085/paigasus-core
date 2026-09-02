// SPDX-License-Identifier: Apache-2.0

//! End-to-end proof of the SMA-446 Slice B Unit-of-Work + outbox + relay stack (Task B10, the
//! LAST task in the slice): a real, authorized role-grant mutation over HTTP produces a
//! `role_grant` row, a correlated `event_outbox` row, and a correlated `audit_log` row in ONE
//! atomic commit (`RoleService::grant`'s UoW reference pattern, `application/roles.rs`), and a
//! single `OutboxRelay` tick then publishes that outbox row. Mirrors `tests/audit_e2e.rs`'s
//! pattern (drive the real `router(AppState::new(db, &cfg))` via `tower::ServiceExt::oneshot`
//! against an ephemeral Postgres) plus `tests/relay_pg.rs`'s `CountingPublisher`, but proves the
//! FULL, stitched chain end to end rather than either half in isolation.
//!
//! The relay is deliberately driven by ONE direct `tick()` call, not a spawned background loop
//! (`tests/support/mod.rs`'s oneshot harness never spawns one) — deterministic, no
//! poll-with-timeout needed. (a)-(d) are read straight off the DB immediately after the HTTP
//! call returns: `RoleService::grant`'s three writes (grant + event + audit entry) are already
//! committed on ONE transaction by the time its `POST` response comes back, unlike
//! `tests/audit_e2e.rs`'s denial path (which goes through an out-of-band async drain and so
//! needs to poll).

mod support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use axum::http::StatusCode;
use paigasus_iam::adapters::events::{CloudEvent, OutboxRelay};
use paigasus_iam::adapters::persistence::entities::{audit_log, event_outbox, role_grant};
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{DomainEvent, EventPublisher, EventType, PublishError};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;
use support::{app_with_state, provision, provision_platform_admin, send};

/// A publisher that always succeeds, counting how many events it has seen — mirrors
/// `tests/relay_pg.rs`'s `CountingPublisher` (each `tests/*.rs` binary compiles its own copy of
/// `mod support;`/small fixtures rather than sharing them across files, the established
/// convention in this crate's integration-test suite).
#[derive(Default)]
struct CountingPublisher {
    count: AtomicUsize,
}

#[async_trait]
impl EventPublisher for CountingPublisher {
    async fn publish(&self, _ev: &DomainEvent) -> Result<(), PublishError> {
        self.count.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

/// The B10 correlation e2e: an authorized `POST /v1/authz/role-grants` (a seeded platform admin
/// granting `platform_admin` at Root to a member — mirrors `tests/http_authz.rs`'s
/// `role_grant_lifecycle_over_http`) must leave behind a `role_grant` row, an `event_outbox` row
/// (`event_type = 'iam.role.granted'`), and an `audit_log` row (`action = 'GrantRole'`,
/// `outcome = 'committed'`) sharing ONE non-null `correlation_id` (G5 stitchability) — and a
/// single `OutboxRelay` tick against the SAME db then marks that outbox row published, with the
/// counting publisher having seen it.
#[tokio::test]
async fn mutation_emits_correlated_outbox_and_audit_rows_and_the_relay_publishes_them() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;

    // A platform admin (bootstrap-seeded, bypassing `RoleService::grant`'s own anti-escalation
    // check — there is no prior authority to authorize the very first grant against) plus an
    // ordinary provisioned member as the grant's target.
    let admin_token = idp.bearer("b10-admin", Some("b10-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &admin_token).await;

    let member_token = idp.bearer("b10-member", Some("b10-member@example.com"), "paigasus", 3600);
    let member_prn = provision(&state, &member_token).await;

    // The authorized mutation: the admin grants `platform_admin` at Root to the member.
    let (status, granted) = send(
        &app,
        "POST",
        "/v1/authz/role-grants",
        Some(json!({
            "principal_prn": member_prn,
            "role_key": "platform_admin",
            "scope_prn": root_prn().canonical(),
        })),
        Some(admin_token.as_str()),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "the authorized role grant must succeed: {granted}");
    let grant_id: uuid::Uuid = granted["id"].as_str().expect("id is a string").parse().expect("id is a valid uuid");

    // (a) the `role_grant` row exists.
    let grant_row = role_grant::Entity::find_by_id(grant_id)
        .one(&state.db)
        .await
        .expect("query role_grant")
        .expect("the role_grant row must exist after a successful grant");
    assert_eq!(grant_row.role_key, "platform_admin");

    // (b) an `event_outbox` row with `event_type = 'iam.role.granted'` exists.
    let outbox_row = event_outbox::Entity::find()
        .filter(event_outbox::Column::EventType.eq("iam.role.granted"))
        .one(&state.db)
        .await
        .expect("query event_outbox")
        .expect("an event_outbox row for the role grant must exist");
    assert_eq!(outbox_row.aggregate_prn, member_prn, "the outbox row's aggregate must be the granted principal");

    // (c) an `audit_log` row with `outcome = 'committed'` and `action = 'GrantRole'` exists.
    let audit_row = audit_log::Entity::find()
        .filter(audit_log::Column::Action.eq("GrantRole"))
        .filter(audit_log::Column::Outcome.eq("committed"))
        .one(&state.db)
        .await
        .expect("query audit_log")
        .expect("an audit_log row for the role grant must exist");

    // (d) the outbox row's `correlation_id` == the audit row's `correlation_id` — G5
    // stitchability: non-null and equal.
    assert!(outbox_row.correlation_id.is_some(), "the outbox row's correlation_id must be non-null");
    assert_eq!(outbox_row.correlation_id, audit_row.correlation_id, "the outbox and audit rows must share one correlation id");

    // (e) run ONE `OutboxRelay` tick, constructed over the SAME db the app is using, with a
    // counting publisher and `max_attempts`/`batch_size` fixed for this test (no longer tracking
    // `OutboxConfig`'s defaults, which SMA-471 raised `max_attempts` on) -> the outbox row now
    // has `published_at` set, and the counting publisher saw it.
    let relay = OutboxRelay::new(state.db.clone(), Duration::from_secs(60), 100, 5);
    let publisher = Arc::new(CountingPublisher::default());
    let report = relay.tick(publisher.as_ref()).await.expect("relay tick succeeds");
    assert!(report.drained >= 1, "the tick must drain at least the role-grant event");
    assert_eq!(
        publisher.count.load(Ordering::SeqCst) as u64,
        report.drained,
        "the counting publisher must have seen every drained event"
    );

    let outbox_row_after = event_outbox::Entity::find_by_id(outbox_row.id)
        .one(&state.db)
        .await
        .expect("query event_outbox")
        .expect("row still present after the relay tick");
    assert!(outbox_row_after.published_at.is_some(), "a successfully published row must have published_at set");
}

/// A publisher that captures every `DomainEvent` it sees (rather than merely counting, like
/// `CountingPublisher` above), so a test can render the exact CloudEvent the relay would hand a
/// real broker and assert on its wire `type` — SMA-606 Task 10 step 4.
#[derive(Default)]
struct CapturingPublisher {
    events: std::sync::Mutex<Vec<DomainEvent>>,
}

#[async_trait]
impl EventPublisher for CapturingPublisher {
    async fn publish(&self, ev: &DomainEvent) -> Result<(), PublishError> {
        self.events.lock().unwrap().push(ev.clone());
        Ok(())
    }
}

/// SMA-606 Task 10 step 4: the SAME HTTP -> row -> relay chain the B10 test above proves for a
/// role grant, but for a TENANCY mutation — `OrganizationService::create` (`application/
/// organizations.rs`) writes an `event_outbox` row with `event_type = 'iam.organization.created'`
/// among its three writes (org/team/grant, all one correlation id) — and asserts the CloudEvent
/// [`CloudEvent::from_domain_event`] renders from that row's `DomainEvent` carries
/// `"type": "iam.organization.created"` on the wire (`adapters/events/cloud_event.rs`'s public
/// CloudEvents 1.0 contract), proven end to end rather than by unit-testing the envelope builder
/// alone.
#[tokio::test]
async fn tenancy_mutation_publishes_a_cloudevent_typed_organization_created() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let (app, state, idp) = app_with_state(db).await;

    let admin_token = idp.bearer("t10-admin", Some("t10-admin@example.com"), "paigasus", 3600);
    provision_platform_admin(&state, &admin_token).await;

    let (status, created) = send(&app, "POST", "/v1/organizations", Some(json!({"slug": "t10-org", "name": "Task 10 Org"})), Some(admin_token.as_str())).await;
    assert_eq!(status, StatusCode::CREATED, "the authorized org create must succeed: {created}");
    let org_prn = created["organization"]["prn"].as_str().expect("organization.prn").to_string();

    // Filtered on `AggregatePrn` too, not `EventType` alone — a bug emitting the right type for
    // the wrong aggregate would otherwise still pass this lookup.
    let outbox_row = event_outbox::Entity::find()
        .filter(event_outbox::Column::EventType.eq(EventType::OrganizationCreated.as_wire()))
        .filter(event_outbox::Column::AggregatePrn.eq(org_prn.clone()))
        .one(&state.db)
        .await
        .expect("query event_outbox")
        .expect("an event_outbox row for this test's org create must exist");

    let relay = OutboxRelay::new(state.db.clone(), Duration::from_secs(60), 100, 5);
    let publisher = Arc::new(CapturingPublisher::default());
    let report = relay.tick(publisher.as_ref()).await.expect("relay tick succeeds");
    assert!(report.drained >= 1, "the tick must drain at least the org-create events");

    let captured = publisher.events.lock().unwrap();
    let org_created = captured
        .iter()
        .find(|ev| ev.id == outbox_row.id)
        .expect("the relay must have published the org-create event this test seeded");

    let cloud_event = CloudEvent::from_domain_event(org_created, "urn:paigasus:iam");
    let rendered = serde_json::to_value(&cloud_event).expect("cloud event serializes");
    assert_eq!(rendered["type"], "iam.organization.created", "the published CloudEvent's type must be iam.organization.created");
}
