# IAM tool policy sets, PR 1 (SMA-774) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give IAM a per-organization tool catalog, a tenant tool policy set and one platform floor set, compiled on save by a separate Dogwood-based compiler binary, with the PR 1 RPCs, HTTP routes, authz actions, error codes, audit entries and the `iam.tool.policies` capability.

**Architecture:** A separate cargo workspace `rs/tools/paigasus-policy-compiler/` links Dogwood (and `cedar-policy` 4.11) and ships a CLI. IAM starts it once per compile (JSON on stdin and stdout, hardened child process). IAM owns the catalog, the Cedar schema generation, the version rows, the 4.14 re-validation, the coupling between catalog and set, and the transports. Pure logic lives in `paigasus-iam-core::tool_policy`. Process I/O, Postgres and transports live in `paigasus-iam`.

**Tech Stack:** Rust 2024 / 1.95, Dogwood `dogwood-language` 1.1.1 (git rev pin), `cedar-policy` 4.11.0 (compiler) and 4.14.x (IAM), SeaORM migrations, tonic + prost via buf, axum, tokio `process`, `libc` rlimits, Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-11-15-iam-tool-policy-sets-design.md` (PR 1 of the §3.2 split). The Cedar names come from `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-752-tool-decisions/docs/superpowers/specs/2026-11-15-sma-752-tool-invoke-decisions-design.md` §6.1.

## Global Constraints

- Dogwood: `git = "https://github.com/dogwood-policy/dogwood"`, `rev = "ff158506cfd9c7871385ed3a87d7435505fd24b1"` (version 1.1.1). No Dogwood bump and no compiler output change in this PR.
- The compiler workspace pins `cedar-policy = "=4.11.0"`. Dogwood itself asks only `"4.11"` (caret), so the exact pin is ours and holds the compiler lock on 4.11.0.
- Dogwood, `rhai` and `cedar-policy` 4.11 never enter the `rs/` workspace or the IAM process.
- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for shell, Python, TOML, YAML).
- Rust edition 2024, rust-version 1.95. The compiler `Cargo.toml` restates both and the lints (`warnings = "deny"`, clippy `all = "warn"`).
- `warnings = "deny"`: no task adds an item that the same task does not use or test. PR 2 and PR 3 enum values exist only in proto enums and SQL CHECK lists, never as unused Rust variants.
- `rs/Cargo.toml` `members` stay literal paths (SMA-663). The compiler goes in `exclude`, never in `members`.
- Every cargo invocation in a Moon task passes `--locked`.
- A `.proto` edit is followed by `buf format -w` and `moon run contracts:generate --force` in the same task. Commit all generated Rust, Python and TS output with it.
- Commits: conventional, with a workspace scope (`feat(rs)`, `feat(contracts)`, `feat(ci)`, `test(rs)`), the subject ends with `(SMA-774)`, and the body ends with a blank line and `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Each "Commit" step: `git add` exactly the files in that task's **Files** block (plus generated output named there, and every lockfile that the task's cargo runs changed), then `git commit -m "<subject shown>" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"`. If the hook reports `commitlint not found`, provision the worktree (root `CLAUDE.md`), never bypass it.
- Never write the text `SMA-751` in a commit message, a PR title or a PR body. Refer to the parent as "IAM tool catalog, tool policy sets and policy sync".
- No `--no-verify`, no `git commit --amend`, no `git reset`, no bare `git stash`. Do not hand-edit `.github/CODEOWNERS`. Do not touch or copy the `ci-targets` or `moon-diagnosis` marker blocks in `CLAUDE.md`.
- Do not install host software. Use `docker run ubuntu:24.04` for a Linux-only check.
- Put `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` in front of every `moon`, `buf`, `uv` or `cargo nextest` command.
- Error codes: the IAM block (1-299) of `contracts/proto/paigasus/common/v1/error.proto`, numbers 47 to 60, in the order of spec §7.
- Capability `CAPABILITY_IAM_TOOL_POLICIES = 7`, key `iam.tool.policies`. Toggle `[tool_policies] enabled`, default `false`.
- Limits (defaults in `iam.toml`): 500 tools per org, 64 KiB per input schema, JSON Schema depth 8, 256 KiB source, 3 MiB result, 10 s timeout, `RLIMIT_AS` 1 GiB, `RLIMIT_CPU` 15 s, 2 compiler permits, `max_window` 24 h with a hard cap of 7 days.
- Tool name part grammar `^[A-Za-z0-9_-]{1,64}$`; qualified name `<namespace>.<name>` or `<name>`; `name_key` = ASCII lowercase of the qualified name; `invoketool` and `unregistered` are reserved.
- `@id` grammar `^[a-z0-9][a-z0-9_-]{0,63}$`; floor ids start with `floor-`, tenant ids do not. `@scope` is `session` (default), `principal` or `key`.
- Starter source, verbatim:
  ```cedar
  @id("starter-allow-all")
  permit (principal, action in Pgs::Gateway::Action::"InvokeTool", resource);
  ```

## Review Focus

1. **An empty or comment-only source.** Dogwood's `lower_many` returns `Error::NoPolicies` on zero entries, but the spec says an empty set is valid. Expect an accepted empty set (version written, no conditions). Test: Task 2 `empty_source_compiles_to_an_empty_set`, Task 12 `an_empty_tenant_source_is_saved`.
2. **A tool name that differs only in case from an existing entry** (`Search` vs `search`) in one import batch, or in a `PutTool` against the catalog. Expect `tool-catalog-conflict`, never a silent overwrite. Test: Task 3 `import_batch_with_two_case_variants_conflicts`, Task 12 `put_tool_with_a_case_variant_of_an_existing_entry_conflicts`.
3. **A policy that names a catalog tool, then the tool is deleted.** Expect the delete to be rejected with `tool-catalog-breaks-policy` naming the policy id, and the catalog unchanged. Test: Task 12 `deleting_a_tool_that_a_live_policy_names_is_rejected`.
4. **A compiler that writes a valid result for a different generation or protocol version** (a stale binary after a partial deploy). Expect `tool-policy-compiler-unavailable` and nothing stored. Test: Task 11 `a_result_with_a_foreign_generation_is_unavailable`.
5. **A non-ASCII or mixed-case action literal in policy source** (`Action::"Send_Email"`, `Action::"ｓearch"`). Expect `tool-policy-invalid` with the policy id, because it can never match a `name_key`. Test: Task 2 `a_non_normal_action_literal_is_rejected`.

---

## Decisions this plan makes (read before Task 1)

- **D1. Lowering uses `lower_many`, not one `lower_with_distincter` call.** The Dogwood source at the pinned commit (`dogwood-language/src/cedarify/mod.rs:183-197`) names rule `k` of a call `<distincter>_<k>`, where `k` is the position in that call. The key is "deliberately not derived from a source `@id`". So one whole-set call renumbers every later slot when an earlier policy is deleted. Spec §2 and §5.3 step 4 assume the opposite. The plan lowers the whole set in ONE `dogwood_language::lower_many` call, with one entry per policy and a distincter derived from its `@id` (`policy_set.rs:95-120`: "Every entry is lowered independently … and keeps the ids that call mints"). This is still "lower the whole set once" (§3.8): one call, one merged `LoweredPolicySet`, one augmented schema. Task 1 proves both facts. **The coordinator must confirm D1 with Sven before Task 2 starts.**
- **D2. The event schema is `UNPINNED_EVENT_SCHEMA`.** Dogwood's default schema pins `callerPrincipal` on every event (`configuration/event-schemas/pinned.dwschema`), which partitions history per principal. That conflicts with `@scope` `session` and `key`. With the unpinned schema, Dogwood adds no pin, `temporal_fields()` equals `nonrelativized_temporal_fields()`, and the gateway partitions by `HistoryCondition.scope` (§3.8).
- **D3. No `rhai` path is reachable.** `rhai` is a mandatory dependency of `dogwood-language` (`Cargo.toml:66`). It runs only provider scripts, in `extension/provider/eval.rs`, called only from the decision path (`api.rs:1443`, `1524`, `1552`). The compiler builds its `ServiceSchema` with no provider declarations and never builds an `Authorizer` outside tests. It also rejects any provider invocation in source (`ParsedPolicy::provider_invocations()` not empty → `tool-policy-invalid`), so tenant text cannot name a provider. No operation limits are needed.
- **D4. The kernel normaliser is not on `main`, and PR 1 does not need it.** SMA-752's `normalise` (trim, NFC, lowercase, NFC) is on its unmerged branch. On the §4.1 ASCII grammar, trim and NFC are the identity and `to_lowercase` equals `to_ascii_lowercase`. So `name_key` is `to_ascii_lowercase` behind the grammar check, with a doc comment that states this equivalence. A copy of NFC would be dead code. PR 2 replaces the body with the kernel call once SMA-752 merges, and keeps the tests.
- **D5. The principal type is `Pgs::Iam::Principal`.** Spec §4.3 gives `Pgs::Iam::User` and `Pgs::Iam::ServiceAccount` as examples, but IAM principal PRNs have the resource type `principal` (`application/roles.rs::parse_principal_prn`), and `paigasus_kernel::to_cedar_uid` maps that to `Pgs::Iam::Principal`. The base schema declares `Pgs::Iam::Principal`. Task 4 asks the SMA-752 author to confirm (spec §4.3 "the PR 1 plan reviews the base schema with the SMA-752 author").
- **D6. The compiler takes the schema as Cedar JSON and converts it.** Dogwood's `PolicySchema` accepts only cedarschema text (`policy_schema.rs:38`). The compiler converts with `cedar_policy::SchemaFragment::from_json_value(..)?.to_cedarschema()` (cedar 4.11), so the protocol stays as §5.2 says.
- **D7. Dogwood validation runs after lowering.** `Validator::validate(&LoweredPolicySet)` (`validator.rs:59`) takes a lowered set. So the order is parse → `@id`/`@scope` → lower → Dogwood validate → translate → cedar 4.11 validate → floor effect → window. The reason codes do not change.
- **D8. The capability gates all eight RPCs.** `iam.tool.policies` is advertised only when `tool_policies.enabled = true` AND the boot `revision` call succeeded. Without it, gRPC answers `capability-disabled` and the HTTP routes are not mounted (404), as `authz.admin_enabled` does. The spec names only the writes for a missing binary; reads are also off, because without the compiler there is nothing to administer.
- **D9. PR 1 writes no outbox event.** Spec §4.5 step 4 writes an outbox event, but the events are PR 2 (§9.7). PR 1 writes the version row, the state row and the audit entry in one transaction. PR 2 adds the `outbox.enqueue` to the same transaction.
- **D10. PR 2 and PR 3 stubs.** The SQL CHECK lists hold every `status` (`live`, `superseded`, `held`, `discarded`) and `origin` value, and the `held` partial index exists, so PR 3 needs no migration for them. The Rust enums hold only `Live`/`Superseded` and `Save`/`CatalogChange`. `base_version` and `report` are nullable and unwritten. `tool-policy-recompile-pending` is live in PR 1; until PR 3, the admin clears it by a save.

## File map

Compiler workspace (new, `rs/tools/paigasus-policy-compiler/`):
- `Cargo.toml`, `Cargo.lock`, `deny.toml`, `moon.yml`
- `src/main.rs` — the `compile` and `revision` subcommands
- `src/lib.rs` — `COMPILER_GENERATION`, `PROTOCOL_VERSION`, `DOGWOOD_COMMIT`, module list
- `src/lower.rs` — split, annotations, distincters, `lower_many`
- `src/translate.rs` — temporal AST → history conditions, the shape and predicate subset
- `src/compile.rs` — the compile pipeline
- `src/protocol.rs` — the compiler's JSON types
- `protocol/fixtures/*.json` — shared request/result fixtures; `protocol/fixtures/base_schema.json`
- `tests/scope_proof.rs`, `tests/shapes.rs`, `tests/protocol_fixtures.rs`, `tests/golden.rs`, `tests/golden/*`, `tests/corpus.rs`, `corpus/*.json`, `tests/fixtures/test_schema.cedarschema`

IAM core (`rs/crates/libs/paigasus-iam-core/src/tool_policy/`, new module):
- `mod.rs`, `name.rs`, `json_schema.rs`, `import.rs`, `schema.rs`, `starter.rs`, `protocol.rs`, `compare.rs`, `check.rs`, `model.rs`, `ports.rs`
- `tests/tool_policy_cross_version.rs`

IAM service (`rs/crates/services/paigasus-iam/`):
- `src/adapters/persistence/migration/m0013_create_tool_policies.rs`, entities `tool_catalog_entry.rs`, `tool_policy_state.rs`, `tool_policy_set_version.rs`, `src/adapters/persistence/pg_tool_policies.rs`
- `src/adapters/policy_compiler/{mod.rs, process.rs}`
- `src/application/tool_policies/{mod.rs, catalog.rs, sets.rs, rejection.rs}`
- `src/adapters/grpc/tool_policies.rs`, `src/adapters/http/tool_policies.rs`
- tests `tests/tool_policies_pg.rs`, `tests/tool_policies_compiler_process.rs`, `tests/grpc_tool_policies.rs`, `tests/http_tool_policies.rs`, `tests/tool_policies_real_compiler.rs`

Modified: `rs/Cargo.toml`, `rs/Dockerfile`, `rs/.dockerignore`, `.moon/workspace.yml`, root `moon.yml`, `rs/crates/services/paigasus-iam/moon.yml`, `rs/crates/libs/paigasus-iam-core/moon.yml`, `.github/workflows/ci.yml`, `.github/workflows/images.yml`, `.github/dependabot.yml`, `ci/cargo-lock-integrity/run.sh` (+ `README.md`), `ci/affected-graph/run.sh` (if needed), `ci/error-registry/check.py`, contracts protos, `rs/crates/libs/paigasus-proto/src/{error.rs,capability.rs}`, `ts/packages/paigasus-discovery/{src/types.ts,tests/vocabulary.test.ts}`, `ts/packages/paigasus-proto/src/capability.test.ts`, iam-core `authz/{action.rs,schema.rs,roles.rs}`, `lib.rs`, `Cargo.toml`, the service's `config.rs`, `iam.toml.example`, `service_info.rs`, `application/{error.rs,mod.rs,fakes.rs}`, `adapters/{retryable.rs,http/error.rs,http/mod.rs,grpc/mod.rs,grpc/convert.rs,persistence/mod.rs,persistence/migration/mod.rs,persistence/entities/mod.rs}`, `main.rs`, `Cargo.toml`, `tests/support/mod.rs`.

---

### Task 1: Compiler workspace and the scope-lowering proof

**Files:**
- Create: `rs/tools/paigasus-policy-compiler/Cargo.toml`, `Cargo.lock`, `src/lib.rs`, `src/lower.rs`, `tests/scope_proof.rs`, `tests/fixtures/test_schema.cedarschema`
- Modify: `rs/Cargo.toml` (`[workspace]`: add `exclude = ["tools/paigasus-policy-compiler"]`, with a comment that cites this spec §5.1)

**Interfaces:**
- Produces (in `src/lower.rs`):
  - `pub struct PolicyUnit { pub index: usize, pub id: Option<String>, pub scope: Option<String>, pub source: String, pub annotations: Vec<(String, Option<String>)> }`
  - `pub fn service_schema() -> dogwood_language::ServiceSchema` — `ServiceSchema::builder().event_schema_str(dogwood_language::UNPINNED_EVENT_SCHEMA).build()`, no providers (D2, D3)
  - `pub fn split_policies(source: &str, svc: &ServiceSchema) -> Result<Vec<PolicyUnit>, dogwood_language::Error>` — one `ParsedPolicySet::parse` of the whole source, then one unit per `ParsedPolicy::expanded_source()`
  - `pub fn read_annotations(expanded: &str) -> Vec<(String, Option<String>)>` — reads the leading `@key` / `@key("value")` lines of Dogwood's rendering (`dogwood-language/src/render.rs:43`)
  - `pub fn distincter_for(id: &str) -> dogwood_language::Distincter` — `"p_"` + `id` with `-` → `_h` and `_` → `_u` (injective, and matches Dogwood's `[_A-Za-z][_A-Za-z0-9]*`)
  - `pub fn lower_units(units: &[PolicyUnit], svc: &ServiceSchema, schema: &PolicySchema) -> Result<LoweredPolicySet, dogwood_language::Error>` — re-parses each unit's source as its own set, then ONE `lower_many` call
  - `pub fn slot_prefix(id: &str) -> String` — `format!("{}_0", distincter_for(id).as_str())`; every slot of that policy is `<prefix>__temporal_<j>`

`Cargo.toml` content to fix: `[workspace]` with `resolver = "3"`; `[package] name = "paigasus-policy-compiler"`, `version = "0.0.0"`, `edition = "2024"`, `rust-version = "1.95"`, `license = "Apache-2.0"`, `publish = false`; `[dependencies] dogwood-language = { git = "https://github.com/dogwood-policy/dogwood", rev = "ff158506cfd9c7871385ed3a87d7435505fd24b1" }`, `cedar-policy = "=4.11.0"`; `[lints.rust] warnings = "deny"`, `[lints.clippy] all = "warn"`. Each dependency line gets a comment with its reason, in the style of `rs/crates/services/paigasus-iam/Cargo.toml`. `cedar-policy` is used in this task by the test that reads annotations from `as_cedar()`.

`tests/fixtures/test_schema.cedarschema`: a minimal schema with `namespace Pgs::Iam { entity Principal; entity Organization; }` and `namespace Pgs::Gateway { action "InvokeTool"; action "search" in ["InvokeTool"] appliesTo { principal: [Pgs::Iam::Principal], resource: [Pgs::Iam::Organization], context: { input: { q: String, n: Long } } }; action "send" in ["InvokeTool"] appliesTo {…same, input: { to: String, amount: Long } }; }`. Adjust the syntax until Dogwood accepts it; this file is test-only.

- [ ] **Step 1: Create the workspace and resolve the lock**

Run (from `rs/tools/paigasus-policy-compiler`): `cargo generate-lockfile && cargo tree --locked -i cedar-policy`
Expected: the lock resolves; `cargo tree` shows exactly `cedar-policy v4.11.0`, and `dogwood-language v1.1.1 (https://github.com/dogwood-policy/dogwood?rev=ff158506cfd9c7871385ed3a87d7435505fd24b1#ff158506)`. If cargo picks a `cedar-policy-core` other than 4.11.0, add `cedar-policy-core = "=4.11.0"` with a reason comment and repeat. Also run `cargo tree --locked -i rhai` and keep the output for Task 2.

- [ ] **Step 2: Write the failing proof tests in `tests/scope_proof.rs`**

Build three sources over `test_schema.cedarschema`. Policy A (`@id("a-count")`, `@scope("session")`) has a `count_within(1h, Pgs::Gateway::Action::"search"::request{ input.q: "x" }) > 2` block via `bind`, and a `formerly within 10m …::"send"::response{}` block. Policy B (`@id("b-formerly")`) has one temporal block. Policy C (`@id("c-plain")`) has none.

```rust
#[test]
fn lowering_a_policy_does_not_depend_on_the_scope_of_other_policies() {
    // S1 = A + B(@scope "principal") + C, S2 = A + B(@scope "key") + C, S3 = A alone.
    // For each set: lower_units over split_policies, in ONE lower_many call.
    // Assert, for policy A, across S1, S2, S3:
    //   - the Cedar text of policy id slot_prefix("a-count") (as_cedar().policy(..).to_string()) is byte-identical;
    //   - the ids of its temporal fields (prefix slot_prefix("a-count")) are identical;
    //   - format!("{:?}", field.condition) of each of its fields is identical.
}

#[test]
fn slot_names_survive_deleting_an_earlier_policy() {
    // Lower [B, A] and [A]. A's slot ids are identical in both and start with slot_prefix("a-count").
}

#[test]
fn one_positional_lowering_renumbers_after_a_delete() {
    // Documents D1: ParsedPolicySet::parse(B+A).lower_with_distincter(schema, "p") gives A the rule key "p_1";
    // parse(A).lower_with_distincter(.., "p") gives "p_0". Assert they differ.
}

#[test]
fn annotations_read_from_the_rendering_match_the_lowered_cedar() {
    // For every unit of S1: read_annotations(unit.source) == the annotations of its lowered Cedar policy
    // (cedar_policy::Policy::annotations()), with Cedar's "no value" = Some("") mapped to None.
}

#[test]
fn distincters_are_injective_and_valid() {
    assert_eq!(distincter_for("floor-no-shell").as_str(), "p_floor_hno_hshell");
    assert_ne!(distincter_for("a-b").as_str(), distincter_for("a_b").as_str());
    assert_eq!(distincter_for("0abc").as_str(), "p_0abc");
}
```

- [ ] **Step 3: Run the tests to make sure they fail**

Run: `cargo test --locked --test scope_proof`
Expected: FAIL to compile (`lower_units` and the others are not defined).

- [ ] **Step 4: Implement `src/lower.rs` and `src/lib.rs`** with the signatures above. `lib.rs` declares `pub mod lower;` only.

- [ ] **Step 5: Run the tests**

Run: `cargo test --locked --test scope_proof`
Expected: all five PASS.

**STOP RULE.** If `lowering_a_policy_does_not_depend_on_the_scope_of_other_policies` or `slot_names_survive_deleting_an_earlier_policy` fails, STOP ALL WORK. Do not change the test, do not try another lowering strategy. Report the failing assertion and the diff of the two outputs to the coordinator, who asks Sven (spec §3.8). If `one_positional_lowering_renumbers_after_a_delete` fails (Dogwood does NOT renumber), also stop and report: then D1 is not needed and the coordinator decides.

- [ ] **Step 6: Commit**

```bash
git add rs/Cargo.toml rs/tools/paigasus-policy-compiler
git commit -m "test(rs): add the policy compiler workspace and the scope-lowering proof (SMA-774)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: Compiler translation and the source rules

**Files:**
- Create: `rs/tools/paigasus-policy-compiler/src/translate.rs`, `src/compile.rs`, `tests/shapes.rs`
- Modify: `src/lib.rs` (`pub mod translate; pub mod compile;`), `Cargo.toml` (`serde_json`), `Cargo.lock`

**Interfaces:**
- Consumes: Task 1 `split_policies`, `lower_units`, `slot_prefix`, `service_schema`.
- Produces (`src/translate.rs`; plain Rust types, serde comes in Task 5):
  - `pub enum Scope { Session, Principal, Key }`
  - `pub enum CmpOp { Lt, Le, Eq, Ge, Gt, Ne }`
  - `pub enum Literal { Long(i64), String(String), Bool(bool), Decimal(String) }`
  - `pub enum EventKind { Request, Response }`
  - `pub enum Predicate { EventKind(EventKind), Tool { name_key: String }, AnyCatalogTool, Field { path: Vec<String>, op: CmpOp, value: Literal }, AllOf(Box<Predicate>, Box<Predicate>), AnyOf(Box<Predicate>, Box<Predicate>), Negation(Box<Predicate>) }`
  - `pub enum Shape { Formerly { predicate: Predicate }, CountWithin { predicate: Predicate, op: CmpOp, threshold: i64 }, SumWithin { predicate: Predicate, field_path: Vec<String>, op: CmpOp, threshold: i64 } }`
  - `pub struct HistoryCondition { pub slot: String, pub policy_id: String, pub scope: Scope, pub window_ms: u64, pub shape: Shape }`
  - `pub fn translate_field(field: &dogwood_language::TemporalField, policy_id: &str, scope: Scope) -> Result<HistoryCondition, ShapeError>`
  - `pub struct ShapeError { pub shape: &'static str, pub detail: String }`
- Produces (`src/compile.rs`):
  - `pub enum SetKind { Tenant, Floor }`
  - `pub enum ReasonCode { ToolPolicyInvalid, ToolPolicyShapeUnsupported, ToolPolicyLoweringFailed, ToolPolicyFloorPermit, ToolPolicyIdInvalid, ToolPolicyWindowTooLong }` with `pub fn as_wire(&self) -> &'static str` (the six kebab codes of spec §7)
  - `pub struct Rejection { pub policy_id: Option<String>, pub line: Option<u32>, pub column: Option<u32>, pub reason: ReasonCode, pub message: String }`
  - `pub struct Compiled { pub cedar_policies: String, pub cedar_schema_json: serde_json::Value, pub conditions: Vec<HistoryCondition> }` (conditions sorted by `slot`)
  - `pub fn compile_cedarschema(kind: SetKind, source: &str, cedarschema: &str, max_window_ms: u64) -> Result<Compiled, Vec<Rejection>>`

Add `serde_json = "1"` to `[dependencies]` (used by `Compiled`).

**The Dogwood node inventory (spec §5.3, §11).** Put this list in the module doc of `translate.rs`, with the file:line of each node in the pinned Dogwood source (`dogwood-language/src/extension/temporal/ast.rs`). Translate from `LoweredPolicySet::nonrelativized_temporal_fields()`; under D2 it equals `temporal_fields()` (assert this in compile, and reject `tool-policy-lowering-failed` if not).

| Accepted node | Maps to |
|---|---|
| `ConditionKind::Formerly { within: WithinSpec::Concrete(i), body }` | `Shape::Formerly`, `window_ms = i.checked_seconds() * 1000` |
| `Comparison { op, left: Term::Agg(Count { for_vars: [t: Timepoint], body: Formerly{W, And(P, Tp(t))} }), right: Term::Integer(c) }`, and the `bind` form `Exists { n: Long, And(Comparison{Eq, Agg(Count..), Var(n)}, Comparison{op, Var(n), Integer(c)}) }` | `Shape::CountWithin` |
| the same two forms with `AggExprKind::Sum { bound_var: a, for_vars: [a: Long, t: Timepoint], body: Formerly{W, And(P, Tp(t))} }`, where `P` holds exactly one `NamedArg { name: "input.<path>", value: Var(a) }` | `Shape::SumWithin { field_path }` |
| `Predicate { namespace: ["Pgs","Gateway","Action"], action, kind, args }` | `AllOf(EventKind, Tool / AnyCatalogTool, Field..)` |
| `Predicate.kind` = `request` / `response` | `EventKind`; `error` → shape-unsupported |
| `action` = `InvokeTool` | `AnyCatalogTool`; a normal-form name → `Tool { name_key }` |
| `NamedArg { name: "input.<path>", value: Term::{Integer, String, Bool, Decimal} }` | `Field { op: Eq }` |
| `Exists { v: Long, And(Predicate{.. input.<p>: Var(v)}, Comparison{op, Var(v), Integer(c)}) }` inside a predicate position | `Field { op }` |
| `And`, `Or`, `Not` | `AllOf`, `AnyOf`, `Negation` |

Every other node or term is rejected with `tool-policy-shape-unsupported`, naming the node (for example `Previous`, `Since`, `Term::ContextField`, `Term::Wildcard`, `count for (k: String)` = `count_distinct_within`, `WithinSpec::ParamRef`, `Term::Arith`). A block whose root combines two shapes (for example `And(Formerly, CountWithin)`) is rejected as `two-shapes`.

**The `rhai` check (spec §5.3, §11).** Record D3 with the `cargo tree --locked -i rhai` output from Task 1 Step 1 in the module doc of `compile.rs`.

**Compile order (D7):** source over 256 KiB is not checked here (IAM checks size); `split_policies` (parse error → `tool-policy-invalid` with location); for each unit: `@id` present, grammar, unique, floor prefix rule → else `tool-policy-id-invalid`; `@scope` value → else `tool-policy-id-invalid`; `provider_invocations()` not empty → `tool-policy-invalid`; BEFORE any lowering, from the Dogwood parse tree of each unit, the action literals in its Cedar scope and in each temporal `Predicate.action` must be `InvokeTool`, `Unregistered`, or equal to their own ASCII lowercase and match `^[a-z0-9_-]{1,64}(\.[a-z0-9_-]{1,64})?$` and not be `invoketool`/`unregistered` → else `tool-policy-invalid` with the policy id (a non-normal literal is never in a generated schema, so this check must run before Dogwood can fail on it as a lowering or validation error; if the parse tree does not expose these literals, stop and report to the coordinator); zero units → return the empty `Compiled` (no Dogwood lowering; `cedar_policies` = `""`, `cedar_schema_json` = the input schema converted with cedar 4.11 `SchemaFragment::from_cedarschema_str(..)?.to_json_value()`, `conditions` = `[]`); `lower_units` error → `tool-policy-lowering-failed`; `Validator::new().validate(&lowered)` failures → `tool-policy-invalid`, mapped to the policy whose slot prefix or policy id matches; translate each field → shape errors; `cedar_policy::Validator` (4.11, strict) over `as_cedar()` against `cedar_schema()` → `tool-policy-invalid`; floor: every policy effect is `Forbid` → else `tool-policy-floor-permit`; each `window_ms <= max_window_ms` → else `tool-policy-window-too-long`. Collect every rejection of a unit; continue with the next unit where the step allows it.

- [ ] **Step 1: Write the failing tests in `tests/shapes.rs`** (each calls `compile_cedarschema(…, test_schema, 86_400_000)`):

```rust
#[test] fn formerly_within_is_one_formerly_condition() { /* window_ms == 600_000; predicate == AllOf(EventKind(Response), Tool{name_key:"send"}) */ }
#[test] fn count_within_with_each_comparison_is_accepted() { /* for op in <,<=,==,>=,>,!=: Shape::CountWithin{op, threshold: 2} */ }
#[test] fn sum_within_of_a_long_field_is_accepted() { /* field_path == ["amount"], threshold == 100 */ }
#[test] fn a_field_comparison_inside_a_predicate_is_accepted() { /* Field{path:["n"], op: Gt, value: Long(5)} */ }
#[test] fn previous_and_since_are_shape_unsupported() { /* reason ToolPolicyShapeUnsupported, message names the node */ }
#[test] fn count_distinct_within_is_shape_unsupported() {}
#[test] fn a_two_shape_block_is_rejected() { /* message contains "two-shapes" */ }
#[test] fn an_error_event_pattern_is_rejected() { /* ::error{…} → ToolPolicyShapeUnsupported */ }
#[test] fn a_context_reference_in_a_predicate_is_rejected() { /* input.q: context.input.q */ }
#[test] fn a_missing_or_duplicate_or_malformed_id_is_rejected() { /* three cases → ToolPolicyIdInvalid */ }
#[test] fn the_floor_prefix_rule_holds_in_both_directions() { /* tenant "floor-x" and floor "x" → ToolPolicyIdInvalid */ }
#[test] fn an_unknown_scope_is_rejected() { /* @scope("global") → ToolPolicyIdInvalid */ }
#[test] fn scope_defaults_to_session() {}
#[test] fn a_floor_permit_is_rejected_in_every_form() { /* permit(...); permit(...) when {...}; permit with a temporal block → ToolPolicyFloorPermit */ }
#[test] fn a_window_over_the_limit_is_rejected() { /* max_window_ms = 3_600_000, formerly within 2h → ToolPolicyWindowTooLong */ }
#[test] fn a_non_normal_action_literal_is_rejected() { /* Action::"Send"; Action::"invoketool"; a non-ASCII literal; in a scope and in a temporal predicate → ToolPolicyInvalid with policy_id set, never ToolPolicyLoweringFailed */ }
#[test] fn a_provider_invocation_is_rejected() { /* Ns::Fn(...) in a when clause → ToolPolicyInvalid (D3) */ }
#[test] fn a_hyphenated_id_with_a_temporal_block_compiles() { /* @id("rate-limit-search"); slot starts with "p_rate_hlimit_hsearch_0__temporal_" */ }
#[test] fn slot_names_stay_when_an_earlier_policy_is_deleted() { /* compile [B,A] and [A]; A's conditions identical */ }
#[test] fn conditions_are_sorted_by_slot() {}
#[test] fn empty_source_compiles_to_an_empty_set() { /* "" and "// only a comment" → Ok, conditions empty, cedar_policies empty */ }
```

- [ ] **Step 2: Run to verify failure.** Run: `cargo test --locked --test shapes`. Expected: FAIL to compile.

- [ ] **Step 3: Implement `translate.rs` and `compile.rs`** per the table and the order above.

- [ ] **Step 4: Run.** Run: `cargo test --locked`. Expected: every test in `scope_proof` and `shapes` PASS; `cargo clippy --locked --all-targets -- -D warnings` clean.

- [ ] **Step 5: Commit** `feat(rs): translate Dogwood temporal blocks into history conditions (SMA-774)` with the files above and the trailer.

---

### Task 3: IAM core — tool names, JSON Schema mapping, import parsers

**Files:**
- Create: `rs/crates/libs/paigasus-iam-core/src/tool_policy/{mod.rs, name.rs, json_schema.rs, import.rs, model.rs}`
- Modify: `rs/crates/libs/paigasus-iam-core/src/lib.rs` (`pub mod tool_policy;`)

**Interfaces:**
- Produces (`name.rs`):
  - `pub const RESERVED_NAME_KEYS: &[&str] = &["invoketool", "unregistered"];`
  - `pub struct ToolName { pub display: String, pub name_key: String }`
  - `pub fn parse_tool_name(namespace: Option<&str>, name: &str) -> Result<ToolName, NameError>` and `pub fn parse_qualified(qualified: &str) -> Result<ToolName, NameError>`
  - `pub enum NameError { Grammar { part: String }, Reserved }` (messages never echo more than the offending part)
  - `pub fn name_key(qualified: &str) -> String` — `to_ascii_lowercase`; doc comment states D4
- Produces (`model.rs`): `pub enum ToolKind { Function, Custom }`, `pub enum ToolSource { Manual, OpenAi, Mcp }`, `pub struct DroppedField { pub path: String, pub reason: String }`, `pub struct ToolEntry { pub name: ToolName, pub kind: ToolKind, pub input_schema: Option<serde_json::Value>, pub cedar_input_type: serde_json::Value, pub dropped_fields: Vec<DroppedField>, pub source: ToolSource }`, `pub struct CatalogLimits { pub max_tools: usize, pub max_input_schema_bytes: usize, pub max_depth: usize }` with `Default` = 500 / 65536 / 8.
- Produces (`json_schema.rs`): `pub fn map_input_schema(schema: &serde_json::Value, max_depth: usize) -> Result<(serde_json::Value /* Cedar JSON record type */, Vec<DroppedField>), MappingError>` and `pub enum MappingError { TooDeep, NotAnObject }`.
- Produces (`import.rs`): `pub enum ImportFormat { OpenAiChatCompletions, OpenAiResponses, McpToolsList }`, `pub struct ImportedTools { pub tools: Vec<ToolEntry>, pub skipped: Vec<(usize, String /* type */)> }`, `pub fn parse_import(format: ImportFormat, document: &serde_json::Value, namespace: Option<&str>, limits: &CatalogLimits) -> Result<ImportedTools, Vec<CatalogProblem>>`, `pub fn build_manual(qualified: &str, kind: ToolKind, input_schema: Option<serde_json::Value>, limits: &CatalogLimits) -> Result<ToolEntry, Vec<CatalogProblem>>`, `pub enum CatalogProblem { Invalid { name: Option<String>, message: String }, Conflict { name_key: String } }`.

The mapping follows the spec §4.3 table exactly. A property path is dotted (`a.b`). A `custom` tool and a built-in tool get an empty record. Duplicate `name_key` in one batch → `CatalogProblem::Conflict`. A schema over 64 KiB (serialised byte length) or over depth 8 → `CatalogProblem::Invalid`. More than `max_tools` entries in one batch → `Invalid`.

- [ ] **Step 1: Write the failing unit tests** (in each file's `#[cfg(test)] mod tests`):

```rust
// name.rs
#[test] fn a_namespaced_name_gets_a_lowercase_key() { assert_eq!(parse_tool_name(Some("GitHub"), "Search").unwrap().name_key, "github.search"); }
#[test] fn display_keeps_the_given_case() { assert_eq!(parse_qualified("GitHub.Search").unwrap().display, "GitHub.Search"); }
#[test] fn each_part_follows_the_openai_grammar() { for bad in ["", "a b", "é", &"x".repeat(65), "a/b"] { assert!(parse_tool_name(None, bad).is_err()); } assert!(parse_tool_name(None, &"x".repeat(64)).is_ok()); }
#[test] fn reserved_names_are_rejected_in_any_case() { for n in ["InvokeTool", "UNREGISTERED", "invoketool"] { assert_eq!(parse_qualified(n).unwrap_err(), NameError::Reserved); } }
#[test] fn a_qualified_name_with_two_dots_is_rejected() { assert!(parse_qualified("a.b.c").is_err()); }
// json_schema.rs
#[test] fn every_mapped_form_maps() { /* string(+format)->String, integer->Long, boolean->Bool, array of string->Set<String>, nested object->record, required vs optional */ }
#[test] fn nullable_forms_become_optional() { /* ["string","null"] and anyOf:[{type:string},{type:null}] → optional String */ }
#[test] fn every_unmapped_form_is_dropped_with_a_reason() { /* number, oneOf, allOf, other anyOf, $ref, additionalProperties without properties, null alone, ["string","integer"] → dropped_fields has the path and a reason; record omits it */ }
#[test] fn an_empty_nested_record_stays() {}
#[test] fn depth_over_eight_is_too_deep() {}
// import.rs
#[test] fn openai_chat_completions_form_imports() {}
#[test] fn openai_responses_form_imports_function_and_custom() { /* custom → ToolKind::Custom, empty record */ }
#[test] fn mcp_tools_list_imports_with_a_namespace() { /* {"tools":[{"name":"search","inputSchema":{…}}]}, ns "gh" → name_key "gh.search" */ }
#[test] fn other_types_are_skipped_and_listed() { /* web_search, file_search → skipped [(1,"web_search"),…] */ }
#[test] fn an_mcp_name_outside_the_grammar_is_rejected_with_the_reason() {}
#[test] fn import_batch_with_two_case_variants_conflicts() { /* "Search" + "search" → Conflict{name_key:"search"} */ }
#[test] fn an_input_schema_over_64_kib_is_invalid() {}
#[test] fn more_than_500_tools_is_invalid() {}
```

- [ ] **Step 2: Run to verify failure.** Run: `cd rs && cargo nextest run --locked -p paigasus-iam-core tool_policy`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** the four files. No new dependency: `serde_json` and `serde` are already deps of iam-core.
- [ ] **Step 4: Run.** Same command. Expected: PASS. Also `cargo clippy --locked -p paigasus-iam-core --all-targets -- -D warnings`.
- [ ] **Step 5: Commit** `feat(rs): add tool names, JSON Schema mapping and tool import parsing to iam-core (SMA-774)`.

---

### Task 4: IAM core — Cedar schema generation, schema revision, starter policy, base-schema fixture

**Files:**
- Create: `rs/crates/libs/paigasus-iam-core/src/tool_policy/{schema.rs, starter.rs}`, `rs/tools/paigasus-policy-compiler/protocol/fixtures/base_schema.json`
- Modify: `tool_policy/mod.rs`, `rs/crates/libs/paigasus-iam-core/moon.yml` (`test` inputs add `/rs/tools/paigasus-policy-compiler/protocol/fixtures/**/*`, because this task's test is the first reader)

**Interfaces:**
- Consumes: Task 3 `ToolEntry`, `ToolKind`.
- Produces (`schema.rs`):
  - `pub const TOOL_SCHEMA_REVISION: u32 = 1;`
  - `pub const ACTION_NAMESPACE: &str = "Pgs::Gateway::Action";`
  - `pub fn base_schema_json() -> serde_json::Value`
  - `pub fn tenant_schema_json(catalog: &[ToolEntry]) -> serde_json::Value` — base plus one action per entry, sorted by `name_key`
  - `pub fn catalog_names(catalog: &[ToolEntry]) -> Vec<String>` — sorted `name_key` list
- Produces (`starter.rs`): `pub const STARTER_TOOL_POLICY_SOURCE: &str` (the Global Constraints text, byte for byte) and `pub const STARTER_TOOL_POLICY_ID: &str = "starter-allow-all";`

**The base schema (Cedar JSON form).** Namespace `Pgs::Iam`: entity types `Principal` and `Organization`, no attributes, no parents (D5). Namespace `Pgs::Gateway`: common types `RequestInfo = { scope: String, ingress: String, model: String }`, `SessionInfo = { id: String, turn: Long }`, `CallInfo = { declared: String }`. Actions `InvokeTool` and `Unregistered`, each with `appliesTo { principalTypes: [Pgs::Iam::Principal], resourceTypes: [Pgs::Iam::Organization], context: { input: {} (empty record), input_text?: String, findings: Set<String>, call: CallInfo, request: RequestInfo, session: SessionInfo } }`. A group that applies to itself keeps `action in InvokeTool` valid with an empty catalog (spec §4.3). A `function` tool: `memberOf: [InvokeTool]`, `input` = its `cedar_input_type`, `input_text` optional. A `custom` tool: `input: {}`, `input_text: String` required.

- [ ] **Step 1: Write the failing tests** (in `schema.rs` and `starter.rs`):

```rust
#[test] fn the_base_schema_parses_under_cedar_4_14() { cedar_policy::Schema::from_json_value(base_schema_json()).unwrap(); }
#[test] fn the_starter_validates_against_an_empty_catalog() { /* cedar 4.14 Validator, strict, over STARTER_TOOL_POLICY_SOURCE and tenant_schema_json(&[]) → passed, no warnings */ }
#[test] fn a_generic_floor_validates_against_the_base_schema() { /* @id("floor-no-unregistered") forbid(principal, action in Pgs::Gateway::Action::"Unregistered", resource); and one on InvokeTool when context.findings.contains("name_collision") */ }
#[test] fn a_catalog_tool_is_a_member_of_invoke_tool() { /* permit(principal, action == Pgs::Gateway::Action::"github.search", resource) when { context.input.q == "x" }; validates */ }
#[test] fn an_unregistered_policy_cannot_read_input_fields() { /* action in Unregistered when { context.input.q == "x" } → validation error */ }
#[test] fn a_custom_tool_requires_input_text() {}
#[test] fn catalog_names_are_sorted() {}
#[test] fn the_starter_source_is_pinned() { assert_eq!(STARTER_TOOL_POLICY_SOURCE, "@id(\"starter-allow-all\")\npermit (principal, action in Pgs::Gateway::Action::\"InvokeTool\", resource);\n"); }
/// The pin test for TOOL_SCHEMA_REVISION (spec §6.2), in the style of
/// `starter_policy_content_is_pinned_to_the_declared_revision` in authz/roles.rs.
#[test] fn tool_schema_is_pinned_to_the_declared_revision() {
    // blake3 over canonical JSON of base_schema_json() + tenant_schema_json(FIXED_CATALOG) + map_input_schema over a fixed
    // JSON Schema corpus that covers every row of the §4.3 table. Compare with EXPECTED_TOOL_SCHEMA_HASH (cfg(test) const).
    // The failure message says: bump TOOL_SCHEMA_REVISION and replace the hash.
}
/// The drift control for the compiler side (Task 5 reads this file).
#[test] fn the_base_schema_fixture_is_current() {
    // Compare base_schema_json() with ../../../tools/paigasus-policy-compiler/protocol/fixtures/base_schema.json
    // (path from CARGO_MANIFEST_DIR). With PAIGASUS_WRITE_FIXTURES=1, write the file instead and pass.
}
```

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-iam-core tool_policy`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** `schema.rs` and `starter.rs`. Write the fixture once with `PAIGASUS_WRITE_FIXTURES=1`, then run again without it.
- [ ] **Step 4: Send the base schema for review.** Send the coordinator the generated `base_schema.json` and D5 (`Pgs::Iam::Principal`) for the SMA-752 author. Do not wait for the answer: continue with Step 5 and the next tasks. If a later answer changes a name, the coordinator orders a fix commit that changes it here and regenerates the fixture and the Task 5 golden files.
- [ ] **Step 5: Run.** Same command. Expected: PASS.
- [ ] **Step 6: Commit** `feat(rs): generate the tool policy Cedar schema and add the starter policy (SMA-774)`.

---

### Task 5: Compiler protocol, binary, golden output and decision corpus

**Files:**
- Create: `rs/tools/paigasus-policy-compiler/src/{main.rs, protocol.rs}`, `protocol/fixtures/{request_tenant_accepted.json, result_tenant_accepted.json, request_tenant_rejected.json, result_tenant_rejected.json, request_floor_accepted.json, result_floor_accepted.json, request_floor_rejected.json, result_floor_rejected.json}` (and `reason_codes.json` only if no source reaches `tool-policy-lowering-failed`), `tests/{protocol_fixtures.rs, golden.rs, corpus.rs}`, `tests/golden/*.dw` and `*.result.json` (a floor golden is named `floor_*.dw`; every other golden is a tenant set), `corpus/*.json`
- Modify: `src/lib.rs`, `src/compile.rs`, `src/translate.rs` (derive `Serialize`), `Cargo.toml` (`serde = { version = "1", features = ["derive"] }`, `blake3 = "1"` as a dev-dependency for the pin test), `Cargo.lock`

**Interfaces:**
- Consumes: Task 2 `compile_cedarschema`, Task 4 fixture `protocol/fixtures/base_schema.json`.
- Produces (`src/lib.rs`): `pub const COMPILER_GENERATION: u32 = 1;`, `pub const PROTOCOL_VERSION: u32 = 1;`, `pub const DOGWOOD_COMMIT: &str = "ff158506cfd9c7871385ed3a87d7435505fd24b1";`, `pub fn revision_string() -> String` → `"1+dogwood.ff158506cfd9+proto.1"`.
- Produces (`src/protocol.rs`, serde, `#[serde(deny_unknown_fields)]` on requests):
  - `CompileRequest { protocol_version: u32, kind: SetKind /* "tenant" | "floor" */, source: String, schema_json: serde_json::Value, max_window_ms: u64 }`
  - `CompileResult` tagged `"outcome"`: `Accepted { protocol_version, compiler_generation, compiled: Compiled }` | `Rejected { protocol_version, compiler_generation, rejections: Vec<Rejection> }`
  - JSON names: `cedar_policies`, `cedar_schema_json`, `conditions[]` with `slot`, `policy_id`, `scope` (`"session"|"principal"|"key"`), `window_ms`, `shape` tagged `"type"` (`"formerly"|"count_within"|"sum_within"`), `predicate` tagged `"type"` (`"event_kind"|"tool"|"any_catalog_tool"|"field"|"all_of"|"any_of"|"negation"`), `op` (`"lt"|"le"|"eq"|"ge"|"gt"|"ne"`), literals tagged `"type"` (`"long"|"string"|"bool"|"decimal"`); rejection `policy_id`, `line`, `column`, `reason` (kebab code), `message`.
  - `RevisionOutput { compiler_generation: u32, protocol_version: u32, revision: String }`
  - `pub fn compile_request(req: &CompileRequest) -> Result<CompileResult, ProtocolError>` — converts `schema_json` to cedarschema text (D6), then `compile_cedarschema`
- `main.rs` behaviour: `paigasus-policy-compiler compile` reads all stdin, writes one JSON line to stdout, exit 0 for accepted and rejected; a request that does not parse, or a `protocol_version` other than 1, writes a one-line reason to stderr and exits 2. `paigasus-policy-compiler revision` prints `RevisionOutput` JSON and exits 0. Any other argument exits 64.

- [ ] **Step 1: Write the failing tests**

```rust
// tests/protocol_fixtures.rs
#[test] fn every_fixture_request_produces_its_fixture_result() { /* for each request_*.json: compile_request(..) serialised == result_*.json (serde_json::Value equality) */ }
#[test] fn every_rejection_reason_matches_the_shared_fixture() { /* the six ReasonCode::as_wire values == the union of "reason" values over every result_*_rejected.json (tenant and floor; floor-permit comes only from the floor pair); if no source reaches lowering-failed, the union plus reason_codes.json == the six */ }
#[test] fn an_unknown_protocol_version_is_a_protocol_error() {}
#[test] fn an_unknown_request_field_is_a_protocol_error() {}
// tests/golden.rs
#[test] fn the_starter_compiles_against_the_base_schema() { /* base_schema.json, kind tenant, STARTER text copied into tests/golden/starter.dw → accepted, 0 conditions */ }
#[test] fn a_generic_floor_compiles_against_the_base_schema() {}
/// spec §6.2: golden output pinned to COMPILER_GENERATION.
#[test] fn golden_output_is_pinned_to_the_compiler_generation() {
    // For every tests/golden/*.dw: compile, compare with *.result.json (PAIGASUS_WRITE_FIXTURES=1 rewrites).
    // Then blake3 over all result files == EXPECTED_GOLDEN_HASH, asserted together with COMPILER_GENERATION == 1;
    // the message says: bump COMPILER_GENERATION and replace the hash.
}
#[test] fn the_binary_round_trips_over_stdin() { /* std::process::Command::new(env!("CARGO_BIN_EXE_paigasus-policy-compiler")).arg("compile"), feed a fixture, exit 0, stdout == fixture result */ }
#[test] fn the_binary_exits_2_on_a_bad_request() {}
#[test] fn revision_prints_generation_and_commit() { /* stdout JSON revision == "1+dogwood.ff158506cfd9+proto.1" */ }
// tests/corpus.rs (spec §10 decision corpus)
#[test] fn the_decision_corpus_is_current() {
    // For each accepted shape (formerly, count_within, sum_within): a policy, an event sequence, and the decisions
    // from dogwood_language::Authorizer::is_authorized over that sequence (the event being decided recorded first).
    // Write corpus/<shape>.json = { policy, compiled (our CompiledSet), events, decisions, paigasus_differs: [...] }.
    // sum_within carries a negative-amount event; `paigasus_differs` states that Paigasus adds 0 (spec §5.4).
    // Compare with the committed files; PAIGASUS_WRITE_FIXTURES=1 rewrites.
    // Also record in each file whether `formerly` saw the event being decided (spec §5.4 open item).
}
```

The golden corpus covers: the starter, a generic floor, each accepted shape, `@scope` principal and key, a hyphenated id, and two policies whose slots must not move.

- [ ] **Step 2: Run to verify failure.** `cargo test --locked`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** `protocol.rs`, `main.rs`, the serde derives, and write the fixtures, golden files and corpus with `PAIGASUS_WRITE_FIXTURES=1`. Read each written file before you commit it: the starter must give 0 conditions, and the `formerly` current-event answer must be stated in `corpus/formerly.json`.
- [ ] **Step 4: Run.** `cargo test --locked && cargo clippy --locked --all-targets -- -D warnings && cargo fmt --check`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): add the policy compiler CLI, protocol fixtures, golden tests and corpus (SMA-774)`.

---

### Task 6: IAM core — protocol types, the identical-output comparator, the 4.14 checks, the cross-version proof

**Files:**
- Create: `rs/crates/libs/paigasus-iam-core/src/tool_policy/{protocol.rs, compare.rs, check.rs, ports.rs}`, `rs/crates/libs/paigasus-iam-core/tests/tool_policy_cross_version.rs`
- Modify: `tool_policy/mod.rs`, `rs/crates/libs/paigasus-iam-core/moon.yml` (`test` inputs add `/rs/tools/paigasus-policy-compiler/tests/golden/**/*`; Task 4 already added `protocol/fixtures/**/*`)

**Interfaces:**
- Produces (`protocol.rs`): IAM's own serde mirror of Task 5's JSON: `CompileRequest`, `CompileResult`, `CompiledOutput { cedar_policies: String, cedar_schema_json: serde_json::Value, conditions: Vec<HistoryCondition> }`, `HistoryCondition`, `Shape`, `Predicate`, `Literal`, `Scope`, `CmpOp`, `EventKind`, `Rejection { policy_id: Option<String>, line: Option<u32>, column: Option<u32>, reason: String, message: String }`, `RevisionOutput`, `pub const PROTOCOL_VERSION: u32 = 1;`, `pub enum SetKind { Tenant, Floor }`.
- Produces (`compare.rs`): `pub fn outputs_identical(a: &CompiledOutput, b: &CompiledOutput) -> bool` — compares only `cedar_policies`, `cedar_schema_json` (canonical: sorted keys) and `conditions` (spec §8.1).
- Produces (`check.rs`): `pub fn check_compiled(kind: SetKind, out: &CompiledOutput) -> Result<(), String>` — cedar 4.14: parse `cedar_policies`, `Schema::from_json_value(cedar_schema_json)`, strict validation (spec §5.3 step 9); for `Floor`, every policy effect is `Forbid` (step 10).
- Produces (`ports.rs`):
  ```rust
  pub enum CompilerFailure { Busy, Unavailable(String) } // the String is for logs only
  pub enum CompileOutcome { Accepted(CompiledOutput), Rejected(Vec<Rejection>) }
  #[async_trait] pub trait PolicyCompiler: Send + Sync {
      fn generation(&self) -> u32;
      async fn compile(&self, req: &CompileRequest) -> Result<CompileOutcome, CompilerFailure>;
  }
  ```
- Recorded exception to the `warnings = "deny"` constraint: no Task 6 test uses `PolicyCompiler`, `CompileOutcome` or `CompilerFailure`; they are `pub` library items (no rustc warning), first used in Task 11.

- [ ] **Step 1: Write the failing tests**

```rust
// protocol.rs tests
#[test] fn every_compiler_fixture_round_trips() { /* each request_/result_ file under rs/tools/.../protocol/fixtures parses into these types and re-serialises to an equal serde_json::Value */ }
// compare.rs tests
#[test] fn identical_ignores_key_order_in_the_schema() {}
#[test] fn a_changed_threshold_is_not_identical() {}
#[test] fn a_changed_policy_text_is_not_identical() {}
// check.rs tests
#[test] fn a_floor_with_a_permit_fails_the_second_check() {}
#[test] fn a_policy_that_fails_4_14_validation_is_rejected() { /* names an attribute absent from the schema */ }
// tests/tool_policy_cross_version.rs (spec §10 cross-version proof)
#[test] fn every_golden_result_validates_under_cedar_4_14() { /* for each tests/golden/*.result.json with outcome accepted: kind = Floor for floor_*, else Tenant; check_compiled(kind, &compiled) == Ok(()) */ }
```

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-iam-core`. Expected: FAIL to compile.
- [ ] **Step 3: Implement.** `async-trait` is already an iam-core dependency.
- [ ] **Step 4: Run.** Same command. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): add the compiler protocol types, the output comparator and the Cedar 4.14 checks (SMA-774)`.

---

### Task 7: Moon project, CI coverage, image build and Dependabot for the compiler

**Files:**
- Create: `rs/tools/paigasus-policy-compiler/{moon.yml, deny.toml}`
- Modify: `.moon/workspace.yml`, `.github/dependabot.yml`, `.github/workflows/images.yml`, `.github/workflows/ci.yml`, `ci/cargo-lock-integrity/run.sh`, `ci/cargo-lock-integrity/README.md`, `rs/.dockerignore`, `rs/Dockerfile`, `ci/affected-graph/run.sh` (only if Step 6 says so)

**Interfaces:**
- Produces: Moon project id `paigasus-policy-compiler-rs` with tasks `build`, `test`, `lint`, `fmt`, `deny`, `machete`; `build` writes `rs/tools/paigasus-policy-compiler/target/debug/paigasus-policy-compiler` (Task 14 relies on this path). The IAM image holds `/usr/local/bin/paigasus-policy-compiler` (Task 11's default `compiler_path`).

Required content:
- `.moon/workspace.yml`: add the literal entry `'rs/tools/paigasus-policy-compiler'` under `projects.globs`.
- `moon.yml`: `id: 'paigasus-policy-compiler-rs'`, `layer: 'tool'`, `language: 'rust'`. Each inherited task gets `options: { mergeInputs: 'replace' }` and these inputs: `@group(sources)`, `@group(tests)` (not on `build`), `Cargo.toml`, `Cargo.lock`, `protocol/**/*`, `tests/golden/**/*`, `corpus/**/*`, `/rs/rust-toolchain.toml`, `/rs/.cargo/config.toml` (A10), `/rs/rustfmt.toml` (fmt only). Do NOT list `/rs/.config/nextest.toml`: nextest reads `.config/nextest.toml` from the compiler's own workspace root, so that input would be dead. It must NOT list `/rs/Cargo.lock` or `/rs/Cargo.toml`, so the `lockfile->all-lint` set in `ci/affected-graph/run.sh` does not change. `deny`: `cargo deny --locked check`, inputs `Cargo.toml`, `Cargo.lock`, `deny.toml`. `machete`: `cargo machete .`. Both `toolchain: 'system'`, as the root `moon.yml` tasks are. All cargo commands pass `--locked`. Record in a `moon.yml` comment (spec §5.1 asks for a reason for each place): `repo:deny` reads only `rs/Cargo.toml`, so this project's `deny` task covers the compiler lock under `:deny`; `repo:machete` runs `cargo machete rs`, which is recursive and so already scans the compiler, and this project's `machete` task keys the check on the compiler's own inputs.
- `deny.toml`: a copy of `rs/deny.toml` with `{ name = "tiny-keccak", allow = ["CC0-1.0"] }` (reason: pulled by `rhai`, a mandatory Dogwood dependency, D3) and `[sources] allow-git = ["https://github.com/dogwood-policy/dogwood"]` (reason: spec §3.7). Each further exception that `cargo deny` asks for gets a written reason in the same style.
- `.github/dependabot.yml`: a second `cargo` entry, `directory: /rs/tools/paigasus-policy-compiler`, same schedule and commit prefix, with `ignore:` for `dogwood-language` and `cedar-policy` (reason comment: spec §3.2, no Dogwood bump before PR 3; cedar must stay 4.11.0).
- `images.yml`: add only `'rs/tools/paigasus-policy-compiler/Cargo.lock'` and `'rs/tools/paigasus-policy-compiler/Cargo.toml'` to the `pull_request.paths` list. The filter keeps sources out on purpose (SMA-520 comment), and spec §5.1 asks only for the second lockfile. The push filter already has `rs/**`.
- `ci.yml`: the lock-integrity step stays where it is; `ci/cargo-lock-integrity/run.sh` checks both workspaces (`rs` and `rs/tools/paigasus-policy-compiler`), and its `--negative-control` proves that a truncated compiler lock reds with rc 1. Add `rs/tools/paigasus-policy-compiler/target` to the Rust cache `path:` list and `'rs/tools/paigasus-policy-compiler/Cargo.lock'` to the `hashFiles(...)` of the key (a new hash input changes the key, so the cache saves).
- `rs/.dockerignore`: `target/` → `**/target/`.
- `rs/Dockerfile` builder stage: add `--mount=type=cache,id=cargo-git,target=/usr/local/cargo/git` next to the registry mount; after the service build, `if [ "${BIN}" = "paigasus-iam" ]; then cargo auditable build --release --locked --manifest-path tools/paigasus-policy-compiler/Cargo.toml (target cache id=target-policy-compiler) && cp …/release/paigasus-policy-compiler /out/extra/; fi` with `mkdir -p /out/extra` always. Final stage: `COPY --from=builder /out/extra/ /usr/local/bin/`. The gateway image gets an empty copy. Keep the self-test stage above `FROM scratch`.

- [ ] **Step 1: Write the failing checks.** Run `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; moon query projects | grep -c paigasus-policy-compiler-rs`. Expected: `0`.
- [ ] **Step 2: Add the files and edits above.**
- [ ] **Step 3: Run the project tasks.** `moon run paigasus-policy-compiler-rs:build paigasus-policy-compiler-rs:test paigasus-policy-compiler-rs:lint paigasus-policy-compiler-rs:fmt paigasus-policy-compiler-rs:deny paigasus-policy-compiler-rs:machete`. Expected: all pass. If `deny` reds on a license or source, add the exception with a reason, never a blanket allow.
- [ ] **Step 4: Run the lock-integrity gate.** `bash ci/cargo-lock-integrity/run.sh --self-test && bash ci/cargo-lock-integrity/run.sh --negative-control && bash ci/cargo-lock-integrity/run.sh`. Expected: rc 0 for each.
- [ ] **Step 5: Build the IAM image and run the compiler in it.** `bash ci/images/run.sh build iam` (or the `docker build -f rs/Dockerfile --build-arg BIN=paigasus-iam rs/` line in the Dockerfile header), then `docker run --rm --entrypoint /usr/local/bin/paigasus-policy-compiler <image> revision`. Expected: JSON with `"revision":"1+dogwood.ff158506cfd9+proto.1"`. Also build the gateway image and check that `/usr/local/bin/paigasus-policy-compiler` is absent.
- [ ] **Step 6: Run the affected-graph gate with system bash.** `moon run repo:affected-smoke` with `/bin/bash` 3.2 (root CLAUDE.md "This development Mac only"; memory: put a bash-only shim dir first if Moon picks Homebrew bash). If a check reds because it enumerates `language: rust` projects against `rs/crates` crates, extend that check for `rs/tools/*` with a written reason. If a case for `rs/Cargo.lock` reds, the `mergeInputs` override is wrong; fix the override, not the expected set. Also run `actionlint` for the two edited workflows (`moon run repo:actionlint`; it needs bash 5 and a healthy pipe — read the preflight line; on a small pipe use the Linux container from memory `small-pipe-linux-container-workaround`).
- [ ] **Step 7: Commit** `feat(ci): build, test and gate the policy compiler workspace (SMA-774)` with every file of this task.

---

### Task 8: Contracts — tool policy messages, `ToolPolicyService`, error codes, capability

**Files:**
- Modify: `contracts/proto/paigasus/iam/v1/iam.proto`, `contracts/proto/paigasus/common/v1/error.proto`, `contracts/proto/paigasus/common/v1/service_info.proto`, `rs/crates/libs/paigasus-proto/src/{error.rs, capability.rs}`, `ts/packages/paigasus-discovery/src/types.ts`, `ts/packages/paigasus-discovery/tests/vocabulary.test.ts`, `ts/packages/paigasus-proto/src/capability.test.ts`
- Regenerate: `rs/crates/libs/paigasus-proto/src/generated/**`, `py/packages/paigasus-proto/src/paigasus_proto/generated/**`, `ts/packages/paigasus-proto/src/generated/**`

**Interfaces:**
- Produces (Rust, generated): `paigasus_proto::paigasus::iam::v1::{tool_policy_service_server::ToolPolicyService, …}` and every message below; `ErrorReason::{ToolPolicyInvalid … ToolPolicyVersionNotFound}`; `Capability::IamToolPolicies`.

`error.proto`, a new block `// ---- IAM: tool policies (1-299) ----` after DPoP, one comment line per code with its "when" from spec §7: `ERROR_REASON_TOOL_POLICY_INVALID = 47`, `…_TOOL_POLICY_SHAPE_UNSUPPORTED = 48`, `…_TOOL_POLICY_LOWERING_FAILED = 49`, `…_TOOL_POLICY_FLOOR_PERMIT = 50`, `…_TOOL_POLICY_ID_INVALID = 51`, `…_TOOL_POLICY_WINDOW_TOO_LONG = 52`, `…_TOOL_CATALOG_BREAKS_POLICY = 53`, `…_TOOL_CATALOG_INVALID = 54`, `…_TOOL_CATALOG_CONFLICT = 55`, `…_TOOL_POLICY_VERSION_CONFLICT = 56`, `…_TOOL_POLICY_RECOMPILE_PENDING = 57`, `…_TOOL_POLICY_COMPILER_UNAVAILABLE = 58`, `…_TOOL_POLICY_COMPILER_BUSY = 59`, `…_TOOL_POLICY_VERSION_NOT_FOUND = 60`. The first eight comments say "a rejection reason in a ToolPolicyRejection, not a gRPC error".

`service_info.proto`: `CAPABILITY_IAM_TOOL_POLICIES = 7;` with the comment `"iam.tool.policies" — the tool catalog and the tool policy sets can be administered (ToolPolicyService). Absent when tool_policies.enabled is false or the compiler binary is missing.`

`iam.proto` (buf lint: every enum value carries its enum prefix; `_UNSPECIFIED = 0`). Oneof members use `all_of`, `any_of`, `negation`, because `and`, `or`, `not` are Python keywords and would break the betterproto output.
- Enums: `ToolKind`, `ToolSource`, `ToolImportFormat` (`OPENAI_CHAT_COMPLETIONS`, `OPENAI_RESPONSES`, `MCP_TOOLS_LIST`), `ToolPolicySetKind` (`TENANT`, `FLOOR`), `ToolPolicyVersionStatus` (`LIVE`, `SUPERSEDED`, `HELD`, `DISCARDED`), `ToolPolicyVersionOrigin` (`SAVE`, `STARTER`, `CATALOG_CHANGE`, `RECOMPILE`, `ACCEPT`), `HistoryScope` (`SESSION`, `PRINCIPAL`, `KEY`), `HistoryEventKind` (`REQUEST`, `RESPONSE`), `ComparisonOp` (`LT`, `LE`, `EQ`, `GE`, `GT`, `NE`).
- `CompiledToolPolicySet { kind=1; org_id=2 (uuid string, empty for floor); uint64 version=3; uint32 compiler_generation=4; uint32 schema_revision=5; uint64 catalog_revision=6; string cedar_policies=7; string cedar_schema=8 /* spec §5.4 name; Cedar JSON schema form as a string; the internal serde types keep cedar_schema_json */; repeated HistoryCondition conditions=9; uint64 max_window_ms=10; repeated string catalog_names=11; }`
- `HistoryCondition { string slot=1; string policy_id=2; HistoryScope scope=3; uint64 window_ms=4; oneof shape { FormerlyShape formerly=5; CountWithinShape count_within=6; SumWithinShape sum_within=7; } }`, `FormerlyShape { HistoryPredicate predicate=1; }`, `CountWithinShape { predicate=1; ComparisonOp op=2; int64 threshold=3; }`, `SumWithinShape { predicate=1; repeated string field_path=2; op=3; threshold=4; }`
- `HistoryPredicate { oneof node { HistoryEventKind event_kind=1; ToolMatch tool=2; FieldComparison field=3; PredicatePair all_of=4; PredicatePair any_of=5; HistoryPredicate negation=6; } }`, `ToolMatch { string name_key=1; bool any_catalog_tool=2; }`, `FieldComparison { repeated string path=1; ComparisonOp op=2; LiteralValue value=3; }`, `LiteralValue { oneof value { int64 long_value=1; string string_value=2; bool bool_value=3; string decimal_value=4; } }`, `PredicatePair { HistoryPredicate left=1; HistoryPredicate right=2; }`
- Doc comments on `CompiledToolPolicySet` and `HistoryCondition` state every evaluation rule of spec §5.4 (closed window, event recorded first, `formerly` and the current event per `corpus/formerly.json`, `name_key` matching, `sum_within` adds 0 for missing or negative and saturates, `KEY` scope falls back to the principal, retention ≥ `max_window_ms` or fail closed, missing slot / unknown shape / unknown enum fails closed, the `floor/` and `tenant/` merge rule).
- Catalog: `Tool { string name_key=1; string display_name=2; ToolKind kind=3; string input_schema_json=4; string cedar_input_type_json=5; repeated DroppedField dropped_fields=6; ToolSource source=7; google.protobuf.Timestamp created_at=8; google.protobuf.Timestamp updated_at=9; }`, `DroppedField { string path=1; string reason=2; }`, `SkippedImportEntry { uint32 index=1; string type=2; }`, `ToolPolicyRejection { string policy_id=1; uint32 line=2; uint32 column=3; string reason=4; string message=5; }`, `ToolPolicyRejections { repeated ToolPolicyRejection rejections=1; }`, `CatalogWriteAccepted { uint64 catalog_revision=1; uint64 tool_policy_set_version=2 /* the new live version, 0 when the set did not change */; repeated Tool tools=3; repeated SkippedImportEntry skipped=4; }`
- `PutToolRequest { string org_prn=1; string name=2; ToolKind kind=3; string input_schema_json=4; uint64 expected_catalog_revision=5; }` → `PutToolResponse { oneof result { CatalogWriteAccepted accepted=1; ToolPolicyRejections rejected=2; } }`; `DeleteToolRequest { org_prn=1; name=2; expected_catalog_revision=3; }` → same oneof; `ListToolsRequest { org_prn=1; uint32 limit=2; uint64 offset=3; }` → `ListToolsResponse { repeated Tool tools=1; uint64 catalog_revision=2; }`; `ImportToolsRequest { org_prn=1; ToolImportFormat format=2; string document_json=3; string namespace=4; bool replace=5; uint64 expected_catalog_revision=6; }` → same oneof.
- `ToolPolicySetVersion { ToolPolicySetKind kind=1; string org_id=2; uint64 version=3; string source=4; ToolPolicyVersionStatus status=5; ToolPolicyVersionOrigin origin=6; uint32 compiler_generation=7; uint32 schema_revision=8; uint64 catalog_revision=9; string created_by=10; google.protobuf.Timestamp created_at=11; CompiledToolPolicySet compiled=12; }`
- `PutToolPolicySetRequest { org_prn=1; string source=2; uint64 expected_version=3; }` → `PutToolPolicySetResponse { oneof result { ToolPolicySetVersion accepted=1; ToolPolicyRejections rejected=2; } }`; `GetToolPolicySetRequest { org_prn=1; optional uint64 version=2; }` → `GetToolPolicySetResponse { ToolPolicySetVersion set=1; }` (no live set and no version: `version = 0`, empty source, no `compiled`); `PutFloorPolicySetRequest { string source=1; uint64 expected_version=2; }` → oneof as above; `GetFloorPolicySetRequest { optional uint64 version=1; }` → `GetFloorPolicySetResponse { ToolPolicySetVersion set=1; }`.
- `service ToolPolicyService { rpc PutTool; rpc DeleteTool; rpc ListTools; rpc ImportTools; rpc PutToolPolicySet; rpc GetToolPolicySet; rpc PutFloorPolicySet; rpc GetFloorPolicySet; }`

- [ ] **Step 1: Write the failing mirror tests.** In `paigasus-proto/src/error.rs`, append the 14 codes to `EXPECTED_REASONS` under `// IAM: tool policies (SMA-774)`, and add `assert_eq!(ErrorReason::ToolPolicyVersionNotFound as i32, 60);`. In `capability.rs`, `ALL: [Capability; 7]` adds `Capability::IamToolPolicies`, and `assert_eq!(Capability::IamToolPolicies.as_wire_key().unwrap(), "iam.tool.policies");`. In TS, add `'iam.tool.policies'` to `CapabilityKey` and to `declared` in `vocabulary.test.ts`, and `expect(capabilityWireKey(Capability.IAM_TOOL_POLICIES)).toBe('iam.tool.policies')`.
- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-proto`. Expected: FAIL to compile (unknown variants).
- [ ] **Step 3: Edit the protos, then** `cd contracts && buf format -w && cd .. && moon run contracts:generate --force`. Check that `ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts` still exists (memory: a BSR rate limit deletes it).
- [ ] **Step 4: Run.** `moon run contracts:lint contracts:fmt contracts:breaking paigasus-proto-rs:test`, `moon run paigasus-discovery-ts:test paigasus-proto-ts:test` (use the Moon target ids from `moon query projects`), and `moon run repo:pyo3-stub-drift` if it is affected. Expected: all pass; `breaking` passes because only new messages, values and a new service are added.
- [ ] **Step 5: Commit** `feat(contracts): add ToolPolicyService, the compiled tool policy set and its error codes (SMA-774)` with the protos, the three generated trees and the mirror edits.

---

### Task 9: IAM authz actions for the tool catalog and the policy sets

**Files:**
- Modify: `rs/crates/libs/paigasus-iam-core/src/authz/{action.rs, schema.rs, roles.rs}`

**Interfaces:**
- Produces: `Action::{ManageToolCatalog, ReadToolCatalog, ManageToolPolicies, ReadToolPolicies, ManageToolPolicyFloor, ReadToolPolicyFloor}` with the same wire names. `ManageToolPolicySettings`, `ManageToolPolicyRecompile` and `ReadCompiledToolPolicies` are PR 2 / PR 3.

Rules: add the six to the enum, `Action::ALL` (length 51, message adds `+ 6 SMA-774 tool policies`), `as_wire`, `is_write` (`Manage*` true, `Read*` false), the exhaustive match in `all_covers_every_variant`, and `SCHEMA_SRC`'s action list. `ORG_ADMIN_ACTIONS` gains `ManageToolCatalog`, `ReadToolCatalog`, `ManageToolPolicies`, `ReadToolPolicies`. `ORG_MEMBER_ACTIONS` gains `ReadToolCatalog`. The two floor actions go to no list (`platform_admin`'s template has no action list, so it holds them). The three `Manage*` writes join the generated `forbid-archived-writes` list, so `STARTER_POLICY_REVISION` becomes `5` with a doc line `5: SMA-774 added ManageToolCatalog, ManageToolPolicies and ManageToolPolicyFloor`, and `EXPECTED_STARTER_CONTENT_HASH` gets the new value that the pin test prints.

- [ ] **Step 1: Write the failing tests**

```rust
// action.rs
#[test] fn the_tool_policy_actions_are_classified_and_round_trip() { /* Manage* is_write && !is_restore; Read* !is_write; all parse(as_wire) */ }
// schema.rs
#[test] fn the_tool_policy_actions_validate_against_the_embedded_schema() { /* six names */ }
// roles.rs, as table cases in the existing decision table
// "org_admin allows ManageToolCatalog on its own org", "org_member allows ReadToolCatalog on its org",
// "org_member denies ReadToolPolicies on its org", "org_admin denies ManageToolPolicyFloor at Root",
// "platform_admin at Root allows ManageToolPolicyFloor at Root",
// "forbid-archived-writes denies ManageToolPolicies on an archived org even for org_admin"
```

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-iam-core authz`. Expected: FAIL.
- [ ] **Step 3: Implement** the edits; run once to read the new hash from the pin test message, then set `EXPECTED_STARTER_CONTENT_HASH`.
- [ ] **Step 4: Run.** `cargo nextest run --locked -p paigasus-iam-core && cargo nextest run --locked -p paigasus-iam --lib`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): add the tool catalog and tool policy authorization actions (SMA-774)`.

---

### Task 10: IAM persistence — migration m0013 and the Postgres repository

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/persistence/migration/m0013_create_tool_policies.rs`, `src/adapters/persistence/entities/{tool_catalog_entry.rs, tool_policy_state.rs, tool_policy_set_version.rs}`, `src/adapters/persistence/pg_tool_policies.rs`, `tests/tool_policies_pg.rs`
- Modify: `migration/mod.rs`, `entities/mod.rs`, `persistence/mod.rs`, `rs/crates/libs/paigasus-iam-core/src/tool_policy/{model.rs, ports.rs}`

**Interfaces:**
- Consumes: Task 3 `ToolEntry`; Task 6 `CompiledOutput`; `paigasus_iam_core::{Transaction, RepositoryError}`; `recover_txn`.
- Produces (iam-core `model.rs` additions):
  ```rust
  pub enum ToolPolicySetKey { Tenant(Uuid), Floor }
  pub enum VersionStatus { Live, Superseded }
  pub enum VersionOrigin { Save, CatalogChange }
  pub struct ToolPolicyState { pub catalog_revision: u64, pub live_version: u64 } // (0, 0) when no row
  pub struct ToolPolicyVersion { pub key: ToolPolicySetKey, pub version: u64, pub source: String, pub compiled: Vec<u8> /* CompiledToolPolicySet proto bytes */, pub compiler_generation: u32, pub schema_revision: u32, pub catalog_revision: Option<u64>, pub status: VersionStatus, pub origin: VersionOrigin, pub created_by: String, pub created_at: DateTime<Utc> }
  pub struct StoredTool { pub entry: ToolEntry, pub created_at: DateTime<Utc>, pub updated_at: DateTime<Utc> }
  ```
- Produces (iam-core `ports.rs` addition):
  ```rust
  #[async_trait] pub trait ToolPolicyRepository: Send + Sync {
      async fn state(&self, key: &ToolPolicySetKey) -> Result<ToolPolicyState, RepositoryError>;
      async fn catalog(&self, org: Uuid) -> Result<Vec<StoredTool>, RepositoryError>;          // sorted by name_key
      async fn live(&self, key: &ToolPolicySetKey) -> Result<Option<ToolPolicyVersion>, RepositoryError>;
      async fn version(&self, key: &ToolPolicySetKey, version: u64) -> Result<Option<ToolPolicyVersion>, RepositoryError>; // live or superseded only
      async fn lock_state_in(&self, tx: &dyn Transaction, key: &ToolPolicySetKey) -> Result<ToolPolicyState, RepositoryError>; // INSERT … ON CONFLICT DO NOTHING, then SELECT … FOR UPDATE
      async fn apply_catalog_in(&self, tx: &dyn Transaction, org: Uuid, upserts: &[ToolEntry], deletes: &[String], now: DateTime<Utc>) -> Result<u64, RepositoryError>; // returns the new catalog_revision (old + 1)
      async fn insert_live_version_in(&self, tx: &dyn Transaction, v: &ToolPolicyVersion) -> Result<(), RepositoryError>; // old live → superseded; state.live_version = v.version
  }
  ```
- Produces: `pub struct PgToolPolicies` in `pg_tool_policies.rs` with `pub fn new(db: DatabaseConnection) -> Self`.

**m0013 DDL** (one migration, `SET LOCAL lock_timeout = '5s'` as m0008-m0012):
- `tool_catalog_entry (org_id UUID NOT NULL REFERENCES organization(id) ON DELETE CASCADE, name_key TEXT NOT NULL, display_name TEXT NOT NULL, kind TEXT NOT NULL CHECK (kind IN ('function','custom')), input_schema JSONB NULL, cedar_input_type JSONB NOT NULL, dropped_fields JSONB NOT NULL DEFAULT '[]', source TEXT NOT NULL CHECK (source IN ('manual','openai','mcp')), created_at TIMESTAMPTZ NOT NULL, updated_at TIMESTAMPTZ NOT NULL, PRIMARY KEY (org_id, name_key), CHECK (kind <> 'custom' OR input_schema IS NULL))` (a `custom` tool has no schema; a `function` tool may have none: a built-in tool with an empty record, spec §4.2)
- `tool_policy_state (set_key TEXT PRIMARY KEY /* 'floor' or 'tenant:<uuid>' */, kind TEXT NOT NULL CHECK (kind IN ('tenant','floor')), org_id UUID NULL REFERENCES organization(id) ON DELETE CASCADE, catalog_revision BIGINT NOT NULL DEFAULT 0, live_version BIGINT NOT NULL DEFAULT 0, CHECK ((kind = 'floor') = (org_id IS NULL)), CHECK (set_key = CASE WHEN kind = 'floor' THEN 'floor' ELSE 'tenant:' || org_id::text END))`, plus `INSERT INTO tool_policy_state (set_key, kind) VALUES ('floor','floor')`
- `tool_policy_set_version (kind TEXT NOT NULL CHECK (kind IN ('tenant','floor')), org_id UUID NULL REFERENCES organization(id) ON DELETE CASCADE, version BIGINT NOT NULL CHECK (version >= 1), source TEXT NOT NULL, compiled BYTEA NOT NULL, compiler_generation INTEGER NOT NULL, schema_revision INTEGER NOT NULL, catalog_revision BIGINT NULL, status TEXT NOT NULL CHECK (status IN ('live','superseded','held','discarded')), origin TEXT NOT NULL CHECK (origin IN ('save','starter','catalog_change','recompile','accept')), base_version BIGINT NULL, report JSONB NULL, created_by TEXT NOT NULL, created_at TIMESTAMPTZ NOT NULL, CHECK ((kind = 'floor') = (org_id IS NULL)), CHECK ((kind = 'floor') = (catalog_revision IS NULL)))`
- `CREATE UNIQUE INDEX uq_tool_policy_set_version ON tool_policy_set_version (kind, COALESCE(org_id, '00000000-0000-0000-0000-000000000000'::uuid), version)`
- `CREATE UNIQUE INDEX uq_tool_policy_set_version_live ON … (kind, COALESCE(org_id, …)) WHERE status = 'live'` and `uq_tool_policy_set_version_held … WHERE status = 'held'`
- `down` drops the three tables.

- [ ] **Step 1: Write the failing Postgres tests** in `tests/tool_policies_pg.rs` (each starts with `support::start_migrated_postgres()` and seeds one organization the way `tests/tenancy_orgs.rs` does):

```rust
#[tokio::test] async fn state_is_zero_for_an_org_with_no_row() {}
#[tokio::test] async fn apply_catalog_upserts_deletes_and_bumps_the_revision() { /* rev 0 → 1 → 2; catalog sorted by name_key */ }
#[tokio::test] async fn insert_live_version_supersedes_the_previous_live_row() { /* v1 live, v2 live → v1 superseded, state.live_version == 2 */ }
#[tokio::test] async fn a_second_live_row_is_refused_by_the_index() { /* raw INSERT of a second 'live' row for one set → unique violation */ }
#[tokio::test] async fn a_second_held_row_is_refused_by_the_index() { /* raw SQL, two 'held' rows → unique violation */ }
#[tokio::test] async fn a_floor_row_with_an_org_id_is_refused() {}
#[tokio::test] async fn a_function_tool_with_no_schema_is_stored_and_a_custom_tool_with_a_schema_is_refused() {}
#[tokio::test] async fn version_does_not_return_a_held_row() { /* raw SQL inserts a held row v3; version(key, 3) == None */ }
#[tokio::test] async fn lock_state_blocks_a_second_locker_until_commit() { /* two tasks lock_state_in on one key; the second waits until the first commits (use support::race) */ }
#[tokio::test] async fn deleting_an_org_cascades() {}
#[tokio::test] async fn the_floor_state_row_is_seeded() {}
```

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-iam --test tool_policies_pg`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** the migration (register as `m0013_create_tool_policies` in `mod.rs`), the entities and `PgToolPolicies`, following `pg_policies.rs` for `recover_txn`. Map database errors with `persistence::map_err` (`adapters/persistence/mod.rs`, returns `RepositoryError`), not `pg_policies::map_db_err` (returns `AuthzError`). Store `compiled` as the proto bytes the service hands in; this adapter does not decode them.
- [ ] **Step 4: Run.** Same command, then `cargo nextest run --locked -p paigasus-iam --lib migration`. Expected: PASS (Docker required; the suite skips without Docker locally, so check that the run did not skip).
- [ ] **Step 5: Commit** `feat(rs): store the tool catalog and the tool policy set versions (SMA-774)`.

---

### Task 11: IAM compiler adapter, process limits and `[tool_policies]` config

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/adapters/policy_compiler/{mod.rs, process.rs}`, `tests/tool_policies_compiler_process.rs`
- Modify: `src/adapters/mod.rs`, `src/config.rs`, `rs/crates/services/paigasus-iam/iam.toml.example`, `rs/crates/services/paigasus-iam/Cargo.toml` (tokio feature `process`; `libc = "0.2"` with a reason comment: `setrlimit` in `pre_exec`), `rs/Cargo.lock`, `rs/crates/services/paigasus-iam/moon.yml` (`test` inputs add `/rs/tools/paigasus-policy-compiler/protocol/**/*`, because this task's tests are the first IAM readers of the fixtures)

**Interfaces:**
- Consumes: Task 6 `PolicyCompiler`, `CompileRequest`, `CompileResult`, `RevisionOutput`, `PROTOCOL_VERSION`.
- Produces (`config.rs`): `pub struct ToolPoliciesConfig { pub enabled: bool /* false */, pub compiler_path: PathBuf /* "/usr/local/bin/paigasus-policy-compiler" */, pub compile_timeout_ms: u64 /* 10_000 */, pub max_source_bytes: usize /* 262_144 */, pub max_result_bytes: usize /* 3_145_728 */, pub rlimit_as_bytes: u64 /* 1_073_741_824 */, pub rlimit_cpu_secs: u64 /* 15 */, pub compiler_permits: usize /* 2 */, pub max_window_secs: u64 /* 86_400 */, pub max_tools: usize /* 500 */, pub max_input_schema_bytes: usize /* 65_536 */, pub max_schema_depth: usize /* 8 */ }` as `IamConfig.tool_policies`, with the same Defaults/shadow pattern as `AuthzConfig`. `validate` rejects `max_window_secs > 604_800`, `compiler_permits == 0` and `compile_timeout_ms == 0`.
- Produces (`process.rs`):
  - `pub struct ProcessLimits { timeout: Duration, max_stdout: usize, rlimit_as: u64, rlimit_cpu: u64 }` with `pub fn from_config(cfg: &ToolPoliciesConfig) -> Self`. `rlimit_as` is applied only on Linux, so on macOS the boot log line (below) must read it, or the field is dead code under `warnings = "deny"`; if that is not possible, use `#[cfg_attr(not(target_os = "linux"), allow(dead_code))]` with a reason comment.
  - `pub struct RawOutput { pub status: std::process::ExitStatus, pub stdout: Vec<u8>, pub stderr_truncated: String }`
  - `pub async fn run_child(path: &Path, args: &[&str], stdin: &[u8], limits: &ProcessLimits) -> Result<RawOutput, CompilerFailure>` — `tokio::process::Command` with `env_clear()`, `current_dir(<fresh tempdir>)`, `stdin/stdout/stderr = piped`, `kill_on_drop(true)`, `pre_exec` setting `RLIMIT_CPU` everywhere and `RLIMIT_AS` on Linux only (macOS refuses it; log once at boot); stdin write and stdout/stderr reads run concurrently (`tokio::join!`); stdout over `max_stdout` → `Unavailable`; the whole call under `tokio::time::timeout` → `Unavailable("timeout")`; stderr is truncated to 4 KiB and logged, never returned to a caller.
- Produces (`mod.rs`):
  - `pub struct ProcessPolicyCompiler` implementing `PolicyCompiler`: holds the path, the limits, `Arc<tokio::sync::Semaphore>` (`compiler_permits`), and the boot generation.
  - `pub async fn probe(cfg: &ToolPoliciesConfig) -> Option<ProcessPolicyCompiler>` — runs `revision`; `None` (and an `error!` log) when the binary is missing, the call fails, or `protocol_version != PROTOCOL_VERSION`.
  - `compile`: `try_acquire` fails → `CompilerFailure::Busy`; exit status not 0, stdout not a `CompileResult`, a result `protocol_version` or `compiler_generation` other than the boot values → `Unavailable`.

The `tempfile` crate is already a dev-dependency only. Make the working directory with `tempfile` as a normal dependency (version `"3"`, reason comment) or with `std::env::temp_dir()` plus a uuid name removed on drop; choose `tempfile`.

- [ ] **Step 1: Write the failing tests** in `tests/tool_policies_compiler_process.rs`. Each fake compiler is a `#!/bin/sh` script written into a `tempfile::TempDir` with mode 0755; scripts use absolute paths (`/bin/sleep`, `/usr/bin/env`), because the child has no `PATH`.

```rust
#[tokio::test] async fn a_crashing_compiler_is_unavailable() { /* exit 3 */ }
#[tokio::test] async fn a_hanging_compiler_is_killed_at_the_timeout() { /* exec /bin/sleep 60; timeout 500 ms; Unavailable within 2 s; the child pid is gone (kill -0 fails) */ }
#[tokio::test] async fn too_much_output_is_unavailable() { /* /usr/bin/head -c 4000000 /dev/zero; max_stdout 3 MiB */ }
#[tokio::test] async fn bad_json_is_unavailable() { /* echo nope */ }
#[tokio::test] async fn a_large_stdin_and_a_large_stdout_do_not_deadlock() { /* exec /bin/cat; 2 MiB stdin; run_child returns within the timeout with 2 MiB stdout */ }
#[tokio::test] async fn the_child_gets_an_empty_environment() { /* exec /usr/bin/env; set a canary var in the test process; stdout is empty */ }
#[tokio::test] async fn the_child_runs_in_an_empty_directory() { /* /bin/ls -A → empty stdout */ }
#[cfg(target_os = "linux")] #[tokio::test] async fn the_child_inherits_only_the_three_pipes() { /* /bin/ls /proc/self/fd → 0 1 2 and the ls dir fd only */ }
#[cfg(target_os = "linux")] #[tokio::test] async fn rlimit_as_stops_a_large_allocation() { /* fake that allocates 2 GiB with /usr/bin/python3 -c (head -c via a shell var is not enough); panic, never skip, if /usr/bin/python3 is absent (CI runners and rust:1.95.0-bookworm have it) → non-zero exit → Unavailable */ }
#[tokio::test] async fn the_semaphore_answers_busy() { /* permits 1; first call blocks on /bin/sleep 2; second call → Busy at once */ }
#[tokio::test] async fn a_result_with_a_foreign_generation_is_unavailable() { /* script prints result_tenant_accepted.json with compiler_generation 99 */ }
#[tokio::test] async fn probe_returns_none_for_a_missing_binary() {}
#[tokio::test] async fn probe_returns_none_for_a_foreign_protocol_version() {}
// config.rs unit tests
#[test] fn tool_policies_defaults_match_the_spec() {}
#[test] fn a_max_window_over_seven_days_is_refused() {}
```

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-iam --test tool_policies_compiler_process` and `--lib config`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** `process.rs`, `mod.rs` and the config. Document `[tool_policies]` in `iam.toml.example` with every key and default.
- [ ] **Step 4: Run.** Same commands. Expected: PASS on macOS; run the two Linux-only tests once in `docker run --rm -v "$PWD":/w -w /w/rs rust:1.95.0-bookworm cargo nextest run --locked -p paigasus-iam --test tool_policies_compiler_process` (if `cargo-nextest` is not in the image, use `cargo test --locked`). Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): run the policy compiler as a hardened child process (SMA-774)`.

---

### Task 12: IAM application — `ToolPolicyService`, errors and the rejection registry

**Files:**
- Create: `rs/crates/services/paigasus-iam/src/application/tool_policies/{mod.rs, catalog.rs, sets.rs, rejection.rs}`
- Modify: `src/application/mod.rs`, `src/application/error.rs`, `src/application/fakes.rs`, `src/adapters/retryable.rs`, `src/adapters/http/error.rs`, `src/adapters/grpc/convert.rs`, `ci/error-registry/check.py` (and `ci/error-registry/README.md` if it states the scan root), root `moon.yml` (`error-code-single-site` inputs), `rs/crates/services/paigasus-iam/Cargo.toml` and `rs/Cargo.lock` (only if `prost` is added)

**Interfaces:**
- Consumes: Tasks 3, 4, 6, 9, 10, 11.
- Produces (`error.rs`): `ErrorClass::{Aborted, Unavailable, Exhausted}`; `TenancyError::{ToolCatalogConflict, ToolPolicyVersionConflict, ToolPolicyRecompilePending, ToolPolicyCompilerUnavailable, ToolPolicyCompilerBusy, ToolPolicyVersionNotFound}` (unit variants), with `code()` = the kebab codes and `class()` = `Conflict`, `Aborted`, `Precondition`, `Unavailable`, `Exhausted`, `NotFound`; `field()` = `None`. Transport maps: `Aborted` → gRPC `Aborted` / HTTP 409; `Unavailable` → `Unavailable` / 503; `Exhausted` → `ResourceExhausted` / 429. `tenancy_retryable`: `Exhausted` → `Yes`, `Unavailable` → `Unknown`, `Aborted` → `No`.
- Produces (`rejection.rs`): `pub enum RejectionReason { PolicyInvalid, ShapeUnsupported, LoweringFailed, FloorPermit, IdInvalid, WindowTooLong, CatalogBreaksPolicy, CatalogInvalid }` with `pub fn code(&self) -> &'static str` and `pub fn from_wire(s: &str) -> Option<Self>` (only the first six are valid from the compiler); `pub struct ToolPolicyRejection { pub policy_id: Option<String>, pub line: Option<u32>, pub column: Option<u32>, pub reason: RejectionReason, pub message: String }`.
- Produces (`mod.rs`):
  ```rust
  pub struct ToolPolicyServiceDeps<I, C> { pub repo: Arc<dyn ToolPolicyRepository>, pub orgs: Arc<dyn OrganizationRepository>, pub compiler: Arc<dyn PolicyCompiler>, pub authorize: Authorize, pub uow: Arc<dyn UnitOfWork>, pub audit: Arc<dyn AuditLog>, pub limits: ToolPolicyLimits, pub ids: I, pub clock: C }
  pub struct ToolPolicyLimits { pub catalog: CatalogLimits, pub max_source_bytes: usize, pub max_window_ms: u64 }
  pub enum WriteResult<T> { Accepted(T), Rejected(Vec<ToolPolicyRejection>) }
  pub struct CatalogAccepted { pub catalog_revision: u64, pub set_version: u64 /* 0 = unchanged */, pub tools: Vec<StoredTool>, pub skipped: Vec<(usize, String)> }
  impl<I: IdGenerator, C: Clock> ToolPolicyService<I, C> {
      pub fn new(deps: ToolPolicyServiceDeps<I, C>) -> Self;
      pub async fn put_tool(&self, actor: &Prn, org: Uuid, name: &str, kind: ToolKind, input_schema: Option<serde_json::Value>, expected_catalog_revision: u64) -> Result<WriteResult<CatalogAccepted>, TenancyError>;
      pub async fn delete_tool(&self, actor: &Prn, org: Uuid, name: &str, expected_catalog_revision: u64) -> Result<WriteResult<CatalogAccepted>, TenancyError>;
      pub async fn list_tools(&self, actor: &Prn, org: Uuid, page: Page) -> Result<(Vec<StoredTool>, u64), TenancyError>;
      pub async fn import_tools(&self, actor: &Prn, org: Uuid, format: ImportFormat, document: &serde_json::Value, namespace: Option<&str>, replace: bool, expected_catalog_revision: u64) -> Result<WriteResult<CatalogAccepted>, TenancyError>;
      pub async fn put_set(&self, actor: &Prn, org: Uuid, source: &str, expected_version: u64) -> Result<WriteResult<ToolPolicyVersion>, TenancyError>;
      pub async fn get_set(&self, actor: &Prn, org: Uuid, version: Option<u64>) -> Result<Option<ToolPolicyVersion>, TenancyError>;
      pub async fn put_floor(&self, actor: &Prn, source: &str, expected_version: u64) -> Result<WriteResult<ToolPolicyVersion>, TenancyError>;
      pub async fn get_floor(&self, actor: &Prn, version: Option<u64>) -> Result<Option<ToolPolicyVersion>, TenancyError>;
  }
  ```
- `CompiledToolPolicySet` proto bytes are built in `sets.rs` from `CompiledOutput` plus `kind`, `org_id`, `version`, generations, `catalog_revision`, `max_window_ms`, `catalog_names` (`prost::Message::encode_to_vec`; add `prost` with `workspace = true` and a reason comment if it is not already a dependency). `sets.rs` holds both directions: `fn encode_compiled(..) -> Vec<u8>` and `fn decode_compiled(bytes: &[u8]) -> Result<CompiledOutput, TenancyError>`. The coupling decodes the live version's stored bytes and compares with `outputs_identical`; there is no second comparison on bytes (spec §8.1).
- `sets.rs` also holds one helper that adds the `dropped_fields` reason to a rejection whose message names a dropped path (spec §4.3). `put_set` and the catalog coupling both call it.

**Algorithms the tests pin (spec §4.5, §4.6):**
- Every method authorizes first: tenant writes `ManageToolCatalog` / `ManageToolPolicies` and reads `ReadToolCatalog` / `ReadToolPolicies` at the org PRN (the org must exist, else `NotFound`); floor writes `ManageToolPolicyFloor`, reads `ReadToolPolicyFloor` at `root_prn()`.
- Write steps: (1) read `state` and inputs; an expected value that is not current → `ToolPolicyVersionConflict` at once. (2) compile with no transaction open; `CompilerFailure::Busy` → `ToolPolicyCompilerBusy`, `Unavailable` → `ToolPolicyCompilerUnavailable`; an unknown reason string in a rejection → `ToolPolicyCompilerUnavailable`; on accept, `check_compiled` failure → rejection `PolicyInvalid` with the message. (3) `uow.begin()`, `lock_state_in`, recheck `live_version` and `catalog_revision`; a change → drop the tx, `ToolPolicyVersionConflict`. (4) write, `audit.record`, commit. No outbox event (D9).
- `put_set` / `put_floor`: source over `max_source_bytes` → rejection `PolicyInvalid`; a source byte-identical to the live source, AND the live `compiler_generation` == `compiler.generation()` AND the live `schema_revision` == `TOOL_SCHEMA_REVISION` → `Accepted(live)` with no write and no audit entry. If either value differs, compile and write a new version (`origin = Save`): this save is how an admin clears `tool-policy-recompile-pending` in PR 1 (D10, spec §4.6). The tenant compile uses `tenant_schema_json(catalog)`; the floor uses `base_schema_json()` and kind `Floor`.
- Catalog writes (`put_tool`, `delete_tool`, `import_tools`): build the entries (Task 3); a problem → `Rejected` with `CatalogInvalid`, or `Err(ToolCatalogConflict)` for a `Conflict`; `put_tool` with the same `name_key` and a different `display_name` → `ToolCatalogConflict`; `import_tools` with `replace = true` deletes every tool not in the batch; the new catalog over `max_tools` → `CatalogInvalid`; `delete_tool` of an absent name → `NotFound`. Coupling: if a live tenant set exists, and its `compiler_generation != compiler.generation()` or `schema_revision != TOOL_SCHEMA_REVISION` → `ToolPolicyRecompilePending`; else compile the live source against the new catalog: rejected → `Rejected`, each rejection re-tagged `CatalogBreaksPolicy` with the policy id, and the message adds the `dropped_fields` reason when the compiler message names a dropped path; accepted and `outputs_identical` with the live output → no new version; else a new live version with `origin = CatalogChange` in the same transaction as the catalog write.
- Audit (spec §9.6): one `AuditEntry` per committed mutation: `action` = the RPC name (`PutTool`, `DeleteTool`, `ImportTools`, `PutToolPolicySet`, `PutFloorPolicySet`), `resource_prn` = the org PRN or `root_prn()`, `detail` = `{"set": "tenant"|"floor", "version": <n or null>, "origin": "save"|"catalog_change"|null, "catalog_revision": <n or null>}`.

- [ ] **Step 1: Write the failing unit tests** in `application/tool_policies/mod.rs`, with fakes added to `application/fakes.rs`: `InMemoryToolPolicies` (implements `ToolPolicyRepository`, honours expected values, can inject a concurrent change between compile and lock) and `FakePolicyCompiler` (scripted `CompileOutcome` / `CompilerFailure` per call, counts calls).

```rust
#[tokio::test] async fn put_set_writes_version_1_and_one_audit_entry() {}
#[tokio::test] async fn an_empty_tenant_source_is_saved() {}
#[tokio::test] async fn a_byte_identical_save_returns_the_live_version_and_writes_nothing() {}
#[tokio::test] async fn a_stale_expected_version_is_a_version_conflict() {}
#[tokio::test] async fn two_saves_on_one_version_conflict() { /* the fake changes live_version between compile and lock → ToolPolicyVersionConflict, nothing written */ }
#[tokio::test] async fn a_save_during_a_catalog_write_conflicts() { /* catalog_revision changes between compile and lock */ }
#[tokio::test] async fn a_rejection_is_a_successful_result_and_writes_nothing() {}
#[tokio::test] async fn a_4_14_failure_becomes_a_policy_invalid_rejection() {}
#[tokio::test] async fn busy_and_unavailable_map_to_their_errors() {}
#[tokio::test] async fn an_unknown_rejection_reason_is_compiler_unavailable() {}
#[tokio::test] async fn an_oversized_source_is_rejected_without_a_compile() {}
#[tokio::test] async fn the_floor_compiles_against_the_base_schema_with_kind_floor() {}
#[tokio::test] async fn a_floor_permit_that_passed_the_compiler_is_rejected_by_the_second_check() {}
#[tokio::test] async fn put_tool_with_no_live_set_does_not_compile() {}
#[tokio::test] async fn put_tool_with_a_case_variant_of_an_existing_entry_conflicts() {}
#[tokio::test] async fn deleting_a_tool_that_a_live_policy_names_is_rejected() { /* CatalogBreaksPolicy with the policy id; catalog unchanged */ }
#[tokio::test] async fn a_catalog_change_with_identical_output_keeps_the_set_version() {}
#[tokio::test] async fn a_catalog_change_with_different_output_writes_a_catalog_change_version() {}
#[tokio::test] async fn a_live_set_from_an_older_generation_blocks_catalog_writes() { /* ToolPolicyRecompilePending */ }
#[tokio::test] async fn a_live_set_from_an_older_schema_revision_blocks_catalog_writes() {}
#[tokio::test] async fn a_save_clears_recompile_pending() { /* live set from generation 0; save the BYTE-IDENTICAL source → new version, then a catalog write succeeds */ }
#[tokio::test] async fn a_byte_identical_save_at_the_running_generation_writes_nothing() {}
#[tokio::test] async fn put_tool_of_a_builtin_with_no_schema_gets_an_empty_record() { /* kind Function, input_schema None → accepted, cedar_input_type is an empty record */ }
#[tokio::test] async fn a_saved_policy_that_reads_a_dropped_field_names_the_drop_reason() { /* put_set rejection message contains the dropped_fields reason */ }
#[tokio::test] async fn import_with_replace_removes_missing_tools() {}
#[tokio::test] async fn every_mutation_writes_exactly_one_audit_entry() { /* table over the five writes */ }
#[tokio::test] async fn each_rpc_checks_its_action() { /* a Deny for the action → Forbidden, and the compiler was not called */ }
#[tokio::test] async fn get_set_returns_not_found_for_an_absent_version() { /* ToolPolicyVersionNotFound */ }
// rejection.rs
#[test] fn every_rejection_reason_is_declared_in_the_canonical_registry() { /* each code() parses via ErrorReason::from_wire_reason */ }
#[test] fn every_compiler_reason_in_the_shared_fixture_parses() { /* include_str! of result_tenant_rejected.json and result_floor_rejected.json (and reason_codes.json if Task 5 made it) under "../../../../../../tools/paigasus-policy-compiler/protocol/fixtures/"; every reason parses via RejectionReason::from_wire, and the union is the six compiler codes */ }
// sets.rs
#[test] fn decode_compiled_inverts_encode_compiled() { /* every shape, every literal type, window_ms, and the schema JSON string ↔ Value; outputs_identical(decoded, original) */ }
// error.rs — extend every_tenancy_code_is_declared_in_the_canonical_registry (strum EnumIter covers the new variants)
```

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-iam --lib tool_policies error`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** the service, the error variants and the three transport class maps.
- [ ] **Step 4: Register the emission site.** In `ci/error-registry/check.py`: add the `emits` row `("rs/crates/services/paigasus-iam/src/application/tool_policies/rejection.rs", "emits", "every_rejection_reason_is_declared_in_the_canonical_registry", "RejectionReason::code()")`; `check.py` uses `SCAN_ROOT = rs/crates` for BOTH the emission scan and `guard_exists`, so widen both to also cover `rs/tools/*` (emission: `src/**/*.rs`; guard lookup: `src/` and `tests/`), and keep the bounded-glob self-test green. Add `("rs/tools/paigasus-policy-compiler/src/compile.rs", "emits", "every_rejection_reason_matches_the_shared_fixture", "ReasonCode::as_wire() — checked against the shared protocol fixture, which IAM checks against the registry")`. In the root `moon.yml` `error-code-single-site` task, add the inputs `rs/tools/**/src/**/*.rs` and `rs/tools/**/tests/**/*.rs`. Run `python3 ci/error-registry/check.py --self-test && python3 ci/error-registry/check.py --single-site` (through `bash ci/python-env/run.sh` if the task does so). Expected: pass.
- [ ] **Step 5: Run.** `cargo nextest run --locked -p paigasus-iam --lib`. Expected: PASS.
- [ ] **Step 6: Commit** `feat(rs): add the tool policy service with compile on save and catalog coupling (SMA-774)`.

---

### Task 13: IAM transports — gRPC `ToolPolicyService`, HTTP routes, capability wiring

**Files:**
- Create: `src/adapters/grpc/tool_policies.rs`, `src/adapters/http/tool_policies.rs`, `tests/grpc_tool_policies.rs`, `tests/http_tool_policies.rs`
- Modify: `src/adapters/grpc/mod.rs` (register inside `routes()`), `src/adapters/http/mod.rs` (`AppState.tool_policies: Option<ToolPolicySvc>`, router merge), `src/adapters/http/dto.rs`, `src/service_info.rs`, `src/main.rs`, `tests/support/mod.rs`

**Interfaces:**
- Consumes: Task 12 `ToolPolicyService`, Task 11 `probe`, Task 8 generated types.
- Produces: `pub type ToolPolicySvc = ToolPolicyService<KernelIdGenerator, SystemClock>;`; `Capabilities.tool_policies: bool` (true iff `cfg.tool_policies.enabled` and `probe` returned `Some`); `Capability::IamToolPolicies` in `enabled()`.
- HTTP routes (mounted only when `capabilities.tool_policies`, D8; bodies mirror the proto fields in snake_case; a rejection is `200 {"result":"rejected","rejections":[…]}`, an accept is `200 {"result":"accepted", …}`):
  - `GET /v1/organizations/{id}/tools?limit&offset` → ListTools
  - `POST /v1/organizations/{id}/tools` → PutTool (body `name`, `kind`, `input_schema`, `expected_catalog_revision`)
  - `DELETE /v1/organizations/{id}/tools/{name}?expected_catalog_revision=N` → DeleteTool
  - `POST /v1/organizations/{id}/tool-imports` → ImportTools (a separate path, so a tool named `import` stays reachable)
  - `PUT /v1/organizations/{id}/tool-policy-set` / `GET …?version=N` → Put/GetToolPolicySet
  - `PUT /v1/tool-policy-floor` / `GET /v1/tool-policy-floor?version=N` → Put/GetFloorPolicySet
- gRPC: each RPC first checks `state.capabilities.tool_policies`, else `convert::capability_disabled("iam.tool.policies")`; `org_prn` via `convert::node_uuid(prn, "organization")`; errors via `status_to_grpc`.

`main.rs`: call `policy_compiler::probe(&cfg.tool_policies)` only when `enabled`; build `ToolPolicyService` with `PgToolPolicies`, the `SeaOrmUnitOfWork`, `PgAuditLog` and the compiler; log the capability state at boot.

- [ ] **Step 1: Write the failing tests.** `tests/support/mod.rs` gains `pub fn fake_compiler_script(dir: &Path, result_json: &str) -> PathBuf` and `app_with_tool_policies(db, compiler_path) -> (Router, AppState)`.

```rust
// service_info.rs unit tests: extend all_enabled/all_flags_off/disabling_one_flag_removes_exactly_its_key with tool_policies
// grpc_tool_policies.rs (Postgres + a fake compiler script)
#[tokio::test] async fn the_eight_rpcs_answer_capability_disabled_when_the_toggle_is_off() {}
#[tokio::test] async fn the_rpcs_answer_capability_disabled_when_the_binary_is_missing() {}
#[tokio::test] async fn every_error_code_reaches_the_wire_with_its_grpc_code() {
    // table: tool-catalog-conflict/ALREADY_EXISTS, tool-policy-version-conflict/ABORTED,
    // tool-policy-recompile-pending/FAILED_PRECONDITION, tool-policy-compiler-unavailable/UNAVAILABLE,
    // tool-policy-compiler-busy/RESOURCE_EXHAUSTED (+ retryable metadata), tool-policy-version-not-found/NOT_FOUND,
    // not-found, forbidden; reason read with support::reason_of
}
#[tokio::test] async fn every_rejection_reason_reaches_the_wire_in_the_rejected_oneof() { /* eight reasons */ }
#[tokio::test] async fn a_saved_set_round_trips_with_its_compiled_message() { /* GetToolPolicySet.compiled decodes; catalog_names sorted; version 1; status LIVE; origin SAVE */ }
#[tokio::test] async fn get_tool_policy_set_with_no_set_returns_version_0() {}
#[tokio::test] async fn org_member_can_list_tools_but_not_read_policies() {}
#[tokio::test] async fn only_platform_admin_can_read_the_floor() {}
// http_tool_policies.rs
#[tokio::test] async fn the_routes_are_404_when_the_capability_is_off() {}
#[tokio::test] async fn every_error_code_reaches_the_http_envelope_with_its_status() { /* 409, 409, 409, 503, 429, 404 */ }
#[tokio::test] async fn a_rejection_is_200_with_result_rejected() {}
#[tokio::test] async fn a_tool_named_import_can_be_deleted() {}
#[tokio::test] async fn http_and_grpc_return_the_same_accepted_version() {}
```

- [ ] **Step 2: Run to verify failure.** `cd rs && cargo nextest run --locked -p paigasus-iam --test grpc_tool_policies --test http_tool_policies`. Expected: FAIL to compile.
- [ ] **Step 3: Implement** both adapters, the DTOs, the capability and the wiring.
- [ ] **Step 4: Run.** Same command plus `cargo nextest run --locked -p paigasus-iam --lib`, and the existing `tests/grpc_service_info.rs`, `tests/http_service_info.rs`. Expected: PASS.
- [ ] **Step 5: Commit** `feat(rs): serve ToolPolicyService over gRPC and HTTP behind iam.tool.policies (SMA-774)`.

---

### Task 14: IAM with the real compiler binary, and the Moon wiring

**Files:**
- Create: `rs/crates/services/paigasus-iam/tests/tool_policies_real_compiler.rs`
- Modify: `rs/crates/services/paigasus-iam/moon.yml`, `ci/affected-graph/run.sh` (only if Step 4 edits it)

**Interfaces:**
- Consumes: Task 7 build path, Tasks 12-13.
- Produces: env var `PAIGASUS_POLICY_COMPILER_BIN`, set by the IAM `test` task.

`moon.yml` (`paigasus-iam-rs`): `test.deps` adds `'paigasus-policy-compiler-rs:build'`; `test.env` sets `PAIGASUS_POLICY_COMPILER_BIN: '$workspaceRoot/rs/tools/paigasus-policy-compiler/target/debug/paigasus-policy-compiler'` (check that Moon 2.5.3 expands the token; else use a relative path from the project root); `test.inputs` adds `/rs/tools/paigasus-policy-compiler/src/**/*` and `/rs/tools/paigasus-policy-compiler/Cargo.lock` (Task 11 already added `protocol/**/*`). Skip policy: the test never reads `CI` (`repo:iam-docker-policy-single-site` reds on `var("CI")` / `var_os("CI")` in an IAM test). The Moon `test` task always sets the variable, and CI runs that task. Env unset → print `skipped: PAIGASUS_POLICY_COMPILER_BIN is not set` and return; env set but the file is absent → panic.

- [ ] **Step 1: Write the failing tests**

```rust
#[tokio::test] async fn the_real_compiler_compiles_the_starter_against_an_empty_catalog() { /* PutToolPolicySet(STARTER_TOOL_POLICY_SOURCE) → accepted, 0 conditions */ }
#[tokio::test] async fn the_real_compiler_compiles_a_generic_floor() {}
#[tokio::test] async fn a_count_within_policy_round_trips_through_the_real_compiler() { /* catalog tool "search"; conditions[0].count_within.threshold == 2; slot starts with the @id prefix */ }
#[tokio::test] async fn a_catalog_delete_that_breaks_a_live_policy_is_rejected_by_the_real_compiler() {}
#[tokio::test] async fn the_boot_probe_reads_generation_1() { /* also asserts that the path in PAIGASUS_POLICY_COMPILER_BIN exists when the variable is set */ }
```

- [ ] **Step 2: Run to verify failure.** `export PATH=…; moon run paigasus-iam-rs:test` (filter is not possible through Moon; read the test names in the output). Expected: FAIL (test file missing → compile error).
- [ ] **Step 3: Implement** the test file and the `moon.yml` edits.
- [ ] **Step 4: Run.** `moon run paigasus-iam-rs:test` and check in the log that the five tests ran (not skipped). Then run `repo:affected-smoke` with `/bin/bash` 3.2 and fix any case that now lists the new IAM test inputs or the cross-project dep (update the expected set only when the new edge is intended, with a comment).
- [ ] **Step 5: Commit** `test(rs): run IAM against the real policy compiler binary (SMA-774)`.

---

### Task 15: Mutation proofs and the full gate graph

**Files:** none new. Temporary edits only, each removed by deleting the marked insert (memory: do not use `git checkout --` to restore).

- [ ] **Step 1: Mutation battery (spec §10).** For each rule: insert `// MUTATION` lines that delete or bypass the rule, run the named test with `--no-fail-fast`, confirm that it FAILS (and that the mutant COMPILES — memory: under `-D warnings`, prefix an unused binding with `_` if needed), then delete the insert. Record each result (rule, mutation, failing test) in the PR body.

| Rule | Mutation | Must fail |
|---|---|---|
| shape check | `translate_field` accepts `Previous` as `Formerly` | `previous_and_since_are_shape_unsupported` |
| floor check 1 (compiler) | skip the effect check | `a_floor_permit_is_rejected_in_every_form` |
| floor check 2 (IAM) | `check_compiled` skips `Floor` | `a_floor_permit_that_passed_the_compiler_is_rejected_by_the_second_check` |
| 4.14 check | `check_compiled` returns `Ok(())` | `a_4_14_failure_becomes_a_policy_invalid_rejection`, `a_policy_that_fails_4_14_validation_is_rejected` |
| coupling check | catalog write skips the coupling compile | `deleting_a_tool_that_a_live_policy_names_is_rejected` |
| version check | skip the step-3 recheck | `two_saves_on_one_version_conflict` |
| one-live index | drop `uq_tool_policy_set_version_live` in m0013 | `a_second_live_row_is_refused_by_the_index` |
| `env_clear` | remove `env_clear()` | `the_child_gets_an_empty_environment` |

After the last restore, run the whole battery's tests once more without mutations (memory: re-run the battery whole) and confirm `git diff` shows no `MUTATION` text.

- [ ] **Step 2: Run the full gate graph** (root `CLAUDE.md`, the command between the `ci-targets` markers; do not copy it anywhere):

Run under system `/bin/bash` 3.2 for the whole `moon ci` invocation (it is what `repo:affected-smoke` needs), with `--base origin/main --include-relations`. Then re-run directly the gates that need another bash, and read those results instead of the `moon ci` verdict for them:
- bash 4+ (`/opt/homebrew/bin/bash`): `ci/ruff/run.sh`, `ci/next-public/run.sh`, `ci/publish-metadata/run.sh`, `ops/nats/check-subjects.sh` plus its nextest half (`cd rs && cargo nextest run --locked --no-tests=pass -p paigasus-iam --test nats_permissions --test docker_preflight --profile iam-nats`).
- bash 5 with a healthy pipe: `ci/actionlint/run.sh` — read the `pipe capacity` preflight line; on a 512-byte pipe, run it in a Linux container (memory `small-pipe-linux-container-workaround`).
- Also run `bash ci/cargo-lock-integrity/run.sh` (a `ci.yml` step, not a Moon target).

Expected: every target passes. On an unattributed failure, follow the root `CLAUDE.md` diagnosis procedure (copy `.moon/cache/ciReport.json` and the state dir before any re-run).

- [ ] **Step 3: Confirm the commit rules.** `git log origin/main..HEAD --format=%B | grep -c 'SMA-751'` → `0`; every subject ends with `(SMA-774)`; every body ends with the `Co-Authored-By` line.

No commit in this task unless Step 2 needs a fix; a fix gets its own `fix(<scope>): … (SMA-774)` commit.

---

## Self-review notes

- **Spec coverage.** §2 constraints: Global Constraints, D1, D2, D6. §3.2 PR 1 scope: Tasks 1-15. §3.8 proof: Task 1. §4.1: Task 3 (D4). §4.2: Tasks 3, 10, 12. §4.3: Task 4 (D5). §4.4-4.5: Tasks 10, 12. §4.6: Task 12. §5.1: Tasks 1, 7. §5.2: Tasks 5, 11. §5.3: Task 2 (D3, D7), Task 6 (steps 9-10). §5.4: Task 8, Task 12 (bytes). §6.1-6.2: Tasks 4, 5, 10, 11, 13. §6.3: Task 4, Task 14. §7: Tasks 8, 12, 13. §9.1 PR 1 RPCs: Tasks 8, 13. §9.5 PR 1 actions: Task 9. §9.6: Task 12. §10 PR 1 tests: Tasks 1, 2, 5, 6, 10-15. Out: §8, §9.2-9.4, §9.7 (PR 2 / PR 3).
- **Proportion.** Signatures, test names and fixed values only; the two algorithms the spec fixes (write steps, coupling) are stated as rules, not bodies.
