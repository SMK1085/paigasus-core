// SPDX-License-Identifier: Apache-2.0

//! Schema-level test for m0006 (SMA-446 Task A4): asserts the `audit_log` table + its SeaORM
//! entity exist with the expected columns. The `PgAuditLog` port adapter itself is a later
//! task (A5) — this only proves the migration + entity line up, via a trivial insert+count
//! through the entity (a missing/mistyped column would fail the INSERT/SELECT this issues,
//! not just panic on a `.unwrap()` of absent data).
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker daemon
//! is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note — same
//! gating pattern as `tests/authz_policy_store.rs`/`tests/service_accounts.rs`.

mod support;

use chrono::Utc;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::entities::audit_log;
use paigasus_iam_core::IdGenerator;
use sea_orm::{ActiveModelTrait, EntityTrait, PaginatorTrait, Set};

#[tokio::test]
async fn migration_creates_audit_log_table_and_accepts_a_row() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let before = audit_log::Entity::find().count(&db).await.unwrap();
    assert_eq!(before, 0, "freshly migrated audit_log must start empty");

    let id = KernelIdGenerator.new_audit_id();
    audit_log::ActiveModel {
        id: Set(id),
        occurred_at: Set(Utc::now()),
        actor_prn: Set(Some("prn:pgs:iam:::principal/00000000-0000-0000-0000-000000000001".to_string())),
        action: Set("CreateOrganization".to_string()),
        resource_prn: Set(None),
        outcome: Set("committed".to_string()),
        determining_policies: Set(None),
        detail: Set("{}".to_string()),
        correlation_id: Set(None),
    }
    .insert(&db)
    .await
    .unwrap();

    let after = audit_log::Entity::find().count(&db).await.unwrap();
    assert_eq!(after, 1, "the inserted row must be counted");
}
