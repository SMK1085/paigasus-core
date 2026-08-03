# SMA-477 — Starter policies reconcile by compare-and-warn, so any action-catalog change drifts forever

**Status:** design
**Date:** 2026-08-03
**Issue:** [SMA-477](https://linear.app/smaschek/issue/SMA-477/iam-starter-policies-reconcile-by-compare-and-warn-so-any-action)
**Project:** Paigasus IAM — Hardening
**Surfaced by:** the [SMA-469](https://linear.app/smaschek/issue/SMA-469/iam-outbox-retention-a-real-dead-letter-path-for-parked-events) adversarial spec review

## 1. Problem

`bootstrap::reconcile_starter` (`rs/crates/services/paigasus-iam/src/application/bootstrap.rs:74-93`)
reconciles `authz::roles::starter_policies()` against what is persisted by **compare-and-warn**.
Its match has three arms, and only one of them writes:

```rust
match current.iter().find(|d| d.policy_id == doc.policy_id) {
    None => policies.put(&doc).await?,
    Some(existing) if existing.source != doc.source => {
        tracing::warn!(policy_id = %doc.policy_id,
            "starter policy drift: the stored source differs from the code-defined source; not overwriting a system-owned row");
    }
    Some(_) => {}
}
```

The stored row is never updated. Not overwriting is the right instinct — it protects an
operator's deliberate edit from being clobbered — but there is no path by which a *legitimate
code change* ever reconciles, so the warning is permanent.

### 1.1 Why this is not hypothetical

`forbid_archived_writes_source()` (`rs/crates/libs/paigasus-iam-core/src/authz/roles.rs:262-266`)
generates its action list from the catalog rather than hand-maintaining it, precisely so it can
never fall out of sync:

```rust
let write_actions = Action::ALL.iter().copied().filter(|a| a.is_write() && !a.is_restore()).collect::<Vec<_>>();
```

So **every future addition of a write action changes that generated policy source**, and every
database seeded before that change logs the drift warning at every boot, forever. SMA-469 added
two (`ReplayOutboxDeadLetter`, `DiscardOutboxDeadLetter`) and is the first to trigger it;
nothing about the problem is specific to that issue.

### 1.2 Why it matters

A permanent boot-time `WARN` that operators learn to ignore destroys the signal's ability to do
the one job it exists for: telling you that someone has tampered with a system-owned policy.
The alert and the false positive are indistinguishable. Worse, the *stored* (stale) policy keeps
governing decisions — so a starter `forbid` that the current code intends to apply silently does
not.

### 1.3 What reading the code found beyond the issue

Four findings materially shaped the design. All verified against `main` at `36c27f5`.

**(a) There is no code path that can update a seeded starter policy.**
`PgPolicyStore::put_in` (`pg_policies.rs:209-213`) and `delete_in` (`pg_policies.rs:295-297`)
both return `AuthzError::SystemImmutable` for *any* mutation of an already-persisted
`system = true` row. So "not overwriting" is not merely a choice `reconcile_starter` makes — it
is the only thing it *can* do through the port it holds. A self-healing fix needs a new store
capability, not just a changed `if`.

**(b) Therefore "an operator edited the row" can only mean direct SQL.** There is no API path to
a system-owned policy. This is what makes it defensible to treat these rows as code-owned rather
than negotiating with whatever is in the database (D1).

**(c) A mixed-version rolling deploy is safe.** `PolicyEngine::compile`
(`rs/crates/libs/paigasus-iam-core/src/authz/engine.rs:74-86`) uses `Policy::parse` /
`Template::parse` — parse only, **no schema validation** (`authz::schema::validate_policy` is a
separate function, called only from `put_in`). An old replica that reloads a newer policy source
referencing an `Action` its binary does not know therefore parses it fine; the clause simply
never matches. Self-healing cannot break a rolling deploy by poisoning an old replica's snapshot.

**(d) `PolicyStore` has seven implementations** — one real (`PgPolicyStore`) and six test fakes
(`cedar_authorizer.rs:384`, `policy_snapshot.rs:530` and `:603`, `policies.rs:291` and `:328`,
`fakes.rs:599`). Adding a boot-only method to that trait would force all seven to change for a
capability no request-path consumer uses (D5).

**(e) The same defect exists one function down.** `seed_role_row` (`bootstrap.rs:49-66`) inserts
a `role` row when absent and never updates it, so a code change to a role's `description` or
`scope_kinds` drifts forever too — and unlike policies, completely silently. In scope (D7).

## 2. Decisions

### D1 — System-owned starter policies are code-owned; boot converges the database to code

Every boot converges each starter policy row to the code-defined source, unconditionally. There
is no supported way to weaken a starter policy by editing the database.

This is what `system = true` plus the API's `SystemImmutable` guard already declare; the
database was simply the one place the declaration was not enforced. Rejected alternative:
preserve out-of-band edits and warn forever (the issue's option 1 as written). That was
rejected because it leaves a weakened authorization boundary in effect indefinitely, and
because a permanent warning is exactly the failure mode this issue exists to remove.

**Accepted cost, stated plainly.** An operator who hand-patches a starter policy during an
incident — the only way to patch one, since the API refuses — has that patch reverted by the
next replica boot. The escape hatch is to fork a *new*, non-system `policy_id`, which the
`PutPolicy` API allows and which Cedar composes additively. That hatch is **partial and we do
not pretend otherwise**: a forked `forbid` can add restrictions but can never remove a
code-defined one, and a forked role *template* is never linked by any grant, because
`PolicyEngine::compile` resolves a grant's template by treating `RoleGrant::role_key` as the
template's `policy_id` (`authz/roles.rs` module docs). So starter policies can be tightened
out-of-band but never loosened. Deliberate.

Rejected alternative: an opt-out config knob (`reconcile = enforce | warn_only`) so a
deployment can pin policies. Rejected as YAGNI — new config surface, docs and tests for a
scenario that has not occurred. If the emergency-patch case ever bites, the knob is a small
follow-up on top of this design.

### D2 — A fingerprint column decides the *log level*, never the *action*

`policy.source_fingerprint TEXT NULL` stores the blake3 hex of the `source` **this service last
wrote** for that row. Reconcile compares the stored source against that fingerprint to answer
one question — "did we write this row, or did someone else?" — and that answer selects the log
level and whether to write an audit entry. It never changes what gets written.

Without it, always-converge still fixes most of the problem (warn once per release that changes
a policy, then silent), but a routine action-catalog addition would still emit a false-positive
WARN in every environment during exactly the upgrade window when operators are most likely to
be reading boot logs. The issue's acceptance criterion — reconcile *silently* on a routine
change, *warn* on a genuine edit — requires the distinction.

blake3 is already a workspace dependency of both crates (`rs/Cargo.toml:149`,
`paigasus-iam-core/Cargo.toml:30`, `paigasus-iam/Cargo.toml:98`) for
`CompiledPolicies::content_hash` and the decision-cache key. No new dependency, no `deny.toml`
or `cargo-machete` churn.

### D3 — No SQL backfill; the column is adopted at first boot

blake3 is not computable in Postgres (`pgcrypto` does not offer it), so m0010 adds the column
and stops. The first `reconcile_starter` after the upgrade stamps every system row, including
rows whose source already matches code (a fingerprint-only write).

This leaves a deliberate **one-boot trust window**: a row that existed before m0010 has no
recorded provenance, so on that first boot it is *adopted* rather than warned about — including
a row that was already hand-edited. That is acceptable because the interim remediation the
current runbook prescribes is "make the stored source match the code", which converges to the
same place, and because the alternative (warn on every unfingerprinted row) would reproduce the
exact false-positive storm this issue is about. After that boot every system row carries a
fingerprint and the window is closed.

### D4 — Classification order: provenance before content

The classifier checks provenance (`fingerprint == blake3(stored_source)`) *before* asking
whether the stored source matches code. A row hand-edited to exactly the code-defined value —
which is remediation #1 in today's runbook — therefore still classifies as
`ExternallyModified`.

That is true, and worth saying once: something wrote that row and it was not this service. The
outcome carries `source_changed: false` so the log line stays honest ("modified out of band;
content already matched"), and it self-heals on the same boot when the fingerprint is stamped.
The alternative — a separate quiet outcome for "edited, but to the right value" — is a sixth
state earning its keep only in one benign case.

### D5 — A narrow boot-only port, not a seventh `PolicyStore` method

`reconcile_system` goes on a new `SystemPolicyReconciler` port implemented only by
`PgPolicyStore` (and one fake for the bootstrap unit tests), not on `PolicyStore`. Per (d)
above, extending `PolicyStore` would force six unrelated test fakes to grow a method no
request-path consumer calls. Interface-segregation, and it keeps the diff honest about what is
actually a boot-time capability.

### D6 — `NonSystemCollision` leaves an operator's own policy alone

If the row occupying a starter `policy_id` has `system = false`, reconcile writes nothing and
warns. That state is reachable: a future release adds a ninth role whose key collides with a
`policy_id` an operator already created through `PutPolicy`. Clobbering an operator's own policy
would be strictly worse than the drift it replaces. This warning *is* permanent by design — the
state needs a human, and there is nothing the service can safely do on its own.

### D7 — Role rows converge too, without a fingerprint

`seed_role_row` becomes `reconcile_role_row`: insert when absent (keeping the existing
unique-violation absorption), otherwise compare `template_id` / `scope_kinds` / `description` /
`system` against the code-defined `Role` and update on any difference.

No fingerprint and no audit entry, because the columns are introspectable-only — nothing parses
them back at runtime; the `role_key -> Role` catalog lookup is always code-defined
(`bootstrap.rs` module docs). There is therefore no operator-edit story worth preserving and no
security-relevant content to record.

### D8 — The tamper signal is durable: WARN plus one audit row

Because reconcile now overwrites, the tampered source is destroyed. A transient boot-log line
would be the only trace it ever existed. So `ExternallyModified` — and only that outcome — also
writes one `audit_log` entry capturing the overwritten source.

The entry follows SMA-468's boot-time null-actor pattern verbatim: `action: "PutPolicy"`,
`actor_prn: None` (no principal authorized this — a code deployment did), `outcome: Committed`,
`resource_prn: Some("policy/{policy_id}")` matching `application::policies`'s existing non-PRN
convention for policy identity (a `policy_id` is a caller-chosen string, not a `Uuid`, so it
cannot round-trip through `Prn::build`), and
`detail: { policy_id, source: "starter_policy_reconcile", reason: "external_modification", source_changed, previous_source }`.

`detail.source = "starter_policy_reconcile"` is what distinguishes this row from an
operator-issued `PutPolicy`, exactly as `detail.source = "bootstrap_admins"` does for the
bootstrap grant.

Rejected: an additional Prometheus counter. It would make the event alertable without anyone
reading boot logs, but it pulls `ops/observability/` rules, dashboards and the
`:observability-drift` gate into an otherwise IAM-local change. Noted as a follow-up (§7).

### D9 — An audit-write failure logs ERROR and does not fail boot

The revert has already committed by the time the audit row is written. Refusing to start the
service because the audit insert hiccupped converts a bookkeeping failure into an outage, and
the WARN still fires either way.

The cost is that the audit row is not atomic with the revert. Making it atomic would mean
constructing the entry inside `reconcile_system`'s transaction, which would drag `IdGenerator`
and `Clock` into the persistence adapter and invert the layering for a rare path. Stated rather
than engineered around.

### D10 — `policy_gen` bumps only when the source actually changed

Reconcile bumps the generation counter best-effort (the `put`/`delete` posture: logged and
swallowed, because the write already committed) — but only when policy *content* changed. A
fingerprint-only stamp (D3) changes nothing a decision can observe, so it must not invalidate
caches.

The bump matters even though `reconcile_starter` runs before this process compiles its own
snapshot: it is what tells the *other* replicas, already serving, to reload.

## 3. The fix

### 3.1 New module — `paigasus_iam_core::authz::reconcile`

Pure, no I/O, no `PolicyDocument` change (the fingerprint never enters the domain model):

```rust
/// A borrowed view of the persisted row's three decision-relevant columns. Deliberately not
/// `PolicyDocument` — the fingerprint is a persistence concern and must not enter the domain
/// model, and the classifier needs no other column.
pub struct StoredPolicyRow<'a> {
    pub source: &'a str,
    pub fingerprint: Option<&'a str>,
    pub system: bool,
}

pub enum StarterPolicyOutcome {
    Absent,
    Unchanged,
    Adopted { source_changed: bool },
    Reconciled,
    ExternallyModified { source_changed: bool, previous_source: String },
    NonSystemCollision,
}

pub fn classify_starter_policy(
    stored: Option<StoredPolicyRow<'_>>,
    code_source: &str,
) -> StarterPolicyOutcome;
```

`ExternallyModified::previous_source` is the source we are about to overwrite. When
`source_changed == false` it equals the code-defined source — the edit landed on exactly the
right value (D4) — which is redundant but harmless, and keeps the audit row's shape uniform.

Truth table:

| stored row | `system` | provenance (`fp == blake3(source)`) | matches code | outcome | writes | log | audit |
|---|---|---|---|---|---|---|---|
| absent | — | — | — | `Absent` | insert | INFO | no |
| present | `false` | — | — | `NonSystemCollision` | nothing | **WARN** | no |
| present | `true` | `fp = NULL` | either | `Adopted { source_changed }` | source + fp | DEBUG/INFO | no |
| present | `true` | mismatch | either | `ExternallyModified { .. }` | source + fp | **WARN** | **yes** |
| present | `true` | match | yes | `Unchanged` | nothing | — | no |
| present | `true` | match | no | `Reconciled` | source + fp | INFO | no |

`Adopted` logs at DEBUG when `source_changed == false` (pure fingerprint stamp, nothing
happened operationally) and INFO when `true` (content converged under unknown provenance).

A pure `role_row_matches(stored, code) -> bool` helper lands here too, serving D7.

### 3.2 New port — `SystemPolicyReconciler`

In `authz/ports.rs`, alongside the existing ports:

```rust
#[async_trait]
pub trait SystemPolicyReconciler: Send + Sync {
    async fn reconcile_system(&self, doc: &PolicyDocument) -> Result<StarterPolicyOutcome, AuthzError>;
}
```

### 3.3 Migration — `m0010_policy_source_fingerprint`

Follows m0009 verbatim: `SET LOCAL lock_timeout = '5s'` so the `ACCESS EXCLUSIVE` request backs
off rather than queueing ahead of in-flight writes during a rolling deploy, and
`ADD COLUMN IF NOT EXISTS` because SeaORM's migrator does not serialize concurrent `up()`
across replicas (m0007/m0008/m0009 module docs).

```sql
ALTER TABLE "policy" ADD COLUMN IF NOT EXISTS source_fingerprint TEXT NULL;
```

`down` drops it. No index — the column is only ever read as part of a `find_by_id` on the
primary key. No backfill (D3).

`entities/policy.rs` gains `pub source_fingerprint: Option<String>`.

### 3.4 `PgPolicyStore::reconcile_system`

1. `validate_policy(&doc.source)?` — the same guard `put_in` applies. A code-defined source
   always passes (`roles.rs`'s own test suite asserts it), so this is a tripwire against a bad
   catalog change reaching the database, not a routine check.
2. Open a transaction; `policy::Entity::find_by_id(...).lock_exclusive().one(txn)`.
3. `classify_starter_policy(...)`.
4. Act:
   - `Absent` → INSERT with `source_fingerprint = blake3(doc.source)`, reusing the existing
     SAVEPOINT unique-violation absorption (`pg_policies.rs:234-275`) — on violation, roll the
     savepoint back, re-read the winner within the same outer transaction, and re-classify
     against it (no second insert is possible, so this terminates).
   - `Unchanged` / `NonSystemCollision` → no write.
   - `Adopted` / `Reconciled` / `ExternallyModified` → UPDATE `source`, `source_fingerprint`,
     `updated_at`; preserve the stored `created_at` (the `put_in` rule — an incoming
     `doc.created_at` must never rewrite history).
5. Commit, then best-effort `policy_gen` bump **only when the source changed** (D10).

`put_in` / `delete_in` and their `SystemImmutable` guard are **not touched**. The public
`PutPolicy` API remains unable to edit a system row.

### 3.5 `bootstrap.rs` — split so the interesting half escapes Docker

```rust
pub async fn reconcile_policies(
    reconciler: &dyn SystemPolicyReconciler,
    audit: &dyn AuditLog,
    ids: &dyn IdGenerator,
    clock: &dyn Clock,
) -> Result<(), AuthzError>;

pub async fn reconcile_roles(db: &DatabaseConnection) -> Result<(), AuthzError>;

/// Policies first, then roles.
pub async fn reconcile_starter(
    reconciler: &dyn SystemPolicyReconciler,
    audit: &dyn AuditLog,
    ids: &dyn IdGenerator,
    clock: &dyn Clock,
    db: &DatabaseConnection,
) -> Result<(), AuthzError>;
```

`ids` mints the `AuditEntry`'s `id` and `correlation_id`; `clock` stamps `occurred_at` — the
same two ports `bootstrap_admin.rs` takes for the same reason. This changes
`reconcile_starter`'s signature, so every call site moves (`adapters/http/mod.rs:338` plus the
four in `tests/authz_bootstrap.rs`).

Policies stay first: every role template's `policy_id == Role::key == Role::template_id`, and
`role.template_id` carries an FK to `policy.policy_id` (`fk_role_template`), so the referenced
policy row must exist before the role row can be inserted.

`reconcile_policies` takes no `DatabaseConnection`, so the outcome → log-level → audit mapping
is unit-testable against fakes. `reconcile_roles` keeps touching the `role` entity directly (as
today — there is no `RoleRepository` port, by design) and stays Docker-covered.

### 3.6 Composition root

`adapters/http/mod.rs:338` currently calls `reconcile_starter(policy_store.as_ref(), &db)`.
`PgAuditLog::new(db.clone())` moves above that call (it is a cheap, stateless handle; the
`with_query_window` chaining stays where it is, on the instance the query API uses). The
`KernelIdGenerator` / `SystemClock` values passed in are the same ones every other service gets.

## 4. Tests

### 4.1 Unit — the classifier (Docker-free, primary guard)

One test per row of §3.1's truth table, including explicitly:
- provenance broken **and** content already matches code → `ExternallyModified { source_changed: false }` (D4);
- `system = false` → `NonSystemCollision` regardless of content (D6);
- `fingerprint = NULL` with content matching → `Adopted { source_changed: false }` (D3).

### 4.2 Unit — `reconcile_policies` against fakes (Docker-free)

A fake `SystemPolicyReconciler` returning scripted outcomes plus the existing in-memory audit
fake, asserting:
- exactly one audit entry, only for `ExternallyModified`, with the D8 shape (`action`,
  null actor, `resource_prn`, `detail.source`, `detail.previous_source`);
- no audit entry for `Absent` / `Adopted` / `Reconciled` / `Unchanged` / `NonSystemCollision`;
- an audit-write failure does not fail the call (D9).

### 4.3 Unit — the role-row comparison helper (Docker-free)

Equal rows compare equal; each of `template_id` / `scope_kinds` / `description` / `system`
differing is detected.

### 4.4 Docker integration — `tests/authz_bootstrap.rs`

Extending the existing suite:
- fresh database → seeds every starter policy, every row carries a fingerprint;
- immediate second run → writes nothing (all `Unchanged`), no audit rows;
- **simulated code change**: rewrite a row's `source` *and* set a matching fingerprint, then
  reconcile → the row converges back to the code-defined source, and **no** audit row is
  written;
- **simulated out-of-band edit**: rewrite a row's `source` only, leaving the fingerprint stale
  → the row converges, and **exactly one** correctly-shaped audit row is written;
- **pre-m0010 row**: set `source_fingerprint = NULL` → stamped, no audit row (D3);
- **non-system collision**: insert a `system = false` row at a starter `policy_id` → left
  untouched, no audit row (D6);
- **role drift**: change a persisted `role.description` → converged on next reconcile (D7).

### 4.5 Existing suites

`authz_boot_smoke.rs`, `grpc_authz.rs`, `authz_policy_store.rs` and `authz_acceptance.rs` all
exercise `reconcile_starter` or the policy store; they must stay green unchanged apart from the
`reconcile_starter` signature.

## 5. Documentation

`docs/ops/RUNBOOK-observability.md` — the "Starter-policy drift warning at boot" section is
rewritten, not amended. The behaviour it documents no longer exists:
- starter policies are code-owned; a stored row is converged to the code-defined source at boot;
- the new WARN fires only for an out-of-band edit, and what it means;
- how to retrieve the audit row, including the SMA-467 lookback trap — `PgAuditLog::query`
  applies a default window (`audit.query_default_window_days`, default 90) whenever both `from`
  and `to` are absent, so an unfiltered query against an older database silently returns
  nothing. Query `action = "PutPolicy"` with an explicit `from`, then match on
  `detail.source = "starter_policy_reconcile"`;
- that a hand-patched starter policy is reverted on the next boot, and that the supported way to
  add restrictions is a new non-system `policy_id` — with D1's honest caveat that a starter
  policy cannot be loosened this way at all.

## 6. Rollout, rollback, residual risk

**Rollout.** m0010 adds a nullable column — no rewrite, no lock beyond the brief
`ACCESS EXCLUSIVE` bounded by `lock_timeout`. The first boot after the upgrade adopts every
system row's fingerprint and converges any drifted content, at INFO.

**Mixed-version window is safe** (§1.3c): compile is parse-only, so an old replica reloading a
newer source referencing an unknown `Action` parses it and simply never matches it. No compile
failure, no snapshot poisoning — and `PolicySnapshot` never swaps in a failed compile anyway
(its module docs: an `Err` is logged and the previous known-good set keeps serving).

**Rollback self-heals.** Downgrading to vN finds vN+1's source carrying a *matching* fingerprint
(vN+1 stamped it), classifies it `Reconciled`, and converges back to vN's source at INFO — not a
false WARN. m0010's column is left in place; vN simply never reads it, and its
compare-and-warn logic behaves exactly as it did before.

**Residual risk 1 — flapping during a mixed-version window.** If vN and vN+1 replicas are both
restarting, each boot rewrites the row to its own version and bumps `policy_gen`, so the row can
flap and other replicas reload their snapshots each time. Bounded by the number of restarts,
self-resolving once the deploy settles, and harmless (each installed set is internally
consistent).

**Residual risk 2 — the one-boot trust window** (D3): a pre-m0010 hand-edit is adopted rather
than warned about on the first boot after upgrade. Accepted and documented.

**Residual risk 3 — audit row not atomic with the revert** (D9).

## 7. Out of scope / follow-ups

- **A Prometheus counter for out-of-band modification** (D8). Worth doing if starter-policy
  tampering ever becomes a real concern; deliberately excluded here to keep the change inside
  the IAM crate and off the `:observability-drift` gate.
- **An opt-out "pin policies" config knob** (D1). Small follow-up if the emergency-patch case
  ever bites.
- **Fingerprinting role rows** (D7). Only worth it if those columns ever become load-bearing at
  runtime, which they are not today.
- **`PutPolicy`'s `SystemImmutable` guard** is unchanged. Making system policies editable
  through the API is a different, larger decision.

## 8. Acceptance criteria

1. A routine action-catalog addition (a new `Action` that is a write and not a restore) changes
   `forbid-archived-writes`'s generated source; on the next boot of a database seeded before the
   change, the stored row is converged to the new source and **no `WARN` is logged**.
2. A starter policy row whose `source` was changed out-of-band is converged to the code-defined
   source, a `WARN` naming the `policy_id` is logged, and **exactly one** `audit_log` entry
   records the overwritten source with `detail.source = "starter_policy_reconcile"`.
3. A second boot with no code change and no external edit writes nothing and logs no `WARN`.
4. A `system = false` row occupying a starter `policy_id` is left untouched and warned about.
5. A `role` row whose persisted columns differ from the code-defined `Role` is converged.
6. `PutPolicy` / `DeletePolicy` still reject mutation of a persisted `system = true` row with
   `SystemImmutable`.
