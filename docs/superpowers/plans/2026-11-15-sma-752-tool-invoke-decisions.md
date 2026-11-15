# SMA-752: InvokeTool decisions in the gateway — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Put Cedar decisions behind the gateway's `ToolCallDecider` port, with an always-enforced floor, per-tenant modes (off, log-only, enforce), an explicit fail-open or fail-closed setting, request-side actions, a cached file-backed policy source and NATS invalidation.

**Architecture:** The pure decision function lives in `paigasus-kernel` behind the cargo feature `tool-policy` (types, normalising, argument mapping, compile, `decide`, `decide_tool`). `paigasus-gateway-core` maps its provider types into the kernel types, maps a kernel verdict to a `Decision`, plans the request-side actions and rewrites the request body. All of that stays I/O-free. The gateway service owns the `PolicySource` port, the file source, the cache, the `CedarDecider`, the `RequestToolPolicy` service, the pipeline wiring and the NATS subscription.

**Tech Stack:** Rust 2024 (cedar-policy 4.14.1, serde_json with `raw_value`, unicode-normalization 0.1.25, proptest, tokio, async-nats 0.51, testcontainers), protobuf + buf (three error reasons), bash (one gate assertion), NATS server config.

**Spec:** `docs/superpowers/specs/2026-11-15-sma-752-tool-invoke-decisions-design.md` (approved at Gate 1 on 2026-11-15, Q1-A and Q2-A, revised the same day: D33 has no `floor_mode`, D34 adds a per-org `on_policy_unavailable`). Read it before any task. Every decision id (D1-D34, § x.y) refers to it.

## Plan-level clarifications of the spec

The spec leaves these choices open (§ 8), or a spec type cannot carry a spec rule as written. Sven can change any of them at plan review.

| Id | Clarification | Where |
|---|---|---|
| P-1 | `ToolInput` is `Json(String)` or `Text(String)`, not `Object(serde_json::Map)`. The workspace `serde_json` has no `arbitrary_precision`, so a `Map` cannot keep the original number text that § 6.1.4 and C16 need. Turning that feature on is workspace-wide (feature unification) and changes IAM. The kernel parses the raw text itself with `serde_json::value::RawValue` (feature `raw_value`, additive) and reads each number from its text. | Task 1 |
| P-2 | A `Json` input that is not one strict JSON object (invalid text, not an object, a duplicate key at any depth) maps to `input = {}` and adds the finding `unparsed_arguments`. `input_text` is set only for `Text`. | Task 1 |
| P-3 | `Verdict` gets one more field, `floor_denied: bool`. It is true when the decision is `Deny` and a floor `forbid` is determining or errored. D33 needs it: a deny that a floor policy decided is always enforced, and any other deny follows the tenant mode. `ToolVerdict::DeniedForAllInputs` carries `by_floor: bool` for the same reason. | Tasks 3, 4 |
| P-4 | "Evaluate the floor alone" (tenant mode `off`, or tenant set unavailable) uses `ToolPolicies::compile(floor, "", &[])`. That set has no permit, so its default deny is ignored: only `floor_denied` counts. | Tasks 8, 13 |
| P-5 | "`Allow`, reported as `tool-policy-unavailable`" (§ 6.4, `open`) is `Decision::Observe { would: BlockCode::PolicyUnavailable }`. It is an allow (`is_allow()` is true), and the report shows `enforced: false` with the reason. No new `Decision` variant. | Task 8 |
| P-6 | A PRN that does not parse (principal or scope) and an unavailable floor are the same case: `PolicySet::Unavailable`. Under the tenant's effective `on_policy_unavailable` (D34: the per-org override, else the global value), `closed` blocks every call with `PolicyUnavailable` in every tenant mode, and `open` gives `Observe { would: PolicyUnavailable }`. | Task 8 |
| P-7 | The request plan names tools by **normalised display name**, and results by **call id** (or by name for a legacy `role:"function"` message). `decide_tool` is a function of the name, so every definition with that name is removed together. The rewrite does not need definition indexes. | Tasks 9, 10 |
| P-8 | `rewrite_request` edits the body through `RawValue` slices. It re-serialises only the members it changes (`tools`, `functions`, `tool_choice`, `function_call`, `parallel_tool_calls`, and the replaced result items). Every other member keeps its original bytes, so every untouched number keeps its original text (§ 6.2). | Task 10 |
| P-9 | Each request-side item has its own `enforced` flag (floor-decided items are always enforced, D33). `rewrite_request` applies only the enforced items. The `tool_choice` rewrite is computed from the enforced removals. When all removals together would reset `tool_choice` but the enforced ones do not, the report adds one `reset-tool-choice` entry with `enforced: false`. | Task 9 |
| P-10 | D17 refuses an `Unavailable` turn only when the tenant mode is `enforce`. The floor is always enforced (D33), but D17 names the tenant mode. In the other modes the request is forwarded, logged and counted as "enforce would refuse". The refusal is `GatewayError::BadRequestBody` (400, `invalid-request-body`). | Task 14 |
| P-11 | Report v2 call entries: `decision` stays `allow`/`block`/`unenforceable`. `Observe` reports `decision: "allow"`, `code: <would>`, `enforced: false`. `Block` reports `enforced: true`. Plain `Allow` reports `enforced: null`. `reason` is `tool-call-denied` for `Policy`, `tool-policy-unavailable` for `PolicyUnavailable`, else `null`. A request entry is `{"v":2,"side":"request","tool","name_sha256_8","action","enforced","reason"}`. The overflow summary is `{"v":2,"side":"request","omitted":<n>}`. | Task 6 |
| P-12 | The NATS creds callback moves from `paigasus-iam/src/adapters/events/creds.rs` into a new lib crate `paigasus-nats-creds`, so the gateway uses the same code (§ 6.3.5 "the same callback that IAM uses"). IAM re-exports nothing new; it imports the crate. | Task 17 |
| P-13 | The registry code strings are spelled only in `paigasus-gateway-core/src/report.rs`. The gateway's `application/tool_policy/` reads them through `RequestReason::as_str()` / `reason_label()`. If an implementer must spell a code in the gateway module, that file gets its own `emits` MANIFEST row and membership test in the same task. | Tasks 6, 14 |
| P-14 | The shipped floor (D15) is the file `rs/crates/services/paigasus-gateway/tool-policies/floor.cedar`. Tests read it with `include_str!`. | Task 12 |
| P-15 | Responses built-in client-run tools that are removable: the definition types `local_shell`, `shell`, `apply_patch`, `computer` and `computer_use_preview`. Every other kind-`Other` definition is a hosted tool and is never removed (§ 6.5.2). | Task 9 |

## Global Constraints

- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for shell, Python, TOML and YAML). A `.cedar` file opens with `// SPDX-License-Identifier: Apache-2.0` too (Cedar has `//` comments).
- Rust crates use edition 2024 and rust-version 1.95 through `edition.workspace` and `rust-version.workspace`.
- The workspace has `warnings = "deny"`. Dead code, an unused import or an unused dependency is a hard error. Never "add now, wire later" across tasks: each task's new code is `pub` and used by a test in the same task, or used by production code in the same task. Never add `#[allow(dead_code)]`.
- The kernel has its own lint table (`warnings = "warn"`); CI still passes `-D warnings`. Treat every kernel warning as an error.
- `repo:machete` reds on a dependency that no code uses. Add each dependency in the task whose code first uses it.
- `rustfmt.toml` sets `max_width = 200`. Run `cargo fmt` in `rs/` after every Rust edit. Verify with `cargo fmt --check`.
- Per task: `cd rs && cargo clippy --workspace --all-targets -- -D warnings` and `cargo nextest run -p <crate>` must pass. `cd rs` before any `cargo` command.
- Prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Conventional commits with a scope: `feat(rs): …`, `feat(contracts): …`, `feat(ops): …`, `test(rs): …`, `ci: …`, `docs(rs): …`. Put `(SMA-752)` at the end of the subject; never start the subject with it. No body line starts with `#NNN` or with `word:`. End every commit message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never use `--no-verify`. Never amend, reset or rebase a commit that an earlier task made. Commit only your own task's files.
- Do not install software on the host. If a tool is missing, stop and report. Use `docker run` for a Linux tool if a task needs one.
- No step needs network access to OpenAI or Anthropic. No API key exists for this work.
- The kernel version is not edited by hand. A `feat(rs)` commit on the kernel makes release-plz cut 0.3.0 (`rs/release-plz.toml`: `features_always_increment_minor = true`), and the release job's `version-lockstep --write` moves the binding versions (D27).
- The kernel never depends on a gateway crate. The bindings never get Cedar (D2, D21). `tool_policy` exists only under the feature `tool-policy`, which is off by default.
- `paigasus-gateway-core` stays I/O-free: no tokio, futures, axum or reqwest.
- With `[tool_policy] enabled = false` (the default), the gateway behaves exactly as today. Every existing gateway test passes unchanged at every commit.
- Logs, metric labels and reports never hold arguments, tool-result content or prompts (ADR-0027 (i)). Tool names appear only through `sanitize_name`.
- Security assertions get red-first proof where the spec asks: delete the guarded rule, see the test fail, restore it. Delete only the marked lines; never use `git checkout --` to restore (it also drops the fix).
- `.github/CODEOWNERS` is Moon-generated. Never hand-edit it. If a Moon run changes it, commit the generated file in the same task.
- Moon edges: a `{ workspace = true }` dependency needs a hand-written `dependsOn` and the `fileGroups.upstreams` entries in `moon.yml` (`repo:affected-smoke` A6 checks the closure).
- Bash on the development Mac: run `repo:affected-smoke` with `/bin/bash` 3.2. Run `repo:nats-permissions`' `ops/nats/check-subjects.sh` with `/opt/homebrew/bin/bash`. See the root `CLAUDE.md`.

## Review Focus

These inputs are implied by the spec, but no spec test names them. Each line has a test in the named task.

1. **A call name that differs from the policy literal only by white space, case or Unicode form** (`" Read_File"`, `"read_file"` with a decomposed `é`). The policy must match. Task 1 `normalise_matches_what_a_policy_literal_matches` and Task 7 `a_call_name_is_normalised_before_decide`.
2. **A request body with escaped keys, odd white space and numbers like `1e400` or `12345678901234567890`**, rewritten by a removal. Every untouched member must keep its bytes. Task 10 `untouched_members_keep_their_bytes`.
3. **A policy change between the request side and the response side of one request.** The decider must use the request's own snapshot (D30), not the new cache entry. Task 14 `the_decider_uses_the_request_snapshot_after_invalidate_all`.
4. **A Responses `namespace` tool and a flat tool with the same inner name** (`ns.read_file`, `read_file`). They are different tools: no collision, and a removal of one keeps the other. Task 10 `a_namespace_removal_keeps_the_flat_tool_and_drops_an_empty_namespace`.
5. **An upper-case org UUID in `[[tool_policy.org]]`**. It must apply to the canonical org, as `[[limits.org]]` does, and a second entry that differs only in case is a duplicate. Task 11 `an_upper_case_org_id_applies_to_the_canonical_org`.

---

### Task 0: Provision the worktree and take the baseline

**Files:** none.

- [ ] **Step 1: Provision.** In the worktree: `proto install`, `pnpm -C ts install`, `( cd py && uv sync )`, `( cd rs && cargo fetch )`.
- [ ] **Step 2: Baseline.** Run `cd rs && cargo nextest run -p paigasus-kernel -p paigasus-gateway-core -p paigasus-gateway` and `cargo clippy --workspace --all-targets -- -D warnings`. Expected: PASS. Record the test counts in your report. If anything is red, stop and report; do not fix it.

No commit.

---

### Task 1: Kernel feature, types, name normalising and argument mapping

**Files:**
- Modify: `rs/crates/libs/paigasus-kernel/Cargo.toml` (feature, optional deps, `[package.metadata.docs.rs]`)
- Modify: `rs/crates/libs/paigasus-kernel/src/lib.rs` (`#[cfg(feature = "tool-policy")] pub mod tool_policy;`, crate doc line)
- Create: `rs/crates/libs/paigasus-kernel/src/tool_policy/mod.rs`, `types.rs`, `normalise.rs`, `input.rs`

**Interfaces:**
- Produces (module `paigasus_kernel::tool_policy`, every public type `#[non_exhaustive]`, each with a `new` constructor):
  - `pub fn normalise(name: &str) -> String`
  - `pub enum ToolInput { Json(String), Text(String) }` (P-1)
  - `pub enum Declared { Yes, No, Unknown }` with `as_str() -> &'static str` (`"yes"`, `"no"`, `"unknown"`)
  - `pub struct ToolInvocation { pub name: String, pub input: ToolInput, pub declared: Declared }`
  - `pub struct RequestInfo { pub scope: String, pub ingress: String, pub model: String }`
  - `pub struct SessionInfo { pub id: String, pub turn: i64 }` with `SessionInfo::placeholder()` = `{ id: "", turn: 0 }` (D14)
  - `pub struct Facts { pub principal: CedarUid, pub org: Uuid, pub request: RequestInfo, pub session: SessionInfo }`
  - `pub struct Findings(BTreeSet<String>)` with `new()`, `insert(&mut self, kind: &str)`, `contains(&self, kind: &str) -> bool`, `iter()`
  - `pub const NAME_COLLISION: &str = "name_collision"; pub const UNPARSED_ARGUMENTS: &str = "unparsed_arguments";`
  - `pub(crate) struct MappedInput { pub input: RestrictedExpression, pub input_text: Option<String>, pub unparsed: bool }` and `pub(crate) fn map_input(input: &ToolInput) -> MappedInput`
  - `pub(crate) enum NumberClass { Long(i64), Decimal(String), Text(String) }` and `pub(crate) fn classify_number(text: &str) -> NumberClass`

- [ ] **Step 1: Add the feature.** In the kernel `Cargo.toml`:
  - `cedar-policy = { workspace = true, optional = true }`, `serde_json = { workspace = true, features = ["raw_value"], optional = true }`, `unicode-normalization = { version = "0.1.25", optional = true }`.
  - `[features] tool-policy = ["dep:cedar-policy", "dep:serde_json", "dep:unicode-normalization"]`. No default features.
  - `[package.metadata.docs.rs] features = ["tool-policy"]` (D2).
  - Add the module doc line to `lib.rs`: "`tool_policy` (feature `tool-policy`): the InvokeTool decision of ADR-0027 (e)."
  - Update the `rs/Cargo.toml` comment on `cedar-policy` ("only consumed by paigasus-iam-core + paigasus-iam"): it is now also used by the kernel under `tool-policy`, and never in the wasm or binding tree (D21).

- [ ] **Step 2: Write the failing tests** (unit tests in `normalise.rs` and `input.rs`).

```rust
#[test] fn trim_nfc_and_case_fold() {
    assert_eq!(normalise("  Read_File\t"), "read_file");
    assert_eq!(normalise("cafe\u{301}"), normalise("caf\u{e9}"));      // decomposed == composed
    assert_eq!(normalise("CAF\u{c9}"), "caf\u{e9}");
}
#[test] fn nfc_runs_again_after_lowercasing() {
    // U+0130 lowercases to "i\u{307}"; U+1E9E lowercases to U+00DF. Assert the result is in NFC:
    let s = normalise("\u{212b}");                                      // ANGSTROM SIGN
    assert_eq!(s, "\u{e5}");
    assert!(unicode_normalization::is_nfc(&normalise("\u{130}X\u{301}")));
}
#[test] fn normalise_matches_what_a_policy_literal_matches() {               // Review Focus 1
    for raw in [" Read_File", "READ_FILE", "read_file\u{a0}"] { assert_eq!(normalise(raw), "read_file"); }
}
#[test] fn integers_in_any_spelling_are_long() {
    for t in ["5", "5.0", "5e0", "0.5e1", "50e-1"] { assert_eq!(classify_number(t), NumberClass::Long(5)); }
    assert_eq!(classify_number("-0"), NumberClass::Long(0));
    assert_eq!(classify_number("9223372036854775807"), NumberClass::Long(i64::MAX));
}
#[test] fn decimal_edges() {
    assert_eq!(classify_number("1.5"), NumberClass::Decimal("1.5".into()));
    assert_eq!(classify_number("0.1234"), NumberClass::Decimal("0.1234".into()));
    assert_eq!(classify_number("0.12345"), NumberClass::Text("0.12345".into()));        // 5 fraction digits
    assert_eq!(classify_number("922337203685477.5807"), NumberClass::Decimal("922337203685477.5807".into()));
    assert_eq!(classify_number("922337203685477.5808"), NumberClass::Text("922337203685477.5808".into()));
    assert_eq!(classify_number("-922337203685477.5808"), NumberClass::Decimal("-922337203685477.5808".into()));
}
#[test] fn a_big_integer_keeps_its_text() {
    assert_eq!(classify_number("12345678901234567890"), NumberClass::Text("12345678901234567890".into()));
    assert_eq!(classify_number("1e400"), NumberClass::Text("1e400".into()));
    assert_eq!(classify_number("0e999999999"), NumberClass::Long(0));
}
#[test] fn every_mapping_row() { /* object→record (keys unchanged), string, bool, array→set, null key left out */ }
#[test] fn entity_and_extension_escapes_stay_records() {
    // {"__entity":{"type":"Pgs::Iam::User","id":"x"}} and {"__extn":{"fn":"decimal","arg":"1.0"}}
    // map to records: a policy `context.input.__entity.type == "Pgs::Iam::User"` evaluates true,
    // and `context.input.__entity is Pgs::Iam::User` is false. Use a one-policy PolicySet in the test.
}
#[test] fn depth_seventeen_becomes_json_text() { /* the value at depth 17 is a string of its compact JSON text */ }
#[test] fn non_object_json_is_unparsed() {
    for raw in ["[1]", "\"x\"", "{", "{\"a\":1,\"a\":2}", "{\"a\":{\"b\":1,\"b\":2}}"] {
        let m = map_input(&ToolInput::Json(raw.into()));
        assert!(m.unparsed); assert_eq!(m.input_text, None);
    }
    let m = map_input(&ToolInput::Text("*** Begin Patch".into()));
    assert!(m.unparsed); assert_eq!(m.input_text.as_deref(), Some("*** Begin Patch"));
}
```

- [ ] **Step 3: Run the tests.** `cd rs && cargo nextest run -p paigasus-kernel --features tool-policy`. Expected: FAIL (the module does not exist).

- [ ] **Step 4: Implement.**
  - `normalise` = `nfc(lowercase(nfc(trim(name))))` (§ 6.1.3). `trim` is `str::trim` (Unicode white space).
  - `classify_number`: parse the JSON number grammar into sign, digit string and a base-10 exponent, with no float. Strip leading zeros. The value is an integer when every digit after the decimal point is zero. `Long` when the integer fits `i64`. `Decimal` when at most 4 significant fraction digits remain and the scaled value `v * 10^4` fits `i64` (Cedar `decimal` range). Format a `Decimal` as `<int>.<frac>` with at least one fraction digit. Otherwise `Text(original)`. Bound the exponent work: an exponent whose magnitude is more than 40 above the digit count is `Text`, unless every digit is zero (`Long(0)`).
  - `map_input`: parse with `serde_json::from_str::<&RawValue>`, then walk each raw value by its first byte. Objects and arrays are read as `Vec<(String, &RawValue)>` / `Vec<&RawValue>` through a small visitor that refuses a duplicate key. Build only with `RestrictedExpression::new_record`, `new_set`, `new_string`, `new_long`, `new_bool`, `new_decimal` (§ 6.1.4). Never call `Context::from_json_value`. Depth limit 16.

- [ ] **Step 5: Run the tests.** Same command. Expected: PASS. Also `cargo tree -p paigasus-kernel -e normal | grep -c cedar` prints `0` (the feature is off by default).

- [ ] **Step 6: Commit.** `feat(rs): add the kernel tool-policy types, name normalising and argument mapping (SMA-752)`.

---

### Task 2: Kernel compile

**Files:**
- Create: `rs/crates/libs/paigasus-kernel/src/tool_policy/compile.rs`
- Modify: `rs/crates/libs/paigasus-kernel/src/tool_policy/mod.rs` (re-exports)

**Interfaces:**
- Consumes: `normalise` (Task 1).
- Produces:
  - `pub struct FloorPolicies` (opaque; `pub fn len(&self) -> usize`, `pub fn is_empty(&self) -> bool`)
  - `pub struct ToolPolicies` (opaque; `pub fn floor(&self) -> &Arc<FloorPolicies>`)
  - `impl ToolPolicies { pub fn compile_floor(floor: &str) -> Result<FloorPolicies, CompileError>; pub fn compile(floor: Arc<FloorPolicies>, tenant: &str, catalog: &[String]) -> Result<ToolPolicies, CompileError>; }`
  - `#[non_exhaustive] #[derive(Debug, thiserror::Error, PartialEq, Eq)] pub enum CompileError { Parse(String), MissingId { origin: PolicyOrigin }, DuplicateId(String), Template(String), FloorPermit(String), ActionNotNormal(String), CatalogCollision(String), ReservedCatalogName(String) }`
  - `#[non_exhaustive] pub enum PolicyOrigin { Floor, Tenant }`; `pub struct PolicyRef { pub set: PolicyOrigin, pub id: String }`
  - `pub(crate) const ACTION_TYPE: &str = "Pgs::Gateway::Action"; pub(crate) const GROUP_INVOKE: &str = "InvokeTool"; pub(crate) const GROUP_UNREGISTERED: &str = "Unregistered"; pub(crate) const RESOURCE_TYPE: &str = "Pgs::Iam::Organization";`
  - `pub(crate) fn action_entity(policies: &ToolPolicies, normalised: &str) -> (EntityUid, Entities)` — the call's action with parent `InvokeTool` (catalog) or `Unregistered` (D22).

- [ ] **Step 1: Write the failing tests** (unit tests in `compile.rs`).

```rust
#[test] fn a_floor_permit_is_a_compile_error() { assert!(matches!(ToolPolicies::compile_floor(r#"@id("p") permit(principal, action, resource);"#), Err(CompileError::FloorPermit(id)) if id == "p")); }
#[test] fn a_policy_without_id_is_a_compile_error() { /* floor and tenant: MissingId { origin } */ }
#[test] fn a_duplicate_id_in_one_set_is_a_compile_error() { /* DuplicateId("x"); the same id in floor and tenant is allowed: floor/x and tenant/x */ }
#[test] fn a_template_is_a_compile_error() { /* permit(principal == ?principal, action, resource) → Template */ }
#[test] fn a_non_normal_action_literal_is_a_compile_error() {
    // in the scope and inside a condition (`action == Pgs::Gateway::Action::"Send_Email"`)
    assert!(matches!(compile_tenant(r#"@id("a") permit(principal, action == Pgs::Gateway::Action::"Send_Email", resource);"#), Err(CompileError::ActionNotNormal(s)) if s == "Send_Email"));
}
#[test] fn the_group_ids_are_exempt_from_the_normal_form_rule() { /* action in Action::"InvokeTool" and Action::"Unregistered" compile */ }
#[test] fn catalog_rules() {
    assert!(matches!(compile_with_catalog(&["Read_File", "read_file"]), Err(CompileError::CatalogCollision(_))));
    assert!(matches!(compile_with_catalog(&["InvokeTool"]), Err(CompileError::ReservedCatalogName(_))));
    assert!(matches!(compile_with_catalog(&[" unregistered"]), Err(CompileError::ReservedCatalogName(_))));
}
#[test] fn catalog_and_unregistered_parents() { /* action_entity: "read_file" in catalog → parent InvokeTool; "x" → parent Unregistered */ }
#[test] fn merged_ids_carry_their_origin() { /* floor/<id> and tenant/<id> map back to PolicyRef { set, id } */ }
```

- [ ] **Step 2: Run.** `cargo nextest run -p paigasus-kernel --features tool-policy compile`. Expected: FAIL.

- [ ] **Step 3: Implement.** Parse each set with `PolicySet::from_str`. Read the `@id` annotation of each policy; re-key the merged set as `floor/<id>` and `tenant/<id>` (`Policy::new_id`). Walk `Policy::to_json` for every entity literal of type `Pgs::Gateway::Action` (scope and conditions); an id that is not `InvokeTool`, `Unregistered` or equal to its `normalise` is `ActionNotNormal`. Build the action `Entities` once at compile. Do not add the optimistic-set fields yet: Task 4 adds them with their first use.

- [ ] **Step 4: Run.** Same command. Expected: PASS.

- [ ] **Step 5: Commit.** `feat(rs): compile the floor and tenant tool-policy sets in the kernel (SMA-752)`.

---

### Task 3: Kernel `decide`

**Files:**
- Create: `rs/crates/libs/paigasus-kernel/src/tool_policy/decide.rs`
- Create: `rs/crates/libs/paigasus-kernel/tests/tool_policy_decide.rs` (`#![cfg(feature = "tool-policy")]`)

**Interfaces:**
- Consumes: Tasks 1 and 2.
- Produces:
  - `#[non_exhaustive] pub enum Effect { Allow, Deny }`
  - `#[non_exhaustive] pub struct Verdict { pub decision: Effect, pub determining: Vec<PolicyRef>, pub errored: Vec<PolicyRef>, pub errored_forbid: bool, pub floor_denied: bool }` (P-3)
  - `pub fn decide(call: &ToolInvocation, facts: &Facts, findings: &Findings, policies: &ToolPolicies) -> Verdict`
  - `pub(crate) fn context(call: Option<&ToolInvocation>, facts: &Facts, findings: &Findings) -> Context` (shared with Task 4; `None` means the optimistic context `{ input: {}, call: { declared: "yes" } }`)

- [ ] **Step 1: Write the failing tests** (integration test file; one helper `facts()` with principal `prn:pgs:iam:::user/0190a1e5-0000-7000-8000-000000000004`, org `0190a100-0000-7000-8000-0000000000aa`).

```rust
#[test] fn an_errored_forbid_denies() {
    // floor: @id("big") forbid(principal, action, resource) when { context.input.amount > 1000 };
    // tenant: @id("all") permit(principal, action, resource);
    let v = decide(&call("pay", r#"{"amount": 1500.5}"#), &facts(), &Findings::new(), &p);
    assert_eq!(v.decision, Effect::Deny); assert!(v.errored_forbid); assert!(v.floor_denied);
    assert_eq!(v.errored, vec![PolicyRef::new(PolicyOrigin::Floor, "big")]);
}
#[test] fn every_permit_errored_is_a_normal_deny() { /* errored_forbid false, floor_denied false */ }
#[test] fn a_floor_forbid_wins_over_a_tenant_permit() { /* determining == [floor/x], floor_denied */ }
#[test] fn a_floor_forbid_that_names_a_tool_applies_outside_the_catalog() { /* catalog [], forbid action == Action::"rm" */ }
#[test] fn unregistered_is_denied_by_default_and_allowed_by_a_permit() {
    // no permit → Deny, floor_denied false; tenant `permit(principal, action in Action::"Unregistered", resource)` → Allow;
    // with a floor forbid on that tool → Deny and floor_denied.
}
#[test] fn the_floor_alone_has_no_floor_denied_default_deny() { /* compile(floor, "", &[]): Deny, floor_denied false */ }
#[test] fn context_shape() {
    // tenant permits when context.request.ingress == "chat_completions" && context.call.declared == "yes"
    // && context.session.turn == 0 && context.findings.contains("name_collision") && principal is Pgs::Iam::User
    // && resource == Pgs::Iam::Organization::"0190a100-0000-7000-8000-0000000000aa"
}
#[test] fn an_argument_policy_on_a_path() { /* permit read_file when context.input.path like "/workspace/*": "/workspace/a" Allow, "/etc/passwd" Deny */ }
#[test] fn unparsed_arguments_reach_the_findings() { /* ToolInput::Text → context.findings.contains("unparsed_arguments") and context.input_text */ }
```

- [ ] **Step 2: Run.** `cargo nextest run -p paigasus-kernel --features tool-policy --test tool_policy_decide`. Expected: FAIL.

- [ ] **Step 3: Implement.** One `Authorizer::is_authorized` call on the merged set. Principal from `facts.principal`, no parents. Resource `Pgs::Iam::Organization::"<org>"`, no parents. Action from `action_entity`. `determining` from `diagnostics().reason()`, `errored` from `diagnostics().errors()`, mapped back through the merged ids. If an errored policy is a `forbid`, set `decision = Deny` and `errored_forbid = true` (D26). `floor_denied` per P-3. No I/O, clock, random numbers or global state.

- [ ] **Step 4: Run.** Same command. Expected: PASS.

- [ ] **Step 5: Commit.** `feat(rs): add the kernel tool-policy decide function (SMA-752)`.

---

### Task 4: Kernel optimistic set, `decide_tool` and the soundness property

**Files:**
- Create: `rs/crates/libs/paigasus-kernel/src/tool_policy/optimistic.rs`
- Modify: `rs/crates/libs/paigasus-kernel/src/tool_policy/compile.rs` (build the optimistic sets at compile)
- Create: `rs/crates/libs/paigasus-kernel/tests/tool_policy_props.rs` (`#![cfg(feature = "tool-policy")]`)

**Interfaces:**
- Consumes: Tasks 2 and 3.
- Produces:
  - `#[non_exhaustive] pub enum ToolVerdict { DeniedForAllInputs { by_floor: bool }, MayAllow }`
  - `pub fn decide_tool(name: &str, facts: &Facts, findings: &Findings, policies: &ToolPolicies) -> ToolVerdict`
  - `pub(crate) enum ContextAttr { Input, InputText, Findings, Call, Session }` and `pub(crate) const DROP_READS: [ContextAttr; 5]`
  - `pub(crate) fn optimistic(merged: &PolicySet, drop_reads: &[ContextAttr], strip_permit_conditions: bool) -> PolicySet` (the parameters exist for the mutation test only; production passes `&DROP_READS, true`)

- [ ] **Step 1: Write the failing unit tests** (in `optimistic.rs`).

```rust
#[test] fn a_forbid_that_reads_only_request_facts_stays() { /* forbid when context.request.model == "m" → kept */ }
#[test] fn each_reading_form_drops_a_forbid() {
    for cond in ["context.input.x == 1", "context has input", "context.input has x", "context.input_text like \"*\"",
                 "context.findings.contains(\"name_collision\")", "context.call.declared == \"no\"", "context.session.turn > 0",
                 "context == {}", "context[\"input\"] == {}"] { assert!(dropped(cond), "{cond}"); }
}
#[test] fn an_unknown_json_shape_drops_the_forbid() { /* feed the walker a hand-made EST node it does not know → reads all */ }
#[test] fn decide_tool_attribution() {
    // floor forbid on Action::"rm" → DeniedForAllInputs { by_floor: true }
    // no tenant permit for "x" → DeniedForAllInputs { by_floor: false }
    // tenant permit "read_file" when context.input.path like "/workspace/*" → MayAllow (permit loses its condition)
    // floor D15 collision forbid + findings {name_collision} + tenant permit-all → MayAllow (the forbid reads findings)
}
#[test] fn removing_any_drop_rule_breaks_soundness() {
    // Mutation proof (§ 7.1). For each ContextAttr a, build optimistic(merged, DROP_READS minus a, true) and a
    // hand-written counterexample (policy, input, findings, declared, session) where the mutated decide_tool says
    // DeniedForAllInputs and decide says Allow. Also strip_permit_conditions = false has a counterexample.
}
```

- [ ] **Step 2: Write the failing property test** (`tests/tool_policy_props.rs`). A proptest generator builds a floor of 0-4 forbids and a tenant set of 0-6 permits and forbids. Each condition is drawn from: `has`, `like`, `==`, `<`, `.contains(...)`, a set literal, a record literal, `decimal("…").lessThan(...)`, `unless`, `action in Action::"InvokeTool"`, `action in Action::"Unregistered"`, `context.call.declared`, `context.findings`, `context.request.*` and `context.session.*` (§ 7.1). It also draws the tool name, the catalog, the input JSON, the findings, `declared` and the session.

```rust
proptest! {
    #![proptest_config(ProptestConfig { cases: 2048, ..ProptestConfig::default() })]
    #[test]
    fn denied_for_all_inputs_implies_deny(case in case_strategy()) {
        if let ToolVerdict::DeniedForAllInputs { by_floor } = decide_tool(&case.name, &case.facts, &case.request_findings, &case.policies) {
            let v = decide(&case.call, &case.facts_with_session, &case.call_findings, &case.policies);
            prop_assert_eq!(v.decision, Effect::Deny);
            if by_floor { prop_assert!(v.floor_denied); }
        }
    }
}
```

  The facts used by `decide` share principal, org and `context.request` with those of `decide_tool`, and may differ in session, findings, input and `declared` (§ 6.1.6 soundness).

- [ ] **Step 3: Run.** `cargo nextest run -p paigasus-kernel --features tool-policy optimistic --test tool_policy_props`. Expected: FAIL.

- [ ] **Step 4: Implement** per § 6.1.6. Evaluate the optimistic set with `context(None, …)` and Cedar's normal handling (an errored forbid is skipped, not turned into a deny). `by_floor` is true when a floor policy is determining.

- [ ] **Step 5: Run.** Same command. Expected: PASS, with 2048 cases.

- [ ] **Step 6: Red-first proof.** Delete the line in `DROP_READS` for `ContextAttr::Input` (only that line). Run the property test. Expected: FAIL with a shrunk counterexample. Restore the line by hand. Record the counterexample in your report.

- [ ] **Step 7: Commit.** `feat(rs): add decide_tool and its soundness property to the kernel (SMA-752)`.

---

### Task 5: The three error reasons

The registry codes must exist before gateway-core spells them (Task 6 has the membership test), so this task comes before the other gateway-core work.

**Files:**
- Modify: `contracts/proto/paigasus/common/v1/error.proto` (after value 314)
- Regenerate: `rs/crates/libs/paigasus-proto/src/generated/paigasus/common/v1/paigasus.common.v1.rs`, `py/packages/paigasus-proto/src/paigasus_proto/generated/paigasus/common/v1/__init__.py`, `ts/packages/paigasus-proto/src/generated/paigasus/common/v1/error_pb.ts`
- Modify: `rs/crates/libs/paigasus-proto/src/error.rs` (`EXPECTED_REASONS`)

- [ ] **Step 1: Write the failing test.** Add the three strings to `EXPECTED_REASONS` in `paigasus-proto/src/error.rs`: `"tool-call-denied"`, `"tool-result-blocked"`, `"tool-policy-unavailable"`.
- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-proto`. Expected: FAIL (the registry has no such values).
- [ ] **Step 3: Implement.** Append, under the new banner `// ---- Gateway: tool decisions (SMA-752) ----`, with one comment line each (the wire string, what it means, that it appears in decision reports and notices, never as an HTTP error, D11):
  `ERROR_REASON_TOOL_CALL_DENIED = 315;`, `ERROR_REASON_TOOL_RESULT_BLOCKED = 316;`, `ERROR_REASON_TOOL_POLICY_UNAVAILABLE = 317;` (D10). Then `moon run contracts:fmt` (or `buf format -w` in `contracts/`) and `moon run contracts:generate`.
- [ ] **Step 4: Run.** `cargo nextest run -p paigasus-proto` and `moon run repo:error-code-single-site repo:breaking contracts:lint`. Expected: PASS. `git status` shows exactly the five files above. If `contracts:generate` deletes `error_details_pb.ts` (BSR rate limit), restore it and re-run.
- [ ] **Step 5: Commit.** `feat(contracts): add the three tool-decision error reasons (SMA-752)`.

---

### Task 6: Gateway-core `Observe`, `PolicyUnavailable` and report v2

**Files:**
- Modify: `rs/crates/libs/paigasus-gateway-core/Cargo.toml` (dev-dependency `paigasus-proto = { workspace = true }`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/types.rs`, `src/report.rs`, `src/interceptor.rs` (`apply_decision`), `src/body.rs` (`report`), `src/lib.rs`
- Modify: `rs/crates/libs/paigasus-gateway-core/moon.yml` (no `dependsOn` for a dev-dependency unless A6 asks; run `repo:affected-smoke` to see)
- Modify: `ci/error-registry/check.py` (one `emits` row)

**Interfaces:**
- Produces:
  - `Decision::Observe { would: BlockCode }` (D28); `is_allow()` is true for it.
  - `BlockCode::PolicyUnavailable` with kebab `"policy-unavailable"`.
  - `ReportEntry` gains `pub enforced: Option<bool>` and `pub reason: Option<&'static str>`; `to_json` writes `"v": 2`, `"side": "response"`, `enforced`, `reason` (P-11).
  - `pub fn reason_label(code: BlockCode) -> Option<&'static str>`
  - `#[non_exhaustive] pub enum RequestAction { RemoveDefinition, ReplaceResult, ResetToolChoice }` (`as_str`: `remove-definition`, `replace-result`, `reset-tool-choice`)
  - `#[non_exhaustive] pub enum RequestReason { ToolCallDenied, ToolResultBlocked }` with `as_str()`
  - `#[non_exhaustive] pub struct RequestEntry { pub tool: String, pub action: RequestAction, pub enforced: bool, pub reason: RequestReason }` with `new` and `to_json()`
  - `pub fn report_header(calls: &[ReportEntry], requests: &[RequestEntry]) -> String` (replaces the crate-private `header_value`)
  - `pub fn request_comments(requests: &[RequestEntry]) -> Vec<Bytes>`
  - `BodyCalls::report(&self, decisions: &[Decision], basis: Basis, requests: &[RequestEntry]) -> String` (signature change; update the gateway's one call site in `interception.rs` to pass `&[]` in this task)

- [ ] **Step 1: Write the failing tests.**

```rust
// types.rs
#[test] fn observe_is_an_allow() { assert!(Decision::Observe { would: BlockCode::Policy }.is_allow()); }
// extend block_codes_have_stable_kebab_labels with PolicyUnavailable → "policy-unavailable"
// report.rs
#[test] fn v2_call_entries() {
    // Block{Policy} → {"v":2,"side":"response","decision":"block","code":"policy","enforced":true,"reason":"tool-call-denied"}
    // Observe{Policy} → decision "allow", code "policy", enforced false, reason "tool-call-denied"
    // Block{PolicyUnavailable} → reason "tool-policy-unavailable"; Block{HoldLimit} → reason null; Allow → enforced null
}
#[test] fn request_entries_go_after_call_entries_and_are_cut_first() {
    let h = report_header(&[call_entry("a")], &many_request_entries(60));
    let v: Vec<serde_json::Value> = serde_json::from_str(&h).unwrap();
    assert!(h.len() <= 2048);
    assert_eq!(v[0]["side"], "response");
    let last = v.last().unwrap();
    assert_eq!(last["side"], "request"); assert!(last["omitted"].as_u64().unwrap() > 0);
}
#[test] fn request_comments_are_sse_comments() { /* each starts with ": paigasus-decision {" and ends "\n\n" */ }
#[test] fn every_report_reason_is_declared_in_the_canonical_registry() {
    for r in ["tool-call-denied", "tool-result-blocked", "tool-policy-unavailable"] {
        assert!(paigasus_proto::ErrorReason::from_wire_reason(r).is_some(), "{r}");
    }
    // and every value reason_label / RequestReason::as_str can return is in that list
}
// interceptor.rs (unit test in the existing module)
#[test] fn observe_releases_the_call_and_reports_not_enforced() { /* decided(hold, Ok(vec![Observe{Policy}])) emits the call unchanged and a comment with "enforced":false */ }
// body.rs
#[test] fn observe_keeps_the_body_bytes() { assert!(b.apply(&[Decision::Observe { would: BlockCode::Policy }]).is_none()); }
```

  Check the exact path of `ErrorReason::from_wire_reason` in `paigasus-proto` (the gateway's `error.rs` test uses it) and use the same path.

- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-gateway-core`. Expected: FAIL.
- [ ] **Step 3: Implement.** `Observe` releases like `Allow` in `Verdict` handling (`protocol.rs` `blocks_any`/`block_code` already use `is_allow`; check every `match` on `Decision` compiles and treats `Observe` as allow). The interceptor metric label for `Observe` stays `"allow"`. Add the MANIFEST row: `("rs/crates/libs/paigasus-gateway-core/src/report.rs", "emits", "every_report_reason_is_declared_in_the_canonical_registry", "decision-report reasons (SMA-752 D10)")`.
- [ ] **Step 4: Run.** `cargo nextest run -p paigasus-gateway-core -p paigasus-gateway` and `moon run repo:error-code-single-site`. Expected: PASS. The existing gateway report assertions that read `"v":1` must be updated to `2` in this task; list each one you change in your report.
- [ ] **Step 5: Commit.** `feat(rs): add Observe decisions and the v2 decision report to gateway-core (SMA-752)`.

---

### Task 7: Gateway-core `ToolChoice`, kernel mapping and `name_collision`

**Files:**
- Modify: `rs/crates/libs/paigasus-gateway-core/Cargo.toml` (`paigasus-kernel = { workspace = true, features = ["tool-policy"] }`, `uuid = { workspace = true }`)
- Modify: `rs/crates/libs/paigasus-gateway-core/moon.yml` (`dependsOn: ['paigasus-kernel-rs']`; `upstreams`: the kernel `src/**/*` and `Cargo.toml`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/turn.rs` (`tool_choice`)
- Create: `rs/crates/libs/paigasus-gateway-core/src/policy/mod.rs`, `src/policy/mapping.rs`
- Modify: `src/lib.rs`

**Interfaces:**
- Consumes: kernel `ToolInvocation`, `ToolInput`, `Declared`, `normalise`, `Findings`, `NAME_COLLISION` (Task 1).
- Produces:
  - `TurnFacts.tool_choice: ToolChoice` (default `Auto`)
  - `#[non_exhaustive] pub enum ToolChoice { Auto, None, Required, Named(String), AllowedTools(Vec<String>), Unknown }` — names are display names (`ns.name` for a Responses namespace tool)
  - `pub fn ingress(format: Format) -> &'static str` (`"chat_completions"`, `"anthropic_messages"`, `"responses"`)
  - `pub fn name_collisions(defs: &[ToolDefinition]) -> BTreeSet<String>` (normalised display names in a group of 2+)
  - `pub fn invocation(call: &ToolCall, turn: &ToolTurn) -> ToolInvocation`
  - `pub fn call_findings(call: &ToolCall, collisions: &BTreeSet<String>) -> Findings`
  - `ToolDefinition::display_name(&self) -> String` (same rule as `ToolCall::display_name`)

- [ ] **Step 1: Write the failing tests.**

```rust
// turn.rs — the ToolChoice parse rules (§ 8, the builder's choice):
#[test] fn chat_tool_choice() {
    // absent/"auto" → Auto; "none" → None; "required" → Required;
    // {"type":"function","function":{"name":"f"}} → Named("f"); {"type":"custom","custom":{"name":"c"}} → Named("c");
    // {"type":"allowed_tools","allowed_tools":{"mode":"auto","tools":[{"type":"function","function":{"name":"a"}}]}} → AllowedTools(["a"]);
    // legacy function_call "auto"/"none"/{"name":"f"} → Auto/None/Named("f"); anything else → Unknown
}
#[test] fn responses_tool_choice() {
    // "auto"/"none"/"required"; {"type":"function","name":"f"} → Named("f"); {"type":"custom","name":"c"};
    // {"type":"allowed_tools","mode":"auto","tools":[{"type":"function","name":"a"}]} → AllowedTools(["a"]);
    // {"type":"local_shell"} etc. (a built-in) → Named("local_shell"); {"type":"mcp",...} → Unknown
}
#[test] fn anthropic_tool_choice() { /* {"type":"auto"|"any"|"none"|"tool","name":"t"} → Auto | Required | None | Named("t"); else Unknown */ }
// policy/mapping.rs
#[test] fn collisions_group_by_normalised_display_name() {
    // "Read_File" and "read_file " collide; "ns.read_file" and "read_file" do not (Review Focus 4)
}
#[test] fn a_call_name_is_normalised_before_decide() { /* invocation(call "READ_FILE ").name stays raw; the kernel normalises; decide with an Action::"read_file" forbid denies */ }
#[test] fn declared_follows_the_turn() { /* Yes when a definition has the call's normalised name; No when not; Unknown when definitions_complete is false or the turn is Unavailable */ }
#[test] fn input_kind_follows_the_call_kind() { /* Function, LegacyFunctionCall, AnthropicToolUse → Json; Custom, ApplyPatch, Other → Text; Computer/LocalShell/Shell/ToolSearch/McpApprovalRequest → Json */ }
#[test] fn collision_finding_per_call() { /* call_findings adds name_collision only for a call whose normalised name is in the set */ }
```

- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-gateway-core`. Expected: FAIL.
- [ ] **Step 3: Implement.** Parse `tool_choice` inside the existing `openai`, `responses` and `anthropic` functions of `turn.rs`; a shape the parser does not know is `ToolChoice::Unknown`, never `Malformed`. `name_collisions` uses the visible definitions only (D23).
- [ ] **Step 4: Run.** `cargo nextest run -p paigasus-gateway-core` and `moon run repo:affected-smoke` under `/bin/bash`. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): map gateway-core calls into the kernel tool-policy types (SMA-752)`.

---

### Task 8: Gateway-core policy snapshot and call decisions

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/src/policy/snapshot.rs`, `src/policy/calls.rs`
- Modify: `rs/crates/libs/paigasus-gateway-core/src/decider.rs` (`DecisionContext.policy`)

**Interfaces:**
- Consumes: kernel `decide`, `Verdict`, `ToolPolicies`, `Facts` (Tasks 2-3); Task 7 mapping.
- Produces:
  - `#[non_exhaustive] pub enum TenantMode { Off, LogOnly, Enforce }` with `as_str()` (`off`, `log_only`, `enforce`)
  - `#[non_exhaustive] pub enum OnUnavailable { Open, Closed }`
  - `#[non_exhaustive] pub enum TenantPart { Evaluated, Off, Unavailable }`
  - `#[non_exhaustive] pub enum PolicySet { Available { policies: Arc<ToolPolicies>, tenant: TenantPart, facts: Facts }, Unavailable }`
  - `#[non_exhaustive] pub struct PolicySnapshot { pub set: PolicySet, pub mode: TenantMode, pub on_unavailable: OnUnavailable }` with `new`
  - `DecisionContext` gains `pub policy: Option<Arc<PolicySnapshot>>` and `pub fn with_policy(self, snapshot: Arc<PolicySnapshot>) -> Self`. `new` keeps its signature and sets `None`.
  - `#[non_exhaustive] pub enum PolicyBasis { Policy, ErroredForbid, Unavailable }` with `as_str()`
  - `#[non_exhaustive] pub struct CallOutcome { pub decision: Decision, pub basis: PolicyBasis, pub verdict: Option<Verdict> }`
  - `pub fn decide_calls(snapshot: &PolicySnapshot, turn: &ToolTurn, calls: &[ToolCall]) -> Vec<CallOutcome>`

- [ ] **Step 1: Write the failing tests** (`policy/calls.rs`). Use one table test: rows `(set, tenant part, mode, on_unavailable, verdict shape) → decision`.

```rust
#[test] fn the_section_6_4_table() {
    // Unavailable, any mode incl. Off, Closed → Block{PolicyUnavailable}; Open → Observe{PolicyUnavailable}   (P-6)
    // Available, floor_denied, any mode incl. Off and LogOnly → Block{Policy}                              (D33)
    // Available+Off, tenant-only deny (default deny of the floor-alone set) → Allow                        (P-4)
    // Available+Unavailable, not floor_denied: LogOnly → Observe{PolicyUnavailable};
    //     Enforce+Closed → Block{PolicyUnavailable}; Enforce+Open → Observe{PolicyUnavailable}
    // Available+Evaluated, Allow → Allow; Deny: LogOnly → Observe{Policy}; Enforce → Block{Policy}
}
#[test] fn an_errored_forbid_has_its_own_basis() { /* basis ErroredForbid, decision Block{Policy} */ }
#[test] fn one_outcome_per_call_in_order() { /* 3 calls → 3 outcomes, matching names */ }
#[test] fn the_shipped_collision_floor_blocks_both_colliding_calls() {
    // floor = D15 text; two definitions "send" and "Send"; both calls Block{Policy} in tenant mode Off
}
```

- [ ] **Step 2: Run.** `cargo nextest run -p paigasus-gateway-core policy`. Expected: FAIL.
- [ ] **Step 3: Implement** the table. Findings per call: `call_findings` plus `unparsed_arguments` comes from the kernel itself.
- [ ] **Step 4: Run.** Same. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): decide tool calls from a policy snapshot in gateway-core (SMA-752)`.

---

### Task 9: Gateway-core request plan

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/src/policy/plan.rs`

**Interfaces:**
- Consumes: kernel `decide_tool`, `ToolVerdict` (Task 4); Tasks 6-8.
- Produces:
  - `#[non_exhaustive] pub enum ResultTarget { CallId(String), Name(String) }`
  - `#[non_exhaustive] pub enum ChoiceRewrite { Keep, Auto, Allowed(Vec<String>) }`
  - `#[non_exhaustive] pub struct PlanItem<T> { pub target: T, pub enforced: bool }`
  - `#[non_exhaustive] pub struct RequestPlan { pub removed: Vec<PlanItem<String>>, pub replaced: Vec<PlanItem<ResultTarget>>, pub choice: ChoiceRewrite, pub refuse: bool, pub entries: Vec<RequestEntry> }`
  - `RequestPlan::is_noop(&self) -> bool` (nothing enforced changes)
  - `pub fn plan_request(snapshot: &PolicySnapshot, format: Format, turn: &TurnFacts) -> RequestPlan`

- [ ] **Step 1: Write the failing tests.**

```rust
#[test] fn unavailable_plans_nothing() { /* PolicySet::Unavailable and TenantPart::Unavailable → no items, no entries (§ 6.4 last paragraph) */ }
#[test] fn a_tool_denied_for_every_input_is_removed() { /* enforce, no permit for "x" → removed ["x"] enforced; entry remove-definition, reason tool-call-denied */ }
#[test] fn an_input_reading_permit_keeps_the_definition() { /* permit read_file when context.input.path like ... → no removal */ }
#[test] fn log_only_plans_but_does_not_enforce() { /* enforced false on every item (D25); is_noop() */ }
#[test] fn a_floor_removal_is_enforced_in_every_mode() { /* tenant Off and LogOnly, floor forbid on "rm" → enforced true (D33) */ }
#[test] fn an_empty_catalog_in_enforce_removes_every_definition() { /* D22 */ }
#[test] fn colliding_definitions_are_not_removed() { /* shipped floor + tenant permit-all, "send"/"Send" → no removal */ }
#[test] fn hosted_responses_tools_are_never_removed() { /* web_search with an empty catalog → kept; local_shell → removed (P-15) */ }
#[test] fn result_replacement_by_call_id_and_by_earlier_call() {
    // Chat role:"tool" with tool_call_id "c1", earlier assistant call c1 named "x" (denied) → ReplaceResult CallId("c1")
    // two earlier calls with id "c1", names "ok" and "x" → replaced (any denied)
    // no earlier call with that id → not replaced
    // legacy role:"function" name "x" → Name("x"); Anthropic tool_result tool_use_id; Responses call_id; mcp_approval_response approval_request_id
    // entry action replace-result, reason tool-result-blocked
}
#[test] fn tool_choice_after_removals() {
    // Named(n) with n removed → Auto; AllowedTools([a, b]) with a removed → Allowed([b]); with both removed → Auto;
    // Unknown + one enforced removal → refuse; Unknown + only log-only removals → no refuse; each reset adds reset-tool-choice
}
#[test] fn a_log_only_reset_is_reported_once() { /* P-9: floor removes "a" (enforced), tenant log-only would remove "b"; Named("b") → choice Keep, one reset entry enforced false */ }
```

- [ ] **Step 2: Run.** `cargo nextest run -p paigasus-gateway-core plan`. Expected: FAIL.
- [ ] **Step 3: Implement** per § 6.5.1 steps 1-4 and P-7, P-9. Request-level findings for `decide_tool(name)`: `{name_collision}` when the name is in `name_collisions`, else empty. `enforced` for an item = `by_floor || mode == Enforce`. With `TenantPart::Off`, only a `by_floor` verdict plans an item (P-4).
- [ ] **Step 4: Run.** Same. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): plan the request-side tool-policy actions in gateway-core (SMA-752)`.

---

### Task 10: Gateway-core `rewrite_request`, fixtures and the not-measured list

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/src/policy/rewrite.rs`, `src/policy/raw.rs` (the `RawValue` object and array splitter)
- Modify: `rs/crates/libs/paigasus-gateway-core/Cargo.toml` (`serde_json` features add `"raw_value"`)
- Create: `rs/crates/libs/paigasus-gateway-core/tests/request_rewrite.rs`
- Create: `rs/crates/libs/paigasus-gateway-core/tests/fixtures/request_rewrite/<case>.{in,out}.json` (one pair per case below)
- Create: `rs/crates/libs/paigasus-gateway-core/tests/fixtures/request_rewrite/README.md` (the not-measured table)

**Interfaces:**
- Consumes: `RequestPlan` (Task 9), `sanitize_name` and the notice rules (`report.rs`).
- Produces:
  - `pub fn rewrite_request(format: Format, body: &[u8], plan: &RequestPlan) -> Option<Bytes>` (pure; `None` when `plan.is_noop()`)
  - `pub fn result_notice(tool: &str, reason: RequestReason) -> String` — fixed text: `[paigasus] tool result for "<sanitized>" was replaced (<reason>).`

- [ ] **Step 1: Write the fixtures and the failing tests.** Each fixture pair is built from the provider docs (OpenAI Chat, OpenAI Responses, Anthropic Messages API references), not from a live call. Cases:
  - chat: remove one of two tools; remove the last tool (drops `tools`, `tool_choice`, `parallel_tool_calls`); legacy `functions` last removal (drops `functions`, `function_call`); `Named` reset to `"auto"`; `allowed_tools` narrowed; `role:"tool"` content replaced; legacy `role:"function"` content replaced.
  - responses: remove a function tool; remove the last tool; remove an inner namespace tool and drop the empty namespace; each output type of § 6.5.2 replaced in `output`; `mcp_approval_response` changed to `"approve": false` with `"reason"` set to the notice.
  - anthropic: remove a tool; last tool with no `tool_use`/`tool_result` blocks (drops `tools`, `tool_choice`); last tool with a `tool_result` block in the messages (the definition stays); `tool_result` content replaced and `is_error: true`.

```rust
#[test] fn every_fixture_rewrites_to_its_expected_bytes() { /* for each case: parse_request → plan from a fixture snapshot → rewrite → equal to .out.json bytes */ }
#[test] fn a_noop_plan_returns_none() { assert!(rewrite_request(Format::OpenAiChat, body, &noop).is_none()); }
#[test] fn untouched_members_keep_their_bytes() {                         // Review Focus 2
    // body with "seed": 12345678901234567890, "top_p": 1.50, "logit_scale": 1e400, a key spelled "model"
    // and odd white space; after a removal every member except "tools" is byte-equal to the input.
}
#[test] fn a_namespace_removal_keeps_the_flat_tool_and_drops_an_empty_namespace() { /* Review Focus 4 */ }
#[test] fn the_output_parses_back_to_the_planned_turn() { /* parse_request(rewritten) has no removed definition and the new tool_choice */ }
#[test] fn only_enforced_items_are_applied() { /* a plan with one enforced and one log-only removal changes only the enforced one */ }
```

  `README.md` lists every case above with the columns `case`, `fixture`, `source (doc page)`, `measured against the real upstream`. Every row says `not measured (no API key in this change)`. The PR body quotes this table (§ 6.5.1).

- [ ] **Step 2: Run.** `cargo nextest run -p paigasus-gateway-core --test request_rewrite`. Expected: FAIL.
- [ ] **Step 3: Implement** per P-8. Read the top object as ordered `(key, &RawValue)` pairs. Re-serialise only a changed member, and only its changed elements (a replaced message item keeps every other member's bytes). Write the result in the original member order.
- [ ] **Step 4: Run.** Same, and `cargo nextest run -p paigasus-gateway-core`. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): rewrite requests for the tool-policy plan in gateway-core (SMA-752)`.

---

### Task 11: The `[tool_policy]` config table

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/config.rs`
- Modify: `rs/crates/services/paigasus-gateway/gateway.toml.example` (a commented `[tool_policy]` block, § 6.4)

**Interfaces:**
- Consumes: `TenantMode`, `OnUnavailable` (Task 8).
- Produces:
  - `GatewayConfig.tool_policy: ToolPolicyConfig` (`#[serde(default)]`, not in the `Defaults` layer)
  - `#[serde(default, deny_unknown_fields)] pub struct ToolPolicyConfig { pub enabled: bool, pub default_mode: TenantModeConfig, pub on_policy_unavailable: Option<OnUnavailableConfig>, pub refresh_secs: u64, pub max_stale_secs: u64, pub idle_evict_secs: u64, pub load_timeout_ms: u64, pub source: Option<PolicySourceConfig>, pub nats: Option<ToolPolicyNatsConfig>, pub org: Vec<OrgToolPolicyConfig> }` (defaults: false, off, None, 30, 300, 3600, 1000)
  - `#[serde(rename_all = "snake_case")] pub enum TenantModeConfig { Off, LogOnly, Enforce }`; `#[serde(rename_all = "lowercase")] pub enum OnUnavailableConfig { Open, Closed }`
  - `#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)] pub enum PolicySourceConfig { File { dir: String } }`
  - `#[serde(deny_unknown_fields)] pub struct ToolPolicyNatsConfig { pub url: String, pub creds_file: Option<String>, pub ca_file: Option<String> }` (`url` skipped on `Serialize`)
  - `#[serde(deny_unknown_fields)] pub struct OrgToolPolicyConfig { pub id: String, pub mode: TenantModeConfig, #[serde(default)] pub on_policy_unavailable: Option<OnUnavailableConfig> }` (D34)
  - `ToolPolicyConfig::validate(&self, ti: &ToolInterceptionConfig) -> Result<(), String>`, called from `GatewayConfig::validate`
  - `ToolPolicyConfig::tenant(&self, org: Uuid) -> (TenantMode, OnUnavailable)` (built once into a map by `settings()`)
  - `pub struct ToolPolicySettings { … }` and `ToolPolicyConfig::settings(&self) -> ToolPolicySettings` (durations, per-org map keyed by `Uuid`, defaults)

- [ ] **Step 1: Write the failing tests** (in `config.rs`, figment `Jail`, the existing `ti_toml` style).

```rust
#[test] fn tool_policy_is_off_by_default() { /* !enabled, refresh 30, max_stale 300, idle 3600, load_timeout_ms 1000, default_mode Off */ }
#[test] fn enabled_without_on_policy_unavailable_fails() { /* message names "tool_policy.on_policy_unavailable" (D9) */ }
#[test] fn enabled_without_tool_interception_fails() { /* message names "tool_interception.enabled" */ }
#[test] fn enabled_without_a_source_fails() {}
#[test] fn an_unknown_key_fails_extraction() { /* "defualt_mode", and an unknown key in [[tool_policy.org]] */ }
#[test] fn a_duplicate_org_fails() { /* message names "tool_policy.org[1].id" */ }
#[test] fn an_upper_case_org_id_applies_to_the_canonical_org() {     // Review Focus 5
    // id "0190A100-0000-7000-8000-0000000000A1" → settings().tenant(ORG_A) has its mode; a lower-case second entry is a duplicate
}
#[test] fn a_per_org_override_wins() { /* global closed, org open → tenant(org) == (mode, Open); another org → (default_mode, Closed) (D34) */ }
#[test] fn load_timeout_must_be_shorter_than_decide_timeout() { /* load_timeout_ms 5000 with decide_timeout_secs 5 → error (D31) */ }
#[test] fn bounds() { /* every *_secs and load_timeout_ms at least 1 and at most 86 400 s; max_stale_secs >= refresh_secs */ }
```

- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-gateway config`. Expected: FAIL.
- [ ] **Step 3: Implement.** Org ids use `domain::parse_org_uuid` and compare parsed `Uuid`s, like `LimitsConfig::validate`. Checked even when `enabled = false`, except the three "required when enabled" rules.
- [ ] **Step 4: Run.** Same, and the whole crate. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): add the [tool_policy] gateway config table (SMA-752)`.

---

### Task 12: The `PolicySource` port and the file source

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/domain/tool_policy.rs`; modify `src/domain/mod.rs` (`pub mod tool_policy;`)
- Create: `rs/crates/services/paigasus-gateway/src/adapters/policy_source/mod.rs`, `file.rs`; modify `src/adapters/mod.rs`
- Create: `rs/crates/services/paigasus-gateway/tool-policies/floor.cedar` (D15, P-14)
- Modify: `rs/crates/services/paigasus-gateway/Cargo.toml` (`sha2 = "0.11"`)

**Interfaces:**
- Produces:
  - `pub struct PolicyText { pub text: String, pub version: String }`
  - `pub struct TenantPolicies { pub text: String, pub catalog: Vec<String>, pub version: String }`
  - `#[derive(Debug, thiserror::Error)] pub enum SourceError { NotFound(String), Io(String), Malformed(String) }`
  - `#[async_trait] pub trait PolicySource: Send + Sync { async fn floor(&self) -> Result<PolicyText, SourceError>; async fn tenant(&self, org: Uuid) -> Result<TenantPolicies, SourceError>; }`
  - `pub struct FilePolicySource { … }` with `pub fn new(dir: impl Into<PathBuf>) -> Self`

- [ ] **Step 1: Write the failing tests** (unit tests in `file.rs`, `tempfile`).

```rust
#[tokio::test] async fn a_missing_floor_is_unavailable() { assert!(matches!(src.floor().await, Err(SourceError::NotFound(_)))); }
#[tokio::test] async fn an_empty_floor_is_an_empty_floor() { /* Ok, text "" */ }
#[tokio::test] async fn a_tenant_without_a_directory_is_empty() { /* text "", catalog [] */ }
#[tokio::test] async fn a_bad_catalog_is_malformed() { /* catalog.json not a string array */ }
#[tokio::test] async fn both_tenant_files_come_from_one_snapshot() {
    // tenants/<org> is a symlink to v1/; swap it to v2/ between the two reads (test hook: read through the resolved
    // real path once); the result is all-v1 or all-v2, never mixed.
}
#[tokio::test] async fn the_version_changes_with_the_content() {}
#[test] fn the_shipped_floor_compiles_and_holds_one_policy() {
    let f = paigasus_kernel::tool_policy::ToolPolicies::compile_floor(include_str!("../../../tool-policies/floor.cedar")).unwrap();
    assert_eq!(f.len(), 1);
}
```

- [ ] **Step 2: Run.** `cargo nextest run -p paigasus-gateway policy_source`. Expected: FAIL.
- [ ] **Step 3: Implement** per § 6.3.2. Use `tokio::fs` (`canonicalize` the tenant directory once, then read both files under the result). Version = hex SHA-256 of the file contents (floor: one file; tenant: both files with a separator).
- [ ] **Step 4: Run.** Same. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): add the tool-policy source port and the file source (SMA-752)`.

---

### Task 13: The policy cache

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/application/tool_policy/mod.rs`, `cache.rs`
- Modify: `rs/crates/services/paigasus-gateway/src/application/mod.rs` (`pub mod tool_policy;`; update the "flat files" doc line: `tool_policy` is the first subdirectory, by spec § 6.3)
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs` (cache metric names, in `ALL`, plus a registration test)
- Modify: `rs/crates/services/paigasus-gateway/src/main.rs` (`describe_*!` for the new names)

**Interfaces:**
- Consumes: `PolicySource` (Task 12), kernel compile (Task 2), `OnUnavailable` (Task 8).
- Produces:
  - `pub struct CacheSettings { pub refresh: Duration, pub max_stale: Duration, pub idle_evict: Duration, pub load_timeout: Duration }`
  - `pub enum Lookup<T> { Ready(T), Unavailable }`
  - `pub struct PolicyCache` with:
    - `pub fn new(source: Arc<dyn PolicySource>, settings: CacheSettings) -> Arc<Self>`
    - `pub async fn floor(&self, on_unavailable: OnUnavailable) -> Lookup<FloorEntry>` where `pub struct FloorEntry { pub floor: Arc<FloorPolicies>, pub floor_only: Arc<ToolPolicies> }` (P-4)
    - `pub async fn tenant(&self, org: Uuid, floor: &FloorEntry, on_unavailable: OnUnavailable) -> Lookup<Arc<ToolPolicies>>`
    - `pub fn invalidate_all(&self)`
    - `pub async fn refresh_once(&self)`
    - `pub async fn run_refresh(self: Arc<Self>, shutdown: watch::Receiver<()>)`
  - `pub trait Invalidate: Send + Sync + 'static { fn invalidate_all(&self); }` implemented by `PolicyCache` (the NATS adapter, Task 18, takes `Arc<dyn Invalidate>`)
  - Metric names: `GATEWAY_TOOL_POLICY_CACHE_LOADS_TOTAL` (`result` = `ok` | `source_error` | `compile_error` | `timeout`), `GATEWAY_TOOL_POLICY_LOAD_SECONDS`, `GATEWAY_TOOL_POLICY_COMPILE_SECONDS`, `GATEWAY_TOOL_POLICY_STALE_ENTRIES` (gauge), `GATEWAY_TOOL_POLICY_EVICTIONS_TOTAL`, `GATEWAY_TOOL_POLICY_REFRESH_ERRORS_TOTAL`

- [ ] **Step 1: Write the failing tests** (unit tests in `cache.rs`, `#[tokio::test(start_paused = true)]`, a fake source that counts calls, can fail, can be slow, and returns a chosen version).

```rust
#[tokio::test(start_paused = true)] async fn concurrent_misses_share_one_load() { /* 8 waiters, source called once */ }
#[tokio::test(start_paused = true)] async fn a_dropped_waiter_does_not_cancel_the_load() { /* drop the first future; a second lookup gets Ready without a second source call */ }
#[tokio::test(start_paused = true)] async fn a_slow_load_is_unavailable_after_load_timeout() { /* source sleeps 2 s, timeout 1 s → Unavailable; after 2 s the next lookup is Ready */ }
#[tokio::test(start_paused = true)] async fn a_failed_refresh_keeps_the_last_good_entry_until_max_stale() { /* Ready at 299 s after load, Unavailable at 301 s */ }
#[tokio::test(start_paused = true)] async fn a_failed_reload_after_invalidate_is_unavailable_under_closed() { /* Closed → Unavailable at once; Open → last good */ }
#[tokio::test(start_paused = true)] async fn an_idle_entry_is_evicted() { /* unused for idle_evict → refresh_once evicts it; eviction metric +1 */ }
#[tokio::test(start_paused = true)] async fn an_unchanged_version_is_not_compiled_again() { /* compile counter stays 1 after refresh_once */ }
#[tokio::test(start_paused = true)] async fn a_miss_whose_load_fails_is_unavailable() {}
#[tokio::test(start_paused = true)] async fn a_tenant_that_fails_to_compile_keeps_the_floor() { /* tenant Unavailable, floor Ready (§ 6.1.2) */ }
#[tokio::test(start_paused = true)] async fn a_new_floor_recompiles_the_tenant_without_a_source_call() {}
```

  Read the metrics with `metrics_util::debugging::DebuggingRecorder` and `set_default_local_recorder`, like `runtime.rs` tests.

- [ ] **Step 2: Run.** `cargo nextest run -p paigasus-gateway cache`. Expected: FAIL.
- [ ] **Step 3: Implement** per § 6.3.3. The load runs in a task the cache spawns (`tokio::spawn`), shared through `futures::future::Shared` over the `JoinHandle` result. Each waiter wraps its wait in `tokio::time::timeout(load_timeout)`. Use `tokio::time::Instant` so paused time works. Keep the stored tenant text, so a new floor recompiles without a source call.
- [ ] **Step 4: Run.** Same, plus `cargo nextest run -p paigasus-observability` and `moon run repo:observability-drift`. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): add the per-tenant tool-policy cache (SMA-752)`.

---

### Task 14: `CedarDecider` and `RequestToolPolicy`

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/application/tool_policy/decider.rs`, `request.rs`
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs` (`GATEWAY_TOOL_POLICY_DECISIONS_TOTAL`, `GATEWAY_TOOL_POLICY_REQUEST_REFUSALS_TOTAL`, `GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS`), `main.rs` describes

**Interfaces:**
- Consumes: `PolicyCache` (Task 13), `ToolPolicySettings` (Task 11), `decide_calls` (Task 8), `plan_request` and `rewrite_request` (Tasks 9-10), `domain::org_of`.
- Produces:
  - `pub struct CedarDecider;` implementing `ToolCallDecider`. It reads `ctx.policy` (D30). `None` is treated as `PolicySet::Unavailable` with `OnUnavailable::Closed`. It records `gateway_tool_policy_decisions_total{side="response", mode, effect, basis}` (`effect` = `allow` | `observe` | `block`) and one `tracing` event per call (request id, sanitized name, verdict, determining and errored policy ids, mode). Never arguments.
  - `pub struct RequestToolPolicy { … }` with `pub fn new(cache: Arc<PolicyCache>, settings: ToolPolicySettings) -> Self`
  - `pub async fn apply(&self, ctx: &DecisionContext, model: &str, format: Format, turn: &ToolTurn, body: Bytes) -> Result<RequestOutcome, Refusal>`
  - `pub struct RequestOutcome { pub body: Bytes, pub entries: Vec<RequestEntry>, pub snapshot: Arc<PolicySnapshot> }`
  - `#[derive(Debug, PartialEq, Eq)] pub enum Refusal { UnreadableTurn, UnknownToolChoice }` (both map to `GatewayError::BadRequestBody` in Task 15)

- [ ] **Step 1: Write the failing tests** (unit tests; a fake `PolicySource` with in-memory texts).

```rust
#[tokio::test] async fn the_decider_returns_one_decision_per_call() {}
#[tokio::test] async fn the_decider_uses_the_request_snapshot_after_invalidate_all() {   // Review Focus 3
    // apply() with policy v1 (allow "x"); change the source to v2 (deny "x") and invalidate_all(); decide → Allow
}
#[tokio::test] async fn tenant_off_still_evaluates_the_floor() { /* floor forbid "rm" → Block{Policy} with default_mode Off */ }
#[tokio::test] async fn a_team_or_project_scope_uses_its_org_settings() { /* scope = team PRN in ORG_A → ORG_A's mode */ }
#[tokio::test] async fn an_unparsed_scope_is_unavailable() { /* "prn:paigasus:iam:default:scope/team-a" → PolicySet::Unavailable */ }
#[tokio::test] async fn an_unavailable_turn_is_refused_only_in_enforce() {
    // ToolTurn::Unavailable: Enforce → Err(Refusal::UnreadableTurn); LogOnly and Off → Ok, original body,
    // refusal metric {mode="log_only", reason="unreadable_turn"} +1 (P-10)
}
#[tokio::test] async fn an_unknown_tool_choice_with_an_enforced_removal_is_refused() {}
#[tokio::test] async fn log_only_returns_the_original_bytes() { /* body is the same Bytes (ptr-equal or byte-equal), entries enforced false */ }
#[tokio::test] async fn enforce_returns_the_rewritten_body() {}
#[tokio::test] async fn a_slow_source_is_unavailable_within_load_timeout() { /* D31: apply returns within load_timeout + a margin, snapshot tenant part Unavailable */ }
```

- [ ] **Step 2: Run.** `cargo nextest run -p paigasus-gateway tool_policy`. Expected: FAIL.
- [ ] **Step 3: Implement.** `apply`: org from `domain::org_of(&ctx.scope)`; principal from `Prn::parse(&ctx.principal)` and `to_cedar_uid`; settings by org (D7, D34); floor lookup; tenant lookup only when the mode is not `Off`; build `Facts` with `RequestInfo { scope, ingress(format), model }` and `SessionInfo::placeholder()`; then `plan_request` and `rewrite_request`. Record the elapsed time of `apply` into `GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS`; the decider adds its own evaluation time to the same histogram. Request-side decisions are counted with `side="request"`.
- [ ] **Step 4: Run.** Same. Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): add the Cedar decider and the request-side tool policy (SMA-752)`.

---

### Task 15: Wire tool policy into the pipeline and the boot

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/mod.rs` (`AppState.tool_policy: Option<Arc<RequestToolPolicy>>`)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/pipeline.rs`, `interception.rs`
- Modify: `rs/crates/services/paigasus-gateway/src/main.rs`, `src/runtime.rs` (only if a helper is needed), `src/lib.rs` (crate doc: the SMA-752 paragraph replaces "This build's decider allows everything")
- Modify: every `AppState { … }` literal: `src/adapters/http/mod.rs` tests, `tests/support/limits.rs`, `tests/responses_proxy.rs`, `tests/endpoint_logs.rs`, `tests/metrics.rs`, `tests/chat_proxy.rs`, `tests/service_info.rs` (add `tool_policy: None`)

**Interfaces:**
- Consumes: Task 14.
- Produces:
  - `pipeline::proxy` order: parse facts → streaming check → interception tuple (turn, ctx) → `RequestToolPolicy::apply` (when `state.tool_policy` is `Some`) → `limits.admit_for` → egress with `outcome.body` (D17, D19).
  - `interception::intercept_body(ti, limits, format, ctx, turn, body, requests: &[RequestEntry]) -> BodyOutcome` (new last parameter)
  - `interception::intercepted_stream(…, prelude: Vec<Bytes>)` (new last parameter: the request-entry comments, sent before the first upstream frame)
  - Non-2xx and no-call bodies: when request entries exist, the `paigasus-tool-decisions` header is `report_header(&[], &entries)` (§ 6.5.3).
  - `main.rs`: with `tool_policy.enabled`, build `FilePolicySource`, `PolicyCache`, `RequestToolPolicy`, set `ToolInterception.decider = Arc::new(CedarDecider)`, and spawn `cache.run_refresh(rx.clone())` in the server `JoinSet`.
  - Test helper in `tests/support/limits.rs`: `pub fn app_with_tool_policy(iam: Arc<dyn Iam>, base_url: &str, ti: Option<Arc<ToolInterception>>, tp: Option<Arc<RequestToolPolicy>>) -> Router`

- [ ] **Step 1: Write the failing tests** (new file `tests/tool_policy.rs`, using the `tool_interception.rs` harness style and a fixture policy directory in a `tempfile::TempDir`).

```rust
#[tokio::test] async fn a_denied_call_is_blocked_in_enforce() { /* stream: notice "(policy)", no CANARY, request entries comments come before the first upstream frame */ }
#[tokio::test] async fn an_unavailable_turn_is_refused_before_the_limits() {
    // enforce + duplicate-key body → 400 invalid-request-body, mock request_count() == 0, limit store checks() == 0
}
#[tokio::test] async fn the_rewritten_body_goes_upstream() { /* mock.recorded().body has no removed tool */ }
#[tokio::test] async fn a_non_2xx_answer_carries_the_request_entries_header() {}
#[tokio::test] async fn off_everywhere_is_byte_identical() { /* tool_policy None vs Some(all off, empty floor): same upstream request bytes and same client bytes */ }
```

- [ ] **Step 2: Run.** `cargo nextest run -p paigasus-gateway --test tool_policy`. Expected: FAIL.
- [ ] **Step 3: Implement.** Move the interception-tuple construction above the limits block; the ctx gets `with_policy(outcome.snapshot)`. Map `Refusal` to `GatewayError::BadRequestBody` and log the refusal reason at debug. Update the gateway `Cargo.toml` comment on `paigasus-kernel` (C2): the kernel now brings Cedar into the gateway through gateway-core's `tool-policy` feature, on purpose (ADR-0027 (e)); `paigasus-iam-core` stays out.
- [ ] **Step 4: Run.** `cargo nextest run -p paigasus-gateway` (every existing test passes unchanged except the `AppState` literals). Expected: PASS.
- [ ] **Step 5: Commit.** `feat(rs): wire tool-policy decisions into the gateway pipeline (SMA-752)`.

---

### Task 16: The end-to-end test matrix

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/tests/tool_policy.rs`
- Create (if helpers grow): `rs/crates/services/paigasus-gateway/tests/support/tool_policy.rs` (fixture directory builder: `fn policy_dir(floor: &str, tenants: &[(&str, &str, &[&str])]) -> TempDir`, a slow and a failing `PolicySource`)

**Interfaces:**
- Consumes: Task 15 helpers.

- [ ] **Step 1: Write the tests** (§ 7.2). Each runs for Chat non-stream, Chat stream and `/v1/responses` unless the line says otherwise.

```rust
// log-only vs enforce (allowed and denied call); log-only bytes equal mode off apart from ": paigasus-decision" comments and headers
#[tokio::test] async fn log_only_is_byte_equal_to_off_apart_from_comments_and_headers() {}
#[tokio::test] async fn enforce_blocks_and_log_only_observes_a_denied_call() {}
// fail-open vs fail-closed, unavailable source and slow source (> load_timeout_ms), log_only and enforce
#[tokio::test] async fn fail_closed_and_fail_open_with_an_unavailable_source() {}
#[tokio::test] async fn fail_closed_and_fail_open_with_a_slow_source() {}
#[tokio::test] async fn an_unavailable_floor_blocks_every_call_under_closed_and_reports_under_open() {}   // § 7.2, P-6
#[tokio::test] async fn a_per_org_override_wins_over_the_global_value() {}                                // D34
#[tokio::test] async fn an_argument_policy_allows_workspace_paths_only() {}                              // read_file under /workspace/
#[tokio::test] async fn definition_removal_and_each_tool_choice_case() {}                                // step 4
#[tokio::test] async fn each_last_definition_case() {}                                                   // step 5 (chat, responses)
#[tokio::test] async fn an_input_reading_permit_keeps_the_definition() {}
#[tokio::test] async fn tool_result_replacement_for_each_result_type() {}                                // incl. chat name from the earlier call
#[tokio::test] async fn name_collision_with_the_shipped_floor() {}
#[tokio::test] async fn an_unavailable_turn_is_refused_in_enforce_and_counted_in_log_only() {}
#[tokio::test] async fn an_empty_catalog_in_enforce_removes_every_definition() {}
#[tokio::test] async fn team_and_project_scoped_keys_use_their_org_mode() {}
#[tokio::test] async fn a_floor_forbid_blocks_in_tenant_off_and_log_only() {}                            // D33
#[tokio::test] async fn mode_off_everywhere_keeps_request_and_response_bytes() {}
```

  The config cases of § 7.2 (missing `on_policy_unavailable`, unknown key, duplicate org UUID) are in Task 11.

- [ ] **Step 2: Run.** `cargo nextest run -p paigasus-gateway --test tool_policy`. Expected: PASS. A test that fails shows a defect in Tasks 6-15. Fix it in the owning file in this task, and say which task's code you changed.
- [ ] **Step 3: Red-first proof.** In `policy/calls.rs`, delete the one line that maps `floor_denied` to `Block{Policy}`. Run `a_floor_forbid_blocks_in_tenant_off_and_log_only`. Expected: FAIL. Restore the line by hand.
- [ ] **Step 4: Commit.** `test(rs): cover tool-policy modes and failure end to end (SMA-752)`.

---

### Task 17: Move the NATS creds callback into `paigasus-nats-creds`

**Files:**
- Create: `rs/crates/libs/paigasus-nats-creds/{Cargo.toml, moon.yml, src/lib.rs}` (the code and tests of `creds.rs`, unchanged)
- Delete: `rs/crates/services/paigasus-iam/src/adapters/events/creds.rs`; modify `src/adapters/events/mod.rs` and `nats_publisher.rs` imports
- Modify: `rs/Cargo.toml` (`members` gets the literal path `"crates/libs/paigasus-nats-creds"`; `[workspace.dependencies] paigasus-nats-creds = { path = "crates/libs/paigasus-nats-creds", version = "0.0.0" }`)
- Modify: `rs/crates/services/paigasus-iam/Cargo.toml`, `rs/crates/services/paigasus-iam/moon.yml` (`dependsOn`, `upstreams`)

**Interfaces:**
- Produces: `paigasus_nats_creds::{parse_credentials, auth_from_credentials, ParsedCredentials, CredsError}` with the same signatures as today.

- [ ] **Step 1: Move the module.** `git mv` the file into the new crate's `src/lib.rs` so history follows. The crate is `publish = false`, `[lints] workspace = true`, dependencies `async-nats`, `nkeys`, `thiserror`, `tokio` (`fs`). The `moon.yml` follows `paigasus-redis/moon.yml` (`id: 'paigasus-nats-creds-rs'`, `layer: 'library'`, empty `upstreams`).
- [ ] **Step 2: Run.** `cd rs && cargo nextest run -p paigasus-nats-creds -p paigasus-iam --lib` and `moon run repo:affected-smoke repo:machete repo:deny` (affected-smoke under `/bin/bash`). Expected: PASS, with the same creds test count as before the move.
- [ ] **Step 3: Commit.** `refactor(rs): move the NATS creds callback into paigasus-nats-creds (SMA-752)`. If Moon regenerates `.github/CODEOWNERS`, include it.

---

### Task 18: The NATS subscription and the `iam.policy.>` grant

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/adapters/nats/mod.rs`, `policy_events.rs`; modify `src/adapters/mod.rs`, `src/main.rs`
- Modify: `rs/crates/services/paigasus-gateway/Cargo.toml` (`async-nats = { workspace = true }`, `paigasus-nats-creds = { workspace = true }`; dev: `nkeys = { workspace = true }`), `moon.yml` (`dependsOn`, `upstreams`)
- Create: `rs/crates/services/paigasus-gateway/tests/tool_policy_nats.rs`
- Modify: `ops/nats/subjects.env` (`CONSUMER_SUB` gets `"iam.policy.>"`), `ops/nats/test/accounts.conf.tmpl`, `ops/nats/permissions.md`, `ops/nats/provision.sh` (only if it does not already read `CONSUMER_SUB`)
- Modify: `rs/crates/services/paigasus-iam/tests/nats_permissions.rs`
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs` (`GATEWAY_TOOL_POLICY_NATS_INVALIDATIONS_TOTAL` with `cause` = `message` | `connect`, `GATEWAY_TOOL_POLICY_NATS_CONNECTED` gauge)

**Interfaces:**
- Consumes: `Invalidate` (Task 13), `ToolPolicyNatsConfig` (Task 11).
- Produces:
  - `pub struct PolicyEvents { client: async_nats::Client, … }`
  - `pub async fn connect(cfg: &ToolPolicyNatsConfig, target: Arc<dyn Invalidate>) -> Result<PolicyEvents, NatsError>` (inbox prefix `_INBOX_GW`; creds through `paigasus_nats_creds::auth_from_credentials` in `with_auth_callback`; optional CA with `add_root_certificates`; an `event_callback` that calls `invalidate_all()` on `Event::Connected` and sets the gauge)
  - `impl PolicyEvents { pub async fn run(self, shutdown: watch::Receiver<()>) -> anyhow::Result<()>; pub fn client(&self) -> &async_nats::Client; }` (`client()` lets a test force a reconnect)

- [ ] **Step 1: Measure first (D24).** Add the IAM test below, then run it against the pinned `nats:2.10.14` before any gateway code. If the core subscriber does not get the JetStream publish with only the new grant, STOP and report to the coordinator. Do not grant a wider right.

```rust
// rs/crates/services/paigasus-iam/tests/nats_permissions.rs
#[tokio::test] async fn the_consumer_core_subscription_receives_a_jetstream_policy_publish() { /* provision; consumer subscribes "iam.policy.>"; publisher JetStream-publishes iam.policy.put; the consumer gets it within VIOLATION_BUDGET */ }
#[tokio::test] async fn the_consumer_cannot_subscribe_to_other_iam_subjects() { /* "iam.api_key.>" and "iam.role.granted" → Permissions Violation */ }
#[tokio::test] async fn the_policy_grant_adds_no_jetstream_right() {
    // publish iam.policy.put → denied (no forging); $JS.API.CONSUMER.CREATE.IAM_EVENTS.gateway-cache-invalidator → denied;
    // $JS.API.CONSUMER.UPDATE… form → denied; subscribe "$JS.>" → denied
}
```

  Update the doc comment above `CONSUMER_PUB` in `subjects.env` and the comment in `nats_permissions.rs` that says subject-level subscribe permissions cannot narrow this user: the new core grant is subject-level, and it is the only subscribe grant outside the inbox.

- [ ] **Step 2: Run.** `/opt/homebrew/bin/bash ops/nats/check-subjects.sh` and `( cd rs && cargo nextest run --locked -p paigasus-iam --test nats_permissions --profile iam-nats )`. Expected: PASS (needs Docker).

- [ ] **Step 3: Write the failing gateway tests** (`tests/tool_policy_nats.rs`; a `nats:2.10.14` container with `-js` and no auth, through `paigasus_test_docker::start_or_skip`; a stream `IAM_EVENTS` on `iam.>`; a counting `Invalidate`).

```rust
#[tokio::test] async fn two_replicas_both_invalidate_on_one_jetstream_publish() { /* two PolicyEvents; one JetStream publish on iam.policy.put; both counters reach connect+1 */ }
#[tokio::test] async fn a_reconnect_invalidates() { /* client().force_reconnect(); counter grows by 1, cause "connect" */ }
#[tokio::test] async fn a_non_policy_subject_does_not_invalidate() { /* publish iam.role.granted → counter unchanged */ }
#[tokio::test] async fn nats_down_never_blocks_a_request() { /* connect to a closed port: connect returns (retry_on_initial_connect), and a router request with tool policy on is answered */ }
```

- [ ] **Step 4: Implement.** `run` loops on `client.subscribe("iam.policy.>")`; every message calls `invalidate_all()` (D16) and counts `cause="message"`. Use `ConnectOptions::retry_on_initial_connect()` and the client's own reconnect backoff (§ 8: the builder's choice; record the values in a comment). `main.rs` spawns `run` in the server `JoinSet` only when `[tool_policy.nats]` is set.
- [ ] **Step 5: Run.** `cargo nextest run -p paigasus-gateway --test tool_policy_nats` and `moon run repo:nats-permissions repo:observability-drift`. Expected: PASS.
- [ ] **Step 6: Commit.** `feat(ops): grant the gateway a core subscription on iam.policy.> and invalidate the tool-policy cache from it (SMA-752)`. Use two commits if the reviewer wants `feat(ops)` and `feat(rs)` apart; both end in `(SMA-752)`.

---

### Task 19: The performance test

**Files:**
- Create: `rs/crates/services/paigasus-gateway/tests/perf.rs`

**Interfaces:**
- Consumes: `GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS` (Task 14), `app_with_tool_policy` (Task 15), `FilePolicySource`, `PolicyCache`.

- [ ] **Step 1: Write the test** (§ 7.4, D18). Both tests are `#[ignore = "a measurement in release mode"]`.

```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "a measurement in release mode"]
async fn p99_policy_overhead_with_50_policies() {
    // DebuggingRecorder installed as the global recorder (nextest runs each test in its own process).
    // Fixture: 50 tenant policies, all in scope for "read_file", with argument and name conditions; 30 definitions;
    // tool results in the history; one planned removal (one rewrite); one call with a 64 KiB argument in the body.
    // Warm the cache, then 2000 non-stream chat requests, 8 at a time, mode enforce.
    let p99 = percentile(&overhead_samples(), 0.99);
    println!("p99 = {:.3} ms over {} samples", p99 * 1e3, n);
    assert!(p99 < 0.015, "p99 {p99}");
}
#[tokio::test]
#[ignore = "a measurement in release mode"]
async fn cold_load_and_compile_of_50_policies() { /* PolicyCache::floor + tenant on an empty cache; assert < 50 ms; print the time */ }
```

- [ ] **Step 2: Run.** `cd rs && cargo nextest run --release -p paigasus-gateway --test perf --run-ignored only --no-capture`. Expected: PASS. Record both numbers and the machine in your report; the PR body quotes them.
- [ ] **Step 3: Commit.** `test(rs): measure the tool-policy overhead per request (SMA-752)`.

---

### Task 20: The D21 gate assertion, docs and the full gate graph

**Files:**
- Modify: `moon.yml` (task `wasm-getrandom-free`: script, description, inputs)
- Modify: `ci/cargo-lock-integrity/README.md` (the row that quotes the gate's `cargo tree` command)
- Modify: `rs/crates/libs/paigasus-kernel/README.md` (one paragraph on the `tool-policy` feature)
- Modify: `rs/crates/services/paigasus-gateway/gateway.toml.example` (check the Task 11 block against the final keys)

- [ ] **Step 1: Measure the command.** Run `cd rs && cargo tree --locked -p paigasus-wasm -e normal -i cedar-policy; echo "rc=$?"` and the same for `paigasus-py-bindings` and `paigasus-node-bindings`. Record what it prints and the exit status when cedar is absent.
- [ ] **Step 2: Add the assertion.** After the getrandom check, for each of the three crates: if the measured `-i` form exits 0 with empty output when absent, use it (D21); if it exits non-zero when absent, run the full `cargo tree --locked -p <crate> -e normal` instead and `grep -qw cedar-policy`. In both forms a failed `cargo tree` exits 2, and a hit exits 1 with `cedar-policy is in the <crate> dependency graph — the kernel tool-policy feature leaked into a binding`. Description: add "and that no binding crate pulls `cedar-policy` (SMA-752 D21)". Inputs: add the py and node binding `Cargo.toml` and `src/**/*`.
- [ ] **Step 3: Red-first proof.** Add `features = ["tool-policy"]` to the kernel dependency in `rs/crates/bindings/paigasus-wasm/Cargo.toml`. Run `moon run repo:wasm-getrandom-free --force`. Expected: FAIL with exit 1 and the message. Remove that edit by hand.
- [ ] **Step 4: Run the full gate graph** as CI does (the command between the `ci-targets` markers in the root `CLAUDE.md`), with `--base origin/main`. Re-run the bash-split gates directly per the root `CLAUDE.md` (`affected-smoke` with `/bin/bash`; `ruff-ci`, `next-public-free`, `publish-metadata`, `nats-permissions` with `/opt/homebrew/bin/bash`). Expected: every gate green. Report each gate that you could not run locally and why.
- [ ] **Step 5: Commit.** `ci: assert that no binding crate pulls cedar-policy (SMA-752)` (the `moon.yml` and README edits), then `docs(rs): document the kernel tool-policy feature (SMA-752)` if docs changed separately.
