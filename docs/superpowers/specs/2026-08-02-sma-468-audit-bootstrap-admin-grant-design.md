# SMA-468 — Audit the bootstrap-admin `platform_admin` grant

**Status:** design (revised after adversarial review)
**Date:** 2026-08-02
**Issue:** [SMA-468](https://linear.app/smaschek/issue/SMA-468/iam-audit-the-bootstrap-admin-platform-admin-grant-seeder-bypasses)
**Project:** Paigasus IAM → Hardening
**Surfaced by:** SMA-446 (M5)

## 1. Problem

`BootstrapAdminSeeder::ensure_platform_admin`
(`rs/crates/services/paigasus-iam/src/application/bootstrap_admin.rs`) mints a
`platform_admin`@`Root` [`RoleGrant`] and persists it with a bare
`self.grants.grant(&grant)` call on the [`RoleGrantStore`] port.

`RoleService::grant` (`application/roles.rs:199-254`) is where a role grant's audit row and
outbox event are built and committed **in the same transaction** as the grant itself. The
seeder bypasses all of it. The word "audit" does not appear in the file.

`platform_admin`@`Root` is the most privileged grant the system can issue, and it is issued
with no trace. For a service whose thesis is an auditable identity backbone, that is the
worst possible grant to leave untraced.

`ensure_platform_admin` is called from **both** the HTTP bearer middleware
(`adapters::http::auth_middleware::require_bearer`) and the gRPC enforcement layer
(`adapters::grpc::authn::AuthEnforce`), on every authenticated request, gated by an in-memory
`HashSet` lookup and then a `list_by_principal` existence check. The grant is persisted, so a
redeploy against the same database finds it and no-ops — only a **fresh database** re-seeds.

### 1.1 It is not the only unaudited grant

An earlier draft claimed "every role grant is auditable except this one." That is **false**.
`OrganizationService::create` (`application/organizations.rs:42-64`) mints an
`org_admin`@Organization grant for the calling principal — a **self-grant**, "the creating
principal becomes the owner of what it creates" — and persists it through
`self.repo.create(&organization, &default_team, &owner_grant)`. That module imports no
`AuditLog`, `Outbox` or `UnitOfWork` (grep: zero matches).

So after this change a reviewer still cannot answer "show me every privilege grant." That does
not weaken the case for fixing this one — `platform_admin`@`Root` is unbounded authority where
`org_admin` is scoped to an org the caller just created — but the honest framing is "the
highest-privilege unaudited grant", not "the only one". §7 files the sibling.

### 1.2 What reading the code changed about the issue's framing

SMA-468 says a system actor is needed "since the schema and the Cedar principal model both
assume a real principal." **The schema half is not true:**

- `audit_log.actor_prn` is declared `.text().null()`
  (`adapters/persistence/migration/m0006_create_audit_log.rs:49`) — nullable.
- That migration's module doc states the intent: *"No foreign keys: `actor_prn`/`resource_prn`
  are free-form PRN text, not FK'd to `principal`/tenancy rows — an audit entry must survive
  its actor or resource being deleted later."*
- `AuditEntry::actor_prn` and `DomainEvent::actor_prn` are both `Option<String>`
  (`audit.rs:34`, `domain_event.rs:63`).

**But there is no production precedent for a null actor, contrary to an earlier draft.**
`BufferedDenialAuditSink` writes `actor_prn: Some(ev.principal_prn.clone())`
(`denial_audit.rs:196`); the `None`s at `denial_audit.rs:219` and `grpc/audit.rs:224` are both
inside `#[cfg(test)] mod tests` (`:208`, `:154`). `create_user.rs:111` is a `DomainEvent`, not
an `AuditEntry` — that module is documented "OUTBOX-ONLY: no audit entry" (`:97-98`). So D2
**establishes** a new precedent rather than conforming to one, and has to stand on its merits.

### 1.3 Two structural facts that constrain the solution

**`RoleService::grant` cannot be reused.** Its first act is
`self.authorize.check(actor, Action::GrantRole, &scope_resource_prn(&scope))`
(`roles.rs:207`). The premise of bootstrap seeding is that *nobody holds authority yet* — that
is why the seeder exists. Routing it through `RoleService::grant` would mean defeating that
authorization check, a worse change than the one being fixed.

**The outbox is transaction-only.** `Outbox::enqueue(&self, tx: &dyn Transaction, ev:
&DomainEvent)` (`ports.rs:344`) has no out-of-band counterpart, unlike `AuditLog`, which offers
both `record(tx, e)` and `record_out_of_band(e)` (`ports.rs:285,292`).

### 1.4 Evidence

- `application/bootstrap_admin.rs` — the seeder; `self.grants.grant(&grant)` is the whole
  persistence path.
- `application/roles.rs:199-254` — the reference implementation.
- `application/organizations.rs:42-64` — the other unaudited grant (§1.1).
- `adapters/persistence/pg_role_grants.rs:149-165` — `RoleGrantStore::grant` = begin →
  `grant_in` → commit → **`bump_policy_gen_best_effort()`** at `:163`.
- `adapters/persistence/pg_role_grants.rs:182-186` — `grant_in` inserts **only**. This is D5.
- `adapters/authz/policy_snapshot.rs:216-218` — `reload_if_stale` short-circuits on
  `store_gen == current_gen`.
- `ports.rs:284-292` (`AuditLog`), `:343-345` (`Outbox`), `:304-306` (`UnitOfWork`),
  `:366-368` (`PolicyGenBumper` — returns `()`, D6).
- `domain_event.rs:14-37` — `EventType` is a plain Rust enum with `as_wire()`; **not**
  proto-backed, so a new variant would not be a contracts/`:breaking` change.
- `application/fakes.rs:893-912` — `FakeUnitOfWork`'s doc: the fakes ignore `tx` and mutate
  immediately; "real cross-table commit/rollback atomicity is proven by the Postgres
  integration tests instead." This is §5.
- `adapters/persistence/pg_audit_log.rs:165-181` + `config.rs:482` — the 90-day default query
  window (D3).

## 2. Decisions

**D1 — Atomic: the grant, its audit row and its outbox event commit together.** The seeder
opens its own `UnitOfWork` and runs `grant_in` + `enqueue` + `record` + `commit`.

*The alternative, named and rejected on merits.* Keep `grants.grant()` and follow it with
`AuditLog::record_out_of_band` (`ports.rs:285`). It is cheaper, adds one dependency instead of
four, and cannot cause the lockout D1 accepts. It is rejected because **the seed is
idempotent-by-existence**: once the grant row exists, `list_by_principal` short-circuits every
future call (`bootstrap_admin.rs:105-108`), so a grant that commits without its audit row is
never revisited and the gap is **permanent**. A best-effort audit here does not degrade
gracefully the way the denial drain's does — the denial drain sees the same event again on the
next denial; this never does. That asymmetry, not tidiness, is what justifies atomicity.

*The cost, accepted explicitly:* a **persistent** audit or outbox failure now means the
bootstrap admin is never seeded — lockout, the exact condition the seeder exists to prevent.
Transient failures are tolerated: nothing is written, the request still succeeds, and the
identity retries on its next authentication. Recovery from persistent failure is the manual
`psql` seed path the module already acknowledges (`bootstrap_admin.rs:195-196`); §6 documents
it.

*A partition-related failure is NOT a new risk.* After SMA-467 `audit_log` is
`LIST(outcome) → RANGE(occurred_at)` partitioned, so the obvious worry is that a missing month
leaf makes `audit.record` fail. It does not: m0008 creates `audit_log_committed_default` /
`audit_log_denied_default` (`:97-101`) and `audit_log_other` (`:87`), so an uncovered month
routes to a default partition rather than erroring.

**D2 — `actor_prn: None`, with the grantee and authority recorded in `detail`.** No principal
authorized this grant; operator configuration did. Inventing a principal to satisfy a nullable
column would be dishonest.

*Rejected:* a synthetic system PRN (`prn:pgs:iam:::system/<sentinel-uuid>`). `Prn::build`
requires a real `Uuid` (`resource_name.rs:155`), so it means inventing both a `system`
resource type and a sentinel UUID that exist nowhere today — speculative generality for one
call site. *Also rejected:* the authenticating admin as their own actor; it reads as "this
person granted themselves platform_admin", misattributing authority that came from the
operator's config.

**D3 — The row must be self-describing, because it cannot be found by actor.** `AuditFilter`
has no filter for a null actor and none for `detail` (`audit.rs:44-53`; neither transport
exposes one — `http/audit.rs:92-106`, `grpc/audit.rs`). So the retrieval path is `action` +
`resource_prn`, and the row must carry everything a reviewer needs once found.

Reuse `action: "GrantRole"` and `EventType::RoleGranted` so the row appears in the standard
role-grant query. A distinct `"SeedBootstrapAdmin"` action would hide it from exactly that
query — recreating this issue in subtler form — and would be the first audit `action` in the
codebase that is not a Cedar `Action` name (every existing one is: `GrantRole`, `RevokeRole`,
`PutPolicy`, `IssueApiKey`, …). Nothing validates `action` against a known set on read or
write, so reuse is safe.

**The exact retrieval query, which the RUNBOOK-facing docs must state:**
`action=GrantRole`, `resource_prn=<root prn>`, **and an explicit `from`**. The `from` is not
optional: `PgAuditLog::query` applies a default lookback when both `from` and `to` are absent
(`pg_audit_log.rs:167-181`) and `audit.query_default_window_days` defaults to **90**
(`config.rs:482`). Because a seed happens once per fresh database, the row falls outside the
default window 90 days later and an unfiltered query returns nothing.

**D4 — `detail` carries `principal_prn` + `issuer`; never the IdP `subject`; the event payload
carries neither.**

The grantee is the single most important fact in this row and an earlier draft omitted it
entirely — with `actor_prn: None` and `resource_prn` = the *scope* (`root_prn()`), the row
contained **no principal PRN at all**. "Who became platform admin" was recoverable only by
joining the IdP `subject` back through `external_identity`
(`m0003_create_external_identity.rs:38-39,63`), a join that breaks the moment that principal
is deleted — precisely the case m0006 says the audit log must survive.

The IdP `subject` is dropped rather than kept. It is an *external* identifier joinable against
the IdP directory, unlike the internal UUID-based actor PRNs already in the table, and
`audit_log` is append-only and explicitly designed to outlive the rows describing its
subjects — so a `subject` written here cannot be removed under an erasure request. `issuer`
alone gives the provenance a reviewer needs; `principal_prn` gives the identity; and
`external_identity` still maps the two for as long as the principal exists.

| field | `AuditEntry.detail` | `DomainEvent.payload` |
|---|---|---|
| `grant_id`, `role_key`, `scope`, `source` | ✅ | ✅ |
| `principal_prn` | ✅ | (already `aggregate_prn`) |
| `issuer` | ✅ | ❌ |
| IdP `subject` | ❌ | ❌ |

*Note:* `RoleService::grant`'s own audit entry has the same grantee gap
(`roles.rs:232-242` — actor + scope, no grantee). Out of scope here; §7 files it.

**D5 — The seeder must bump `policy_gen` itself.** Today it gets this free:
`RoleGrantStore::grant` ends with `bump_policy_gen_best_effort()` (`pg_role_grants.rs:163`),
and the seeder's module doc explicitly depends on it. `grant_in` does **not** bump
(`:182-186`). Moving to the transactional path therefore silently drops policy-snapshot
invalidation: `reload_if_stale` short-circuits on `store_gen == current_gen`
(`policy_snapshot.rs:216-218`), so only the TTL backstop would reload —
`policy_cache_ttl_secs`(30) + `refresh_interval_secs`(1) ≈ **31 s** at defaults
(`config.rs:443-446`). For a brand-new principal there is no cached slice or decision to mask
it, so the policy reload is genuinely the only gate: the freshly seeded admin would be denied
for ~31 s.

The seeder therefore takes a `PolicyGenBumper` and calls an awaited, best-effort `bump()`
**after** commit — identical to `roles.rs:252`. Nothing in the issue hints at this; §5 gives it
a dedicated regression test.

*Scope note:* `authz.cache.backend` defaults to `memory` (`config.rs:448`), whose `Generations`
counters are per-process (`generation.rs:26-35`). So same-request immediacy is a
single-replica property by default; under the `redis` backend it is fleet-wide.

**D6 — One new counter, with a `stage` label, plus a dashboard panel.**
`iam_bootstrap_admin_seed_failures_total{stage}` where `stage` ∈ `{list, txn}`.

- `list` — the pre-existing `list_by_principal` failure path (`bootstrap_admin.rs:97-104`),
  which is a swallowed seed failure today and is counted for the first time here.
- `txn` — any failure in the `begin`/`grant_in`/`enqueue`/`record`/`commit` sequence.
- **`bump` is deliberately excluded and cannot be included:** `PolicyGenBumper::bump(&self)`
  returns `()` (`ports.rs:366-368`) and `GenerationsPolicyGenBumper::bump` swallows internally
  (`generation.rs:138-144`). A lost bump is therefore invisible to this counter. Changing the
  port signature is out of scope; the exclusion is stated so nobody assumes coverage.

*An earlier draft justified registering the metric by claiming `repo:observability-drift`
requires it. That is backwards.* `drift.rs:137-188`
(`dashboards_and_rules_reference_only_known_metrics`) walks committed dashboards and rules and
asserts every metric they reference is in `names::ALL`. It never asserts the converse — an
entry in `names.rs` that nothing references passes, and so does omitting it.

So the counter must land somewhere an operator actually looks, or D6 is inert and D1's
lockout risk is unmitigated. This issue therefore also adds a **panel to
`ops/observability/grafana/dashboards/iam.json`**, which is what makes the drift gate
load-bearing for this metric. Note `ops/` has no `moon.yml`, so it belongs to the root `repo`
project — the drift gate's narrow `inputs` are what make it run on an `ops/`-only change.

*Deliberately not in scope:* an alert rule. The metric makes the condition visible; whether it
pages needs operator input and its own runbook entry.

**D7 — Errors are logged with their source, and the helper's return type is pinned.** The
seed body returns `Result<(), SeedError>` internally, where `SeedError` is a thin local enum
wrapping `RepositoryError` (from `uow`/`outbox`/`audit`) and `AuthzError` (from `grant_in`);
`ensure_platform_admin` matches on it, counts, logs and discards it, still returning `()`.

It must **not** funnel through `TenancyError`. `From<AuthzError> for TenancyError` collapses
`Backend` to `TenancyError::Internal` (asserted at `roles.rs:559`) whose `Display` is the
constant `"internal server error"` (`error.rs:89-90`). Today's warn logs the concrete error
(`bootstrap_admin.rs:119-125`, `error = %e`) — including the Postgres constraint name. Losing
that would replace the one diagnostic explaining *why* the bootstrap admin was never seeded
with a fixed string, directly undercutting D1.

**D8 — `application/bootstrap.rs` stays out of scope.** It seeds starter policies and the
system role *catalog* — code-defined definitions, not grants.

## 3. The fix

### 3.1 Dependencies and construction

`BootstrapAdminSeeder` gains four fields: `uow: Arc<dyn UnitOfWork>`, `outbox: Arc<dyn Outbox>`,
`audit: Arc<dyn AuditLog>`, `gen_bumper: Arc<dyn PolicyGenBumper>`.

That brings the constructor to eight arguments, so it moves to a `BootstrapAdminSeederDeps`
struct mirroring `RoleServiceDeps` (`roles.rs:120`). `admins`, `grants`, `ids`, `clock` are
unchanged. The `HashSet` fast path still returns before touching any new dependency.

### 3.2 The write path

Replaces the lone `self.grants.grant(&grant)`. The scope is a hardcoded `GrantScope::Root`, so
the resource PRN is exactly `root_prn()` (`authz/model.rs:30`, already `pub`) — no need to
touch `roles.rs`'s private `scope_resource_prn` (`:82`).

```rust
let corr = self.ids.new_correlation_id();
let event = DomainEvent {
    id: self.ids.new_event_id(),
    event_type: EventType::RoleGranted,
    schema_version: 1,
    aggregate_prn: grant.principal.canonical(),         // the grantee
    actor_prn: None,                                    // D2
    occurred_at: now,
    payload: json!({                                    // D4: no issuer, no subject
        "grant_id": grant.id,
        "role_key": grant.role_key,
        "scope": grant.scope.canonical_prn(),
        "source": "bootstrap_admins",
    }),
    correlation_id: Some(corr),
};
let entry = AuditEntry {
    id: self.ids.new_audit_id(),
    occurred_at: now,
    actor_prn: None,                                    // D2
    action: "GrantRole".into(),                         // D3
    resource_prn: Some(root_prn().canonical()),
    outcome: AuditOutcome::Committed,
    determining_policies: vec![],
    detail: json!({                                     // D4
        "principal_prn": grant.principal.canonical(),   // the grantee — the key field
        "grant_id": grant.id,
        "role_key": grant.role_key,
        "scope": grant.scope.canonical_prn(),
        "source": "bootstrap_admins",
        "issuer": issuer.as_str(),                      // provenance; NOT the subject
    }),
    correlation_id: Some(corr),
};

let tx = self.uow.begin().await?;
self.grants.grant_in(&*tx, &grant).await?;
self.outbox.enqueue(&*tx, &event).await?;
self.audit.record(&*tx, &entry).await?;
tx.commit().await?;

self.gen_bumper.bump().await;   // D5 — post-commit, awaited, best-effort
```

`correlation_id` is a fresh id, matching `roles.rs:221`. There is no ambient request-scoped id
available at this call site to join on.

### 3.3 Error handling

The best-effort contract is **unchanged**: every failure is logged and swallowed, never
propagated, and `ensure_platform_admin` keeps returning `()`. A seeding hiccup can never fail
the request that triggered it.

Any failure before `commit()` drops the transaction, so **nothing** is written and `bump()`
never runs. The identity retries on its next authentication, where the `list_by_principal`
idempotence check still holds. Per D7 the log keeps the source error; per D6 it also
increments the counter with its `stage`.

**One expected-and-benign failure:** two concurrent first authentications by the same bootstrap
admin can both pass the existence check and both `grant_in`; the loser violates
`uq_role_grant_principal_role_scope` (`m0004_create_authz.rs:187-188`) and rolls back. Net
state is correct and self-correcting. The counter will register it under `stage="txn"`, so the
metric's documentation must say a low nonzero value is not necessarily pathological.

## 4. What is deliberately unchanged

- The `HashSet` fast path and its zero-round-trip guarantee for non-bootstrap identities.
- Idempotence via `list_by_principal` + the `platform_admin`@`Root` existence check.
- The `Provisioning::Disabled` exclusion — `introspect` still never calls the seeder (D10:
  introspect has no side effects); a role-grant insert plus an audit row is a stronger reason
  for that to hold, not a weaker one.
- `ensure_platform_admin`'s signature and its `()` return.

## 5. Tests

**The fakes are not transactional, and the test plan must respect that.** `FakeUnitOfWork`'s
own doc (`fakes.rs:893-903`) states it: `InMemoryRoleGrants::grant_in` (`:566-569`),
`FakeOutbox::enqueue` (`:922-925`) and `FakeAuditLog::record` all ignore the `&dyn Transaction`
and mutate immediately — "real cross-table commit/rollback atomicity is proven by the Postgres
integration tests instead." A unit test asserting "the audit failed, therefore no grant row
exists" would therefore **fail**, because `audit.record` is the *last* step and the grant is
already in the map. (The `roles.rs:408` failing fake works only because it errors on the
*first* step.)

So atomicity is split across the two levels.

### 5.1 Unit — `bootstrap_admin.rs` `#[cfg(test)] mod tests` (Docker-free)

Existing fakes suffice: `FakeUnitOfWork` (`:905`), `FakeOutbox` (`:918`), `FakeAuditLog`
(`:935`), `FakePolicyGenBumper` (`:957`) — all with public backings — plus
`InMemoryRoleGrants`, `SeqIds`, `FixedClock`. One new local fake: a `RoleGrantStore` whose
`grant_in` errors (copy the shape at `roles.rs:408`).

1. **The audit row is correct and self-describing.** Exactly one `AuditEntry`:
   `action="GrantRole"`, `actor_prn=None`, `outcome=Committed`, `resource_prn=root_prn()`,
   and `detail` containing `principal_prn` (the grantee), `source="bootstrap_admins"` and
   `issuer`.
2. **Neither artifact carries the IdP `subject`.** Assert the string is absent from the
   serialized `detail` *and* the serialized `payload` — a direct guard on D4, which a
   copy-paste between the two would otherwise erode.
3. **Control-flow ordering.** With `grant_in` erroring (the *first* step), **no** event is
   enqueued, **no** audit entry is recorded, and **no** bump occurs. This is what the
   in-memory fakes can honestly prove.
4. **`gen_bumper` runs exactly once on success, never on failure** — the D5 regression guard.
5. **Idempotence extends to the new artifacts.** A second authentication produces no second
   grant, no second audit entry and no second event.
6. **The fast path is untouched.** A non-configured identity produces zero grants, zero audit
   entries and zero events. *(Not "zero uow calls" — `FakeUnitOfWork` is a stateless unit
   struct with no counter, and adding one is not worth new shared infrastructure.)*

Existing tests must still pass, updated only for the new constructor shape.

### 5.2 Integration — `tests/authz_bootstrap_admin.rs` (Postgres, Docker)

The file already exists, and its first test (`:48-53` — a bootstrap admin's *first*
authenticated request must already succeed) is the real end-to-end guard for D5. It must keep
passing and should be labelled as such so a future edit does not weaken it.

Add the one property the unit level cannot prove:

7. **True atomicity.** With the audit write forced to fail (the last step), assert **no
   `role_grant` row exists** for that principal. This is AC1's real verification.

## 6. Rollout, rollback, residual risk

**Rollout.** No schema change, no migration, no config change, no contract change (`EventType`
gains no variant — D3). One new metric family plus one dashboard panel. A rolling deploy is
safe.

**Rollback.** Revert. Audit rows already written remain valid and readable.

**Residual risks.**

- *Lockout on a persistent audit/outbox failure* (D1, accepted). Observable via D6's counter +
  panel. Recovery is the manual `psql` grant insert the module already documents
  (`bootstrap_admin.rs:195-196`); an operator doing that should also insert the audit row, or
  the gap this issue closes reopens by hand.
- *A lost `policy_gen` bump is uncountable* (D6) — a seeded admin is denied for ~31 s and the
  metric reads zero.
- *Retention can erase the row permanently.* `audit.retention.committed_months` defaults to
  `0` = never drop (`config.rs:257-259,269`), but a non-zero value drops the committed monthly
  leaf. Because the seed is idempotent and never re-runs, that permanently erases the only
  audit row for the `platform_admin` grant — restoring exactly the condition SMA-468 exists to
  fix. Operators enabling committed-retention should know this row is not reproducible.
- *A bounded new stall on the auth path.* The partition maintainer takes
  `pg_advisory_xact_lock` + DDL on the parent with `lock_timeout = '5s'`
  (`pg_partition_maintainer.rs:45,137-151`). An `audit_log` insert arriving while that DDL is
  queued can wait up to ~5 s — now inside `require_bearer`. This applies **only to bootstrap
  identities on a seeding authentication**, not to ordinary traffic, which never reaches the
  insert.
- *`org_admin` self-grants remain unaudited* (§1.1) — filed in §7.

## 7. Out of scope / follow-ups

- **Audit `OrganizationService::create`'s `org_admin` self-grant** (§1.1) — the other
  unaudited privilege grant.
- **Add the grantee to `RoleService::grant`'s audit entry** (D4 note) — the normal grant path
  records actor and scope but not who received the role.
- **An alert on `iam_bootstrap_admin_seed_failures_total`** (D6) — needs operator input and a
  runbook entry.
- **Auditing `application/bootstrap.rs`'s starter-policy and role-catalog seeding** (D8).
- **A general system-actor concept** (D2's rejected option) — revisit if a second
  system-initiated mutation needs auditing.

## 8. Acceptance criteria

1. A bootstrap-admin seed writes its grant, one `RoleGranted` outbox event and one `GrantRole`
   audit row **atomically**. Verified at two levels: unit tests prove nothing downstream runs
   after an early failure (§5.1 #3), and a Postgres integration test proves a failed audit
   write leaves no `role_grant` row (§5.2 #7).
2. The audit row has `actor_prn = None`, `outcome = Committed`,
   `resource_prn = root_prn()`, and a `detail` carrying **`principal_prn`** (the grantee),
   `source = "bootstrap_admins"` and `issuer`.
3. Neither the audit `detail` nor the event payload contains the IdP `subject`.
4. `policy_gen` is bumped exactly once after a successful seed and never after a failed one.
5. `ensure_platform_admin` still swallows every failure, never fails the calling request, and
   logs the **source** error rather than a mapped taxonomy error; the non-bootstrap fast path
   still makes zero round trips.
6. `iam_bootstrap_admin_seed_failures_total{stage}` is emitted for `list` and `txn` failures,
   registered in `names.rs`, **and surfaced on `ops/observability/grafana/dashboards/iam.json`**
   so the drift gate covers it.
7. The full CI gate graph passes:
   `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
   :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool
   :observability-drift :release-parity :release-parity-py :release-parity-ts
   --base origin/main --include-relations`.
