# SMA-699: an index for the scope-filtered ListRoleGrants query

- Linear: [SMA-699](https://linear.app/smaschek/issue/SMA-699)
- Follow-up of: SMA-676 (PR #315), spec `2026-09-26-sma-676-gateway-user-grant-design.md`
- Status: draft

## Problem

`RoleGrantQuery::find` (`rs/crates/services/paigasus-iam/src/adapters/persistence/pg_role_grants.rs`)
runs one static statement, `FIND_SQL`. Each filter is a predicate of the form
`($n IS NULL OR col = $n)`. The scope filter is `($2::text IS NULL OR g.scope_node_prn = $2)`.

No index can serve a lookup by scope alone:

- `role_grant` has single-column indexes on `principal_id`, `scope_org_id`, `scope_team_id` and
  `scope_project_id` (`migration/m0004_create_authz.rs:164-173`). None is on `scope_node_prn`.
- The unique key `uq_role_grant_principal_role_scope` is on
  `(principal_id, role_key, scope_node_prn)`. It starts with `principal_id`, so a query with a
  scope and no principal cannot use it.
- Postgres can change a prepared statement to a generic plan. In a generic plan the planner does
  not know which `$n` is NULL, so an `IS NULL OR` predicate cannot use an index.

The gateway-console org page (`/gateway/orgs/[org]`) reads the grants at one org with this query:
up to 5 pages of 200 rows, plus one probe, plus one call for each revoke. Each call can scan the
full `role_grant` table. The cost grows with the number of grants on the platform, not with the
size of one org.

## Goal and acceptance criteria

- `EXPLAIN` of the org-scope query (scope, role and kind set, no principal) shows an index scan on
  `role_grant`. This is true with a custom plan and with a generic plan
  (`plan_cache_mode = force_generic_plan`). The PR records both plans.
- No change in behaviour. The Rust unit tests and the Docker suites `tests/authz_role_grants.rs`
  and `tests/authz_forged_org_slot_escalation.rs` pass with no changes.

## Constraints kept from SMA-676

- **D5.** The scope match is exact on the stored `scope_node_prn`. It returns no grant at a
  descendant node. A team PRN with a forged org slot matches no row. The Root sentinel matches
  the Root grants.
- **D6.** The order is `principal_id`, then `id`. `limit` and `offset` work as before. The
  principal-only path (`list_by_principal`) does not change.

## Facts that the design uses

- Every `role_grant` row is written by `grant_to_model` (`pg_role_grants.rs:134-148`). No
  production code inserts a row with raw SQL (checked with a grep over `rs/crates`, excluding
  `tests/`).
- `grant_to_model` sets `scope_node_prn = g.scope.canonical_prn()`. For `GrantScope::Root` this
  is `root_prn().canonical()`. So every Root row has the same, known `scope_node_prn`.
- `RoleGrantFilter::new` returns `None` when both the principal and the scope are absent. So a
  filter that reaches `find` always has a principal or a scope.
- The test Postgres is `16-alpine` (`tests/support/mod.rs:78`).
- `tests/outbox_retention_pg.rs` already runs `EXPLAIN` on the real statement, through a
  `#[doc(hidden)] pub fn published_sweep_sql()`. This design uses the same pattern.

## Decisions

| # | Decision | Reason |
|---|---|---|
| E1 | Add the index `ix_role_grant_scope_node_prn_role` on `role_grant (scope_node_prn, role_key)` in a new migration `m0012_role_grant_scope_index`. | The index is on the same column as the D5 predicate. `scope_node_prn` leads, so a scope-only lookup uses it. `role_key` second serves the org page, which also filters on the role. |
| E2 | Build the `WHERE` clause from the filters that are set. Do not keep any `IS NULL OR` predicate. | A predicate that is always present has a plain `col = $n` form. The planner can then use an index in a generic plan too. |
| E3 | The Root filter becomes `g.scope_node_prn = $n AND g.scope_kind = 'root'`, with `$n` = `GrantScope::Root.canonical_prn()`. | Every Root row stores that PRN (see Facts), so the result is the same rows as before. The new index now also serves a Root lookup. The `scope_kind = 'root'` guard stays, so a row with a different kind can never match. |
| E4 | Keep the `SELECT` list, the inner join on `principal`, `ORDER BY g.principal_id, g.id` and `LIMIT $a OFFSET $b`. | D6. The mapping through `role_grant::Model::find_by_statement` does not change. |
| E5 | Build the index without `CONCURRENTLY`, with `SET LOCAL lock_timeout = '5s'`, and with `IF NOT EXISTS`. | The SeaORM migrator runs a migration in a transaction, and `CREATE INDEX CONCURRENTLY` cannot run in a transaction. The build blocks writes to `role_grant` while it runs. The table is small, and the lock timeout limits the wait. `IF NOT EXISTS` and `SET LOCAL lock_timeout` follow m0008-m0011, because the migrator does not serialize `up()` across replicas. |
| E6 | Expose the builder as `#[doc(hidden)] pub fn find_statement(filter, limit, offset) -> Statement`. | The Docker test must run `EXPLAIN` on the real statement, not on a copy that can drift (the SMA-469 precedent). |

## Design

### Migration `m0012_role_grant_scope_index`

- File: `rs/crates/services/paigasus-iam/src/adapters/persistence/migration/m0012_role_grant_scope_index.rs`.
  Register it in `migration/mod.rs` after m0011.
- `up`: `SET LOCAL lock_timeout = '5s';` then
  `CREATE INDEX IF NOT EXISTS ix_role_grant_scope_node_prn_role ON "role_grant" (scope_node_prn, role_key);`
- `down`: `SET LOCAL lock_timeout = '5s';` then
  `DROP INDEX IF EXISTS ix_role_grant_scope_node_prn_role;`
- The module doc comment gives the reason (SMA-699) and E5.

### The statement builder

`find_statement` replaces the `FIND_SQL` constant. The fixed parts stay as they are:

```sql
SELECT g.id, g.principal_id, g.role_key, g.scope_kind, g.scope_node_prn, g.scope_org_id,
       g.scope_team_id, g.scope_project_id, g.linked_policy_id, g.created_at
  FROM "role_grant" g JOIN "principal" pr ON pr.id = g.principal_id
 WHERE <predicates>
 ORDER BY g.principal_id, g.id
 LIMIT $a OFFSET $b
```

The builder adds one predicate for each filter that is set, in this fixed order, and joins them
with `AND`. Each value gets the next `$n` number, starting at `$1`:

| Filter | Predicate |
|---|---|
| principal | `g.principal_id = $n` (uuid) |
| scope = Node | `g.scope_node_prn = $n` (the node's canonical PRN) |
| scope = Root | `g.scope_node_prn = $n AND g.scope_kind = 'root'` (`$n` = the Root canonical PRN) |
| role key | `g.role_key = $n` |
| principal kind | `pr.kind = $n` (`PrincipalKind::as_str`) |

`limit` and `offset` take the last two numbers. The `WHERE` clause is never empty, because the
filter always has a principal or a scope. The builder does not need to handle an empty filter,
but it must not produce invalid SQL for one: if no predicate is set, it writes `WHERE TRUE`. No
caller can reach that case.

The values are bound parameters. The builder never puts a filter value into the SQL text. The
only literal in the SQL is the constant `'root'`.

`RoleGrantQuery::find` calls `find_statement` and then `find_by_statement(...).all(...)`, as now.

### Error handling

No change. Database errors map through `map_err`, and row mapping through `model_to_grant`.

## Tests

### Unit tests (no Docker), in `pg_role_grants.rs`

For the builder, over the filter mixes that `RoleGrantFilter::new` accepts:

- The SQL has no `IS NULL`.
- The number of bound values equals the highest `$n` in the SQL, and the numbers are `$1..$k`
  with no gap.
- The node scope gives `g.scope_node_prn = $n`, and its value is the node's canonical PRN.
- The Root scope gives `g.scope_node_prn = $n AND g.scope_kind = 'root'`, and its value is
  `GrantScope::Root.canonical_prn()`.
- An unset filter adds no predicate (for example, no `pr.kind` in the SQL when no kind is set).
- `ORDER BY g.principal_id, g.id` and `LIMIT`/`OFFSET` are present, and `limit` and `offset` are
  the last two values.

### Docker test: `tests/authz_role_grant_query_plan.rs` (new)

- Start a migrated Postgres with `support::start_migrated_postgres()`. Skip when Docker is not
  available, as the other suites do.
- Seed a large table: many organizations and many grants spread over them, so that one org holds
  a small part of the table. The plan fixes the counts. Then run `ANALYZE` on `role_grant`,
  `principal` and `organization` (the reason is the one in `outbox_retention_pg.rs`).
- Build the org-scope statement with `find_statement` (scope = one org, role = `gateway_user`,
  kind = `user`, no principal, `limit` 200, `offset` 0).
- `plan_cache_mode` controls only the plan cache of a prepared statement. A plain `EXPLAIN` with
  bound parameters does not go through that cache, so it cannot show the generic plan. The test
  therefore uses a named prepared statement, in one transaction:
  `PREPARE q(...) AS <find_statement SQL>`, then `SET LOCAL plan_cache_mode = force_custom_plan`
  and `EXPLAIN EXECUTE q(<values>)` for the custom plan, then
  `SET LOCAL plan_cache_mode = force_generic_plan` and `EXPLAIN EXECUTE q(<values>)` for the
  generic plan. The `PREPARE` text is the SQL that `find_statement` returns, not a copy. The
  `EXECUTE` values are test literals. A generic plan shows `$n` in its filter lines, and the
  test asserts this, to prove that the second plan really is generic.
- For both plans, assert that the plan has no `Seq Scan on role_grant`, and that it names
  `ix_role_grant_scope_node_prn_role`. Print both plans with `eprintln!`, so they can go into the
  PR.
- One more assertion shows that the test can fail: without the index (`DROP INDEX` in the same
  transaction, then `EXPLAIN`), the plan does not name the index. This proves that the index
  assertion depends on the index.

### Suites that must pass with no change

- `tests/authz_role_grants.rs`
- `tests/authz_forged_org_slot_escalation.rs`
- the unit tests in `paigasus-iam` and `paigasus-iam-core`

## Out of scope

- The principal-only path (`list_by_principal`).
- Any change to the proto, the HTTP layer, the gRPC layer, the domain types or the console.
- Keyset paging in place of `OFFSET`.
