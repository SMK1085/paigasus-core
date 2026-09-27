# SMA-699 Role Grant Scope Index Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The scope-filtered `RoleGrantQuery::find` uses an index for the org page's query (scope + kind, no principal), with a custom plan and with a generic plan, and its results do not change.

**Architecture:** A new migration `m0012` adds `ix_role_grant_scope_node_prn_principal_id` on `role_grant (scope_node_prn, principal_id, id)`. A small builder, `find_statement`, replaces the static `FIND_SQL`: it writes one `col = $n` predicate for each filter that is set, so no `IS NULL OR` predicate stays. One new Docker test binary proves the migration, the results (against the domain oracle) and the plans.

**Tech Stack:** Rust (edition 2024), sea-orm 2 / sea-orm-migration 2 (sqlx-postgres), Postgres 16 (testcontainers), cargo-nextest.

**Spec:** `docs/superpowers/specs/2026-09-26-sma-699-role-grant-scope-index-design.md` (approved). Read it before you start.

## Global Constraints

- Work only in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-699`, branch `feature/sma-699-role-grant-scope-index`. Check `git branch --show-current` before the first commit.
- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Prefix every shell with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Rust gates, from `rs/`: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo nextest run -p paigasus-iam ...`. Warnings are errors: no dead code, no unused imports.
- The Docker tests need Docker Desktop running. A test that prints `skipping` did NOT run; that is not a pass.
- Conventional commits with scope `rs`, and `(SMA-699)` at the end of the subject. End every commit message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Do NOT use `git commit --amend`, `git reset`, `git stash`, `--no-verify`, or `brew install`. One new commit per task.
- Do not change `tests/authz_role_grants.rs` or `tests/authz_forged_org_slot_escalation.rs`. They must pass unchanged.
- Index name, exactly: `ix_role_grant_scope_node_prn_principal_id`. Index columns, exactly: `(scope_node_prn, principal_id, id)`.
- The Root filter stays `g.scope_kind = 'root'` (spec E3). Do not bind a Root PRN.
- If a measured plan does not match what a step expects, STOP and report the plan text. Do not weaken an assertion to make it pass.

## Review Focus

1. Shape B (principal + scope + role + kind) has four predicates and the highest `$n`. A wrong number there breaks the org page's revoke check. Pinned in Task 1 (numbering over all 20 shapes) and Task 3 (every shape against Postgres).
2. A Root filter must return exactly the `scope_kind = 'root'` rows, with no PRN bound. Pinned in Task 1 and Task 3.
3. A team PRN with a forged org slot must match no row. Pinned in Task 3.
4. An INVALID leftover index with the same name must make m0012 fail, not pass silently. Pinned in Task 2.
5. `limit` 1 and an `offset` past the end must behave as before. Pinned in Task 3.

---

### Task 1: The statement builder

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_role_grants.rs:249-287` (the `FIND_SQL` constant and `RoleGrantQuery::find`), and add a `#[cfg(test)] mod tests` at the end of the file.

**Interfaces:**
- Produces: `#[doc(hidden)] pub fn find_statement(f: &RoleGrantFilter, limit: u64, offset: u64) -> sea_orm::Statement`, reachable as `paigasus_iam::adapters::persistence::pg_role_grants::find_statement`. Bound values are in this order: principal uuid (if set), node scope PRN (if a node scope is set), role key (if set), kind string (if set), then `limit`, then `offset`.

- [ ] **Step 1: Write the failing unit tests**

Append to the end of `pg_role_grants.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use paigasus_iam_core::{OrganizationId, PrincipalKind};
    use sea_orm::Value;

    fn principal() -> PrincipalId {
        PrincipalId::from_prn(Prn::build("iam", "", None, "principal", Uuid::from_u128(7)).unwrap())
    }

    fn org_scope() -> GrantScope {
        GrantScope::Node(TenancyNodeRef::Organization(OrganizationId::from_uuid(Uuid::from_u128(9))))
    }

    /// Every `$n` in `sql`, in order of appearance.
    fn placeholders(sql: &str) -> Vec<usize> {
        let bytes = sql.as_bytes();
        let mut out = Vec::new();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] == b'$' {
                let start = i + 1;
                let mut j = start;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                if j > start {
                    out.push(sql[start..j].parse().unwrap());
                }
                i = j;
            } else {
                i += 1;
            }
        }
        out
    }

    fn values(stmt: &Statement) -> Vec<Value> {
        stmt.values.clone().map(|v| v.0).unwrap_or_default()
    }

    /// Every filter shape `RoleGrantFilter::new` accepts: principal x scope {none, node, Root}
    /// x role x kind, minus the 4 shapes with no principal and no scope (D3). 20 shapes.
    fn all_shapes() -> Vec<RoleGrantFilter> {
        let mut out = Vec::new();
        for principal in [None, Some(principal())] {
            for scope in [None, Some(org_scope()), Some(GrantScope::Root)] {
                for role in [None, Some("gateway_user".to_string())] {
                    for kind in [None, Some(PrincipalKind::User)] {
                        if let Some(f) = RoleGrantFilter::new(principal.clone(), scope.clone(), role.clone(), kind) {
                            out.push(f);
                        }
                    }
                }
            }
        }
        assert_eq!(out.len(), 20);
        out
    }

    #[test]
    fn no_shape_keeps_an_is_null_or_predicate() {
        for f in all_shapes() {
            let stmt = find_statement(&f, 200, 0);
            assert!(!stmt.sql.contains("IS NULL"), "{f:?}: {}", stmt.sql);
            assert!(!stmt.sql.contains("IS FALSE"), "{f:?}: {}", stmt.sql);
        }
    }

    #[test]
    fn placeholders_are_numbered_one_to_k_with_no_gap() {
        for f in all_shapes() {
            let stmt = find_statement(&f, 200, 0);
            let mut seen = placeholders(&stmt.sql);
            seen.sort_unstable();
            seen.dedup();
            let k = values(&stmt).len();
            assert_eq!(seen, (1..=k).collect::<Vec<_>>(), "{f:?}: {}", stmt.sql);
        }
    }

    #[test]
    fn limit_and_offset_are_the_last_two_values_after_the_fixed_order() {
        for f in all_shapes() {
            let stmt = find_statement(&f, 17, 34);
            let vals = values(&stmt);
            let k = vals.len();
            assert_eq!(vals[k - 2], Value::from(17u64), "{f:?}");
            assert_eq!(vals[k - 1], Value::from(34u64), "{f:?}");
            assert!(stmt.sql.contains(&format!("ORDER BY g.principal_id, g.id\n LIMIT ${} OFFSET ${k}", k - 1)), "{f:?}: {}", stmt.sql);
        }
    }

    #[test]
    fn shape_b_binds_every_filter_in_the_fixed_order() {
        let f = RoleGrantFilter::new(Some(principal()), Some(org_scope()), Some("gateway_user".to_string()), Some(PrincipalKind::User)).unwrap();
        let stmt = find_statement(&f, 50, 0);
        assert!(stmt.sql.contains("g.principal_id = $1::uuid"), "{}", stmt.sql);
        assert!(stmt.sql.contains("g.scope_node_prn = $2::text"), "{}", stmt.sql);
        assert!(stmt.sql.contains("g.role_key = $3::text"), "{}", stmt.sql);
        assert!(stmt.sql.contains("pr.kind = $4::text"), "{}", stmt.sql);
        assert_eq!(
            values(&stmt),
            vec![
                Value::from(Uuid::from_u128(7)),
                Value::from(org_scope().canonical_prn()),
                Value::from("gateway_user".to_string()),
                Value::from("user".to_string()),
                Value::from(50u64),
                Value::from(0u64),
            ]
        );
    }

    #[test]
    fn shape_a_has_only_the_scope_and_kind_predicates() {
        let f = RoleGrantFilter::new(None, Some(org_scope()), None, Some(PrincipalKind::User)).unwrap();
        let stmt = find_statement(&f, 200, 800);
        assert!(stmt.sql.contains("g.scope_node_prn = $1::text"), "{}", stmt.sql);
        assert!(stmt.sql.contains("pr.kind = $2::text"), "{}", stmt.sql);
        assert!(!stmt.sql.contains("g.principal_id ="), "{}", stmt.sql);
        assert!(!stmt.sql.contains("g.role_key ="), "{}", stmt.sql);
        assert!(!stmt.sql.contains("scope_kind"), "{}", stmt.sql);
        assert_eq!(values(&stmt).len(), 4);
    }

    #[test]
    fn root_filters_on_scope_kind_and_binds_no_scope_value() {
        let f = RoleGrantFilter::new(None, Some(GrantScope::Root), None, None).unwrap();
        let stmt = find_statement(&f, 200, 0);
        assert!(stmt.sql.contains("g.scope_kind = 'root'"), "{}", stmt.sql);
        assert!(!stmt.sql.contains("g.scope_node_prn ="), "{}", stmt.sql);
        assert_eq!(values(&stmt), vec![Value::from(200u64), Value::from(0u64)]);
    }

    #[test]
    fn the_select_list_and_the_join_do_not_change() {
        let f = RoleGrantFilter::new(Some(principal()), None, None, None).unwrap();
        let stmt = find_statement(&f, 1, 0);
        assert!(stmt.sql.contains(
            "SELECT g.id, g.principal_id, g.role_key, g.scope_kind, g.scope_node_prn, g.scope_org_id,\n       g.scope_team_id, g.scope_project_id, g.linked_policy_id, g.created_at\n  FROM \"role_grant\" g JOIN \"principal\" pr ON pr.id = g.principal_id"
        ));
    }

    /// E7: no caller can build an empty filter (`RoleGrantFilter::new` refuses it), so this
    /// drives the private core directly. An empty filter must return no rows, never every grant.
    #[test]
    fn an_empty_predicate_list_matches_no_row() {
        let stmt = FindSql::default().render(200, 0);
        assert!(stmt.sql.contains(" WHERE FALSE\n"), "{}", stmt.sql);
        assert_eq!(values(&stmt), vec![Value::from(200u64), Value::from(0u64)]);
    }
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run (from `rs/`): `cargo nextest run -p paigasus-iam --lib pg_role_grants`
Expected: compile FAIL, `cannot find function find_statement` and `cannot find type FindSql`.

- [ ] **Step 3: Replace `FIND_SQL` with the builder**

In `pg_role_grants.rs`, replace the doc comment + `const FIND_SQL` (lines 249-266) with:

```rust
/// The fixed head of `RoleGrantQuery::find`'s statement (SMA-676). The inner join on
/// `principal` supplies the kind (every grant has a principal row, `fk_role_grant_principal`,
/// so the join drops nothing unless a kind filter asks it to). The selected columns are exactly
/// `role_grant`'s own, in `role_grant::Model`'s field order, so
/// `role_grant::Model::find_by_statement` maps the row directly.
const FIND_SELECT: &str = r#"
SELECT g.id, g.principal_id, g.role_key, g.scope_kind, g.scope_node_prn, g.scope_org_id,
       g.scope_team_id, g.scope_project_id, g.linked_policy_id, g.created_at
  FROM "role_grant" g JOIN "principal" pr ON pr.id = g.principal_id"#;

/// The predicates and bound values of one `find` statement (SMA-699). Each set filter adds one
/// plain `col = $n` predicate. There is no `($n IS NULL OR col = $n)` form: under a generic plan
/// (sqlx caches `find_by_statement` as a named prepared statement) that form stops the planner
/// from using any index.
#[derive(Default)]
struct FindSql {
    predicates: Vec<String>,
    values: Vec<sea_orm::Value>,
}

impl FindSql {
    /// Adds `<column> = $n::<cast>` and binds `value` as `$n`.
    fn bind(&mut self, column: &str, cast: &str, value: sea_orm::Value) {
        self.values.push(value);
        self.predicates.push(format!("{column} = ${}::{cast}", self.values.len()));
    }

    /// Adds a predicate that binds no value.
    fn literal(&mut self, predicate: &str) {
        self.predicates.push(predicate.to_owned());
    }

    /// The full statement. `limit` and `offset` take the last two numbers (D6). No predicates
    /// gives `WHERE FALSE` (E7): D3 guarantees a principal or a scope, so no caller reaches it,
    /// and if one does it must list nothing, not every tenant's grants.
    fn render(mut self, limit: u64, offset: u64) -> Statement {
        let predicates = if self.predicates.is_empty() { "FALSE".to_owned() } else { self.predicates.join("\n   AND ") };
        self.values.push(limit.into());
        let limit_n = self.values.len();
        self.values.push(offset.into());
        let offset_n = self.values.len();
        let sql = format!("{FIND_SELECT}\n WHERE {predicates}\n ORDER BY g.principal_id, g.id\n LIMIT ${limit_n} OFFSET ${offset_n}");
        Statement::from_sql_and_values(DbBackend::Postgres, sql, self.values)
    }
}

/// `RoleGrantQuery::find`'s statement (SMA-676, SMA-699). One predicate per set filter, in a
/// fixed order: principal, scope, role, kind. D5: a node scope matches its stored canonical PRN
/// exactly (`ix_role_grant_scope_node_prn_principal_id`, m0012, serves it); the Root sentinel
/// matches `scope_kind = 'root'` and binds nothing (spec E3: no stored-PRN invariant for Root).
///
/// `pub` but `#[doc(hidden)]`, so `tests/authz_role_grant_query_plan.rs` can `EXPLAIN` the exact
/// statement, not a copy that can drift (the SMA-469 precedent, `published_sweep_sql`).
#[doc(hidden)]
#[must_use]
pub fn find_statement(f: &RoleGrantFilter, limit: u64, offset: u64) -> Statement {
    let mut q = FindSql::default();
    if let Some(p) = f.principal() {
        q.bind("g.principal_id", "uuid", p.uuid().into());
    }
    match f.scope() {
        None => {}
        Some(GrantScope::Root) => q.literal("g.scope_kind = 'root'"),
        Some(scope @ GrantScope::Node(_)) => q.bind("g.scope_node_prn", "text", scope.canonical_prn().into()),
    }
    if let Some(role_key) = f.role_key() {
        q.bind("g.role_key", "text", role_key.to_owned().into());
    }
    if let Some(kind) = f.principal_kind() {
        q.bind("pr.kind", "text", kind.as_str().to_owned().into());
    }
    q.render(limit, offset)
}
```

Then replace the body of `impl RoleGrantQuery for PgRoleGrantStore { async fn find ... }` with:

```rust
    async fn find(&self, f: &RoleGrantFilter, limit: u64, offset: u64) -> Result<Vec<RoleGrant>, AuthzError> {
        let models = role_grant::Model::find_by_statement(find_statement(f, limit, offset)).all(&self.db).await.map_err(map_err)?;
        models.into_iter().map(model_to_grant).collect()
    }
```

Note (a deliberate small deviation from spec E7): there is no `debug_assert!` on the empty case. A `debug_assert!` would panic in the debug-build unit test above, so `WHERE FALSE` could not be tested. The unit test pins `WHERE FALSE` instead.

If clippy reports an unused import after this change (for example `TenancyNodeRef` or `Prn` outside the tests), remove only that import.

- [ ] **Step 4: Run the unit tests and see them pass**

Run: `cargo nextest run -p paigasus-iam --lib pg_role_grants`
Expected: 8 tests PASS.

- [ ] **Step 5: Run the existing Docker suites (behaviour unchanged)**

Run: `cargo nextest run -p paigasus-iam --no-fail-fast -E 'binary(authz_role_grants) | binary(authz_forged_org_slot_escalation) | binary(grpc_authz) | binary(http_authz)'`
Expected: all PASS, and no test prints `skipping`.

- [ ] **Step 6: fmt + clippy, then commit**

```bash
cd rs && cargo fmt && cargo clippy -p paigasus-iam --all-targets -- -D warnings
cd .. && git add rs/crates/services/paigasus-iam/src/adapters/persistence/pg_role_grants.rs
git commit -m "feat(rs): build the role grant query from the set filters (SMA-699)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Migration m0012 and its round-trip test

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/migration/m0012_role_grant_scope_index.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/persistence/migration/mod.rs` (add `mod m0012_role_grant_scope_index;` after m0011, and `Box::new(m0012_role_grant_scope_index::Migration),` after the m0011 entry)
- Create: `rs/crates/services/paigasus-iam/tests/authz_role_grant_query_plan.rs`

**Interfaces:**
- Produces: the index `ix_role_grant_scope_node_prn_principal_id`; m0012 is the 12th migration. The test file and its helper `index_validity(db) -> Option<bool>` are extended by Tasks 3 and 4.

- [ ] **Step 1: Write the failing tests**

Create `tests/authz_role_grant_query_plan.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! SMA-699: the scope-filtered `RoleGrantQuery::find` and its index
//! (`ix_role_grant_scope_node_prn_principal_id`, m0012). Three parts: m0012's migration round
//! trip; the results of every filter shape against the domain oracle
//! `RoleGrantFilter::matches`; and the plans of the three real query shapes (spec "The real
//! query shapes"), with a custom and a generic plan.

mod support;

use paigasus_iam::adapters::persistence::Migrator;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use sea_orm_migration::MigratorTrait;

const INDEX: &str = "ix_role_grant_scope_node_prn_principal_id";

/// m0012 is the 12th migration. Pinning the `up` count (not counting `down` steps from the tip)
/// keeps `down(Some(1))` equal to m0012's `down` when later migrations land (the
/// `audit_log_partition_pg.rs` precedent).
const M0012: u32 = 12;

/// `None`: no such index. `Some(valid)`: the index exists, with `pg_index.indisvalid`.
async fn index_validity(db: &DatabaseConnection) -> Option<bool> {
    let row = db
        .query_one_raw(Statement::from_string(
            DbBackend::Postgres,
            format!("SELECT i.indisvalid FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid WHERE c.relname = '{INDEX}'"),
        ))
        .await
        .unwrap()?;
    Some(row.try_get::<bool>("", "indisvalid").unwrap())
}

#[tokio::test]
async fn m0012_up_down_up_keeps_a_valid_index() {
    let Some((_pg, db)) = support::start_raw_postgres().await else { return };
    Migrator::up(&db, Some(M0012)).await.expect("m0001..m0012 must apply");
    assert_eq!(index_validity(&db).await, Some(true), "after up");

    Migrator::down(&db, Some(1)).await.expect("m0012 down must succeed");
    assert_eq!(index_validity(&db).await, None, "after down");

    Migrator::up(&db, Some(1)).await.expect("m0012 must apply again");
    assert_eq!(index_validity(&db).await, Some(true), "after the second up");
}

/// E5: `CREATE INDEX IF NOT EXISTS` skips an INVALID index with the same name (what a failed
/// out-of-band `CREATE INDEX CONCURRENTLY` leaves), and the planner ignores an INVALID index.
/// m0012 must fail with an operator message, not pass.
#[tokio::test]
async fn m0012_refuses_an_invalid_leftover_index() {
    let Some((_pg, db)) = support::start_raw_postgres().await else { return };
    Migrator::up(&db, Some(M0012 - 1)).await.expect("m0001..m0011 must apply");
    db.execute_unprepared(&format!(r#"CREATE INDEX {INDEX} ON "role_grant" (scope_node_prn, principal_id, id);"#)).await.unwrap();
    db.execute_unprepared(&format!("UPDATE pg_index SET indisvalid = false WHERE indexrelid = '{INDEX}'::regclass;")).await.unwrap();
    assert_eq!(index_validity(&db).await, Some(false), "sanity: the leftover index is INVALID");

    let err = Migrator::up(&db, Some(1)).await.expect_err("m0012 must refuse an INVALID index");
    let msg = err.to_string();
    assert!(msg.contains(INDEX) && msg.contains("INVALID"), "the error must name the index and the cause: {msg}");
}
```

- [ ] **Step 2: Run the tests and see them fail**

Run: `cargo nextest run -p paigasus-iam -E 'binary(authz_role_grant_query_plan)'`
Expected: FAIL. `m0012_up_down_up...` fails because `Migrator::up(Some(12))` applies only 11 migrations and the index is absent (`left: None, right: Some(true)`). `m0012_refuses...` fails at `expect_err` (there is no 12th migration, so `up` succeeds).

- [ ] **Step 3: Write the migration**

Create `m0012_role_grant_scope_index.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! m0012 — an index for the scope-filtered role grant query (SMA-699).
//!
//! `RoleGrantQuery::find` with a scope and no principal (the gateway-console org page) had no
//! index to use: no index led with `scope_node_prn`, so each call could scan the full
//! `role_grant` table. `(scope_node_prn, principal_id, id)` finds one node's rows and returns
//! them in the query's `ORDER BY g.principal_id, g.id` order, so a page stops after
//! `offset + limit` rows with no sort.
//!
//! **No `CONCURRENTLY`.** Production runs every pending migration in one outer transaction
//! (`migrate_under_lock`), and `CREATE INDEX CONCURRENTLY` cannot run in a transaction. The
//! build holds a SHARE lock on `role_grant` until that transaction commits: grants, revokes, org
//! creation and cascading deletes wait, reads continue. `SET LOCAL lock_timeout` bounds the wait,
//! as in m0008-m0011. For a large table, an operator builds the index `CONCURRENTLY` before the
//! deploy; `IF NOT EXISTS` then makes this migration only check it.
//!
//! **An INVALID index fails the migration.** A failed `CREATE INDEX CONCURRENTLY` leaves an
//! INVALID index with this name. `IF NOT EXISTS` would skip the create, and the planner ignores
//! an INVALID index, so the query would scan again with no signal.

use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{DbBackend, Statement};

const INDEX: &str = "ix_role_grant_scope_node_prn_principal_id";

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        conn.execute_unprepared(&format!(r#"CREATE INDEX IF NOT EXISTS {INDEX} ON "role_grant" (scope_node_prn, principal_id, id);"#)).await?;
        let row = conn
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                format!("SELECT i.indisvalid FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid WHERE c.relname = '{INDEX}'"),
            ))
            .await?;
        let valid = match row {
            Some(row) => row.try_get::<bool>("", "indisvalid")?,
            None => false,
        };
        if !valid {
            return Err(DbErr::Migration(format!(
                "m0012: index {INDEX} is INVALID or missing (a failed CREATE INDEX CONCURRENTLY leaves an INVALID index). Run `DROP INDEX {INDEX};`, then run the migration again."
            )));
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        conn.execute_unprepared("SET LOCAL lock_timeout = '5s';").await?;
        conn.execute_unprepared(&format!("DROP INDEX IF EXISTS {INDEX};")).await?;
        Ok(())
    }
}
```

If `sea_orm_migration::sea_orm::{DbBackend, Statement}` does not resolve, look at how `m0008_partition_audit_log.rs:184-195` imports `Statement` and use the same path. Register the module in `migration/mod.rs` as described under **Files**.

- [ ] **Step 4: Run the tests and see them pass**

Run: `cargo nextest run -p paigasus-iam -E 'binary(authz_role_grant_query_plan)'`
Expected: 2 tests PASS, and no `skipping` line.

- [ ] **Step 5: Check that no other test pins the migration count**

Run: `cargo nextest run -p paigasus-iam --no-fail-fast -E 'binary(migration_lock_pg) | binary(audit_log_partition_pg) | binary(boot_install_pg) | binary(authz_role_grants)'`
Expected: all PASS. (If a binary name does not exist, nextest reports it; drop that name and continue.)

- [ ] **Step 6: fmt + clippy, then commit**

```bash
cd rs && cargo fmt && cargo clippy -p paigasus-iam --all-targets -- -D warnings
cd .. && git add rs/crates/services/paigasus-iam/src/adapters/persistence/migration/ rs/crates/services/paigasus-iam/tests/authz_role_grant_query_plan.rs
git commit -m "feat(rs): index role grants by scope node, principal and id (SMA-699)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Equivalence with the domain oracle

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/authz_role_grant_query_plan.rs` (append; extend the `use` lines)

**Interfaces:**
- Consumes: `PgRoleGrantStore::new(db, Generations::memory())`, `RoleGrantStore::grant`, `RoleGrantQuery::find`, `RoleGrantFilter::{new, matches}`.

- [ ] **Step 1: Write the test**

Extend the `use` block at the top of the file to:

```rust
use chrono::{SubsecRound, Utc};
use paigasus_iam::adapters::authz::Generations;
use paigasus_iam::adapters::persistence::entities::{policy, role};
use paigasus_iam::adapters::persistence::{Migrator, PgRoleGrantStore};
use paigasus_iam_core::{GrantScope, OrganizationId, PrincipalId, PrincipalKind, ProjectId, RoleGrant, RoleGrantFilter, RoleGrantQuery, RoleGrantStore, TeamId, TenancyNodeRef};
use paigasus_kernel::Prn;
use sea_orm::{ActiveModelTrait, ActiveValue::NotSet, ConnectionTrait, DatabaseConnection, DbBackend, Set, Statement};
use sea_orm_migration::MigratorTrait;
use std::collections::HashMap;
use uuid::Uuid;
```

Append:

```rust
fn pid(u: Uuid) -> PrincipalId {
    PrincipalId::from_prn(Prn::build("iam", "", None, "principal", u).unwrap())
}

async fn exec(db: &DatabaseConnection, sql: String) {
    db.execute_raw(Statement::from_string(DbBackend::Postgres, sql)).await.unwrap();
}

async fn seed_principal(db: &DatabaseConnection, u: Uuid, kind: PrincipalKind) {
    exec(
        db,
        format!(
            r#"INSERT INTO "principal" (id, prn, kind, status, created_at, updated_at)
               VALUES ('{u}', 'prn:pgs:iam:::principal/{u}', '{}', 'active', now(), now())"#,
            kind.as_str()
        ),
    )
    .await;
}

async fn seed_org(db: &DatabaseConnection, org: &OrganizationId, slug: &str) {
    exec(
        db,
        format!(
            r#"INSERT INTO "organization" (id, prn, slug, name, status, created_at, updated_at)
               VALUES ('{}', '{}', '{slug}', '{slug}', 'active', now(), now())"#,
            org.uuid(),
            org.canonical()
        ),
    )
    .await;
}

/// A `policy` template + a `role` row: the `fk_role_grant_role` target (copied from
/// `authz_role_grants.rs::seed_role`; each test binary compiles its own helpers).
async fn seed_role(db: &DatabaseConnection, role_key: &str) {
    let now = Utc::now().trunc_subsecs(6);
    let template_id = format!("{role_key}_template");
    policy::ActiveModel {
        policy_id: Set(template_id.clone()),
        kind: Set("template".to_string()),
        source: Set("permit(principal == ?principal, action, resource in ?resource);".to_string()),
        description: Set(None),
        system: Set(false),
        created_at: Set(now),
        updated_at: Set(now),
        content_fingerprint: NotSet,
        starter_revision: NotSet,
    }
    .insert(db)
    .await
    .unwrap();
    role::ActiveModel {
        key: Set(role_key.to_string()),
        template_id: Set(template_id),
        scope_kinds: Set(r#"["root","organization","team","project"]"#.to_string()),
        description: Set(None),
        system: Set(false),
        created_at: Set(now),
    }
    .insert(db)
    .await
    .unwrap();
}

/// Every filter shape `RoleGrantFilter::new` accepts, over values from a fixed grant set, gives
/// the rows the domain oracle `RoleGrantFilter::matches` selects, in `(principal_id, id)` order.
/// The candidate scopes include Root, a project, and a team PRN with a forged org slot (D5: it
/// matches no row). Paging: `limit` 1 gives the first oracle row; an `offset` past the end gives
/// no row.
#[tokio::test]
async fn find_matches_the_domain_oracle_for_every_filter_shape() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let now = Utc::now().trunc_subsecs(6);

    let (u1, u2, s1) = (Uuid::from_u128(0x11), Uuid::from_u128(0x12), Uuid::from_u128(0x13));
    let kinds: HashMap<Uuid, PrincipalKind> = HashMap::from([(u1, PrincipalKind::User), (u2, PrincipalKind::User), (s1, PrincipalKind::ServiceAccount)]);
    for (u, k) in &kinds {
        seed_principal(&db, *u, *k).await;
    }
    let (o1, o2) = (OrganizationId::from_uuid(Uuid::from_u128(0x21)), OrganizationId::from_uuid(Uuid::from_u128(0x22)));
    seed_org(&db, &o1, "one").await;
    seed_org(&db, &o2, "two").await;
    let team = TeamId::from_parts(o1.uuid(), Uuid::from_u128(0x31));
    exec(
        &db,
        format!(
            r#"INSERT INTO "team" (id, org_id, prn, slug, name, status, created_at, updated_at)
               VALUES ('{}', '{}', '{}', 'core', 'Core', 'active', now(), now())"#,
            team.uuid(),
            o1.uuid(),
            team.canonical()
        ),
    )
    .await;
    let project = ProjectId::from_parts(o1.uuid(), Uuid::from_u128(0x41));
    exec(
        &db,
        format!(
            r#"INSERT INTO "project" (id, team_id, org_id, prn, slug, name, status, created_at, updated_at)
               VALUES ('{}', '{}', '{}', '{}', 'svc', 'Svc', 'active', now(), now())"#,
            project.uuid(),
            team.uuid(),
            o1.uuid(),
            project.canonical()
        ),
    )
    .await;
    seed_role(&db, "gateway_user").await;
    seed_role(&db, "org_admin").await;

    let at_o1 = GrantScope::Node(TenancyNodeRef::Organization(o1.clone()));
    let at_o2 = GrantScope::Node(TenancyNodeRef::Organization(o2.clone()));
    let at_team = GrantScope::Node(TenancyNodeRef::Team(team.clone()));
    let at_project = GrantScope::Node(TenancyNodeRef::Project(project.clone()));
    let forged_team = GrantScope::Node(TenancyNodeRef::Team(TeamId::from_parts(o2.uuid(), team.uuid())));

    let store = PgRoleGrantStore::new(db.clone(), Generations::memory());
    let mut grants: Vec<RoleGrant> = Vec::new();
    for (n, (who, role_key, scope)) in [
        (u2, "gateway_user", at_o1.clone()),
        (u1, "gateway_user", at_o1.clone()),
        (s1, "gateway_user", at_o1.clone()),
        (u1, "org_admin", at_o1.clone()),
        (u1, "gateway_user", at_o2.clone()),
        (u1, "gateway_user", at_team.clone()),
        (u2, "org_admin", at_project.clone()),
        (u1, "org_admin", GrantScope::Root),
        (s1, "gateway_user", GrantScope::Root),
    ]
    .into_iter()
    .enumerate()
    {
        let id = Uuid::from_u128(0x1000 + n as u128);
        let g = RoleGrant {
            id,
            principal: pid(who),
            role_key: role_key.to_string(),
            scope,
            linked_policy_id: format!("grant:{id}"),
            created_at: now,
        };
        store.grant(&g).await.unwrap();
        grants.push(g);
    }

    let mut checked = 0;
    for principal in [None, Some(pid(u1)), Some(pid(s1))] {
        for scope in [None, Some(at_o1.clone()), Some(at_team.clone()), Some(at_project.clone()), Some(GrantScope::Root), Some(forged_team.clone())] {
            for role_key in [None, Some("gateway_user".to_string())] {
                for kind in [None, Some(PrincipalKind::User), Some(PrincipalKind::ServiceAccount)] {
                    let Some(f) = RoleGrantFilter::new(principal.clone(), scope.clone(), role_key.clone(), kind) else { continue };
                    let mut expected: Vec<RoleGrant> = grants.iter().filter(|g| f.matches(g, kinds.get(&g.principal.uuid()).copied())).cloned().collect();
                    expected.sort_by_key(|g| (g.principal.uuid(), g.id));

                    assert_eq!(RoleGrantQuery::find(&store, &f, 200, 0).await.unwrap(), expected, "{f:?}");
                    assert_eq!(RoleGrantQuery::find(&store, &f, 1, 0).await.unwrap(), expected.iter().take(1).cloned().collect::<Vec<_>>(), "limit 1: {f:?}");
                    assert!(RoleGrantQuery::find(&store, &f, 200, expected.len() as u64 + 1).await.unwrap().is_empty(), "offset past the end: {f:?}");
                    checked += 1;
                }
            }
        }
    }
    assert_eq!(checked, 102, "every accepted shape: 3 x 6 x 2 x 3, minus the 6 with no principal and no scope");

    let forged = RoleGrantFilter::new(None, Some(forged_team), None, None).unwrap();
    assert!(RoleGrantQuery::find(&store, &forged, 200, 0).await.unwrap().is_empty(), "D5: a forged org slot matches no row");
}
```

Notes for the implementer:
- `ProjectId::from_parts(org, id)` exists (`paigasus-iam-core/src/tenancy.rs:123`). If `RoleGrant` does not derive `Clone`/`PartialEq`, check `authz_role_grants.rs`: it compares `Vec<RoleGrant>` with `assert_eq!`, so both exist.
- `RoleGrant.created_at` must round-trip: `trunc_subsecs(6)` matches Postgres microseconds.
- If a candidate import is unused after this task (for example `Migrator` is used by Task 2 already), keep only what the file uses; clippy denies unused imports.

- [ ] **Step 2: Run the test**

Run: `cargo nextest run -p paigasus-iam -E 'binary(authz_role_grant_query_plan)'`
Expected: 3 tests PASS. This test pins behaviour, so it passes on Task 1's code. To prove that it can fail, temporarily change `"g.scope_kind = 'root'"` in `find_statement` to `"g.scope_kind = 'organization'"`, run again, and see it FAIL on a Root shape. Then undo that change with the Edit tool (do not use `git checkout`), and run again to see PASS.

- [ ] **Step 3: fmt + clippy, then commit**

```bash
cd rs && cargo fmt && cargo clippy -p paigasus-iam --all-targets -- -D warnings
cd .. && git add rs/crates/services/paigasus-iam/tests/authz_role_grant_query_plan.rs
git commit -m "test(rs): compare every role grant filter shape with the domain oracle (SMA-699)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The plan test, its controls, and the retry override

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/authz_role_grant_query_plan.rs` (append)
- Modify: `rs/.config/nextest.toml` (a new override block ABOVE the `package(=paigasus-iam) and kind(test)` block)

**Interfaces:**
- Consumes: `paigasus_iam::adapters::persistence::pg_role_grants::find_statement` (Task 1), the helpers `exec`, `seed_role` (Task 3), `INDEX` (Task 2).

- [ ] **Step 1: Write the tests**

Add to the `use` block: `use paigasus_iam::adapters::persistence::pg_role_grants::find_statement;` and `use sea_orm::TransactionTrait;`.

Append:

```rust
/// Bulk seed for the plan tests (spec "Tests / Seed"). 10,007 principals (every 5th a service
/// account), 2,000 orgs, 25,000 org grants over 4 roles. 10,007 is prime, so
/// (principal, role, org) never repeats within 25,000 rows and `uq_role_grant_principal_role_scope`
/// holds. The target org (`md5('o1')`) gets 12 grants (n = 2000, 4000, ..., 24000): under 0.1% of the table. Every table stays
/// under 30,000 rows, so `ANALYZE` reads it whole and the statistics are the same on every run.
/// Autovacuum is off, so no background `ANALYZE` changes them during the test.
async fn seed_bulk(db: &DatabaseConnection) -> OrganizationId {
    for r in ["r0", "r1", "r2", "r3"] {
        seed_role(db, r).await;
    }
    exec(
        db,
        r#"INSERT INTO "principal" (id, prn, kind, status, created_at, updated_at)
           SELECT md5('p' || n)::uuid, 'prn:pgs:iam:::principal/' || md5('p' || n)::uuid,
                  CASE WHEN n % 5 = 0 THEN 'service_account' ELSE 'user' END, 'active', now(), now()
             FROM generate_series(1, 10007) n"#
            .to_string(),
    )
    .await;
    exec(
        db,
        r#"INSERT INTO "organization" (id, prn, slug, name, status, created_at, updated_at)
           SELECT md5('o' || n)::uuid, 'prn:pgs:iam:::organization/' || md5('o' || n)::uuid,
                  'org-' || n, 'Org ' || n, 'active', now(), now()
             FROM generate_series(1, 2000) n"#
            .to_string(),
    )
    .await;
    exec(
        db,
        r#"INSERT INTO "role_grant" (id, principal_id, role_key, scope_kind, scope_node_prn, scope_org_id,
                                     scope_team_id, scope_project_id, linked_policy_id, created_at)
           SELECT md5('g' || n)::uuid, md5('p' || (1 + n % 10007))::uuid, 'r' || (n % 4), 'organization',
                  'prn:pgs:iam:::organization/' || md5('o' || (1 + n % 2000))::uuid, md5('o' || (1 + n % 2000))::uuid,
                  NULL, NULL, 'grant:' || md5('g' || n)::uuid, now()
             FROM generate_series(1, 25000) n"#
            .to_string(),
    )
    .await;
    exec(db, r#"ALTER TABLE "role_grant" SET (autovacuum_enabled = false)"#.to_string()).await;
    exec(db, r#"ALTER TABLE "principal" SET (autovacuum_enabled = false)"#.to_string()).await;
    exec(db, r#"ANALYZE "role_grant""#.to_string()).await;
    exec(db, r#"ANALYZE "principal""#.to_string()).await;

    let target = db
        .query_one_raw(Statement::from_string(DbBackend::Postgres, "SELECT md5('o1')::uuid AS id".to_string()))
        .await
        .unwrap()
        .unwrap()
        .try_get::<Uuid>("", "id")
        .unwrap();
    let org = OrganizationId::from_uuid(target);
    // Sanity: the seeded PRN text is the canonical form the service writes and queries with.
    let stored: String = db
        .query_one_raw(Statement::from_string(DbBackend::Postgres, format!(r#"SELECT prn FROM "organization" WHERE id = '{target}'"#)))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "prn")
        .unwrap();
    assert_eq!(stored, org.canonical(), "the seed must write canonical org PRNs");
    org
}

/// One query shape for `EXPLAIN EXECUTE`: the filter, and the Postgres type and SQL literal of
/// every bound value in `find_statement` order (principal, scope, role, kind, limit, offset).
struct Shape {
    name: &'static str,
    filter: RoleGrantFilter,
    limit: u64,
    offset: u64,
    types: Vec<&'static str>,
    literals: Vec<String>,
}

/// The custom plan and the generic plan of `sql`, through a named prepared statement:
/// `plan_cache_mode` controls only the plan cache, which a plain `EXPLAIN` with bound values
/// never goes through. `drop_index` drops the new index first (control 1); the transaction
/// rolls back, so the index comes back. Returns `(custom, generic)`.
async fn plans(db: &DatabaseConnection, sql: &str, types: &[&str], literals: &[String], drop_index: bool) -> (String, String) {
    let txn = db.begin().await.unwrap();
    if drop_index {
        txn.execute_unprepared(&format!("DROP INDEX {INDEX};")).await.unwrap();
    }
    txn.execute_unprepared(&format!("PREPARE sma699_q({}) AS {sql}", types.join(", "))).await.unwrap();
    let mut out = Vec::new();
    for mode in ["force_custom_plan", "force_generic_plan"] {
        txn.execute_unprepared(&format!("SET LOCAL plan_cache_mode = {mode};")).await.unwrap();
        let rows = txn
            .query_all_raw(Statement::from_string(DbBackend::Postgres, format!("EXPLAIN EXECUTE sma699_q({})", literals.join(", "))))
            .await
            .unwrap();
        out.push(rows.iter().map(|r| r.try_get::<String>("", "QUERY PLAN").unwrap()).collect::<Vec<_>>().join("\n"));
    }
    txn.execute_unprepared("DEALLOCATE sma699_q;").await.unwrap();
    txn.rollback().await.unwrap();
    let generic = out.pop().unwrap();
    let custom = out.pop().unwrap();
    (custom, generic)
}

fn shapes(org: &OrganizationId) -> Vec<Shape> {
    let scope = GrantScope::Node(TenancyNodeRef::Organization(org.clone()));
    let prn = format!("'{}'", org.canonical());
    // A principal with a grant at the target org: n = 2000 gives org 1 + 2000 % 2000 = 1 and
    // principal 1 + 2000 % 10007 = 2001, role r0 (2000 % 4 = 0), kind user (2001 % 5 != 0).
    let principal_sql = "md5('p2001')::uuid";
    vec![
        Shape {
            name: "A offset 0 (org page)",
            filter: RoleGrantFilter::new(None, Some(scope.clone()), None, Some(PrincipalKind::User)).unwrap(),
            limit: 200,
            offset: 0,
            types: vec!["text", "text", "bigint", "bigint"],
            literals: vec![prn.clone(), "'user'".into(), "200".into(), "0".into()],
        },
        Shape {
            name: "A offset 800 (org page, page 5)",
            filter: RoleGrantFilter::new(None, Some(scope.clone()), None, Some(PrincipalKind::User)).unwrap(),
            limit: 200,
            offset: 800,
            types: vec!["text", "text", "bigint", "bigint"],
            literals: vec![prn.clone(), "'user'".into(), "200".into(), "800".into()],
        },
        Shape {
            name: "B (revoke check)",
            filter: RoleGrantFilter::new(Some(pid(Uuid::nil())), Some(scope.clone()), Some("r0".into()), Some(PrincipalKind::User)).unwrap(),
            limit: 50,
            offset: 0,
            types: vec!["uuid", "text", "text", "text", "bigint", "bigint"],
            literals: vec![principal_sql.into(), prn.clone(), "'r0'".into(), "'user'".into(), "50".into(), "0".into()],
        },
        Shape {
            name: "C (grant pre-check)",
            filter: RoleGrantFilter::new(Some(pid(Uuid::nil())), Some(scope), Some("r0".into()), None).unwrap(),
            limit: 1,
            offset: 0,
            types: vec!["uuid", "text", "text", "bigint", "bigint"],
            literals: vec![principal_sql.into(), prn, "'r0'".into(), "1".into(), "0".into()],
        },
    ]
}

/// SMA-699 acceptance. Shape A uses the new index with a custom and a generic plan; shapes B
/// and C never scan `role_grant` in full. The test pins the index name for shape A (the outbox
/// precedent does not): the seed rules fix the table shape, and the index is the object under
/// test. The prepared statement's text is `find_statement`'s own SQL; only the principal value
/// comes from the `EXECUTE` literals (the filter's own principal is a placeholder with the same
/// shape). Plans print with `--no-capture` for the PR.
#[tokio::test]
async fn the_real_query_shapes_use_an_index_in_custom_and_generic_plans() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let org = seed_bulk(&db).await;

    let store = PgRoleGrantStore::new(db.clone(), Generations::memory());
    let org_page = RoleGrantFilter::new(None, Some(GrantScope::Node(TenancyNodeRef::Organization(org.clone()))), None, Some(PrincipalKind::User)).unwrap();
    let rows = RoleGrantQuery::find(&store, &org_page, 200, 0).await.unwrap();
    assert!(!rows.is_empty() && rows.len() <= 12, "sanity: the target org holds at most 12 grants, got {}", rows.len());

    for s in shapes(&org) {
        let stmt = find_statement(&s.filter, s.limit, s.offset);
        let (custom, generic) = plans(&db, &stmt.sql, &s.types, &s.literals, false).await;
        eprintln!("=== {} / custom plan ===\n{custom}\n=== {} / generic plan ===\n{generic}\n", s.name, s.name);

        assert!(!custom.contains("$1"), "{}: the custom plan must bind the values:\n{custom}", s.name);
        assert!(generic.contains("$1"), "{}: the generic plan must show parameters:\n{generic}", s.name);
        for (mode, plan) in [("custom", &custom), ("generic", &generic)] {
            assert!(!plan.contains("Seq Scan on role_grant"), "{} / {mode}: no full scan of role_grant:\n{plan}", s.name);
            if s.name.starts_with('A') {
                assert!(plan.contains(INDEX), "{} / {mode}: shape A must use {INDEX}:\n{plan}", s.name);
            }
        }
    }
}

/// Control 1: without the new index, shape A's generic plan scans `role_grant` in full. This
/// proves that the seed makes the index matter, so the positive assertion above can fail.
#[tokio::test]
async fn control_without_the_index_the_org_page_query_scans_role_grant() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let org = seed_bulk(&db).await;
    let a = shapes(&org).into_iter().next().unwrap();
    let stmt = find_statement(&a.filter, a.limit, a.offset);
    let (custom, generic) = plans(&db, &stmt.sql, &a.types, &a.literals, true).await;
    eprintln!("=== control 1 / custom ===\n{custom}\n=== control 1 / generic ===\n{generic}");
    assert!(!generic.contains(INDEX), "the index was dropped:\n{generic}");
    assert!(generic.contains("Seq Scan on role_grant"), "with no index, the generic plan must scan role_grant:\n{generic}");
}

/// The SMA-676 statement, kept as a fixture for control 2 only.
const OLD_IS_NULL_OR_SQL: &str = r#"
SELECT g.id, g.principal_id, g.role_key, g.scope_kind, g.scope_node_prn, g.scope_org_id,
       g.scope_team_id, g.scope_project_id, g.linked_policy_id, g.created_at
  FROM "role_grant" g JOIN "principal" pr ON pr.id = g.principal_id
 WHERE ($1::uuid IS NULL OR g.principal_id = $1)
   AND ($2::text IS NULL OR g.scope_node_prn = $2)
   AND ($3::boolean IS FALSE OR g.scope_kind = 'root')
   AND ($4::text IS NULL OR g.role_key = $4)
   AND ($5::text IS NULL OR pr.kind = $5)
 ORDER BY g.principal_id, g.id
 LIMIT $6 OFFSET $7"#;

/// Control 2: with the index in place, the old `IS NULL OR` statement cannot use it in a
/// generic plan. This proves that the builder (spec E2) is needed, not only the index.
#[tokio::test]
async fn control_the_old_is_null_or_statement_cannot_use_the_index_in_a_generic_plan() {
    let Some((_pg, db)) = support::start_migrated_postgres().await else { return };
    let org = seed_bulk(&db).await;
    let types = ["uuid", "text", "boolean", "text", "text", "bigint", "bigint"];
    let literals = ["NULL".to_string(), format!("'{}'", org.canonical()), "false".into(), "NULL".into(), "'user'".into(), "200".into(), "0".into()];
    let (custom, generic) = plans(&db, OLD_IS_NULL_OR_SQL, &types, &literals, false).await;
    eprintln!("=== control 2 / custom ===\n{custom}\n=== control 2 / generic ===\n{generic}");
    assert!(generic.contains("$2"), "sanity: a generic plan:\n{generic}");
    assert!(!generic.contains(INDEX), "the IS NULL OR form must not reach the index in a generic plan:\n{generic}");
}
```

- [ ] **Step 2: Add the retry override**

In `rs/.config/nextest.toml`, directly ABOVE the `[[profile.default.overrides]]` block whose filter is `'package(=paigasus-iam) and kind(test)'`, insert:

```toml
[[profile.default.overrides]]
# SMA-699: the query-plan suite asserts on planner choices. A plan that changes from run to run
# must red at once, not show as FLAKY after a retry hides it. Scoped to the whole binary, for
# the same reason as keycloak_e2e above. Does NOT set `test-group`; it inherits
# `docker-containers` from the block below.
filter = 'package(=paigasus-iam) and binary(authz_role_grant_query_plan)'
retries = 0
```

Then check that a CI gate does not pin this file's content: `grep -rn "nextest.toml" ci/ .github/ | head`. If a gate reads it, run that gate (`<bash> ci/<gate>/run.sh`, bash per `CLAUDE.md`).

- [ ] **Step 3: Run the tests, three times**

Run: `cargo nextest run -p paigasus-iam --no-capture -E 'binary(authz_role_grant_query_plan)' 2>&1 | tee /tmp/sma699-plans-1.txt`
Repeat twice more (`-2`, `-3`). Expected: 6 tests PASS each time, with no `FLAKY` and no `skipping`.

If any plan assertion fails, STOP. Report the failing plan text and the assertion. Do not change the seed counts, the index, or the assertions without a decision from the controller.

- [ ] **Step 4: Prove the acceptance test can fail**

Temporarily remove the `CREATE INDEX` line's effect: in `m0012_role_grant_scope_index.rs` change `(scope_node_prn, principal_id, id)` in the `CREATE INDEX` statement to `(linked_policy_id)`. Run `cargo nextest run -p paigasus-iam -E 'test(the_real_query_shapes_use_an_index)'` and see it FAIL on shape A. Undo the change with the Edit tool (not `git checkout`), and see it PASS again.

- [ ] **Step 5: fmt + clippy, then commit**

```bash
cd rs && cargo fmt && cargo clippy -p paigasus-iam --all-targets -- -D warnings
cd .. && git add rs/crates/services/paigasus-iam/tests/authz_role_grant_query_plan.rs rs/.config/nextest.toml
git commit -m "test(rs): prove the role grant query shapes use an index in both plan modes (SMA-699)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: Final verification and plan capture

**Files:** none changed, unless a gate reds.

- [ ] **Step 1: Rust gates over the workspace**

From `rs/`: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 2: The paigasus-iam suites**

Run: `cargo nextest run -p paigasus-iam -p paigasus-iam-core --no-fail-fast`
Expected: all PASS. Any `skipping` line means Docker was not reachable; fix that and run again. Known flaky suites are listed in the memory file `paigasus-docker-gated-suites-silent-skip.md`; a failure there needs a re-run of that binary only, and a report.

- [ ] **Step 3: Save the plans for the PR**

Run: `cargo nextest run -p paigasus-iam --no-capture -E 'binary(authz_role_grant_query_plan)' 2>&1 | sed -n '/=== /,/^$/p' > <scratchpad>/sma699-plans.txt`
Expected: the file holds the custom and the generic plan for shapes A (offset 0 and 800), B and C, and the two controls.

- [ ] **Step 4: Affected graph**

From the repo root: `moon ci :build :test :lint :fmt --base origin/main --include-relations`
Expected: green, or a failure that Step 2 already explains.
