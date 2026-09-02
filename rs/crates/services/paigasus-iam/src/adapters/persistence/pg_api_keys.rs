// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed `ApiKeyRepository` (SeaORM). `issue`/`revoke` (SMA-446, Slice B Task B6)
//! are thin one-shot-`UnitOfWork` wrappers around [`ApiKeyRepository::issue_in`]/
//! [`ApiKeyRepository::revoke_in`] — the exact reference pattern `PgRoleGrantStore::grant`/
//! `revoke` establish over `grant_in`/`revoke_in` (B4), except there is no generation counter
//! to bump afterward (module docs on the port trait): open a `SeaOrmTransaction`, drive the
//! write on it, commit. `issue_in` is a single insert (the domain `ApiKey` never carries hash
//! material — `key_hash: &[u8]` is a separate argument, stored as lowercase hex in the
//! `key_hash TEXT UNIQUE` column: deterministic, comparable, and symmetric with `find_by_id`'s
//! decode back to `Vec<u8>`; a duplicate hash surfaces as `Conflict(ApiKeyHashCollision)` via
//! `mod.rs`'s `conflict_kind`, D7). `revoke_in` mirrors `pg_organizations.rs::set_status`'s
//! idempotent posture: an already-revoked key (or one that no longer exists — a benign TOCTOU
//! race, the caller already resolved it via `find_by_id` before opening its transaction) is a
//! no-op returning `false` (status/`revoked_at` left untouched, never re-stamping `revoked_at`
//! to a later `now`); only a genuine Active -> Revoked transition returns `true`. Deliberately
//! never touches the [`crate::adapters::api_keys::ApiKeyValidationCache`] itself — the
//! caller's own awaited, POST-COMMIT responsibility (`application::api_keys::ApiKeyService::
//! revoke`), SECURITY-CRITICAL (spec §9/D5): a stale cached key must stop authenticating the
//! moment the revoke actually commits, never before (a rolled-back revoke must not evict) and
//! never skipped (even a `false`/no-op `revoke_in` outcome still gets a post-commit evict
//! attempt from the caller, in case an earlier revoke's own evict failed). `touch_last_used`
//! is a single guarded `UPDATE … WHERE id = $1 AND (last_used_at IS NULL OR last_used_at <
//! $3)` (raw SQL, mirrors `pg_memberships.rs`'s precedent) rather than a fetch-then-update:
//! the throttle exists specifically to bound write amplification from concurrent hot-key
//! touches, so the guard must be atomic in the database, not a read-then-write race in the
//! adapter.
//!
//! `scope: TenancyNodeRef` maps to the (at most one non-null) `scope_org_id`/`scope_team_id`/
//! `scope_project_id` columns exactly like `pg_service_accounts.rs`'s owner-column mapping
//! (bare uuids, no denormalized PRN — unlike `role_grant`'s `scope_node_prn` — so a team/
//! project scope needs one join back to recover its embedded org segment).
//! `scope_actions`/`scope_roles` map to `TEXT NULL` columns as a JSON array (`Action::as_wire`
//! ids for the former, plain role keys for the latter), `NULL` for an empty `Vec`.

use super::entities::{api_key, project, team};
use super::map_err;
use super::uow::{SeaOrmTransaction, recover_txn};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use paigasus_iam_core::{Action, ApiKey, ApiKeyId, ApiKeyRepository, ApiKeyStatus, OrganizationId, PrincipalId, ProjectId, RepositoryError, TeamId, TenancyNodeRef, Transaction};
use paigasus_kernel::Prn;
use sea_orm::{ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, IntoActiveModel, QueryFilter, QueryOrder, QuerySelect, Set, Statement, TransactionTrait};
use uuid::Uuid;

// `Clone` lets the composition root hold a repo handle inside a `#[derive(Clone)]` service —
// cheap, `DatabaseConnection` clones an `Arc`-backed pool handle, not a connection (mirrors
// `PgServiceAccountRepository`/`PgOrganizationRepository`'s precedent).
#[derive(Clone)]
pub struct PgApiKeyRepository {
    db: DatabaseConnection,
}

impl PgApiKeyRepository {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgApiKeyRepository { db }
    }
}

/// Hex-encodes `bytes` to lowercase hex — the `key_hash` TEXT column's storage format
/// (deterministic and directly comparable across rows; `hex` is not otherwise a workspace
/// dependency, so this is a small self-contained helper rather than a new crate for two
/// one-line functions).
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Decodes lowercase hex produced by [`hex_encode`] back into raw bytes. Malformed stored hex
/// (odd length, non-ASCII, or a non-hex digit) is a `Backend` error — the row was written by
/// this same adapter's `hex_encode`, so a failure here means the data is corrupt (mirrors
/// `pg_organizations.rs::model_to_org`'s parse-failure posture). Operates on the raw BYTES
/// (never `&s[i..i + 2]` string slicing, which would panic on a non-char-boundary cut through
/// a multi-byte UTF-8 char) — every window is bounds-safe, so this never panics regardless of
/// input (the bounds-safety discipline `api_key.rs::parse_token` established).
fn hex_decode(key_id: Uuid, s: &str) -> Result<Vec<u8>, RepositoryError> {
    let backend = |msg: String| RepositoryError::Backend(Box::new(std::io::Error::other(msg)));
    let bytes = s.as_bytes();
    if !bytes.len().is_multiple_of(2) {
        return Err(backend(format!("api_key {key_id}: key_hash has odd length {}", bytes.len())));
    }
    bytes
        .chunks_exact(2)
        .map(|pair| {
            // `pair` is exactly two raw bytes. `from_utf8` rejects any non-ASCII byte (a
            // multi-byte char's lead/continuation bytes are never valid two-byte UTF-8 that's
            // also valid hex), and `from_str_radix` then rejects any non-hex ASCII — both
            // surface as `Backend`, never a panic.
            let hex = std::str::from_utf8(pair).map_err(|_| backend(format!("api_key {key_id}: key_hash is not ascii hex")))?;
            u8::from_str_radix(hex, 16).map_err(|_| backend(format!("api_key {key_id}: key_hash is not valid hex")))
        })
        .collect()
}

/// Serializes `actions` to the `scope_actions` TEXT column: `None` for an empty vec, else a
/// JSON array of each `Action`'s wire id (`Action::as_wire`) — the same string used over
/// HTTP/gRPC, so a stored key's narrowing round-trips exactly. Serializing `&[&'static str]`
/// cannot fail.
fn actions_to_column(actions: &[Action]) -> Option<String> {
    if actions.is_empty() {
        return None;
    }
    let wire: Vec<&'static str> = actions.iter().map(Action::as_wire).collect();
    Some(serde_json::to_string(&wire).expect("Vec<&str> always serializes"))
}

/// Inverse of [`actions_to_column`]: a `NULL`/empty column is an empty vec. Unparseable JSON
/// or an unrecognized wire id is a `Backend` error (never a silent default) — the row was
/// written by this same adapter, so either means the data is corrupt or the `Action` enum's
/// wire ids changed underneath it.
fn actions_from_column(raw: Option<&str>) -> Result<Vec<Action>, RepositoryError> {
    let backend = |msg: String| RepositoryError::Backend(Box::new(std::io::Error::other(msg)));
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let wire: Vec<String> = serde_json::from_str(raw).map_err(|e| backend(e.to_string()))?;
    wire.into_iter().map(|w| Action::parse(&w).ok_or_else(|| backend(format!("unknown action wire id: {w}")))).collect()
}

/// Serializes `roles` to the `scope_roles` TEXT column: `None` for an empty vec, else a JSON
/// array of the role keys as-is (plain strings, unlike `Action`'s wire-id indirection).
fn roles_to_column(roles: &[String]) -> Option<String> {
    if roles.is_empty() {
        return None;
    }
    Some(serde_json::to_string(roles).expect("Vec<String> always serializes"))
}

/// Inverse of [`roles_to_column`]: a `NULL`/empty column is an empty vec.
fn roles_from_column(raw: Option<&str>) -> Result<Vec<String>, RepositoryError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    serde_json::from_str(raw).map_err(|e| RepositoryError::Backend(Box::new(std::io::Error::other(e.to_string()))))
}

/// Splits a [`TenancyNodeRef`] scope into the row's (at most one non-null) `scope_org_id`/
/// `scope_team_id`/`scope_project_id` columns — the write-side half of [`scope_from_columns`],
/// mirrors `pg_service_accounts.rs::owner_columns`'s column-split shape (no `Root` arm here:
/// an API key's scope, like a service account's owner, is always a concrete tenancy node).
fn scope_columns(scope: &TenancyNodeRef) -> (Option<Uuid>, Option<Uuid>, Option<Uuid>) {
    match scope {
        TenancyNodeRef::Organization(id) => (Some(id.uuid()), None, None),
        TenancyNodeRef::Team(id) => (None, Some(id.uuid()), None),
        TenancyNodeRef::Project(id) => (None, None, Some(id.uuid())),
    }
}

/// A missing scope row for an existing `api_key` is a data-integrity break (the FK should
/// prevent it) — surfaced as `Backend`, never a silent default (mirrors
/// `pg_service_accounts.rs::missing_owner`'s posture).
fn missing_scope(kind: &str, scope_id: Uuid, key_id: Uuid) -> RepositoryError {
    RepositoryError::Backend(Box::new(std::io::Error::other(format!("{kind} {scope_id} missing for api_key {key_id}"))))
}

/// Reconstructs the domain `TenancyNodeRef` for a stored `api_key` row's scope column triple
/// (exactly one set, `ck_api_key_scope`) — mirrors `pg_service_accounts.rs::owner_from_columns`
/// exactly: an organization scope needs no further lookup, a team/project scope needs one join
/// back to the owning `team`/`project` row to recover the `TeamId`/`ProjectId`'s embedded org
/// segment (the `api_key` row only stores the bare uuid, like `service_account`'s owner
/// columns — unlike `role_grant`, which denormalizes the scope's full canonical PRN).
async fn scope_from_columns<C: ConnectionTrait>(
    db: &C,
    key_id: Uuid,
    scope_org_id: Option<Uuid>,
    scope_team_id: Option<Uuid>,
    scope_project_id: Option<Uuid>,
) -> Result<TenancyNodeRef, RepositoryError> {
    if let Some(id) = scope_org_id {
        return Ok(TenancyNodeRef::Organization(OrganizationId::from_uuid(id)));
    }
    if let Some(id) = scope_team_id {
        let Some(team_model) = team::Entity::find_by_id(id).one(db).await.map_err(map_err)? else {
            return Err(missing_scope("team", id, key_id));
        };
        return Ok(TenancyNodeRef::Team(TeamId::from_parts(team_model.org_id, id)));
    }
    if let Some(id) = scope_project_id {
        let Some(project_model) = project::Entity::find_by_id(id).one(db).await.map_err(map_err)? else {
            return Err(missing_scope("project", id, key_id));
        };
        return Ok(TenancyNodeRef::Project(ProjectId::from_parts(project_model.org_id, id)));
    }
    // Unreachable in practice: `ck_api_key_scope` guarantees exactly one column is set.
    // Surfaced as `Backend` (never a silent default) rather than a `panic!`/`unreachable!` in
    // case that invariant is ever broken by a future migration or a hand-edited row.
    Err(RepositoryError::Backend(Box::new(std::io::Error::other(format!("api_key {key_id} has no scope column set")))))
}

/// Builds the insertable `api_key` row from a domain `ApiKey` plus its already-hashed secret
/// (`issue`'s `key_hash: &[u8]` argument — the domain `ApiKey` itself never carries hash
/// material, D-api_key).
fn key_to_model(key: &ApiKey, key_hash: &[u8]) -> api_key::ActiveModel {
    let (scope_org_id, scope_team_id, scope_project_id) = scope_columns(&key.scope);
    api_key::ActiveModel {
        id: Set(key.id.uuid()),
        service_account_id: Set(key.service_account_id.uuid()),
        scope_org_id: Set(scope_org_id),
        scope_team_id: Set(scope_team_id),
        scope_project_id: Set(scope_project_id),
        prefix: Set(key.prefix.clone()),
        key_hash: Set(hex_encode(key_hash)),
        status: Set(key.status.as_str().to_string()),
        expires_at: Set(key.expires_at),
        last_used_at: Set(key.last_used_at),
        created_at: Set(key.created_at),
        revoked_at: Set(key.revoked_at),
        scope_actions: Set(actions_to_column(&key.scope_actions)),
        scope_roles: Set(roles_to_column(&key.scope_roles)),
    }
}

/// Re-parses a stored `api_key` row back into the pure-core `ApiKey`: a parse failure on
/// stored data becomes a `Backend` error (never a silent default) — the row was written by
/// this same adapter, so a failure here means the data is corrupt or the domain's parsing
/// rules changed underneath it (mirrors `pg_service_accounts.rs::model_to_sa`). The owning
/// service account is a bare uuid with no stored PRN of its own — like
/// `pg_service_accounts.rs`/`pg_role_grants.rs`'s `principal_id` handling, its PRN is
/// synthesized directly (a principal's PRN shape is fully deterministic: `iam`, no org,
/// `principal`, the uuid).
async fn model_to_key<C: ConnectionTrait>(db: &C, model: api_key::Model) -> Result<ApiKey, RepositoryError> {
    let backend = |msg: String| RepositoryError::Backend(Box::new(std::io::Error::other(msg)));
    let prn = Prn::build("iam", "", None, "principal", model.service_account_id).map_err(|e| backend(e.to_string()))?;
    let scope = scope_from_columns(db, model.id, model.scope_org_id, model.scope_team_id, model.scope_project_id).await?;
    let status = ApiKeyStatus::parse(&model.status).ok_or_else(|| backend(format!("unknown api_key status: {}", model.status)))?;
    let scope_actions = actions_from_column(model.scope_actions.as_deref())?;
    let scope_roles = roles_from_column(model.scope_roles.as_deref())?;

    Ok(ApiKey {
        id: ApiKeyId::from_uuid(model.id),
        service_account_id: PrincipalId::from_prn(prn),
        scope,
        prefix: model.prefix,
        status,
        expires_at: model.expires_at,
        last_used_at: model.last_used_at,
        created_at: model.created_at,
        revoked_at: model.revoked_at,
        scope_actions,
        scope_roles,
    })
}

/// `touch_last_used`'s guarded UPDATE (binding SQL): only bumps `last_used_at` when it's
/// currently `NULL` or older than `now - throttle_secs` — a single atomic statement so a
/// throttle-window race between concurrent hot-key touches can never both "win" (mirrors
/// `pg_memberships.rs`'s raw-SQL precedent for statements sea-orm's typed query builder
/// doesn't express cleanly).
const TOUCH_LAST_USED_SQL: &str = r#"UPDATE "api_key" SET last_used_at = $2 WHERE id = $1 AND (last_used_at IS NULL OR last_used_at < $3)"#;

#[async_trait]
impl ApiKeyRepository for PgApiKeyRepository {
    async fn issue(&self, key: &ApiKey, key_hash: &[u8]) -> Result<(), RepositoryError> {
        // Thin one-shot-`UnitOfWork` wrapper (SMA-446 Slice B Task B6), mirroring
        // `PgRoleGrantStore::grant`/`PgPolicyStore::put`: open a `SeaOrmTransaction`, drive
        // the insert via `issue_in`, commit.
        let txn = self.db.begin().await.map_err(map_err)?;
        let tx: Box<dyn Transaction> = Box::new(SeaOrmTransaction { txn });
        self.issue_in(&*tx, key, key_hash).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn issue_in(&self, tx: &dyn Transaction, key: &ApiKey, key_hash: &[u8]) -> Result<(), RepositoryError> {
        // A `uq_api_key_hash` violation surfaces here as `Conflict(ApiKeyHashCollision)`
        // (mod.rs's `conflict_kind`) — never silently swallowed, and no row is written.
        let txn = recover_txn(tx)?;
        key_to_model(key, key_hash).insert(txn).await.map_err(map_err)?;
        Ok(())
    }

    async fn find_by_id(&self, id: ApiKeyId) -> Result<Option<(ApiKey, Vec<u8>)>, RepositoryError> {
        let Some(model) = api_key::Entity::find_by_id(id.uuid()).one(&self.db).await.map_err(map_err)? else {
            return Ok(None);
        };
        let stored_hash = hex_decode(model.id, &model.key_hash)?;
        let key = model_to_key(&self.db, model).await?;
        Ok(Some((key, stored_hash)))
    }

    async fn revoke(&self, id: ApiKeyId, now: DateTime<Utc>) -> Result<(), RepositoryError> {
        // Thin one-shot-`UnitOfWork` wrapper, mirroring `issue` above: the `revoke_in` bool
        // (did this call actually flip Active -> Revoked) is not this method's concern —
        // `revoke` is a bare success/failure signal, unchanged from its pre-Slice-B contract,
        // and it's idempotent either way.
        let txn = self.db.begin().await.map_err(map_err)?;
        let tx: Box<dyn Transaction> = Box::new(SeaOrmTransaction { txn });
        self.revoke_in(&*tx, id, now).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn revoke_in(&self, tx: &dyn Transaction, id: ApiKeyId, now: DateTime<Utc>) -> Result<bool, RepositoryError> {
        // `FOR UPDATE` row lock, mirroring `pg_organizations.rs::set_status`'s txn+lock idiom
        // EXACTLY: a plain `find_by_id` + separate `.update()` races — two concurrent revokes
        // could both read an `Active` row and the later commit would re-stamp `revoked_at`.
        // `lock_exclusive` serializes them so the first revoke wins and any later contender
        // reads the already-revoked row here.
        let txn = recover_txn(tx)?;

        let Some(model) = api_key::Entity::find_by_id(id.uuid()).lock_exclusive().one(txn).await.map_err(map_err)? else {
            // Idempotent no-op (SMA-446 Slice B Task B6, port docs): nothing to revoke — the
            // caller (`ApiKeyService::revoke`) already resolved the key via `find_by_id`
            // before opening this transaction, so this only fires on a genuine TOCTOU race
            // (e.g. a concurrent revoke of the same id winning first).
            return Ok(false);
        };

        if model.status == ApiKeyStatus::Revoked.as_str() {
            // Idempotent: revoking an already-revoked key is a no-op — `revoked_at` stays
            // pinned to whenever it was FIRST set, never re-stamped to this call's `now` (the
            // lock above guarantees a concurrent first-revoke has already committed by the
            // time a contender observes this branch).
            return Ok(false);
        }

        let mut active = model.into_active_model();
        active.status = Set(ApiKeyStatus::Revoked.as_str().to_string());
        active.revoked_at = Set(Some(now));
        active.update(txn).await.map_err(map_err)?;
        Ok(true)
    }

    async fn list_by_service_account(&self, sa: &PrincipalId, limit: u64, offset: u64) -> Result<Vec<ApiKey>, RepositoryError> {
        let models = api_key::Entity::find()
            .filter(api_key::Column::ServiceAccountId.eq(sa.uuid()))
            .order_by_asc(api_key::Column::CreatedAt)
            .order_by_asc(api_key::Column::Id)
            .limit(limit)
            .offset(offset)
            .all(&self.db)
            .await
            .map_err(map_err)?;

        let mut keys = Vec::with_capacity(models.len());
        for model in models {
            keys.push(model_to_key(&self.db, model).await?);
        }
        Ok(keys)
    }

    async fn list_ids_by_service_account(&self, sa: &PrincipalId) -> Result<Vec<ApiKeyId>, RepositoryError> {
        let models = api_key::Entity::find().filter(api_key::Column::ServiceAccountId.eq(sa.uuid())).all(&self.db).await.map_err(map_err)?;
        Ok(models.into_iter().map(|m| ApiKeyId::from_uuid(m.id)).collect())
    }

    async fn touch_last_used(&self, id: ApiKeyId, now: DateTime<Utc>, throttle_secs: u64) -> Result<(), RepositoryError> {
        let threshold = now - chrono::Duration::seconds(throttle_secs as i64);
        let stmt = Statement::from_sql_and_values(DbBackend::Postgres, TOUCH_LAST_USED_SQL, [id.uuid().into(), now.into(), threshold.into()]);
        self.db.execute_raw(stmt).await.map_err(map_err)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Docker-independent coverage of the `key_hash` hex codec — the round-trip and the
    // malformed-input rejection paths (the `find_by_id` integration test in
    // `tests/service_accounts.rs` additionally proves the round-trip end-to-end through
    // Postgres, but only when Docker is available).

    #[test]
    fn hex_round_trips_arbitrary_bytes() {
        let bytes = [0x00u8, 0x0f, 0x10, 0xab, 0xcd, 0xef, 0xff];
        let encoded = hex_encode(&bytes);
        assert_eq!(encoded, "000f10abcdefff");
        assert_eq!(hex_decode(Uuid::nil(), &encoded).unwrap(), bytes.to_vec());
    }

    #[test]
    fn hex_decode_rejects_malformed_input_without_panicking() {
        // Non-ASCII (multi-byte UTF-8 — the panic the review flagged: `&s[i..i + 2]` would cut
        // through a char boundary), odd length, and a non-hex ASCII digit must all return
        // `Backend`, never panic. `"a€"` is 4 bytes (even) so it clears the length check and
        // exercises the byte-window path specifically.
        for bad in ["a€", "a€bc", "€€", "abc", "a", "zz", "0g", "  "] {
            let result = hex_decode(Uuid::nil(), bad);
            assert!(matches!(result, Err(RepositoryError::Backend(_))), "expected Backend for {bad:?}, got {result:?}");
        }
        // An empty string is even-length and decodes to no bytes (not an error).
        assert_eq!(hex_decode(Uuid::nil(), "").unwrap(), Vec::<u8>::new());
    }
}
