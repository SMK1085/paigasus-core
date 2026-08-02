# SMA-468 — Audit the Bootstrap-Admin Grant: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the `platform_admin`@`Root` bootstrap grant write its audit row and outbox event atomically with the grant itself, so the most privileged grant in the system stops being the least traceable one.

**Architecture:** `BootstrapAdminSeeder` stops calling the one-shot `RoleGrantStore::grant` and instead opens its own `UnitOfWork`, mirroring `RoleService::grant` — `grant_in` + `outbox.enqueue` + `audit.record` + `commit`, then an awaited best-effort `PolicyGenBumper::bump()` post-commit. It cannot reuse `RoleService::grant` itself, because that method's first act is an authorization check and bootstrap exists precisely to precede any authority.

**Tech Stack:** Rust (edition 2024, rust-version 1.95), SeaORM + Postgres, `metrics` crate counters, `cargo nextest`, Moon 2.3.2 task graph, Grafana dashboard JSON.

**Spec:** `docs/superpowers/specs/2026-08-02-sma-468-audit-bootstrap-admin-grant-design.md`

## Global Constraints

- SPDX header on every source file: `// SPDX-License-Identifier: Apache-2.0` (`#` for YAML/Python).
- Rust **edition 2024 + rust-version 1.95**.
- `[workspace.lints.rust] warnings = "deny"` in-source; clippy `all = "warn"` in-source and `-D warnings` in CI. `clippy::pedantic` is NOT enabled.
- Conventional commits with a workspace scope. **Subject must start lowercase and be ≤100 chars.**
- **Commit-body trap (cost a CI failure on SMA-473):** never start a body line with `word:` — conventional-commits-parser reads it as a footer token and fails `footer-leading-blank`. Also no bare `#NNN`; write `SMA-468`. The local `commit-msg` hook is **not** proof: an `Entire-Checkpoint:` trailer is appended *after* it runs, so CI sees a different message. Verify with:
  `moon run ts:commitlint -- --from $(git merge-base origin/main HEAD) --to HEAD; echo $?`
- Do **not** use `git commit --no-verify`.
- If a commit fails with `1Password: failed to fill whole buffer`, STOP and report it — the signing key is locked and you cannot fix it.
- Prefix every shell command with: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`
- `cargo nextest` needs `--no-tests=pass`.
- Branch `feature/sma-468-audit-bootstrap-admin-grant` is already checked out. Do not create another.
- Integration tests (`rs/crates/services/paigasus-iam/tests/*.rs`) need Docker; they self-skip when it is absent (`tests/support/mod.rs:61-72`).

## File Structure

| File | Responsibility |
|---|---|
| **Modify** `rs/crates/libs/paigasus-observability/src/names.rs` | Register `IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL` (const + `ALL`). |
| **Modify** `ops/observability/grafana/dashboards/iam.json` | A panel for the new counter — this is what makes the drift gate load-bearing for it. |
| **Modify** `rs/crates/services/paigasus-iam/src/application/bootstrap_admin.rs` | The whole behavior change: `Deps` struct, `SeedError`, the transactional write path, the counter, and the unit tests. |
| **Modify** `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:454` | Construct the seeder from the new `Deps` struct. |
| **Modify** `docs/ops/RUNBOOK-observability.md` | How to actually retrieve the audit row — it is not findable by actor, and the 90-day default query window hides it. |
| **Modify** `rs/crates/services/paigasus-iam/tests/authz_bootstrap_admin.rs` | The Postgres atomicity test the in-memory fakes cannot prove. |

**Why the module is not split:** `bootstrap_admin.rs` is ~210 lines including tests and has one clear responsibility. The change adds a `Deps` struct, a small error enum and one private helper. Splitting would separate `ensure_platform_admin` from the error type only it uses.

---

### Task 1: Dependency plumbing (no behavior change)

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/bootstrap_admin.rs`
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:454`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `BootstrapAdminSeederDeps<I, C>` with public fields `admins_config: Vec<BootstrapAdmin>`, `grants: Arc<dyn RoleGrantStore>`, `uow: Arc<dyn UnitOfWork>`, `outbox: Arc<dyn Outbox>`, `audit: Arc<dyn AuditLog>`, `gen_bumper: Arc<dyn PolicyGenBumper>`, `ids: I`, `clock: C`; and `BootstrapAdminSeeder::new(deps: BootstrapAdminSeederDeps<I, C>) -> Self`. Task 3 uses the four new fields.

This task deliberately changes **no behavior** — `ensure_platform_admin` still calls `self.grants.grant(&grant)`. Separating the mechanical refactor from the semantic change means a reviewer can judge each on its own.

- [ ] **Step 1: Add the Deps struct and switch the constructor**

In `bootstrap_admin.rs`, add these imports to the existing `use` block:

```rust
use paigasus_iam_core::{AuditLog, Outbox, PolicyGenBumper, UnitOfWork};
```

Add the struct above `impl<I, C> BootstrapAdminSeeder<I, C>`, mirroring `RoleServiceDeps` (`application/roles.rs:120`):

```rust
/// Named-field constructor input, mirroring `RoleServiceDeps` (`application/roles.rs:120`)
/// and for the same reason: with eight dependencies — four of them `Arc<dyn …>` — positional
/// arguments let a reordering silently swap two same-typed values past the compiler.
pub struct BootstrapAdminSeederDeps<I, C> {
    pub admins_config: Vec<BootstrapAdmin>,
    pub grants: Arc<dyn RoleGrantStore>,
    /// SMA-468: the seed's grant, audit row and outbox event commit in ONE transaction, so
    /// the seeder owns a `UnitOfWork` rather than leaning on `RoleGrantStore::grant`'s
    /// internal one-shot wrapper.
    pub uow: Arc<dyn UnitOfWork>,
    pub outbox: Arc<dyn Outbox>,
    pub audit: Arc<dyn AuditLog>,
    /// SMA-468 D5: `grant_in` does NOT bump `policy_gen` (only the `grant` wrapper does), so
    /// the seeder must bump post-commit itself or a freshly seeded admin is denied until the
    /// snapshot's TTL backstop.
    pub gen_bumper: Arc<dyn PolicyGenBumper>,
    pub ids: I,
    pub clock: C,
}
```

Add the four fields to the `BootstrapAdminSeeder` struct itself, after `grants`:

```rust
    uow: Arc<dyn UnitOfWork>,
    outbox: Arc<dyn Outbox>,
    audit: Arc<dyn AuditLog>,
    gen_bumper: Arc<dyn PolicyGenBumper>,
```

Replace the `pub fn new(configured: &[BootstrapAdmin], grants: …, ids: I, clock: C)` signature and body with:

```rust
    #[must_use]
    pub fn new(deps: BootstrapAdminSeederDeps<I, C>) -> Self {
        let admins = deps
            .admins_config
            .iter()
            .filter_map(|admin| match Issuer::parse(&admin.issuer) {
                Ok(issuer) => Some((issuer, admin.subject.clone())),
                Err(e) => {
                    tracing::warn!(
                        issuer = %admin.issuer,
                        error = %e,
                        "authz.bootstrap_admins entry has an unparseable issuer (IamConfig::validate should have rejected this at boot) — skipping"
                    );
                    None
                }
            })
            .collect();
        Self {
            admins: Arc::new(admins),
            grants: deps.grants,
            uow: deps.uow,
            outbox: deps.outbox,
            audit: deps.audit,
            gen_bumper: deps.gen_bumper,
            ids: deps.ids,
            clock: deps.clock,
        }
    }
```

Keep the existing doc comment on `new` verbatim — its rationale about `IamConfig::validate` is unchanged.

- [ ] **Step 2: Update the composition root**

In `adapters/http/mod.rs`, replace line 454:

```rust
        let bootstrap_seeder = BootstrapAdminSeeder::new(&authz_cfg.bootstrap_admins, role_grant_store.clone(), KernelIdGenerator, SystemClock);
```

with:

```rust
        let bootstrap_seeder = BootstrapAdminSeeder::new(BootstrapAdminSeederDeps {
            admins_config: authz_cfg.bootstrap_admins.clone(),
            grants: role_grant_store.clone(),
            // SMA-468: the SAME uow/outbox/audit/gen_bumper instances `roles` uses, so a
            // seeded grant is written and audited exactly like an operator-issued one.
            uow: role_uow.clone(),
            outbox: role_outbox.clone(),
            audit: audit_log.clone(),
            gen_bumper: role_gen_bumper.clone(),
            ids: KernelIdGenerator,
            clock: SystemClock,
        });
```

Add `BootstrapAdminSeederDeps` to the existing `use crate::application::…` import for `BootstrapAdminSeeder`.

**If any of `role_uow` / `role_outbox` / `audit_log` / `role_gen_bumper` is not in scope at line 454** (it is constructed later in the function), move the `bootstrap_seeder` construction to just after the `roles` service is built rather than moving the other bindings — `bootstrap_seeder` is only read when `AppState` is assembled at the end. Confirm with `cargo check`.

- [ ] **Step 3: Update the existing unit tests' constructor calls**

In `bootstrap_admin.rs`'s `#[cfg(test)] mod tests`, replace the `seeder` helper:

```rust
    fn seeder(configured: &[BootstrapAdmin]) -> (BootstrapAdminSeeder<SeqIds, FixedClock>, GrantsBacking) {
        let grants = InMemoryRoleGrants::default();
        let backing = grants.0.clone();
        (BootstrapAdminSeeder::new(configured, Arc::new(grants), SeqIds::default(), FixedClock::default()), backing)
    }
```

with a version that also returns the new fakes' backings, since Task 3's tests assert on them:

```rust
    /// Everything a test needs to assert on: the seeder plus the backing stores of every
    /// fake it writes through.
    struct Harness {
        seeder: BootstrapAdminSeeder<SeqIds, FixedClock>,
        grants: GrantsBacking,
        events: Arc<std::sync::Mutex<Vec<paigasus_iam_core::DomainEvent>>>,
        entries: Arc<std::sync::Mutex<Vec<paigasus_iam_core::AuditEntry>>>,
        bumps: FakePolicyGenBumper,
    }

    fn seeder(configured: &[BootstrapAdmin]) -> Harness {
        let grants = InMemoryRoleGrants::default();
        let grants_backing = grants.0.clone();
        let outbox = FakeOutbox::default();
        let events = outbox.0.clone();
        let audit = FakeAuditLog::default();
        let entries = audit.0.clone();
        let bumps = FakePolicyGenBumper::default();
        let seeder = BootstrapAdminSeeder::new(BootstrapAdminSeederDeps {
            admins_config: configured.to_vec(),
            grants: Arc::new(grants),
            uow: Arc::new(FakeUnitOfWork),
            outbox: Arc::new(outbox),
            audit: Arc::new(audit),
            gen_bumper: Arc::new(bumps.clone()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        });
        Harness { seeder, grants: grants_backing, events, entries, bumps }
    }
```

Add to the test module's imports:

```rust
    use crate::application::fakes::{FakeAuditLog, FakeOutbox, FakePolicyGenBumper, FakeUnitOfWork};
```

Update every existing test to use the harness — e.g.
`let (seeder, backing) = seeder(&[]);` becomes `let h = seeder(&[]);` with `h.seeder` / `h.grants`. There are six existing tests; all keep their assertions unchanged.

- [ ] **Step 4: Verify nothing changed behaviourally**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib bootstrap_admin --no-tests=pass
cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
```

Expected: all six existing tests pass unchanged, clippy and fmt clean. If a test's *assertions* had to change, you changed behavior — revert and re-read Step 1.

- [ ] **Step 5: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add rs/crates/services/paigasus-iam/src/application/bootstrap_admin.rs rs/crates/services/paigasus-iam/src/adapters/http/mod.rs
git commit -m "refactor(rs): give the bootstrap seeder a named-field deps struct (SMA-468)"
```

---

### Task 2: The failure metric and its dashboard panel

**Files:**
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs`
- Modify: `ops/observability/grafana/dashboards/iam.json`

**Interfaces:**
- Consumes: nothing.
- Produces: `paigasus_observability::names::IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL` (a `&str` const equal to `"iam_bootstrap_admin_seed_failures_total"`). Task 3 increments it with a `stage` label.

**Why the panel is not optional (spec D6).** `repo:observability-drift` asserts that dashboards and rules reference only *known* metrics — it never asserts the converse (`paigasus-observability/tests/drift.rs:137`, `dashboards_and_rules_reference_only_known_metrics`). So a `names.rs` entry nothing references passes the gate, and so does omitting it. Without a panel the counter lands nowhere an operator looks, which is the exact "nobody is watching" failure D6 exists to fix.

- [ ] **Step 1: Register the metric name**

In `rs/crates/libs/paigasus-observability/src/names.rs`, add the const next to the other IAM authz/audit names (after `IAM_DENIAL_AUDITS_ENQUEUED_TOTAL`):

```rust
/// SMA-468: a bootstrap-admin seed attempt that failed and was swallowed. `stage="list"` is
/// the pre-seed existence check, `stage="txn"` the grant+audit+event transaction. A lost
/// `policy_gen` bump is NOT counted — `PolicyGenBumper::bump` returns `()` and swallows
/// internally, so it is structurally invisible here. A low nonzero value is not necessarily
/// pathological: two concurrent first authentications by the same admin race, and the loser
/// rolls back on the unique constraint with the net state still correct.
pub const IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL: &str = "iam_bootstrap_admin_seed_failures_total";
```

Add it to the `ALL` array, after `IAM_DENIAL_AUDITS_ENQUEUED_TOTAL`:

```rust
    IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL,
```

- [ ] **Step 2: Add the dashboard panel**

`ops/observability/grafana/dashboards/iam.json` currently has 15 panels with a maximum `gridPos.y` of 48 (`schemaVersion: 39`). Append a panel to the `panels` array, matching the shape of the existing "Denial-audit drops" panel:

```json
{
  "title": "Bootstrap-admin seed failures",
  "type": "timeseries",
  "gridPos": { "h": 8, "w": 12, "x": 0, "y": 56 },
  "targets": [
    {
      "expr": "sum(rate(iam_bootstrap_admin_seed_failures_total[$__rate_interval])) by (stage)",
      "legendFormat": "{{stage}}"
    }
  ]
}
```

Copy the `datasource`, `id`, `fieldConfig` and `options` keys from the neighbouring "Denial-audit drops" panel so the new panel is structurally identical to its siblings; give it an `id` one higher than the current maximum. Do not hand-edit unrelated panels.

- [ ] **Step 3: Verify the drift gate now covers the metric**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
jq -e '.panels | map(select(.title == "Bootstrap-admin seed failures")) | length == 1' ops/observability/grafana/dashboards/iam.json
moon run repo:observability-drift
```

Expected: `jq` prints `true`, gate passes.

- [ ] **Step 4: Prove the gate is actually load-bearing now**

A gate never seen red proves nothing. Temporarily break the registration and confirm the drift gate fails, then restore:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
perl -pi -e 's/^\s*IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL,\n//' rs/crates/libs/paigasus-observability/src/names.rs
moon run repo:observability-drift --force   # expect FAIL naming the metric
git checkout -- rs/crates/libs/paigasus-observability/src/names.rs
moon run repo:observability-drift --force   # expect PASS
git status --porcelain rs/crates/libs/paigasus-observability/src/names.rs
```

Expected: FAIL, then PASS, then empty `git status` output for that file. If the first run *passes*, the panel is not being read — recheck Step 2's placement inside the `panels` array.

- [ ] **Step 5: Document how to actually retrieve the audit row**

Spec D3 requires this and nothing else in the plan covers it. The row is **not** findable the
obvious way: `actor_prn` is null and `AuditFilter` has no filter for a null actor nor for
`detail` (`audit.rs:44-53`; neither transport exposes one). Worse, `PgAuditLog::query` applies
a default lookback when both `from` and `to` are absent (`pg_audit_log.rs:167-181`) and
`audit.query_default_window_days` defaults to **90** (`config.rs:482`) — and a seed happens
once per fresh database, so 90 days later the unfiltered query returns nothing.

Add a short subsection to `docs/ops/RUNBOOK-observability.md` under the existing
`### Audit retention & partitioning` heading (line ~982) stating:

- The bootstrap `platform_admin` grant is audited as `action="GrantRole"` with
  `resource_prn` = the Root PRN and `actor_prn` **null** — null because operator
  configuration, not a principal, authorized it (SMA-468 D2).
- The grantee is in `detail.principal_prn`, and `detail.source = "bootstrap_admins"`
  distinguishes it from an operator-issued grant.
- **Retrieval requires an explicit `from`.** Query `action=GrantRole` +
  `resource_prn=<root prn>` with `from` set at or before the deployment date; without it the
  90-day default window silently excludes a row written once at cold start.
- If `audit.retention.committed_months` is set to a nonzero value, this row is eventually
  dropped and **is not reproducible** — the seed is idempotent and never re-runs.

- [ ] **Step 6: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add rs/crates/libs/paigasus-observability/src/names.rs ops/observability/grafana/dashboards/iam.json docs/ops/RUNBOOK-observability.md
git commit -m "feat(repo): add a bootstrap-admin seed-failure counter and panel (SMA-468)"
```

---

### Task 3: The transactional write path

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/bootstrap_admin.rs`

**Interfaces:**
- Consumes: `BootstrapAdminSeederDeps` (Task 1); `names::IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL` (Task 2).
- Produces: no new public API. `ensure_platform_admin`'s signature and `()` return are unchanged.

This is the core deliverable. Write the tests first.

- [ ] **Step 1: Write the failing tests**

Add to `bootstrap_admin.rs`'s `#[cfg(test)] mod tests`. These need one new local fake — a `RoleGrantStore` whose `grant_in` errors, copying the shape at `roles.rs:408`:

```rust
    /// A `RoleGrantStore` that fails on the FIRST write step. Deliberately errors on
    /// `grant_in` rather than on the audit write: `FakeUnitOfWork` has no real transaction
    /// (`fakes.rs:893-903` — the fakes ignore `tx` and mutate immediately), so a failure on
    /// the LAST step would leave the grant already in the map and could not prove rollback.
    /// True atomicity is proven against Postgres in `tests/authz_bootstrap_admin.rs`.
    #[derive(Default)]
    struct FailingGrants;

    #[async_trait::async_trait]
    impl RoleGrantStore for FailingGrants {
        async fn list_by_principal(&self, _p: &PrincipalId) -> Result<Vec<RoleGrant>, AuthzError> {
            Ok(Vec::new())
        }
        async fn grant_in(&self, _tx: &dyn paigasus_iam_core::Transaction, _g: &RoleGrant) -> Result<(), AuthzError> {
            Err(AuthzError::Backend(Box::new(std::io::Error::other("simulated mid-txn store failure"))))
        }
        async fn grant(&self, _g: &RoleGrant) -> Result<(), AuthzError> {
            unimplemented!("the seeder only uses grant_in")
        }
        async fn revoke(&self, _id: Uuid) -> Result<(), AuthzError> {
            unimplemented!("the seeder never revokes")
        }
        async fn revoke_in(&self, _tx: &dyn paigasus_iam_core::Transaction, _id: Uuid) -> Result<bool, AuthzError> {
            unimplemented!("the seeder never revokes")
        }
        async fn list_all(&self) -> Result<Vec<RoleGrant>, AuthzError> {
            Ok(Vec::new())
        }
        async fn find(&self, _id: Uuid) -> Result<Option<RoleGrant>, AuthzError> {
            unimplemented!("the seeder never looks up by id")
        }
    }

    fn admin_cfg() -> Vec<BootstrapAdmin> {
        vec![BootstrapAdmin {
            issuer: "https://idp.example.com".to_string(),
            subject: "sub-admin".to_string(),
        }]
    }

    /// Test 1 — the audit row is correct and, crucially, SELF-DESCRIBING. With
    /// `actor_prn: None` and `resource_prn` set to the SCOPE, `principal_prn` in `detail` is
    /// the only thing naming who actually became platform admin (SMA-468 D4).
    #[tokio::test]
    async fn the_seeded_grant_writes_a_self_describing_audit_row() {
        let h = seeder(&admin_cfg());
        let p = principal(1);
        h.seeder.ensure_platform_admin(&p, &issuer("https://idp.example.com"), "sub-admin").await;

        let entries = h.entries.lock().unwrap();
        assert_eq!(entries.len(), 1, "a seeded grant must write exactly one audit entry");
        let e = &entries[0];
        assert_eq!(e.action, "GrantRole", "SMA-468 D3: reuse the standard action so the row appears in the standard query");
        assert_eq!(e.actor_prn, None, "SMA-468 D2: no principal authorized this — configuration did");
        assert_eq!(e.outcome, AuditOutcome::Committed);
        assert_eq!(e.resource_prn.as_deref(), Some(root_prn().canonical().as_str()));
        assert_eq!(
            e.detail["principal_prn"], serde_json::json!(p.canonical()),
            "SMA-468 D4: with a null actor this is the ONLY field naming the grantee"
        );
        assert_eq!(e.detail["source"], serde_json::json!("bootstrap_admins"));
        assert_eq!(e.detail["issuer"], serde_json::json!("https://idp.example.com"));
        assert_eq!(e.detail["role_key"], serde_json::json!("platform_admin"));
    }

    /// Test 2 — the IdP `subject` must appear in NEITHER artifact (SMA-468 D4). `audit_log`
    /// is append-only and designed to outlive the rows it describes, so an external
    /// identifier written here cannot be removed under an erasure request. Asserted over the
    /// serialized forms so a nested placement cannot slip through.
    #[tokio::test]
    async fn neither_artifact_carries_the_idp_subject() {
        let h = seeder(&admin_cfg());
        h.seeder.ensure_platform_admin(&principal(1), &issuer("https://idp.example.com"), "sub-admin").await;

        let detail = h.entries.lock().unwrap()[0].detail.to_string();
        let payload = h.events.lock().unwrap()[0].payload.to_string();
        assert!(!detail.contains("sub-admin"), "the IdP subject must not reach audit_log: {detail}");
        assert!(!payload.contains("sub-admin"), "the IdP subject must not cross the outbox boundary: {payload}");
    }

    /// Test 3 — control flow: a failure on the FIRST write step must stop everything after
    /// it. This is what the in-memory fakes can honestly prove; see `FailingGrants`.
    #[tokio::test]
    async fn a_failed_grant_write_stops_the_event_the_audit_row_and_the_bump() {
        let bumps = FakePolicyGenBumper::default();
        let outbox = FakeOutbox::default();
        let events = outbox.0.clone();
        let audit = FakeAuditLog::default();
        let entries = audit.0.clone();
        let seeder = BootstrapAdminSeeder::new(BootstrapAdminSeederDeps {
            admins_config: admin_cfg(),
            grants: Arc::new(FailingGrants),
            uow: Arc::new(FakeUnitOfWork),
            outbox: Arc::new(outbox),
            audit: Arc::new(audit),
            gen_bumper: Arc::new(bumps.clone()),
            ids: SeqIds::default(),
            clock: FixedClock::default(),
        });

        seeder.ensure_platform_admin(&principal(1), &issuer("https://idp.example.com"), "sub-admin").await;

        assert!(events.lock().unwrap().is_empty(), "no event may be enqueued once the grant write failed");
        assert!(entries.lock().unwrap().is_empty(), "no audit row may be written once the grant write failed");
        assert_eq!(bumps.calls(), 0, "SMA-468 D5: the post-commit bump must not run for a transaction that never committed");
    }

    /// Test 4 — the D5 regression guard. `grant_in` does NOT bump `policy_gen` (only the
    /// `RoleGrantStore::grant` wrapper this replaced did), so without an explicit post-commit
    /// bump a freshly seeded admin is denied until the snapshot's ~31s TTL backstop.
    #[tokio::test]
    async fn a_successful_seed_bumps_policy_gen_exactly_once() {
        let h = seeder(&admin_cfg());
        h.seeder.ensure_platform_admin(&principal(1), &issuer("https://idp.example.com"), "sub-admin").await;
        assert_eq!(h.bumps.calls(), 1, "SMA-468 D5: a seeded grant must invalidate the policy snapshot immediately");
    }

    /// Test 5 — idempotence now covers the new artifacts too, not just the grant row.
    #[tokio::test]
    async fn a_second_authentication_writes_no_second_audit_row_or_event() {
        let h = seeder(&admin_cfg());
        let p = principal(1);
        let iss = issuer("https://idp.example.com");
        h.seeder.ensure_platform_admin(&p, &iss, "sub-admin").await;
        h.seeder.ensure_platform_admin(&p, &iss, "sub-admin").await;

        assert_eq!(h.grants.lock().unwrap().len(), 1);
        assert_eq!(h.entries.lock().unwrap().len(), 1, "idempotent: the second authentication must not re-audit");
        assert_eq!(h.events.lock().unwrap().len(), 1, "idempotent: the second authentication must not re-emit");
        assert_eq!(h.bumps.calls(), 1, "idempotent: the second authentication must not re-bump");
    }

    /// Test 6 — the fast path is untouched: a non-configured identity produces nothing at all.
    #[tokio::test]
    async fn a_non_configured_identity_writes_no_audit_row_or_event() {
        let h = seeder(&[]);
        h.seeder.ensure_platform_admin(&principal(1), &issuer("https://idp.example.com"), "sub-1").await;
        assert!(h.grants.lock().unwrap().is_empty());
        assert!(h.entries.lock().unwrap().is_empty());
        assert!(h.events.lock().unwrap().is_empty());
        assert_eq!(h.bumps.calls(), 0);
    }
```

Add to the test module imports:

```rust
    use paigasus_iam_core::authz::model::root_prn;
    use paigasus_iam_core::{AuditOutcome, AuthzError};
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib bootstrap_admin --no-tests=pass
```

Expected: **FAIL**. Tests 1, 2, 5 fail because no audit entry or event is written yet (index-out-of-bounds or a length assertion); test 4 fails with `0 != 1` because nothing bumps.

- [ ] **Step 3: Add the error type**

In `bootstrap_admin.rs`, above the `impl` block:

```rust
/// Why a seed attempt failed. Deliberately a local enum rather than funnelling through
/// `TenancyError`: `From<AuthzError> for TenancyError` collapses `Backend` into
/// `TenancyError::Internal`, whose `Display` is the constant `"internal server error"`
/// (`application/error.rs`). That would destroy the one diagnostic explaining WHY the
/// bootstrap admin was never seeded — the Postgres constraint name in the source error
/// (SMA-468 D7).
#[derive(Debug, thiserror::Error)]
enum SeedError {
    #[error(transparent)]
    Repository(#[from] paigasus_iam_core::RepositoryError),
    #[error(transparent)]
    Authz(#[from] paigasus_iam_core::AuthzError),
}
```

- [ ] **Step 4: Write the transactional seed helper**

Add this private method inside `impl<I, C> BootstrapAdminSeeder<I, C>`:

```rust
    /// The write half of a seed: the grant, its outbox event and its audit row commit in ONE
    /// transaction, then an awaited best-effort `policy_gen` bump post-commit — the same
    /// shape as `RoleService::grant` (`application/roles.rs:244-252`), which this cannot
    /// reuse because that method authorizes the caller first and bootstrap exists precisely
    /// to precede any authority.
    async fn seed_grant(&self, grant: &RoleGrant, issuer: &Issuer) -> Result<(), SeedError> {
        let corr = self.ids.new_correlation_id();
        let event = DomainEvent {
            id: self.ids.new_event_id(),
            event_type: EventType::RoleGranted,
            schema_version: 1,
            aggregate_prn: grant.principal.canonical(),
            // SMA-468 D2: no principal authorized this — operator configuration did.
            actor_prn: None,
            occurred_at: grant.created_at,
            // SMA-468 D4: PII-minimal — this crosses the outbox to an external broker, so it
            // carries neither the issuer nor the IdP subject.
            payload: serde_json::json!({
                "grant_id": grant.id,
                "role_key": grant.role_key,
                "scope": grant.scope.canonical_prn(),
                "source": "bootstrap_admins",
            }),
            correlation_id: Some(corr),
        };
        let entry = AuditEntry {
            id: self.ids.new_audit_id(),
            occurred_at: grant.created_at,
            actor_prn: None,
            action: "GrantRole".into(),
            resource_prn: Some(root_prn().canonical()),
            outcome: AuditOutcome::Committed,
            determining_policies: vec![],
            // SMA-468 D4: `principal_prn` is the ONLY field naming the grantee, since the
            // actor is null and `resource_prn` is the scope. The `issuer` gives provenance;
            // the IdP `subject` is deliberately absent (append-only table, erasure).
            detail: serde_json::json!({
                "principal_prn": grant.principal.canonical(),
                "grant_id": grant.id,
                "role_key": grant.role_key,
                "scope": grant.scope.canonical_prn(),
                "source": "bootstrap_admins",
                "issuer": issuer.as_str(),
            }),
            correlation_id: Some(corr),
        };

        let tx = self.uow.begin().await?;
        self.grants.grant_in(&*tx, grant).await?;
        self.outbox.enqueue(&*tx, &event).await?;
        self.audit.record(&*tx, &entry).await?;
        tx.commit().await?;

        // SMA-468 D5: `grant_in` does NOT bump (only `RoleGrantStore::grant` did), so this is
        // load-bearing, not polish — without it a freshly seeded admin is denied until the
        // policy snapshot's TTL backstop (~31s at defaults).
        self.gen_bumper.bump().await;
        Ok(())
    }
```

Add these imports to the module's `use` block:

```rust
use paigasus_iam_core::authz::model::root_prn;
use paigasus_iam_core::{AuditEntry, AuditOutcome, DomainEvent, EventType};
use metrics::counter;
use paigasus_observability::names;
```

- [ ] **Step 5: Call it, and count both failure stages**

Replace the tail of `ensure_platform_admin` — the `if let Err(e) = self.grants.grant(&grant).await { … }` block — with:

```rust
        if let Err(e) = self.seed_grant(&grant, issuer).await {
            counter!(names::IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL, "stage" => "txn").increment(1);
            tracing::warn!(
                principal = %principal.canonical(),
                error = %e,
                "bootstrap-admin seeding: failed to persist the platform_admin grant with its audit row; will retry on the next authentication. If this persists the bootstrap admin is NEVER seeded (lockout) — seed it manually and record the matching audit row"
            );
        }
```

And add the `list` stage counter to the **existing** `list_by_principal` failure arm, which is a swallowed seed failure that has never been counted:

```rust
            Err(e) => {
                counter!(names::IAM_BOOTSTRAP_ADMIN_SEED_FAILURES_TOTAL, "stage" => "list").increment(1);
                tracing::warn!(
                    principal = %principal.canonical(),
                    error = %e,
                    "bootstrap-admin seeding: failed to list existing role grants; will retry on the next authentication"
                );
                return;
            }
```

Add `paigasus-observability` to `rs/crates/services/paigasus-iam/Cargo.toml` `[dependencies]` **only if it is not already there** — check first with `grep -n paigasus-observability rs/crates/services/paigasus-iam/Cargo.toml`; it is already a dependency for the existing metrics, so no change is expected.

- [ ] **Step 6: Update the module doc**

The module doc's `BootstrapAdminSeeder` paragraph currently claims the bump comes free from `RoleGrantStore::grant`:

> `AppState` clones hold the SAME `Arc<dyn RoleGrantStore>` the rest of the composition root shares, so a seeded grant bumps the identical `policy_gen` counter `CedarAuthorizer` polls (mirrors `RoleService::grant`'s wiring, and `PgRoleGrantStore::grant`'s own doc contract: "inserts the grant row and bumps the policy generation counter").

That is now false — `grant_in` does not bump. Replace the parenthetical so it states the seeder bumps explicitly post-commit via its own `PolicyGenBumper`, exactly as `RoleService::grant` does, and note that this is required precisely *because* `grant_in` (unlike `grant`) does not.

- [ ] **Step 7: Run the tests to verify they pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --lib bootstrap_admin --no-tests=pass
cargo clippy -p paigasus-iam --all-targets -- -D warnings && cargo fmt --check
```

Expected: **12 passed** (6 existing + 6 new), clippy and fmt clean.

- [ ] **Step 8: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add rs/crates/services/paigasus-iam/src/application/bootstrap_admin.rs
git commit -m "fix(rs): audit the bootstrap-admin grant atomically with its write (SMA-468)"
```

---

### Task 4: Postgres atomicity test

**Files:**
- Modify: `rs/crates/services/paigasus-iam/tests/authz_bootstrap_admin.rs`

**Interfaces:**
- Consumes: the behavior from Task 3.
- Produces: nothing consumed by later tasks.

This proves the one property the in-memory fakes structurally cannot (`fakes.rs:893-903`): that a failed audit write leaves **no** `role_grant` row. It is AC1's real verification.

**How to force the failure.** The test drives the real `AppState::new(db, &cfg)`, so there is no fake to inject. Instead make the audit insert fail at the database: rename `audit_log` out from under it after migration. Each test gets its own ephemeral Postgres (`support::start_migrated_postgres`), so this is contained.

- [ ] **Step 1: Write the failing test**

Append to `tests/authz_bootstrap_admin.rs`:

```rust
/// SMA-468 AC1, the half the unit suite cannot prove: the seed's grant, audit row and outbox
/// event are ONE transaction, so a failing audit write must leave NO `role_grant` row.
///
/// `application/bootstrap_admin.rs`'s unit tests use `FakeUnitOfWork`, which has no real
/// transaction — its own doc says the fakes ignore `tx` and mutate immediately, so a failure
/// on the LAST step there leaves the grant already written. Only Postgres can prove rollback.
///
/// The audit write is forced to fail by renaming `audit_log` after migration (this test owns
/// its own ephemeral database). The request itself must still succeed: seeding is best-effort
/// and must never fail the request that triggered it.
#[tokio::test]
async fn a_failed_audit_write_rolls_back_the_bootstrap_grant() {
    let Some((_node, db)) = support::start_migrated_postgres().await else {
        return;
    };
    let idp = support::start_mock_idp().await;
    let mut cfg = test_config_with(&[(&idp, true)], 30);
    cfg.authz.bootstrap_admins = vec![BootstrapAdmin {
        issuer: idp.issuer.clone(),
        subject: "bootstrap-sub".to_string(),
    }];
    let (app, state) = app_with_config(db.clone(), &cfg).await;

    // Make every `audit_log` insert fail. Renaming the partitioned parent takes its children
    // with it, so the seeder's `audit.record` errors and its transaction rolls back.
    use sea_orm::ConnectionTrait;
    db.execute_unprepared("ALTER TABLE audit_log RENAME TO audit_log_hidden")
        .await
        .expect("rename audit_log");

    let token = idp.bearer("bootstrap-sub", Some("bootstrap-admin@example.com"), "paigasus", 3600);
    // Any authenticated route: the seeder runs in the bearer middleware, before the handler.
    let (status, body) = send(&app, "GET", "/v1/organizations", None, Some(&token)).await;
    assert_ne!(
        status,
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "seeding is best-effort: a failed audit write must never turn into a 500: {body}"
    );

    // Restore the table so the assertion below can use the normal query path.
    db.execute_unprepared("ALTER TABLE audit_log_hidden RENAME TO audit_log")
        .await
        .expect("restore audit_log");

    // The principal exists (JIT-provisioned by authn) but must hold NO platform_admin grant:
    // the audit failure rolled the whole seed back.
    let principal = state.authn.resolve(&token, Provisioning::Disabled).await.expect("already provisioned");
    let grants = state.role_grant_store.list_by_principal(&principal.principal_id).await.expect("list_by_principal");
    assert!(
        !grants.iter().any(|g| g.role_key == "platform_admin" && g.scope == GrantScope::Root),
        "SMA-468 AC1: a failed audit write must leave no platform_admin grant behind, got {grants:?}"
    );
}
```

Note `principal.principal_id` is a **field**, not a method — copied from the existing test at
`authz_bootstrap_admin.rs:62`, which reaches grants the same way (`state.role_grant_store`).
`DatabaseConnection` is `Clone` (it is an `Arc`-backed pool handle), so `db.clone()` above is
fine.

- [ ] **Step 2: Run it against the pre-fix code to confirm it would have caught the bug**

This test must fail on `main`'s behavior. Verify by temporarily reverting just the write path:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
git stash push rs/crates/services/paigasus-iam/src/application/bootstrap_admin.rs
cd rs && cargo nextest run -p paigasus-iam --test authz_bootstrap_admin a_failed_audit_write --no-tests=pass
cd .. && git stash pop
```

Expected: with the old (non-transactional) seeder the grant is written regardless of audit, so the test **FAILS**. If Docker is unavailable the test self-skips and this check is inconclusive — say so in the report rather than claiming it passed.

- [ ] **Step 3: Run it against the real code**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo nextest run -p paigasus-iam --test authz_bootstrap_admin --no-tests=pass
```

Expected: all tests in the file pass, including the pre-existing
`bootstrap_identity_is_seeded_platform_admin_on_first_authentication_and_can_create_an_organization`, which is the real end-to-end guard that D5's bump still works.

- [ ] **Step 4: Commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
git add rs/crates/services/paigasus-iam/tests/authz_bootstrap_admin.rs
git commit -m "test(rs): prove a failed audit write rolls back the bootstrap grant (SMA-468)"
```

---

### Task 5: Full-graph verification

**Files:** none modified (verification only; fix-ups land wherever the gates point).

**Interfaces:**
- Consumes: Tasks 1-4.
- Produces: a branch ready for a PR.

Per-project Moon tasks do **not** run the repo-level gates. This runs the graph the way CI does.

- [ ] **Step 1: Verify the commit messages against the CI parity gate**

Do this *before* the graph — it is the gate that reds late and needs a force-push to fix:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon run ts:commitlint -- --from $(git merge-base origin/main HEAD) --to HEAD; echo "exit=$?"
```

Expected: `exit=0`. If not, amend the offending message (look for a body line starting `word:`).

- [ ] **Step 2: Run the full CI gate graph**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
moon ci :build :test :lint :fmt :deny :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :wasm-getrandom-free :redis-connect-single-site :promtool \
  :observability-drift :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

Expected: all green. Diagnose an unattributed failure with:

```bash
jq '.actions[] | select(.status=="failed")' .moon/cache/ciReport.json
```

No new crates or dependencies are added, so `:deny` and `:machete` need no waivers. `:observability-drift` is the one gate this change genuinely exercises (Task 2).

- [ ] **Step 3: Confirm the diff matches the plan**

```bash
git diff origin/main --stat
git diff origin/main -- '*.rs' | grep -nE '^\+.*(dbg!|todo!|unimplemented!\("(?!the seeder)|eprintln!|println!)' || echo "(no stray debug code)"
```

Expected files only: `bootstrap_admin.rs`, `http/mod.rs`, `names.rs`, `iam.json`, `RUNBOOK-observability.md`, `authz_bootstrap_admin.rs`, plus the spec and this plan. The `unimplemented!` calls inside `FailingGrants` are intentional (unreached trait methods) and match the existing fake convention.

---

## Notes for the implementer

**Do not skip the red states.** Task 2 Step 4 (break the metric registration, watch the drift gate fail), Task 3 Step 2 (failing unit tests) and Task 4 Step 2 (the test failing against pre-fix behavior) each exist because a guard that has never failed proves nothing. Task 4 Step 2 is the most important of the three: it is the only evidence that the atomicity test tests atomicity.

**The single highest-risk line is `self.gen_bumper.bump().await`.** Everything else in this change is additive; that one line replaces behavior `RoleGrantStore::grant` used to provide. Omit it and every test still passes except Task 3's test 4, while a freshly seeded platform admin is silently denied for ~31 seconds.

**What must NOT change.** `ensure_platform_admin` keeps its signature and its `()` return; every failure stays logged-and-swallowed; the `HashSet` fast path still returns before any store or transaction work; and the seeder is still never called from the `introspect` path.
