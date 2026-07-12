// SPDX-License-Identifier: Apache-2.0

//! `PgApiKeyRepository` UoW-atomicity test (SMA-446 Slice B, Task B4/B5 parity — review
//! finding): `issue_in` + `PgOutbox::enqueue` + `PgAuditLog::record`, driven through the SAME
//! `SeaOrmUnitOfWork` transaction and committed together (exactly the
//! `application::api_keys::ApiKeyService::issue` reference pattern, module docs there), land
//! as three durable rows — `api_key`/`event_outbox`/`audit_log` — sharing the ONE
//! correlation id the caller mints per mutation, and never leak the key's secret hash into
//! either the outbox payload or the audit detail. Mirrors `tests/authz_role_grants.rs`'s
//! `grant_in`-atomicity tests and `tests/outbox_uow_pg.rs`'s commit/rollback scenarios, but
//! through the real `PgApiKeyRepository` instead of a raw SeaORM insert or `PgRoleGrantStore`.
//!
//! Runs against an ephemeral Postgres in Docker. In CI (`CI` env set) a missing Docker daemon
//! is a HARD FAILURE; on a Docker-less laptop the test skips (returns) with a note — same
//! gating pattern as `tests/authz_role_grants.rs`/`tests/outbox_uow_pg.rs`.

mod support;

use chrono::Utc;
use paigasus_iam::adapters::id::KernelIdGenerator;
use paigasus_iam::adapters::persistence::entities::{api_key, audit_log, event_outbox};
use paigasus_iam::adapters::persistence::{PgApiKeyRepository, PgAuditLog, PgOutbox, PgServiceAccountRepository, SeaOrmUnitOfWork};
use paigasus_iam_core::{ApiKeyRepository, AuditEntry, AuditLog, AuditOutcome, ConflictKind, DomainEvent, EventType, IdGenerator, Outbox, RepositoryError, ServiceAccountRepository, UnitOfWork};
use sea_orm::EntityTrait;

/// Hex-encodes `bytes` the same way `pg_api_keys.rs`'s private `hex_encode` does (lowercase
/// hex, the `key_hash` column's storage format) — duplicated here (rather than made `pub`
/// across an unrelated module boundary) purely so the secret-safety assertions below can
/// search the persisted outbox/audit JSON text for the exact substring a leaked hash would
/// take.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// SMA-446 Slice B Task B4/B5 parity (review finding, Fix 1) — the UoW reference pattern's
/// atomicity proof for API-key issuance at the store level: `PgApiKeyRepository::issue_in` +
/// `PgOutbox::enqueue` + `PgAuditLog::record`, driven through the SAME `SeaOrmUnitOfWork`
/// transaction and committed together, land as three durable rows sharing ONE non-null
/// correlation id — and neither the outbox payload nor the audit detail ever contains the
/// key's hex-encoded secret hash.
#[tokio::test]
async fn issue_in_enqueue_and_record_commit_atomically_sharing_correlation_id_no_secret_leak() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let sar = PgServiceAccountRepository::new(db.clone());
    let owner = support::seed_org_ref(&db).await;
    let (p, sa) = support::sample_sa("ci-bot", owner.clone());
    sar.create(&p, &sa).await.unwrap();

    let repo = PgApiKeyRepository::new(db.clone());
    let (key, hash) = support::sample_key(&sa.principal_id, owner.clone());

    let uow = SeaOrmUnitOfWork::new(db.clone());
    let outbox = PgOutbox::new();
    let audit = PgAuditLog::new(db.clone());

    let now = Utc::now();
    let corr = KernelIdGenerator.new_correlation_id();
    let event = DomainEvent {
        id: KernelIdGenerator.new_event_id(),
        event_type: EventType::ApiKeyIssued,
        schema_version: 1,
        aggregate_prn: sa.principal_id.canonical(),
        actor_prn: None,
        occurred_at: now,
        payload: serde_json::json!({
            "key_id": key.id.uuid(),
            "prefix": key.prefix,
            "scope": key.scope.canonical(),
            "status": key.status.as_str(),
            "expires_at": key.expires_at,
        }),
        correlation_id: Some(corr),
    };
    let entry = AuditEntry {
        id: KernelIdGenerator.new_audit_id(),
        occurred_at: now,
        actor_prn: None,
        action: "IssueApiKey".to_string(),
        resource_prn: Some(owner.canonical()),
        outcome: AuditOutcome::Committed,
        determining_policies: Vec::new(),
        detail: serde_json::json!({"key_id": key.id.uuid(), "prefix": key.prefix}),
        correlation_id: Some(corr),
    };

    let tx = uow.begin().await.unwrap();
    repo.issue_in(&*tx, &key, &hash).await.unwrap();
    outbox.enqueue(&*tx, &event).await.unwrap();
    audit.record(&*tx, &entry).await.unwrap();
    tx.commit().await.unwrap();

    let key_row = api_key::Entity::find_by_id(key.id.uuid()).one(&db).await.unwrap().expect("the committed api_key row must be visible");
    assert_eq!(key_row.id, key.id.uuid());

    let outbox_row = event_outbox::Entity::find_by_id(event.id).one(&db).await.unwrap().expect("outbox row present");
    assert_eq!(outbox_row.event_type, EventType::ApiKeyIssued.as_wire());
    assert_eq!(outbox_row.event_type, "iam.api_key.issued");

    let audit_row = audit_log::Entity::find_by_id(entry.id).one(&db).await.unwrap().expect("audit row present");
    assert_eq!(audit_row.action, "IssueApiKey");

    assert!(outbox_row.correlation_id.is_some(), "the outbox row's correlation id must be non-null");
    assert_eq!(outbox_row.correlation_id, Some(corr));
    assert_eq!(audit_row.correlation_id, Some(corr));
    assert_eq!(
        outbox_row.correlation_id, audit_row.correlation_id,
        "the outbox event and the audit entry must share one correlation id"
    );

    // SECRET SAFETY: neither the outbox payload nor the audit detail ever contains the key's
    // hex-encoded hash (nor the raw hash bytes reinterpreted as a lossy string) — the payload/
    // detail built above only ever carry key_id/prefix/scope/status/expires_at, mirroring
    // `ApiKeyService::issue`'s own contract (module docs there).
    let hex_hash = hex_encode(&hash);
    let payload_str = outbox_row.payload.clone();
    let detail_str = audit_row.detail.clone();
    assert!(!payload_str.contains(&hex_hash), "outbox payload must never contain the key's hex-encoded hash");
    assert!(!detail_str.contains(&hex_hash), "audit detail must never contain the key's hex-encoded hash");
    let lossy_hash = String::from_utf8_lossy(&hash).into_owned();
    assert!(!payload_str.contains(&lossy_hash), "outbox payload must never contain the raw hash bytes");
    assert!(!detail_str.contains(&lossy_hash), "audit detail must never contain the raw hash bytes");
}

/// Guard D2 analogue (SMA-446 Task B4/B5 parity, Fix 1): a store error mid-txn — here,
/// `issue_in` hitting `uq_api_key_hash` on a duplicate hash — rolls the WHOLE unit of work
/// back: an outbox event and an audit entry enqueued/recorded earlier on the SAME transaction
/// must never become visible either.
#[tokio::test]
async fn a_store_error_mid_txn_leaves_no_outbox_or_audit_rows() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };

    let sar = PgServiceAccountRepository::new(db.clone());
    let owner = support::seed_org_ref(&db).await;
    let (p, sa) = support::sample_sa("ci-bot", owner.clone());
    sar.create(&p, &sa).await.unwrap();

    let repo = PgApiKeyRepository::new(db.clone());

    // Seed a first, successfully committed key out of band — its hash is what the in-txn
    // attempt below will collide with (`uq_api_key_hash`).
    let (first_key, hash) = support::sample_key(&sa.principal_id, owner.clone());
    repo.issue(&first_key, &hash).await.unwrap();

    let uow = SeaOrmUnitOfWork::new(db.clone());
    let outbox = PgOutbox::new();
    let audit = PgAuditLog::new(db.clone());
    let now = Utc::now();
    let corr = KernelIdGenerator.new_correlation_id();
    let event = DomainEvent {
        id: KernelIdGenerator.new_event_id(),
        event_type: EventType::ApiKeyIssued,
        schema_version: 1,
        aggregate_prn: sa.principal_id.canonical(),
        actor_prn: None,
        occurred_at: now,
        payload: serde_json::json!({}),
        correlation_id: Some(corr),
    };
    let entry = AuditEntry {
        id: KernelIdGenerator.new_audit_id(),
        occurred_at: now,
        actor_prn: None,
        action: "IssueApiKey".to_string(),
        resource_prn: Some(owner.canonical()),
        outcome: AuditOutcome::Committed,
        determining_policies: Vec::new(),
        detail: serde_json::json!({}),
        correlation_id: Some(corr),
    };

    let tx = uow.begin().await.unwrap();
    outbox.enqueue(&*tx, &event).await.unwrap();
    audit.record(&*tx, &entry).await.unwrap();

    // A distinct key id, but the SAME hash as `first_key` — `uq_api_key_hash` rejects it; the
    // txn is now aborted at the DB level and must be dropped, never committed.
    let (dup_key, _distinct_hash) = support::sample_key(&sa.principal_id, owner);
    let err = repo.issue_in(&*tx, &dup_key, &hash).await.unwrap_err();
    assert!(
        matches!(err, RepositoryError::Conflict(ConflictKind::ApiKeyHashCollision)),
        "expected Conflict(ApiKeyHashCollision) for a unique-hash violation, got {err:?}"
    );
    drop(tx); // no commit -> rollback

    assert!(
        event_outbox::Entity::find_by_id(event.id).one(&db).await.unwrap().is_none(),
        "the rolled-back outbox row must never become visible"
    );
    assert!(
        audit_log::Entity::find_by_id(entry.id).one(&db).await.unwrap().is_none(),
        "the rolled-back audit row must never become visible"
    );
    assert!(
        api_key::Entity::find_by_id(dup_key.id.uuid()).one(&db).await.unwrap().is_none(),
        "the rejected duplicate-hash key must never have been written"
    );
}
