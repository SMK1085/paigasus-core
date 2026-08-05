// SPDX-License-Identifier: Apache-2.0

//! Postgres-backed [`SystemRowRetirer`] (SMA-481): the privileged, operator-triggered path
//! that deletes an orphaned system-owned `role`+`policy` chain, role row first (`fk_role_
//! template`/`fk_role_grant_role` are both restrict — the port's own module doc explains why
//! every locking read here takes `FOR UPDATE`, D6). This adapter deliberately bypasses
//! `PgPolicyStore::delete_in`'s `SystemImmutable` guard — that guard must keep holding for the
//! ordinary `DeletePolicy` API (D3); nothing reachable from an operator request other than this
//! adapter may skip it.

use super::entities::{policy, role, role_grant};
use super::pg_policies::{kind_from_str, map_db_err};
use super::uow::{SeaOrmTransaction, recover_txn};
use async_trait::async_trait;
use paigasus_iam_core::authz::roles as authz_roles;
use paigasus_iam_core::{AuthzError, GrantRef, RepositoryError, StoredPolicy, StoredRole, SurvivingGrants, SystemRowRetirer, Transaction};
use paigasus_kernel::Prn;
use sea_orm::{ColumnTrait, ConnectionTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryFilter, QueryOrder, QuerySelect, TransactionTrait};
use std::time::Duration;

// `Clone` lets the composition root hold a retirer handle inside a `#[derive(Clone)]` service
// (mirrors `PgPolicyStore`/`PgRoleGrantStore`'s precedent) — cheap: `DatabaseConnection` clones
// an `Arc`-backed pool handle.
#[derive(Clone)]
pub struct PgSystemRowRetirer {
    db: DatabaseConnection,
}

impl PgSystemRowRetirer {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgSystemRowRetirer { db }
    }
}

/// Maps a [`recover_txn`] failure (an opaque `&dyn Transaction` that isn't a
/// `SeaOrmTransaction` — never happens in production, only a misbuilt fake could trigger it)
/// into `AuthzError::Backend`, mirroring `pg_policies.rs`/`pg_role_grants.rs`'s own `map_txn_err`.
fn map_txn_err(e: RepositoryError) -> AuthzError {
    AuthzError::Backend(Box::new(e))
}

/// A stored-error helper for a corrupt/unparseable `principal_id` — a data-integrity break
/// (the row was written by `pg_role_grants.rs`'s own adapter), surfaced as `Backend`, never a
/// silent default (mirrors `pg_role_grants.rs::backend_err`).
fn backend_err(msg: impl std::fmt::Display) -> AuthzError {
    AuthzError::Backend(Box::new(std::io::Error::other(msg.to_string())))
}

/// Maps a stored `policy` row to the port's view. `kind_from_str` is the shared parse
/// `pg_policies.rs` uses for the same column — a value outside `{static, template}` is a
/// data-integrity break and must surface as `Backend`, never a silent default (unlike `pg_
/// policies.rs::stored_row`'s OWN internal degrade-to-`Static`, which feeds a classifier that
/// self-corrects a bad `kind` on converge; nothing here converges anything).
fn to_stored_policy(model: policy::Model) -> Result<StoredPolicy, AuthzError> {
    Ok(StoredPolicy {
        policy_id: model.policy_id,
        kind: kind_from_str(&model.kind)?,
        source: model.source,
        description: model.description.unwrap_or_default(),
        system: model.system,
    })
}

fn to_stored_role(model: role::Model) -> StoredRole {
    StoredRole { key: model.key, system: model.system }
}

/// Maps a stored `role_grant` row to the port's stringly-typed `GrantRef` — same PRN
/// reconstruction `pg_role_grants.rs::model_to_grant` uses: `scope_node_prn` is already the
/// scope's canonical PRN as written (`GrantScope::canonical_prn`, including the `root` case),
/// so it round-trips with no reparsing; `principal_id` carries no stored PRN of its own, so its
/// canonical PRN is synthesized the same way `model_to_grant` synthesizes it.
fn to_grant_ref(model: role_grant::Model) -> Result<GrantRef, AuthzError> {
    let principal_prn = Prn::build("iam", "", None, "principal", model.principal_id).map_err(backend_err)?;
    Ok(GrantRef {
        id: model.id.to_string(),
        principal_prn: principal_prn.canonical(),
        scope_prn: model.scope_node_prn,
    })
}

#[async_trait]
impl SystemRowRetirer for PgSystemRowRetirer {
    async fn begin_retirement(&self, lock_timeout: Duration) -> Result<Box<dyn Transaction>, AuthzError> {
        let txn = self.db.begin().await.map_err(map_db_err)?;
        // Mirrors `PgPolicyStore::reconcile_system`'s placement: immediately after `begin`,
        // before any locking read. Postgres takes an interval literal, so the duration is
        // rendered in milliseconds — an operator-triggered request must fail with a message
        // rather than hang behind a concurrent writer's row lock.
        //
        // Zero is clamped to 1ms, never rendered verbatim: Postgres reads `lock_timeout = '0'`
        // as "disable the timeout entirely", the exact INVERSE of this method's contract, and
        // hanging forever is precisely the failure mode being designed out on a privileged,
        // operator-triggered path. No caller passes `Duration::ZERO` today (`LOCK_TIMEOUT` is
        // 5s); a future one plainly means "do not wait", which 1ms delivers and `'0'` would not.
        //
        // The upper bound is clamped for the mirror-image reason: `as_millis()` is `u128`, but
        // Postgres rejects a `lock_timeout` above `INT_MAX` milliseconds, so an over-large
        // `Duration` would fail `begin_retirement` outright with an opaque `Backend` error
        // instead of yielding the long-but-usable timeout the caller asked for. Clamping both
        // ends keeps every `Duration` a caller can construct mapping to a legal statement.
        let millis = lock_timeout.as_millis().clamp(1, u128::from(i32::MAX.unsigned_abs()));
        txn.execute_unprepared(&format!("SET LOCAL lock_timeout = '{millis}ms';")).await.map_err(map_db_err)?;
        Ok(Box::new(SeaOrmTransaction { txn }))
    }

    async fn lock_policy_in(&self, tx: &dyn Transaction, policy_id: &str) -> Result<Option<StoredPolicy>, AuthzError> {
        let txn = recover_txn(tx).map_err(map_txn_err)?;
        let found = policy::Entity::find_by_id(policy_id.to_string()).lock_exclusive().one(txn).await.map_err(map_db_err)?;
        found.map(to_stored_policy).transpose()
    }

    async fn lock_role_in(&self, tx: &dyn Transaction, key: &str) -> Result<Option<StoredRole>, AuthzError> {
        let txn = recover_txn(tx).map_err(map_txn_err)?;
        let found = role::Entity::find_by_id(key.to_string()).lock_exclusive().one(txn).await.map_err(map_db_err)?;
        Ok(found.map(to_stored_role))
    }

    async fn surviving_grants_in(&self, tx: &dyn Transaction, role_key: &str, cap: u64) -> Result<SurvivingGrants, AuthzError> {
        let txn = recover_txn(tx).map_err(map_txn_err)?;
        // The COUNT and the page run on the SAME transaction that already holds the role row's
        // FOR UPDATE lock, so no grant can appear between them (D6).
        let total = role_grant::Entity::find().filter(role_grant::Column::RoleKey.eq(role_key)).count(txn).await.map_err(map_db_err)?;
        let models = role_grant::Entity::find()
            .filter(role_grant::Column::RoleKey.eq(role_key))
            .order_by_asc(role_grant::Column::Id)
            .limit(cap)
            .all(txn)
            .await
            .map_err(map_db_err)?;
        let grants = models.into_iter().map(to_grant_ref).collect::<Result<Vec<_>, _>>()?;
        Ok(SurvivingGrants { grants, total })
    }

    async fn min_starter_revision(&self) -> Result<Option<u32>, AuthzError> {
        // The CODE-DEFINED starter policy ids only (D11) — deliberately NOT `system = true`.
        // `system = true` also matches the orphan currently being retired, and an orphan's own
        // `starter_revision` is by construction the revision of the last binary that still
        // DEFINED it: dropping an id from `starter_policies()` changes the starter content hash,
        // which forces a `STARTER_POLICY_REVISION` bump, and `reconcile_policies` only iterates
        // code-defined docs so nothing ever restamps the orphan. An orphan is therefore always
        // strictly below this binary (or NULL for a pre-m0010 row) — counting it would refuse
        // every real orphan, forever. The starter rows are exactly the set THIS binary converged
        // at boot, so their revision is genuine evidence about what the rest of the fleet wrote.
        let revisions: Vec<Option<i32>> = policy::Entity::find()
            .select_only()
            .column(policy::Column::StarterRevision)
            .filter(policy::Column::PolicyId.is_in(authz_roles::STARTER_POLICY_IDS.iter().copied()))
            .into_tuple()
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        // No starter rows at all (an unseeded database, or one whose boot convergence never
        // ran) is NOT convergence — it is the absence of any evidence, and an empty set's
        // minimum must never silently read as proof. `None` here refuses, same as a NULL.
        if revisions.is_empty() {
            return Ok(None);
        }
        // Any NULL means "unprovable", never "zero" — a pre-m0010 row proves nothing about
        // which binary last wrote it, and reading it as 0 would be the safe-sounding direction
        // that silently permits the retirement D11 exists to defer.
        if revisions.iter().any(Option::is_none) {
            return Ok(None);
        }
        // The column is Postgres `i32` (no unsigned integer type exists), but every value this
        // service itself ever writes is a non-negative `u32` cast up (`pg_policies.rs::
        // converged_model`), so a negative value can only come from a hand edit — the same
        // data-integrity-break posture `kind_from_str` takes for a `kind` outside its domain.
        // Unlike the NULL case above, there is no safe direction to coerce a negative into:
        // clamping it to `0` would read as "oldest possible" and defer retirement behind a row
        // that is actually just corrupt, not genuinely old. Surface it loudly instead of
        // guessing which way a silent default should lean.
        revisions
            .into_iter()
            .flatten()
            .map(|r| u32::try_from(r).map_err(|_| backend_err(format!("policy.starter_revision is negative: {r}"))))
            .collect::<Result<Vec<u32>, AuthzError>>()
            .map(|revisions| revisions.into_iter().min())
    }

    async fn delete_role_in(&self, tx: &dyn Transaction, key: &str) -> Result<bool, AuthzError> {
        let txn = recover_txn(tx).map_err(map_txn_err)?;
        let result = role::Entity::delete_by_id(key.to_string()).exec(txn).await.map_err(map_db_err)?;
        Ok(result.rows_affected > 0)
    }

    async fn delete_policy_in(&self, tx: &dyn Transaction, policy_id: &str) -> Result<bool, AuthzError> {
        let txn = recover_txn(tx).map_err(map_txn_err)?;
        // No pre-check here (unlike `PgPolicyStore::delete_in`'s `SystemImmutable` guard):
        // this port's whole reason to exist is deleting a system-owned row (module doc). A
        // surviving `role` row still referencing this policy surfaces as `fk_role_template`
        // violation -> `Backend`, which is exactly right — the caller must have already
        // deleted the role row first (the only order the schema permits).
        let result = policy::Entity::delete_by_id(policy_id.to_string()).exec(txn).await.map_err(map_db_err)?;
        Ok(result.rows_affected > 0)
    }
}
