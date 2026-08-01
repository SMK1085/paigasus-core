// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed `PolicyStore` (SeaORM). `put`/`delete` (SMA-446, Slice B Task B5) are
//! thin one-shot-`UnitOfWork` wrappers around [`PolicyStore::put_in`]/[`PolicyStore::
//! delete_in`] — the exact reference pattern `PgRoleGrantStore::grant`/`revoke` establish
//! over `grant_in`/`revoke_in` (B4): open a `SeaOrmTransaction`, drive the write on it,
//! commit, then best-effort bump. `put_in` validates the Cedar source against the embedded
//! schema before touching the database, and — like `delete_in` — rejects any mutation of an
//! existing `system = true` row (`AuthzError::SystemImmutable`); a *new* row with
//! `system = true` (the seeding path, Task 17) is not rejected, only edits/deletes of an
//! already-persisted system row are. `list_all` reads every row in one REPEATABLE READ
//! transaction so a concurrent write can't tear the snapshot (spec §6.2 challenge MINOR).
//! `put`/`delete` best-effort bump `policy_gen` via the shared `Generations` handle (spec
//! §7/D11: "bumped on any policy CRUD or role grant/revoke") — logged and swallowed on
//! error, mirroring `pg_organizations.rs::bump_entity_gen`: the write already committed, so
//! a Redis-down bump failure must never fail it, it just means the change lands on the policy
//! snapshot's TTL backstop (`policy_cache_ttl_secs + refresh_interval_secs`) instead of
//! immediately — the decision cache follows for free, since its key's policy component is the
//! compiled set's `content_hash` (SMA-470 D4), which rotates the moment that reload installs.
//!
//! `put_in`'s INSERT branch handles a unique-constraint violation via a SAVEPOINT (SMA-446
//! Slice B Task B5 — preserving the pre-Slice-B "abort whole txn, re-read on a fresh
//! connection" semantics WITHOUT the fresh connection): the INSERT runs on a nested SeaORM
//! transaction opened directly off the recovered `&DatabaseTransaction` via
//! `TransactionTrait::begin` (which only needs `&self` — deliberately NOT through
//! [`crate::adapters::persistence::uow::SeaOrmTransaction`]'s own `Transaction::savepoint`,
//! whose `&mut self` receiver `put_in`'s `&dyn Transaction` argument can't supply). A unique
//! violation rolls the savepoint back (`ROLLBACK TO SAVEPOINT`) — the caller's outer `tx`
//! stays alive and usable — then re-reads the row that won the race WITHIN that same outer
//! txn and compares content: SAME content (a concurrent cold-boot `reconcile_starter` race
//! between replicas inserting the identical starter policy, Task 17 follow-up) absorbs as
//! [`PutOutcome::AbsorbedIdempotent`], mirroring `bootstrap.rs::seed_role_row`; DIFFERENT
//! content (two `PutPolicy` API callers racing to create the same `policy_id`) is a genuine
//! lost-update conflict and surfaces as `AuthzError::Conflict` — see the inline comments on
//! that branch. `put_in`/`delete_in` themselves never bump `policy_gen` (the caller's own
//! awaited, post-commit responsibility, and — for `put_in` — skipped entirely on
//! `AbsorbedIdempotent`, since the winning writer already bumped it for this row).

use super::entities::policy;
use super::uow::{SeaOrmTransaction, recover_txn};
use crate::adapters::authz::Generations;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use paigasus_iam_core::authz::model::PolicyKind;
use paigasus_iam_core::authz::schema::validate_policy;
use paigasus_iam_core::{AuthzError, PolicyDocument, PolicyStore, PutOutcome, RepositoryError, Transaction};
use sea_orm::{ActiveModelTrait, DatabaseConnection, DbErr, EntityTrait, IsolationLevel, QuerySelect, Set, SqlErr, TransactionTrait};

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

    /// Best-effort `policy_gen` bump (spec §7/D11): logged and swallowed on error — mirrors
    /// `pg_organizations.rs::bump_entity_gen`/`bump_policy_gen` exactly: `put`/`delete`'s
    /// mutation already committed, so a Redis-down bump failure must never fail an
    /// already-successful write; it just means the change lands on the policy snapshot's TTL
    /// backstop — `policy_cache_ttl_secs + refresh_interval_secs`, NOT the decision cache's own
    /// TTL — instead of immediately (D11: a swallowed bump degrades to backstop-bounded
    /// staleness). The decision cache needs no expiry of its own to follow: its key's policy
    /// component is the compiled set's `content_hash` (SMA-470 D4), so that reload rotates the
    /// key space for it — which is also why the claim has to be phrased this way, since
    /// `MemoryDecisionCache` has no TTL at all. Distinct from the public
    /// `PolicyStore::bump_policy_gen` trait method (which forces a bump and propagates a
    /// failure to the caller by design).
    async fn bump_policy_gen_best_effort(&self) {
        if let Err(err) = self.gens.bump_policy_gen().await {
            tracing::warn!(error = %err, "pg_policies: policy_gen bump failed after a committed write — authz decisions may be stale until the policy snapshot's TTL backstop reloads");
        }
    }
}

/// Compares a stored `policy` row's content-bearing fields against an incoming
/// `PolicyDocument` — used by `put`'s unique-constraint-violation branch to distinguish an
/// idempotent same-content race from a genuine different-content conflict. Timestamps are
/// deliberately excluded: the two racers' `created_at`/`updated_at` differ by construction
/// even when the policy content itself is identical.
fn policy_content_matches(stored: &policy::Model, doc: &PolicyDocument) -> bool {
    stored.kind == kind_to_str(doc.kind)
        && stored.source == doc.source
        && stored.description == if doc.description.is_empty() { None } else { Some(doc.description.clone()) }
        && stored.system == doc.system
}

fn map_err(e: DbErr) -> AuthzError {
    AuthzError::Backend(Box::new(e))
}

/// Maps a [`recover_txn`] failure (an opaque `&dyn Transaction` that isn't a
/// `SeaOrmTransaction` — never happens in production, only a misbuilt fake could trigger it)
/// into `AuthzError::Backend`, mirroring `pg_role_grants.rs`'s `map_txn_err`.
fn map_txn_err(e: RepositoryError) -> AuthzError {
    AuthzError::Backend(Box::new(e))
}

/// A stored-error helper for the unique-violation-but-no-row-on-re-read shouldn't-happen
/// case — a data-integrity break, surfaced as `Backend`, never a silent default (mirrors
/// `pg_role_grants.rs::backend_err`).
fn backend_err(msg: impl std::fmt::Display) -> AuthzError {
    AuthzError::Backend(Box::new(std::io::Error::other(msg.to_string())))
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
        // Thin one-shot-`UnitOfWork` wrapper (SMA-446 Slice B Task B5), mirroring
        // `PgRoleGrantStore::grant`: open a `SeaOrmTransaction`, drive the write via
        // `put_in`, commit, then best-effort bump — skipped entirely when the outcome is an
        // idempotent absorb (module docs: the winning writer already bumped for this row).
        let txn = self.db.begin().await.map_err(map_err)?;
        let tx: Box<dyn Transaction> = Box::new(SeaOrmTransaction { txn });
        let outcome = self.put_in(&*tx, doc).await?;
        tx.commit().await.map_err(map_txn_err)?;
        if !matches!(outcome, PutOutcome::AbsorbedIdempotent) {
            self.bump_policy_gen_best_effort().await;
        }
        Ok(())
    }

    async fn delete(&self, policy_id: &str) -> Result<(), AuthzError> {
        // Thin one-shot-`UnitOfWork` wrapper, mirroring `put` above / `PgRoleGrantStore::
        // revoke`.
        let txn = self.db.begin().await.map_err(map_err)?;
        let tx: Box<dyn Transaction> = Box::new(SeaOrmTransaction { txn });
        let existed = self.delete_in(&*tx, policy_id).await?;
        tx.commit().await.map_err(map_txn_err)?;
        if existed {
            self.bump_policy_gen_best_effort().await;
        }
        Ok(())
    }

    async fn put_in(&self, tx: &dyn Transaction, doc: &PolicyDocument) -> Result<PutOutcome, AuthzError> {
        validate_policy(&doc.source)?;

        let txn = recover_txn(tx).map_err(map_txn_err)?;

        let existing = policy::Entity::find_by_id(doc.policy_id.clone()).lock_exclusive().one(txn).await.map_err(map_err)?;
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
                active.update(txn).await.map_err(map_err)?;
                Ok(PutOutcome::Updated)
            }
            None => {
                // Our existence check (`existing == None`) and this INSERT aren't atomic:
                // two replicas/callers can both see an absent row and both attempt to insert
                // the same `policy_id`. Run the INSERT on a SAVEPOINT — a nested SeaORM
                // transaction opened directly off the recovered `&DatabaseTransaction`
                // (`TransactionTrait::begin` only needs `&self`, so this works without `&mut`
                // on `put_in`'s own `&dyn Transaction` receiver — see the module doc's borrow
                // note) — so a unique-constraint violation rolls back only the savepoint, not
                // the caller's outer `tx`.
                let active = doc_to_model(doc, doc.created_at);
                let sp = txn.begin().await.map_err(map_err)?;
                match active.insert(&sp).await {
                    Ok(_) => {
                        sp.commit().await.map_err(map_err)?;
                        Ok(PutOutcome::Inserted)
                    }
                    Err(e) if matches!(e.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) => {
                        // `ROLLBACK TO SAVEPOINT`: discards only the failed INSERT — the
                        // caller's outer `tx` stays alive and usable (savepoint isolation).
                        sp.rollback().await.map_err(map_err)?;

                        // Re-read the row that won the race, WITHIN the same outer UoW txn
                        // (no fresh connection needed — the savepoint rollback cleared the
                        // abort without touching `txn`), and compare its content to what we
                        // tried to write:
                        //   - SAME content (the cold-boot `reconcile_starter` race, SMA-444
                        //     Task 17, where both replicas race to insert the IDENTICAL
                        //     starter policy against a fresh/unseeded DB) — the upsert intent
                        //     ("this policy_id is present with this content") is satisfied
                        //     either way; absorb it as `PutOutcome::AbsorbedIdempotent`,
                        //     mirroring `bootstrap.rs::seed_role_row`'s absorption. The
                        //     caller skips its post-commit `policy_gen` bump on this outcome:
                        //     the winning writer's own `put`/`put_in` call already bumped it
                        //     for this row's creation, so bumping again here would just be a
                        //     redundant invalidation signal for a change we didn't make.
                        //   - DIFFERENT content (two callers of the public `PutPolicy` API
                        //     racing to CREATE the same `policy_id` with different documents)
                        //     — the loser's write was silently discarded; that's a lost
                        //     update, not an idempotent no-op, so it must be surfaced as a
                        //     conflict, never a silent success.
                        let winner = policy::Entity::find_by_id(doc.policy_id.clone())
                            .one(txn)
                            .await
                            .map_err(map_err)?
                            .ok_or_else(|| backend_err(format!("policy {}: unique-constraint violation on insert but no row found on re-read", doc.policy_id)))?;

                        if policy_content_matches(&winner, doc) {
                            Ok(PutOutcome::AbsorbedIdempotent)
                        } else {
                            Err(AuthzError::Conflict(doc.policy_id.clone()))
                        }
                    }
                    // Any OTHER `DbErr` still propagates as `Backend` — only a
                    // unique-constraint violation gets the race-absorption treatment above.
                    // The failed INSERT aborted only the savepoint (already rolled back
                    // implicitly by dropping `sp` here without commit); the outer `tx`
                    // remains usable by the caller.
                    Err(e) => Err(map_err(e)),
                }
            }
        }
    }

    async fn delete_in(&self, tx: &dyn Transaction, policy_id: &str) -> Result<bool, AuthzError> {
        let txn = recover_txn(tx).map_err(map_txn_err)?;

        let Some(existing) = policy::Entity::find_by_id(policy_id.to_string()).lock_exclusive().one(txn).await.map_err(map_err)? else {
            // Idempotent: nothing to delete, nothing to invalidate — mirrors
            // `RoleGrantStore::revoke_in`'s posture.
            return Ok(false);
        };
        if existing.system {
            return Err(AuthzError::SystemImmutable(policy_id.to_string()));
        }

        policy::Entity::delete_by_id(policy_id.to_string()).exec(txn).await.map_err(map_err)?;
        Ok(true)
    }

    async fn policy_gen(&self) -> Result<u64, AuthzError> {
        self.gens.policy_gen().await
    }

    async fn bump_policy_gen(&self) -> Result<u64, AuthzError> {
        self.gens.bump_policy_gen().await
    }
}
