# SMA-468 — Audit the bootstrap-admin `platform_admin` grant

**Status:** design
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

**The asymmetry is the point.** Every role grant in the system is auditable except the one
that makes someone a platform administrator — the single most privileged grant there is, and
the first thing a compliance reviewer asks about. For a service whose thesis is an auditable
identity backbone, that is precisely the wrong grant to leave untraced.

It is also not a one-time boot event. `ensure_platform_admin` is called from **both** the HTTP
bearer middleware (`adapters::http::auth_middleware::require_bearer`) and the gRPC enforcement
layer (`adapters::grpc::authn::AuthEnforce`), on every authenticated request, gated by an
in-memory `HashSet` lookup and then a `list_by_principal` existence check. It re-evaluates on
every fresh deployment and on retry after a failed persist.

### 1.1 What reading the code changed about the issue's framing

SMA-468 says a system actor is needed "since the schema and the Cedar principal model both
assume a real principal." **The schema half is not true**, and that materially simplifies the
central design question:

- `audit_log.actor_prn` is declared `.text().null()`
  (`adapters/persistence/migration/m0006_create_audit_log.rs:49`) — nullable.
- That migration's own module doc states the intent: *"No foreign keys: `actor_prn`/
  `resource_prn` are free-form PRN text, not FK'd to `principal`/tenancy rows — an audit entry
  must survive its actor or resource being deleted later."*
- `AuditEntry::actor_prn` and `DomainEvent::actor_prn` are both `Option<String>`
  (`audit.rs:34`, `domain_event.rs:63`).
- There is **existing precedent** for omitting it: `create_user.rs:111-119` writes
  `actor_prn: None` with an explicit rationale, as do `authz/denial_audit.rs:219` and
  `grpc/audit.rs:224`.
- Conversely, **no "system actor" concept exists anywhere in the codebase.** Introducing one
  would be genuinely new, not a matter of conforming to an existing model.

So the real question is not "how do we fit a system actor into a schema that rejects one" but
"is a null actor honest and sufficient here" — settled in D2.

### 1.2 Two structural facts that constrain the solution

**`RoleService::grant` cannot be reused.** Its first act is
`self.authorize.check(actor, Action::GrantRole, &scope_resource_prn(&scope))`
(`roles.rs:207`). The entire premise of bootstrap seeding is that *nobody holds authority
yet* — that is why the seeder exists. Routing it through `RoleService::grant` would require
defeating that authorization check, a worse change than the one being fixed.

**The outbox is transaction-only.** `Outbox::enqueue(&self, tx: &dyn Transaction, ev:
&DomainEvent)` (`ports.rs:344`) has no out-of-band counterpart, unlike `AuditLog`, which
offers both `record(tx, e)` and `record_out_of_band(e)` (`ports.rs:285,292`). So wanting the
domain event at all forces the transactional design; atomicity and the event are one decision,
not two.

### 1.3 Evidence

- `application/bootstrap_admin.rs` — the seeder; `self.grants.grant(&grant)` is the whole
  persistence path.
- `application/roles.rs:199-254` — the reference implementation (event + entry + txn + bump).
- `adapters/persistence/pg_role_grants.rs:149-165` — `RoleGrantStore::grant` is a one-shot
  UnitOfWork wrapper: begin → `grant_in` → commit → **`bump_policy_gen_best_effort()`**.
- `adapters/persistence/pg_role_grants.rs:182-186` — `grant_in` inserts **only**; it does not
  bump. This is the trap D5 exists for.
- `ports.rs:284-292` (`AuditLog`), `:343-345` (`Outbox`), `:304-306` (`UnitOfWork`),
  `:366-368` (`PolicyGenBumper`).
- `domain_event.rs:14-34` — `EventType` is a plain Rust enum with an `as_wire()` mapping; it
  is **not** proto-backed, so adding a variant would not be a contracts/`:breaking` change.

## 2. Decisions

**D1 — Atomic: the grant, its audit row and its outbox event commit together.** The seeder
opens its own `UnitOfWork` and runs `grant_in` + `enqueue` + `record` + `commit`. There is no
window in which the most privileged grant in the system exists without its audit row, and it
is the only shape that can carry the outbox event at all (§1.2).

*The cost, accepted explicitly:* a **persistent** audit or outbox failure now means the
bootstrap admin is never seeded — lockout, which is the exact condition the seeder exists to
prevent. Previously a broken audit backend was invisible here because there was no audit
write; now it can block seeding. D6 makes that observable rather than silent. The failure is
still transient-tolerant: nothing is written, the request still succeeds, and the identity
self-heals on its next authentication.

**D2 — `actor_prn: None`, with the authority recorded in `detail`.** No principal authorized
this grant; operator configuration did, and saying so plainly is more honest than inventing a
principal. This follows the three existing `actor_prn: None` precedents (§1.1) and introduces
no new concepts.

*Rejected:* a synthetic system PRN (`prn:pgs:iam:::system/<sentinel-uuid>`). It would be
queryable via `AuditFilter::actor_prn` and reusable by other unaudited system writes, but
`Prn::build` requires a real `Uuid` (`resource_name.rs:155`), so it means inventing both a
`system` resource type and a sentinel UUID that exist nowhere today — speculative generality
for one call site. *Also rejected:* using the authenticating admin as their own actor; it
reads as "this person granted themselves platform_admin", which is misleading about where the
authority came from and is arguably worse than null for the very question this issue exists to
answer.

*Consequence to accept:* `AuditFilter` cannot filter **for** a null actor, so a reviewer finds
this entry by `action`/`detail`, not by actor. D3 is what makes that workable.

**D3 — Reuse `action: "GrantRole"` and `EventType::RoleGranted`; mark provenance in the
payloads.** The seeded grant must appear in the standard query a reviewer runs ("every
`GrantRole` touching `platform_admin`"). A distinct action such as `"SeedBootstrapAdmin"`
would hide it from exactly that query — recreating this issue in subtler form — and would be
the first audit `action` string in the codebase that is not a Cedar `Action` name (every
existing one is: `GrantRole`, `RevokeRole`, `PutPolicy`, `IssueApiKey`, …). A
`source: "bootstrap_admins"` marker in `detail`/`payload` gives greppability at no cost.
`outcome: AuditOutcome::Committed`, matching `RoleService::grant`.

**D4 — The audit `detail` carries the matched identity; the event `payload` does not.** The
audit row is an internal compliance artifact and should record *which* configured identity
matched — that is the question a reviewer will ask. The `DomainEvent` crosses a broker
boundary via the outbox, so it stays PII-minimal, following `create_user.rs:111`'s explicit
posture ("The payload is PII-minimal: `principal_id` + `kind` only, never the email address").

| field | `AuditEntry.detail` | `DomainEvent.payload` |
|---|---|---|
| `role_key`, `scope`, `source` | ✅ | ✅ |
| `grant_id` | ✅ | ✅ |
| matched `issuer` + `subject` | ✅ | ❌ |

**D5 — The seeder must bump `policy_gen` itself.** Today it gets this for free:
`RoleGrantStore::grant` ends with `bump_policy_gen_best_effort()`, and the seeder's module doc
explicitly depends on that ("a seeded grant bumps the identical `policy_gen` counter
`CedarAuthorizer` polls"). `grant_in` does **not** bump. So moving to the transactional path
silently drops policy-snapshot invalidation, and a freshly seeded admin would hold no
effective permissions until the TTL backstop (~31 s at defaults).

The seeder therefore takes a `PolicyGenBumper` and calls an awaited, best-effort
`bump()` **after** commit — identical to `RoleService::grant:252`. Nothing in the issue hints
at this; it is the highest-risk part of the change and §5 gives it a dedicated test.

**D6 — One new counter for seeding failures.** `iam_bootstrap_admin_seed_failures_total`,
incremented on any swallowed failure in the seed path. This is the one addition beyond the
issue's literal scope, and it exists to make D1's accepted lockout risk observable: without it,
"the bootstrap admin is silently never seeded" is a `tracing::warn!` nobody is watching. Must
be registered in `paigasus-observability`'s `names.rs` or the `repo:observability-drift` gate
fails.

*Deliberately not in scope:* an alert rule on that counter. The metric makes the condition
visible; whether it pages is an operator decision that wants its own runbook entry, and this
issue has no evidence about how often it fires.

**D7 — `application/bootstrap.rs` stays out of scope.** It seeds starter policies and the
system role *catalog* — code-defined definitions, not grants. The issue says so explicitly, and
auditing definition-seeding is a different question with a different answer.

## 3. The fix

### 3.1 Dependencies and construction

`BootstrapAdminSeeder` gains four fields: `uow: Arc<dyn UnitOfWork>`,
`outbox: Arc<dyn Outbox>`, `audit: Arc<dyn AuditLog>`, `gen_bumper: Arc<dyn PolicyGenBumper>`.

That brings the constructor to eight arguments, so it moves to a
`BootstrapAdminSeederDeps` struct, mirroring `RoleServiceDeps` (`roles.rs:129`) — the same
pattern already used next door for the same reason. `admins`, `grants`, `ids` and `clock` are
unchanged.

The existing hot-path property is preserved exactly: `ensure_platform_admin` still returns
after a single `HashSet` lookup for any non-bootstrap identity, before touching any of the new
dependencies.

### 3.2 The write path

Replaces the lone `self.grants.grant(&grant)`:

```rust
let corr = self.ids.new_correlation_id();
let event = DomainEvent {
    id: self.ids.new_event_id(),
    event_type: EventType::RoleGranted,
    schema_version: 1,
    aggregate_prn: grant.principal.canonical(),
    actor_prn: None,                                    // D2
    occurred_at: now,
    payload: json!({                                    // D4: no issuer/subject
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
    resource_prn: Some(scope_resource_prn(&grant.scope).canonical()),
    outcome: AuditOutcome::Committed,
    determining_policies: vec![],
    detail: json!({                                     // D4: issuer/subject included
        "role_key": grant.role_key,
        "scope": grant.scope.canonical_prn(),
        "source": "bootstrap_admins",
        "issuer": issuer.as_str(),
        "subject": subject,
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

`scope_resource_prn` is `roles.rs`'s existing helper and is **private** (`roles.rs:82`, no
`pub`). It moves to a shared location within `application/` and both call sites use it —
duplicating a PRN-construction helper across two modules is exactly the drift this audit row
cannot afford (the two would silently disagree about the resource a grant was recorded
against).

### 3.3 Error handling

The best-effort contract is **unchanged**: every failure is logged with the existing
`tracing::warn!` shape and swallowed, never propagated, so a seeding hiccup can never fail the
request that triggered it. `ensure_platform_admin` keeps returning `()`.

What changes is the granularity. Any failure before `commit()` drops the transaction, so
**nothing** is written — no grant, no event, no audit row — and `bump()` never runs. The
identity retries on its next authentication, where the `list_by_principal` idempotence check
still holds. The warning message must name the lockout consequence (D1), not just the error.

## 4. What is deliberately unchanged

- The `HashSet` fast path and its zero-round-trip guarantee for non-bootstrap identities.
- Idempotence via `list_by_principal` + the `platform_admin`@`Root` existence check.
- The `Provisioning::Disabled` exclusion — `introspect` still never calls the seeder (D10:
  introspect has no side effects), and a role-grant insert plus an audit row is now an even
  stronger reason for that to hold.
- `ensure_platform_admin`'s signature and its `()` return.

## 5. Tests

All Docker-free, in `bootstrap_admin.rs`'s existing `#[cfg(test)] mod tests`.

**Every fake needed already exists** in `application/fakes.rs` — no new test infrastructure
beyond one erroring `AuditLog`:

- `FakeUnitOfWork` (`:905`), `FakeOutbox(pub Arc<Mutex<Vec<DomainEvent>>>)` (`:918`),
  `FakeAuditLog(pub Arc<Mutex<Vec<AuditEntry>>>)` (`:935`),
  `FakePolicyGenBumper(pub Arc<AtomicU64>)` (`:957`) — their public backing fields make
  assertions on captured events/entries and on the bump count direct.
- `InMemoryRoleGrants`, `SeqIds`, `FixedClock` — already used by this module's tests.
- Test 3 needs the one genuinely new fake: an `AuditLog` whose `record` errors. Copy the
  local failing-fake shape at `roles.rs:408` rather than adding a shared one.

1. **The audit row exists and is correct.** Exactly one `AuditEntry`, with `action="GrantRole"`,
   `actor_prn=None`, `outcome=Committed`, `resource_prn` = the Root scope PRN, and
   `detail.source == "bootstrap_admins"` with the matched `issuer`/`subject` present.
2. **The event exists and is PII-minimal.** Exactly one `RoleGranted` event whose payload
   contains `grant_id`/`role_key`/`scope`/`source` and **does not** contain `issuer` or
   `subject` — a direct guard on D4, which is otherwise easy to erode by copy-paste from the
   audit entry.
3. **Atomicity.** With an `AuditLog` fake that errors on `record`, **no grant row is
   persisted** and no event is enqueued. Copies the "simulated mid-txn store failure" fake
   shape already at `roles.rs:408`.
4. **`gen_bumper` runs once on success, never on failure.** Two assertions over a counting
   fake: one bump after a successful seed; zero bumps when the transaction failed. This is the
   direct regression guard for D5 — the silent-permissions-regression risk.
5. **Idempotence is unchanged.** A second authentication produces no second grant **and no
   second audit entry or event** — the existing test extended to the new artifacts.
6. **The fast path is unchanged.** A non-configured identity still makes zero store round
   trips, and now also zero audit/outbox/uow calls.

The existing tests (`configured_identity_gets_a_platform_admin_root_grant`,
`an_existing_platform_admin_grant_is_left_untouched`, the issuer/subject mismatch pair, the
unparseable-issuer case) must all still pass, updated only for the new constructor shape.

## 6. Rollout, rollback, residual risk

**Rollout.** No schema change, no migration, no config change, no contract change (`EventType`
gains no variant — D3). One new metric family. A rolling deploy is safe; an old and a new
replica differ only in whether a seed writes an audit row.

**Rollback.** Revert. Audit rows already written remain valid and readable.

**Residual risks.**

- *Lockout on a persistent audit/outbox failure* (D1, accepted). Observable via D6's counter
  and the existing warning. Not alerted — see D6.
- *The seeder now does more work inside a request.* It is still behind the `HashSet` gate and
  the existence check, so the added cost is paid only on the authentications that actually
  seed — in practice once per bootstrap identity per deployment. No hot-path regression for
  ordinary traffic.
- *`detail` now contains the IdP `subject`.* It is an opaque identifier rather than an email,
  and `audit_log` already stores actor PRNs, so this does not change the table's sensitivity
  class. It is called out here because D4's asymmetry is a deliberate boundary and a future
  edit that "helpfully" copies `detail` into `payload` would breach it — test 2 exists to
  catch that.

## 7. Out of scope / follow-ups

- **An alert on `iam_bootstrap_admin_seed_failures_total`** (D6) — needs operator input and a
  runbook entry.
- **Auditing `application/bootstrap.rs`'s starter-policy and role-catalog seeding** (D7) — a
  separate question about whether code-defined definitions warrant an audit trail at all.
- **A general system-actor concept** (D2's rejected option) — worth revisiting if a second
  system-initiated mutation needs auditing, at which point it stops being speculative.

## 8. Acceptance criteria

1. A bootstrap-admin seed writes its grant, one `RoleGranted` outbox event and one `GrantRole`
   audit row **atomically**; a failure in any of them writes none of them.
2. The audit row has `actor_prn = None`, `outcome = Committed`, and a `detail` naming
   `source = "bootstrap_admins"` plus the matched issuer and subject.
3. The outbox event payload contains no `issuer` and no `subject`.
4. `policy_gen` is bumped exactly once after a successful seed and never after a failed one, so
   a seeded admin's permissions take effect immediately rather than at the TTL backstop.
5. `ensure_platform_admin` still swallows every failure and never fails the calling request;
   the non-bootstrap fast path still makes zero round trips.
6. `iam_bootstrap_admin_seed_failures_total` is emitted on failure and registered in
   `names.rs`.
7. The full CI gate graph passes:
   `moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke
   :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool
   :observability-drift :release-parity :release-parity-py :release-parity-ts
   --base origin/main --include-relations`.
