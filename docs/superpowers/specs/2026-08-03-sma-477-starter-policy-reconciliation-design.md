# SMA-477 — Starter policies reconcile by compare-and-warn, so any action-catalog change drifts forever

**Status:** design (revised after adversarial review)
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

All verified against `main` at `36c27f5`.

**(a) There is no code path that can update a seeded starter policy.**
`PgPolicyStore::put_in` (`pg_policies.rs:209-213`) and `delete_in` (`pg_policies.rs:295-297`)
both return `AuthzError::SystemImmutable` for *any* mutation of an already-persisted
`system = true` row. So "not overwriting" is not merely a choice `reconcile_starter` makes — it
is the only thing it *can* do through the port it holds. A self-healing fix needs a new store
capability, not just a changed `if`.

**(b) Therefore "an operator edited the row" can only mean direct SQL.** There is no API path to
a system-owned policy. This is what makes it defensible to treat these rows as code-owned rather
than negotiating with whatever is in the database (D1). It is also, as D2 admits, what makes the
fingerprint a hint rather than a tamper-proof.

**(c) A mixed-version rolling deploy cannot break an old replica's snapshot.**
`PolicyEngine::compile` (`rs/crates/libs/paigasus-iam-core/src/authz/engine.rs:71-100`) uses
`Policy::parse` / `Template::parse` — parse only, **no schema validation**
(`authz::schema::validate_policy` is a separate function, called only from `put_in`). An old
replica that reloads a newer policy source referencing an `Action` its binary does not know
therefore parses it fine; the clause simply never matches. (This is *not* the same as saying
mixed-version operation is safe — see D11, which addresses the direction that genuinely is
unsafe.)

**(d) `PolicyStore` has seven implementations** — one real (`PgPolicyStore`) and six test fakes
(`cedar_authorizer.rs:384`, `policy_snapshot.rs:530` and `:603`, `policies.rs:291` and `:328`,
`fakes.rs:599`). Adding a boot-only method to that trait would force all seven to change for a
capability no request-path consumer uses (D5).

**(e) The same defect exists one function down.** `seed_role_row` (`bootstrap.rs:49-66`) inserts
a `role` row when absent and never updates it, so a code change to a role's `description` or
`scope_kinds` drifts forever too — and unlike policies, completely silently. In scope (D7).

**(f) Reconcile failure is currently harmless and would stop being so.** Against a seeded
database today's `reconcile_starter` performs **zero writes**, so it cannot fail. Every path out
of it propagates to `AppState::new` (`adapters/http/mod.rs:338`) and then to `main.rs:60`
(`AppState::new(db.clone(), &config).await?`), i.e. process exit. Turning reconcile into an
unconditional writer therefore introduces a new way for a replica to fail to start (D12).

## 2. Decisions

### D1 — System-owned starter policies are code-owned; boot converges the database to code

Every boot converges each starter policy row to the code-defined content, subject only to the
revision guard in D11.

This is what `system = true` plus the API's `SystemImmutable` guard already declare; the
database was simply the one place the declaration was not enforced. Rejected alternative:
preserve out-of-band edits and warn forever (the issue's option 1 as written). That leaves a
weakened authorization boundary in effect indefinitely, and a permanent warning is exactly the
failure mode this issue exists to remove.

**Accepted cost, stated plainly.** An operator who hand-patches a starter policy during an
incident — the only way to patch one, since the API refuses — has that patch reverted by the
next replica boot.

**There is effectively no escape hatch, and the earlier draft of this spec overstated one.**
Forking a *new*, non-system `policy_id` lets an operator add a `forbid` that composes additively
with the starter set, but: `PutPolicy` is Root-only (`platform_admin`), the policy must be
hand-authored in Cedar, a fork can only ever *tighten* — it can never remove a code-defined
`forbid` — and a forked role *template* is never linked by any grant, because
`PolicyEngine::compile` resolves a grant's template by treating `RoleGrant::role_key` as the
template's `policy_id` (`engine.rs:88-93`, `authz/roles.rs` module docs). So: starter policies
can be tightened out-of-band and cannot be loosened at all. That is the intended posture, stated
as a limitation rather than dressed up as a hatch.

Rejected alternative: an opt-out config knob (`reconcile = enforce | warn_only`). YAGNI — new
config surface, docs and tests for a scenario that has not occurred. If the emergency-patch case
ever bites, the knob is a small follow-up on top of this design.

### D2 — A content fingerprint decides the *log level*, never the *action* — and it is not tamper-proof

`policy.content_fingerprint TEXT NULL` stores the blake3 hex of a canonical encoding of the
`(kind, source, description)` **this service last wrote** for that row. Reconcile compares the
stored row against that fingerprint to answer one question — "did we write this row, or did
something else?" — and that answer selects the log level, the metric label, and whether to write
an audit entry. It never changes what gets written.

Without it, always-converge still fixes most of the problem (warn once per release that changes
a policy, then silent), but a routine action-catalog addition would still emit a false-positive
WARN in every environment during exactly the upgrade window when operators are most likely to be
reading boot logs. The issue's acceptance criterion — reconcile *silently* on a routine change,
*warn* on a genuine edit — requires the distinction.

**It detects accidents and naive edits, not adversaries.** Per (b), the only actor who can
modify a system row is one with direct SQL access, and that same access trivially recomputes the
fingerprint (`UPDATE policy SET source = <weakened>, content_fingerprint = <blake3 of weakened>`),
which classifies as `Reconciled` — INFO, no audit row. The fingerprint is a **provenance hint**,
not tamper evidence. Making it real would mean an HMAC under a pepper (the codebase has the
idiom in `SecretHasher`, `paigasus-iam-core/src/ports.rs:199-202`), which drags secret material
into the boot path for a threat model where the attacker already has write access to the
authorization tables — and could equally grant themselves `platform_admin` in `role_grant`. Not
worth it. The limit is stated here and in the runbook so nobody reads the WARN as a security
guarantee.

The fingerprint is **lowercase hex, 64 chars**: `blake3::hash(canonical.as_bytes()).to_hex()`,
where `canonical` is a length-prefixed encoding of the three fields (length-prefixed so no field
value can forge a boundary). A `CHECK (content_fingerprint ~ '^[0-9a-f]{64}$')` pins it.

blake3 is already a workspace dependency of both crates (`rs/Cargo.toml:149`,
`paigasus-iam-core/Cargo.toml:30`, `paigasus-iam/Cargo.toml:98`). No new dependency, no
`deny.toml` or `cargo-machete` churn.

### D3 — No SQL backfill; the columns are adopted at first boot, and adoption is audited

blake3 is not computable in Postgres (`pgcrypto` does not offer it), so m0010 adds the columns
and stops. The first `reconcile_starter` after the upgrade stamps every system row.

This leaves a deliberate **one-boot trust window**: a row that existed before m0010 has no
recorded provenance, so on that first boot it is *adopted* rather than warned about. Warning on
every unfingerprinted row would reproduce the exact false-positive storm this issue is about.

But that first boot is also the moment a pre-existing hand-edit is most likely to exist, and
converging destroys it. So `Adopted { content_changed: true }` **writes the D8 audit row**
(`reason: "adopted_unfingerprinted"`, carrying `previous_content`) while logging at **INFO, not
WARN**. Forensics are preserved; no false alarm is raised. `Adopted { content_changed: false }`
is a pure stamp — DEBUG, no audit.

### D4 — Classification order: provenance before content

The classifier checks provenance (`fingerprint == blake3(canonical(stored))`) *before* asking
whether the stored content matches code. A row hand-edited to exactly the code-defined value —
which is remediation #1 in today's runbook — therefore still classifies as `ExternallyModified`.

That is true, and worth saying once: something wrote that row and it was not this service. The
outcome carries `content_changed: false` so the log line stays honest ("modified out of band;
content already matched"), and it self-heals on the same boot when the fingerprint is stamped.

### D5 — Narrow boot-only ports, not a seventh `PolicyStore` method

`reconcile_system` goes on a new `SystemPolicyReconciler` port implemented only by
`PgPolicyStore`, not on `PolicyStore`. Per (d), extending `PolicyStore` would force six
unrelated test fakes to grow a method no request-path consumer calls.

A symmetric `SystemRoleReconciler` port covers the role half (D7), so `reconcile_starter` is
fully fakeable and its whole orchestration — log level, metric, audit — is unit-testable without
Docker. The earlier draft left the role half reaching for the SeaORM entity directly while
*growing* that code; a pure comparison helper whose only caller needs Docker is the signature of
a missing port, not a well-placed helper.

### D6 — The starter `policy_id` namespace is reserved, and non-system rows at those ids converge

The earlier draft had a `NonSystemCollision` outcome that wrote nothing when the row at a
starter `policy_id` had `system = false`. **That was a one-`UPDATE` bypass of D1's entire
premise**: the only actor who can tamper is one with SQL access, and

```sql
UPDATE policy SET system = false, source = 'permit(principal, action, resource);'
 WHERE policy_id = 'forbid-archived-writes';
```

would have exempted the row from convergence permanently, with a WARN and no audit row — cheaper
than the tamper D8 exists to catch, and leaving the weakened policy governing decisions forever.

Two changes close it:

1. **`put_in` reserves the namespace.** Creating or updating a policy whose `policy_id` is in
   `authz::roles::STARTER_POLICY_IDS` is rejected with `AuthzError::SystemImmutable` — reusing
   the existing variant, so `TenancyError::SystemImmutable` and its API mapping are unchanged.
   The check is on the id, not on the stored row's `system` flag, so it holds even for an id
   that is not yet seeded.
2. **`system = false` at a starter id is treated as broken provenance**, not as an operator's
   policy: it classifies `ExternallyModified`, and the UPDATE restores `system = true` along
   with the content. `NonSystemCollision` is deleted from the design.

**The `!system` check must precede the fingerprint branches, and the NULL-fingerprint branch must
split on the revision.** (Added after the final whole-branch review, which found the first draft
of §3.1's row ordering silently overriding this decision's prose.) `system = false` combined with
a cleared `content_fingerprint` would otherwise reach row 3's adoption path — INFO, and with
untouched content not audited at all — which is *cheaper* than the bypass this decision closed.
And clearing the fingerprint alone downgrades a WARN-plus-audit to a routine `adopted` INFO. That
second case is decidable rather than heuristic: this service writes `content_fingerprint` and
`starter_revision` **together** (`converged_model` sets both, `doc_to_model` sets neither, m0010
back-fills neither), so a revision without a fingerprint is provably a cleared column and never a
pre-m0010 row. See §3.1's table for the resulting order.

The residual case — an operator legitimately created a policy at an id that a *later* release
turns into a role key — is now converged over rather than preserved. That is correct: a role
template *must* exist at that `policy_id` or every grant of that role silently contributes
nothing (`engine.rs:88-93`), so preserving the squatter would break the role. The audit row
records exactly what was overwritten.

### D7 — Role rows converge too, without a fingerprint

`seed_role_row` becomes `reconcile_role_row` behind `SystemRoleReconciler`: insert when absent
(keeping the existing unique-violation absorption), otherwise compare `template_id` /
`scope_kinds` / `description` / `system` against the code-defined `Role` and update on any
difference.

No fingerprint and no audit entry, because the columns are introspectable-only — nothing parses
them back at runtime; the `role_key -> Role` catalog lookup is always code-defined
(`bootstrap.rs` module docs). There is no operator-edit story worth preserving and no
security-relevant content to record.

### D8 — The tamper signal is durable: WARN, one audit row, and a metric

Because reconcile now overwrites, the modified content is destroyed. A transient boot-log line
would be the only trace it ever existed. So `ExternallyModified` — and `Adopted` with
`content_changed: true`, per D3 — also writes one `audit_log` entry capturing the overwritten
content.

The entry follows SMA-468's boot-time null-actor pattern: `action: "PutPolicy"`,
`actor_prn: None` (no principal authorized this — a code deployment did), `outcome: Committed`,
`determining_policies: vec![]`, and
`detail: { policy_id, source: "starter_policy_reconcile", reason, content_changed, previous_content }`.

`resource_prn` is **`Some(root_prn().canonical())`**. The earlier draft said `"policy/{id}"` and
claimed that matched `application::policies` — it does not. `policy_aggregate_prn`
(`policies.rs:55`) feeds only `DomainEvent::aggregate_prn`; every `PutPolicy`/`DeletePolicy`
*audit* row uses `root_prn().canonical()` with the id in `detail` (`policies.rs:138+141`,
`:188+191`), as does SMA-468's bootstrap grant (`bootstrap_admin.rs:171`). Using anything else
would make reconcile's rows the only `PutPolicy` rows not reachable by
`AuditFilter::resource_prn` (`audit.rs:46`), silently splitting the audit query surface.

`detail.source = "starter_policy_reconcile"` distinguishes this row from an operator-issued
`PutPolicy`, exactly as `detail.source = "bootstrap_admins"` does for the bootstrap grant.
`previous_content` is **truncated to 8 KiB** with a `previous_content_truncated: true` marker —
it is attacker-influenced text being copied into an append-only table.

`AuditLog::record_out_of_band` is the method used (`reconcile_policies` holds no `Transaction`);
see D9.

**A metric ships too**, reversing the earlier draft's rejection, which rested on a factually
wrong claim. The `:observability-drift` gate's test is
`dashboards_and_rules_reference_only_known_metrics`
(`paigasus-observability/tests/drift.rs:138`) — it asserts that *ops artifacts reference
registered families*, the reverse direction. Registering a name and emitting it touches nothing
under `ops/observability/**` and cannot red the gate; `IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL`
(`bootstrap_admin.rs:217,240`) is exact boot-time precedent. So:
`iam_starter_policy_reconciles_total{outcome}` with a closed label set
(`unchanged` | `seeded` | `adopted` | `reconciled` | `externally_modified` | `stale_binary` |
`orphaned` | `failed`). Without it the tamper signal's only surfaces are a boot WARN — which
§1.2 argues operators learn to ignore — and an audit row nobody queries. The alert rule stays a
follow-up (§7).

### D9 — An audit-write failure logs ERROR and does not fail boot

The convergence has already committed by the time the audit row is written. Refusing to start
the service because the audit insert hiccupped converts a bookkeeping failure into an outage,
and the WARN and metric still fire.

The cost is that the audit row is not atomic with the convergence. Making it atomic would mean
constructing the entry inside `reconcile_system`'s transaction, which would drag `IdGenerator`
and `Clock` into the persistence adapter and invert the layering for a rare path. Stated rather
than engineered around.

### D10 — `policy_gen` bumps only when content actually changed

Reconcile bumps the generation counter best-effort (the `put`/`delete` posture: logged and
swallowed, because the write already committed) — but only when policy *content* changed. A
fingerprint-only stamp (D3) changes nothing a decision can observe, so it must not invalidate
caches.

The bump matters even though `reconcile_starter` runs before this process compiles its own
snapshot: it is what tells the *other* replicas, already serving, to reload. That is also
exactly why D11 is necessary.

### D11 — Convergence is monotonic: an older binary never rewrites a newer starter policy set

There is **one** `policy` table for the whole fleet. So a vN replica booting during a
mixed-version window rewrites the shared row to vN's content and, via D10's bump, makes every
already-serving vN+1 replica reload it (`PolicySnapshot::reload_if_stale`). Today's
compare-and-warn design cannot do this because it never writes; unguarded always-converge can.

The consequences split by policy kind, and neither is acceptable as a silent regression:
- `forbid-archived-writes` reverting to a shorter action list is fail-**open** in a secondary
  control — `roles.rs:249-253` documents it as a belt-and-braces guard with M1's in-txn guards
  as the real gate — but fail-open is exactly the direction D1 exists to prevent.
- A role *template* reverting is fail-**closed**: admins transiently lose a newly added action.

Neither settles under blue/green or a long-lived canary, and an old pod can boot without anyone
deciding to deploy it (HPA scale-up, crashloop restart, a held canary).

**Mechanism.** `authz::roles::STARTER_POLICY_REVISION: u32`, persisted per row as
`policy.starter_revision INTEGER NULL`. Reconcile writes only when
`CODE_REVISION >= stored_revision`; a lower code revision yields `StaleBinary` — **no write, no
audit** (an older binary has no authority over a row a newer release wrote, and deferring entirely
is what keeps convergence monotonic). `NULL` reads as `0`, so every pre-m0010 row proceeds to
normal classification.

**`StaleBinary` carries `provenance_ok`.** (Added after the final whole-branch review.) Because
the revision check runs first and writes nothing, one
`UPDATE policy SET source = '<weakened>', starter_revision = 2147483647` leaves the row diverged
on every future boot — structurally the same bypass D6 closed, and recovery would need a
`STARTER_POLICY_REVISION` past 2^31. The no-write behaviour cannot change without breaking this
decision, so the outcome instead reports whether the stored row's provenance holds up
(`system = true` plus a fingerprint over its own content). A genuine newer release always stamps
both, so `provenance_ok == false` is unambiguous tampering with no false positives:

- `provenance_ok: true` → INFO, metric `stale_binary` — the routine mixed-version case.
- `provenance_ok: false` → **WARN** naming the row as diverged and stating that this binary will
  not repair it, and metric **`externally_modified`**, so the planned alert (§7) fires rather than
  the label operators are told to ignore during a deploy.

**No audit row in either case.** Unlike a converged `ExternallyModified` — a one-off, because the
next boot finds the row repaired — this recurs on every boot of every replica until a human acts,
and `audit_log` is append-only. That is exactly the reasoning D13 applies to orphans. The WARN and
the metric are the whole signal, and the runbook carries the remediation.

`CARGO_PKG_VERSION` cannot serve here: `paigasus-iam`'s version is `0.0.0`
(`services/paigasus-iam/Cargo.toml:3`). So the constant is hand-maintained — and therefore
paired with a guard so it cannot be forgotten: a unit test pins a blake3 hash over the canonical
encoding of every starter policy, and reds the moment any starter policy's content changes,
with a message instructing the author to bump `STARTER_POLICY_REVISION` and update the literal.
This is the same self-enforcing drift-guard shape the repo already uses for codegen, the parity
corpus, and observability.

The guard also makes D1's rollback story honest: after a genuine downgrade, vN leaves vN+1's
policy set in place rather than "self-healing" backwards into a looser one.

### D12 — Reconcile failure does not stop a replica booting, except on the seeding path

Per (f), every reconcile error currently reaches `main.rs:60` and exits the process. With
reconcile writing on every boot, that would turn a transient Postgres blip, a lock wait, or a
deadlock into "this replica will not start" — where before it started fine.

The rule is split by whether the row exists:
- **`Absent` (seeding) failures stay fatal.** `AppState::new`'s documented invariant is that the
  initial snapshot "always compiles at least that starter set, never an empty one"
  (`adapters/http/mod.rs:290-294`). Continuing past a failed *seed* would boot a replica with an
  empty or partial policy set, denying everything. This preserves today's behaviour exactly.

  **This applies symmetrically to the role half (D7).** (Clarified after the final whole-branch
  review, which found the implementation had made every role error survivable — a regression from
  `main`, where `seed_role_row`'s error propagated.) `role_grant.role_key` carries an FK to
  `role.key` (`fk_role_grant_role`, `m0004_create_authz.rs:134`), so a replica that boots past a
  failed `platform_admin` INSERT on a fresh database does not fail at boot — it fails the first
  bootstrap-admin grant with a raw foreign-key violation, at authentication time. So
  `SystemRoleReconciler` gains `existing_role_keys`, the twin of `existing_policy_ids`, and
  `reconcile_roles` decides fatality against a pre-loop snapshot exactly as `reconcile_policies`
  does. Role *convergence* failures remain survivable: those columns are introspectable-only.
- **Convergence failures are logged at ERROR, counted (`outcome = "failed"`), and skipped.** The
  stored row governed decisions perfectly well before this change; keeping it for one more boot
  is strictly better than not booting.

`validate_policy` failing on a *code-defined* source (§3.4 step 1) is a broken release, not an
operator error — but it is also skip-and-continue, for the same reason: turning a bad build into
"no replica in any environment starts" is a worse outage than one stale policy.

The reconcile transaction sets `SET LOCAL lock_timeout = '5s'` (mirroring m0009/m0008) so a row
lock held by a concurrent `PolicyService::put_in` — which also takes `lock_exclusive()`
(`pg_policies.rs:208`) — cannot block startup indefinitely, before the HTTP listener binds and
before the health endpoint can answer.

### D13 — Orphaned system rows are reported, not removed

A `system = true` policy row (or `role` row) whose id has left `starter_policies()` /
`system_roles()` — a retired role — is not deleted. It keeps compiling via `list_all`, keeps
linking any surviving `role_grant` (`engine.rs:88-93`), and `DeletePolicy` refuses to remove it
(`SystemImmutable`). So "converges to code" is **additive-only**, and this design does not
change that.

Reconcile logs a WARN and counts `outcome = "orphaned"` for each such row. No audit entry: this
fires on *every* boot until a human acts, and an append-only table must not accrue a row per
replica per boot forever. A safe retirement path (revoking grants, dropping the FK'd `role` row,
then the policy) is a real piece of work with its own ordering constraints — §7 follow-up.

## 3. The fix

### 3.1 New module — `paigasus_iam_core::authz::reconcile`

Pure, no I/O. `PolicyDocument` is unchanged — the fingerprint and revision are **port DTO**
fields, not domain-model fields, which is why they live on `StoredPolicyRow` rather than on the
document the rest of the system passes around:

```rust
/// A borrowed view of the persisted row's decision-relevant columns, as read by the
/// reconciler port. Not a domain model and not part of `PolicyDocument`.
pub struct StoredPolicyRow<'a> {
    pub kind: PolicyKind,
    pub source: &'a str,
    pub description: &'a str,
    pub system: bool,
    pub fingerprint: Option<&'a str>,
    pub revision: Option<u32>,
}

pub enum StarterPolicyOutcome {
    Absent,
    Unchanged,
    StaleBinary { provenance_ok: bool },
    Adopted { content_changed: bool, previous_content: Option<String> },
    Reconciled,
    ExternallyModified { content_changed: bool, previous_content: String },
}

pub fn classify_starter_policy(
    stored: Option<StoredPolicyRow<'_>>,
    code: &PolicyDocument,
    code_revision: u32,
) -> StarterPolicyOutcome;

/// Length-prefixed canonical encoding of the content-bearing triple, hashed for the
/// fingerprint. Length-prefixed so no field value can forge a field boundary.
pub fn content_fingerprint(kind: PolicyKind, source: &str, description: &str) -> String;

/// D7's comparison, pure so it tests without Docker.
pub fn role_row_matches(stored: &StoredRoleRow<'_>, code: &Role) -> bool;
```

Classification, in order:

| # | condition | outcome | writes | log | audit | metric label |
|---|---|---|---|---|---|---|
| 1 | no row | `Absent` | insert | INFO | no | `seeded` |
| 2 | `stored.revision > code_revision` | `StaleBinary { provenance_ok }` | nothing | INFO / **WARN** | no | `stale_binary` / `externally_modified` |
| 3 | `!system` | `ExternallyModified { .. }` | content + stamp (+ `system = true`) | **WARN** | **yes** | `externally_modified` |
| 4 | `fingerprint IS NULL` **and** `revision IS NULL` | `Adopted { content_changed }` | content + stamp | DEBUG / INFO | only if changed | `adopted` |
| 5 | `fingerprint IS NULL` **but** `revision IS NOT NULL` | `ExternallyModified { .. }` | content + stamp | **WARN** | **yes** | `externally_modified` |
| 6 | fingerprint mismatch | `ExternallyModified { .. }` | content + stamp | **WARN** | **yes** | `externally_modified` |
| 7 | content matches code | `Unchanged` | nothing | — | no | `unchanged` |
| 8 | otherwise | `Reconciled` | content + stamp | INFO | no | `reconciled` |

**The order is the security boundary, not presentation** — rows 3–5 are each a one-`UPDATE` way to
downgrade the tamper signal, and the naive order (NULL-fingerprint before `!system`, no revision
discriminator) makes every one of them cheaper than the edit it hides. This table's first draft
had exactly that defect and shipped with it; the final whole-branch review caught it.

Row 2 precedes everything else because an older binary defers unconditionally (D11), and its split
log level / metric label is that decision's `provenance_ok` — the deferral itself is unconditional
either way, and neither variant audits. Row 3 is D6, and it must precede rows 4–6 or a cleared
`system` flag plus a cleared fingerprint lands in row 4. Row 5 is decidable because this service
writes both provenance columns together (D6's addendum), so it is not a heuristic. Row 4 logs
DEBUG when `content_changed == false` (a pure stamp) and INFO when `true`, and audits only in the
latter case (D3).

`content_changed` compares the code-defined `(kind, source, description)` against the stored
triple — **all three**, not `source` alone. The earlier draft converged only `source`, which left
`kind` and `description` drifting forever; `pg_policies.rs::policy_content_matches` (`:88`)
already treats all three as content-bearing, and a stale `kind` is not cosmetic: a `template`
row stored as `static` makes `PolicyEngine::compile` call `Policy::parse` on template source
(`engine.rs:78`), returning `Err`, which fails `PolicySnapshot::new` and therefore boot — a state
a source-only reconcile could never repair.

### 3.2 New ports

In `authz/ports.rs`:

```rust
#[async_trait]
pub trait SystemPolicyReconciler: Send + Sync {
    async fn reconcile_system(&self, doc: &PolicyDocument, revision: u32) -> Result<StarterPolicyOutcome, AuthzError>;
    /// Ids of persisted `system = true` rows that are no longer code-defined (D13).
    async fn orphaned_system_policy_ids(&self, known: &[&str]) -> Result<Vec<String>, AuthzError>;
}

#[async_trait]
pub trait SystemRoleReconciler: Send + Sync {
    async fn reconcile_role(&self, role: &Role) -> Result<RoleOutcome, AuthzError>;
    /// Sorted ascending by key, like its policy twin — boot logs one line per orphan.
    async fn orphaned_system_role_keys(&self, known: &[&str]) -> Result<Vec<String>, AuthzError>;
    /// The twin of `existing_policy_ids`: boot's fatal-vs-survivable snapshot (D12).
    async fn existing_role_keys(&self) -> Result<Vec<String>, AuthzError>;
}
```

### 3.3 `roles.rs` additions

```rust
/// Bumped whenever any starter policy's content changes. Guarded by
/// `starter_policy_content_is_pinned` below — that test reds until this is bumped.
pub const STARTER_POLICY_REVISION: u32 = 1;

/// Every `policy_id` `starter_policies()` produces. A `const` so `put_in`'s reserved-namespace
/// check (D6) is a slice scan, not nine `PolicyDocument` allocations per call.
pub const STARTER_POLICY_IDS: &[&str] = &[FORBID_ARCHIVED_WRITES_ID, "platform_admin", /* ... */];
```

Two guard tests: `STARTER_POLICY_IDS` equals the ids `starter_policies()` actually produces
(so the const cannot drift), and the D11 content-hash pin.

### 3.4 Migration — `m0010_policy_reconcile_columns`

Follows m0009 verbatim: `SET LOCAL lock_timeout = '5s'` so the `ACCESS EXCLUSIVE` request backs
off rather than queueing ahead of in-flight writes during a rolling deploy, and
`ADD COLUMN IF NOT EXISTS` because SeaORM's migrator does not serialize concurrent `up()` across
replicas (m0007/m0008/m0009 module docs).

```sql
ALTER TABLE "policy"
  ADD COLUMN IF NOT EXISTS content_fingerprint TEXT NULL,
  ADD COLUMN IF NOT EXISTS starter_revision INTEGER NULL;
ALTER TABLE "policy" DROP CONSTRAINT IF EXISTS ck_policy_fingerprint;
ALTER TABLE "policy" ADD CONSTRAINT ck_policy_fingerprint
  CHECK (content_fingerprint IS NULL OR content_fingerprint ~ '^[0-9a-f]{64}$');
```

`down` drops both columns and the constraint. No index — the columns are only read as part of a
`find_by_id` on the primary key. No backfill (D3).

`entities/policy.rs` gains `pub content_fingerprint: Option<String>` and
`pub starter_revision: Option<i32>`.

**`doc_to_model` must not learn about them** (`pg_policies.rs:139`): it is shared with `put_in`,
and an operator policy written through `PutPolicy` must leave both columns NULL. Only
`reconcile_system` sets them.

### 3.5 `PgPolicyStore::reconcile_system`

1. `validate_policy(&doc.source)?` — the same guard `put_in` applies. A code-defined source
   always passes (`roles.rs`'s own suite asserts it), so this is a tripwire against a bad catalog
   change reaching the database. Failure is skip-and-continue, not fatal (D12).
2. Begin; `SET LOCAL lock_timeout = '5s'` (D12); `policy::Entity::find_by_id(..).lock_exclusive().one(txn)`.
3. `classify_starter_policy(..)`.
4. Act:
   - `Absent` → INSERT with fingerprint + revision, reusing the SAVEPOINT unique-violation
     absorption (`pg_policies.rs:234-275`). On violation: roll the savepoint back, re-read the
     winner **with `lock_exclusive()`** — the existing code's re-read is unlocked
     (`pg_policies.rs:264`) because it only compares and returns, whereas we may UPDATE
     afterwards — and re-classify against it. No second insert is possible, so this terminates.
   - `Unchanged` / `StaleBinary` → no write.
   - `Adopted` / `Reconciled` / `ExternallyModified` → UPDATE `kind`, `source`, `description`,
     `system = true`, `content_fingerprint`, `starter_revision`, `updated_at`; preserve the
     stored `created_at` (the `put_in` rule — an incoming `doc.created_at` must never rewrite
     history). `updated_at` comes from the injected `Clock`, not `doc.updated_at`
     (`starter_policies()` stamps that with its own `Utc::now()`, `roles.rs:231`).
5. Commit, then best-effort `policy_gen` bump **only when content changed** (D10).

`put_in` gains D6's reserved-namespace rejection. `delete_in` and the existing `SystemImmutable`
guard are otherwise untouched.

Note: a pure fingerprint stamp still bumps `updated_at`, which is operator-visible through
`ListPolicies`. Documented in the runbook so it is not misread as a content change.

### 3.6 `bootstrap.rs`

Following the repo's DI convention at this arity — `BootstrapAdminSeederDeps`
(`bootstrap_admin.rs:62-65`) and `RoleServiceDeps` use named-field `*Deps` structs with generic
(not `&dyn`) `IdGenerator`/`Clock`:

```rust
pub struct ReconcileStarterDeps<I: IdGenerator, C: Clock> {
    pub policies: Arc<dyn SystemPolicyReconciler>,
    pub roles: Arc<dyn SystemRoleReconciler>,
    pub audit: Arc<dyn AuditLog>,
    pub ids: I,
    pub clock: C,
}

pub async fn reconcile_policies(..) -> Result<(), AuthzError>;  // policies half
pub async fn reconcile_roles(..) -> Result<(), AuthzError>;     // roles half
pub async fn reconcile_starter(..) -> Result<(), AuthzError>;   // policies first, then roles
```

Policies stay first: every role template's `policy_id == Role::key == Role::template_id`, and
`role.template_id` carries an FK to `policy.policy_id` (`fk_role_template`,
`m0004_create_authz.rs:103`), so the referenced policy row must exist before the role row can be
inserted.

Neither half takes a `DatabaseConnection` any more (both sit behind ports, D5), so the whole
orchestration — outcome → log level → metric → audit — is unit-testable against fakes. Only
`Absent` failures propagate (D12).

### 3.7 Composition root

`adapters/http/mod.rs:338`. `PgAuditLog` is built **once**, above the reconcile call, with its
`with_query_window(..)` chaining already applied — `with_query_window` takes `mut self` and
returns `Self` (`pg_audit_log.rs:54`), so building a plain instance for reconcile and a windowed
one for the query API would create two instances and violate the one-instance invariant
documented at `adapters/http/mod.rs:393-400`. Reconcile simply does not use the window.

## 4. Tests

### 4.1 Unit — the classifier (Docker-free, primary guard)

One test per row of §3.1, plus explicitly:
- `revision` strictly greater than code → `StaleBinary`, even when content differs and provenance
  is broken (D11 precedence);
- `system = false` → `ExternallyModified` regardless of fingerprint state (D6);
- provenance broken **and** content already matches → `ExternallyModified { content_changed: false }` (D4);
- `kind` differing alone, and `description` differing alone, each → `Reconciled` (§3.1's
  three-field comparison);
- `fingerprint IS NULL` + content matching → `Adopted { content_changed: false }`, no audit (D3);
- `revision IS NULL` reads as `0` and does not trigger `StaleBinary`.

### 4.2 Unit — `starter_policy_content_is_pinned` (D11's self-enforcing guard)

blake3 over the canonical encoding of every starter policy, pinned to a literal, with a failure
message instructing the author to bump `STARTER_POLICY_REVISION` and update the literal.
Plus: `STARTER_POLICY_IDS` equals `starter_policies()`'s actual ids.

### 4.3 Unit — `reconcile_starter` against fakes (Docker-free)

Fake `SystemPolicyReconciler` / `SystemRoleReconciler` returning scripted outcomes, asserting:
- exactly one audit entry for `ExternallyModified` and for `Adopted { content_changed: true }`,
  with the D8 shape (`action: "PutPolicy"`, `actor_prn: None`,
  `resource_prn == root_prn().canonical()`, `detail.source`, `detail.previous_content`);
- no audit entry for `Unchanged` / `StaleBinary` / `Reconciled` / `Absent` /
  `Adopted { content_changed: false }`;
- `previous_content` over 8 KiB is truncated and marked;
- an audit-write failure does not fail the call (D9);
- a convergence failure is logged and skipped, an `Absent` failure propagates (D12);
- the metric label matches the outcome for every variant.

**`FakeAuditLog` needs extending first.** Its `record_out_of_band` is currently
`unimplemented!("application-layer unit tests never call record_out_of_band")`
(`fakes.rs:958-960`) and would panic — reconcile uses exactly that method (D9). A
`FailingAuditLog` fake is also new work.

### 4.4 Unit — the role-row comparison helper (Docker-free)

Equal rows compare equal; each of `template_id` / `scope_kinds` / `description` / `system`
differing is detected.

### 4.5 Docker integration — `tests/authz_bootstrap.rs`

- fresh database → seeds every starter policy; every row carries a fingerprint and revision;
- immediate second run → writes nothing, no audit rows;
- **simulated code change**: rewrite a row's content *and* set a matching fingerprint → converges
  back to code, **no** audit row, `policy_gen` **incremented** (D10);
- **simulated out-of-band edit**: rewrite content only, leaving the fingerprint stale → converges,
  **exactly one** correctly-shaped audit row;
- **`system = false` tamper**: set `system = false` → converged *and* `system` restored to `true`,
  one audit row (D6);
- **reserved namespace**: `PutPolicy` on a starter `policy_id` is rejected `SystemImmutable` (D6);
- **pre-m0010 row**: set both new columns `NULL` → stamped; audit row iff content changed (D3);
- **stale binary**: set `starter_revision` above `STARTER_POLICY_REVISION` → row untouched, no
  audit, no bump (D11);
- **fingerprint-only stamp** leaves `policy_gen` unchanged (D10);
- **concurrent boot**: two `reconcile_system` calls raced against a fresh database → both succeed,
  exactly one row, correct content (the SAVEPOINT re-classify path, §3.5);
- **orphan**: a `system = true` row at an unknown id → WARN, left in place, no audit (D13);
- **role drift**: change a persisted `role.description` → converged (D7).

### 4.6 Existing suites

`authz_boot_smoke.rs`, `grpc_authz.rs`, `authz_policy_store.rs`, `authz_acceptance.rs` must stay
green apart from the `reconcile_starter` signature change.

## 5. Documentation

`docs/ops/RUNBOOK-observability.md` — the "Starter-policy drift warning at boot" section is
rewritten, not amended; the behaviour it documents no longer exists:
- starter policies are code-owned and converged at boot; `PutPolicy` now rejects the starter id
  namespace (D6);
- the new WARN fires only for an out-of-band edit — **and is a provenance hint, not tamper
  evidence** (D2's limit, stated plainly);
- `iam_starter_policy_reconciles_total{outcome}` and what each label means;
- how to retrieve the audit row, including the SMA-467 lookback trap: `PgAuditLog::query`
  applies a default window (`audit.query_default_window_days`, default 90) whenever both `from`
  and `to` are absent, so an unfiltered query against an older database silently returns nothing.
  Query `action = "PutPolicy"` with an explicit `from`, then match
  `detail.source = "starter_policy_reconcile"`;
- that a hand-patched starter policy is reverted on the next boot, with D1's honest statement
  that there is effectively no escape hatch;
- that a fingerprint-only stamp bumps `updated_at` without changing content;
- the `StaleBinary`/orphan lines and what an operator should do about each.
- Fix the pre-existing id typo at `RUNBOOK-observability.md:1463` — `forbid_archived_writes`
  should be `forbid-archived-writes` (`roles.rs:41`).

Also stale and fixed in passing, since D7 rewrites both files: `bootstrap.rs:3` says "the seven
system roles" and `tests/authz_bootstrap.rs:68` is named `..._the_seven_system_roles`;
`system_roles()` returns **eight**.

## 6. Rollout, rollback, residual risk

**Rollout.** m0010 adds two nullable columns and a CHECK — no rewrite, and the brief
`ACCESS EXCLUSIVE` is bounded by `lock_timeout`. The first boot after the upgrade stamps every
system row and converges any drifted content, at INFO, auditing only where content changed (D3).

**Mixed-version is guarded, not merely survivable.** (c) establishes an old replica can always
*parse* a newer source; D11 establishes an old replica never *overwrites* one. Together those
give monotonic convergence: the fleet's policy set only ever moves forward, whichever replica
boots.

**Rollback.** m0010's columns are left in place; vN never reads them, and its compare-and-warn
logic behaves exactly as before. If a playbook ever runs `down()`, a vN+1 replica's entity breaks
until the migration is re-applied — `down()` is not part of any deployment playbook here, and
this is called out so it stays that way.

**Residual risk 1 — the one-boot trust window** (D3): a pre-m0010 hand-edit is adopted rather
than warned about on the first boot after upgrade. It is audited, so it is recoverable.

**Residual risk 2 — the fingerprint is not tamper-proof** (D2). An adversary with SQL write
access recomputes it and the edit reads as a routine code change.

**Residual risk 2b — a forged high `starter_revision` is detected but cannot be auto-repaired**
(D11). Because deferring to a newer revision is unconditional and writes nothing, a row stamped
with a revision no running binary can beat stays diverged on every boot. It is no longer *silent*:
the outcome carries `provenance_ok: false`, which raises a WARN and the `externally_modified`
metric. But no replica will converge it — that requires either repairing the row directly or
shipping a build with a higher `STARTER_POLICY_REVISION`, both documented in the runbook. This is
the one place where D11's monotonicity guarantee and D1's convergence guarantee genuinely conflict,
and monotonicity wins: an older binary that "repaired" such a row could equally be reverting a
real newer release across the whole fleet.

**Residual risk 3 — the audit row is not atomic with the convergence** (D9).

**Residual risk 4 — `STARTER_POLICY_REVISION` is hand-maintained.** The pinned-content test
(§4.2) makes forgetting it a red build rather than a silent regression, but a determined author
can bump the literal without bumping the revision.

**Residual risk 5 — orphaned system rows are reported, never removed** (D13).

**Unmeasured:** the fleet-wide cost of a `policy_gen` bump (`list_all` + full Cedar compile per
replica, plus a decision-cache key-space rotation via `content_hash`, `engine.rs:98`). D11 bounds
how often reconcile can trigger one — at most once per replica per release — which is why this is
noted rather than measured.

## 7. Out of scope / follow-ups

- **A retirement path for orphaned system policies/roles** (D13) — revoking grants, dropping the
  FK'd `role` row, then the policy, in that order. Real work with its own ordering constraints.
- **An alert rule on `iam_starter_policy_reconciles_total{outcome="externally_modified"}`.** The
  metric ships here (D8); the rule and dashboard panel belong with the `ops/observability/` work.
- **No `DomainEvent` / outbox event for a reconcile-driven change.** Every other writer of
  `policy` content enqueues `EventType::PolicyPut` (`policies.rs:123-132`). Boot-time convergence
  deliberately does not: it is a deployment consequence rather than an operator action, no
  consumer of that stream exists yet, and emitting from bootstrap would require threading a
  `UnitOfWork` through — which would also re-open D9's atomicity decision. Revisit when a
  consumer exists.
- **An HMAC-under-pepper fingerprint** (D2), if starter-policy tampering ever becomes a real
  threat model rather than an accident-detection concern.
- **An opt-out "pin policies" config knob** (D1).
- **Fingerprinting role rows** (D7) — only worth it if those columns become load-bearing.
- **`starter_policies()` taking an injected `now`** rather than calling `Utc::now()` internally
  (`roles.rs:231`). Cosmetic here, since reconcile ignores the document's timestamps.

## 8. Acceptance criteria

1. A routine action-catalog addition changes `forbid-archived-writes`'s generated source; on the
   next boot of a database seeded before the change, the stored row is converged and **no `WARN`
   is logged**.
2. The boot after that one writes nothing and logs no `WARN`.
3. A starter policy row whose content was changed out-of-band is converged, a `WARN` naming the
   `policy_id` is logged, and **exactly one** `audit_log` entry records the overwritten content
   with `detail.source = "starter_policy_reconcile"` and `resource_prn == root_prn()`.
4. A row set to `system = false` is converged **and** has `system` restored to `true`, with an
   audit row; `PutPolicy` rejects any `policy_id` in the starter namespace with `SystemImmutable`.
5. A row whose `starter_revision` exceeds the running binary's `STARTER_POLICY_REVISION` is left
   untouched, with no audit row and no `policy_gen` bump.
6. A fingerprint-only stamp does not bump `policy_gen`; a content change does.
7. A pre-m0010 row (both new columns `NULL`) is stamped, and audited only if its content changed.
8. A `role` row whose persisted columns differ from the code-defined `Role` is converged.
9. A transient failure converging an existing row logs ERROR, increments
   `iam_starter_policy_reconciles_total{outcome="failed"}`, and the replica still boots; a failure
   *seeding* an absent row still fails boot.
10. `PutPolicy` / `DeletePolicy` still reject mutation of a persisted `system = true` row with
    `SystemImmutable`.
11. Changing any starter policy's content without bumping `STARTER_POLICY_REVISION` reds
    `starter_policy_content_is_pinned`.
