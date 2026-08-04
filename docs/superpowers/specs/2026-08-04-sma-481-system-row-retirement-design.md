# SMA-481 — A retired system role's policy and role rows are undeletable and keep granting

**Status:** design (revised after adversarial review)
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

`role_grant.linked_policy_id` carries no foreign key, so it offers no back door either.

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

**(a) A retired role can no longer be granted through the public API.** `RoleService::grant`
(`roles.rs:201`) resolves `authz_roles::role(role_key)` and returns `TenancyError::UnknownRole`
when the code catalog does not define it. The code catalog, not the `role` table, is the gate.
It is not the only insert path — `BootstrapAdminSeeder` calls `grant_in` directly, as does
`RoleGrantStore::grant`/`grant_in` — but the seeder is hardcoded to `platform_admin`, which D7
can never retire, so no reachable path grants a retired key on a current binary. D6 covers the
one path that remains.

**(b) `platform_admin`'s template carries no action list.** `template_source`
(`roles.rs:316-318`) returns `permit(principal == ?principal, action, resource in ?resource);`
for `platform_admin` — every action, unrestricted. A new action therefore needs **no role
template change at all**.

**(c) An operator-maintenance HTTP surface already exists.** SMA-469 added
`/v1/outbox/dead-letters/{id}/replay|discard` — Root-only, enforced inside the service rather
than the Cedar schema, HTTP-only with no gRPC mirror and no `contracts/` change
(`http/dead_letters.rs:19-22`). That is the precedent this design follows in full.

**(d) `EventType::PolicyDeleted` already exists.** `PolicyService::delete`
(`policies.rs:175`) emits it, with payload `{"policy_id"}` and
`aggregate_prn = "policy/{id}"`. Retirement needs no new event type, but does need a
discriminator in the payload (D9).

**(e) The Cedar schema's action list is hand-maintained.** `SCHEMA_SRC`
(`schema.rs:19-27`) enumerates every action by name. A new action must be added there or
`validate_policy` rejects the newly-generated `forbid-archived-writes` source.

**(f) Static policies compile unconditionally — they are *not* inert.** `PolicyEngine::compile`
(`engine.rs:76-79`) adds every `PolicyKind::Static` document to the `PolicySet` with no grant
involved; only the `Template` branch depends on a grant to have any effect. Its own doc comment
states the asymmetry: "a grant naming an absent template is silently skipped — it contributes no
permission, which is the fail-safe outcome." Deleting a retired **template** is therefore
provably inert. Deleting a retired **static** policy changes decisions fleet-wide. This drives
D4.

**(g) `classify_starter_policy` returns `Absent` before any revision check.**
(`reconcile.rs:181-184`) — the "nothing persisted, seed it" branch is check (1); the
`STARTER_POLICY_REVISION` monotonicity guard is check (2). `STARTER_POLICY_REVISION` therefore
protects *content convergence*, never *existence*. A replica whose code catalog still defines a
retired id will re-seed the deleted row unconditionally. This drives D11.

**(h) `RevokeRole` is a write action.** `Action::RevokeRole.is_write() == true` and
`is_restore() == false` (`action.rs:195`), so it is inside `forbid_archived_writes_source()`'s
generated action list, and that `forbid` fires for every principal including `platform_admin`.
A grant whose scope node is archived therefore cannot be revoked through the normal API. This
drives D12.

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
const, retired automatically once grants reach zero. It sits directly against the issue's stated
non-goal, and it makes a destructive act a side effect of a deploy. Deferred, not adopted.

### D2 — One endpoint retires the whole chain, keyed by the policy id

For a role template, `policy_id == Role::key == Role::template_id` (`roles.rs:285`), so a single
id names the policy row, the role row, and the grant key at once.

```
POST /v1/authz/system-policies/{id}/retire
```

A retired **static** starter policy takes the identical path and finds no role row — but it is
*not* treated identically, because retiring one is a decision change (D4).

`POST …/retire` rather than `DELETE /v1/authz/system-policies/{id}`: retirement is a guarded
operation that can legitimately refuse and that takes a request body (D4's acknowledgement), not
a plain delete. It matches `/v1/outbox/dead-letters/{id}/discard`, the destructive operator
action this design otherwise copies.

**Limitation, stated rather than hidden:** the "one id names all three" identity holds only for
rows this service wrote. A hand-inserted `role` row whose `template_id ≠ key` is unreachable
through this endpoint and will be `WARN`ed about on every boot forever. The runbook (§5) names
this as the one case still requiring direct database work, so an operator does not follow the
normal procedure into a dead end.

**Rejected alternative — two endpoints** (`/system-roles/{key}/retire` +
`/system-policies/{id}/retire`). The nouns match the tables more exactly, but it makes every
role retirement two ordered calls and introduces "role gone, policy still there" as a real,
reachable state whose only merit is being an artifact of the API shape.

### D3 — The destructive deletes never touch `PolicyStore::delete_in`

`delete_in`'s `SystemImmutable` guard is exactly what must keep holding for the public
`DeletePolicy` API, and SMA-477 D6 added a second layer (`put_in` rejects on the reserved *id*,
not merely on a stored row's `system` flag). Neither is relaxed here.

**Rejected alternative — relax `delete_in` for orphans**, i.e. refuse a system row only while
`is_starter_policy_id(id)` holds. It needs no new action and no catalog churn, which is
genuinely attractive. But nothing can then delete the `role` row, so `fk_role_template` blocks
the policy delete for every retired *role* — leaving it useful only for retired static policies,
which is the rarer half of the problem.

### D4 — Retiring a template is inert; retiring a static policy is a decision change, and says so

The first draft of this design claimed retirement "only ever removes rows that are provably
inert, so it cannot change who can do what." Per §1.4(f) that is true of templates and **false
of static policies**. The claim is split rather than patched:

**Templates (every system role).** Retirement never deletes a grant. When grants of the retired
key survive, the endpoint refuses and names them; the operator revokes each through the existing
`DELETE /v1/authz/role-grants/{id}`, then retries. This is not timidity about blast radius — it
is what each path produces. A revocation through `RoleService::revoke` gets its own audit row,
its own `DomainEvent`, and the anti-escalation check at that grant's own scope; a bulk cascade
inside retirement would have to reproduce all three faithfully or become precisely the "silently
dropping grants is an authorization change" the issue warns against. With zero grants linked, a
template contributes nothing to the `PolicySet`, so its removal provably cannot change a
decision.

**Static policies.** Deleting one removes a policy that was being evaluated on every request.
For the only static starter policy that exists today, `forbid-archived-writes`, that means
archived resources become writable fleet-wide. The endpoint therefore refuses a static
retirement unless the caller explicitly acknowledges it:

```json
POST /v1/authz/system-policies/{id}/retire
{ "acknowledge_decision_change": true }
```

Without the flag it returns `409 decision-change-unacknowledged` **and the body carries the
`kind`, `source` and `description` that would be destroyed**. That refusal doubles as the
preview: an operator sees exactly what they are removing before they remove it, rather than
afterwards in an audit table nobody watches. The flag is ignored (not rejected) for a template,
so the operator never has to know which kind they are dealing with in advance.

### D5 — A blocked retirement is an outcome, not an error — and cannot be mistaken for success

The 409 must carry the surviving grants, and `TenancyError` is a flat, `Clone + PartialEq` enum
shared by every service with a `code()` mapping under a stability test. So
`SystemRetirementService::retire` returns `Result<RetireOutcome, TenancyError>`:

```rust
#[must_use]
pub enum RetireOutcome {
    /// The chain was removed. `role_deleted` is false for a retired static policy.
    Retired { policy_id: String, kind: PolicyKind, role_deleted: bool },
    /// Nothing was written. The listed grants must be revoked first.
    Blocked { role_key: String, grants: Vec<GrantRef>, total_surviving: u64, truncated: bool },
    /// Nothing was written. A static policy needs `acknowledge_decision_change` (D4).
    NeedsAcknowledgement { policy_id: String, content: PolicyContent },
}
```

`#[must_use]` plus an `is_retired()` helper is the guard against the real hazard the adversarial
review identified: a future caller writing `svc.retire(..).await?;` and discarding the value
would otherwise treat a refusal as success.

**The grant list is capped.** `list_by_role_key_in` selects `LIMIT 101` and the response reports
`total_surviving` and `truncated`, matching every other list surface in this service. An
unbounded `Vec<RoleGrant>` loaded inside a transaction and serialised whole is a denial of
service against the operator's own tooling.

**The wire codes are registered.** `grants-survive`, `decision-change-unacknowledged` and
`not-system-owned` are added to the same `code()` registry and stability test as every other
error code, so the code set stays enumerable even though two of them are produced by the handler
rather than by `TenancyError`.

### D6 — The role row is locked `FOR UPDATE`, and the resulting FK failure is mapped, not raw

Per §1.4(a) the public API cannot grant a retired role on a current binary. One path remains,
and it is not hypothetical: **a replica running an older binary mid-deploy still has the role in
its code catalog** and will happily grant it — a rollback, a crashloop restart, an HPA scale-up,
a held canary. Same fleet-skew scenario `STARTER_POLICY_REVISION` exists for (SMA-477 D11).

`SELECT … FROM role WHERE key = $1 FOR UPDATE` blocks such an insert: PostgreSQL's FK check
takes a `FOR KEY SHARE` lock on the parent row, which conflicts with `FOR UPDATE`.

**What the lock does not do — corrected from the first draft.** It defers the conflict; it does
not remove it. When the retirement commits with the role row deleted, the blocked `INSERT`
resumes, re-runs its FK check, finds no parent, and raises `fk_role_grant_role`. The failure
moves from the *retiring* caller to the *granting* caller. `PgRoleGrantStore::grant_in` maps
every `DbErr` to `AuthzError::Backend` → `TenancyError::Internal` → a `500 internal error`,
which tells that caller nothing.

So the fix is two-part: take the lock (so the retirement's own transaction is never the one that
fails), **and** map `fk_role_grant_role` through the existing constraint-name mapping in
`pg_repository::conflict_kind` so the granting caller receives `UnknownRole` — which is exactly
what happened, and exactly what a current binary would have returned before reaching the
database at all.

### D7 — Retiring a still-code-defined id is refused, and it is the load-bearing guard

`is_starter_policy_id(id)` is checked before anything is read. Retiring a live starter policy
would be re-seeded by `reconcile_starter` at the next boot anyway — but in the window between
the two, that policy is not governing decisions. For `forbid-archived-writes` that means archived
resources become writable; for a role template it means every grant of that role silently stops
working.

Reused as `TenancyError::SystemImmutable(id)` — which is `ErrorClass::Precondition`
(`error.rs:148`), not `Conflict` as the first draft stated. Both map to `409` today, but the
class is the thing a future gRPC mirror would translate, so the new `NotSystemOwned(String)` is
classified `Precondition` too rather than letting two sibling refusals on one endpoint diverge.

Two rows are guarded, not one. A `policy` row with `system = false` is refused — retirement must
not become a second, differently-audited delete path for operator-authored policies. A `role`
row with `system = false` at the same id is refused for the identical reason, which the first
draft omitted.

### D8 — One new action, Root-only, enforced in the service

`Action::RetireSystemPolicy`. Root-only-ness lives in `SystemRetirementService`, which always
authorizes at `root_prn()` — the pattern `DeadLetterService`, `AuditQueryService::list` and
`PolicyService::list` all use, rather than encoding it in the Cedar schema's shared `appliesTo`
block (`dead_letters.rs:5-8`).

It is `is_write() == true` and `is_restore() == false`, so it enters the generated
`forbid_archived_writes_source()` action list. That changes starter policy content, requiring
`STARTER_POLICY_REVISION` 1→2 and a new `EXPECTED_STARTER_CONTENT_HASH` — the exact scenario
SMA-477 was built to absorb, converging silently on the next boot of every seeded database.

No role template gains the action: `platform_admin` is unrestricted (§1.4(b)), and no
lower-privilege role should be able to retire a system row.

### D9 — Retirement audits what it destroyed, and its event says what it was

One audit entry, `action = Action::RetireSystemPolicy.as_wire()` (built from the enum, matching
`DeadLetterService::audit_entry`, never a hardcoded string), `resource_prn = root_prn()`, actor
from the bearer-resolved `AuthContext`. Its detail records the destroyed `kind`, `source` and
`description` — retirement removes the evidence, which is precisely the case SMA-477's
`truncate_audited_text` / `MAX_AUDITED_SOURCE_BYTES` cap was written for. That helper is
currently a private `fn` in `application/bootstrap.rs`; it becomes `pub(crate)` and is reused
rather than re-derived.

One `DomainEvent` with the existing `EventType::PolicyDeleted`, payload extended to
`{"policy_id", "reason": "system_retirement", "role_deleted": bool}` so a consumer can
distinguish an operator policy delete from a system-chain retirement and learn that a `role` row
went with it. No new event type, and no separate role event — no role-catalog consumer exists.

The event and the audit entry share ONE freshly-minted `correlation_id`, the repo-wide
convention with dedicated tests in `roles.rs` and `policies.rs`. Both ride the mutation's single
`UnitOfWork` transaction, so a mid-transaction failure leaves none of the three.

### D10 — `policy_gen` is bumped post-commit and awaited; one metric records the act

The bump is exactly `PolicyService::delete`'s: after the commit, awaited, so the change is
visible to the next `is_authorized` rather than landing on the snapshot's TTL backstop. A bump
failure is swallowed and logged by the `PolicyGenBumper` implementation — the deletion already
committed, and a Redis blip must never fail an already-successful write.

`iam_system_rows_retired_total{outcome}` counts `retired` / `blocked` / `refused`. The first
draft deferred this as speculative on the grounds that "the audit row is the durable record".
That was wrong, and SMA-477 says why in its own words: the metric label is the whole remediation
path, because a log line is "one line in a log nobody is watching" (`bootstrap.rs:665-670`).
Nothing alerts on `audit_log`. This is a destructive, Root-only, irreversible operation and it
is the least speculative place in the service for a counter.

### D11 — Retirement requires a converged fleet; the endpoint proves what it can and the rest is a precondition

Per §1.4(g), a replica whose code catalog still defines a retired id re-seeds the deleted rows
unconditionally — `Absent` is classified before the revision guard runs. So a retirement
performed mid-rollout, or followed by a rollback, is silently undone.

**No in-band mechanism can fully close this.** A tombstone row honoured by
`classify_starter_policy` was considered and rejected: a binary old enough to still define the
id is also old enough to predate the tombstone check. Any guard only binds binaries that already
carry it.

What is achievable is honest, and is three things:

1. **A stated precondition.** Retirement is only safe once every replica is on a binary that no
   longer defines the id. §5's runbook states this first, before the procedure.
2. **The strongest in-band evidence available.** The endpoint refuses unless every *remaining*
   starter policy row carries `starter_revision >= STARTER_POLICY_REVISION` — proof that the
   last writer of each row was a binary at least as new as this one. It does not prove no older
   replica is running, and the spec must not pretend it does; it does catch the common case of
   retiring during a half-finished rollout. Refused as `409 fleet-not-converged`.
3. **A loud, recoverable failure mode.** A re-seeded row is detected: the next boot's orphan
   scan `WARN`s again and the metric increments. Retirement is idempotent-retryable, so the
   operator simply repeats it once the fleet has converged. Nothing is corrupted — the cost is a
   window in which a retired template is compilable again.

§6 and AC9 are written against this reality rather than against the first draft's claim that
"rollout is a single deploy".

### D12 — A grant at an archived scope cannot be revoked; the endpoint says so instead of looping

Per §1.4(h), `RevokeRole` is inside `forbid-archived-writes`, and that forbid fires on any
resource whose `effective_status` is `archived` — for `platform_admin` too. So D4's escape hatch
("revoke each grant, then retry") is unavailable for a grant whose scope node is an archived
org, team or project, and retirement of that role could never succeed. Root-scoped grants are
unaffected: the synthetic `Root` entity carries no `effective_status`.

Retirement does **not** delete these grants — that would breach D4's template guarantee for the
convenience of one case. Instead the `Blocked` response flags each listed grant with
`scope_archived: true`, and both the response message and §5's runbook name the supported
sequence: **restore the node → revoke the grant → re-archive the node**. An operator who is not
told this hits a refusal loop with no exit, which is worse than the original bug.

## 3. The fix

### 3.1 Action catalog — `paigasus-iam-core`

- `authz/action.rs`: add `Action::RetireSystemPolicy` to the enum, to `Action::ALL`, to
  `as_wire()`, and to the `is_write()` write arm. The `Action::ALL.len()` assertion
  (`action.rs:290`) goes 39→40 and its explanatory message gains a clause.
- `authz/schema.rs`: add `RetireSystemPolicy` to `SCHEMA_SRC`'s action list (§1.4(e)) — without
  this, `validate_policy` rejects the newly-generated `forbid-archived-writes` source and boot
  fails.
- `authz/roles.rs`: `STARTER_POLICY_REVISION` 1→2, `EXPECTED_STARTER_CONTENT_HASH` updated. No
  `*_ACTIONS` const changes.

### 3.2 One new port — `SystemRowRetirer`

The first draft spread five methods across `PolicyStore`, `RoleGrantStore`,
`SystemPolicyReconciler` and `SystemRoleReconciler` while citing SMA-477 D5. That citation was
self-contradicting: D5's stated rationale is that `PolicyStore` "has seven implementations, six
of which are test fakes on the request path that would gain a method nothing calls"
(`ports.rs:102`) — and `RoleGrantStore` has seven too. Adding methods to either forces fourteen
`unimplemented!()` stubs, the exact cost D5 rejected.

D5 applied properly means one narrow, purpose-built port with one production implementation and
one fake:

```rust
#[async_trait]
pub trait SystemRowRetirer: Send + Sync {
    /// Opens the retirement transaction with `SET LOCAL lock_timeout` already applied.
    /// A dedicated constructor because the `Transaction` port exposes no way to set it
    /// after the fact, and `reconcile_system` can only do so by owning its own `begin`.
    async fn begin_retirement(&self, lock_timeout: Duration) -> Result<Box<dyn Transaction>, AuthzError>;

    /// Reads the `policy` row FOR UPDATE — locked, not a plain read, so nothing can
    /// insert an FK child against it between the checks and the delete.
    async fn lock_policy_in(&self, tx: &dyn Transaction, policy_id: &str) -> Result<Option<StoredPolicy>, AuthzError>;

    /// Locks `key`'s `role` row FOR UPDATE, returning it if present (D6).
    async fn lock_role_in(&self, tx: &dyn Transaction, key: &str) -> Result<Option<StoredRole>, AuthzError>;

    /// Up to `limit + 1` surviving grants of `role_key`, ordered by id, plus the true
    /// total — capped per D5. Each carries whether its scope node is archived (D12).
    async fn surviving_grants_in(&self, tx: &dyn Transaction, role_key: &str, limit: u64) -> Result<SurvivingGrants, AuthzError>;

    /// Proof-of-convergence check for D11: the minimum `starter_revision` across all
    /// remaining system-owned rows, or `None` if any is NULL.
    async fn min_starter_revision(&self) -> Result<Option<u32>, AuthzError>;

    async fn delete_role_in(&self, tx: &dyn Transaction, key: &str) -> Result<bool, AuthzError>;
    async fn delete_policy_in(&self, tx: &dyn Transaction, policy_id: &str) -> Result<bool, AuthzError>;
}
```

Implemented once by a new `PgSystemRowRetirer` adapter over `DatabaseConnection`, plus one
in-memory fake for the service tests. The public `PolicyStore` / `RoleGrantStore` /
`SystemPolicyReconciler` / `SystemRoleReconciler` ports are untouched, and `delete_in`'s
`SystemImmutable` guard is neither relaxed nor bypassed by anything the public API can reach
(D3).

**Both locks matter.** `lock_policy_in` must be `FOR UPDATE`, not a plain read: an older
replica's `reconcile_role` INSERT takes `FOR KEY SHARE` on the `policy` parent via
`fk_role_template`, and if `lock_role_in` found no role row to lock, nothing else would block it
— `delete_policy_in` would then fail on `fk_role_template` with an unmapped `Backend` error.
`lock_policy_in` is taken first, so `begin_retirement`'s `lock_timeout` covers both.

### 3.3 Application — `application/system_retirement.rs` (new)

A dedicated service rather than a method on `PolicyService`: retirement holds a port
`PolicyService` does not, and `policies.rs` already carries the full CRUD surface. Modelled
field-for-field on `DeadLetterService` — a `SystemRetirementDeps` bag, `Arc`-held ports,
`Authorize`, Root-only enforcement inside the service.

```rust
pub async fn retire(&self, actor: &Prn, id: &str, ack: bool) -> Result<RetireOutcome, TenancyError> {
    // 1. Root-only.
    self.authorize.check(actor, Action::RetireSystemPolicy, &root_prn()).await?;

    // 2. Still code-defined → refuse before reading anything (D7).
    if authz_roles::is_starter_policy_id(id) {
        return Err(TenancyError::SystemImmutable(id.to_string()));
    }

    // 3. The fleet must have converged past the release that dropped this id (D11).
    if self.retirer.min_starter_revision().await?.is_none_or(|r| r < STARTER_POLICY_REVISION) {
        return Err(TenancyError::FleetNotConverged);
    }

    let tx = self.retirer.begin_retirement(LOCK_TIMEOUT).await?;

    // 4. Lock the policy row FIRST — it is the FK parent of the role row (§3.2).
    let Some(policy) = self.retirer.lock_policy_in(&*tx, id).await? else { return Err(TenancyError::NotFound) };
    if !policy.system { return Err(TenancyError::NotSystemOwned(id.to_string())); }

    // 5. Lock the role row before any grant is counted (D6). A non-system role row at a
    //    system policy's id is refused for D7's reason, not silently deleted.
    let role = self.retirer.lock_role_in(&*tx, id).await?;
    if role.as_ref().is_some_and(|r| !r.system) { return Err(TenancyError::NotSystemOwned(id.to_string())); }

    // 6. Survivors block the retirement; nothing is written (D4/D5/D12). Read on the SAME
    //    transaction that holds the locks above.
    if role.is_some() {
        let survivors = self.retirer.surviving_grants_in(&*tx, id, GRANT_LIST_CAP).await?;
        if survivors.total > 0 {
            return Ok(RetireOutcome::Blocked { /* … incl. scope_archived per grant */ });
        }
    }

    // 7. A static policy is a decision change and needs explicit acknowledgement (D4).
    //    The refusal carries the content, so it doubles as the preview.
    if policy.kind == PolicyKind::Static && !ack {
        return Ok(RetireOutcome::NeedsAcknowledgement { policy_id: id.into(), content: policy.content() });
    }

    // 8/9. role → policy, the only order the FKs permit (§1.2). Both deletes are asserted:
    //      the rows are locked and were observed present, so a `false` here is a data-integrity
    //      break and surfaces as Backend, never a silent `role_deleted: false`.
    let role_deleted = match &role {
        Some(_) => { require_deleted(self.retirer.delete_role_in(&*tx, id).await?, "role", id)?; true }
        None => false,
    };
    require_deleted(self.retirer.delete_policy_in(&*tx, id).await?, "policy", id)?;

    // 10. Audit + event share the transaction and ONE correlation_id (D9).
    self.outbox.enqueue(&*tx, &event).await?;
    self.audit.record(&*tx, &entry).await?;
    tx.commit().await?;

    // 11. Awaited, post-commit (D10).
    self.gen_bumper.bump().await;
    Ok(RetireOutcome::Retired { policy_id: id.into(), kind: policy.kind, role_deleted })
}
```

Dropping `tx` without committing rolls back, so every early return is safe — the
`DeadLetterService::replay` posture. Steps 1–3 precede `begin_retirement`, so an unauthorized or
obviously-invalid request never opens a transaction or takes a lock.

**Repeated retirement returns `404`.** This deliberately differs from `delete_in` / `revoke_in`'s
idempotent-`false` posture: those are DELETEs of caller-owned resources where a vanished row is a
benign race, whereas a second retire of the same id means the operator's model of the system is
wrong and they should be told, not silently congratulated. Stated here because the divergence is
otherwise a reviewer's reasonable objection.

### 3.4 HTTP — `adapters/http/system_retirement.rs` (new)

One route, its own file, mirroring `http/dead_letters.rs`. `http/authz.rs` — which also carries
the `is_authorized` hot path — is left untouched.

```rust
Router::new().route("/v1/authz/system-policies/{id}/retire", post(retire))
```

Mounted on the bearer-gated `protected` sub-router; the actor comes from the auth middleware's
`AuthContext`, never a client-supplied value. `http/mod.rs`'s
`protected_router_merge_has_no_path_conflicts` test (`mod.rs:816`) reproduces the merge chain
and gains the new router.

The optional body is `{"acknowledge_decision_change": bool}`, defaulting to `false` when absent
or when the body is empty. The handler returns `Result<Response, ApiError>`:

- `Retired` → `200 OK` with `{"policy_id", "kind", "role_deleted"}`. **Not `204`** — the first
  draft's 204 contradicted its own acceptance criterion that a static retirement "reports
  `role_deleted: false`", and a body is the operator's only immediate record of what was
  destroyed.
- `Blocked` → `409` with code `grants-survive` and
  `{"grants": [{"id", "principal_prn", "scope_prn", "scope_archived"}], "total_surviving", "truncated"}`.
- `NeedsAcknowledgement` → `409` with code `decision-change-unacknowledged` and
  `{"kind", "source", "description"}`.

All three keep the stable `{"error": {"code", "message"}}` envelope and add sibling fields.

### 3.5 Error type — `application/error.rs`

Two new variants: `NotSystemOwned(String)` and `FleetNotConverged`, both
`ErrorClass::Precondition` (D7), with codes `not-system-owned` and `fleet-not-converged` added to
`code()` and its stability test. `SystemImmutable` and `NotFound` are reused as-is. The two
handler-produced codes are registered alongside them (D5).

`pg_repository::conflict_kind` gains a `fk_role_grant_role` mapping so a concurrent grant that
loses the D6 race surfaces as `UnknownRole`, not `Internal`.

### 3.6 Composition root

`AppState::new` builds `PgSystemRowRetirer` and `SystemRetirementService` from it plus the
shared `AuditLog`, `Outbox`, `PolicyGenBumper` and `Authorize`. `AppState` gains one field,
`retirement`, and `http::router` mounts the new router.

### 3.7 The orphan WARN gains a pointer

`bootstrap.rs`'s two orphan loops currently end at "…and `DeletePolicy` refuses to remove it" and
"existing grants of it still resolve". Both are amended to name the endpoint. No behavioural
change to the scan itself.

## 4. Tests

### 4.1 Unit — the service against fakes (Docker-free, primary guard)

In `application/system_retirement.rs`, mirroring `policies.rs`'s existing service tests:

1. An unauthorized actor gets `Forbidden`, and no retirer port is called.
2. A still-code-defined id gets `SystemImmutable` — asserted for a role key *and* for
   `forbid-archived-writes`, and asserted to short-circuit before any transaction opens.
3. An unconverged fleet gets `FleetNotConverged`, asserted for both a low revision and a `NULL`.
4. An absent id gets `NotFound`; a `system = false` **policy** row and a `system = false`
   **role** row each get `NotSystemOwned`.
5. Surviving grants return `Blocked` with every grant, the true total, the `truncated` flag past
   the cap, and `scope_archived` set — and the fakes prove **no delete, no event, no audit row,
   no bump**.
6. A static policy without the flag returns `NeedsAcknowledgement` carrying the content, writes
   nothing, and succeeds when the flag is set. The flag is a no-op on a template.
7. The template happy path deletes role then policy, enqueues exactly one `PolicyDeleted` whose
   payload carries `reason: "system_retirement"` and `role_deleted: true`, records exactly one
   audit entry **sharing the event's `correlation_id`**, commits, and awaits exactly one bump.
8. A store failure mid-chain rolls back: no event, no audit row, no bump.
9. The audit detail carries the destroyed `kind`/`source`/`description`, truncated and flagged
   for an oversized source (reusing SMA-477's cap).
10. A `delete_*_in` returning `false` under a held lock surfaces as `Backend`, never as a silent
    `role_deleted: false`.
11. `iam_system_rows_retired_total` increments with the right `outcome` label on each of
    retired / blocked / refused.

### 4.2 Unit — the catalog change

Pinned assertions rather than a re-derivation: `forbid_archived_writes_source()` **contains**
`Pgs::Iam::Action::"RetireSystemPolicy"`, `Action::ALL.len() == 40`, the wire string round-trips,
`is_write()` is true and `is_restore()` false, and `SCHEMA_SRC` names the action so
`validate_policy` accepts the generated source.

### 4.3 Docker integration — `tests/authz_system_retirement_pg.rs` (new)

1. **The bug is actually fixed.** Seed a system template + role + grant at a non-code-defined id;
   assert `PolicyEngine::compile` links the grant and a `decide` call allows; retire; assert the
   same request is now denied. This is the test the first draft was missing entirely — everything
   else checks rows, this checks *decisions*.
2. The FK ordering is real: deleting the `policy` row while the `role` row references it fails,
   and the service's order succeeds.
3. `lock_role_in` blocks a concurrent `role_grant` insert until the retirement transaction ends,
   **and** the granting caller then receives a mapped `UnknownRole`, not a 500 (D6). Asserting
   only the blocking — as the first draft did — would go green against the unmapped bug.
4. `lock_policy_in` blocks a concurrent `role` insert against `fk_role_template`.
5. `lock_timeout` bounds the wait rather than hanging when another transaction holds a row.
6. **Fleet skew (D11).** Retire, then run `reconcile_starter` with the id still present in a
   simulated code catalog, and assert the specified behaviour: the row is re-seeded and the
   subsequent orphan scan `WARN`s again. Pinning the *known* failure mode is what stops it being
   discovered in production. (Replaces the first draft's near-tautological "no orphan WARN after
   retirement" — `orphaned_system_policy_ids` scans persisted rows, so a deleted row is trivially
   absent and the assertion proved nothing beyond the test above it.)
7. Repeat retirement of the same id returns `404` and writes nothing.
8. End-to-end: grant → retire → `409` listing the grant → revoke → retire → `200`, row-set gone.

Fixtures seed the non-code-defined row-set by direct SeaORM insert — no supported path writes a
`role` row for a key the code catalog does not define, which is the whole premise of the issue.

### 4.4 Existing suites

- `authz/roles.rs`'s `starter_policy_content_is_pinned_to_the_declared_revision` reds until
  `STARTER_POLICY_REVISION` and `EXPECTED_STARTER_CONTENT_HASH` are both updated — the guard
  doing its job, not a breakage.
- `error.rs`'s code-stability test extends to the new variants and codes.
- `pg_policies.rs`'s existing `SystemImmutable` tests must stay green: `put_in` / `delete_in` are
  not relaxed (D3).

## 5. Documentation

`docs/ops/RUNBOOK-observability.md` gains a retirement procedure, in this order:

1. **The precondition, first:** every replica must be on a binary that no longer defines the id
   (D11). Retiring mid-rollout is silently undone.
2. Read the orphan `WARN`; call the endpoint.
3. On `409 grants-survive`, revoke each listed grant. **If a grant is flagged
   `scope_archived`, restore the node → revoke → re-archive** (D12) — without this the operator
   loops forever.
4. On `409 decision-change-unacknowledged`, read the returned content, decide, re-call with the
   flag (D4).
5. On `409 fleet-not-converged`, wait for the rollout to finish.
6. Confirm the `WARN` is gone on the next boot; if it returns, the fleet had not converged —
   repeat.
7. The one case this procedure does not cover: a hand-inserted `role` row whose `template_id ≠
   key` (D2), which still needs direct database work.

`SystemRetirementService` and the endpoint module docs carry the D4/D6/D11/D12 reasoning inline,
in the style the surrounding modules already use.

## 6. Rollout, rollback, residual risk

**Rollout.** The action-catalog change converges every seeded database's `forbid-archived-writes`
row on first boot via SMA-477's existing path, at INFO. The endpoint is inert until called.

**Rollback** to a binary at `STARTER_POLICY_REVISION = 1` leaves the converged row alone (D11's
monotonicity guard), logging the documented `StaleBinary` INFO. That row's source names
`Pgs::Iam::Action::"RetireSystemPolicy"`, which the rolled-back binary's `SCHEMA_SRC` does not
define — this is safe because `Policy::parse` does not schema-validate and Cedar does not
validate the policy set at decision time; an unknown action entity simply never matches. This is
the same posture SMA-477 already shipped for its own revision bump, not a new exposure.

**A retirement performed before the fleet converged is undone**, not corrupted — see D11. §5's
step 6 makes the recovery explicit.

**Residual risks.**

- *A retired role with very many grants is tedious to clean up.* Accepted; bulk revocation is
  §7. The capped 409 body gives the operator the first 100 ids and the true total.
- *An operator can retire an orphaned row that a future release re-introduces at the same id.*
  Boot re-seeds it, so the outcome is correct; the audit row records the interim.
- *The endpoint is Root-only and destructive.* Mitigated by D7's guards, D4's template
  guarantee and static-policy acknowledgement, D11's convergence check, and a full audit trail.

## 7. Out of scope / follow-ups

- **Bulk grant revocation** for a retired role. Worth it only once a retirement with a large
  grant count actually happens; the capped 409 list is the interim.
- **`GET /v1/authz/system-policies/orphans`.** A natural home for the boot `WARN`'s remedy
  pointer and a standalone preview — but D4's `NeedsAcknowledgement` refusal already returns the
  content before anything is destroyed, which was the actual need.
- **A gRPC mirror.** Deliberately HTTP-only, following the dead-letter surface's scope decision
  (`dead_letters.rs:20-22`) — it keeps `contracts/` and the generated bindings untouched.
- **Quarantine / two-phase retirement** (a `policy.quarantined` column stopping the template
  compiling while rows stay for inspection). It is reversible and fails safe under fleet skew,
  which is a real advantage over this design. It is deferred rather than dismissed: it needs a
  column, a new `classify_starter_policy` state and a state machine, and — decisively — it does
  not actually remove the rows, which is what the issue asks for. Revisit if D11's precondition
  proves unworkable in practice.
- **Skipping non-code-defined system policies at compile time** — roughly three lines in
  `PolicyEngine::compile` or `list_all`, no new action, no revision bump, no ports, no endpoint.
  It fixes the *security* half of §1 outright and is by far the cheapest option, which is why it
  is recorded here rather than passed over. It is not adopted because it leaves every row in
  place (the issue's actual ask is removal), and because it silently drops retired **static**
  policies fleet-wide with no operator involvement at all — §1.4(f)'s asymmetry, in its worst
  form. Worth reconsidering as a defence-in-depth companion, not a replacement.
- **Automatic retirement at boot**, whether unconditional or code-declared — the issue's stated
  non-goal, and D1's rejected alternative.
- **An alert on the existing orphan WARN** — belongs with the `ops/observability/` work, as
  SMA-477 §7 already recorded.

## 8. Acceptance criteria

1. `POST /v1/authz/system-policies/{id}/retire` with a non-Root actor returns `403` and calls no
   retirer port (the `Authorize` check itself reads an entity slice; nothing else is touched).
2. Retiring a **code-defined** starter policy id — a role key or `forbid-archived-writes` —
   returns `409 system-immutable` before any row is read or transaction opened.
3. Retiring when any remaining system-owned row's `starter_revision` is below this binary's, or
   is `NULL`, returns `409 fleet-not-converged` and writes nothing.
4. Retiring an id with no `policy` row returns `404`; a `policy` or `role` row with
   `system = false` returns `409 not-system-owned`.
5. Retiring an orphaned role while grants survive returns `409 grants-survive`; the body lists up
   to 100 grants with id, principal PRN, scope PRN and `scope_archived`, plus the true
   `total_surviving` and a `truncated` flag — and **no row is deleted, no event enqueued, no
   audit row written, `policy_gen` not bumped**.
6. Retiring an orphaned **static** policy without `acknowledge_decision_change` returns
   `409 decision-change-unacknowledged` carrying the `kind`/`source`/`description` that would be
   destroyed, and writes nothing; with the flag it succeeds and reports `role_deleted: false`.
7. With zero surviving grants, retiring a template deletes the `role` row and the `policy` row in
   one transaction, returns `200` with `{"policy_id", "kind", "role_deleted": true}`, writes
   exactly one audit entry recording the destroyed content, enqueues exactly one `PolicyDeleted`
   carrying `reason: "system_retirement"` **and sharing the audit entry's `correlation_id`**, and
   awaits exactly one `policy_gen` bump.
8. **A grant of the retired role that allowed a request before retirement is denied after it** —
   asserted through `compile` + a real decision, not by inspecting rows.
9. A concurrent grant of the retiring key blocks on the `FOR UPDATE` lock; the retirement
   transaction itself never fails with an FK violation, and once it commits the granting caller
   receives a mapped `UnknownRole`, never a `500`.
10. A failure at any point before commit leaves the `role` row, the `policy` row, the outbox and
    the audit log exactly as they were; a repeated retirement returns `404` and writes nothing.
11. Running `reconcile_starter` from a binary whose catalog still defines a retired id re-seeds
    the rows and the orphan scan `WARN`s again — the documented D11 failure mode, pinned by a
    test rather than discovered in production.
12. `PutPolicy` and `DeletePolicy` still reject mutation of a persisted `system = true` row with
    `SystemImmutable` — the public guards are unchanged.
13. `starter_policy_content_is_pinned_to_the_declared_revision` passes with
    `STARTER_POLICY_REVISION = 2`, `forbid_archived_writes_source()` contains
    `RetireSystemPolicy`, and a database seeded at revision 1 converges silently on the next boot.
