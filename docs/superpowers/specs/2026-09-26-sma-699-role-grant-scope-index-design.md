# SMA-699: an index for the scope-filtered ListRoleGrants query

- Linear: [SMA-699](https://linear.app/smaschek/issue/SMA-699)
- Follow-up of: SMA-676 (PR #315), spec `2026-09-26-sma-676-gateway-user-grant-design.md`
- Status: draft, revision 2 (after the adversarial challenge)

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
- sea-orm sends `find_by_statement` through a cached, named sqlx prepared statement. Postgres can
  change such a statement to a generic plan. In a generic plan the planner does not know which
  `$n` is NULL, so an `IS NULL OR` predicate cannot use an index.

## The real query shapes

Three callers send this query today:

| Shape | Caller | Filters | Paging |
|---|---|---|---|
| A | the org page list, `ts/apps/gateway-console/app/(console)/people-model-access/load.ts:78` | scope (org) + kind (`user`). No principal, no role. | `limit` 200, `offset` 0, 200, ..., 800, plus one probe |
| B | the revoke check, `.../people-model-access/commands.ts:91` | principal + scope + role + kind | default (`limit` 50, `offset` 0) |
| C | the grant pre-check, `application/roles.rs:211-213` | principal + scope + role | `limit` 1 |

The console filters holders on `roleKey` after the read (SMA-676 spec). So shape A has no role
predicate. Shape A is the query that can scan the full table, 6 times for one page view.
Shapes B and C have a principal, so the unique key can serve them when the SQL has no
`IS NULL OR` form.

## Goal and acceptance criteria

- **Shape A** (scope + kind, no principal, at `offset` 0 and at `offset` 800): the plan uses the
  new index `ix_role_grant_scope_node_prn_principal_id` on `role_grant`, with a custom plan and
  with a generic plan (`plan_cache_mode = force_generic_plan`). "Uses the index" means an Index
  Scan or a Bitmap Index Scan node that names the index, and no `Seq Scan on role_grant`.
- **Shapes B and C**: the plan has no `Seq Scan on role_grant`, with a custom plan and with a
  generic plan.
- The PR records the plans for all three shapes, both plan modes.
- No change in behaviour. The Rust unit tests and the Docker suites `tests/authz_role_grants.rs`
  and `tests/authz_forged_org_slot_escalation.rs` pass with no changes. A new equivalence test
  (see Tests) shows that every accepted filter shape returns the rows that the domain oracle
  `RoleGrantFilter::matches` selects, in `(principal_id, id)` order.

## Constraints kept from SMA-676

- **D3.** A filter must have a principal or a scope. No call lists the grants of every tenant.
- **D5.** The scope match is exact on the stored `scope_node_prn`. It returns no grant at a
  descendant node. A team PRN with a forged org slot matches no row. The Root sentinel matches
  `scope_kind = 'root'` rows.
- **D6.** The order is `principal_id`, then `id`. `limit` and `offset` work as before. The
  principal-only path (`list_by_principal`) does not change.

## Facts that the design uses

- Two production writers insert `role_grant` rows: `grant_to_model`
  (`pg_role_grants.rs:134-148`) and `owner_grant_to_model` (`pg_organizations.rs:120-134`). Both
  set `scope_node_prn = g.scope.canonical_prn()`.
- No CHECK constraint pins the `scope_node_prn` of a Root row, and no migration rewrites it. The
  SMA-444 plan once stored Root as `"paigasus"`. So this design does not assume that every Root
  row stores `root_prn().canonical()`.
- `RoleGrantFilter::new` returns `None` when both the principal and the scope are absent.
- The test Postgres is `16-alpine` (`tests/support/mod.rs:78`).
- Production runs every pending migration in one outer transaction under an advisory lock
  (`migrate_under_lock`, `migration_lock.rs:124-183`).
- `tests/outbox_retention_pg.rs` already runs `EXPLAIN` on the real statement, through a
  `#[doc(hidden)] pub fn published_sweep_sql()`.
- The nextest `kind(test)` override gives Docker test binaries `retries = 2`
  (`rs/.config/nextest.toml:89-97`).

## Decisions

| # | Decision | Reason |
|---|---|---|
| E1 | Add the index `ix_role_grant_scope_node_prn_principal_id` on `role_grant (scope_node_prn, principal_id, id)` in a new migration `m0012_role_grant_scope_index`. | `scope_node_prn` leads, so shape A finds the org's rows. `principal_id, id` follow the `ORDER BY`, so an Index Scan returns rows in order with no Sort node, and a `LIMIT` stops after `offset + limit` rows. A nested loop to `principal_pkey` keeps that order. Rejected: `(scope_node_prn, role_key)`. Shape A has no role, so the second column gives no benefit, and every page reads and sorts all rows of the org. |
| E2 | Build the `WHERE` clause from the filters that are set, with a small hand-written builder. Do not keep any `IS NULL OR` predicate. | A predicate that is always present has a plain `col = $n` form, so the planner can use an index in a generic plan too. Rejected: two static statements (scope-required and principal-required, the `pg_memberships.rs` pattern). They keep `IS NULL OR` on the optional role and kind predicates, and shape B then depends on those. Rejected: the sea-query builder. It works, but the SQL is then not readable in one place, and the test must `PREPARE` the exact text. The hand-written builder has one job and full unit tests. |
| E3 | The Root filter stays `g.scope_kind = 'root'`, as today. | This is the current behaviour, so no stored-data invariant is needed. A Root list is a platform-admin path with few rows, and it is not in the acceptance criteria. No Root index is added. |
| E4 | Keep the `SELECT` list, the inner join on `principal`, `ORDER BY g.principal_id, g.id` and `LIMIT/OFFSET`. Keep the type casts on the parameters (`$n::uuid`, `$n::text`). | D6. The mapping through `role_grant::Model::find_by_statement` does not change. |
| E5 | Build the index without `CONCURRENTLY`, with `SET LOCAL lock_timeout = '5s'` and `IF NOT EXISTS`. After the create, fail the migration if the index is INVALID. | Production runs all pending migrations in one outer transaction, so `CONCURRENTLY` is not possible. `IF NOT EXISTS` makes `up` idempotent and lets an operator build the index `CONCURRENTLY` before the deploy. An INVALID index (from a failed out-of-band build) with the same name would make `IF NOT EXISTS` skip the create while the planner ignores the index. The check reads `pg_index.indisvalid`. |
| E6 | Expose the builder as `#[doc(hidden)] pub fn find_statement(filter, limit, offset) -> Statement`. | The Docker tests must use the real statement, not a copy that can drift (the SMA-469 precedent). |
| E7 | An empty filter gives `WHERE FALSE`. No `debug_assert!`: `RoleGrantFilter::new` is the only constructor. An assert would make the `WHERE FALSE` unit test panic in debug. | D3. No caller can reach this case. If one does, it must return no rows, not every grant. |

Also rejected: filter a node scope on the typed columns (`scope_org_id`, `scope_team_id`,
`scope_project_id`, indexes exist) next to the `scope_node_prn` equality. It needs no migration,
and `ck_role_grant_scope` keeps D5. But it gives no ordered scan (every page sorts all rows of
the org), and the SQL has one branch for each node kind. Sven chose the index approach.

## Design

### Migration `m0012_role_grant_scope_index`

- File: `rs/crates/services/paigasus-iam/src/adapters/persistence/migration/m0012_role_grant_scope_index.rs`.
  Register it in `migration/mod.rs` after m0011.
- `up`:
  1. `SET LOCAL lock_timeout = '5s';`
  2. `CREATE INDEX IF NOT EXISTS ix_role_grant_scope_node_prn_principal_id ON "role_grant" (scope_node_prn, principal_id, id);`
  3. Read `SELECT i.indisvalid FROM pg_index i JOIN pg_class c ON c.oid = i.indexrelid WHERE c.relname = 'ix_role_grant_scope_node_prn_principal_id'`.
     If the row is absent or `indisvalid` is false, return a `DbErr::Migration` that names the
     index and says: drop the index, then run the migration again.
- `down`: `SET LOCAL lock_timeout = '5s';` then `DROP INDEX IF EXISTS ix_role_grant_scope_node_prn_principal_id;`
- The module doc comment gives the reason (SMA-699), E5, and the operator path for a large table.

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
with `AND`. Each bound value gets the next `$n` number, starting at `$1`:

| Filter | Predicate | Bound value |
|---|---|---|
| principal | `g.principal_id = $n::uuid` | the principal uuid |
| scope = Node | `g.scope_node_prn = $n::text` | the node's canonical PRN |
| scope = Root | `g.scope_kind = 'root'` | none |
| role key | `g.role_key = $n::text` | the role key |
| principal kind | `pr.kind = $n::text` | `PrincipalKind::as_str` |

`limit` and `offset` take the last two numbers, bound as today (`u64`, which sea-query-sqlx binds
as `bigint`). If no predicate is set, the builder writes `WHERE FALSE` (E7). The builder never
puts a filter value into the SQL text. The only literal is the constant `'root'`.

`RoleGrantQuery::find` calls `find_statement` and then `find_by_statement(...).all(...)`, as now.

### Error handling

No change. Database errors map through `map_err`, and row mapping through `model_to_grant`.

## Rollout and rollback

- The old binary works with the new index. The new binary returns correct results without the
  index. So the deploy order does not matter.
- A rollback is the old binary. The index stays in place. `down` is for development only.
- The `CREATE INDEX` holds a SHARE lock on `role_grant` until the outer migration transaction
  commits. The lock blocks grants, revokes, org creation (the owner grant insert) and deletes
  that cascade into `role_grant`. Reads continue. The platform is pre-GA and the table holds
  few rows, so the build takes milliseconds. If the lock wait exceeds 5 s, the migration run
  fails, the boot fails, the pod restarts, and the migration runs again. For a large table, an
  operator builds the index `CONCURRENTLY` before the deploy, and m0012 then only checks it.

## Tests

### Unit tests (no Docker), in `pg_role_grants.rs`

For the builder, over every filter shape that `RoleGrantFilter::new` accepts (principal x scope
{none, node, Root} x role x kind, minus the shapes with no principal and no scope):

- The SQL has no `IS NULL`.
- The number of bound values equals the highest `$n` in the SQL, and the numbers are `$1..$k`
  with no gap.
- A node scope gives `g.scope_node_prn = $n::text` with the node's canonical PRN. Root gives
  `g.scope_kind = 'root'` and binds no scope value.
- An unset filter adds no predicate.
- `ORDER BY g.principal_id, g.id`, `LIMIT` and `OFFSET` are present, and `limit` and `offset`
  are the last two values.
- The empty filter case: call the private builder core with no predicates and assert
  `WHERE FALSE` (the public function cannot receive an empty filter).

### Docker test: `tests/authz_role_grant_query_plan.rs` (new)

**Seed.** Written with a bulk `INSERT ... SELECT generate_series` after the migrations, as in
`outbox_retention_pg.rs`:

- About 2,000 organizations and about 10,000 principals (both kinds, mostly `user`).
- About 25,000 grants over those orgs and principals: each principal has few grants, several role
  keys are used, and the target org holds fewer than 1% of the rows. Every `scope_node_prn` is
  the canonical org PRN (`TenancyNodeRef::canonical`), so the rows match what the service writes.
- At most 30,000 rows in each seeded table, so `ANALYZE` reads the full table at the default
  statistics target and the statistics are the same on every run.
- `ALTER TABLE ... SET (autovacuum_enabled = false)` on `role_grant` and `principal`, then
  `ANALYZE` on both. (The query does not read `organization`.)
- A sanity check: the real `RoleGrantQuery::find` for shape A on the target org returns the
  expected row count. This also proves that the seeded PRNs use the canonical form.

**Plans.** `plan_cache_mode` controls only the plan cache of a prepared statement. A plain
`EXPLAIN` with bound parameters does not go through that cache. So, in one transaction, through
`execute_unprepared` on that transaction:

1. `PREPARE q(<types>) AS <the SQL from find_statement>`. The types are the ones production
   binds, in `find_statement` order (`uuid`, `text`, `bigint`).
2. `SET LOCAL plan_cache_mode = force_custom_plan`, then `EXPLAIN EXECUTE q(<literals>)`.
   Assert that the plan contains no `$1` (proof that it is a custom plan).
3. `SET LOCAL plan_cache_mode = force_generic_plan`, then `EXPLAIN EXECUTE q(<literals>)`.
   Assert that the plan contains `$` parameters in its filter or index condition lines (proof
   that it is a generic plan).

Do this for shape A at `offset` 0 and at `offset` 800, and for shapes B and C. Assertions:

- Shape A, both plan modes: the plan names `ix_role_grant_scope_node_prn_principal_id` in an
  Index Scan or Bitmap Index Scan node, and has no `Seq Scan on role_grant`. The test pins the
  index name, although the outbox precedent does not. Here the seed rules fix the table shape,
  and the new index is the object under test.
- Shapes B and C, both plan modes: no `Seq Scan on role_grant`.

**Controls** (each one can fail):

1. Without the new index (`DROP INDEX` in a transaction, then roll back): shape A's custom plan
   asserts `Seq Scan on role_grant`. The measured generic plan takes a different full read: a full
   Index Scan on `uq_role_grant_principal_role_scope` (m0004), because PostgreSQL 16 has no B-tree
   skip scan. Both are full reads of `role_grant`, so either one proves that the seed makes the
   index matter.
2. With the index, `PREPARE` the old `IS NULL OR` statement (kept in the test as a fixture) under
   `force_generic_plan`: its plan does not name the new index. This proves that E2 is needed.

**Retries.** Add a nextest override so that this binary runs with `retries = 0`. A plan that
changes from run to run must show as a failure, not as FLAKY.

**Plans for the PR.** Every plan is printed with `eprintln!`. Capture them with
`cargo nextest run -p paigasus-iam --no-capture -E 'binary(authz_role_grant_query_plan)'`.

### Docker test: equivalence with the domain oracle (new, in the same file)

- Seed a fixed grant set: grants at Root, at two orgs, at a team and at a project; several
  principals of both kinds; several roles.
- For every filter shape that `RoleGrantFilter::new` accepts, with values taken from the seed,
  plus a team PRN with a forged org slot: compare `RoleGrantQuery::find` with the grants that
  `RoleGrantFilter::matches` selects, sorted by `(principal_id, id)`.
- Include `limit`/`offset` cases: `limit` 1, and an `offset` past the end.

### Docker test: migration round trip (new, in the same file)

With `start_raw_postgres` and `Migrator::up(&db, Some(n))` (the pattern in
`audit_log_partition_pg.rs:191-215`): run `up` to m0012, then `down` one step, then `up` again.
After each `up`, assert that the index exists and `indisvalid` is true. After `down`, assert that
it does not exist. Also: create an INVALID index with the same name (a build that fails on a
duplicate is not needed; mark it with `UPDATE pg_index SET indisvalid = false`), then run `up`
and assert that m0012 fails with the message from E5.

### Suites that must pass with no change

- `tests/authz_role_grants.rs`
- `tests/authz_forged_org_slot_escalation.rs`
- the unit tests in `paigasus-iam` and `paigasus-iam-core`

## Out of scope

- The principal-only path (`list_by_principal`).
- An index for a Root list (E3).
- The access path into `principal`. The plans are printed, but the test does not assert on it.
- Any change to the proto, the HTTP layer, the gRPC layer, the domain types or the console.
- Keyset paging in place of `OFFSET`.

## Open questions for Sven

- Production Postgres version, `plan_cache_mode`, and any connection pooler (for example
  PgBouncer in transaction mode). The tests measure Postgres 16 with direct connections only.
- Expected production row counts for `role_grant` and `principal`. The lock estimate above
  assumes a pre-GA table.
