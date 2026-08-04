# SMA-481 — A retired system role's policy and role rows are undeletable and keep granting

**Status:** design
**Date:** 2026-08-04
**Issue:** [SMA-481](https://linear.app/smaschek/issue/SMA-481/iam-a-retired-system-roles-policy-and-role-rows-are-undeletable-and)
**Project:** Paigasus IAM — Hardening
**Follows:** [SMA-477](https://linear.app/smaschek/issue/SMA-477/iam-starter-policies-reconcile-by-compare-and-warn-so-any-action) (PR 104), whose design doc §7 recorded this as out of scope

## 1. Problem

SMA-477 made boot converge system-owned starter policies and system role rows to the
code-defined content — but **only additively**. Retire a role by dropping it from
`authz::roles::system_roles()` / `starter_policies()` and its persisted rows stay behind
forever, still granting.

All line references verified against `main` at `6f2ed27`.

### 1.1 The four walls

1. **The `policy` row keeps compiling.** `PgPolicyStore::list_all` returns every row
   unconditionally, so `PolicyEngine::compile` parses the retired role's template on every
   snapshot reload.
2. **Grants of the retired role keep conferring permission.** `compile` links a grant whenever
   its template is present in the set (`engine.rs:88-93`):

   ```rust
   for grant in grants {
       let template_id = PolicyId::new(&grant.role_key);
       if policy_set.template(&template_id).is_some() {
           link_grant(&mut policy_set, &grant.role_key, grant)?;
       }
   }
   ```

   The template is present, so the grant links, so it grants.
3. **`DeletePolicy` refuses to remove it.** `PgPolicyStore::delete_in` (`pg_policies.rs:376-378`)
   returns `AuthzError::SystemImmutable` for any persisted `system = true` row.
4. **The `role` row cannot simply be dropped either.** `role_grant.role_key` carries
   `fk_role_grant_role` to `role.key` (`m0004_create_authz.rs:134`) with no `ON DELETE` action,
   i.e. `NO ACTION` / restrict.

Net effect: an undeletable, un-revocable grant of a role the codebase no longer defines.

### 1.2 The ordering is forced by the schema, and there is no shortcut

Because §1.1(2) links on the *template*, deleting the **policy** row alone would be enough to
stop a retired role granting. That shortcut does not exist: `role.template_id` carries
`fk_role_template` to `policy.policy_id` (`m0004_create_authz.rs:103`), also restrict. So while
the role row is present the policy row cannot be deleted, and while grants are present the role
row cannot be deleted. The order the issue names is the only order the schema permits:

```
role_grant  →  role  →  policy
```

### 1.3 What SMA-477 shipped instead, and why it is not enough

Detection only, deliberately (D13). `orphaned_system_policy_ids` / `orphaned_system_role_keys`
scan for system-owned rows whose id is no longer code-defined, and `reconcile_policies` /
`reconcile_roles` log one `WARN` per orphan (`bootstrap.rs:220-226`, `:347-349`), with the
policy half also counting `outcome="orphaned"`. No audit row, because the scan runs on every
boot of every replica and `audit_log` is append-only.

So an operator is told, precisely, and has no supported way to act. Raw SQL remains available to
anyone with database access, but it is unordered, unaudited, invisible to `policy_gen`, and
easy to get wrong in exactly the way §1.2 describes.

### 1.4 What reading the code found beyond the issue

**(a) A retired role can no longer be granted through the API.** `RoleService::grant`
(`roles.rs:201`) resolves `authz_roles::role(role_key)` and returns `TenancyError::UnknownRole`
when the code catalog does not define it. The code catalog, not the `role` table, is the gate.
This materially shrinks the concurrency surface — see D6 for the one case it does not cover.

**(b) `platform_admin`'s template carries no action list.** `template_source`
(`roles.rs:315-317`) returns `permit(principal == ?principal, action, resource in ?resource);`
for `platform_admin` — every action, unrestricted. A new action therefore needs **no role
template change at all**.

**(c) An operator-maintenance HTTP surface already exists.** SMA-469 added
`/v1/outbox/dead-letters/{id}/replay|discard` — Root-only, enforced inside the service rather
than the Cedar schema, HTTP-only with no gRPC mirror and no `contracts/` change
(`http/dead_letters.rs:19-22`). That is the precedent this design follows in full.

**(d) `EventType::PolicyDeleted` already exists.** `PolicyService::delete`
(`policies.rs:175`) emits it. Retirement needs no new event type.

**(e) The Cedar schema's action list is hand-maintained.** `SCHEMA_SRC`
(`schema.rs:19-27`) enumerates every action by name. A new action must be added there or
`validate_policy` rejects the generated `forbid-archived-writes` source.

## 2. Decisions

### D1 — Retirement is a runtime, operator-initiated operation, not a migration

The issue asks the question directly: migration or runtime? A migration is simpler and safer to
reason about, but it fails the actual requirement. The operator who reads the boot `WARN` cannot
act on it — they must wait for someone to author, review, and release a bespoke migration. That
is only marginally better than today, and it makes each retirement a code change rather than an
operation.

A runtime path is also the only one that can audit through the normal path, emit the domain
event every other writer of `policy` emits, and bump `policy_gen` so serving replicas pick the
change up immediately rather than on the snapshot TTL backstop. A migration runs before
`AppState::new` and can do none of those things.

**Rejected alternative — code-declared retirement executed at boot.** A `RETIRED_SYSTEM_ROLES`
const, retired automatically once grants reach zero. It is defensible (it never deletes a grant,
so it never changes who can do what) but it sits directly against the issue's stated non-goal,
and it makes a destructive act a side effect of a deploy. Deferred, not adopted.

### D2 — One endpoint retires the whole chain, keyed by the policy id

For a role template, `policy_id == Role::key == Role::template_id` (`roles.rs:285`), so a single
id names the policy row, the role row, and the grant key at once.

```
POST /v1/authz/system-policies/{id}/retire
```

A retired **static** starter policy (`forbid-archived-writes` is the only one today) takes the
identical path and simply finds no role row. No special-casing, no second endpoint, no
half-retired intermediate state for an operator to reason about.

**Rejected alternative — two endpoints** (`/system-roles/{key}/retire` +
`/system-policies/{id}/retire`). The nouns match the tables more exactly, but it makes every
role retirement two ordered calls and introduces "role gone, policy still there" as a real,
reachable, observable state whose only merit is that it is an artifact of the API shape.

### D3 — The destructive deletes live on the reconciler ports, never on `PolicyStore::delete_in`

`delete_in`'s `SystemImmutable` guard is exactly what must keep holding for the public
`DeletePolicy` API, and SMA-477 D6 added a second layer (`put_in` rejects on the reserved *id*,
not merely on a stored row's `system` flag). Neither is relaxed here.

`SystemPolicyReconciler` is already the port that owns system rows and already bypasses the
public guard by design — `reconcile_system` "deliberately does NOT go through `put_in`"
(`pg_policies.rs:42-51`). Removal belongs on the same port, for the same reason. This reuses
SMA-477's argument rather than re-opening it.

**Rejected alternative — relax `delete_in` for orphans**, i.e. refuse a system row only while
`is_starter_policy_id(id)` holds. It needs no new action and no catalog churn, which is
genuinely attractive. But nothing can then delete the `role` row, so `fk_role_template` blocks
the policy delete for every retired *role* — leaving it useful only for retired static policies,
which is the rarer half of the problem.

### D4 — Retirement never deletes a grant

When grants of the retired key survive, the endpoint refuses and names them. The operator
revokes each through the existing `DELETE /v1/authz/role-grants/{id}`, then retries.

This is not timidity about blast radius; it is what each path actually produces. A revocation
through `RoleService::revoke` gets its own audit row, its own `DomainEvent`, and the
anti-escalation check at that grant's own scope. A bulk cascade inside retirement would have to
reproduce all three faithfully or become precisely the "silently dropping grants is an
authorization change" the issue warns against.

The consequence is worth stating plainly, because it is the design's central safety property:
**retirement only ever removes rows that are provably inert, so it cannot change who can do
what.** A `role` row with no grants confers nothing. A `policy` row whose template no grant
links confers nothing.

The cost is a retired role with many grants being tedious to clean up. Bulk revocation is a
follow-up (§7), and the 409 body gives the operator the exact id list to drive it.

### D5 — A blocked retirement is an outcome, not an error

The 409 must carry the surviving grants, and `TenancyError` is shared by every service in the
application layer with a flat `code()` mapping. Threading a `Vec<RoleGrant>` through it to reach
one handler would distort a type that fourteen call sites depend on.

So `SystemRetirementService::retire` returns `Result<RetireOutcome, TenancyError>`:

```rust
pub enum RetireOutcome {
    /// The chain was removed. `role_deleted` is false for a retired static policy.
    Retired { role_deleted: bool },
    /// Nothing was written. The listed grants must be revoked first.
    Blocked { role_key: String, grants: Vec<RoleGrant> },
}
```

`TenancyError` keeps its meaning: something went wrong. A blocked retirement is the system
working correctly and saying so.

The handler builds the 409 itself rather than going through `ApiError`, preserving the stable
`{"error": {"code", "message"}}` contract and adding a sibling field:

```json
{ "error": { "code": "grants-survive",
             "message": "3 grants of 'legacy_auditor' must be revoked before it can be retired" },
  "grants": [ { "id": "…", "principal_prn": "…", "scope_prn": "…" } ] }
```

### D6 — The role row is locked `FOR UPDATE` before the grant count is trusted

Per §1.4(a) the public API cannot grant a retired role, so the obvious race is already closed.
One case remains, and it is not hypothetical: **a replica running an older binary mid-deploy
still has the role in its code catalog** and will happily grant it — a rollback, a crashloop
restart, an HPA scale-up, a held canary. This is the same fleet-skew scenario
`STARTER_POLICY_REVISION` exists for (SMA-477 D11), against a shared table.

Postgres takes a `FOR KEY SHARE` lock on the parent row when inserting a foreign-key child, so
`SELECT … FROM role WHERE key = $1 FOR UPDATE` genuinely blocks such an insert for the
transaction's duration. Without it the FK still protects correctness — the `DELETE` fails — but
it surfaces as a raw `fk_role_grant_role` violation mapped to an opaque `AuthzError::Backend`,
which tells an operator nothing about what actually happened.

The transaction also sets `SET LOCAL lock_timeout = '5s'`, mirroring `reconcile_system`
(`pg_policies.rs:404`): an operator-triggered request must fail with a message rather than hang
on a lock held by a concurrent writer.

### D7 — Retiring a still-code-defined id is refused, and it is the load-bearing guard

`is_starter_policy_id(id)` is checked before anything is read. Retiring a live starter policy
would be re-seeded by `reconcile_starter` at the next boot anyway — but in the window between
the two, that policy is not governing decisions. For `forbid-archived-writes` that means archived
resources become writable; for a role template it means every grant of that role silently stops
working.

Reusing `TenancyError::SystemImmutable(id)` gives the right words and the existing `Conflict`
mapping, with no new error variant.

A row that exists but has `system = false` is refused too, with a new
`TenancyError::NotSystemOwned(String)` (`ErrorClass::Conflict`). Retirement must not become a
second, differently-audited delete path for operator-authored policies; `DeletePolicy` already
serves those and applies its own authorization.

### D8 — One new action, Root-only, enforced in the service

`Action::RetireSystemPolicy`. Root-only-ness lives in `SystemRetirementService`, which always
authorizes at `root_prn()` — the pattern `DeadLetterService`, `AuditQueryService::list` and
`PolicyService::list` all use, rather than encoding it in the Cedar schema's shared `appliesTo`
block (`dead_letters.rs:5-8`).

It is `is_write() == true` and `is_restore() == false`, so it enters the generated
`forbid_archived_writes_source()` action list. That changes starter policy content, which
requires `STARTER_POLICY_REVISION` 1→2 and a new `EXPECTED_STARTER_CONTENT_HASH`. This is a
known, tested, one-line cost now — it is the exact scenario SMA-477 was built to absorb, and
every seeded database converges silently on the next boot.

No role template gains the action: `platform_admin` is unrestricted (§1.4(b)), and no
lower-privilege role should be able to retire a system row.

### D9 — Retirement audits what it destroyed, and emits `PolicyDeleted`

One audit entry, `action = "RetireSystemPolicy"`, `resource_prn = root_prn()`, actor set from
the bearer-resolved `AuthContext`. Its detail records the destroyed `kind`, `source` and
`description` — retirement is the one operation that removes the evidence, which is precisely
the case SMA-477's `truncate_audited_text` / `MAX_AUDITED_SOURCE_BYTES` cap was written for, and
it is reused verbatim rather than re-derived.

One `DomainEvent` with the existing `EventType::PolicyDeleted`. This deliberately differs from
`DeadLetterService`, which emits nothing because "an outbox event about outbox operations would
be circular" — retirement genuinely is a change to policy content, so it emits like every other
writer of that table.

Both share the mutation's single `UnitOfWork` transaction (the `application::roles` reference
pattern), so a mid-transaction failure leaves none of the three.

### D10 — `policy_gen` is bumped post-commit and awaited

Exactly as `PolicyService::delete` does: after the commit, awaited, so the change is guaranteed
visible to the next `is_authorized` rather than landing on the snapshot's TTL backstop. A bump
failure is swallowed and logged by the `PolicyGenBumper` implementation itself — the deletion
already committed, and a Redis blip must never fail an already-successful write.

## 3. The fix

### 3.1 Action catalog — `paigasus-iam-core`

- `authz/action.rs`: add `Action::RetireSystemPolicy` to the enum, to `Action::ALL`, to
  `as_wire()`, and to the `is_write()` write arm. Extend the existing wire-string and
  write/restore classification tests.
- `authz/schema.rs`: add `RetireSystemPolicy` to `SCHEMA_SRC`'s action list (§1.4(e)) — without
  this, `validate_policy` rejects the newly-generated `forbid-archived-writes` source and boot
  fails.
- `authz/roles.rs`: `STARTER_POLICY_REVISION` 1→2, and `EXPECTED_STARTER_CONTENT_HASH` updated to
  the new pinned hash. No `*_ACTIONS` const changes.

### 3.2 Ports — `paigasus-iam-core`

Five additions, each narrow and on the port that already owns the concern (SMA-477 D5's
"narrow boot-only ports, not a seventh `PolicyStore` method" applied consistently):

```rust
// authz/ports.rs — RoleGrantStore
/// Every surviving grant of `role_key`, ordered by `id` so a retirement refusal
/// lists them deterministically. Used only by the retirement path.
async fn list_by_role_key_in(&self, tx: &dyn Transaction, role_key: &str) -> Result<Vec<RoleGrant>, AuthzError>;

// PolicyStore
/// Reads one policy document inside the caller's transaction, so the retirement
/// path's system/orphan checks and its deletes see one consistent view.
async fn find_in(&self, tx: &dyn Transaction, policy_id: &str) -> Result<Option<PolicyDocument>, AuthzError>;

// SystemRoleReconciler
/// Locks `key`'s row `FOR UPDATE` and returns whether it exists — taken BEFORE the
/// grant count so a concurrent FK-child insert from an older replica blocks (D6).
async fn lock_role_in(&self, tx: &dyn Transaction, key: &str) -> Result<bool, AuthzError>;
/// Deletes the `role` row. Returns whether one existed.
async fn delete_role_in(&self, tx: &dyn Transaction, key: &str) -> Result<bool, AuthzError>;

// SystemPolicyReconciler
/// Deletes a system-owned `policy` row, bypassing `delete_in`'s SystemImmutable
/// guard. Callers must have established the row is orphaned (D3/D7).
async fn retire_policy_in(&self, tx: &dyn Transaction, policy_id: &str) -> Result<bool, AuthzError>;
```

All five are transaction-scoped, so the whole chain commits or none of it does — and, critically,
the grant count is read on the **same** transaction that holds the D6 lock, not through a second
connection whose view of the lock is irrelevant. `PgSystemRoleReconciler` has no
`&dyn Transaction` methods today; these are its first, using the existing `uow::recover_txn`
helper the other adapters use.

### 3.3 Application — `application/system_retirement.rs` (new)

A dedicated service rather than a method on `PolicyService`: retirement spans three ports
`PolicyService` does not hold, and `policies.rs` is already carrying the full CRUD surface.
Modelled field-for-field on `DeadLetterService` — a `SystemRetirementDeps` bag, `Arc`-held ports,
`Authorize`, and Root-only enforcement inside the service.

```rust
pub async fn retire(&self, actor: &Prn, id: &str) -> Result<RetireOutcome, TenancyError> {
    // 1. Root-only.
    self.authorize.check(actor, Action::RetireSystemPolicy, &root_prn()).await?;

    // 2. Still code-defined → refuse before reading anything (D7).
    if authz_roles::is_starter_policy_id(id) {
        return Err(TenancyError::SystemImmutable(id.to_string()));
    }

    let tx = self.uow.begin().await?;

    // 3. The row must exist and must be system-owned.
    let Some(doc) = self.policies.find_in(&*tx, id).await? else { return Err(TenancyError::NotFound) };
    if !doc.system { return Err(TenancyError::NotSystemOwned(id.to_string())); }

    // 4. Lock the role row BEFORE counting grants (D6).
    let role_exists = self.roles.lock_role_in(&*tx, id).await?;

    // 5. Survivors block the retirement; nothing is written (D4/D5). Read on the SAME
    //    transaction that holds the lock taken in step 4.
    let grants = self.grants.list_by_role_key_in(&*tx, id).await?;
    if !grants.is_empty() {
        return Ok(RetireOutcome::Blocked { role_key: id.to_string(), grants });
    }

    // 6/7. role → policy, the only order the FKs permit (§1.2).
    let role_deleted = if role_exists { self.roles.delete_role_in(&*tx, id).await? } else { false };
    self.policies.retire_policy_in(&*tx, id).await?;

    // 8. Audit + event share the mutation's transaction (D9).
    self.outbox.enqueue(&*tx, &event).await?;
    self.audit.record(&*tx, &entry).await?;
    tx.commit().await?;

    // 9. Awaited, post-commit (D10).
    self.gen_bumper.bump().await;
    Ok(RetireOutcome::Retired { role_deleted })
}
```

Dropping `tx` without committing rolls back, so every early return above is safe — the
`DeadLetterService::replay` posture.

### 3.4 HTTP — `adapters/http/system_retirement.rs` (new)

One route, its own file, mirroring `http/dead_letters.rs`. `http/authz.rs` — which also carries
the `is_authorized` hot path — is left untouched.

```rust
Router::new().route("/v1/authz/system-policies/{id}/retire", post(retire))
```

Mounted on the same bearer-gated `protected` sub-router; the actor comes from the auth
middleware's `AuthContext`, never a client-supplied value. The handler returns
`Result<Response, ApiError>`: `Retired` → `204 No Content`, `Blocked` → the 409 body in D5.

### 3.5 Error type — `application/error.rs`

One new variant, `NotSystemOwned(String)`, classified `ErrorClass::Conflict` with code
`not-system-owned`. `SystemImmutable` and `NotFound` are reused as-is.

### 3.6 Composition root

`AppState::new` builds `SystemRetirementService` from the already-constructed `PgPolicyStore`
(which implements both `PolicyStore` and `SystemPolicyReconciler`), `PgSystemRoleReconciler`,
`PgRoleGrantStore`, the shared `UnitOfWork`, `AuditLog`, `Outbox`, `PolicyGenBumper` and
`Authorize`. `AppState` gains one field, `retirement`, and `http::router` mounts the new router.

### 3.7 The orphan WARN gains a pointer

`bootstrap.rs`'s two orphan loops currently end at "…and `DeletePolicy` refuses to remove it" and
"existing grants of it still resolve". Both are amended to name the endpoint, so the log line
that reports the problem also states the remedy. No behavioural change to the scan itself.

## 4. Tests

### 4.1 Unit — the service against fakes (Docker-free, primary guard)

In `application/system_retirement.rs`, mirroring `policies.rs`'s existing service tests:

1. An unauthorized actor gets `Forbidden`, and nothing is read or written.
2. A still-code-defined id gets `SystemImmutable` — asserted for a role key *and* for
   `forbid-archived-writes`, and asserted to short-circuit before the store is touched.
3. An absent id gets `NotFound`.
4. A `system = false` row gets `NotSystemOwned`.
5. Surviving grants return `Blocked` carrying every grant, and the fakes prove **no delete, no
   event, no audit row, and no bump** happened.
6. The happy path deletes role then policy, enqueues exactly one `PolicyDeleted`, records
   exactly one audit entry, commits, and awaits exactly one bump.
7. A retired **static** policy (no role row) succeeds with `role_deleted: false`.
8. A store failure mid-chain rolls back: no event, no audit row, no bump.
9. The audit detail carries the destroyed `kind`/`source`/`description`, truncated and flagged
   for an oversized source (reusing SMA-477's cap).

### 4.2 Docker integration — `tests/authz_system_retirement_pg.rs` (new)

1. The FK ordering is real: deleting the `policy` row while the `role` row references it fails,
   and the service's order succeeds.
2. `lock_role_in` blocks a concurrent `role_grant` insert on a second connection until the
   retirement transaction commits or rolls back (D6) — the test that pins the lock's purpose.
3. `lock_timeout` bounds the wait rather than hanging when another transaction holds the row.
4. End-to-end: seed a system row-set at a non-code-defined id, grant it, retire → 409 with the
   grant listed; revoke; retire → 204; the row-set is gone.
5. After retirement the next `reconcile_starter` logs no orphan `WARN` for that id.

### 4.3 Existing suites

- `authz/roles.rs`'s `starter_policy_content_is_pinned_to_the_declared_revision` reds until
  `STARTER_POLICY_REVISION` and `EXPECTED_STARTER_CONTENT_HASH` are both updated — this is the
  guard doing its job, not a breakage.
- `action.rs`'s wire-string and `is_write`/`is_restore` tests extend to the new action.
- `schema.rs`'s validation tests cover the new action name.
- `pg_policies.rs`'s existing `SystemImmutable` tests must stay green: `put_in` / `delete_in` are
  not relaxed (D3).

## 5. Documentation

- `docs/ops/RUNBOOK-observability.md`: a retirement procedure — read the orphan `WARN`, call the
  endpoint, revoke the listed grants if it 409s, retry, confirm the `WARN` is gone on the next
  boot. Includes the explicit warning that retirement removes only inert rows, so a 409 is the
  system working correctly.
- The `SystemRetirementService` and endpoint module docs carry the D3/D4/D6 reasoning inline, in
  the style the surrounding modules already use.

## 6. Rollout, rollback, residual risk

**Rollout** is a single deploy. The action-catalog change converges every seeded database's
`forbid-archived-writes` row on first boot via SMA-477's existing path, at INFO. The endpoint is
inert until an operator calls it.

**Rollback** to a binary at `STARTER_POLICY_REVISION = 1` leaves the converged row alone (D11's
monotonicity guard), logging the documented `StaleBinary` INFO. Any retirement already performed
is not undone by a rollback — the rows are gone. This is inherent to a destructive operation and
is why it is operator-initiated and audited.

**Residual risks.**

- *A retired role with very many grants is tedious to clean up.* Accepted; bulk revocation is
  §7. The 409 body gives the exact id list.
- *An operator can retire an orphaned row that a future release intends to re-introduce at the
  same id.* Boot would simply re-seed it, so the outcome is correct; the audit row records what
  was removed in the meantime.
- *The endpoint is Root-only and destructive.* Mitigated by D7's guards (a live starter policy
  can never be retired), D4 (only inert rows are removed), and a full audit trail.

## 7. Out of scope / follow-ups

- **Bulk grant revocation** for a retired role — a `revoke_grants: true` cascade, or a
  `DELETE /v1/authz/role-grants?role_key=…` filter. Worth it only once a retirement with a large
  grant count actually happens.
- **A gRPC mirror.** Deliberately HTTP-only, following the dead-letter surface's scope decision
  (`dead_letters.rs:20-22`) — it keeps `contracts/` and the generated bindings untouched.
- **Quarantine / two-phase retirement** (a `policy.quarantined` column that stops the template
  compiling while the rows stay for inspection). It would stop a leak instantly without deleting
  anything, but needs a new column, a new reconciler state, and a state machine on a path whose
  appeal is having none.
- **Automatic retirement at boot**, whether unconditional or code-declared — the issue's stated
  non-goal, and D1's rejected alternative.
- **A metric for retirements.** One `iam_system_rows_retired_total` counter would be cheap, but
  the audit row is the durable record and retirement is a rare, deliberate act; adding it here
  would be speculative.
- **An alert on the existing orphan WARN** — belongs with the `ops/observability/` work, as
  SMA-477 §7 already recorded.

## 8. Acceptance criteria

1. `POST /v1/authz/system-policies/{id}/retire` with a non-Root actor returns `403` and touches
   no store.
2. Retiring a **code-defined** starter policy id — a role key or `forbid-archived-writes` —
   returns `409 system-immutable`, before any row is read.
3. Retiring an id with no `policy` row returns `404`; retiring a row with `system = false`
   returns `409 not-system-owned`.
4. Retiring an orphaned role while grants of it survive returns `409 grants-survive`, the body
   lists every surviving grant's id, principal PRN and scope PRN, and **no row is deleted, no
   event enqueued, no audit row written, and `policy_gen` is not bumped**.
5. With zero surviving grants, retirement deletes the `role` row and the `policy` row in one
   transaction, returns `204`, writes exactly one audit entry recording the destroyed content,
   enqueues exactly one `PolicyDeleted`, and awaits exactly one `policy_gen` bump.
6. Retiring an orphaned **static** system policy (no role row) succeeds with the same guarantees
   and reports `role_deleted: false`.
7. A concurrent `role_grant` insert against the retiring key blocks on the `FOR UPDATE` lock
   until the retirement transaction ends, and never produces a raw FK-violation error to the
   caller.
8. A failure at any point before commit leaves the `role` row, the `policy` row, the outbox and
   the audit log exactly as they were.
9. After a successful retirement, the next boot's orphan scan logs no `WARN` for that id.
10. `PutPolicy` and `DeletePolicy` still reject mutation of a persisted `system = true` row with
    `SystemImmutable` — the public guards are unchanged.
11. `starter_policy_content_is_pinned_to_the_declared_revision` passes with
    `STARTER_POLICY_REVISION = 2`, and a database seeded at revision 1 converges silently on the
    next boot.
