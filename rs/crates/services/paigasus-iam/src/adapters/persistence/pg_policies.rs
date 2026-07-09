// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed `PolicyStore` (SeaORM). `put` validates the Cedar source against the
//! embedded schema before touching the database, and — like `delete` — rejects any
//! mutation of an existing `system = true` row (`AuthzError::SystemImmutable`); a *new*
//! row with `system = true` (the seeding path, Task 17) is not rejected, only edits/deletes
//! of an already-persisted system row are. `list_all` reads every row in one REPEATABLE
//! READ transaction so a concurrent write can't tear the snapshot (spec §6.2 challenge
//! MINOR). Every successful mutation bumps `policy_gen` via the shared `Generations`
//! handle (spec §7/D11: "bumped on any policy CRUD or role grant/revoke").

use super::entities::policy;
use crate::adapters::authz::Generations;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use paigasus_iam_core::authz::model::PolicyKind;
use paigasus_iam_core::authz::schema::validate_policy;
use paigasus_iam_core::{AuthzError, PolicyDocument, PolicyStore};
use sea_orm::{ActiveModelTrait, DatabaseConnection, DbErr, EntityTrait, IsolationLevel, QuerySelect, Set, TransactionTrait};

// `Clone` lets the composition root hold a store handle inside a `#[derive(Clone))]`
// service (mirrors `PgOrganizationRepository`'s precedent) — cheap: `DatabaseConnection`
// clones an `Arc`-backed pool handle, and `Generations` is `Arc`-backed too.
#[derive(Clone)]
pub struct PgPolicyStore {
    db: DatabaseConnection,
    gens: Generations,
}

impl PgPolicyStore {
    #[must_use]
    pub fn new(db: DatabaseConnection, gens: Generations) -> Self {
        PgPolicyStore { db, gens }
    }
}

fn map_err(e: DbErr) -> AuthzError {
    AuthzError::Backend(Box::new(e))
}

fn kind_to_str(kind: PolicyKind) -> &'static str {
    match kind {
        PolicyKind::Static => "static",
        PolicyKind::Template => "template",
    }
}

/// A stored `kind` value outside `{static, template}` is a data-integrity break (the
/// `ck_policy_kind` CHECK should prevent it) — surfaced as `Backend`, never a silent
/// default, mirroring `pg_memberships.rs::parse_status`.
fn kind_from_str(raw: &str) -> Result<PolicyKind, AuthzError> {
    match raw {
        "static" => Ok(PolicyKind::Static),
        "template" => Ok(PolicyKind::Template),
        other => Err(AuthzError::Backend(Box::new(std::io::Error::other(format!("bad policy kind: {other}"))))),
    }
}

/// Builds the insertable/updatable `policy` row from a domain `PolicyDocument`. An empty
/// `description` is stored as `NULL` (the column is nullable; the domain field is not) so
/// an untouched-by-hand row round-trips byte-for-byte either way.
///
/// `created_at` is threaded in separately from `doc`: on INSERT it's `doc.created_at` (the
/// row is new), but on UPDATE the caller must pass the *stored* row's `created_at` — the
/// incoming `doc.created_at` is untrusted client input and must never overwrite the
/// original creation timestamp.
fn doc_to_model(doc: &PolicyDocument, created_at: DateTime<Utc>) -> policy::ActiveModel {
    policy::ActiveModel {
        policy_id: Set(doc.policy_id.clone()),
        kind: Set(kind_to_str(doc.kind).to_string()),
        source: Set(doc.source.clone()),
        description: Set(if doc.description.is_empty() { None } else { Some(doc.description.clone()) }),
        system: Set(doc.system),
        created_at: Set(created_at),
        updated_at: Set(doc.updated_at),
    }
}

fn model_to_doc(model: policy::Model) -> Result<PolicyDocument, AuthzError> {
    Ok(PolicyDocument {
        policy_id: model.policy_id,
        kind: kind_from_str(&model.kind)?,
        source: model.source,
        description: model.description.unwrap_or_default(),
        system: model.system,
        created_at: model.created_at,
        updated_at: model.updated_at,
    })
}

#[async_trait]
impl PolicyStore for PgPolicyStore {
    async fn list_all(&self) -> Result<Vec<PolicyDocument>, AuthzError> {
        // REPEATABLE READ: every row this transaction sees is a single consistent snapshot,
        // even if a concurrent `put`/`delete` commits mid-read (port doc contract).
        let txn = self.db.begin_with_config(Some(IsolationLevel::RepeatableRead), None).await.map_err(map_err)?;
        let models = policy::Entity::find().all(&txn).await.map_err(map_err)?;
        txn.commit().await.map_err(map_err)?;

        models.into_iter().map(model_to_doc).collect()
    }

    async fn put(&self, doc: &PolicyDocument) -> Result<(), AuthzError> {
        validate_policy(&doc.source)?;

        let txn = self.db.begin().await.map_err(map_err)?;

        let existing = policy::Entity::find_by_id(doc.policy_id.clone()).lock_exclusive().one(&txn).await.map_err(map_err)?;
        if let Some(existing) = &existing
            && existing.system
        {
            return Err(AuthzError::SystemImmutable(doc.policy_id.clone()));
        }

        // On UPDATE, preserve the stored row's `created_at` — only INSERT takes it from
        // `doc` (the row is genuinely new then). This must read the fetched `existing`, not
        // `doc.system`/`doc.created_at`, or a caller could silently rewrite history.
        match existing {
            Some(existing) => {
                let active = doc_to_model(doc, existing.created_at);
                active.update(&txn).await.map_err(map_err)?;
            }
            None => {
                let active = doc_to_model(doc, doc.created_at);
                active.insert(&txn).await.map_err(map_err)?;
            }
        }

        txn.commit().await.map_err(map_err)?;
        self.gens.bump_policy_gen().await?;
        Ok(())
    }

    async fn delete(&self, policy_id: &str) -> Result<(), AuthzError> {
        let txn = self.db.begin().await.map_err(map_err)?;

        let Some(existing) = policy::Entity::find_by_id(policy_id.to_string()).lock_exclusive().one(&txn).await.map_err(map_err)? else {
            // Idempotent: nothing to delete, nothing to invalidate.
            txn.commit().await.map_err(map_err)?;
            return Ok(());
        };
        if existing.system {
            return Err(AuthzError::SystemImmutable(policy_id.to_string()));
        }

        policy::Entity::delete_by_id(policy_id.to_string()).exec(&txn).await.map_err(map_err)?;

        txn.commit().await.map_err(map_err)?;
        self.gens.bump_policy_gen().await?;
        Ok(())
    }

    async fn policy_gen(&self) -> Result<u64, AuthzError> {
        self.gens.policy_gen().await
    }

    async fn bump_policy_gen(&self) -> Result<u64, AuthzError> {
        self.gens.bump_policy_gen().await
    }
}
