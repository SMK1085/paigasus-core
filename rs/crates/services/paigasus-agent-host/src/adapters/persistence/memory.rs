// SPDX-License-Identifier: Apache-2.0

//! The Postgres `MemoryStore` (SMA-789 spec §6.8).
//!
//! Every query binds all three scope columns (`org_id`, `agent_id`, `principal_id`). A version is
//! a value of the sequence `memory_file_version_seq` as decimal text, so a version never repeats
//! for a path. `apply` and `apply_in` take the scope row with `SELECT … FOR UPDATE` first: that
//! row lock serializes the two writers of a scope, and it gives the cap check its `before` value.
//!
//! The `Drop` of a `DatabaseTransaction` can panic (see `migration_lock.rs`), so every error path
//! rolls back explicitly.

use async_trait::async_trait;
use paigasus_agent_host_core::memory::{Expected, MemoryCaps, MemoryChange, MemoryCommitSet, MemoryDir, MemoryPath, MemoryScope, MemoryVersion};
use paigasus_agent_host_core::ports::{MemoryApplyError, MemoryEntry, MemoryFile, MemoryStore, MemoryStoreError, MemoryUsage};
use sea_orm::{ConnectionTrait, DatabaseBackend, DatabaseConnection, DatabaseTransaction, DbErr, QueryResult, Statement, TransactionTrait, Value};

/// The `WHERE` term of every query on one scope. The scope is always `$1`, `$2` and `$3`.
const SCOPE: &str = "org_id = $1 AND agent_id = $2 AND principal_id = $3";

/// The Postgres memory store.
#[derive(Debug, Clone)]
pub struct PgMemoryStore {
    db: DatabaseConnection,
}

impl PgMemoryStore {
    /// Build a store on a pool. The schema of m0002 must exist.
    #[must_use]
    pub fn new(db: DatabaseConnection) -> Self {
        PgMemoryStore { db }
    }

    /// Apply a change set inside a transaction that the caller owns (spec §6.12).
    ///
    /// It opens a savepoint on `txn`. On success it releases the savepoint, and the changes
    /// commit with the caller's transaction. On any error it rolls back to the savepoint and
    /// returns the error, so the caller's transaction stays usable and holds no part of the
    /// change set. It never commits or rolls back `txn`. The scope row lock is held until `txn`
    /// ends.
    ///
    /// It expects `txn` at READ COMMITTED, the Postgres default. Under REPEATABLE READ or
    /// SERIALIZABLE, a console write that commits on the same scope after the snapshot of `txn`
    /// makes the `SELECT … FOR UPDATE` fail with a serialization error, and that maps to
    /// `MemoryApplyError::Store`.
    pub async fn apply_in(&self, txn: &DatabaseTransaction, set: &MemoryCommitSet, caps: &MemoryCaps) -> Result<(), MemoryApplyError> {
        let savepoint = txn.begin().await.map_err(apply_err)?;
        let result = apply_body(&savepoint, set, caps).await;
        finish(savepoint, result).await
    }
}

#[async_trait]
impl MemoryStore for PgMemoryStore {
    async fn get(&self, scope: &MemoryScope, path: &MemoryPath) -> Result<Option<MemoryFile>, MemoryStoreError> {
        let row = self
            .db
            .query_one_raw(stmt(
                &format!("SELECT content, version FROM memory_file WHERE {SCOPE} AND path = $4"),
                with_scope(scope, [path.as_str().into()]),
            ))
            .await
            .map_err(store_err)?;
        row.map(|r| {
            Ok(MemoryFile {
                content: r.try_get("", "content").map_err(store_err)?,
                version: version_of(&r)?,
            })
        })
        .transpose()
    }

    async fn list(&self, scope: &MemoryScope, dir: &MemoryDir) -> Result<Vec<MemoryEntry>, MemoryStoreError> {
        let rows = self
            .db
            .query_all_raw(stmt(
                &format!("SELECT path, size_bytes, version FROM memory_file WHERE {SCOPE} AND path LIKE $4 ESCAPE '\\' ORDER BY path"),
                with_scope(scope, [like_prefix(dir).into()]),
            ))
            .await
            .map_err(store_err)?;
        rows.iter()
            .map(|r| {
                let path: String = r.try_get("", "path").map_err(store_err)?;
                let size: i32 = r.try_get("", "size_bytes").map_err(store_err)?;
                Ok(MemoryEntry {
                    path: MemoryPath::parse(&path).map_err(|_| MemoryStoreError::Store("a stored path is not valid".to_string()))?,
                    size_bytes: u64::try_from(size).map_err(|e| MemoryStoreError::Store(e.to_string()))?,
                    version: version_of(r)?,
                })
            })
            .collect()
    }

    async fn usage(&self, scope: &MemoryScope) -> Result<MemoryUsage, MemoryStoreError> {
        let row = self
            .db
            .query_one_raw(stmt(&format!("SELECT files, bytes FROM memory_scope WHERE {SCOPE}"), scope_values(scope)))
            .await
            .map_err(store_err)?;
        row.map_or(Ok(MemoryUsage::default()), |r| usage_of(&r).map_err(store_err))
    }

    async fn apply(&self, set: &MemoryCommitSet, caps: &MemoryCaps) -> Result<(), MemoryApplyError> {
        let txn = self.db.begin().await.map_err(apply_err)?;
        let result = apply_body(&txn, set, caps).await;
        finish(txn, result).await
    }

    async fn delete_scope(&self, scope: &MemoryScope) -> Result<(), MemoryStoreError> {
        let txn = self.db.begin().await.map_err(store_err)?;
        // The scope row lock comes first, as in `apply`: the row is inserted when it does not
        // exist, then locked. So a concurrent first `apply` of this scope waits for the erasure,
        // or the erasure waits for it. Without the insert, an `UPDATE` of a missing row locks
        // nothing, and the counters can disagree with the files.
        let result = async {
            txn.execute_raw(stmt(
                "INSERT INTO memory_scope (org_id, agent_id, principal_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
                scope_values(scope),
            ))
            .await?;
            txn.query_one_raw(stmt(&format!("SELECT files FROM memory_scope WHERE {SCOPE} FOR UPDATE"), scope_values(scope)))
                .await?;
            txn.execute_raw(stmt(&format!("UPDATE memory_scope SET files = 0, bytes = 0 WHERE {SCOPE}"), scope_values(scope)))
                .await?;
            txn.execute_raw(stmt(&format!("DELETE FROM memory_file WHERE {SCOPE}"), scope_values(scope))).await?;
            Ok::<(), DbErr>(())
        }
        .await;
        match result {
            Ok(()) => txn.commit().await.map_err(store_err),
            Err(e) => {
                let _ = txn.rollback().await;
                Err(store_err(e))
            }
        }
    }
}

/// The work of `apply` and `apply_in` on one transaction (spec §6.8). The caller commits or
/// rolls back.
async fn apply_body(txn: &DatabaseTransaction, set: &MemoryCommitSet, caps: &MemoryCaps) -> Result<(), MemoryApplyError> {
    // A set with no change takes no scope row and no lock.
    if set.changes.is_empty() {
        return Ok(());
    }
    let scope = &set.scope;
    txn.execute_raw(stmt(
        "INSERT INTO memory_scope (org_id, agent_id, principal_id) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING",
        scope_values(scope),
    ))
    .await
    .map_err(apply_err)?;
    let locked = txn
        .query_one_raw(stmt(&format!("SELECT files, bytes FROM memory_scope WHERE {SCOPE} FOR UPDATE"), scope_values(scope)))
        .await
        .map_err(apply_err)?
        .ok_or_else(|| MemoryApplyError::Store("the memory scope row is missing after its insert".to_string()))?;
    let before = usage_of(&locked).map_err(apply_err)?;

    for change in set.changes.changes() {
        let conflict = || MemoryApplyError::Conflict { path: change.path().clone() };
        let (sql, extra): (String, Vec<Value>) = match change {
            MemoryChange::Put {
                path,
                content,
                expected: Expected::Absent,
            } => (
                "INSERT INTO memory_file (org_id, agent_id, principal_id, path, content) VALUES ($1, $2, $3, $4, $5) ON CONFLICT DO NOTHING".to_string(),
                vec![path.as_str().into(), content.as_str().into()],
            ),
            MemoryChange::Put {
                path,
                content,
                expected: Expected::Version(v),
            } => (
                format!(
                    "UPDATE memory_file SET content = $5, version = nextval('memory_file_version_seq'), updated_at = now() \
                     WHERE {SCOPE} AND path = $4 AND version = $6"
                ),
                vec![path.as_str().into(), content.as_str().into(), parse_version(v).ok_or_else(conflict)?.into()],
            ),
            MemoryChange::Delete { path, expected } => (
                format!("DELETE FROM memory_file WHERE {SCOPE} AND path = $4 AND version = $5"),
                vec![path.as_str().into(), parse_version(expected).ok_or_else(conflict)?.into()],
            ),
        };
        let done = txn.execute_raw(stmt(&sql, with_scope(scope, extra))).await.map_err(apply_err)?;
        if done.rows_affected() == 0 {
            return Err(conflict());
        }
        if let MemoryChange::Put { path, content, .. } = change {
            caps.check_file(path, content.len() as u64).map_err(MemoryApplyError::CapExceeded)?;
        }
    }

    let counted = txn
        .query_one_raw(stmt(
            &format!("SELECT count(*)::bigint AS files, coalesce(sum(size_bytes), 0)::bigint AS bytes FROM memory_file WHERE {SCOPE}"),
            scope_values(scope),
        ))
        .await
        .map_err(apply_err)?
        .ok_or_else(|| MemoryApplyError::Store("the count query returned no row".to_string()))?;
    let after = usage_of(&counted).map_err(apply_err)?;
    caps.check(before, after).map_err(MemoryApplyError::CapExceeded)?;
    txn.execute_raw(stmt(
        &format!("UPDATE memory_scope SET files = $4, bytes = $5 WHERE {SCOPE}"),
        with_scope(scope, [to_i64(after.files)?.into(), to_i64(after.bytes)?.into()]),
    ))
    .await
    .map_err(apply_err)?;
    Ok(())
}

/// Commit `txn` when `result` is `Ok`. Otherwise roll it back explicitly and return the error.
async fn finish(txn: DatabaseTransaction, result: Result<(), MemoryApplyError>) -> Result<(), MemoryApplyError> {
    match result {
        Ok(()) => txn.commit().await.map_err(apply_err),
        Err(e) => {
            let _ = txn.rollback().await;
            Err(e)
        }
    }
}

fn stmt(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Postgres, sql, values)
}

/// The values of `$1`, `$2` and `$3`.
fn scope_values(scope: &MemoryScope) -> Vec<Value> {
    vec![scope.org().into(), scope.agent().uuid().into(), scope.principal().uuid().into()]
}

/// The scope values, then `extra` from `$4` on.
fn with_scope(scope: &MemoryScope, extra: impl IntoIterator<Item = Value>) -> Vec<Value> {
    let mut values = scope_values(scope);
    values.extend(extra);
    values
}

/// The `LIKE` pattern of every path under `dir`. The directory ends in `/` (the root is `/`), and
/// `%`, `_` and `\` are escaped, so a directory name matches only itself.
fn like_prefix(dir: &MemoryDir) -> String {
    let base = if dir.is_root() { String::new() } else { dir.as_str().to_string() };
    let mut pattern = String::with_capacity(base.len() + 2);
    for c in base.chars() {
        if matches!(c, '%' | '_' | '\\') {
            pattern.push('\\');
        }
        pattern.push(c);
    }
    pattern.push_str("/%");
    pattern
}

/// The bigint of a version. A version that is not a bigint never matches a row.
fn parse_version(version: &MemoryVersion) -> Option<i64> {
    version.as_str().parse::<i64>().ok()
}

fn version_of(row: &QueryResult) -> Result<MemoryVersion, MemoryStoreError> {
    let version: i64 = row.try_get("", "version").map_err(store_err)?;
    Ok(MemoryVersion::new(version.to_string()))
}

fn usage_of(row: &QueryResult) -> Result<MemoryUsage, DbErr> {
    let files: i64 = row.try_get("", "files")?;
    let bytes: i64 = row.try_get("", "bytes")?;
    Ok(MemoryUsage {
        files: files.max(0) as u64,
        bytes: bytes.max(0) as u64,
    })
}

fn to_i64(n: u64) -> Result<i64, MemoryApplyError> {
    i64::try_from(n).map_err(|e| MemoryApplyError::Store(e.to_string()))
}

fn store_err(e: impl ToString) -> MemoryStoreError {
    MemoryStoreError::Store(e.to_string())
}

fn apply_err(e: impl ToString) -> MemoryApplyError {
    MemoryApplyError::Store(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_root_pattern_is_one_slash() {
        assert_eq!(like_prefix(&MemoryDir::root()), "/%");
    }

    #[test]
    fn a_dir_pattern_ends_in_a_slash_and_escapes_wildcards() {
        assert_eq!(like_prefix(&MemoryDir::parse("/a_b/c").expect("dir")), "/a\\_b/c/%");
    }

    #[test]
    fn a_non_numeric_version_does_not_parse() {
        assert_eq!(parse_version(&MemoryVersion::new("12")), Some(12));
        assert_eq!(parse_version(&MemoryVersion::new("x")), None);
        assert_eq!(parse_version(&MemoryVersion::new("99999999999999999999")), None);
    }
}
