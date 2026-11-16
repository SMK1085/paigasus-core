# IAM tool policy sets, PR 2 (SMA-775) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give each organization a tool-policy setting (mode and fail mode) with the starter policy on first enable, give the gateway its own IAM identity, serve the compiled tenant set and the floor through two fetch RPCs, publish the change events, and add the gateway adapter that reads IAM through SMA-752's `PolicySource` port.

**Architecture:** IAM gets one new table (`tool_policy_settings`, m0014), four new RPCs on the existing `ToolPolicyService`, two new authz actions, one new system role (`tool_policy_reader`, granted at Root to a gateway service account), and three new outbox event types under `iam.policy.>`. Every write that makes a new live version now also writes an outbox event in the same transaction. The gateway gets a second `PolicySource` adapter (`IamPolicySource`) that calls the two fetch RPCs with a service credential read from a file. The port's `TenantPolicies` gains the tenant settings, so the cache keeps them with the set and the request side takes the mode and the fail mode from IAM instead of from `[[tool_policy.org]]`.

**Tech Stack:** Rust 2024 / 1.95, tonic + prost via buf, SeaORM migrations, axum, async-nats (core subscription, unchanged), Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-11-15-iam-tool-policy-sets-design.md` (PR 2 of the §3.2 split: §6.3 save, §7 one new code, §9.1 PR 2 RPCs, §9.2, §9.3, §9.4, §9.5 PR 2 actions, §9.6, §9.7, §10 PR 2 tests, and the "Tenant documentation (PR 2)" changelog item). PR 1 plan: `docs/superpowers/plans/2026-11-15-iam-tool-policy-sets-pr1.md`. The gateway side is SMA-752 (merged, PR 475): `rs/crates/services/paigasus-gateway/src/domain/tool_policy.rs`, `src/application/tool_policy/{cache.rs,request.rs}`, `src/config.rs` `[tool_policy]`.

**Before merge (not a code task):** the ADR-0027 amendment of spec §9.4 must exist in Notion. It records that the gateway holds one IAM credential, that the credential is used only for the two fetch RPCs, and that the D9 self-query rule is unchanged. The coordinator does this with Sven.

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for shell, TOML, YAML; `<!-- … -->` is not needed for Markdown).
- Rust edition 2024, rust-version 1.95. `warnings = "deny"`: no task adds an item that the same task does not use or test.
- `rs/Cargo.toml` `members` stay literal paths (SMA-663). This PR adds no crate.
- Every cargo invocation in a Moon task passes `--locked`.
- A `.proto` edit is followed by `buf format -w` and `moon run contracts:generate --force` in the same task. Commit all generated Rust, Python and TS output with it. Only new messages, enum values and RPCs: `repo:breaking` must stay green (in a worktree, pass `git rev-parse --git-common-dir` to `--against`, memory `buf-breaking-worktree-gitlink`).
- New error codes: numbers 61 (`tool-policy-mode-transition-invalid`, an error) and 62 (`tool-policy-history-unsupported`, a rejection reason) in the IAM block (1-299) of `contracts/proto/paigasus/common/v1/error.proto`. Register it in `rs/crates/libs/paigasus-proto/src/error.rs` (the wire list and its test), in `ts/packages/paigasus-sdk/src/errors/presentation.ts`, and in the `TenancyError` membership test, so `repo:error-code-single-site` stays green. If a new emission file is added, give it a `MANIFEST` row in `ci/error-registry/check.py`.
- New event subjects stay under `iam.policy.>`. `ops/nats/subjects.env` does not change (the publisher grant is `iam.>`, the gateway grant is `iam.policy.>`). `repo:nats-permissions` and `tests/nats_permissions.rs` must pass.
- `STARTER_POLICY_REVISION` goes from 5 to 6 in one task (Task 2), with the new `EXPECTED_STARTER_CONTENT_HASH`.
- Commits: conventional, with a workspace scope (`feat(rs)`, `feat(contracts)`, `test(rs)`, `docs(docs)`). Every subject ends with `(SMA-775)` and is at most 100 characters (`header-max-length`). The body is only the trailer paragraph `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. No other body line starts with `word:` or `#NNN` (commitlint `footer-leading-blank`).
- Each "Commit" step: `git add` exactly the files in that task's **Files** block (plus the generated output and every lockfile the task's cargo runs changed), then `git commit -m "<subject>" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"`. If the hook reports `commitlint not found`, provision the worktree (root `CLAUDE.md`); never bypass it.
- Never write the parent issue key (the parent of SMA-775) in a commit message, a PR title or a PR body. Refer to the parent as "IAM tool catalog, tool policy sets and policy sync". PR 2 must not close it.
- No `--no-verify`, no `git commit --amend`, no `git reset`, no bare `git stash`. Do not hand-edit `.github/CODEOWNERS`. Do not touch or copy the `ci-targets` or `moon-diagnosis` marker blocks in any `CLAUDE.md`.
- Do not install host software. Use `docker run ubuntu:24.04` or `rust:1.95.0-bookworm` for a Linux-only check.
- Put `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` in front of every `moon`, `buf`, `uv` or `cargo nextest` command.
- Docker-gated IAM tests (Postgres, NATS) follow the existing pattern: `support::start_migrated_postgres()`, the quiet skip, and the `tests/docker_preflight.rs` canary. A test never reads `CI` (`repo:iam-docker-policy-single-site`). After a local run, check that the suite did not skip.
- Keep test runs narrow: each task names a nextest filter or a `--test` binary. Run the full crate only in Task 13.
- Values pinned by the spec: mode `off` (default when no row) / `log_only` / `enforce`; fail mode `open` / `closed` (default `closed`); events `iam.policy.tool_set.published`, `iam.policy.tool_floor.published`, `iam.policy.tool_settings.changed`; role `tool_policy_reader` with `ReadCompiledToolPolicies` only, granted at Root; gateway key `iam.service_credential_file`; starter source = `STARTER_TOOL_POLICY_SOURCE` from PR 1, saved as version 1 with `origin = starter`.

## Review Focus

1. **An organization in mode `off` must still get the floor.** A floor `forbid` blocks a call in every tenant mode (spec §3.4). Expect a block. Test: Task 11 `an_org_in_mode_off_still_gets_the_floor`, Task 8 `the_fetch_of_an_org_in_mode_off_returns_the_floor_version`.
2. **A settings change with no set change** (an admin moves `log_only` → `enforce`). The gateway cache skips a reload when the version token is equal, so the token must include the settings. Expect the next load to apply `enforce`. Test: Task 11 `a_settings_change_alone_changes_the_version_token`, Task 10 `a_changed_mode_from_the_source_applies_on_the_next_load`.
3. **A gateway cold start while IAM is unreachable.** There are no known tenant settings. Expect the global `on_policy_unavailable` to decide, never a silent `off`. Test: Task 10 `with_no_known_settings_the_global_fail_mode_applies`.
4. **A credential rotation.** The operator replaces the credential file while the gateway runs. Expect the next fetch to send the new key, with no restart. A file with a trailing newline must still work. Test: Task 11 `a_rotated_credential_is_used_on_the_next_fetch`, `a_trailing_newline_in_the_credential_file_is_trimmed`.
5. **A set saved between the settings read and the lock** during the first enable. Expect no second version 1 and no lost save: the settings write is `tool-policy-version-conflict` and writes nothing. Test: Task 7 `a_set_saved_during_the_first_enable_is_a_version_conflict`.

---

## Pre-plan: rulings and questions

The merged code (PR 1 c44bab5c, SMA-752 PR 475) differs from the spec in the places below. Each ruling is a call this plan makes; the coordinator can overturn any of them before Task 1.

- **R1.** Ruling: `TenantPolicies` gains `settings: Option<TenantSettings>` (mode + fail mode); the file source returns `None` — the merged port carries only text, catalog and version, but spec §9.3 needs the settings in the same response and the same cache entry — cost if wrong: a later split into a second port method rewrites the adapter and the cache read (about one task).
- **R2.** Ruling: with the IAM source and no known settings for an org, the request side uses mode `enforce` and the global `on_policy_unavailable` — spec §9.3 says "the global value when it has none", and `enforce` makes that value decide; `off` would silently drop the tenant set — cost if wrong: during an IAM outage, a tenant that is really `off` is blocked (global `closed`) or allowed and reported (global `open`).
  - **When "no settings known" happens, exactly:** the request side runs the tenant lookup first, so a reachable IAM always supplies the settings. R2 applies only when that lookup fails (IAM down, a timeout, an error status, a malformed answer) AND this gateway process has no good load of that org in its cache: the org was never loaded since the process started, or its entry was evicted after `idle_evict_secs` with no use. A failed reload of an org with a good load keeps its last known settings.
  - **ADR-0027 (d) is not affected.** "A tenant starts in log-only mode" is an IAM rule: IAM answers `off` for an org with no row and refuses `off` → `enforce` with no set, so the first mode a tenant can turn on is `log_only` (Task 7). Whenever IAM answers, the gateway uses the mode that IAM returns, never `enforce` from R2. R2 is a fail-safe for an unknown value; it never sets or changes a tenant's mode.
- **R3.** Ruling: with `source.kind = "iam"`, `validate` refuses any `[[tool_policy.org]]` entry; `default_mode` stays, and applies only to a scope with no organization — spec §9.3 says the IAM `fail_mode` replaces the per-org override, and two sources of one setting would conflict — cost if wrong: operators lose a per-org emergency override; adding it back is a config change.
- **R4.** Ruling: the gateway keeps SMA-752 D16 (every message invalidates every tenant); IAM still puts the org UUID in each payload — the merged subscriber ignores the payload by design, and per-tenant clearing is an optimisation, not a correctness rule — cost if wrong: each event makes every active tenant reload lazily on its next request (more fetches, no wrong decision).
- **R5.** Ruling: `IamPolicySource` refuses (`SourceError::Malformed`) a compiled set with any `HistoryCondition` until SMA-753 ships — the spec anti-goal says the gateway must never accept a set it cannot fully evaluate, and the merged kernel evaluates Cedar text only — cost if wrong: a tenant set with a history rule is unavailable (tenant fail mode decides); a floor with a history rule makes the floor unavailable for every tenant (global value decides). **Kept as defence in depth next to R19:** IAM refuses new saves, but a set saved with the PR 1 code before this PR has no such check, and an IAM that drops R19 (SMA-753) can run before every gateway is updated. SMA-753 removes both.
- **R6.** Ruling: an IAM floor with `version = 0` is a valid empty floor, not `SourceError::NotFound` — IAM answered, and spec §9.1 defines the empty answer; the port's "an unavailable floor is an error" means an unreachable source — cost if wrong: a platform that forgot its floor runs with no floor and no alarm.
- **R7.** Ruling: the version tokens are `t<version>:<mode>:<fail_mode>` for a tenant and `f<version>` for the floor — the cache keeps the stored set when the token is equal (`cache.rs` "An unchanged version needs no compile"), so the token must change with the settings — cost if wrong: none known.
- **R8.** Ruling: the adapter reads the credential file on every fetch, with no file watcher — a fetch is one cache load (at most one per tenant per refresh interval), and spec §9.4 asks only that a rotation needs no restart — cost if wrong: one small file read per load.
- **R9.** Ruling: `GetToolPolicySettings` authorizes `ReadToolPolicies` at the org PRN; `PutToolPolicySettings` authorizes `ManageToolPolicySettings` — spec §9.5 lists no read action for the settings, and `org_admin` holds `ReadToolPolicies` — cost if wrong: `org_member` cannot read the mode (the same as the policies today).
- **R10.** Ruling: a new error code 61 `tool-policy-mode-transition-invalid` (`FAILED_PRECONDITION` / 409) for `off` → `enforce` when the org has no tenant set — spec §9.2 gives the rule but no code, and every other rule in §7 has a precise reason — cost if wrong: one registry row to rename.
- **R11.** Ruling: the four new RPCs are behind the `iam.tool.policies` capability, the same as PR 1's eight (PR 1 D8) — one service, one wiring — cost if wrong: an IAM replica whose compiler probe failed answers `capability-disabled` to the gateway fetch, and the gateway treats that as an unavailable source (last good set under `open`, block under `closed`).
- **R12.** Ruling: a settings write has no expected value; the starter insert locks the state row and rechecks `live_version == 0`, else `tool-policy-version-conflict` — spec §9.2 defines no expected value for settings — cost if wrong: two admins who change the mode at the same time get last-write-wins on the mode (never a duplicate starter).
- **R13.** Ruling: one audit entry `PutToolPolicySettings` per settings write; its `detail` carries `starter_version` when the write saved the starter — spec §9.6 says one entry per mutation — cost if wrong: an auditor who filters on `PutToolPolicySet` does not see the starter.
- **R14.** Ruling: every new live version (save, floor save, catalog coupling, starter) writes `tool_set.published` or `tool_floor.published` in the same transaction; a byte-identical save and a catalog write with identical output write no set event — this completes PR 1 D9, and spec §9.7 says a catalog change publishes only through the version it makes — cost if wrong: none known.
- **R15.** Ruling: the tenant guide goes to `docs/tool-policies/tenant-guide.md` and the operator steps to `docs/ops/RUNBOOK-tool-policies.md` — the repo has no tenant docs home; `docs/ops/` holds the other runbooks — cost if wrong: a file move.
- **R16.** Ruling: the Helm chart does not change in this PR (no secret mount, no `source.kind = "iam"`) — the chart has no tool-policy wiring at all today (`grep tool_policy charts/` is empty), so this is deployment work for a follow-up — cost if wrong: a deploy of the feature needs that follow-up first.
- **R17.** Ruling: `name_key` now calls `paigasus_kernel::tool_policy::normalise` (PR 1 D4 named this for PR 2); `paigasus-iam-core` enables the kernel feature `tool-policy` — SMA-752 merged the normaliser — cost if wrong: `unicode-normalization` enters the iam-core graph (it is already in `rs/Cargo.lock` through the gateway).
- **R19 (Sven, Q2).** Ruling: until SMA-753 ships the gateway history evaluator, IAM rejects a save of a tenant set or of the floor whose compiled result has any history condition, with the new rejection reason 62 `tool-policy-history-unsupported` — Sven chose to refuse the save, so no new set reaches the gateway's fail-closed path of R5 — cost if wrong: tenants cannot store a history rule until SMA-753. The check is one function, `reject_history_until_gateway_supports_it`, behind one constant, `HISTORY_CONDITIONS_ENFORCED_BY_GATEWAY = false`. It is called from the one PR 1 `compile` helper, so it covers the tenant save, the floor save, the catalog coupling and the starter (Task 5).
- **R18.** Ruling: `tool_policy_reader` has `scope_kinds = [Root]`; `org_admin` does not get `ReadCompiledToolPolicies`; `platform_admin` has it through its every-action template — spec §3.6 rejects the read for API-key holders of an org — cost if wrong: none known.

**Questions for Sven: answered.** Q1: the runbook, with no boot seeding. Q2: refuse the save (R19). The question text stays below as the record.

- **Q1. How is the gateway service account provisioned?** (Answer: the runbook, Task 12. Task 9's test helper follows the same steps.) Spec §9.4 and §11 say "in the same way as the other system service accounts", but the code has none: a service account must be owned by a concrete org, team or project (`ck_service_account_owner`, `TenancyNodeRef` has no Root arm). Default in this plan (no new IAM code): an operator runbook. A platform admin makes an operator-owned "platform" organization, makes the service account there, grants `tool_policy_reader` at Root, issues an API key, and mounts it as the gateway's credential file (Task 12). The other option is a boot-seeded system account whose key IAM writes to a secret. That is a new key-handling path and needs a home for the key.
- **Q2. Should IAM refuse to save a set with a history rule until SMA-753 ships?** (Answer: yes. R19, Task 5.) PR 1 accepts history rules, and with R5 the gateway fails closed on them. A floor with one history rule then blocks every call of every tenant under a global `closed`. The first draft's default was no IAM-side block. Sven chose the other option: a temporary rejection in IAM, removed by SMA-753. It uses its own reason (62), not `tool-policy-shape-unsupported`, so a tenant can tell "not yet" from "never".

---

## File map

Contracts:
- `contracts/proto/paigasus/iam/v1/iam.proto` (new enums, messages, 4 RPCs), `contracts/proto/paigasus/common/v1/error.proto` (code 61), all generated Rust / Python / TS output

IAM core (`rs/crates/libs/paigasus-iam-core/`):
- `src/authz/{action.rs, schema.rs, roles.rs}` — 2 actions, 1 role, revision 6
- `src/domain_event.rs` — 3 event types
- `src/tool_policy/{model.rs, ports.rs, name.rs, mod.rs}` — settings types, `VersionOrigin::Starter`, repository methods, the kernel normaliser
- `Cargo.toml` — kernel feature `tool-policy`

IAM service (`rs/crates/services/paigasus-iam/`):
- Create: `src/adapters/persistence/migration/m0014_create_tool_policy_settings.rs`, `src/adapters/persistence/entities/tool_policy_settings.rs`, `src/application/tool_policies/{events.rs, settings.rs, fetch.rs}`, `tests/tool_policies_settings_pg.rs`, `tests/grpc_tool_policies_fetch.rs`
- Modify: `migration/mod.rs`, `entities/mod.rs`, `pg_tool_policies.rs`, `entities/tool_policy_set_version.rs` (origin mapping), `application/tool_policies/mod.rs`, `application/error.rs`, `application/fakes.rs`, `adapters/grpc/{tool_policies.rs, convert.rs}`, `adapters/http/{tool_policies.rs, error.rs, mod.rs}`, `adapters/retryable.rs`, `tests/nats_permissions.rs`, `tests/http_tool_policies.rs`, `tests/support/mod.rs`
- `ops/nats/permissions.md` (subject count 22 → 25)

Gateway (`rs/crates/services/paigasus-gateway/`):
- Create: `src/adapters/policy_source/iam.rs`, `tests/tool_policy_iam_source.rs`
- Modify: `src/domain/tool_policy.rs`, `src/adapters/policy_source/{mod.rs, file.rs}`, `src/adapters/iam/client.rs` (`build_channel` → `pub(crate)`), `src/application/tool_policy/{cache.rs, request.rs, testing.rs}`, `src/config.rs`, `src/runtime.rs`, `gateway.toml.example`

Docs: `docs/tool-policies/tenant-guide.md`, `docs/ops/RUNBOOK-tool-policies.md`, `rs/crates/services/paigasus-iam/iam.toml.example` (comment only, if needed).

---

### Task 1: Contracts — settings, fetch RPCs and error codes 61 and 62

**Files:**
- Modify: `contracts/proto/paigasus/iam/v1/iam.proto`, `contracts/proto/paigasus/common/v1/error.proto`, `rs/crates/libs/paigasus-proto/src/error.rs`, `ts/packages/paigasus-sdk/src/errors/presentation.ts`, and all generated output under `rs/crates/libs/paigasus-proto/src/generated/`, `py/packages/paigasus-proto/src/paigasus_proto/generated/`, `ts/packages/paigasus-proto/src/generated/`

**Interfaces:**
- Produces (proto, package `paigasus.iam.v1`):
  ```proto
  enum ToolPolicyMode { TOOL_POLICY_MODE_UNSPECIFIED = 0; TOOL_POLICY_MODE_OFF = 1; TOOL_POLICY_MODE_LOG_ONLY = 2; TOOL_POLICY_MODE_ENFORCE = 3; }
  enum ToolPolicyFailMode { TOOL_POLICY_FAIL_MODE_UNSPECIFIED = 0; TOOL_POLICY_FAIL_MODE_OPEN = 1; TOOL_POLICY_FAIL_MODE_CLOSED = 2; }
  message ToolPolicySettings { ToolPolicyMode mode = 1; ToolPolicyFailMode fail_mode = 2; google.protobuf.Timestamp updated_at = 3; string updated_by = 4; }
  message GetToolPolicySettingsRequest { string org_prn = 1; }
  message GetToolPolicySettingsResponse { ToolPolicySettings settings = 1; }
  message PutToolPolicySettingsRequest { string org_prn = 1; ToolPolicyMode mode = 2; ToolPolicyFailMode fail_mode = 3; }
  // starter_version is 0 when the write did not save the starter.
  message PutToolPolicySettingsResponse { ToolPolicySettings settings = 1; uint64 starter_version = 2; }
  message GetCompiledToolPoliciesRequest { string org_prn = 1; optional uint64 version = 2; }
  message GetCompiledToolPoliciesResponse { CompiledToolPolicySet set = 1; ToolPolicySettings settings = 2; uint64 floor_version = 3; }
  message GetCompiledFloorPoliciesRequest { optional uint64 version = 1; }
  message GetCompiledFloorPoliciesResponse { CompiledToolPolicySet set = 1; }
  ```
  Four RPCs appended to `service ToolPolicyService`: `GetToolPolicySettings`, `PutToolPolicySettings`, `GetCompiledToolPolicies`, `GetCompiledFloorPolicies`.
- Produces (error): `ERROR_REASON_TOOL_POLICY_MODE_TRANSITION_INVALID = 61;` wire `tool-policy-mode-transition-invalid`; Rust `ErrorReason::ToolPolicyModeTransitionInvalid`.
- Produces (a rejection reason): `ERROR_REASON_TOOL_POLICY_HISTORY_UNSUPPORTED = 62;` wire `tool-policy-history-unsupported`; Rust `ErrorReason::ToolPolicyHistoryUnsupported`. Doc comment: "rejection: the set has a history rule (formerly, count_within, sum_within). The gateway cannot enforce history rules yet, so IAM does not store them. Temporary; removed by SMA-753 (R19)." 

Doc comments to write (they are the contract with SMA-752 and SMA-753):
- `ToolPolicySettings`: with no row, IAM answers `OFF` / `CLOSED` and no `updated_at`. The mode and the fail mode apply only to the tenant set; the floor is enforced in every mode (spec §3.4). `UNSPECIFIED` in a response is malformed: a reader fails closed.
- `PutToolPolicySettingsRequest`: the first change away from `OFF` with no tenant set must be to `LOG_ONLY`, and it saves the starter as version 1; `OFF` → `ENFORCE` with no tenant set is `tool-policy-mode-transition-invalid` (spec §9.2). `UNSPECIFIED` is `missing-required-field`.
- `GetCompiledToolPoliciesResponse`: an org with no tenant set gets a set with `version = 0`, empty `cedar_policies` and no conditions. A `held` or `discarded` version is never returned (`tool-policy-version-not-found`). `floor_version` is the live floor version at the time of the read; the floor is fetched with `GetCompiledFloorPolicies`, so one response stays below 4 MiB.
- `GetCompiledFloorPoliciesResponse`: a platform with no floor gets `version = 0`.
- `ERROR_REASON_TOOL_POLICY_MODE_TRANSITION_INVALID`: "the mode change is not allowed: an organization with no tool policy set must go to log_only first (spec §9.2)".

- [ ] **Step 1: Write the failing test.** In `rs/crates/libs/paigasus-proto/src/error.rs`, extend `the_tool_policy_reasons_resolve_both_ways`:

```rust
assert_eq!(ErrorReason::from_wire_reason("tool-policy-mode-transition-invalid"), Some(ErrorReason::ToolPolicyModeTransitionInvalid));
assert_eq!(ErrorReason::ToolPolicyModeTransitionInvalid as i32, 61);
assert_eq!(ErrorReason::from_wire_reason("tool-policy-history-unsupported"), Some(ErrorReason::ToolPolicyHistoryUnsupported));
assert_eq!(ErrorReason::ToolPolicyHistoryUnsupported as i32, 62);
```

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-proto the_tool_policy_reasons`. Expected: FAIL to compile (`ToolPolicyModeTransitionInvalid` not found).
- [ ] **Step 3: Edit both protos, then** `export PATH=…; cd contracts && buf format -w && cd .. && moon run contracts:generate --force`. Add both wire strings to the list in `error.rs` (after `"tool-policy-version-not-found"`). Add `[ErrorReason.TOOL_POLICY_MODE_TRANSITION_INVALID]: 'from-transport',` and `[ErrorReason.TOOL_POLICY_HISTORY_UNSUPPORTED]: 'from-transport',` to `presentation.ts` next to the other tool-policy codes (PR 1 lists the rejection reasons there too, for example `TOOL_POLICY_SHAPE_UNSUPPORTED`).
- [ ] **Step 4: Run.** `cargo nextest run --locked -p paigasus-proto`, `moon run contracts:lint contracts:breaking paigasus-sdk-ts:test paigasus-sdk-ts:typecheck`. Expected: PASS. `git status` shows generated files in all three languages.
- [ ] **Step 5: Commit** `feat(contracts): add the tool policy settings, the fetch RPCs and two error codes (SMA-775)`.

---

### Task 2: IAM core authz — two actions, the `tool_policy_reader` role, revision 6

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/{action.rs, schema.rs, roles.rs}`

**Interfaces:**
- Produces: `Action::ManageToolPolicySettings` (wire `"ManageToolPolicySettings"`, `is_write() == true`, not a restore), `Action::ReadCompiledToolPolicies` (wire `"ReadCompiledToolPolicies"`, `is_write() == false`). Both in `Action::ALL` (length test +2) and in the wildcard-free exhaustiveness match.
- Produces: role `Role { key: "tool_policy_reader", template_id: "tool_policy_reader", scope_kinds: vec![NodeKind::Root], description: "Read the compiled tool policies of every organization and the floor (the gateway's service identity).", system: true }`, template actions `TOOL_POLICY_READER_ACTIONS = &[Action::ReadCompiledToolPolicies]`; `"tool_policy_reader"` appended to `STARTER_POLICY_IDS` after `"gateway_user"`.
- Produces: `ORG_ADMIN_ACTIONS` gains `Action::ManageToolPolicySettings`. No other role gains either action (R9, R18).
- Produces: `STARTER_POLICY_REVISION = 6`, with a doc paragraph: "`6`: SMA-775 added `ManageToolPolicySettings` (a non-restore write, so it joins the generated forbid list) and the `tool_policy_reader` role template."
- `SCHEMA_SRC`: both actions copy the `appliesTo` principal types and context of `ReadToolPolicyFloor`, with the resource types `[Root, Organization]`.

- [ ] **Step 1: Write the failing tests** in `roles.rs` and `action.rs`:

```rust
// roles.rs starter_policy_table() rows
"org_admin allows ManageToolPolicySettings on its own org (SMA-775)"           // allow
"org_member denies ManageToolPolicySettings on its org (SMA-775)"             // deny
"tool_policy_reader at Root allows ReadCompiledToolPolicies on any org (SMA-775)" // allow, org_o
"tool_policy_reader at Root allows ReadCompiledToolPolicies at Root (SMA-775)"  // allow, root
"tool_policy_reader at Root denies ReadToolPolicies on an org (SMA-775)"        // deny
"gateway_user denies ReadCompiledToolPolicies on its org (SMA-775)"             // deny (spec §3.6)
"org_admin denies ReadCompiledToolPolicies on its own org (SMA-775)"            // deny (R18)
"forbid-archived-writes denies ManageToolPolicySettings on an archived org even for org_admin (SMA-775)"
#[test] fn the_settings_write_is_in_the_generated_forbid_source_and_the_compiled_read_is_not() {}
#[test] fn tool_policy_reader_is_root_only() { assert_eq!(role("tool_policy_reader").unwrap().scope_kinds, vec![NodeKind::Root]); }
// action.rs: extend the is_write table and the wire round-trip with the two actions.
```

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-iam-core authz`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** the actions, the schema entries, the role and its template branch in `template_source`. Bump the revision. Run `starter_policy_content_is_pinned_to_the_declared_revision` once to read the new hash from its failure message, and set `EXPECTED_STARTER_CONTENT_HASH`.
- [ ] **Step 4: Run.** Same command, then `cargo nextest run --locked -p paigasus-iam --lib roles bootstrap` and `--test authz_schema --test authz_bootstrap` (Docker). Fix any count of system roles that a test pins (8 → 9 roles, 9 → 10 starter ids) and any module doc that says "eight system roles". Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): add the tool policy settings and compiled-read actions and a reader role (SMA-775)`.

---

### Task 3: IAM core — event types, settings model, repository port, kernel normaliser; NATS docs and test

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/domain_event.rs`, `src/tool_policy/{model.rs, name.rs, mod.rs}`, `rs/crates/libs/paigasus-iam-core/Cargo.toml`, `rs/Cargo.lock` (only if it changes), `ops/nats/permissions.md`, `rs/crates/services/paigasus-iam/tests/nats_permissions.rs`

**Interfaces:**
- Produces (`domain_event.rs`): `EventType::ToolPolicySetPublished` → `"iam.policy.tool_set.published"`, `EventType::ToolFloorPublished` → `"iam.policy.tool_floor.published"`, `EventType::ToolPolicySettingsChanged` → `"iam.policy.tool_settings.changed"`; `ALL` length 25; `parse` and the exhaustive match updated.
- Produces (`tool_policy/model.rs`, `pub`, re-exported from `tool_policy/mod.rs`):
  ```rust
  pub enum ToolPolicyMode { Off, LogOnly, Enforce }          // as_str: "off" | "log_only" | "enforce"; parse
  pub enum FailMode { Open, Closed }                         // as_str: "open" | "closed"; parse
  pub struct ToolPolicySettings { pub mode: ToolPolicyMode, pub fail_mode: FailMode, pub updated_at: Option<DateTime<Utc>>, pub updated_by: Option<String> }
  impl ToolPolicySettings { pub fn default_for_absent_row() -> Self /* Off, Closed, None, None */ }
  ```
- Produces (`name.rs`): `name_key(qualified)` = `paigasus_kernel::tool_policy::normalise(qualified)`; the doc comment drops "PR 2 changes this" and states that the ASCII grammar makes the result equal to ASCII lowercase. `Cargo.toml`: `paigasus-kernel = { workspace = true, features = ["tool-policy"] }` with a reason comment (spec §4.1, one normaliser, ADR-0005).

The repository methods and `VersionOrigin::Starter` are in Task 4: they need the m0014 table and the Pg mapping, and an unmapped enum arm does not compile.

- [ ] **Step 1: Write the failing tests:**

```rust
// domain_event.rs: the existing round-trip and namespacing tests cover the new variants; update the
// exhaustive match and assert_eq!(EventType::ALL.len(), 25).
#[test] fn the_tool_policy_events_have_four_segments_under_iam_policy() {
    for et in [EventType::ToolPolicySetPublished, EventType::ToolFloorPublished, EventType::ToolPolicySettingsChanged] {
        let w = et.as_wire(); assert!(w.starts_with("iam.policy.")); assert_eq!(w.split('.').count(), 4);
    }
}
// model.rs
#[test] fn mode_and_fail_mode_round_trip_their_wire_strings() {}
#[test] fn an_absent_row_reads_as_off_and_closed() {}
// name.rs: the PR 1 tests stay unchanged and must pass on the kernel call.
#[test] fn name_key_equals_ascii_lowercase_on_the_grammar() { for n in ["GitHub.Search", "a_B-9", "X"] { assert_eq!(name_key(n), n.to_ascii_lowercase()); } }
```

`tests/nats_permissions.rs`: extend `the_consumer_core_subscription_receives_a_jetstream_policy_publish` (or add `the_consumer_core_subscription_receives_each_tool_policy_event`) so the publisher sends one event of each of the three new types through the real `NatsEventPublisher`, and the `gateway-consumer` core subscription on `iam.policy.>` receives all three subjects.

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-iam-core domain_event tool_policy`. Expected: FAIL to compile.
- [ ] **Step 3: Implement.** Update `ops/nats/permissions.md`: "The publishable surface is 22 subjects" → 25, with one sentence that SMA-775 added the three `iam.policy.tool_*` subjects, that they sit under `iam.>` and `iam.policy.>`, and that `subjects.env` does not change. Update the "22 variants" comment in `tests/nats_permissions.rs`.
- [ ] **Step 4: Run.** Same command; `cargo nextest run --locked -p paigasus-iam --lib cloud_event`; then `/opt/homebrew/bin/bash ops/nats/check-subjects.sh` and `cd rs && cargo nextest run --locked --no-tests=pass -p paigasus-iam --test nats_permissions --test docker_preflight --profile iam-nats` (Docker). Expected: PASS, not skipped.
- [ ] **Step 5: Commit** `feat(rs): add the tool policy events and settings model, and use the kernel normaliser (SMA-775)`.

---

### Task 4: IAM persistence — m0014 settings table and the repository methods

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/migration/m0014_create_tool_policy_settings.rs`, `src/adapters/persistence/entities/tool_policy_settings.rs`, `tests/tool_policies_settings_pg.rs`
- Modify: `migration/mod.rs`, `entities/mod.rs`, `pg_tool_policies.rs` (origin `starter` mapping + the two methods), `rs/crates/libs/paigasus-iam-core/src/tool_policy/{ports.rs, model.rs}`, `src/application/fakes.rs` (`InMemoryToolPolicies` implements both), `src/application/tool_policies/mod.rs` (`origin_str`), `src/adapters/grpc/tool_policies.rs` (origin to proto `STARTER`)

**Interfaces:**
- Consumes: Task 3 `ToolPolicySettings`, `ToolPolicyMode`, `FailMode`.
- Produces (`model.rs`): `VersionOrigin::Starter`, mapped to `'starter'` in Postgres (the m0013 CHECK already allows it), to `"starter"` in `origin_str`, and to `TOOL_POLICY_VERSION_ORIGIN_STARTER` on the wire.
- Produces (`ports.rs`, `ToolPolicyRepository` additions), implemented by `PgToolPolicies` and `InMemoryToolPolicies`:
  ```rust
  async fn settings(&self, org: Uuid) -> Result<Option<ToolPolicySettings>, RepositoryError>;
  async fn upsert_settings_in(&self, tx: &dyn Transaction, org: Uuid, s: &ToolPolicySettings) -> Result<(), RepositoryError>;
  ```

**m0014 DDL** (`SET LOCAL lock_timeout = '5s'`, as m0013):
`tool_policy_settings (org_id UUID PRIMARY KEY REFERENCES organization(id) ON DELETE CASCADE, mode TEXT NOT NULL CHECK (mode IN ('off','log_only','enforce')), fail_mode TEXT NOT NULL CHECK (fail_mode IN ('open','closed')), updated_at TIMESTAMPTZ NOT NULL, updated_by TEXT NOT NULL)`. `down` drops the table. `upsert_settings_in` is `INSERT … ON CONFLICT (org_id) DO UPDATE`.

- [ ] **Step 1: Write the failing Postgres tests** in `tests/tool_policies_settings_pg.rs` (seed one org as `tests/tool_policies_pg.rs` does):

```rust
#[tokio::test] async fn settings_are_none_for_an_org_with_no_row() {}
#[tokio::test] async fn upsert_inserts_then_updates_one_row() { /* log_only/closed, then enforce/open; one row; updated_by is the last actor */ }
#[tokio::test] async fn an_unknown_mode_is_refused_by_the_check() { /* raw INSERT mode 'strict' → check violation */ }
#[tokio::test] async fn deleting_the_org_cascades_to_its_settings() {}
#[tokio::test] async fn a_starter_origin_version_round_trips() { /* insert_live_version_in with VersionOrigin::Starter; live() returns origin Starter */ }
#[tokio::test] async fn an_upsert_rolls_back_with_its_transaction() { /* upsert_settings_in, drop tx; settings() == None */ }
```

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-iam --test tool_policies_settings_pg`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** the migration (register `m0014_create_tool_policy_settings`), the entity, the Pg methods (errors through `persistence::map_err`) and the fake.
- [ ] **Step 4: Run.** Same command, then `--lib migration` and `--test tool_policies_pg`. Expected: PASS, not skipped.
- [ ] **Step 5: Commit** `feat(rs): store the tool policy settings of each organization (SMA-775)`.

---

### Task 5: IAM application — refuse history rules until SMA-753

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/application/tool_policies/{mod.rs, rejection.rs, sets.rs}`, `src/adapters/grpc/tool_policies.rs` (only if `rejection_to_pb` needs the new arm), `tests/grpc_tool_policies.rs`, `tests/http_tool_policies.rs`, `tests/tool_policies_real_compiler.rs` (all under `rs/crates/services/paigasus-iam/`)

**Interfaces:**
- Consumes: Task 1 `ErrorReason::ToolPolicyHistoryUnsupported`; PR 1 `compile`, `CompiledOutput.conditions`.
- Produces (`rejection.rs`): `RejectionReason::HistoryUnsupported`, `code()` = `"tool-policy-history-unsupported"` (proto value 62). It is IAM-only: `from_wire` (the compiler's six codes) does not accept it.
- Produces (`sets.rs`):
  ```rust
  /// SMA-753: set this to `true`, or delete it with `reject_history_until_gateway_supports_it`,
  /// when the gateway evaluates history conditions. Until then IAM stores no set with one (R19).
  pub(crate) const HISTORY_CONDITIONS_ENFORCED_BY_GATEWAY: bool = false;
  /// One rejection per policy id that has a history condition, sorted by policy id.
  pub(crate) fn reject_history_until_gateway_supports_it(out: &CompiledOutput) -> Vec<ToolPolicyRejection>;
  fn history_rejections(out: &CompiledOutput, enforced_by_gateway: bool) -> Vec<ToolPolicyRejection>; // empty when `enforced_by_gateway`
  ```
  Each rejection has `policy_id` = the condition's `policy_id`, no location, and this exact message: `"policy <id> uses a history rule (formerly, count_within or sum_within). The gateway cannot enforce history rules yet, so this set cannot be saved. Remove the temporal block, or wait for history support."`
- The one call site is the PR 1 `compile` helper, right after `check_compiled` passes: a non-empty result → `Compiled::Rejected`. So the tenant save, the floor save, the catalog coupling (re-tagged `CatalogBreaksPolicy` as PR 1 does, with the reason code in the message) and the starter all get it, and nothing is stored. Put a `// SMA-753:` comment on the call site too.

- [ ] **Step 1: Write the failing tests:**

```rust
// application/tool_policies/mod.rs (FakePolicyCompiler returns an accepted output with conditions)
#[tokio::test] async fn a_tenant_save_with_a_history_condition_is_rejected_and_stores_nothing() { /* reason HistoryUnsupported, policy_id set; repo.writes() == 0; no audit entry, no event */ }
#[tokio::test] async fn a_floor_save_with_a_history_condition_is_rejected_and_stores_nothing() {}
#[tokio::test] async fn a_set_with_no_history_condition_still_saves() { /* conditions empty → Accepted, version 1 */ }
#[tokio::test] async fn each_policy_with_a_history_condition_gets_one_rejection() { /* two conditions of policy a, one of policy b → two rejections, a then b */ }
// sets.rs
#[test] fn the_history_check_is_empty_when_the_gateway_enforces_history() { /* history_rejections(out_with_conditions, true) is empty */ }
// rejection.rs: every_rejection_reason_is_declared_in_the_canonical_registry covers the new variant;
//   every_compiler_reason_in_the_shared_fixture_parses still expects exactly the six compiler codes.
// tests/grpc_tool_policies.rs: every_rejection_reason_reaches_the_wire_in_the_rejected_oneof gains the ninth reason.
// tests/http_tool_policies.rs: a_history_rule_is_200_with_result_rejected_and_its_code().
// tests/tool_policies_real_compiler.rs: a_count_within_policy_round_trips_through_the_real_compiler becomes
//   a_count_within_policy_is_refused_until_the_gateway_supports_history (real compiler; reason
//   tool-policy-history-unsupported). The compiler workspace tests still prove the translation.
```

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-iam --lib tool_policies`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** the variant, the two functions and the call. The existing `rejection.rs` `emits` row in `ci/error-registry/check.py` covers the new code through its membership test; add no row unless `--single-site` asks for one.
- [ ] **Step 4: Run.** Same command; then `--test grpc_tool_policies --test http_tool_policies`; then `moon run paigasus-iam-rs:test` for the real-compiler test (check in the log that it ran, not skipped); then `bash ci/python-env/run.sh python3 ci/error-registry/check.py --single-site` (or the form that `repo:error-code-single-site` uses). Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): refuse tool policy history rules until the gateway enforces them (SMA-775)`.

---

### Task 6: IAM application — outbox events on every new live version

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/application/tool_policies/events.rs`
- Modify: `src/application/tool_policies/mod.rs` (deps, `save`, `catalog_write`, module doc), `src/adapters/http/mod.rs` (`with_tool_policies` passes the outbox), `src/application/fakes.rs` (only if `FakeOutbox` needs a helper)

**Interfaces:**
- Consumes: Task 3 event types; `paigasus_iam_core::{DomainEvent, Outbox}`.
- Produces: `ToolPolicyServiceDeps.outbox: Arc<dyn Outbox>`.
- Produces (`events.rs`):
  ```rust
  pub(crate) fn set_published(ids: &impl IdGenerator, actor: &Prn, org_prn: &Prn, org: Uuid, v: &ToolPolicyVersion, now: DateTime<Utc>) -> DomainEvent;
  pub(crate) fn floor_published(ids: &impl IdGenerator, actor: &Prn, v: &ToolPolicyVersion, now: DateTime<Utc>) -> DomainEvent;
  pub(crate) fn settings_changed(ids: &impl IdGenerator, actor: &Prn, org_prn: &Prn, org: Uuid, s: &ToolPolicySettings, now: DateTime<Utc>) -> DomainEvent;
  ```
  `schema_version = 1`. `aggregate_prn` = the org PRN, or `root_prn()` for the floor. Payloads (exact keys): set `{"org_id": "<uuid>", "version": n, "origin": "save"|"starter"|"catalog_change"}`; floor `{"version": n, "origin": "save"}`; settings `{"org_id": "<uuid>", "mode": "off"|"log_only"|"enforce", "fail_mode": "open"|"closed"}`. The correlation id is the one in the audit entry of the same write.

Rule (R14): `save` enqueues `set_published` or `floor_published` after `insert_live_version_in` and before `audit.record`. `catalog_write` enqueues `set_published` only when it wrote a `CatalogChange` version. Remove "PR 1 writes no outbox event (D9)" from the module doc.

- [ ] **Step 1: Write the failing unit tests** in `application/tool_policies/mod.rs` (the harness gains a `FakeOutbox`):

```rust
#[tokio::test] async fn a_tenant_save_writes_one_set_published_event() { /* event_type, aggregate_prn == org PRN, payload {"org_id","version":1,"origin":"save"} */ }
#[tokio::test] async fn a_floor_save_writes_one_floor_published_event() { /* aggregate root PRN, payload {"version":1,"origin":"save"} */ }
#[tokio::test] async fn a_catalog_change_version_writes_a_set_published_event() { /* origin "catalog_change" */ }
#[tokio::test] async fn a_catalog_write_with_identical_output_writes_no_event() {}
#[tokio::test] async fn a_byte_identical_save_writes_no_event() {}
#[tokio::test] async fn a_rejected_or_conflicting_write_writes_no_event() { /* rejection, version conflict between compile and lock */ }
#[tokio::test] async fn the_event_and_the_audit_entry_share_one_correlation_id() {}
```

Add one Postgres check to `tests/tool_policies_pg.rs` or the existing outbox-reading pattern of `tests/tenancy_events_pg.rs`: `a_committed_save_leaves_one_outbox_row_with_the_four_segment_subject` (reads `event_outbox.event_type`).

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-iam --lib tool_policies`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** `events.rs` and the two enqueue sites.
- [ ] **Step 4: Run.** Same command, plus `--test tool_policies_pg --test grpc_tool_policies`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): publish a tool policy event for each new live version (SMA-775)`.

---

### Task 7: IAM application — settings, the mode rules and the starter on first enable

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/application/tool_policies/settings.rs`
- Modify: `src/application/tool_policies/mod.rs` (two public methods; `save` gains an `origin` parameter or a private helper shared with the starter), `src/application/error.rs`, `src/adapters/retryable.rs`, `src/adapters/http/error.rs`, `src/adapters/grpc/convert.rs`

**Interfaces:**
- Consumes: Tasks 3-6; PR 1 `STARTER_TOOL_POLICY_SOURCE`.
- Produces (`error.rs`): `TenancyError::ToolPolicyModeTransitionInvalid` (unit variant), `code()` = `"tool-policy-mode-transition-invalid"`, `class()` = `Precondition` (→ gRPC `FailedPrecondition`, HTTP 409), retryable `No`.
- Produces (`mod.rs`):
  ```rust
  pub struct SettingsAccepted { pub settings: ToolPolicySettings, pub starter_version: u64 /* 0 = none */ }
  pub async fn get_settings(&self, actor: &Prn, org: Uuid) -> Result<ToolPolicySettings, TenancyError>;
  pub async fn put_settings(&self, actor: &Prn, org: Uuid, mode: ToolPolicyMode, fail_mode: FailMode) -> Result<SettingsAccepted, TenancyError>;
  ```
- Produces (`settings.rs`, pure): `pub(crate) fn check_transition(current: ToolPolicyMode, next: ToolPolicyMode, has_set: bool) -> Result<bool /* save the starter */, TenancyError>`.

**Rules the tests pin (spec §9.2, R9, R12, R13):**
- `get_settings`: `ReadToolPolicies` at the org PRN (`authorize_org`); no row → `default_for_absent_row()`.
- `put_settings`: `ManageToolPolicySettings` at the org PRN.
- `check_transition` (`has_set` = `state.live_version > 0`): `next == Off` → ok, no starter. `current == Off && next == LogOnly && !has_set` → ok, save the starter. `current == Off && next == Enforce && !has_set` → `ToolPolicyModeTransitionInvalid`. `next != Off && has_set` → ok, no starter. When `!has_set`, `current` is treated as `Off` (a row in `log_only` or `enforce` always has a set, but the rule must not depend on that).
- Write steps: (1) read `state`, the current settings, and decide. (2) If the starter is due: read the catalog and compile `STARTER_TOOL_POLICY_SOURCE` with the PR 1 `compile` helper, no transaction open. Busy / unavailable map as in PR 1. A rejection of the starter is `TenancyError::Internal` with an `error!` log (it cannot happen; PR 1 tests compile it against an empty catalog). (3) `uow.begin()`, `lock_state_in`; a locked state that differs from step 1 → `ToolPolicyVersionConflict`, nothing written. (4) If the starter is due: `insert_live_version_in` (version 1, `origin = Starter`, `created_by` = actor) and enqueue `set_published` (origin `"starter"`). Then `upsert_settings_in`, enqueue `settings_changed`, one audit entry, commit.
- Audit entry: `action = "PutToolPolicySettings"`, `resource_prn` = org PRN, `detail = {"mode": "...", "fail_mode": "...", "starter_version": <n or null>}`.
- A write with the same mode and fail mode as the stored row still writes (the row's `updated_at` moves, one event, one audit entry). Keep it simple; no no-op shortcut.

- [ ] **Step 1: Write the failing unit tests:**

```rust
// settings.rs
#[test] fn the_transition_table_holds() { /* every (current, next, has_set) of the 3x3x2 table → the result above */ }
// mod.rs
#[tokio::test] async fn get_settings_of_an_org_with_no_row_is_off_and_closed() {}
#[tokio::test] async fn the_first_enable_to_log_only_saves_the_starter_as_version_1() { /* live v1, origin Starter, source == STARTER_TOOL_POLICY_SOURCE; starter_version == 1 */ }
#[tokio::test] async fn the_first_enable_writes_two_events_and_one_audit_entry() { /* set_published(origin starter) + settings_changed; audit detail starter_version 1 */ }
#[tokio::test] async fn off_to_enforce_with_no_set_is_a_mode_transition_error_and_writes_nothing() {}
#[tokio::test] async fn off_to_enforce_with_a_set_is_allowed_and_saves_no_starter() {}
#[tokio::test] async fn log_only_with_an_existing_set_saves_no_starter() { /* a set saved before any settings write */ }
#[tokio::test] async fn a_fail_mode_change_alone_is_allowed_in_every_mode() {}
#[tokio::test] async fn any_mode_can_go_to_off() {}
#[tokio::test] async fn a_set_saved_during_the_first_enable_is_a_version_conflict() { /* ConcurrentChange between compile and lock → ToolPolicyVersionConflict; no version, no settings row, no event, no audit */ }
#[tokio::test] async fn a_busy_compiler_during_the_first_enable_writes_nothing() {}
#[tokio::test] async fn the_settings_methods_check_their_actions() { /* Deny → Forbidden; the compiler was not called */ }
#[tokio::test] async fn an_unknown_org_is_not_found() {}
// error.rs: every_tenancy_code_is_declared_in_the_canonical_registry covers the new variant (EnumIter).
```

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-iam --lib tool_policies error`. Expected: FAIL to compile.
- [ ] **Step 3: Implement.** Reuse the PR 1 write steps; do not copy them. If `save` cannot take the starter case cleanly, extract one private helper `write_new_live_version(tx, …, origin)` that both call.
- [ ] **Step 4: Run.** Same command. Then `bash ci/python-env/run.sh python3 ci/error-registry/check.py --self-test` and `--single-site` (or the form `repo:error-code-single-site` uses). Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): add the tool policy settings with the starter policy on first enable (SMA-775)`.

---

### Task 8: IAM application — the compiled-policy fetch

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/application/tool_policies/fetch.rs`
- Modify: `src/application/tool_policies/mod.rs`

**Interfaces:**
- Consumes: Task 3 settings; PR 1 `read_version`, `ToolPolicyVersion.compiled` (proto bytes).
- Produces:
  ```rust
  pub struct CompiledTenant { pub set: Option<ToolPolicyVersion> /* None = version 0 */, pub settings: ToolPolicySettings, pub floor_version: u64 }
  pub async fn get_compiled(&self, actor: &Prn, org: Uuid, version: Option<u64>) -> Result<CompiledTenant, TenancyError>;
  pub async fn get_compiled_floor(&self, actor: &Prn, version: Option<u64>) -> Result<Option<ToolPolicyVersion>, TenancyError>;
  ```
- Rules: `get_compiled` authorizes `ReadCompiledToolPolicies` at the org PRN; `get_compiled_floor` at `root_prn()` (never a PRN from the request). The version read is PR 1 `read_version` (an explicit `held` / `discarded` / absent version → `ToolPolicyVersionNotFound`). `floor_version` = `repo.state(&Floor).live_version`. The transport (Task 9) turns `None` into the empty `version = 0` message. Reads hold no lock and write no audit entry.

- [ ] **Step 1: Write the failing unit tests:**

```rust
#[tokio::test] async fn get_compiled_of_an_org_with_no_set_is_none_with_default_settings() {}
#[tokio::test] async fn get_compiled_returns_the_live_set_the_settings_and_the_floor_version() {}
#[tokio::test] async fn get_compiled_by_version_returns_a_superseded_version() {}
#[tokio::test] async fn get_compiled_of_a_held_version_is_version_not_found() { /* InMemoryToolPolicies seeds a held row */ }
#[tokio::test] async fn the_fetch_of_an_org_in_mode_off_returns_the_floor_version() { /* settings off; floor v2 live → floor_version 2; the set is still returned */ }
#[tokio::test] async fn get_compiled_floor_with_no_floor_is_none() {}
#[tokio::test] async fn the_fetch_methods_check_read_compiled_tool_policies() { /* Deny → Forbidden; the floor check uses root_prn() */ }
```

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-iam --lib tool_policies::fetch tool_policies::tests`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** `fetch.rs`.
- [ ] **Step 4: Run.** Same command. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): read the compiled tool policies for the gateway (SMA-775)`.

---

### Task 9: IAM transports — four RPCs over gRPC and HTTP, and the service identity end to end

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/grpc_tool_policies_fetch.rs`
- Modify: `src/adapters/grpc/tool_policies.rs`, `src/adapters/http/tool_policies.rs`, `src/adapters/http/dto.rs` (only if the settings DTO lives there), `tests/http_tool_policies.rs`, `tests/support/mod.rs`

**Interfaces:**
- Consumes: Tasks 1, 7, 8.
- Produces (gRPC): the four `ToolPolicyService` methods. Each first calls `self.svc()` (capability, R11). Enum conversion: `UNSPECIFIED` or an unknown number in a request → `missing-required-field` (the PR 1 `kind_from_wire` pattern). A `None` set → `CompiledToolPolicySet { kind, org_id: "", version: 0, ..Default }` with `kind` set (`TENANT` / `FLOOR`). The set is decoded from the stored bytes with the same conversion `get_tool_policy_set` uses.
- Produces (HTTP, mounted with the PR 1 router, same `compiled_json` helper):
  - `GET /v1/organizations/{id}/tool-policy-settings` → `{"mode","fail_mode","updated_at","updated_by"}`
  - `PUT /v1/organizations/{id}/tool-policy-settings` body `{"mode","fail_mode"}` → `{"settings":{…},"starter_version":n}`
  - `GET /v1/organizations/{id}/compiled-tool-policies?version=N` → `{"set":{…compiled_json…},"settings":{…},"floor_version":n}`
  - `GET /v1/compiled-tool-policy-floor?version=N` → `{"set":{…}}`
- Produces (`tests/support/mod.rs`): `pub async fn gateway_reader_key(state: &AppState, platform_admin: &Prn) -> (Uuid /* platform org */, String /* key */)`. It runs the Task 12 runbook steps 1-4 in the same order, through the same application services that the RPCs use, all as `platform_admin`: (1) create the organization `paigasus-platform`; (2) create the service account `paigasus-gateway` owned by it; (3) issue an API key with scope = that organization, no expiry, empty `scope_actions` and `scope_roles`; (4) grant `tool_policy_reader` to the account at Root. It returns the org id and the plaintext key.

- [ ] **Step 1: Write the failing tests** in `tests/grpc_tool_policies_fetch.rs` (Postgres + a fake compiler script, as `tests/grpc_tool_policies.rs`):

```rust
#[tokio::test] async fn the_four_rpcs_answer_capability_disabled_when_the_toggle_is_off() {}
#[tokio::test] async fn the_gateway_key_reads_the_compiled_set_of_any_org_and_the_floor() { /* gateway_reader_key; Bearer key; two tenant orgs other than the platform org: both readable; floor readable. The key scope is the platform org, so this also proves that the key scope does not limit the fetch */ }
#[tokio::test] async fn the_gateway_key_without_the_grant_reads_nothing() { /* runbook steps 1-3 only → PERMISSION_DENIED on both fetch RPCs */ }
#[tokio::test] async fn the_gateway_key_cannot_read_the_source_or_write() { /* GetToolPolicySet, PutToolPolicySettings, ListTools → PERMISSION_DENIED */ }
#[tokio::test] async fn a_gateway_user_key_cannot_read_compiled_policies() { /* spec §3.6 */ }
#[tokio::test] async fn an_org_admin_cannot_read_compiled_policies() { /* R18 */ }
#[tokio::test] async fn an_org_with_no_set_gets_version_0_and_off_closed() {}
#[tokio::test] async fn the_first_enable_over_grpc_returns_starter_version_1_and_the_fetch_sees_it() { /* compiled.cedar_policies non-empty, catalog_names [], settings LOG_ONLY */ }
#[tokio::test] async fn off_to_enforce_with_no_set_is_failed_precondition_with_its_reason() { /* support::reason_of == "tool-policy-mode-transition-invalid" */ }
#[tokio::test] async fn a_held_version_is_not_found_over_grpc() { /* raw SQL held row v2; version=2 → NOT_FOUND "tool-policy-version-not-found" */ }
#[tokio::test] async fn an_unspecified_mode_is_missing_required_field() {}
#[tokio::test] async fn the_first_enable_commits_two_outbox_rows() { /* event_outbox: iam.policy.tool_set.published + iam.policy.tool_settings.changed, org_id in both payloads */ }
// tests/http_tool_policies.rs
#[tokio::test] async fn the_settings_and_fetch_routes_are_404_when_the_capability_is_off() {}
#[tokio::test] async fn http_and_grpc_return_the_same_compiled_set() {}
#[tokio::test] async fn a_mode_transition_error_is_409_with_its_code() {}
```

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-iam --test grpc_tool_policies_fetch --test http_tool_policies`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** both adapters and the support helper.
- [ ] **Step 4: Run.** Same command plus `--test grpc_tool_policies --lib`. Expected: PASS, not skipped.
- [ ] **Step 5: Commit** `feat(rs): serve the tool policy settings and the compiled-policy fetch over gRPC and HTTP (SMA-775)`.

---

### Task 10: Gateway — settings through the port, the cache and the request side

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/domain/tool_policy.rs`, `src/adapters/policy_source/file.rs`, `src/application/tool_policy/{cache.rs, request.rs, testing.rs}`, `src/config.rs`, `gateway.toml.example`

**Interfaces:**
- Produces (`domain/tool_policy.rs`):
  ```rust
  /// The tenant settings from the source (spec §9.2, §9.3). `None` from a source with no settings (the file source).
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub struct TenantSettings { pub mode: TenantMode, pub on_unavailable: OnUnavailable }
  // TenantPolicies gains: pub settings: Option<TenantSettings>
  ```
  Update the module doc: the IAM source returns the settings too (R1).
- Produces (`cache.rs`): `pub fn tenant_settings(&self, org: Uuid) -> Option<TenantSettings>` — the settings of the last good load of that tenant, whatever its age and its `stale` flag ("the last known value", spec §9.3). It never starts a load. An evicted tenant has none.
- Produces (`config.rs`):
  - `PolicySourceConfig::Iam {}` (`source = { kind = "iam" }`).
  - `IamClientConfig.service_credential_file: Option<String>` (`#[serde(default)]`), doc: "the gateway's own IAM credential (an API key) for the two fetch RPCs only; D9 self-queries never use it (spec §9.4)".
  - `ToolPolicySettings.settings_from_source: bool` — true iff the source is `Iam`.
  - `validate` with `enabled` and kind `iam`: `iam.service_credential_file` required and not blank; any `[[tool_policy.org]]` entry → error naming `tool_policy.org` and "kind = \"iam\"" (R3). With kind `file`, nothing changes.
- Request side (`request.rs`), only when `settings_from_source`:
  1. `pre` = `cache.tenant_settings(org)`, else `TenantSettings { mode: Enforce, on_unavailable: global }` (R2).
  2. Look up the floor with `pre.on_unavailable`, then ALWAYS look up the tenant (also when `pre.mode` is `Off`), so a fresh load can change the mode.
  3. `fin` = `cache.tenant_settings(org)` after the lookup, else `pre`.
  4. Use `fin.mode` and `fin.on_unavailable` for the snapshot, the refusal check (D17) and the metrics. If `fin.mode == Off`, the tenant part is `TenantPart::Off` with `floor.floor_only` (P-4), even when the tenant lookup returned a set.
  5. A scope with no org keeps `default_mode` and the global value, as today.
  With `settings_from_source == false`, `apply_inner` and `policy_set` keep today's order and behaviour; every existing test passes unchanged.

- [ ] **Step 1: Write the failing tests** (`testing.rs` `FakeSource` gains `set_settings(Option<TenantSettings>)`, which also bumps the version; `settings()` helper gains a variant with `settings_from_source = true`):

```rust
// cache.rs
#[tokio::test] async fn tenant_settings_are_none_before_the_first_load() {}
#[tokio::test] async fn tenant_settings_survive_a_failed_reload() { /* load with settings, then the source errors; tenant_settings still Some */ }
// request.rs
#[tokio::test] async fn the_mode_from_the_source_overrides_the_default_mode() { /* default_mode Off, source Enforce → a forbidden call is blocked */ }
#[tokio::test] async fn a_changed_mode_from_the_source_applies_on_the_next_load() { /* LogOnly then Enforce after invalidate_all */ }
#[tokio::test] async fn mode_off_from_the_source_still_applies_the_floor() { /* FLOOR forbids rm; source Off; rm is blocked, a tenant-only forbid is not */ }
#[tokio::test] async fn with_no_known_settings_the_global_fail_mode_applies() { /* source fails on the first load; global Closed → every call blocked; global Open → allowed and reported */ }
#[tokio::test] async fn the_last_known_fail_mode_applies_when_the_tenant_set_becomes_unavailable() { /* first load Enforce/Open; then source error → Open fallback with the last good set */ }
#[tokio::test] async fn an_unreadable_turn_uses_the_mode_from_the_source() { /* source Enforce → refused; source LogOnly → counted, not refused */ }
// config.rs
#[test] fn the_iam_source_needs_the_service_credential_file() {}
#[test] fn the_iam_source_refuses_tool_policy_org_entries() {}
#[test] fn the_file_source_does_not_need_the_credential_file() {}
#[test] fn source_kind_iam_parses_and_sets_settings_from_source() {}
```

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-gateway --lib tool_policy config`. Expected: FAIL to compile.
- [ ] **Step 3: Implement.** Add `settings: None` at the four existing `TenantPolicies` construction sites (`file.rs`, `cache.rs` tests, `testing.rs`, `domain` doc example). Document `source = { kind = "iam" }` and `iam.service_credential_file` in `gateway.toml.example`, with the R3 rule.
- [ ] **Step 4: Run.** Same command, then `--test tool_policy --test tool_interception`. Expected: PASS; no existing test changed its assertions.
- [ ] **Step 5: Commit** `feat(rs): take the tenant mode and fail mode from the policy source in the gateway (SMA-775)`.

---

### Task 11: Gateway — the `IamPolicySource` adapter and its wiring

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/adapters/policy_source/iam.rs`, `tests/tool_policy_iam_source.rs`
- Modify: `src/adapters/policy_source/mod.rs`, `src/adapters/iam/client.rs` (`build_channel` → `pub(crate)`), `src/runtime.rs`

**Interfaces:**
- Consumes: Task 1 generated `tool_policy_service_client::ToolPolicyServiceClient`; Task 10 `TenantSettings`, `PolicySourceConfig::Iam`.
- Produces (`iam.rs`):
  ```rust
  pub struct IamPolicySource { client: ToolPolicyServiceClient<Channel>, credential_file: PathBuf }
  impl IamPolicySource { pub fn new(channel: Channel, credential_file: impl Into<PathBuf>) -> Self; }
  impl PolicySource for IamPolicySource { /* floor() → GetCompiledFloorPolicies; tenant(org) → GetCompiledToolPolicies */ }
  pub(crate) fn tenant_from_response(org: Uuid, r: GetCompiledToolPoliciesResponse) -> Result<TenantPolicies, SourceError>;
  pub(crate) fn floor_from_response(r: GetCompiledFloorPoliciesResponse) -> Result<PolicyText, SourceError>;
  ```
- `runtime.rs` `build_tool_policy`: `Some(PolicySourceConfig::Iam {})` → `IamPolicySource::new(build_channel(&config.iam)?, credential_file)`. The channel is lazy, so IAM being down at boot does not stop the gateway.

**Rules the tests pin:**
- Each call reads the credential file (R8), trims ASCII whitespace, and sends `authorization: Bearer <key>` plus the correlation metadata (`with_correlation`). An empty or unreadable file → `SourceError::Io("service credential file: <ErrorKind>")`. No message, log or error ever holds the key, the file path content, or policy text.
- The org PRN in the request is `prn:pgs:iam:::organization/<uuid hyphenated>`.
- gRPC errors → `SourceError`: `NOT_FOUND` → `NotFound("tenant" | "floor")`; every other status (including `capability-disabled`, `PERMISSION_DENIED`, `UNAVAILABLE`) → `Io("iam <code>")`.
- `tenant_from_response` → `Malformed` when: `set` or `settings` is absent; `set.kind != TENANT`; `set.version > 0` and `set.org_id != org`; mode or fail mode is `UNSPECIFIED` or unknown; `set.conditions` is not empty (R5, message "history conditions need SMA-753"). Else `TenantPolicies { text: cedar_policies, catalog: catalog_names, version: format!("t{}:{}:{}", version, mode, fail_mode), settings: Some(..) }` (R7). Version 0 is a valid empty set.
- `floor_from_response` → `Malformed` when `set` is absent, `kind != FLOOR`, or `conditions` is not empty. Else `PolicyText { text, version: format!("f{}", version) }`; version 0 is a valid empty floor (R6).
- The D9 self-query path (`IamClient::is_authorized_self`) is untouched; its tests keep passing.

- [ ] **Step 1: Write the failing tests.** Unit tests in `iam.rs` for the two pure mappers. Integration tests in `tests/tool_policy_iam_source.rs` start an in-process tonic `ToolPolicyServiceServer` on `127.0.0.1:0` (a fake that records the request metadata and returns scripted responses; every other RPC answers `Unimplemented`), and a gateway config with `iam.tls.mode = "loopback_insecure"`.

```rust
// iam.rs (unit)
#[test] fn a_tenant_response_maps_text_catalog_settings_and_token() {}
#[test] fn a_settings_change_alone_changes_the_version_token() {}
#[test] fn version_0_is_an_empty_tenant_set() {}
#[test] fn a_set_with_a_history_condition_is_malformed() {}
#[test] fn an_unspecified_mode_or_fail_mode_is_malformed() {}
#[test] fn a_set_for_another_org_is_malformed() {}
#[test] fn an_empty_floor_is_valid_and_a_floor_with_a_condition_is_malformed() {}
// tests/tool_policy_iam_source.rs
#[tokio::test] async fn the_fetch_sends_the_service_credential_as_bearer() {}
#[tokio::test] async fn a_rotated_credential_is_used_on_the_next_fetch() {}
#[tokio::test] async fn a_trailing_newline_in_the_credential_file_is_trimmed() {}
#[tokio::test] async fn a_missing_credential_file_is_an_io_error_without_the_path_content() {}
#[tokio::test] async fn not_found_maps_to_not_found_and_other_codes_to_io() {}
#[tokio::test] async fn an_org_in_mode_off_still_gets_the_floor() { /* through RequestToolPolicy + CedarDecider: fake IAM floor forbids "rm", tenant mode OFF; "rm" blocked */ }
#[tokio::test] async fn an_iam_that_is_down_at_boot_does_not_stop_the_gateway() { /* build_tool_policy with an unused port succeeds; a lookup is Unavailable */ }
```

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-gateway --lib policy_source --test tool_policy_iam_source`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** `iam.rs` and the wiring. If tonic's `server` feature is not enabled for the gateway's test build, add `tonic` to `[dev-dependencies]` with `features = ["server"]` and a reason comment (it is already in `rs/Cargo.lock` through IAM).
- [ ] **Step 4: Run.** Same command, plus `--lib iam::client` and `--test tool_policy_nats` (Docker). Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): fetch the tool policies from IAM with the gateway service credential (SMA-775)`.

---

### Task 12: Tenant guide and operator runbook

**Files:**
- Create: `docs/tool-policies/tenant-guide.md`, `docs/ops/RUNBOOK-tool-policies.md`

Content the docs must hold (plain technical English, short sentences; no code is tested here, so the review checks each item against the code):

`tenant-guide.md`:
- The modes `off`, `log_only`, `enforce` and the fail modes; the first enable must be `log_only`; it saves the starter (quote `STARTER_TOOL_POLICY_SOURCE` verbatim), which permits catalog tools only, so an unregistered tool is blocked (logged only in `log_only`).
- The floor is enforced in every mode, `off` included; a tenant cannot loosen it.
- **An off-type value makes a policy inert** (PR 1 deferred item). The gateway maps arguments by value: an integer in range is `Long`, a decimal with at most four fraction digits is `decimal`, any other number is a `String`. When the model sends a value of another type than the schema, a comparison is false or errors. An errored `permit` is skipped; an errored `forbid` denies. So guard every argument read with `has` and a type-safe comparison. Give one `permit … when { context.input has amount && context.input.amount <= 100 }` example.
- **Deny lists versus allow lists.** A `forbid` on argument values is a deny list. It is not robust: argument keys keep their case, and a model can vary values. Prefer `permit` allow lists with `has` guards (SMA-752's request).
- A catalog change that would break a live policy is rejected with `tool-catalog-breaks-policy`; change the policy first.
- History rules (`formerly`, `count_within`, `sum_within`): IAM refuses a save that holds one, with `tool-policy-history-unsupported`, until the gateway can enforce them (SMA-753, R19). The guide marks them as "not yet available".
- Every error and rejection code of spec §7 plus `tool-policy-mode-transition-invalid` and `tool-policy-history-unsupported`, one line each.

`RUNBOOK-tool-policies.md`:
- Provision the gateway identity (Sven's choice, Q1). Do all calls as a `platform_admin`, in this order. Give each step its HTTP route and its gRPC method, read from the code:
  1. `POST /v1/organizations` (`CreateOrganization`): the operator-owned organization `paigasus-platform`. No tenant uses it.
  2. `POST /v1/service-accounts` (`CreateServiceAccount`): `owner_prn` = that organization, `name` = `paigasus-gateway`.
  3. `POST /v1/service-accounts/{sa}/api-keys` (`IssueApiKey`): `scope_prn` = the platform organization, no `expires_at` (or the site's rotation period), empty `scope_actions` and `scope_roles`. The key scope does not limit the fetch; the Root grant does. Store the `token` at once: IAM shows it only once.
  4. `POST /v1/authz/role-grants` (`GrantRole`): `principal_prn` = the service account, `role_key` = `tool_policy_reader`, `scope_prn` = the Root PRN.
  5. Write the token to a file with mode 0400, mount it into the gateway, and set `iam.service_credential_file` to its path and `tool_policy.source = { kind = "iam" }`.
  6. Check: `GET /v1/compiled-tool-policy-floor` with the key answers 200; `GET /v1/organizations/{id}/tool-policy-set` with the key answers 403.
  Do not grant this account any other role. Do not archive the platform organization: that archives the account.
- Rotation: issue a new key, replace the file content, wait one refresh interval, revoke the old key. No restart.
- What the gateway does when IAM is down (last known settings; the global `on_policy_unavailable` when none), and the metrics to watch (`GATEWAY_TOOL_POLICY_CACHE_LOADS_TOTAL` `result="source_error"`).
- History rules: IAM refuses them until SMA-753 (R19). The gateway also refuses a stored set with one (R5); under a global `closed`, such a floor blocks every call of every tenant. A set saved before this PR can hold one: read the floor and each tenant set, and save it again without the temporal block.
- The chart does not wire this yet (R16).

- [ ] **Step 1: Write both files.**
- [ ] **Step 2: Check** every code name, config key, RPC name and route in the docs with `grep` against the code of Tasks 1-11. Expected: every name exists.
- [ ] **Step 3: Commit** `docs(docs): add the tool policy tenant guide and the gateway identity runbook (SMA-775)`.

---

### Task 13: Mutation proofs and the full gate graph

**Files:** none new. Temporary edits only, each removed by deleting the marked insert (do not use `git checkout --` to restore, memory `mutation-restore-discards-the-fix`).

- [ ] **Step 1: Mutation battery.** For each rule: insert `// MUTATION` lines that remove or bypass the rule, run the named test with `--no-fail-fast`, confirm that the mutant COMPILES and the test FAILS, then delete the insert. Record each result in the PR body.

| Rule | Mutation | Must fail |
|---|---|---|
| mode rule | `check_transition` allows `Off → Enforce` with no set | `off_to_enforce_with_no_set_is_a_mode_transition_error_and_writes_nothing` |
| starter lock recheck | skip the locked-state comparison in `put_settings` | `a_set_saved_during_the_first_enable_is_a_version_conflict` |
| outbox on save | remove the `set_published` enqueue in `save` | `a_tenant_save_writes_one_set_published_event` |
| role scope | add `ReadCompiledToolPolicies` to `GATEWAY_USER_ACTIONS` | `a_gateway_user_key_cannot_read_compiled_policies` |
| floor in mode off | request side uses `TenantPart::Off` with an empty floor when `fin.mode == Off` | `mode_off_from_the_source_still_applies_the_floor` |
| version token | drop the settings from the tenant token | `a_settings_change_alone_changes_the_version_token`, `a_changed_mode_from_the_source_applies_on_the_next_load` |
| history guard (gateway) | `tenant_from_response` ignores `conditions` | `a_set_with_a_history_condition_is_malformed` |
| history refusal (IAM) | `compile` discards the result (`let _ = reject_history_until_gateway_supports_it(&out);`, so the function stays used and the mutant compiles) | `a_tenant_save_with_a_history_condition_is_rejected_and_stores_nothing`, `a_floor_save_with_a_history_condition_is_rejected_and_stores_nothing` |
| unknown settings | the R2 fallback uses `Off` | `with_no_known_settings_the_global_fail_mode_applies` |
| credential reload | cache the key at `new` | `a_rotated_credential_is_used_on_the_next_fetch` |

After the last delete, run every test of the battery once more without mutations (memory `mutation-battery-rerun-whole`), and confirm that `git diff` shows no `MUTATION` text.

- [ ] **Step 2: Run the full gate graph** (root `CLAUDE.md`, the command between the `ci-targets` markers; do not copy it anywhere). Run it under system `/bin/bash` 3.2 for the whole `moon ci` invocation, with `--base origin/main --include-relations`. Then re-run directly the gates that need another bash, and read those results instead of the `moon ci` verdict for them:
  - bash 4+ (`/opt/homebrew/bin/bash`): `ci/ruff/run.sh`, `ci/next-public/run.sh`, `ci/publish-metadata/run.sh`, `ops/nats/check-subjects.sh` plus its nextest half (`cd rs && cargo nextest run --locked --no-tests=pass -p paigasus-iam --test nats_permissions --test docker_preflight --profile iam-nats`).
  - bash 5 with a healthy pipe: `ci/actionlint/run.sh`; read its `pipe capacity` preflight line; on a 512-byte pipe, run it in a Linux container (memory `small-pipe-linux-container-workaround`).
  - `bash ci/cargo-lock-integrity/run.sh`.
  Expected: every target passes. On an unattributed failure, follow the root `CLAUDE.md` diagnosis procedure (copy the Moon cache report and the task state directory before any re-run).
- [ ] **Step 3: Confirm the commit rules.** `git log origin/main..HEAD --format=%B | grep -c 'SMA-75[1]'` → `0`; every subject ends with `(SMA-775)`; every body is only the `Co-Authored-By` paragraph.

No commit in this task unless Step 2 needs a fix; a fix gets its own `fix(<scope>): … (SMA-775)` commit.

---

## Self-review notes

- **Spec coverage.** §3.2 PR 2 scope: Tasks 1-12. §3.4 floor in every mode: Tasks 8, 10, 11 (Review Focus 1). §3.6 / §9.4 service identity: Tasks 2, 9, 11, 12 (Q1); the ADR amendment is the "Before merge" line. §6.3 starter saved: Task 7. §7 new codes: Tasks 1, 7 (R10) and Tasks 1, 5 (R19, Sven's Q2 answer). §9.1 PR 2 RPCs and HTTP routes: Tasks 1, 8, 9. §9.2: Tasks 4, 7. §9.3: Task 10 (R1, R2, R3) and Task 11. §9.5 PR 2 actions: Task 2. §9.6 audit: Task 7 (R13); PR 1 writes keep theirs. §9.7 events, outbox, NATS test and `permissions.md`: Tasks 3, 6, 7, 9 (R4, R14). §10 PR 2 tests: fetch by version, with no set, with a held version (Tasks 8, 9); starter on first enable and mode rules (Task 7); identity and grant (Tasks 2, 9); `off` and the floor (Tasks 8, 11); outbox rows (Tasks 6, 9); NATS permission test (Task 3). Changelog "Tenant documentation (PR 2)" and the PR 1 deferred `has` item: Task 12. PR 1 D4 normaliser: Task 3 (R17). Out: §8 and the held-version RPCs (PR 3).
- **Type consistency.** `TenantSettings { mode, on_unavailable }` (gateway, Tasks 10-11) is separate from IAM's `ToolPolicySettings { mode, fail_mode, … }` (Tasks 3-9); the adapter maps one to the other. `VersionOrigin::Starter` and the port methods `settings` / `upsert_settings_in` are added in Task 4, with their Pg mapping, and used from Task 7.
- **Proportion.** Signatures, test names, pinned values and the rules the spec fixes; no bodies except the proto messages, which are the contract.
