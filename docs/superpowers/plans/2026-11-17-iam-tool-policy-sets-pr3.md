# IAM tool policy sets, PR 3 (SMA-781) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the tool policy re-compile job and the held versions: a job that compiles each out-of-date live set again with the running compiler, applies an identical result, and holds a different or failed result for an admin; plus the four RPCs `ListHeldToolPolicyVersions`, `AcceptHeldToolPolicyVersion`, `DiscardHeldToolPolicyVersion` and `RunToolPolicyRecompile`.

**Architecture:** PR 1 made the `held` and `discarded` statuses, the `base_version` and `report` columns and the one-held partial index (PR 1 D10). This PR adds one migration (m0015) for a per-set version counter and a nullable `compiled` column, a pure diff report in `paigasus-iam-core::tool_policy`, held-row methods on the existing `ToolPolicyRepository` port, a `RecompileLock` port with a Postgres advisory-lock adapter, and two new application modules (`held.rs`, `recompile.rs`) on the existing `ToolPolicyService`. The job reuses the PR 1 `compile` helper, the one comparator `outputs_identical` (spec §8.1) and the PR 2 events. It runs once at boot in a background task and on demand through `RunToolPolicyRecompile`.

**Tech Stack:** Rust 2024 / 1.95, tonic + prost via buf, SeaORM migrations, Postgres advisory locks, axum, `metrics`, `cedar-policy` 4.14 (IAM side only), Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-11-15-iam-tool-policy-sets-design.md` (PR 3 of the §3.2 split: §3.5, §4.4 held rows, §4.6 recompile-pending, §7, §8, §8.1, §9.1 PR 3 RPCs, §9.5 `ManageToolPolicyRecompile`, §9.6, §10 PR 3 tests, §11, §12). PR 1 plan: `docs/superpowers/plans/2026-11-15-iam-tool-policy-sets-pr1.md` (D1-D10). PR 2 plan: `docs/superpowers/plans/2026-11-16-iam-tool-policy-sets-pr2.md` (R1-R19). This plan keeps every one of those rulings.

**Closing the parent.** This is the only PR of the stack that closes the parent issue (spec §3.2). The PR body says so in words. It does not write the parent key: the coordinator closes the parent in Linear after the merge.

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for shell, TOML, YAML).
- Rust edition 2024, rust-version 1.95. `warnings = "deny"`, and `cargo clippy --workspace --all-targets -- -D warnings` is a hard gate. No task adds an item that the same task does not use or test. Do not stage "add now, wire later".
- Hexagonal architecture in `paigasus-iam`: pure logic and ports in `paigasus-iam-core`, I/O in adapters, use cases in `application/`. Each new collaborator comes in through a port and a constructor (dependency injection), with a fake in `application/fakes.rs`.
- `rs/Cargo.toml` `members` stay literal paths (SMA-663). This PR adds no crate.
- Every cargo invocation in a Moon task passes `--locked`.
- A `.proto` edit is followed by `buf format -w` and `moon run contracts:generate --force` in the same task. Commit all generated Rust, Python and TS output with it. Only new messages, enum values and RPCs: `contracts:fmt`, `contracts:breaking`, the codegen-drift step and the proto TS registry-count tests stay green. In a worktree, pass `git rev-parse --git-common-dir` to `--against` (memory `buf-breaking-worktree-gitlink`).
- New error code: number 63 `tool-policy-held-not-acceptable` in the IAM block (1-299) of `contracts/proto/paigasus/common/v1/error.proto`. Numbers 47-62 are used; 63 is the next free number. Register it in `rs/crates/libs/paigasus-proto/src/error.rs` (the wire list and its test), in `ts/packages/paigasus-sdk/src/errors/presentation.ts`, and in the `TenancyError` membership test. Every new file that spells a registry code gets a `MANIFEST` row in `ci/error-registry/check.py`. This plan puts every new code through `TenancyError::code()` or `RejectionReason::code()`, which already have rows, so it expects no new row. `repo:error-code-single-site` must stay green.
- `STARTER_POLICY_REVISION` goes from 6 to 7 in Task 2 only, with the new `EXPECTED_STARTER_CONTENT_HASH`.
- Commits: conventional, with a workspace scope (`feat(rs)`, `feat(contracts)`, `test(rs)`, `docs(docs)`). Every subject ends with `(SMA-781)` and has at most 100 characters. The body is only the trailer paragraph `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No other body line starts with `word:` or `#NNN` (commitlint `footer-leading-blank`).
- Each "Commit" step: `git add` exactly the files in that task's **Files** block (plus generated output and each lockfile that the task's cargo runs changed), then `git commit -m "<subject>" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"`. Commits are signed through 1Password. If signing fails with "failed to fill whole buffer", stop and report that 1Password is locked. If the hook reports `commitlint not found`, provision the worktree (root `CLAUDE.md`).
- The plan, the commits, the PR title and the PR body name this PR's issue only as SMA-781. Refer to the parent only as "the parent issue". Never write the parent key.
- No `--no-verify`, no `git commit --amend`, no `git reset`, no rebase, no bare `git stash`. Do not hand-edit `.github/CODEOWNERS`. Do not touch or copy the `ci-targets` or `moon-diagnosis` marker blocks in any `CLAUDE.md`.
- Do not install host software. Use `docker run ubuntu:24.04` or `rust:1.95.0-bookworm` for a Linux-only check.
- Put `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` in front of every `moon`, `buf`, `uv` or `cargo nextest` command.
- Docker suites run with `PAIGASUS_REQUIRE_DOCKER=1` on every filtered run, so a skip is a failure. Docker-gated IAM tests use `support::start_migrated_postgres()` and never read `CI`.
- Keep test runs narrow: each task names a nextest filter or a `--test` binary. Run the full crate only in Task 11.
- A mutation proves something only when the mutant compiles. Run each mutation with `--no-fail-fast` (memory `mutation-must-compile-to-prove-anything`).
- Values pinned by the spec: statuses `live`, `superseded`, `held`, `discarded`; origins `recompile` and `accept`; job results `identical`, `held_different`, `held_failed`; one live row and at most one held row for each set; `ManageToolPolicyRecompile` for `platform_admin` only; accept and discard of a floor row need `ManageToolPolicyFloor`.

## Review Focus

1. **An admin discards a held row, and IAM restarts.** Expect the job not to make the same held row again for the same live version and the same compiler values (H6, Q1). Test: Task 7 `a_discarded_proposal_is_not_held_again_on_the_next_run`.
2. **A save, an accept or a catalog write commits while the job compiles a set.** Expect no held row and no new version from the job for that set; the result is `conflict`, and the next run sees the new live version. Test: Task 7 `a_save_between_the_read_and_the_lock_is_a_conflict_and_writes_nothing`.
3. **Two replicas boot at the same time with a new compiler.** Expect one run; the other reports `AlreadyRunning` and writes nothing. Test: Task 6 `a_second_try_acquire_gets_none_until_the_first_guard_is_released`, Task 9 `two_concurrent_runs_write_one_held_row`.
4. **An old replica serves a write during a rolling deploy after a new replica stored a held row.** The old binary does not know `last_version`. Expect the new replica to take the next free number from `max(live_version, last_version) + 1` and never collide on the version index (H2). Test: Task 4 `the_next_version_skips_a_number_that_an_old_replica_wrote`.
5. **A held row whose compile failed.** Expect accept to refuse with `tool-policy-held-not-acceptable`, discard to work, and catalog writes to stay `tool-policy-recompile-pending` until a save. Test: Task 5 `accepting_a_failed_held_row_is_held_not_acceptable`, `after_a_discard_catalog_writes_stay_recompile_pending`.

---

## Pre-plan: rulings

The merged code (PR 1 c44bab5c, PR 2 bd0e0379) and the spec leave the choices below open. Each ruling is a call that this plan makes. The coordinator can overturn any of them before Task 1. The prefix H keeps them apart from PR 1 D1-D10 and PR 2 R1-R19.

- **H1. Held rows share the version numbers of the set.** Spec §4.4 says a held row has a version, and §8 says accept copies the output "to a new version number". So a held row takes the next number, and accept takes the number after it. Live versions can then have gaps (v4 live, v5 held, v6 accepted). The gateway reads the version only as a token, so a gap has no effect. Cost if wrong: one migration column and the version CAS change back.
- **H2. m0015 adds `tool_policy_state.last_version`.** The next number of a set is `max(live_version, last_version) + 1`. The `max` is on purpose: during a rolling deploy, a PR 2 replica bumps only `live_version`. The insert CAS becomes `WHERE GREATEST(live_version, last_version) = $new - 1`, and it sets both columns. The backfill is `MAX(version)` of each set. Cost if wrong: none known. Accepted cost of the rollout: a PR 2 replica numbers a save `live_version + 1` and does not read `last_version`. While a set has a held or discarded row at that number, every save of that set on a PR 2 replica hits the version unique index and fails with `Internal` (the PR 1 `map_err` mapping of `ConflictKind::Other`), not only one write. A discard does not end this, because the discarded row keeps its number. It ends when a new replica writes a new live version of the set (a save or an accept), or when the PR 2 replicas are gone. The plan accepts this window, because it lasts only for the rollout; `docs/ops/RUNBOOK-tool-policies.md` ("Rolling deploy and mixed replicas") says the same.
- **H3. The write recheck compares `live_version` and `catalog_revision` only.** Spec §4.5 step 3 names these two. The PR 1 and PR 2 code compares the whole `ToolPolicyState`, which now holds `last_version` too. A held row from the job must not make a concurrent admin save a conflict. The number for the write comes from the locked state. Cost if wrong: a save during a job run fails once with a version conflict.
- **H4. m0015 makes `compiled` nullable for a failed result.** A `held_failed` row has no compile output. The CHECK is `compiled IS NOT NULL OR status IN ('held','discarded')`, and a second CHECK is `status <> 'held' OR (base_version IS NOT NULL AND report IS NOT NULL)`. No sentinel bytes. Cost if wrong: a migration column.
- **H5. Held rows have their own Rust type.** `HeldVersion` (with `HeldStatus::{Held, Discarded}`, `compiled: Option<Vec<u8>>`, `base_version`, `report`) is separate from `ToolPolicyVersion`. `VersionStatus` stays `Live | Superseded`, so no existing `match` and no fetch path can see a held row. `VersionOrigin` gains `Recompile` and `Accept`. Cost if wrong: none known.
- **H6. A discard sticks (Q1).** The job skips a set when a held OR discarded row already exists with the same `base_version`, the running `compiler_generation` and the running `schema_revision`. Spec §8 names only the held row. Without this rule, each boot makes again the held row that an admin discarded. Cost if wrong: an admin who wants the proposal back must save the set.
- **H7. Any new live version discards the held row of its set, inside `insert_live_version_in`.** One site covers every origin (save, starter, catalog change, recompile, accept), as spec §8 requires. An accepted held row also becomes `discarded`: the CHECK list has no `accepted` status, and the audit entry of the accept names the held version. Cost if wrong: none known.
- **H8. A held row that the job replaces is discarded.** When the live version has a held row from an older generation (two compiler bumps in a row), the job discards it and stores the new one in one transaction, so the one-held index holds. Cost if wrong: none known.
- **H9. Accept checks, in this order:** the held row exists with the requested number (else `tool-policy-version-not-found`); it has an output (else the new `tool-policy-held-not-acceptable`, `FAILED_PRECONDITION` / 409); its generation and schema revision are the running ones, its `base_version` is the live version, and for a tenant its `catalog_revision` is the current one (else `tool-policy-version-conflict`, spec §8 "the held row is stale"). The new live row gets the output with its `version` field restamped to the new number. Cost if wrong: one registry row.
- **H10. Authorization.** List of one tenant: `ReadToolPolicies` at the org. Accept and discard of a tenant row: `ManageToolPolicies` at the org. Accept and discard of a floor row: `ManageToolPolicyFloor` at `root_prn()` (spec §8). List of every held row of every set, and `RunToolPolicyRecompile`: `ManageToolPolicyRecompile` at `root_prn()` (spec §9.5). Cost if wrong: an `org_admin` cannot see held rows of other organizations (intended).
- **H11. The lock is `pg_try_advisory_xact_lock(5_580_781)` in a transaction that stays open for the whole run.** That transaction holds no row lock, so it does not break spec §4.5 ("no compile inside a database transaction" is about the state row lock). `try`, not wait: a second replica reports `AlreadyRunning` at once. The lock goes when the guard is released or dropped (rollback). Cost if wrong: the job holds one pooled connection for its run.
- **H12. The older-replica rule runs after the lock.** If the largest stored `compiler_generation` or `schema_revision` over every row of every status is higher than the running value, the run stops with `OlderReplica` and writes nothing. Cost if wrong: none known.
- **H13. Result labels.** The metric `iam_tool_policy_recompile_results_total{result}` has the three spec values `identical`, `held_different`, `held_failed`, plus `conflict` (the set changed between the read and the lock) and `error` (the compiler was busy or unavailable). A set that `error`s is skipped and stays out of date until the next run; the job never holds it for a compiler outage. Each label is primed at 0 at boot (memory `prometheus-new-counter-first-sample-trap`). Cost if wrong: a dashboard label to rename.
- **H14. The boot run is a background task that does not end early.** It runs after `with_tool_policies` when the feature is on, then waits for the shutdown signal. `boot_deferred` stops when any `servers` task ends, so the task must not return before shutdown. On shutdown, the job stops between two sets. A boot run that fails logs an error and IAM keeps serving. Cost if wrong: none known.
- **H15. Actors.** The RPC run uses the caller as the actor. The boot run has no principal: the audit entries and events carry `actor_prn: None` (the bootstrap precedent), and `created_by` is the constant `RECOMPILE_JOB_ACTOR = "system:tool-policy-recompile"`. Cost if wrong: one constant.
- **H16. Audit and events.** One audit entry per set result that wrote a row (`RunToolPolicyRecompile`, detail `{set, version, origin, result, base_version}`), one for each accept (`AcceptHeldToolPolicyVersion`, detail adds `held_version`) and one for each discard (`DiscardHeldToolPolicyVersion`). A new live version (identical result, accept) enqueues `tool_set.published` or `tool_floor.published` with origin `recompile` or `accept` (PR 2 R14). A held row and a discard change nothing that the gateway reads, so they enqueue no event. Cost if wrong: none known.
- **H17. The report.** `report` (JSONB) holds `result` (`different` or `failed`) and three sorted name diffs: policies by `@id` (from a cedar 4.14 parse of `cedar_policies`, the Cedar policy id when a policy has no `@id`), conditions by `slot`, and schema entries by `<namespace>::<entityTypes|actions|commonTypes>::<name>` compared with the PR 1 `canonical_json`. A failed result holds the rejections instead (`policy_id`, `code`, `message`). Names only, no content, so the size stays bounded by the catalog cap. Cost if wrong: the report shape is in the proto, so a change is a new field.
- **H18. `RunToolPolicyRecompile` is synchronous (Q2).** It returns the run status and one row per set. Cost if wrong: a very large backlog can pass a client deadline; the job still finishes on the server, because the RPC future is not the job's only owner (it holds the lock guard and runs to the end of the current set).
- **H19. Unchanged.** `tool-policy-recompile-pending` keeps the PR 1 rule: a catalog write is refused while the live version is out of date, after a discard too. A save still clears it (PR 1 D10). R19 stays: a set stored by PR 1 with a history rule now compiles to a `tool-policy-history-unsupported` rejection, so the job holds it as `held_failed`.

---

## File map

Contracts:
- `contracts/proto/paigasus/iam/v1/iam.proto` (new enums, messages, 4 RPCs), `contracts/proto/paigasus/common/v1/error.proto` (code 63), all generated Rust / Python / TS output, `rs/crates/libs/paigasus-proto/src/error.rs`, `ts/packages/paigasus-sdk/src/errors/presentation.ts`

IAM core (`rs/crates/libs/paigasus-iam-core/`):
- `src/authz/{action.rs, schema.rs, roles.rs}` — 1 action, revision 7
- Create `src/tool_policy/report.rs` — the held report and its diff
- Modify `src/tool_policy/{model.rs, ports.rs, compare.rs, mod.rs}`

IAM service (`rs/crates/services/paigasus-iam/`):
- Create: `src/adapters/persistence/migration/m0015_tool_policy_held_versions.rs`, `src/adapters/persistence/pg_recompile_lock.rs`, `src/application/tool_policies/{held.rs, recompile.rs}`, `tests/tool_policies_held_pg.rs`, `tests/grpc_tool_policies_held.rs`
- Modify: `migration/mod.rs`, `entities/{tool_policy_state.rs, tool_policy_set_version.rs}`, `persistence/{mod.rs, pg_tool_policies.rs}`, `application/tool_policies/{mod.rs, events.rs, sets.rs}`, `application/{error.rs, fakes.rs}`, `adapters/{retryable.rs, tool_policy_wire.rs}`, `adapters/grpc/{tool_policies.rs, convert.rs}`, `adapters/http/{tool_policies.rs, error.rs, mod.rs}`, `src/main.rs`, `tests/{tool_policies_pg.rs, http_tool_policies.rs, tool_policies_real_compiler.rs}`
- `rs/crates/libs/paigasus-observability/src/names.rs` — one metric name

Docs: `docs/ops/RUNBOOK-tool-policies.md`, `docs/tool-policies/tenant-guide.md`.

---

### Task 1: Contracts — held versions, the re-compile RPCs and error code 63

**Files:**
- Modify: `contracts/proto/paigasus/iam/v1/iam.proto`, `contracts/proto/paigasus/common/v1/error.proto`, `rs/crates/libs/paigasus-proto/src/error.rs`, `ts/packages/paigasus-sdk/src/errors/presentation.ts`, and all generated output under `rs/crates/libs/paigasus-proto/src/generated/`, `py/packages/paigasus-proto/src/paigasus_proto/generated/`, `ts/packages/paigasus-proto/src/generated/`

**Interfaces:**
- Produces (proto, package `paigasus.iam.v1`):
  ```proto
  enum HeldToolPolicyResult { HELD_TOOL_POLICY_RESULT_UNSPECIFIED = 0; HELD_TOOL_POLICY_RESULT_DIFFERENT = 1; HELD_TOOL_POLICY_RESULT_FAILED = 2; }
  message NameDiff { repeated string added = 1; repeated string removed = 2; repeated string changed = 3; }
  message HeldToolPolicyReport { HeldToolPolicyResult result = 1; NameDiff policies = 2; NameDiff conditions = 3; NameDiff schema = 4; ToolPolicyRejections rejections = 5; }
  message HeldToolPolicyVersion {
    ToolPolicySetKind kind = 1; string org_id = 2; uint64 version = 3; uint64 base_version = 4; string source = 5;
    ToolPolicyVersionStatus status = 6; uint32 compiler_generation = 7; uint32 schema_revision = 8; uint64 catalog_revision = 9;
    HeldToolPolicyReport report = 10; string created_by = 11; google.protobuf.Timestamp created_at = 12;
    CompiledToolPolicySet compiled = 13; // absent for a FAILED result
  }
  message ListHeldToolPolicyVersionsRequest { string org_prn = 1; uint32 limit = 2; uint64 offset = 3; }
  message ListHeldToolPolicyVersionsResponse { repeated HeldToolPolicyVersion versions = 1; }
  message AcceptHeldToolPolicyVersionRequest { ToolPolicySetKind kind = 1; string org_prn = 2; uint64 held_version = 3; }
  message AcceptHeldToolPolicyVersionResponse { ToolPolicySetVersion accepted = 1; }
  message DiscardHeldToolPolicyVersionRequest { ToolPolicySetKind kind = 1; string org_prn = 2; uint64 held_version = 3; }
  message DiscardHeldToolPolicyVersionResponse { HeldToolPolicyVersion discarded = 1; }
  enum RecompileRunStatus { RECOMPILE_RUN_STATUS_UNSPECIFIED = 0; RECOMPILE_RUN_STATUS_RAN = 1; RECOMPILE_RUN_STATUS_ALREADY_RUNNING = 2; RECOMPILE_RUN_STATUS_OLDER_REPLICA = 3; }
  enum RecompileOutcome { RECOMPILE_OUTCOME_UNSPECIFIED = 0; RECOMPILE_OUTCOME_IDENTICAL = 1; RECOMPILE_OUTCOME_HELD_DIFFERENT = 2; RECOMPILE_OUTCOME_HELD_FAILED = 3; RECOMPILE_OUTCOME_ALREADY_PROPOSED = 4; RECOMPILE_OUTCOME_CONFLICT = 5; RECOMPILE_OUTCOME_ERROR = 6; }
  message RecompileSetResult { ToolPolicySetKind kind = 1; string org_id = 2; uint64 base_version = 3; RecompileOutcome outcome = 4; uint64 version = 5; /* the new live or held version; 0 otherwise */ }
  message RunToolPolicyRecompileRequest {}
  message RunToolPolicyRecompileResponse { RecompileRunStatus status = 1; repeated RecompileSetResult results = 2; }
  ```
  Four RPCs appended to `service ToolPolicyService`: `ListHeldToolPolicyVersions`, `AcceptHeldToolPolicyVersion`, `DiscardHeldToolPolicyVersion`, `RunToolPolicyRecompile`.
- Produces (error): `ERROR_REASON_TOOL_POLICY_HELD_NOT_ACCEPTABLE = 63;` wire `tool-policy-held-not-acceptable`; Rust `ErrorReason::ToolPolicyHeldNotAcceptable`. Comment: "the held version has no compiled output, because its compile failed. Discard it, or save the set again. gRPC FAILED_PRECONDITION, HTTP 409."

Doc comments to write (they are the contract for console and operator tools):
- `HeldToolPolicyVersion`: a held row is never enforced and never returned by `GetCompiledToolPolicies`. Any new live version of its set makes it `DISCARDED`. At most one `HELD` row exists for each set.
- `ListHeldToolPolicyVersionsRequest`: an empty `org_prn` lists every held row of every set, the floor included, and needs `ManageToolPolicyRecompile`. Only `HELD` rows are listed.
- `AcceptHeldToolPolicyVersionRequest` / `Discard…`: `kind = FLOOR` needs an empty `org_prn`; `UNSPECIFIED` is `missing-required-field`. Accept of a stale row is `tool-policy-version-conflict`: run the job again (spec §8).
- `RunToolPolicyRecompileResponse`: `ALREADY_RUNNING` and `OLDER_REPLICA` write nothing and have no results.

- [ ] **Step 1: Write the failing test.** In `rs/crates/libs/paigasus-proto/src/error.rs`, extend `the_tool_policy_reasons_resolve_both_ways`:

```rust
assert_eq!(ErrorReason::from_wire_reason("tool-policy-held-not-acceptable"), Some(ErrorReason::ToolPolicyHeldNotAcceptable));
assert_eq!(ErrorReason::ToolPolicyHeldNotAcceptable as i32, 63);
```

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-proto the_tool_policy_reasons`. Expected: FAIL to compile.
- [ ] **Step 3: Edit both protos, then** `export PATH=…; cd contracts && buf format -w && cd .. && moon run contracts:generate --force`. Add the wire string to the list in `error.rs` after `"tool-policy-history-unsupported"`. Add `[ErrorReason.TOOL_POLICY_HELD_NOT_ACCEPTABLE]: 'from-transport',` to `presentation.ts` next to the other tool-policy codes.
- [ ] **Step 4: Run.** `cargo nextest run --locked -p paigasus-proto`; `moon run contracts:fmt contracts:lint contracts:breaking paigasus-proto-ts:test paigasus-sdk-ts:test paigasus-sdk-ts:typecheck`. If a TS test pins the count of registry entries or service methods, update the count it pins (+1 code, +4 methods). Expected: PASS. `git status` shows generated files in all three languages.
- [ ] **Step 5: Commit** `feat(contracts): add the held tool policy versions, the re-compile RPCs and code 63 (SMA-781)`.

---

### Task 2: IAM core authz — `ManageToolPolicyRecompile`, revision 7

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/{action.rs, schema.rs, roles.rs}`

**Interfaces:**
- Produces: `Action::ManageToolPolicyRecompile` (wire `"ManageToolPolicyRecompile"`, `is_write() == true`, not a restore). In `Action::ALL` (length test +1) and in every exhaustive match.
- `SCHEMA_SRC`: copy the `appliesTo` of `ManageToolPolicyFloor`.
- No role list gains it. `platform_admin` gets it through its every-action template (spec §9.5).
- Produces: `STARTER_POLICY_REVISION = 7`, doc paragraph: "`7`: SMA-781 added `ManageToolPolicyRecompile`. It is a non-restore write, so it joins the generated forbid list."

- [ ] **Step 1: Write the failing tests:**

```rust
// roles.rs starter_policy_table() rows
"platform_admin allows ManageToolPolicyRecompile at Root (SMA-781)"   // allow
"org_admin denies ManageToolPolicyRecompile at Root (SMA-781)"        // deny
"org_admin denies ManageToolPolicyRecompile on its own org (SMA-781)" // deny
"tool_policy_reader denies ManageToolPolicyRecompile at Root (SMA-781)" // deny
#[test] fn the_recompile_action_is_in_the_generated_forbid_source() {}
// schema.rs: add "ManageToolPolicyRecompile" to the_tool_policy_actions_validate_against_the_embedded_schema
// action.rs: extend the is_write table and the wire round-trip
```

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-iam-core authz`. Expected: FAIL to compile.
- [ ] **Step 3: Implement.** Bump the revision. Run `starter_policy_content_is_pinned_to_the_declared_revision` once, read the new hash from its failure message, and set `EXPECTED_STARTER_CONTENT_HASH`.
- [ ] **Step 4: Run.** Same command, then `cargo nextest run --locked -p paigasus-iam --lib roles bootstrap` and `PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test authz_schema --test authz_bootstrap`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): add the ManageToolPolicyRecompile action for platform admins (SMA-781)`.

---

### Task 3: IAM core — held types and the diff report

**Files:**
- Create: `rs/crates/libs/paigasus-iam-core/src/tool_policy/report.rs`
- Modify: `src/tool_policy/{model.rs, compare.rs, mod.rs}`

**Interfaces:**
- Consumes: PR 1 `CompiledOutput`, `compare::canonical_json` (crate-private, same crate).
- Produces (`report.rs`, re-exported from `mod.rs`; serde `Serialize + Deserialize`, snake_case):
  ```rust
  pub enum HeldResult { Different, Failed }                       // "different" | "failed"
  pub struct NameDiff { pub added: Vec<String>, pub removed: Vec<String>, pub changed: Vec<String> } // each sorted, no duplicates
  pub struct ReportRejection { pub policy_id: Option<String>, pub code: String, pub message: String }
  pub struct HeldReport { pub result: HeldResult, pub policies: NameDiff, pub conditions: NameDiff, pub schema: NameDiff, pub rejections: Vec<ReportRejection> }
  pub fn diff_outputs(old: &CompiledOutput, new: &CompiledOutput) -> HeldReport; // result Different, rejections empty
  pub fn failed_report(rejections: Vec<ReportRejection>) -> HeldReport;   // result Failed, three empty diffs
  ```
  The keys are those of H17. `diff_outputs` parses `cedar_policies` with `cedar_policy::PolicySet::from_str` (4.14). A text that does not parse gives the one key `"<unparsed>"` in `changed` when the two texts differ, so a parse failure never hides a change.
- Produces (`model.rs`):
  ```rust
  pub enum HeldStatus { Held, Discarded }
  pub struct HeldVersion {
      pub key: ToolPolicySetKey, pub version: u64, pub base_version: u64, pub source: String,
      pub compiled: Option<Vec<u8>>, /* None for a failed result */
      pub compiler_generation: u32, pub schema_revision: u32, pub catalog_revision: Option<u64>,
      pub status: HeldStatus, pub report: HeldReport, pub created_by: String, pub created_at: DateTime<Utc>,
  }
  ```
  `ToolPolicyState` gains `pub last_version: u64` and `pub fn next_version(&self) -> u64 { self.live_version.max(self.last_version) + 1 }` (H2). Every `ToolPolicyState { … }` literal in the workspace gets `last_version: 0` in this task.

- [ ] **Step 1: Write the failing tests** in `report.rs` and `model.rs`:

```rust
#[test] fn identical_outputs_give_three_empty_diffs() {}
#[test] fn a_new_policy_id_is_added_and_a_missing_one_is_removed() {}
#[test] fn the_same_id_with_another_text_is_changed() {}
#[test] fn a_condition_is_keyed_by_slot() { /* threshold change → changed ["p_rate_0__temporal_0"] */ }
#[test] fn a_new_tool_action_is_an_added_schema_entry() { /* key "Pgs::Gateway::actions::search" */ }
#[test] fn a_key_order_change_in_the_schema_is_not_a_change() {}
#[test] fn an_unparsable_policy_text_still_reports_a_change() { /* old "garbage", new valid → changed contains "<unparsed>" */ }
#[test] fn the_report_round_trips_through_json_with_snake_case_keys() { /* {"result":"different","policies":{"added":[…]…}} */ }
#[test] fn next_version_is_one_above_the_larger_counter() { /* (live 4, last 5) → 6; (live 6, last 5) → 7 */ }
```

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-iam-core tool_policy`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** `report.rs` and the model additions.
- [ ] **Step 4: Run.** Same command, then `cargo build --locked -p paigasus-iam` (the new `last_version` field compiles in every literal). Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): add the held tool policy version types and the diff report (SMA-781)`.

---

### Task 4: IAM persistence — m0015, the version counter and the held rows

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/migration/m0015_tool_policy_held_versions.rs`, `tests/tool_policies_held_pg.rs`
- Modify: `migration/mod.rs`, `entities/{tool_policy_state.rs, tool_policy_set_version.rs}`, `pg_tool_policies.rs`, `rs/crates/libs/paigasus-iam-core/src/tool_policy/{model.rs, ports.rs}`, `src/application/fakes.rs`, `src/application/tool_policies/mod.rs` (`origin_str`, the recheck, the version numbers), `src/adapters/tool_policy_wire.rs`, `tests/tool_policies_pg.rs`

**Interfaces:**
- Consumes: Task 3 `HeldVersion`, `HeldStatus`, `HeldReport`, `ToolPolicyState::next_version`.
- Produces (`model.rs`): `VersionOrigin::Recompile`, `VersionOrigin::Accept`, mapped to `'recompile'` / `'accept'` in Postgres, in both `origin_str` functions, and to `TOOL_POLICY_VERSION_ORIGIN_RECOMPILE` / `_ACCEPT` in `tool_policy_wire.rs`.
- Produces (`ports.rs`, `ToolPolicyRepository` additions), implemented by `PgToolPolicies` and `InMemoryToolPolicies`:
  ```rust
  /// Live versions whose generation or schema revision differs from the arguments. The floor
  /// first, then tenants by org UUID.
  async fn stale_live_versions(&self, compiler_generation: u32, schema_revision: u32) -> Result<Vec<ToolPolicyVersion>, RepositoryError>;
  /// The largest compiler_generation and schema_revision over every row of every status. (0, 0) when empty.
  async fn max_stored_revisions(&self) -> Result<(u32, u32), RepositoryError>;
  /// The held row of a set (status held only).
  async fn held(&self, key: &ToolPolicySetKey) -> Result<Option<HeldVersion>, RepositoryError>;
  /// Every held row (status held only): of one tenant, or of every set when `org` is None. Floor first, then by org UUID.
  async fn list_held(&self, org: Option<Uuid>) -> Result<Vec<HeldVersion>, RepositoryError>;
  /// True when a held or discarded row of the set has this base version, generation and schema revision (H6).
  async fn proposal_exists(&self, key: &ToolPolicySetKey, base_version: u64, compiler_generation: u32, schema_revision: u32) -> Result<bool, RepositoryError>;
  /// Discard the held row of the set, if any, then insert `h` (status must be Held). CAS:
  /// `h.version` must be `next_version()` of the stored state, else Conflict(ToolPolicyVersion). Sets last_version.
  async fn insert_held_in(&self, tx: &dyn Transaction, h: &HeldVersion) -> Result<(), RepositoryError>;
  /// Set the held row `version` of the set to discarded. False when no held row has that number.
  async fn discard_held_in(&self, tx: &dyn Transaction, key: &ToolPolicySetKey, version: u64) -> Result<bool, RepositoryError>;
  ```
- Changes `insert_live_version_in` (doc and both implementations): the CAS is `GREATEST(live_version, last_version) = v.version - 1`, it sets both counters to `v.version`, and it sets the held row of the set to `discarded` (H7). `state()` and `lock_state_in` return `last_version`.
- Changes the service (`mod.rs`): `write_new_live_version` and `catalog_write` compare only `live_version` and `catalog_revision` of the locked state with step 1 (H3), and number the new version with `locked.next_version()`.

**m0015 DDL** (`SET LOCAL lock_timeout = '5s'`, as m0013):
```sql
ALTER TABLE tool_policy_state ADD COLUMN last_version BIGINT NOT NULL DEFAULT 0;
UPDATE tool_policy_state s SET last_version = COALESCE((SELECT MAX(v.version) FROM tool_policy_set_version v
  WHERE v.kind = s.kind AND v.org_id IS NOT DISTINCT FROM s.org_id), 0);
ALTER TABLE tool_policy_set_version ALTER COLUMN compiled DROP NOT NULL;
ALTER TABLE tool_policy_set_version ADD CONSTRAINT ck_tool_policy_version_compiled CHECK (compiled IS NOT NULL OR status IN ('held','discarded'));
ALTER TABLE tool_policy_set_version ADD CONSTRAINT ck_tool_policy_version_held_fields CHECK (status <> 'held' OR (base_version IS NOT NULL AND report IS NOT NULL));
```
`down` drops both constraints, sets `compiled` NOT NULL after deleting rows with a NULL `compiled`, and drops the column. The entity `compiled` becomes `Option<Vec<u8>>`; `model_to_version` treats a NULL on a live or superseded row as corrupt.

- [ ] **Step 1: Write the failing Postgres tests** in `tests/tool_policies_held_pg.rs` (seed one org as `tests/tool_policies_pg.rs` does):

```rust
#[tokio::test] async fn m0015_backfills_last_version_from_the_stored_versions() { /* insert v1..v3, set last_version to 0 by raw SQL, run M0015_BACKFILL_SQL, read last_version == 3; a set with no rows reads 0 */ }
#[tokio::test] async fn a_held_row_takes_the_next_number_and_the_next_save_skips_it() { /* live v1; insert_held_in v2; insert_live_version_in v3 succeeds; v2 is discarded */ }
#[tokio::test] async fn a_held_row_with_a_stale_number_is_a_version_conflict() {}
#[tokio::test] async fn a_new_live_version_discards_the_held_row_from_every_origin() { /* Save, CatalogChange, Starter, Recompile, Accept */ }
#[tokio::test] async fn insert_held_discards_an_older_held_row_first() { /* H8; one held row after */ }
#[tokio::test] async fn a_failed_held_row_stores_no_compiled_bytes() { /* compiled None round-trips */ }
#[tokio::test] async fn a_live_row_without_compiled_bytes_is_refused_by_the_check() { /* raw INSERT status live, compiled NULL → check violation */ }
#[tokio::test] async fn a_held_row_without_a_base_version_is_refused_by_the_check() {}
#[tokio::test] async fn the_held_report_round_trips_through_jsonb() {}
#[tokio::test] async fn version_and_live_never_return_a_held_or_discarded_row() {}
#[tokio::test] async fn stale_live_versions_lists_the_floor_first_then_tenants() {}
#[tokio::test] async fn max_stored_revisions_reads_every_status() { /* only a discarded row has generation 9 → (9, _) */ }
#[tokio::test] async fn proposal_exists_sees_held_and_discarded_rows_only_for_the_same_base() {}
#[tokio::test] async fn discard_held_of_an_absent_number_is_false_and_changes_nothing() {}
#[tokio::test] async fn the_next_version_skips_a_number_that_an_old_replica_wrote() { /* raw UPDATE live_version = 5 with last_version 4 (a PR 2 replica); insert_live_version_in v6 succeeds */ }
#[tokio::test] async fn a_rolled_back_held_insert_leaves_no_row_and_the_old_counter() {}
```

In `tests/tool_policies_pg.rs`, the existing `a_second_held_row_is_refused_by_the_index` and `version_does_not_return_a_held_row` stay and must pass with the new CHECKs (give their raw rows a `base_version` and a `report`).

Unit tests in `application/tool_policies/mod.rs` for H3:
```rust
#[tokio::test] async fn a_held_row_stored_during_a_save_does_not_make_the_save_a_conflict() { /* InMemoryToolPolicies inserts a held row between compile and lock; the save is Accepted with the version after it */ }
```

- [ ] **Step 2: Run to verify failure.** `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test tool_policies_held_pg`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** the migration (register `m0015_tool_policy_held_versions`), the entities, the Pg methods (errors through `persistence::map_err`), the fake, the origin mappings and the service changes. Keep the backfill SQL in one `pub const M0015_BACKFILL_SQL: &str` in the migration module, which the migration and the backfill test both use.
- [ ] **Step 4: Run.** Same command, then `--test tool_policies_pg --test tool_policies_settings_pg --test grpc_tool_policies --test grpc_tool_policies_fetch` and `--lib tool_policies migration`. Expected: PASS, not skipped.
- [ ] **Step 5: Commit** `feat(rs): store held tool policy versions with a shared version counter (SMA-781)`.

---

### Task 5: IAM application — list, accept and discard held versions

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/application/tool_policies/held.rs`
- Modify: `src/application/tool_policies/{mod.rs, sets.rs, events.rs}`, `src/application/error.rs`, `src/adapters/retryable.rs`, `src/adapters/http/error.rs`, `src/adapters/grpc/convert.rs`

**Interfaces:**
- Consumes: Task 4 repository methods; PR 1 `read_version`, `authorize_org`; PR 2 `events`.
- Produces (`error.rs`): `TenancyError::ToolPolicyHeldNotAcceptable` (unit variant), `code()` = `"tool-policy-held-not-acceptable"`, `class()` = `Precondition` (gRPC `FailedPrecondition`, HTTP 409), retryable `No`. Add it to the transport tables in `convert.rs` and `http/error.rs`, next to `ToolPolicyRecompilePending`.
- Produces (`sets.rs`): `pub(crate) fn restamp_version(bytes: &[u8], version: u64) -> Result<Vec<u8>, TenancyError>` — decode `CompiledToolPolicySet`, set `version`, encode; a decode failure is `Internal` (as `decode_compiled`).
- Produces (`events.rs`): the three helpers take `actor: Option<&Prn>` (H15). Existing callers pass `Some(actor)`.
- Produces (`held.rs`):
  ```rust
  /// The set that a held-row RPC names.
  pub enum HeldTarget { Tenant(Uuid), Floor }
  pub async fn list_held(&self, actor: &Prn, org: Option<Uuid>, page: Page) -> Result<Vec<HeldVersion>, TenancyError>;
  pub async fn accept_held(&self, actor: &Prn, target: HeldTarget, held_version: u64) -> Result<ToolPolicyVersion, TenancyError>;
  pub async fn discard_held(&self, actor: &Prn, target: HeldTarget, held_version: u64) -> Result<HeldVersion, TenancyError>;
  ```

**Rules the tests pin (spec §8, H7, H9, H10, H16):**
- Authorization per H10. A tenant target looks up the organization first (`NotFound`).
- `accept_held` step 1: read `state` and `held`; then the H9 checks in the H9 order. Step 2 (no compile): `uow.begin()`, `lock_state_in`, recheck `live_version` and `catalog_revision` (H3); a change → `ToolPolicyVersionConflict`, nothing written. Under the same lock, take the held row with `discard_held_in`; `false` (another admin discarded it after step 1) → `ToolPolicyVersionNotFound`, nothing written. Insert a live version: number `locked.next_version()`, `origin = Accept`, `source` = held source, `compiled = restamp_version(held.compiled, n)`, the held generation, schema revision and catalog revision, `created_by` = actor. Enqueue `set_published` / `floor_published` (origin `"accept"`). One audit entry `AcceptHeldToolPolicyVersion`, detail `{"set", "version", "origin": "accept", "held_version", "base_version"}`. Commit.
- `discard_held`: absent row or another number → `ToolPolicyVersionNotFound`. Lock the state row, `discard_held_in`; `false` → `ToolPolicyVersionNotFound`. One audit entry `DiscardHeldToolPolicyVersion`, detail `{"set", "held_version", "base_version"}`. No event. Returns the row with `status = Discarded`.
- `list_held`: the repository order, then `page.offset` / `page.limit` as `list_tools` does.

- [ ] **Step 1: Write the failing unit tests** in `held.rs` (reuse the `mod.rs` test harness; make the harness helpers `pub(super)`; `InMemoryToolPolicies` seeds held rows):

```rust
#[tokio::test] async fn accept_writes_a_new_live_version_with_origin_accept() { /* live v1 (gen 0), held v2 (gen 1) → live v3, origin Accept, compiled version field == 3, v2 discarded */ }
#[tokio::test] async fn accept_writes_one_event_and_one_audit_entry_with_one_correlation_id() {}
#[tokio::test] async fn accepting_a_stale_held_row_is_a_version_conflict() { /* three cases, each writes nothing: the catalog revision moved; the held generation is not the running one; the base is not live (seed the rows directly in the fake, because a real save discards the held row) */ }
#[tokio::test] async fn accepting_a_failed_held_row_is_held_not_acceptable() {}
#[tokio::test] async fn accepting_another_number_is_version_not_found() {}
#[tokio::test] async fn a_save_between_the_accept_read_and_the_lock_is_a_version_conflict() { /* ConcurrentChange; nothing written */ }
#[tokio::test] async fn discard_sets_the_row_discarded_and_writes_one_audit_entry_and_no_event() {}
#[tokio::test] async fn after_a_discard_catalog_writes_stay_recompile_pending() { /* H19 */ }
#[tokio::test] async fn after_a_discard_a_save_clears_recompile_pending() {}
#[tokio::test] async fn a_floor_accept_needs_manage_tool_policy_floor_at_root() { /* org_admin actions only → Forbidden */ }
#[tokio::test] async fn list_of_every_set_needs_manage_tool_policy_recompile() {}
#[tokio::test] async fn list_of_one_tenant_needs_read_tool_policies_and_pages() {}
#[tokio::test] async fn the_held_methods_on_an_unknown_org_are_not_found() {}
// sets.rs
#[test] fn restamp_changes_only_the_version() { /* decode both; every other field equal */ }
// error.rs: every_tenancy_code_is_declared_in_the_canonical_registry covers the new variant.
```

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-iam --lib tool_policies error`. Expected: FAIL to compile.
- [ ] **Step 3: Implement.** Reuse `write_new_live_version` for the accept write if it can take the restamped bytes; else extract one private helper that both call. Do not copy the write steps.
- [ ] **Step 4: Run.** Same command; then `bash ci/python-env/run.sh python3 ci/error-registry/check.py --self-test` and the form that `repo:error-code-single-site` uses. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): list, accept and discard held tool policy versions (SMA-781)`.

---

### Task 6: IAM persistence — the advisory lock adapter

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/pg_recompile_lock.rs`
- Modify: `src/adapters/persistence/mod.rs`, `rs/crates/libs/paigasus-iam-core/src/tool_policy/{ports.rs, mod.rs}`, `tests/tool_policies_held_pg.rs`

**Interfaces:**
- Produces (`ports.rs`, re-exported from `mod.rs`):
  ```rust
  #[async_trait] pub trait RecompileLock: Send + Sync {
      /// `None` when another holder has the lock. Never waits.
      async fn try_acquire(&self) -> Result<Option<Box<dyn RecompileLockGuard>>, RepositoryError>;
  }
  #[async_trait] pub trait RecompileLockGuard: Send {
      async fn release(self: Box<Self>) -> Result<(), RepositoryError>;
  }
  ```
- Produces:
  ```rust
  /// One key for the re-compile job. Distinct from MIGRATION_LOCK_KEY and AUDIT_PARTITION_LOCK_KEY.
  pub const TOOL_POLICY_RECOMPILE_LOCK_KEY: i64 = 5_580_781;
  pub struct PgRecompileLock { db: DatabaseConnection }
  impl PgRecompileLock { pub fn new(db: DatabaseConnection) -> Self; }
  impl RecompileLock for PgRecompileLock { /* begin(); SELECT pg_try_advisory_xact_lock($KEY) AS locked; false → drop tx, None */ }
  ```
  The guard owns the `DatabaseTransaction`. `release` commits. A dropped guard rolls back, which also frees the lock (H11).

- [ ] **Step 1: Write the failing tests** (in `tests/tool_policies_held_pg.rs`, plus one unit test):

```rust
#[tokio::test] async fn a_second_try_acquire_gets_none_until_the_first_guard_is_released() {}
#[tokio::test] async fn a_dropped_guard_frees_the_lock() {}
#[tokio::test] async fn the_lock_does_not_block_a_tool_policy_write() { /* hold the guard; a lock_state_in + insert_live_version_in on another transaction commits */ }
#[test] fn the_recompile_lock_key_differs_from_the_other_advisory_keys() { /* != MIGRATION_LOCK_KEY, != AUDIT_PARTITION_LOCK_KEY */ }
```

- [ ] **Step 2: Run to verify failure.** `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test tool_policies_held_pg -E 'test(/lock/)'`. Expected: FAIL to compile.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run.** Same command, and `--lib persistence`. Expected: PASS, not skipped.
- [ ] **Step 5: Commit** `feat(rs): add the advisory lock of the tool policy re-compile job (SMA-781)`.

---

### Task 7: IAM application — the re-compile job

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/application/tool_policies/recompile.rs`
- Modify: `src/application/tool_policies/mod.rs` (deps, module list), `src/application/fakes.rs`, `src/adapters/http/mod.rs` (`with_tool_policies` passes `PgRecompileLock::new(self.db.clone())`), `rs/crates/libs/paigasus-observability/src/names.rs`

**Interfaces:**
- Consumes: Task 6 `RecompileLock`, `RecompileLockGuard`, `PgRecompileLock`; Task 4 repository methods; the Task 5 write helper.
- Produces (`fakes.rs`): `FakeRecompileLock` (an `Arc<AtomicBool>`; `held()` helper; a dropped guard releases).
- Produces: `ToolPolicyServiceDeps.recompile_lock: Arc<dyn RecompileLock>`.
- Produces (`names.rs`): `IAM_TOOL_POLICY_RECOMPILE_RESULTS_TOTAL = "iam_tool_policy_recompile_results_total"`, in `ALL`, with a `the_iam_tool_policy_recompile_family_is_registered` test.
- Produces (`recompile.rs`):
  ```rust
  pub const RECOMPILE_JOB_ACTOR: &str = "system:tool-policy-recompile";
  pub enum RecompileOutcome { Identical { version: u64 }, HeldDifferent { version: u64 }, HeldFailed { version: u64 }, AlreadyProposed, Conflict, Error }
  pub struct SetRecompile { pub key: ToolPolicySetKey, pub base_version: u64, pub outcome: RecompileOutcome }
  pub enum RecompileRun { Ran(Vec<SetRecompile>), AlreadyRunning, OlderReplica { stored_generation: u32, stored_schema_revision: u32 } }
  /// The RPC entry: authorizes ManageToolPolicyRecompile at root_prn(), then runs.
  pub async fn run_recompile(&self, actor: &Prn) -> Result<RecompileRun, TenancyError>;
  /// The boot entry: no actor (H15). Stops between two sets when `stop` resolves.
  pub async fn run_recompile_at_boot(&self, stop: impl Future<Output = ()> + Send) -> Result<RecompileRun, TenancyError>;
  pub(crate) fn outcome_label(o: &RecompileOutcome) -> Option<&'static str>; // None for AlreadyProposed
  ```

**The algorithm (spec §8, H6, H8, H11-H16):**
1. `try_acquire`; `None` → `AlreadyRunning`.
2. `max_stored_revisions()`; either value above the running one → release, `OlderReplica` (H12).
3. For each row of `stale_live_versions(running generation, TOOL_SCHEMA_REVISION)`, one at a time:
   a. `proposal_exists(key, live.version, running values)` → `AlreadyProposed`, next set (H6).
   b. Read `state`. `state.live_version != live.version` → `Conflict`. Read the catalog (tenant) at `state.catalog_revision`.
   c. Compile with the PR 1 `compile` helper, no transaction open. `Busy` / `Unavailable` → `Error` (log, next set).
   d. Accepted and `outputs_identical(decode_compiled(&live.compiled)?, &out)` → live write, `origin = Recompile` (reuse the PR 1 / Task 5 write helper). Accepted and different → held write with `diff_outputs`. Rejected → held write with `failed_report` (each rejection: `policy_id`, `reason.code()`, `message`) and `compiled = None`.
   e. Every write: `uow.begin()`, `lock_state_in`, recheck `live_version` and `catalog_revision` (H3); a change → `Conflict`, nothing written. A held write uses `insert_held_in` with `version = locked.next_version()`, `base_version = live.version`, the running generation and schema revision, the current catalog revision, and the `encode_compiled` bytes of the new output at that number. One audit entry `RunToolPolicyRecompile` (H16). A live write also enqueues its published event. Commit.
   f. Increment the metric with `outcome_label`, and write one `info!` line with the set, the base version, the outcome and the new version.
4. Release the lock. A run that stops for `stop` releases it too.

- [ ] **Step 1: Write the failing unit tests** in `recompile.rs` (the `mod.rs` harness gains `FakeRecompileLock`; `seed_live_tenant(h, source, generation, schema)` exists):

```rust
#[tokio::test] async fn an_identical_result_stores_a_new_live_version_with_origin_recompile() { /* live v1 gen 0; compiler gen 1, same output → live v2, gen 1, event origin "recompile" */ }
#[tokio::test] async fn a_different_result_keeps_the_old_version_live_and_holds_the_new_output() { /* held v2, base 1, report.result Different, live still v1 */ }
#[tokio::test] async fn a_rejected_compile_holds_a_failed_row_with_its_rejections() { /* compiled None, report.rejections[0].code == "tool-policy-invalid" via RejectionReason::code() */ }
#[tokio::test] async fn a_set_stored_with_a_history_rule_is_held_failed() { /* H19, reason tool-policy-history-unsupported */ }
#[tokio::test] async fn a_restart_does_not_write_a_second_held_row() { /* run twice → second run AlreadyProposed, one held row, compiler called once */ }
#[tokio::test] async fn a_discarded_proposal_is_not_held_again_on_the_next_run() { /* H6 */ }
#[tokio::test] async fn a_newer_generation_replaces_an_older_held_row() { /* held at gen 1; compiler gen 2 → old row discarded, new held row */ }
#[tokio::test] async fn a_current_live_set_is_not_compiled() {}
#[tokio::test] async fn the_floor_is_recompiled_against_the_base_schema() {}
#[tokio::test] async fn a_save_between_the_read_and_the_lock_is_a_conflict_and_writes_nothing() { /* ConcurrentChange */ }
#[tokio::test] async fn a_busy_or_unavailable_compiler_is_an_error_and_the_job_goes_on() { /* two sets; first Busy → Error; second Identical */ }
#[tokio::test] async fn a_held_lock_is_already_running_and_compiles_nothing() {}
#[tokio::test] async fn a_higher_stored_generation_is_older_replica_and_writes_nothing() { /* and the lock is released after */ }
#[tokio::test] async fn a_higher_stored_schema_revision_is_older_replica() {}
#[tokio::test] async fn the_rpc_entry_needs_manage_tool_policy_recompile_at_root() { /* Forbidden; the lock was not taken */ }
#[tokio::test] async fn the_boot_run_has_no_actor_and_the_system_created_by() { /* audit actor_prn None; created_by RECOMPILE_JOB_ACTOR; event actor_prn None */ }
#[tokio::test] async fn each_written_result_has_one_audit_entry() {}
#[tokio::test] async fn the_boot_run_stops_between_sets_and_releases_the_lock() { /* stop resolves after the first set */ }
#[test] fn outcome_labels_are_the_five_metric_values() { /* identical, held_different, held_failed, conflict, error; AlreadyProposed → None */ }
// names.rs
#[test] fn the_iam_tool_policy_recompile_family_is_registered() {}
```

Check the metric with the existing `metrics` test pattern of the crate (a `DebuggingRecorder`, or the pattern that `tests/metrics.rs` uses): one `identical` increment after the first test.

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-iam --lib tool_policies::recompile` and `-p paigasus-observability`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** `recompile.rs`, the fake lock, the dependency and the metric name.
- [ ] **Step 4: Run.** Same commands, then `--lib tool_policies` (every PR 1 and PR 2 unit test still passes). Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): add the tool policy re-compile job (SMA-781)`.

---

### Task 8: IAM transports — four RPCs over gRPC and HTTP

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/grpc_tool_policies_held.rs`
- Modify: `src/adapters/grpc/tool_policies.rs`, `src/adapters/http/tool_policies.rs`, `src/adapters/tool_policy_wire.rs`, `tests/http_tool_policies.rs`, `tests/support/mod.rs` (only if a seed helper is needed)

**Interfaces:**
- Consumes: Tasks 1, 5, 6.
- Produces (gRPC): the four methods. Each first calls `self.svc()` (capability, PR 2 R11). `kind` `UNSPECIFIED` or unknown → `missing-required-field`; `kind = FLOOR` with a non-empty `org_prn` → `invalid-argument` (the PR 1 field-error pattern). `held_version = 0` → `missing-required-field`. `HeldVersion` → `pb::HeldToolPolicyVersion` in `tool_policy_wire.rs` (one conversion that gRPC and HTTP share; `compiled` decoded from the stored bytes as `get_tool_policy_set` does; absent for `None`). `RecompileRun` → the response status and results.
- Produces (HTTP, same router as PR 1; JSON field names follow the proto):
  - `GET /v1/organizations/{id}/held-tool-policy-versions?limit&offset` and `GET /v1/held-tool-policy-versions?limit&offset` → `{"versions":[…]}`
  - `POST /v1/organizations/{id}/held-tool-policy-versions/{version}/accept` and `POST /v1/tool-policy-floor/held-versions/{version}/accept` → `{"accepted":{…}}`
  - `POST /v1/organizations/{id}/held-tool-policy-versions/{version}/discard` and `POST /v1/tool-policy-floor/held-versions/{version}/discard` → `{"discarded":{…}}`
  - `POST /v1/tool-policy-recompile` → `{"status":"ran"|"already_running"|"older_replica","results":[…]}`

- [ ] **Step 1: Write the failing tests** in `tests/grpc_tool_policies_held.rs` (Postgres + a fake compiler script whose `revision` prints a chosen generation, as `tests/grpc_tool_policies.rs` does):

```rust
#[tokio::test] async fn the_four_rpcs_answer_capability_disabled_when_the_toggle_is_off() {}
#[tokio::test] async fn a_platform_admin_run_holds_a_different_result_and_lists_it() { /* live v1 at gen 0 by raw SQL; compiler gen 1 returns another output; Run → RAN, HELD_DIFFERENT v2; List("") shows v2 with report.policies */ }
#[tokio::test] async fn an_identical_result_is_applied_and_the_fetch_sees_origin_recompile() {}
#[tokio::test] async fn an_org_admin_cannot_run_the_job_or_list_every_set() { /* PERMISSION_DENIED */ }
#[tokio::test] async fn an_org_admin_accepts_its_held_row_and_the_fetch_returns_it() { /* GetCompiledToolPolicies version 3, origin ACCEPT */ }
#[tokio::test] async fn a_stale_accept_is_aborted_with_version_conflict() { /* support::reason_of == "tool-policy-version-conflict" */ }
#[tokio::test] async fn a_failed_held_accept_is_failed_precondition_held_not_acceptable() {}
#[tokio::test] async fn discard_then_catalog_write_is_recompile_pending() {}
#[tokio::test] async fn a_floor_held_row_needs_the_floor_action() {}
#[tokio::test] async fn a_held_version_is_not_found_through_the_fetch() { /* GetCompiledToolPolicies version 2 → NOT_FOUND */ }
#[tokio::test] async fn an_unspecified_kind_is_missing_required_field() {}
#[tokio::test] async fn each_mutation_writes_its_audit_row() { /* RunToolPolicyRecompile, AcceptHeldToolPolicyVersion, DiscardHeldToolPolicyVersion in audit_log */ }
#[tokio::test] async fn accept_commits_one_outbox_row_and_discard_none() {}
// tests/http_tool_policies.rs
#[tokio::test] async fn the_held_and_recompile_routes_are_404_when_the_capability_is_off() {}
#[tokio::test] async fn http_and_grpc_return_the_same_held_row() {}
#[tokio::test] async fn a_held_not_acceptable_accept_is_409_with_its_code() {}
```

- [ ] **Step 2: Run to verify failure.** `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test grpc_tool_policies_held --test http_tool_policies`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** both adapters and the shared conversion.
- [ ] **Step 4: Run.** Same command plus `--test grpc_tool_policies --test grpc_tool_policies_fetch --lib`. Expected: PASS, not skipped.
- [ ] **Step 5: Commit** `feat(rs): serve the held tool policy versions and the re-compile run over gRPC and HTTP (SMA-781)`.

---

### Task 9: IAM boot — run the job at boot, the metric, and the real compiler

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/main.rs`, `src/adapters/http/mod.rs` (an accessor for the tool policy service, if `main.rs` has none), `tests/tool_policies_real_compiler.rs`, `tests/tool_policies_held_pg.rs`

**Interfaces:**
- Consumes: Task 7 `run_recompile_at_boot`, `IAM_TOOL_POLICY_RECOMPILE_RESULTS_TOTAL`; Task 6 `PgRecompileLock`.
- `describe_iam_metrics`: `describe_counter!(names::IAM_TOOL_POLICY_RECOMPILE_RESULTS_TOTAL, "Tool policy re-compile job results, labeled by result (identical/held_different/held_failed/conflict/error). held_* means an admin must accept or discard a held version (SMA-781).")`.
- `boot_deferred`, right after `with_tool_policies`, only when the service exists (H14): prime the five labels at 0, then `servers.spawn` one task that runs `run_recompile_at_boot(stop)` with `stop` = a clone of the shutdown watch, logs the run (`info!` for `Ran` with the counts, `info!` for `AlreadyRunning`, `warn!` for `OlderReplica`, `error!` for `Err`), then awaits the shutdown watch and returns `Ok(())`.

- [ ] **Step 1: Write the failing tests:**

```rust
// tests/tool_policies_real_compiler.rs (real binary; Moon wires the build dependency since PR 1)
#[tokio::test] async fn the_job_applies_an_identical_result_with_the_real_compiler() { /* save the starter with the real compiler; raw UPDATE compiler_generation = generation - 1 on the live row; run_recompile_at_boot → Identical; live origin recompile at the real generation */ }
// tests/tool_policies_held_pg.rs
#[tokio::test] async fn two_concurrent_runs_write_one_held_row() { /* two services on one database, PgRecompileLock each, a fake compiler that sleeps 300 ms; tokio::join! → one Ran, one AlreadyRunning; one held row */ }
```

`main.rs` has no unit test seam. Check the wiring by hand in Step 4.

- [ ] **Step 2: Run to verify failure.** `cd rs && PAIGASUS_REQUIRE_DOCKER=1 cargo nextest run --locked -p paigasus-iam --test tool_policies_held_pg -E 'test(two_concurrent_runs)'`. Expected: both new tests PASS at once, because Tasks 6 and 7 hold the logic. They are regression tests for the composed parts. The lock mutation of Task 11 proves that `two_concurrent_runs_write_one_held_row` bites. The new code of this task is the `main.rs` wiring, which Step 4 checks.
- [ ] **Step 3: Implement** the wiring.
- [ ] **Step 4: Run.** Same command; `moon run paigasus-iam-rs:test` for the real-compiler test (read the log: it ran, not skipped); then start IAM locally with `tool_policies.enabled = true` against a Docker Postgres and read one `tool policy re-compile` log line and the zero-valued metric on `/metrics`. Expected: PASS, and the process keeps serving after the run.
- [ ] **Step 5: Commit** `feat(rs): run the tool policy re-compile job at boot (SMA-781)`.

---

### Task 10: Runbook and tenant guide

**Files:**
- Modify: `docs/ops/RUNBOOK-tool-policies.md`, `docs/tool-policies/tenant-guide.md`

Content (STE, short sentences; the review checks each item against the code):

`RUNBOOK-tool-policies.md`, a new section "Compiler or schema changes":
- When the job runs (each boot, after the migration; `POST /v1/tool-policy-recompile` / `RunToolPolicyRecompile`, `platform_admin`), the advisory lock, and the older-replica rule. During a rolling deploy, the old replicas do not run it.
- The five results and the metric `iam_tool_policy_recompile_results_total`. A non-zero `held_*` needs an admin. An `error` result needs a second run after the compiler is healthy.
- How to read a held row (`GET /v1/held-tool-policy-versions`) and its report, then accept or discard. A floor row needs `ManageToolPolicyFloor`.
- After a discard, the set stays at the old generation, and catalog writes stay `tool-policy-recompile-pending` until a save. The job does not hold the same proposal again (H6).
- A set stored with a history rule is held as failed (R19): remove the temporal block and save.
- No Dogwood bump and no compiler output change may merge without a `COMPILER_GENERATION` bump; the golden test enforces it (spec §6.2).

`tenant-guide.md`: one section "Held versions": what a held version is, that the live set keeps working, the accept and discard routes of the tenant, and `tool-policy-held-not-acceptable` in the error list.

- [ ] **Step 1: Write both changes.**
- [ ] **Step 2: Check** each route, RPC, metric, code and config name with `grep` against the code of Tasks 1-9. Expected: every name exists.
- [ ] **Step 3: Commit** `docs(docs): document the tool policy re-compile job and held versions (SMA-781)`.

---

### Task 11: Mutation proofs and the full gate graph

**Files:** none new. Temporary edits only. Remove each one by deleting the marked insert; do not restore with `git checkout --` (memory `mutation-restore-discards-the-fix`).

- [ ] **Step 1: Mutation battery.** For each rule: insert `// MUTATION` lines that remove or bypass the rule, confirm that the mutant COMPILES, run the named tests with `--no-fail-fast`, confirm that they FAIL, then delete the insert. Record each result in the PR body.

| Rule | Mutation | Must fail |
|---|---|---|
| identical check | treat every accepted result as identical | `a_different_result_keeps_the_old_version_live_and_holds_the_new_output` |
| hold on failure | skip the held write on a rejection (`let _ = failed_report(..)`) | `a_rejected_compile_holds_a_failed_row_with_its_rejections` |
| idempotent restart | `proposal_exists` result ignored | `a_restart_does_not_write_a_second_held_row`, `a_discarded_proposal_is_not_held_again_on_the_next_run` |
| advisory lock | `try_acquire` SQL uses a constant `true` | `a_second_try_acquire_gets_none_until_the_first_guard_is_released`, `two_concurrent_runs_write_one_held_row` |
| older replica | skip the `max_stored_revisions` comparison | `a_higher_stored_generation_is_older_replica_and_writes_nothing` |
| job recheck | skip the locked-state comparison in the job | `a_save_between_the_read_and_the_lock_is_a_conflict_and_writes_nothing` |
| discard on live | remove the held discard from `insert_live_version_in` (Pg and fake) | `a_new_live_version_discards_the_held_row_from_every_origin` |
| one-held index | comment out the `uq_tool_policy_set_version_held` statement in m0013 | `a_second_held_row_is_refused_by_the_index` |
| version counter | CAS on `live_version` only | `the_next_version_skips_a_number_that_an_old_replica_wrote`, `a_held_row_takes_the_next_number_and_the_next_save_skips_it` |
| stale accept | skip the `base_version` check | `accepting_a_stale_held_row_is_a_version_conflict`, `a_stale_accept_is_aborted_with_version_conflict` |
| failed accept | skip the `compiled.is_none()` check (use an empty `Vec`) | `accepting_a_failed_held_row_is_held_not_acceptable` |
| restamp | accept stores the held bytes unchanged | `accept_writes_a_new_live_version_with_origin_accept` |
| recompile authz | skip the `ManageToolPolicyRecompile` check | `the_rpc_entry_needs_manage_tool_policy_recompile_at_root`, `an_org_admin_cannot_run_the_job_or_list_every_set` |
| floor authz | authorize a floor accept with `ManageToolPolicies` at the org | `a_floor_accept_needs_manage_tool_policy_floor_at_root` |

After the last delete, run every test of the battery once more without mutations (memory `mutation-battery-rerun-whole`), and confirm that `git diff` shows no `MUTATION` text.

- [ ] **Step 2: Run the full gate graph.** Use the command between the `ci-targets` markers of the root `CLAUDE.md` (do not copy it anywhere). Run it under system `/bin/bash` 3.2 for the whole `moon ci` invocation, with `--base origin/main --include-relations`. Then re-run directly the gates that need another bash, and read those results instead of the `moon ci` verdict for them:
  - bash 4+ (`/opt/homebrew/bin/bash`): `ci/ruff/run.sh`, `ci/next-public/run.sh`, `ci/publish-metadata/run.sh`, `ops/nats/check-subjects.sh` plus its nextest half (`cd rs && cargo nextest run --locked --no-tests=pass -p paigasus-iam --test nats_permissions --test docker_preflight --profile iam-nats`).
  - bash 5 with a healthy pipe: `ci/actionlint/run.sh`; read its `pipe capacity` preflight line; on a 512-byte pipe, run it in a Linux container (memory `small-pipe-linux-container-workaround`).
  - `bash ci/cargo-lock-integrity/run.sh`.
  Expected: every target passes. On an unattributed failure, follow the root `CLAUDE.md` diagnosis procedure (copy the Moon cache report and the task state directory before any re-run).
- [ ] **Step 3: Confirm the commit rules.** `git log origin/main..HEAD --format=%B | grep -c 'SMA-75[1]'` → `0`; every subject ends with `(SMA-781)`; every body is only the `Co-Authored-By` paragraph.

No commit in this task unless Step 2 needs a fix. A fix gets its own `fix(<scope>): … (SMA-781)` commit.

---

## Self-review notes

- **Spec coverage.** §3.2 PR 3 scope: Tasks 1-10. §3.5 apply identical, hold different: Task 7. §4.4 held and discarded statuses, `base_version`, `report`, the one-held index: Tasks 3, 4 (H1-H5, H7, H8); the index exists since PR 1 and Task 11 proves it. §4.6 recompile-pending: kept (H19), tested after a discard in Tasks 5 and 8. §7: one new code, 63 (H9); the stale accept uses `tool-policy-version-conflict` as §8 says. §8 job, boot and on demand, advisory lock, older-replica rule, skip with a held row, the three results with log and metric: Tasks 6, 7, 9 (H6, H11-H14). §8 held rows (discard on a new live version, accept checks, discard, floor action): Tasks 4, 5 (H7, H9, H10). §8.1 one comparator: Task 7 uses `outputs_identical`. §9.1 PR 3 RPCs and HTTP routes: Tasks 1, 8. §9.5 `ManageToolPolicyRecompile`: Task 2. §9.6 audit: Tasks 5, 7 (H16). §10 PR 3 tests: identical, different, failed (Task 7); restart without a second held row (Task 7); advisory lock (Tasks 6, 9); older replica (Task 7); accept, stale accept, discard (Tasks 5, 8); mutation proofs (Task 11). §11: routes and pagination follow PR 1 (Task 8). §12: the calls stay; H6 and H18 are new calls and are listed as questions.
- **Type consistency.** `HeldVersion`, `HeldStatus`, `HeldReport`, `NameDiff`, `ReportRejection` (Task 3) are used by the port (Task 4), the use cases (Tasks 5, 7) and the wire (Task 8). `ToolPolicyState::next_version` (Task 3) is the one numbering rule (Tasks 4-7). `RecompileLock` is defined in Task 6 and used in Task 7. `RecompileOutcome` (Rust, Task 7) maps one-to-one to the proto `RecompileOutcome` (Task 1).
- **Order.** Task 6 (the lock adapter) comes before Task 7 (the job), so that `with_tool_policies` gets a real lock with no placeholder.
- **Proportion.** Signatures, test names, pinned values and the rules the spec fixes. The only bodies are the proto messages (the contract), the m0015 DDL and the job algorithm, which the signatures do not determine.

---

## Decisions of the product owner (2026-11-17)

The product owner accepted the recommended answer for Q1, Q2 and Q3 on 2026-11-17. The plan follows these answers.

- **Q1. After an admin discards a held version, should the job propose it again on the next run?** Spec §8 skips a set only when a held row exists, so a discarded proposal comes back at each boot and each run. **Recommended: no.** The job skips a proposal with the same live version and the same compiler values that is held or discarded (H6). A new compiler bump, a save or an accept makes a new proposal possible. To get the proposal back, the admin saves the set.
- **Q2. Should `RunToolPolicyRecompile` wait for the job, or start it and return at once?** **Recommended: wait (H18).** The boot run normally does the work, so the RPC usually has few sets. It returns one row per set, which an operator can read at once. An asynchronous run needs a status RPC and a stored run record, which this PR does not have.
- **Q3. Should a held version notify the tenant?** Today a tenant sees a held row only through `ListHeldToolPolicyVersions` and the `tool-policy-recompile-pending` error on a catalog write. **Recommended: no event in this PR.** The gateway does not need one. A console notice or an event can come with the console work.
