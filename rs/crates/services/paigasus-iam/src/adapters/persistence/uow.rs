// SPDX-License-Identifier: Apache-2.0

//! SeaORM-backed Unit-of-Work mechanism (SMA-446, Slice B — the de-risking spine).
//!
//! Concretises the pure-core [`UnitOfWork`]/[`Transaction`]/[`Savepoint`] ports over a single
//! SeaORM `DatabaseConnection`. The application layer `begin`s a [`SeaOrmTransaction`], drives
//! one or more txn-scoped mutations plus the outbox/audit writes through it, then commits — so
//! the aggregate mutation, its outbox row, and its audit row become visible atomically.
//!
//! # Recovering the concrete transaction (B2/B4 will copy this)
//!
//! The ports are backend-agnostic (ADR-0005): a txn-scoped adapter receives an opaque
//! `&dyn Transaction` and recovers the SeaORM `DatabaseTransaction` it needs as a
//! `ConnectionTrait` via `as_any().downcast_ref`. Use the [`recover_txn`] helper, which is
//! exactly:
//!
//! ```ignore
//! tx.as_any()
//!     .downcast_ref::<SeaOrmTransaction>()
//!     .map(|t| &t.txn)
//!     .ok_or_else(|| RepositoryError::Backend(Box::new(std::io::Error::other(
//!         "Transaction is not a SeaOrmTransaction",
//!     ))))
//! ```
//!
//! In-crate adapters (siblings of this module) may reach the field directly as `&t.txn`
//! (it is `pub(crate)`); the [`recover_txn`]/[`recover_savepoint_txn`] helpers are the
//! canonical, `pub` entry points so integration tests and out-of-crate callers get the same
//! error handling without touching internals.
//!
//! # Savepoints and the `as_any` / lifetime resolution
//!
//! [`Transaction::savepoint`] opens a SeaORM *nested* transaction (`self.txn.begin()`), which
//! SeaORM maps to a Postgres `SAVEPOINT`: a conflict-absorbing mutation can roll the savepoint
//! back without aborting the outer transaction (see `tests/uow_mechanism_pg.rs`, scenario 3).
//!
//! The port signature is `savepoint(&mut self) -> Result<Box<dyn Savepoint<'_>>, _>`, so the
//! returned box borrows `&mut self` for `'_`: the borrow checker keeps the outer transaction
//! locked while the savepoint is live (you cannot commit or re-savepoint the outer txn until
//! the savepoint is committed/rolled back and dropped), which matches SeaORM's single shared
//! connection. That borrow is enforced entirely by the trait signature at the *call site*.
//!
//! Crucially, [`SeaOrmSavepoint`] itself carries **no** lifetime parameter. The nested
//! `DatabaseTransaction` it owns is `'static` (it holds its own connection handle, it does not
//! borrow the parent), and `Savepoint::as_any(&self) -> &dyn Any` requires the concrete type to
//! be `'static` — `Any` is only implemented for `'static` types, and `downcast_ref` demands the
//! same. A lifetime-parameterised `SeaOrmSavepoint<'a>` (e.g. with a `PhantomData<&'a ()>`)
//! would be non-`'static` and could neither implement `Any` nor be downcast. Dropping the
//! struct lifetime and using a blanket `impl<'a> Savepoint<'a> for SeaOrmSavepoint` resolves
//! this cleanly while the trait signature still supplies the `&mut self` borrow at call sites.
//!
//! All SeaORM `DbErr`s from a UoW lifecycle op (begin/commit/savepoint/rollback) are mapped to
//! [`RepositoryError::Backend`] — these are infrastructure failures, never a domain conflict
//! (unlike the row-level `map_err` its sibling repository adapters use).

use async_trait::async_trait;
use paigasus_iam_core::{RepositoryError, Savepoint, Transaction, UnitOfWork};
use sea_orm::{DatabaseConnection, DatabaseTransaction, DbErr, TransactionTrait};
use std::any::Any;

/// Maps a SeaORM `DbErr` from a unit-of-work lifecycle operation to
/// [`RepositoryError::Backend`]. `begin`/`commit`/`savepoint`/`rollback` failures are always
/// infrastructure errors (a dropped connection, a protocol fault) — never a uniqueness or FK
/// conflict, which only the row-level writes B2/B4 run can raise — so this deliberately does
/// not attempt the constraint-name attribution that `persistence::map_err` does.
fn map_uow_err(e: DbErr) -> RepositoryError {
    RepositoryError::Backend(Box::new(e))
}

/// Opens one atomic unit of work backed by a single SeaORM transaction. Cloning is cheap:
/// `DatabaseConnection` is an `Arc`-backed pool handle (mirrors `PgAuditLog`), so the
/// composition root can hold a UoW inside a `#[derive(Clone)]` service.
#[derive(Clone)]
pub struct SeaOrmUnitOfWork {
    db: DatabaseConnection,
}

impl SeaOrmUnitOfWork {
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        SeaOrmUnitOfWork { db }
    }
}

#[async_trait]
impl UnitOfWork for SeaOrmUnitOfWork {
    async fn begin(&self) -> Result<Box<dyn Transaction>, RepositoryError> {
        let txn = self.db.begin().await.map_err(map_uow_err)?;
        Ok(Box::new(SeaOrmTransaction { txn }))
    }
}

/// A single in-flight SeaORM transaction. Adapters recover the inner `DatabaseTransaction`
/// (usable as a `ConnectionTrait`) via [`recover_txn`] / `as_any().downcast_ref`.
pub struct SeaOrmTransaction {
    /// The live SeaORM transaction. `pub(crate)` so in-crate adapters can reach it as `&t.txn`
    /// after a downcast (the documented recovery pattern); out-of-crate callers use
    /// [`recover_txn`].
    pub(crate) txn: DatabaseTransaction,
}

#[async_trait]
impl Transaction for SeaOrmTransaction {
    async fn commit(self: Box<Self>) -> Result<(), RepositoryError> {
        self.txn.commit().await.map_err(map_uow_err)
    }

    async fn savepoint(&mut self) -> Result<Box<dyn Savepoint<'_>>, RepositoryError> {
        // SeaORM maps a nested `begin` on a transaction to a Postgres `SAVEPOINT`. The nested
        // `DatabaseTransaction` is owned (`'static`) — it does not borrow `self.txn` — so the
        // returned box's `'_` borrow of `&mut self` is supplied by the trait signature, not by
        // the wrapped value (see the module doc).
        let sp = self.txn.begin().await.map_err(map_uow_err)?;
        Ok(Box::new(SeaOrmSavepoint { sp }))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A nested transaction opened via [`Transaction::savepoint`] — a Postgres `SAVEPOINT`.
/// Carries no lifetime parameter (see the module doc): the wrapped nested transaction is
/// `'static`, which is what `Any`/`downcast_ref` require, and the caller-side borrow of the
/// outer transaction is enforced by [`Transaction::savepoint`]'s `&mut self` signature.
pub struct SeaOrmSavepoint {
    /// The nested SeaORM transaction backing this savepoint. `pub(crate)` for the same reason
    /// as [`SeaOrmTransaction::txn`]; out-of-crate callers use [`recover_savepoint_txn`].
    pub(crate) sp: DatabaseTransaction,
}

#[async_trait]
impl<'a> Savepoint<'a> for SeaOrmSavepoint {
    async fn commit(self: Box<Self>) -> Result<(), RepositoryError> {
        // `RELEASE SAVEPOINT`: the savepoint's writes fold into the outer transaction.
        self.sp.commit().await.map_err(map_uow_err)
    }

    async fn rollback(self: Box<Self>) -> Result<(), RepositoryError> {
        // `ROLLBACK TO SAVEPOINT`: discards only the savepoint's writes (and clears an abort
        // left by a failed statement inside it), leaving the outer transaction usable.
        self.sp.rollback().await.map_err(map_uow_err)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// Recovers the concrete SeaORM `DatabaseTransaction` from an opaque [`Transaction`] port
/// object. This is the canonical downcast helper txn-scoped adapters (B2/B4) use to run writes
/// against the same transaction; a non-`SeaOrmTransaction` implementation (there is none in
/// production) surfaces as [`RepositoryError::Backend`] rather than a panic.
pub fn recover_txn(tx: &dyn Transaction) -> Result<&DatabaseTransaction, RepositoryError> {
    tx.as_any()
        .downcast_ref::<SeaOrmTransaction>()
        .map(|t| &t.txn)
        .ok_or_else(|| RepositoryError::Backend(Box::new(std::io::Error::other("Transaction is not a SeaOrmTransaction"))))
}

/// [`recover_txn`]'s savepoint analogue: recovers the nested `DatabaseTransaction` from an
/// opaque [`Savepoint`] port object, for adapters that run a conflict-absorbing write inside a
/// savepoint.
pub fn recover_savepoint_txn<'r>(sp: &'r dyn Savepoint<'_>) -> Result<&'r DatabaseTransaction, RepositoryError> {
    sp.as_any()
        .downcast_ref::<SeaOrmSavepoint>()
        .map(|s| &s.sp)
        .ok_or_else(|| RepositoryError::Backend(Box::new(std::io::Error::other("Savepoint is not a SeaOrmSavepoint"))))
}
