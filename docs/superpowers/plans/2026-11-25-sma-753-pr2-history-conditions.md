# SMA-753 PR 2: History Conditions Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the history conditions that IAM compiles work in the gateway: evaluate them from the trusted session history, fill the Cedar slots, and let IAM accept saves with conditions behind a setting (spec S1 to S14).

**Architecture:** A pure kernel module `tool_policy::history` holds the condition types, one typed argument walker (shared with `map_input`), the field extraction, an injected `HistoryDigester` port and the three-valued evaluator. `decide_with_slots` adds the slots to the Cedar context. `paigasus-gateway-core` maps the IAM proto into kernel conditions at load, validates them, and computes the `FieldPlan` and the `ReadPlan`. The gateway cache keeps one bound `HistoryPolicy` per org. The reserve script reads the window in the same atomic step that appends the hold, and the decider computes the slots of every call of the hold once, before any decision.

**Tech Stack:** Rust edition 2024 (rust-version 1.95), `cedar-policy` 4 (the rs/ lock resolves 4.15.0), `hmac` 0.13 + `sha2` 0.11, `base64` 0.23, `redis` + `paigasus-redis` (Lua), `testcontainers-modules` Redis, cargo-nextest, Helm, buf, Dogwood 1.2.0 (compiler workspace only).

**Spec:** `docs/superpowers/specs/2026-11-25-sma-753-pr2-history-conditions-design.md` (revision 2, approved). The parent design is `docs/superpowers/specs/2026-11-22-sma-753-session-history-design.md`. Read both with this plan. The PR 1 plan's key layout section is stale. Use the layout in `adapters/history/redis.rs` (module doc) and this plan.

## Global Constraints

- Every new source file starts with `// SPDX-License-Identifier: Apache-2.0` (`#` for shell and YAML).
- Commits: conventional, with a scope (`feat(rs): …`, `test(rs): …`, `feat(charts): …`, `docs(ops): …`, `chore(contracts): …`). No line in a commit body starts with `#` and a number. End each commit message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- After every task: `cd rs && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check` pass. Dead code is a hard error. A task adds no private or `pub(crate)` item that only a later task uses. A `pub` item of a library crate is not dead code, but each such item gets a test in its own task.
- Bash PATH: prefix every command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Do not install software on the host. Do not change a crate version by hand (release-plz versions `paigasus-kernel`; `.github/CLAUDE.md`). The kernel change is additive (spec P2).
- Redis clients come only from `paigasus_redis::connect`. Tests read Redis only with `redis-cli` through `container.exec`.
- Every new gateway Docker test binary goes into a gateway override of `rs/.config/nextest.toml` with `test-group = 'docker-containers'`.
- A new in-tree `{ workspace = true }` dependency needs a `dependsOn` entry and the `fileGroups.upstreams` entries in the crate's `moon.yml` (`ci/CLAUDE.md`, A6 of `repo:affected-smoke`).
- No `Bytes` parameter in a function of `adapters/http/` (`repo:http-extractor-envelope`).
- Constants (spec P1, P3, P4, P7; design D11): `RETENTION_MS = 90_000_000` (one value, in the kernel, re-exported by the gateway domain); `MAX_WINDOW_MS = 86_400_000`; `MAX_EVENTS_PER_TOOL = 10_000`; `MAX_READ_SETS = 64`; `MAX_READ_EVENTS = 20_000`; `MAX_INDEXED_TOOLS = 256`; `MAX_PREDICATE_DEPTH = 32` (the gateway-core mapping keeps its own constant `MAX_PREDICATE_DEPTH: usize = 32`, guarded by the equality test with the IAM value in Task 17); digest length **16 bytes**; key id **first 8 bytes of `HMAC-SHA-256(key, "paigasus-history-kid")`, lower-case hex**; minimum key length **32 bytes**.
- Digest input (spec P3): `HMAC-SHA-256(key, org || 0x00 || path || 0x00 || tag || value)`, cut to 16 bytes. `org` is the 36-character hyphenated lower-case UUID text. `path` is the segments joined with `.`. Tags: `s` + UTF-8 bytes; `b` + `1` or `0`; `d` + the scaled value (decimal × 10,000) as an 8-byte big-endian `i64`.
- Event body (spec P3): keeps `v: 1`. New members: `f` (object, path → value), `kid` (hex key id), `b: 1` (unparsed, and then no `f`). A `Long` is a JSON number, a digest is base64url with no padding, `Absent` is `null`, `Mistyped` is `{"x":1}`.
- Reserved context names (spec P2): `input`, `input_text`, `findings`, `max_severity`, `call`, `request`, `session`.
- Metric names (spec P7, P10): `gateway_tool_history_slots_missing_total{part, reason}`, `part` ∈ `floor`, `tenant`; `reason` ∈ `disabled`, `store`, `no_session`, `truncated`, `unknown`, `retention`, `no_key`, `catalog` (all 16 pairs primed at 0); `gateway_tool_history_events_missing_total` (no label, primed at 0).
- Config (spec P3, P13): `[tool_history] digest_key`, `previous_digest_key` (env `GATEWAY_TOOL_HISTORY__DIGEST_KEY`, `GATEWAY_TOOL_HISTORY__PREVIOUS_DIGEST_KEY`); IAM `tool_policies.history_conditions` (env `IAM_TOOL_POLICIES__HISTORY_CONDITIONS`, default `false`); chart `zones.iam.backend.toolPolicies.historyConditions` (default `false`).
- `Debug` of a key, of `ExtractedFields`, of `Fields` and of a digest prints no value: counts, key ids and fixed names only.
- The decision record gets no slot value and no field (spec § 2, ADR-0027 (i)).
- A test that the plan shows with an empty body `{}` must assert the rule in its name. A test that asserts nothing is a defect.

## Plan decisions

The spec leaves these to the builder (§ 5). Each task uses them.

- **D-A. Unbound literals.** The kernel `Literal` has a third variant, `Literal::Unbound(Scalar)`, for a string, bool or decimal literal before the digester binds it. `bind_digests` turns it into `Literal::Digests`. The evaluator treats `Unbound` and an empty `Digests` the same way: the slot is missing with reason `no_key`. The reason: a floor literal digest depends on the org (the digest input holds the org), but the floor is cached once for every org.
- **D-B. No key.** With no history key, a string, bool or decimal argument value at a stored path is stored as `Mistyped`, not left out. So `sum_within` over a field that holds `"4000"` stays fail closed (A3). The value is still not stored (A4).
- **D-C. A non-Long value at a `sum_within` path.** On an event whose predicate is unknown, the slot is missing (reason `unknown`), whatever the stored value is (spec P1). On an event whose predicate is true, a stored value other than `Long` or `Absent` (a `Digest` or `Mistyped`), or unparsed arguments, makes the slot missing (reason `unknown`). A path that is not in the event adds 0 (design R3). This is the general form of the decided-call rule (spec P1).
- **D-D. Hold intent.** `SessionContext` keeps a list of `(CallFacts, Uuid)` entries. The decider adds the entries of a hold at the first poll of its future, before the reserve. The report site claims, for each report entry that has call facts and no decision id, the oldest unclaimed entry with equal `CallFacts`. A claimed entry is removed.
- **D-E. Append under a known id.** An append under an id that a reserve already wrote replaces the body, the verdict and the score of that one member. It never adds a second event (`ZADD` and `HSET` on the same member and key). So an append after a reserve that returned late counts the call once.
- **D-F. The cache bundle.** A tenant lookup returns `CompiledTenant { policies, history }`. The cache keeps the last known `Catalog` and `FieldPlan` of each org in a separate map, so a later unavailable set still stores the right fields (design D7) and a floor condition still knows the catalog (spec P7).

## Review Focus

1. **A condition path deeper than 16 levels.** `context.input` holds the raw JSON text there as a Cedar string. `extract_fields` must store the same value (a string digest of the raw text), not `Mistyped`. Test: Task 1 `a_value_below_depth_16_is_its_raw_text_string`.
2. **A decimal argument with more than four fraction digits** (`1.23456`) or an exponent (`1e2`). `classify_number` makes `1.23456` a Text number, so the field is `Mistyped` and a comparison on it is unknown (the slot is missing), while `1e2` is `Long(100)`. Test: Task 1 `numbers_are_classified_as_context_input_classifies_them`.
3. **A tool name that differs from the catalog `name_key` only in case or Unicode form** (`"Send"`, `"send"`). The event tool is `normalise(display_name)`, and load validation refuses a `name_key` that is not in normal form, so both count. Test: Task 11 `a_call_named_in_another_case_counts_for_its_name_key`.
4. **A history that holds events under the previous key during a rotation.** The literal holds both digests, so an old event still matches. An event under a third key id is unknown. Test: Task 2 `an_event_under_the_previous_key_matches_and_an_unknown_key_is_unknown`.
5. **Two calls with the same call facts in one hold** (a provider that repeats `call_0` in one message). Each report entry claims its own intent entry, in order. Test: Task 8 `equal_call_facts_claim_their_intents_in_order`.

---

### Task 1: The kernel argument walker, the field extraction and the digester port

**Files:**
- Create: `rs/crates/libs/paigasus-kernel/src/tool_policy/walk.rs` (the typed walk)
- Create: `rs/crates/libs/paigasus-kernel/src/tool_policy/history/mod.rs`, `history/fields.rs`
- Modify: `rs/crates/libs/paigasus-kernel/src/tool_policy/input.rs` (`map_input` builds from the walk)
- Modify: `rs/crates/libs/paigasus-kernel/src/tool_policy/mod.rs` (`pub mod history;`)
- Test: `rs/crates/libs/paigasus-kernel/src/tool_policy/history/fields.rs` (unit tests), `tests/tool_policy_props.rs` (one property)

**Interfaces:**
- Produces (crate-private): `enum Arg { Record(Vec<(String, Arg)>), Set(Vec<Arg>), Str(String), Bool(bool), Long(i64), Decimal(String), TextNumber(String), RawText(String) }` and `fn parse_arguments(input: &ToolInput) -> Parsed { args: Option<Arg>, input_text: Option<String>, unparsed: bool }`. The rules are the rules of `map_input` today: nulls dropped, depth limit 16 (a deeper value is `RawText`), duplicate keys and nesting over 128 unparsed.
- Produces (public, in `tool_policy::history`):

```rust
#[non_exhaustive] #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FieldPath(Vec<String>);            // new(segments) -> Result<Self, PathError>; segments(); joined() -> "a.b"
#[non_exhaustive] #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyId(pub [u8; 8]);               // new([u8; 8]) -> Self; to_hex(); from_hex(&str) -> Option<Self>
#[non_exhaustive] #[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Digest(pub [u8; 16]);             // new([u8; 16]) -> Self (the tuple constructor is closed outside the kernel by non_exhaustive); Debug prints "Digest(..)"
#[non_exhaustive] #[derive(Clone, PartialEq, Eq)]
pub enum Scalar { Str(String), Bool(bool), Decimal(i64) }   // canonical_bytes() -> Vec<u8> (tag + value, spec P3)
pub type PathSet = std::collections::BTreeSet<FieldPath>;
#[non_exhaustive] #[derive(Clone, PartialEq, Eq)]
pub enum Extracted { Long(i64), Scalar(Scalar), Absent, Mistyped }
#[non_exhaustive] #[derive(Clone, PartialEq, Eq)]   // Debug is hand-written: counts only
pub struct ExtractedFields { /* None = unparsed */ }  // is_unparsed(); get(&FieldPath) -> Option<&Extracted>; into_fields(self, org: Uuid, d: &dyn HistoryDigester) -> (Fields, Option<KeyId>)
#[non_exhaustive] #[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field { Long(i64), Digest(Digest), Absent, Mistyped }
#[non_exhaustive] #[derive(Clone, PartialEq, Eq)]   // Debug is hand-written: counts only
pub enum Fields { Unparsed, Values(std::collections::BTreeMap<FieldPath, Field>) }
pub trait HistoryDigester: Send + Sync {
    /// The digest under the current key, or None when the gateway has no key.
    fn current(&self, org: Uuid, path: &FieldPath, value: &Scalar) -> Option<(KeyId, Digest)>;
    /// The digests under the current key and the previous key (a literal), current first.
    fn all(&self, org: Uuid, path: &FieldPath, value: &Scalar) -> Vec<(KeyId, Digest)>;
}
pub fn extract_fields(input: &ToolInput, plan: &PathSet) -> ExtractedFields;
```

Rules:
- `FieldPath::new` refuses an empty path, an empty segment and a segment that holds `.` (spec P3, P5).
- `extract_fields`: unparsed arguments give `is_unparsed()`. Else for each path: walk `Record`s by segment. A missing member or a dropped null gives `Absent`. A path through a non-record gives `Absent`. The value: `Long` → `Long`; `Str` and `RawText` → `Scalar::Str`; `Bool` → `Scalar::Bool`; `Decimal(text)` → `Scalar::Decimal(scaled)` (parse the kernel decimal text, × 10,000); `Record`, `Set`, `TextNumber` → `Mistyped`.
- `into_fields`: `Scalar` → `Field::Digest` under `current`, or `Field::Mistyped` when `current` is `None` (plan decision D-B). The key id is `Some` only when at least one digest was made.
- `Debug` of `ExtractedFields` and `Fields` is written by hand (no derive, because a derived `Debug` prints paths and `Long` values). It prints the path count and `unparsed` only.

- [ ] **Step 1: Write the failing tests**

```rust
struct TestDigester; // current = Some((KeyId([1;8]), Digest(the first 16 bytes of path.joined() + canonical_bytes(), zero padded))); all = vec![current]. The kernel has no hash dependency, and the test needs none.
fn paths(p: &[&[&str]]) -> PathSet { /* FieldPath::new(..).unwrap() */ }

#[test]
fn every_value_class_maps_to_its_field() {
    let raw = r#"{"n":5,"s":"x","t":true,"d":1.5,"o":{"k":1},"a":[1],"big":12345678901234567890,"z":null}"#;
    let e = extract_fields(&ToolInput::Json(raw.into()), &paths(&[&["n"], &["s"], &["t"], &["d"], &["o"], &["a"], &["big"], &["z"], &["missing"], &["o", "k"]]));
    assert_eq!(e.get(&p(&["n"])), Some(&Extracted::Long(5)));
    assert_eq!(e.get(&p(&["s"])), Some(&Extracted::Scalar(Scalar::Str("x".into()))));
    assert_eq!(e.get(&p(&["t"])), Some(&Extracted::Scalar(Scalar::Bool(true))));
    assert_eq!(e.get(&p(&["d"])), Some(&Extracted::Scalar(Scalar::Decimal(15_000))));
    for m in [&["o"][..], &["a"], &["big"]] { assert_eq!(e.get(&p(m)), Some(&Extracted::Mistyped)); }
    for a in [&["z"][..], &["missing"]] { assert_eq!(e.get(&p(a)), Some(&Extracted::Absent)); }
    assert_eq!(e.get(&p(&["o", "k"])), Some(&Extracted::Long(1)));
}

#[test]
fn numbers_are_classified_as_context_input_classifies_them() {
    // 5.0 -> Long(5), -0.0 -> Long(0), 1e2 -> Long(100), 1.5000 -> Decimal(15000), 1.23456 and 1e19 -> Mistyped
}

#[test]
fn a_value_below_depth_16_is_its_raw_text_string() {
    // 16 nested "a" records, then {"x":1}: path a×16 . x is Scalar::Str("1"), as map_input gives context.input...x == "1"
}

#[test]
fn unparsed_and_free_text_arguments_have_no_fields() {
    for input in [ToolInput::Json(r#"{"k":1,"k":2}"#.into()), ToolInput::Json("[1]".into()), ToolInput::Text("x".into())] {
        assert!(extract_fields(&input, &paths(&[&["k"]])).is_unparsed());
    }
}

#[test]
fn no_key_stores_a_scalar_as_mistyped_and_no_key_id() { /* digester whose current() is None: s -> Field::Mistyped, n -> Long, kid None */ }

#[test]
fn a_path_with_an_empty_or_dotted_segment_is_refused() {
    assert!(FieldPath::new(vec![]).is_err());
    assert!(FieldPath::new(vec!["".into()]).is_err());
    assert!(FieldPath::new(vec!["a.b".into()]).is_err());
}

#[test]
fn scalar_canonical_bytes() {
    assert_eq!(Scalar::Str("é".into()).canonical_bytes(), [b"s".as_slice(), "é".as_bytes()].concat());
    assert_eq!(Scalar::Bool(false).canonical_bytes(), b"b0");
    assert_eq!(Scalar::Decimal(-15_000).canonical_bytes(), [b"d".as_slice(), &(-15_000i64).to_be_bytes()].concat());
}

#[test]
fn debug_prints_no_value() { /* format!("{:?}", extracted) and Fields contain neither "secret-value", nor a path name, nor a Long value, nor its digest bytes; they hold the path count */ }

#[test]
fn key_id_and_digest_build_outside_the_module_with_new() {
    assert_eq!(KeyId::new([7; 8]).0, [7; 8]);
    assert_eq!(Digest::new([9; 16]).0, [9; 16]);
}
```

In `tests/tool_policy_props.rs`, add `map_input_and_extract_fields_agree`: for generated JSON objects with string, bool, integer and nested values, every `Scalar::Str(s)` at a path satisfies `context.input.<path> == "<s>"` in a Cedar evaluation, and every `Long(n)` satisfies `context.input.<path> == n`. All existing `input.rs` tests stay and pass unchanged.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-kernel --features tool-policy history::fields`
Expected: FAIL to compile (`history` not found).

- [ ] **Step 3: Implement**

Move the walk of `input.rs` into `walk.rs` as `parse_arguments` over `Arg`. `map_input` maps `Arg` into `RestrictedExpression` with no change in result (`TextNumber` and `RawText` become strings). `extract_fields` and `into_fields` live in `history/fields.rs`. Re-export the public names from `history/mod.rs`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-kernel --features tool-policy`
Expected: PASS, the old `input.rs` tests included.

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/libs/paigasus-kernel
git commit -m "feat(rs): add one typed argument walk and the history field extraction (SMA-753)"
```

---

### Task 2: The kernel condition types and the evaluator

**Files:**
- Create: `rs/crates/libs/paigasus-kernel/src/tool_policy/history/types.rs`, `history/evaluate.rs`, `history/tools.rs`
- Modify: `rs/crates/libs/paigasus-kernel/src/tool_policy/history/mod.rs`
- Test: `rs/crates/libs/paigasus-kernel/tests/history_evaluate.rs` (new, `#![cfg(feature = "tool-policy")]`)

**Interfaces:**
- Consumes: `FieldPath`, `Field`, `Fields`, `KeyId`, `Digest`, `Scalar`, `HistoryDigester` (Task 1).
- Produces (all `#[non_exhaustive]`, each with a `new` constructor where it is a struct):

```rust
pub const RETENTION_MS: u64 = 90_000_000;
pub enum Part { Floor, Tenant }                       // as_str(): "floor" | "tenant"
pub enum Scope { Session, Principal, Key }
pub enum EventKind { Request, Response }
pub enum CmpOp { Lt, Le, Eq, Ge, Gt, Ne }             // apply(a: i64, b: i64) -> bool
pub enum ToolMatch { NameKey(String), AnyCatalogTool }
pub enum Literal { Long(i64), Unbound(Scalar), Digests(Vec<(KeyId, Digest)>) }   // plan decision D-A
pub struct FieldTest { pub path: FieldPath, pub op: CmpOp, pub literal: Literal }
pub enum Predicate { EventKind(EventKind), Tool(ToolMatch), Field(FieldTest), AllOf(Box<Predicate>, Box<Predicate>), AnyOf(Box<Predicate>, Box<Predicate>), Not(Box<Predicate>) }
pub enum Shape { Formerly { predicate: Predicate }, CountWithin { predicate: Predicate, op: CmpOp, threshold: i64 }, SumWithin { predicate: Predicate, path: FieldPath, op: CmpOp, threshold: i64 } }
pub struct Condition { pub slot: String, pub part: Part, pub scope: Scope, pub window_ms: u64, pub shape: Shape }
pub enum SetTool { Tool(String), Every }
pub struct Event { pub id: Uuid, pub ts_ms: u64, pub kind: EventKind, pub tool: String, pub fields: Fields, pub key: Option<KeyId> }
pub struct WindowSet { pub scope: Scope, pub tool: SetTool, pub events: Vec<Event>, pub mark_ms: Option<u64>, pub truncated: bool }
pub struct Window { /* sets, or unavailable(reason) */ }  // new(Vec<WindowSet>); unavailable(MissingReason)
pub struct Catalog(/* Option<HashSet<String>> */);       // known(impl IntoIterator<Item = String>); unknown(); contains(&str) -> Option<bool>
pub struct Slots(/* BTreeMap<String, bool> */);          // new(); insert; get(&str) -> Option<bool>; iter(); is_empty()
pub enum MissingReason { Disabled, Store, NoSession, Truncated, Unknown, Retention, NoKey, Catalog }   // as_str(): the metric label; ALL: [MissingReason; 8]
pub struct Missing { pub slot: String, pub part: Part, pub reason: MissingReason }
pub struct Evaluation { pub slots: Slots, pub missing: Vec<Missing> }
pub struct ToolSet { pub names: BTreeSet<String>, pub catalog: bool, pub every: bool }

impl Predicate {
    pub fn tools(&self) -> ToolSet;                       // the tool set of the whole predicate (rules below)
    /// The per-field walk of spec P3: one `(ToolSet, &FieldTest)` for each `FieldTest`, where the
    /// `ToolSet` is the tool set that holds at that test (the same `AllOf`/`AnyOf` rules as `tools()`).
    /// The only walk. `Task 5` `FieldPlan::of` calls it.
    pub fn field_tests(&self) -> Vec<(ToolSet, &FieldTest)>;
}
impl Condition {
    pub fn tools(&self) -> ToolSet;                       // = self.shape's predicate .tools()
    pub fn bind_digests(&self, org: Uuid, d: &dyn HistoryDigester) -> Condition;   // Unbound -> Digests(d.all(..))
}
pub fn evaluate(conditions: &[Condition], window: &Window, hold: &[Event], index: usize, now_ms: u64, catalog: &Catalog) -> Evaluation;
```

Rules (spec P1; plan decision D-C):
- `Predicate::tools()` and `Predicate::field_tests()` share one walk (no second walk anywhere). `Condition::paths()` does not exist. `FieldPlan::of` (Task 5) takes the `sum_within` path from the shape and the test paths from `field_tests()`.
- `tools()`: `Tool(NameKey(k))` → `{k}`; `AnyCatalogTool` → `catalog: true`; `AllOf` → intersection, where a side with `every` is neutral; `AnyOf` → union; `Not`, `EventKind` and `Field` → `every`.
- Window of a condition: an event counts when `ts_ms >= now_ms.saturating_sub(window_ms)`, also when `ts_ms > now_ms`. The events of the condition's scope come from the `WindowSet`s of that scope, de-duplicated by id, plus `hold[0..=index]`.
- The sets that a condition reads: the sets of its scope whose `tool` is in `tools().names`, and every catalog tool when `catalog`. The condition requires the `Every` set only when `ToolSet.every` is true, or when `catalog` is true and the catalog is unknown (then it reads all sets of the scope). A named-tool condition does not require the `Every` set. `ReadPlan::of` (Task 5) uses the same rule. A read set with `truncated`, or with `mark_ms >= window start`, makes the slot missing (`Truncated`). A required set that is not in the window makes the slot missing (`Store`).
- `FieldTest` result table: spec P1. `Literal::Unbound`, or `Digests` with no entry, makes the slot missing (`NoKey`) before any event is read.
- `AllOf`, `AnyOf`, `Not`: Kleene logic.
- Shapes: spec P1 (`formerly`, `count_within` with the range rule, `sum_within` with plan decision D-C: an event whose predicate is unknown makes the slot missing whatever its value, negative adds 0, saturating sum).
- Order of the pre-checks: `window_ms > RETENTION_MS` → `Retention`; unbound digest → `NoKey`; `AnyCatalogTool` in the predicate and `catalog` unknown → `Catalog`; `Window::unavailable(r)` → `r`; then the set checks; then the events (`Unknown`).

- [ ] **Step 1: Write the failing tests**

One helper `ev(ts_s, kind, tool, fields)` and `win(scope, events)`. The tests (assertions in code; values exact):

```rust
#[test] fn formerly_is_true_on_one_true_event_and_missing_on_one_unknown_event() {}
#[test] fn the_window_is_closed_and_a_future_event_is_inside_every_window() {
    // window 600_000, now 1_000_000: an event at 400_000 counts, 399_999 does not, 5_000_000 counts
}
#[test] fn count_within_takes_the_value_only_when_every_possible_count_agrees() {
    // <= 3 with 2 true + 1 unknown -> Some(true); with 3 true + 1 unknown -> missing(Unknown); >= 1 with 1 true + 5 unknown -> Some(true)
}
#[test] fn sum_within_adds_longs_adds_zero_for_absent_and_negative_and_saturates() {
    // 60 + Absent + (-30) + 50 <= 110 -> true; i64::MAX + 1 saturates at i64::MAX
}
#[test] fn a_non_long_value_at_a_sum_path_makes_the_slot_missing() { /* Digest, Mistyped, Fields::Unparsed on a true event -> Missing{reason: Unknown} */ }
#[test] fn a_free_text_call_of_another_tool_does_not_remove_a_sum_slot() { /* predicate Tool(send); an unparsed `search` event is false, so the slot has a value */ }
#[test] fn a_path_that_is_not_in_the_event_is_false_and_adds_zero() {}
#[test] fn field_tests_follow_the_three_valued_table() { /* one assertion per row of the spec P1 table */ }
#[test] fn a_string_value_against_a_long_literal_is_unknown() { /* formerly(transfer{amount > 1000}) with a Digest at `amount` -> Missing{reason: Unknown}; a Long 5 against a digest literal -> Missing{reason: Unknown} */ }
#[test] fn kleene_logic_for_all_of_any_of_and_not() { /* the 9 pairs of AllOf and AnyOf over {T, F, U}, and Not */ }
#[test] fn the_call_at_index_i_sees_the_hold_up_to_i_only() { /* hold of 3 sends of 2000, <= 5000: index 0, 1 true; index 2 false */ }
#[test] fn the_events_of_a_window_and_the_hold_are_counted_once_by_id() {}
#[test] fn a_key_scope_condition_reads_the_key_sets() {}
#[test] fn a_mark_at_or_after_the_window_start_makes_the_slot_missing() { /* mark == start -> Truncated; mark == start - 1 -> value */ }
#[test] fn a_truncated_set_makes_only_the_conditions_that_read_it_missing() { /* send set truncated: a search condition keeps its value; an Every condition is missing */ }
#[test] fn a_window_longer_than_the_retention_is_missing() { /* window_ms = RETENTION_MS + 1 -> Retention; = RETENTION_MS -> value */ }
#[test] fn an_unbound_or_empty_digest_literal_is_missing_with_no_key() {}
#[test] fn any_catalog_tool_with_an_unknown_catalog_is_missing() {}
#[test] fn an_event_under_the_previous_key_matches_and_an_unknown_key_is_unknown() {
    // literal Digests[(k1, d1), (k0, d0)]: an event under k0 with d0 -> true; under k9 -> Unknown
}
#[test] fn a_sum_event_with_an_unknown_predicate_makes_the_slot_missing_even_with_a_long() { /* Field test on an unknown key id, Long 5 at the sum path: Missing{reason: Unknown} */ }
#[test] fn a_named_tool_condition_does_not_require_the_every_set() { /* window with only the send set: slot has a value. An Not(..) condition with no Every set: Missing{reason: Store}. AnyCatalogTool with an unknown catalog requires Every */ }
#[test] fn field_tests_gives_each_field_test_its_own_tool_set() { /* AllOf(Tool(send), Field(a)) -> [({send}, a)]; AnyOf(AllOf(Tool(x), Field(a)), Field(b)) -> [({x}, a), (every, b)] */ }
#[test] fn missing_reason_as_str_gives_the_metric_labels_and_all_lists_each_once() { /* the 8 labels of the Global Constraints; ALL.len() == 8, no duplicate */ }
#[test] fn part_as_str_gives_floor_and_tenant() { assert_eq!((Part::Floor.as_str(), Part::Tenant.as_str()), ("floor", "tenant")); }
#[test] fn catalog_contains_is_none_when_unknown() { /* known(["a"]).contains("a") == Some(true), ("b") == Some(false); unknown().contains("a") == None */ }
#[test] fn tools_walk_intersects_all_of_and_unites_any_of() {
    // AllOf(Tool(send), Field) -> {send}; AnyOf(Tool(a), Tool(b)) -> {a, b}; Not(Tool(a)) -> every; AllOf(AnyCatalogTool, Tool(a)) -> {a}, catalog
}
#[test] fn bind_digests_replaces_every_unbound_literal() { /* nested AllOf/Not; Long literals stay */ }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-kernel --features tool-policy --test history_evaluate`
Expected: FAIL to compile (`evaluate` not found).

- [ ] **Step 3: Implement the types in `history/types.rs`, `Predicate::tools()`/`Predicate::field_tests()`/`Condition::tools()`/`bind_digests` in `history/tools.rs`, and `evaluate` in `history/evaluate.rs`**

Use an internal `enum Tri { True, False, Unknown }`. Evaluate each condition on its own; one condition's missing slot never touches another slot.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-kernel --features tool-policy`
Expected: PASS.

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/libs/paigasus-kernel
git commit -m "feat(rs): add the history condition types and the three-valued evaluator (SMA-753)"
```

---

### Task 3: `decide_with_slots` in the kernel

**Files:**
- Modify: `rs/crates/libs/paigasus-kernel/src/tool_policy/decide.rs` (`context`, `request`, new `decide_with_slots`, `RESERVED_CONTEXT_NAMES`)
- Modify: `rs/crates/libs/paigasus-kernel/src/tool_policy/mod.rs` (re-exports)
- Modify: `rs/crates/libs/paigasus-kernel/src/tool_policy/optimistic.rs` (test only)
- Test: `rs/crates/libs/paigasus-kernel/tests/tool_policy_decide.rs`

**Interfaces:**
- Consumes: `Slots` (Task 2).
- Produces:

```rust
pub const RESERVED_CONTEXT_NAMES: [&str; 7] = ["input", "input_text", "findings", "max_severity", "call", "request", "session"];
#[must_use]
pub fn decide_with_slots(call: &ToolInvocation, facts: &Facts, findings: &Findings, slots: &Slots, policies: &ToolPolicies) -> Verdict;
```

`decide(call, facts, findings, policies)` stays and calls `decide_with_slots` with `&Slots::new()`. Each slot becomes `RestrictedExpression::new_bool` at the top of the context. A slot whose name is in `RESERVED_CONTEXT_NAMES` is skipped (no panic, no `debug_assert`). The optimistic context (`request(None, ..)`) gets no slots.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_slot_reaches_the_context_and_a_missing_slot_errors() {
    let p = tenant(r#"@id("all") permit(principal, action, resource); @id("cap") forbid(principal, action, resource) when { context.over };"#);
    let mut over = Slots::new(); over.insert("over", true);
    assert_eq!(decide_with_slots(&call("x"), &facts(), &Findings::new(), &over, &p).decision, Effect::Deny);
    let mut under = Slots::new(); under.insert("over", false);
    assert_eq!(decide_with_slots(&call("x"), &facts(), &Findings::new(), &under, &p).decision, Effect::Allow);
    let v = decide(&call("x"), &facts(), &Findings::new(), &p);
    assert_eq!(v.decision, Effect::Deny);
    assert!(v.errored_forbid, "a missing slot errors the forbid");
}

#[test]
fn a_reserved_slot_name_is_skipped_and_does_not_panic() {
    for name in RESERVED_CONTEXT_NAMES {
        let mut s = Slots::new(); s.insert(name, true);
        let _ = decide_with_slots(&call("x"), &facts(), &Findings::new(), &s, &permit_all());
    }
}

#[test]
fn a_permit_on_a_missing_slot_does_not_allow() {}
```

In `optimistic.rs` tests: `a_forbid_that_reads_a_slot_is_never_optimistic` (design D6: `decide_tool` with a forbid `when { context.slot_x }` and no other forbid gives no optimistic allow that drops it).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-kernel --features tool-policy --test tool_policy_decide`
Expected: FAIL to compile (`decide_with_slots` not found).

- [ ] **Step 3: Implement `decide_with_slots` in `decide.rs`**

`context` and `request` take `slots: &Slots`. Push each non-reserved slot as a pair before `Context::from_pairs`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-kernel --features tool-policy`
Expected: PASS.

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/libs/paigasus-kernel
git commit -m "feat(rs): add decide_with_slots, which puts the history slots into the Cedar context (SMA-753)"
```

---

### Task 4: The proto mapping and the load validation in gateway-core

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/src/policy/history/mod.rs`, `history/mapping.rs`
- Modify: `rs/crates/libs/paigasus-gateway-core/src/policy/mod.rs`, `src/lib.rs` (exports)
- Modify: `rs/crates/libs/paigasus-gateway-core/Cargo.toml` (`paigasus-proto` moves from `[dev-dependencies]` to `[dependencies]`)
- Test: `rs/crates/libs/paigasus-gateway-core/src/policy/history/mapping.rs` (unit tests)

**Interfaces:**
- Consumes: the kernel types of Task 2, `RESERVED_CONTEXT_NAMES` (Task 3), `normalise`.
- Produces:

```rust
pub const MAX_PREDICATE_DEPTH: usize = 32;
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("history condition {index} is malformed: {reason}")]
pub struct MalformedCondition { pub index: usize, pub reason: &'static str }
/// Map and validate the conditions of one set. `other_slots` holds the slot names of the floor
/// when `part` is Tenant (a clash makes the tenant set malformed), and is empty for the floor.
pub fn map_conditions(part: Part, pb: &[paigasus_proto::paigasus::iam::v1::HistoryCondition], max_window_ms: u64, other_slots: &BTreeSet<String>) -> Result<Vec<Condition>, MalformedCondition>;
```

Literals map to `Literal::Long` or `Literal::Unbound`. A decimal literal parses with the Cedar grammar: optional `-`, digits, `.`, 1 to 4 digits, inside the `i64` range after × 10,000.

Malformed forms (design D6, spec P5). Each `reason` is a fixed string:

| reason | form |
|---|---|
| `shape_unset` | `shape` is `None` |
| `node_unset` | a predicate `node` is `None` |
| `value_unset` | a `LiteralValue.value` is `None`, or `FieldComparison.value` is `None` |
| `pair_side_missing` | a `PredicatePair` side or a `negation` body is `None` |
| `enum_unknown` | scope, event kind or op is `UNSPECIFIED` or unknown |
| `tool_match` | a `ToolMatch` has both fields or neither |
| `name_key_not_normal` | `name_key != normalise(name_key)` |
| `path` | an empty path or field path, an empty segment, a segment with `.` |
| `window` | `window_ms == 0` or `window_ms > max_window_ms` |
| `slot_name` | not a Cedar identifier (`[A-Za-z_][A-Za-z0-9_]*`), or in `RESERVED_CONTEXT_NAMES` |
| `slot_duplicate` | two conditions with one slot |
| `slot_clash` | a tenant slot equals a floor slot |
| `depth` | a predicate deeper than `MAX_PREDICATE_DEPTH` (an `all_of`, `any_of` or `negation` adds one level, as `paigasus_iam_core::tool_policy::check` counts) |
| `decimal` | a decimal literal that does not parse |
| `op_not_eq` | an op other than `EQ` on a string, bool or decimal literal |

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn every_node_and_every_literal_kind_maps() {
    // one condition per shape; predicates: event_kind, tool(name_key), tool(any_catalog_tool), field(long/string/bool/decimal), all_of, any_of, negation
    // assert the exact kernel value, e.g. decimal "1.5" -> Literal::Unbound(Scalar::Decimal(15_000)), "-0.0001" -> Decimal(-1)
}

#[test]
fn each_malformed_form_is_refused_with_its_reason() {
    for (reason, pb) in malformed_cases() { // one case per row of the table above
        assert_eq!(map_conditions(Part::Tenant, &[pb], 86_400_000, &floor_slots()).unwrap_err().reason, reason);
    }
}

#[test]
fn a_predicate_of_depth_32_loads_and_33_is_refused() {}

#[test]
fn a_decimal_with_five_fraction_digits_or_out_of_range_is_refused() {
    for bad in ["1.23456", "1", "1.", ".5", "922337203685477.5808", "1e2"] { /* reason "decimal" */ }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway-core policy::history`
Expected: FAIL to compile (`map_conditions` not found).

- [ ] **Step 3: Implement `map_conditions` in `policy/history/mapping.rs`**

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway-core`
Expected: PASS.

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/libs/paigasus-gateway-core
git commit -m "feat(rs): map and validate the IAM history conditions in gateway-core (SMA-753)"
```

---

### Task 5: The field plan, the read plan, the history policy, and slots in the call decisions

**Files:**
- Create: `rs/crates/libs/paigasus-gateway-core/src/policy/history/plan.rs`
- Modify: `rs/crates/libs/paigasus-gateway-core/src/policy/calls.rs` (`decide_calls_with`, `decide_with`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/policy/snapshot.rs` (`PolicySet::Available.floor_only`, `PolicySnapshot.history`, `with_tenant_unavailable`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/policy/plan.rs` (tests only: the request-side differential)
- Modify: every `PolicySet::Available { .. }` construction site in `paigasus-gateway` (`application/tool_policy/request.rs`, `decider.rs` tests, `testing.rs`, `tests/*.rs`): pass `FloorEntry::floor_only`
- Modify: the callers of `decide_calls_with` (`application/tool_policy/decider.rs`): pass `&[]` for now

**Interfaces:**
- Consumes: Tasks 2 and 4. `FieldPlan::of` calls `Predicate::field_tests()` (Task 2) and adds the `sum_within` path from the shape. It has no walk of its own.
- Produces:

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldPlan { /* per tool: PathSet; every tool: PathSet */ }
impl FieldPlan {
    pub fn of(conditions: &[Condition], catalog: &Catalog) -> Self;   // spec P3 path walk; `catalog` flag -> each catalog tool, or every tool when the catalog is unknown
    pub fn union(&self, other: &FieldPlan) -> FieldPlan;
    pub fn paths_for(&self, tool: &str) -> PathSet;
    pub fn is_empty(&self) -> bool;
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadSet { pub scope: Scope, pub tool: SetTool }
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadPlan { pub w_max_ms: u64, pub sets: Vec<ReadSet> }   // sorted, no duplicates
impl ReadPlan { pub fn of(conditions: &[Condition], catalog: &Catalog) -> Option<ReadPlan>; }   // None with no condition
#[derive(Debug)]
pub struct HistoryPolicy { /* bound conditions (floor first), Arc<FieldPlan>, Option<ReadPlan>, Catalog */ }
impl HistoryPolicy {
    pub fn build(floor: &[Condition], tenant: &[Condition], org: Uuid, catalog: Catalog, stored: Option<&FieldPlan>, d: &dyn HistoryDigester) -> Self;
    pub fn conditions(&self) -> &[Condition];
    pub fn has_part(&self, part: Part) -> bool;
    pub fn fields(&self) -> &Arc<FieldPlan>;   // HistoryPolicy stores Arc<FieldPlan>; the interceptor clones the Arc (Task 8)
    pub fn read(&self) -> Option<&ReadPlan>;
    pub fn catalog(&self) -> &Catalog;
}
// snapshot.rs
PolicySet::Available { policies: Arc<ToolPolicies>, floor_only: Arc<ToolPolicies>, tenant: TenantPart, facts: Facts }
pub history: Option<Arc<HistoryPolicy>>          // new pub field of PolicySnapshot; `with_history(self, Arc<HistoryPolicy>) -> Self`
pub fn with_tenant_unavailable(&self) -> PolicySnapshot;   // Available -> policies = floor_only, tenant = Unavailable; Unavailable stays
// calls.rs
pub fn decide_calls_with(snapshot: &PolicySnapshot, turn: &ToolTurn, calls: &[ToolCall], scans: &[CallScan], extra: &Findings, slots: &[Slots], now: &dyn Fn() -> Instant) -> Vec<CallOutcome>;
```

Rules:
- `slots` is empty (no slots for any call) or holds one entry per call. Another count is a caller defect: debug panic, release uses no slots (the same pattern as `scans`).
- `ReadPlan::of`: one `ReadSet` per (scope of a condition, tool): a name gives `Tool(name)`, the `catalog` flag gives `Tool(t)` for each catalog tool (`Every` when the catalog is unknown), `every` gives `Every`. A condition with only named tools adds no `Every` set (the Task 2 rule). `w_max_ms` is the largest `window_ms`.
- `HistoryPolicy::build` binds every condition with `bind_digests(org, d)`. Its `FieldPlan` is the plan of all conditions, united with `stored` when it is `Some`.
- `policy/plan.rs` (request side) keeps `decide()` with no slots (spec P2).

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn the_field_plan_holds_paths_per_tool_and_for_every_tool() { /* AllOf(Tool(send), Field(amount)) -> send: {amount}; Not(Field(x)) -> every: {x}; sum path under Tool(send) -> send */ }
#[test] fn any_catalog_tool_paths_go_to_each_catalog_tool_or_every_tool() {}
#[test] fn the_read_plan_lists_each_scope_and_tool_once_and_the_largest_window() {}
#[test] fn no_condition_gives_no_read_plan() { assert_eq!(ReadPlan::of(&[], &Catalog::unknown()), None); }
#[test] fn history_policy_binds_the_literals_of_both_parts() {}
#[test] fn each_call_gets_its_own_slots() { /* two calls, slots [{over: true}, {over: false}], forbid when context.over: Block, Allow */ }
#[test] fn with_tenant_unavailable_decides_with_the_floor_only() {}
#[test] fn field_plan_union_merges_per_tool_and_every_tool_paths() {}
#[test] fn field_plan_is_empty_only_with_no_path() { assert!(FieldPlan::default().is_empty()); }
#[test] fn history_policy_has_part_is_true_only_for_a_part_with_conditions() {}
#[test] fn with_history_sets_the_snapshot_history_and_keeps_the_rest() {}
#[test] fn the_field_plan_of_a_named_tool_condition_adds_no_every_tool_path() { /* the plan comes from field_tests(), with the same sets as the evaluator reads */ }
```

In `policy/plan.rs` tests (spec P2): `a_slot_forbid_errors_in_both_decisions_and_is_no_difference` (`forbid when { context.s }`: no result replacement) and `a_findings_and_slot_forbid_is_replaced` (`forbid when { context.findings.contains("secret_in_args") && context.s }` with the finding on the result: the result is replaced).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway-core`
Expected: FAIL to compile (`FieldPlan` not found).

- [ ] **Step 3: Implement**

Add the types in `policy/history/plan.rs`. Change `decide_with` to call `decide_with_slots` with the slots of call `i`. Add `floor_only` to `PolicySet::Available` and fix every construction site in gateway-core and in `paigasus-gateway` (they hold a `FloorEntry`, so they pass `Arc::clone(&floor.floor_only)`).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway-core && cargo nextest run -p paigasus-gateway --lib`
Expected: PASS.

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/libs/paigasus-gateway-core rs/crates/services/paigasus-gateway
git commit -m "feat(rs): add the field plan, the read plan and per-call slots to the call decisions (SMA-753)"
```

---

### Task 6: The history key and the HMAC digester

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/adapters/history/digest.rs`
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/history/mod.rs` (export)
- Modify: `rs/crates/services/paigasus-gateway/src/config.rs:244-300` (`ToolHistoryConfig`: two fields, `validate`)
- Test: `digest.rs` unit tests, `config.rs` tests

**Interfaces:**
- Consumes: `HistoryDigester`, `KeyId`, `Digest`, `Scalar`, `FieldPath` (Task 1).
- Produces:

```rust
pub struct HistoryKey { /* id: KeyId, key: SecretString */ }   // Debug prints the id only
impl HistoryKey { pub fn new(key: SecretString) -> Result<Self, String>; pub fn id(&self) -> KeyId; }   // >= 32 bytes; id = first 8 bytes of HMAC(key, "paigasus-history-kid")
pub struct HmacHistoryDigester { /* current: Option<HistoryKey>, previous: Option<HistoryKey> */ }
impl HmacHistoryDigester {
    pub fn new(current: Option<HistoryKey>, previous: Option<HistoryKey>) -> Result<Self, String>;   // refuses previous without current, and equal ids
    pub fn keyless() -> Self;
    pub fn from_config(c: &ToolHistoryConfig) -> Result<Self, String>;
}
impl HistoryDigester for HmacHistoryDigester { /* spec P3 digest input; cut to 16 bytes */ }
// config.rs, ToolHistoryConfig: #[serde(skip_serializing)] pub digest_key: Option<SecretString>, #[serde(skip_serializing)] pub previous_digest_key: Option<SecretString>
```

`ToolHistoryConfig::validate` calls `HmacHistoryDigester::from_config` and prefixes the error with `tool_history.digest_key: ` or `tool_history.previous_digest_key: `. This task does not touch `runtime.rs`. The wiring of the digester into the runtime is in Task 7 (cache), and the boot warning for a missing key is in Task 7 too, because Task 7 is the first task that builds the digester in `runtime.rs` (a warning in Task 6 would have no call site). Task 9 passes the same digester to the history. The `base64` dependency is added in Task 9, where the digest encoding first uses it.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn the_key_id_is_derived_from_the_key_bytes() { /* two keys -> two ids; one key twice -> one id; id hex is 16 chars */ }
#[test] fn the_digest_binds_the_org_the_path_and_the_value() { /* change any one of the three -> another digest; len 16 */ }
#[test] fn a_literal_gets_both_digests_and_a_call_value_gets_the_current_one() { /* all() len 2, current first; current() == all()[0] */ }
#[test] fn keyless_gives_no_digest() {}
#[test] fn a_short_key_a_previous_without_current_and_equal_ids_are_refused() {}
#[test] fn debug_never_prints_a_key() { /* format!("{:?}", digester) does not contain the key text */ }
```

In `config.rs` tests: `the_history_keys_come_from_the_environment` (figment with `GATEWAY_TOOL_HISTORY__DIGEST_KEY` and `GATEWAY_TOOL_HISTORY__PREVIOUS_DIGEST_KEY` set gives both, and `serde_json::to_string(&cfg)` holds neither), and `a_pr1_config_with_no_key_still_validates`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib history::digest config::`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

Use `Hmac<Sha256>` as `application/decision_log.rs` `DigestKey::digest` does.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib`
Expected: PASS.

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/services/paigasus-gateway
git commit -m "feat(rs): add the history digest key, its rotation pair and the HMAC digester (SMA-753)"
```

---

### Task 7: The policy source and the cache carry the conditions; remove the refusal fields

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/domain/tool_policy.rs` (`PolicyText`, `TenantPolicies`: new fields; remove `refused`)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/policy_source/iam.rs:120-170` (copy `conditions`, `max_window_ms`)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/policy_source/file.rs` (empty conditions)
- Modify: `rs/crates/services/paigasus-gateway/src/application/tool_policy/cache.rs` (`FloorEntry`, `TenantData`, `tenant_set`, `build`, `load`; remove `store_refused`, the `refused` branch and their tests; new `CompiledTenant`, `last_known`, `with_digester`)
- Modify: `rs/crates/services/paigasus-gateway/src/application/tool_policy/request.rs` (consume `CompiledTenant`; put `HistoryPolicy` on the snapshot)
- Modify: `rs/crates/services/paigasus-gateway/src/runtime.rs` (build the digester once with `HmacHistoryDigester::from_config`; `PolicyCache::with_digester`; with no key, log one boot `warn!`: `"[tool_history] has no digest_key: conditions on string, bool or decimal values have no value (missing slot, reason no_key)"`)
- Modify: `rs/crates/services/paigasus-gateway/src/application/tool_policy/testing.rs` (`FakeSource` can carry conditions)
- Test: `cache.rs`, `iam.rs`, `request.rs` tests

**Interfaces:**
- Consumes: `map_conditions` (Task 4), `HistoryPolicy`, `FieldPlan` (Task 5), `HmacHistoryDigester` (Task 6).
- Produces:

```rust
// domain/tool_policy.rs
pub struct PolicyText { pub text: String, pub version: String, pub conditions: Vec<pb::HistoryCondition>, pub max_window_ms: u64 }
pub struct TenantPolicies { pub text, pub catalog, pub version, pub settings, pub conditions: Vec<pb::HistoryCondition>, pub max_window_ms: u64 }   // `refused` removed
// cache.rs
pub struct FloorEntry { pub floor: Arc<FloorPolicies>, pub floor_only: Arc<ToolPolicies>, pub conditions: Arc<Vec<Condition>> }   // unbound floor conditions
impl FloorEntry { pub fn has_conditions(&self) -> bool; }
#[derive(Debug, Clone)]
pub struct CompiledTenant { pub policies: Arc<ToolPolicies>, pub history: Arc<HistoryPolicy> }
pub fn tenant_until(..) -> Lookup<CompiledTenant>;                 // and `tenant`, `tenant_with_settings_until`
pub fn with_digester(self: Arc<Self>, d: Arc<dyn HistoryDigester>) -> Arc<Self>;   // builder; default HmacHistoryDigester::keyless()
pub fn floor_history(&self, org: Uuid, floor: &FloorEntry) -> Arc<HistoryPolicy>;   // the floor alone, bound to org, with the last known catalog and FieldPlan of org (plan decision D-F)
```

Rules:
- The `version` token does not change: the conditions change only with the set version.
- Floor load: `map_conditions(Part::Floor, ..)` failure is a compile failure of the floor (the floor is unavailable, `Err(None)` in `build`).
- Tenant load and recompile: `map_conditions(Part::Tenant, .., &floor slot names)` failure is a compile failure of the tenant set (`compiled: None`, the settings stay). Recompile after a floor change also re-checks the slot clash and rebinds.
- After a good tenant load, `last_known[org] = (catalog, FieldPlan)`. Idle eviction removes the org from `last_known` too.
- A cache test asserts `FloorEntry::has_conditions` for a floor with and without a condition (Task 11 is its production caller).
- The request side: an `Evaluated` set gets the tenant `history`; mode `off` and an unavailable tenant get `floor_history(org, floor)`. `PolicySnapshot.history` is `None` when that policy has no condition and an empty `FieldPlan`.

- [ ] **Step 1: Write the failing tests**

In `iam.rs` tests: `the_iam_source_copies_the_conditions_and_the_window` (a set with one condition gives `conditions.len() == 1` and `max_window_ms == 86_400_000`, for the floor and the tenant).

In `cache.rs` tests:

```rust
#[tokio::test] async fn a_tenant_set_with_conditions_compiles_with_its_history_policy() {}
#[tokio::test] async fn a_malformed_tenant_condition_makes_the_tenant_unavailable_and_keeps_the_settings() {}
#[tokio::test] async fn a_malformed_floor_condition_makes_the_floor_unavailable() {}
#[tokio::test] async fn a_tenant_slot_equal_to_a_floor_slot_makes_the_tenant_malformed_not_the_floor() {}
#[tokio::test] async fn a_new_floor_with_a_clashing_slot_makes_the_stored_tenant_unavailable() {}
#[tokio::test] async fn the_floor_history_uses_the_last_known_catalog_and_paths_of_the_org() {
    // good load (catalog [send], path amount), then the source fails: floor_history(org) has catalog {send} and fields for send: {amount}
}
#[tokio::test] async fn an_org_with_no_good_load_has_an_unknown_catalog() {}
```

Delete the tests that set `refused` (`FakeSource::set_refused`, the R5 tests) and the field.

In `request.rs` tests: `the_snapshot_carries_the_tenant_history_policy` and `mode_off_carries_the_floor_history_policy`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib tool_policy:: policy_source::`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

`grep -rn "refused" rs/crates/services/paigasus-gateway` must show no field, branch or test of the refusal after this step (spec P12).

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway`
Expected: PASS (Docker suites skip without Docker).

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/services/paigasus-gateway
git commit -m "feat(rs): carry the history conditions from the policy source into the cache, and remove the refusal fields (SMA-753)"
```

---

### Task 8: Fields on the report entries, and the hold intent (gateway-core)

**Files:**
- Modify: `rs/crates/libs/paigasus-gateway-core/src/decision_log/record.rs:265-320` (`ReportedCall.fields`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/session.rs` (intent API)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/interceptor.rs` (`with_field_plan`, `apply_decision`)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/body.rs` (`reported` takes the plan)
- Modify: `rs/crates/libs/paigasus-gateway-core/src/decision_log/build.rs` (test: no field in a record)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/http/interception.rs` (pass `Arc::clone(ctx.policy.history.fields())` to `with_field_plan`, and `&**` of it to `reported`)

**Interfaces:**
- Consumes: `extract_fields`, `ExtractedFields` (Task 1), `FieldPlan` (Task 5).
- Produces:

```rust
// record.rs
pub fields: Option<ExtractedFields>,     // new field of ReportedCall; `with_fields(self, Option<ExtractedFields>) -> Self`; None when no plan
// session.rs
pub fn record_intent(&self, entries: Vec<(CallFacts, Uuid)>);     // appends
pub fn claim_intent(&self, call: &CallFacts) -> Option<Uuid>;    // oldest unclaimed entry with equal facts; removes it (plan decision D-D)
pub fn forget_intent(&self, ids: &[Uuid]);                       // the decider's own ids, when it returns
// interceptor.rs
pub fn with_field_plan(self, plan: Option<Arc<FieldPlan>>) -> Self;
// body.rs
pub fn reported(&self, decided: &[DecidedCall], basis: Basis, plan: Option<&FieldPlan>) -> Vec<ReportedCall>;
```

`fields` is `extract_fields(&call input, &plan.paths_for(&normalise(&call.display_name())))`, computed where `CallFacts::of(call)` runs. The decision record builder ignores `fields`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn a_reported_call_carries_the_fields_of_its_plan() {}          // interceptor.rs, a decider error path
#[test] fn a_body_report_carries_the_fields_of_its_plan() {}            // body.rs
#[test] fn no_plan_gives_no_fields() {}
#[test] fn the_record_holds_no_field() { /* build.rs: the record JSON of a ReportedCall with fields has no "f" and no argument value */ }
#[test] fn equal_call_facts_claim_their_intents_in_order() {
    // record_intent([(c0, a), (c0, b)]); claim(c0) == Some(a); claim(c0) == Some(b); claim(c0) == None
}
#[test] fn a_forgotten_intent_cannot_be_claimed() {}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway-core`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

The intent list is a `Mutex<Vec<(CallFacts, Uuid)>>` with the `PoisonError::into_inner` pattern of `reserved`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway-core && cargo nextest run -p paigasus-gateway --lib`
Expected: PASS. `git status` shows no change under `rs/crates/libs/paigasus-gateway-core/tests/golden/`.

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/libs/paigasus-gateway-core rs/crates/services/paigasus-gateway/src/adapters/http/interception.rs
git commit -m "feat(rs): carry the stored fields on report entries and keep the hold intent in the session (SMA-753)"
```

---

### Task 9: The store port, write side: fields, the append verdict, the reserve link and the settle guard

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/domain/tool_history.rs` (types; re-export kernel `RETENTION_MS`)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/history/memory.rs`, `adapters/history/redis.rs` (encoding, scripts)
- Modify: `rs/crates/services/paigasus-gateway/src/application/tool_history.rs` (`settle_plan`, `reserve` sends empty call fields (Task 10 fills them), `run_settle`; `ToolHistory::new` takes the digester)
- Modify: `rs/crates/services/paigasus-gateway/src/runtime.rs` (pass the one digester)
- Modify: `rs/crates/services/paigasus-gateway/Cargo.toml` (`base64 = { workspace = true }`; the digest encoding in `adapters/history/` is its first use, so `:machete` stays green)
- Modify: `rs/crates/services/paigasus-gateway/tests/support/history_contract.rs`, `tests/history_store_memory.rs`, `tests/history_store_redis.rs`

**Interfaces:**
- Consumes: Tasks 1, 6, 8.
- Produces (domain):

```rust
pub use paigasus_kernel::tool_policy::history::RETENTION_MS;    // replaces the local const; same value
pub struct StoredEvent { pub id, pub ts_ms, pub kind, pub tool, pub verdict, pub fields: Fields, pub key: Option<KeyId> }
pub struct CallEvent { pub id: Uuid, pub tool: String, pub call_id: Option<String>, pub item_id: Option<String>, pub fields: Fields, pub key: Option<KeyId> }
pub struct AppendCall { pub id: Uuid, pub tool: String, pub verdict: EventVerdict, pub call_id: Option<String>, pub item_id: Option<String>, pub fields: Fields, pub key: Option<KeyId> }   // replaces ForwardedCall
pub struct AppendRequest { pub histories, pub turn, pub calls: Vec<AppendCall>, pub now_ms }
// SettleItem keeps its fields; a Deny item now carries call_id and item_id (the delete guard)
// application
pub fn settle_plan(reported: &[ReportedCall], reserved: &dyn Fn(Uuid) -> bool, claim: &mut dyn FnMut(&CallFacts) -> Option<Uuid>, fields: &dyn Fn(&ReportedCall) -> (Fields, Option<KeyId>), mint: &mut dyn FnMut() -> Uuid) -> SettlePlan;
pub struct SettlePlan { pub settle: Vec<SettleItem>, pub append: Vec<AppendCall> }
pub fn new(store: Arc<dyn SessionStore>, clock: Arc<dyn Clock>, entropy: Arc<dyn Entropy>, deadline: Duration, digester: Arc<dyn HistoryDigester>) -> Self;   // ToolHistory
```

Rules:
- Encoding: plan Global Constraints "Event body". A reader that finds no `f` and no `b` reads `Fields::Values(empty)` (a PR 1 event: every path "not in the event").
- Result events (spec P3): the write script copies `f`, `kid` and `b` from the call body that the guard reads into the result body.
- Reserve link (spec P8): for each call, `c:` under `call_id` and under `item_id` when `Some`, next to `d:`. A `c:` key that exists and whose `e` is another event is kept.
- Settle (spec P8): `Allow` keeps (or writes) the link as in PR 1. `Deny` deletes a `c:` key only when its `e` equals the event id.
- Append (spec P9): writes the verdict of the call. Only `Allow` writes `c:` links. An id that a reserve wrote keeps one event (plan decision D-E).
- `settle_plan` (spec P9, plan decision D-D): the id is `decision_id`, else `claim(call facts)`. A reserved id gives a settle (Allow or Deny, both with the call ids). An unreserved id, or no id, with call facts gives an append with the entry verdict (`Allow` → allow, `Block` → deny) and its fields; no id mints one. No call facts gives nothing.
- `reserve` in this task sends `Fields::Values(empty)` and key id `None` for each `CallEvent`. The `policy` parameter and the field plan start in Task 10, which fills the fields from `policy.fields().paths_for(tool)`.
- Accepted transient state: between Task 9 and Task 11 nothing records hold intents, so `claim_intent` finds nothing. A timed-out hold under `allow` can count twice, and an interceptor block appends. Task 11 restores the end state with its own tests. In this task, run `cargo nextest run -p paigasus-gateway --test tool_history` and change only a test that asserts the old R10 count. Change no other test.
- `ToolHistory::settle` uses `session.claim_intent` and digests `ReportedCall.fields` with `into_fields(org, digester)`; `None` fields give `Fields::Values(empty)`.

- [ ] **Step 1: Write the failing tests**

New contract cases in `history_contract.rs` (each `pub async fn …(h: &impl Harness)`, added to `run_all`):

```rust
a_reserved_event_keeps_its_fields_and_key_id            // events() returns the Fields and KeyId that the reserve sent
an_unparsed_call_is_stored_as_unparsed
a_pr1_body_reads_as_no_fields                            // Harness::put_raw_body writes {"v":1,"id",..} with no f
a_result_event_copies_the_fields_of_its_call
the_reserve_writes_the_call_links_of_each_call           // call id and item id; lookup finds them
the_reserve_keeps_a_link_of_another_event                // call_0 linked to event A, a reserve of event B with call_0: the link still names A
a_deny_settle_deletes_only_its_own_link
an_allow_settle_relinks_a_reused_call_id
an_appended_denied_call_counts_and_writes_no_link
an_append_under_a_reserved_id_keeps_one_event            // plan decision D-E
```

`Harness` gets `async fn put_raw_body(&self, org: Uuid, id: Uuid, body: &str)`.

In `application/tool_history.rs` tests: `settle_plan` cases `a_timed_out_hold_under_allow_settles_its_reserved_event_and_does_not_append`, `an_interceptor_block_settles_the_reserved_event_with_deny`, `a_denied_call_with_no_reserved_event_is_appended_as_deny`, `a_call_with_no_decision_and_no_intent_is_appended_under_a_new_id`, `an_entry_without_call_facts_gives_nothing`. Delete or rename the PR 1 case `a_timed_out_hold_under_allow_appends_and_keeps_the_pending_event`: its rule changes (spec R10 revised).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway --test history_store_memory --lib tool_history`
Expected: FAIL to compile (`AppendCall` not found).

- [ ] **Step 3: Implement both adapters and the application changes**

The Lua stays within the PR 1 key layout. Update the module doc of `redis.rs` with the new body members and the link rules.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway --test history_store_memory --lib && cargo nextest run -p paigasus-gateway --test history_store_redis`
Expected: PASS (the Redis suite needs Docker; run it, do not skip it).

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/services/paigasus-gateway
git commit -m "feat(rs): store argument fields, link calls in the reserve and record the append verdict (SMA-753)"
```

---

### Task 10: The store port, read side: the window read, the truncation mark and the read limits

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/src/domain/tool_history.rs` (read types, constants)
- Modify: `rs/crates/services/paigasus-gateway/src/adapters/history/memory.rs`, `adapters/history/redis.rs`
- Modify: `rs/crates/services/paigasus-gateway/src/application/tool_history.rs` (`reserve` returns the window and fills the call fields from the field plan)
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs` (`GATEWAY_TOOL_HISTORY_EVENTS_MISSING_TOTAL`, `ALL`, the registered test)
- Modify: `docs/ops/RUNBOOK-observability.md` § 2.3 (one row)
- Modify: `tests/support/history_contract.rs`, `tests/history_store_redis.rs`

**Interfaces:**
- Consumes: `SetTool`, `Event`, `Window`, `WindowSet`, `Scope` (Task 2), `ReadPlan` (Task 5).
- Produces (domain):

```rust
pub const MAX_READ_SETS: usize = 64;
pub const MAX_READ_EVENTS: usize = 20_000;
pub const MAX_INDEXED_TOOLS: usize = 256;
pub struct ReadRequest { pub from_ms: u64, pub sets: Vec<(Scope, SetTool)> }
pub struct SetWindow { pub scope: Scope, pub tool: SetTool, pub events: Vec<StoredEvent>, pub mark_ms: Option<u64>, pub truncated: bool }
// ReserveRequest: `window_from_ms` is replaced by `read: Option<ReadRequest>`
// AppendReply gets: pub window: Vec<SetWindow>, pub missing_bodies: u64
pub fn check_window(req: &ReadRequest, reply: &[SetWindow]) -> Result<(), HistoryStoreError>;   // length and (scope, tool) order; a decode error
// application
pub enum WindowRead { NotNeeded, Read(Window), Failed }
pub struct ReserveOutcome { pub findings: Findings, pub now_ms: u64, pub hold: Vec<Event>, pub window: WindowRead }
pub async fn reserve(&self, ctx: &DecisionContext, session: &SessionContext, calls: &[ToolCall], ids: &[Uuid], policy: Option<&HistoryPolicy>) -> ReserveOutcome;
```

Rules (spec P4, P10):
- The application builds the `ReadRequest` from `policy.read()`: `from_ms = now_ms - w_max_ms` (saturating), and each `Scope::Key` set maps to the `Principal` set when the request has no API key (de-duplicated). It maps the reply back to a kernel `Window` with one `WindowSet` per kernel scope, so a `Key` condition reads the principal events.
- The `truncated` list of `AppendReply` keeps the PR 1 rule, with `read.from_ms` in place of `window_from_ms`.
- Tool sets: `Tool(t)` reads that tool's set. `Every` reads each tool of the `t:` index. The script builds an `h:` key from a `t:` member and a prefix in `ARGV`, the same exception that `LOOKUP_SCRIPT` uses for `e:`.
- Limits: the script stops at `MAX_READ_SETS` sets or `MAX_READ_EVENTS` events, and returns the current set and every later set with `truncated: true`. This is not a store error.
- `t:` cap: a new tool past `MAX_INDEXED_TOOLS` goes to its own set, not to the index, and the history gets the `x:` mark of `Every` (`x:t:<org>:<s|p|k>:<sha256(scope id)>`, value `now_ms`, the larger value kept).
- Mark: a cap trim writes `x:<h-key suffix>` = the largest removed score, keeps the larger value, and gets the TTL of the set. A read returns the mark of each set; an `Every` read returns the largest of the index mark and the marks of the sets it read.
- Reply: one entry per requested set, in order. Events sorted by `(ts_ms, id)`, future events and this hold's pending events included. A member whose `e:` body is gone is skipped and counted in `missing_bodies`.
- The application removes this hold's own ids from the window, builds the hold `Event`s (`ts_ms = now_ms`, kind `Request`, tool `history_tool(c)`, fields from `extract_fields` with `policy.fields().paths_for(tool)` and the digester), and increments `gateway_tool_history_events_missing_total` by `missing_bodies`. `prime_tool_history_metrics` primes it at 0.
- `WindowRead::Failed` on a reserve error, a deadline or a budget skip; `NotNeeded` when `policy.read()` is `None`.

- [ ] **Step 1: Write the failing tests**

New contract cases:

```rust
the_window_returns_the_events_of_each_requested_set_in_order
the_window_includes_future_events_and_the_pending_events_of_this_hold
an_every_tool_read_returns_the_events_of_every_indexed_tool
a_cap_trim_writes_a_mark_that_a_later_read_of_that_set_returns   // flood tool A over 10,000; a later reserve that reads A and only appends B still returns the mark of A
the_mark_keeps_the_larger_value
a_read_over_64_sets_returns_the_rest_as_truncated
a_read_over_20_000_events_returns_the_rest_as_truncated
a_tool_past_256_in_the_index_marks_every_tool
a_missing_body_is_skipped_and_counted
a_reply_with_a_missing_entry_is_a_decode_error                    // check_window unit test in domain, plus a Redis decode case
```

`Harness` gets `async fn drop_body(&self, org: Uuid, id: Uuid)`.

In `application/tool_history.rs` tests: `the_hold_own_events_are_removed_from_the_window`, `a_key_scope_without_an_api_key_reads_the_principal_sets`, `a_reserve_error_gives_a_failed_window`, `no_read_plan_reads_nothing`, `the_missing_body_metric_is_primed_and_counts`.

In `names.rs`: add the constant to `ALL` and to `the_gateway_tool_history_families_are_registered`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway --test history_store_memory --lib tool_history`
Expected: FAIL to compile (`ReadRequest` not found).

- [ ] **Step 3: Implement both adapters and the application changes**

Task 9 sent empty fields in the reserve. This task fills them: for each call, `policy.fields().paths_for(tool)` (an empty plan with no policy) and `extract_fields`, then `into_fields(org, digester)` into `CallEvent.fields` and `CallEvent.key`. The `CedarDecider` calls `reserve(.., ctx.policy.and_then(|s| s.history.as_deref()))` and keeps only `outcome.findings` in this task. Task 11 uses the window.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway --test history_store_memory --lib && cargo nextest run -p paigasus-gateway --test history_store_redis && cargo nextest run -p paigasus-observability`
Expected: PASS.

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/services/paigasus-gateway rs/crates/libs/paigasus-observability docs/ops/RUNBOOK-observability.md
git commit -m "feat(rs): read the history window in the reserve, with a persistent truncation mark and read limits (SMA-753)"
```

---

### Task 11: The decider evaluates the slots, with the failure rules

**Files:**
- Create: `rs/crates/services/paigasus-gateway/src/application/tool_policy/slots.rs`
- Modify: `rs/crates/services/paigasus-gateway/src/application/tool_policy/mod.rs`, `decider.rs` (`decide_by`, `decide_now`, `HoldFacts`)
- Modify: `rs/crates/services/paigasus-gateway/src/application/tool_policy/request.rs:488-495` (link when the floor has a condition)
- Modify: `rs/crates/libs/paigasus-observability/src/names.rs` (`GATEWAY_TOOL_HISTORY_SLOTS_MISSING_TOTAL`), `docs/ops/RUNBOOK-observability.md` § 2.3
- Modify: `rs/crates/services/paigasus-gateway/src/application/tool_history.rs` (`prime_tool_history_metrics` primes the 16 pairs)
- Test: `slots.rs` unit tests, `decider.rs` tests, `rs/crates/services/paigasus-gateway/tests/tool_history.rs`

**Interfaces:**
- Consumes: `evaluate` (Task 2), `decide_calls_with(.., slots, ..)` and `with_tenant_unavailable` (Task 5), `ReserveOutcome`, `WindowRead` (Task 10), `record_intent`/`forget_intent` (Task 8).
- Produces:

```rust
pub enum StoreOutcome { Disabled, NoSession, Read(ReserveOutcome) }
pub struct HoldSlots { pub per_call: Vec<Slots>, pub tenant_unavailable: bool, pub missing: Vec<Missing> }
/// P7 as one pure function. `calls` is the hold length.
pub fn hold_slots(policy: Option<&HistoryPolicy>, outcome: &StoreOutcome, calls: usize) -> HoldSlots;
```

Rules (spec P7 table, one row per arm):
- `policy` `None` or no condition: no slots, `tenant_unavailable: false`, no missing (A6).
- `Disabled` (no `[tool_history]`): every slot missing, reason `disabled`, both parts; `tenant_unavailable: false`.
- `NoSession` (the request had a lookup error): floor slots `no_session`; `tenant_unavailable` when the policy has a tenant condition. Each tenant condition slot is also counted as missing with reason `no_session`, as the tenant part becomes unavailable for this hold (spec P7).
- `Read` with `WindowRead::Failed`: floor slots `store`; `tenant_unavailable` when the policy has a tenant condition. Each tenant condition slot is also counted as missing with reason `store`.
- `Read` with `WindowRead::Read(w)`: `evaluate(policy.conditions(), &w, &hold, i, now_ms, policy.catalog())` for each call `i`.
- `NotNeeded` with conditions cannot happen (`ReadPlan::of` gives `Some` for any condition): treat it as `Failed`.
- The decider: record the hold intent (Task 8) first; then reserve; then `hold_slots`; when `tenant_unavailable`, decide the hold on `snapshot.with_tenant_unavailable()` (spec P7: under `open` every tenant policy is observed for that hold, under `closed` every call is blocked); count each `Missing` in `gateway_tool_history_slots_missing_total{part, reason}`; `forget_intent(ids)` before it returns.
- `StoreOutcome::NoSession` when `ctx.session` is `None` and the decider has a history; `Disabled` when the decider has no history.
- The request side links when the history is on and the org is known, and the tenant mode is not `off` **or** the floor has a condition (spec P7, design D5).

- [ ] **Step 1: Write the failing tests**

In `slots.rs` tests, one test per row of the spec P7 table:

```rust
#[test] fn no_condition_has_no_effect() {}
#[test] fn no_history_table_misses_every_slot_as_disabled_and_keeps_the_tenant() {}
#[test] fn a_store_failure_misses_the_floor_slots_and_makes_a_conditioned_tenant_unavailable() {}
#[test] fn a_store_failure_keeps_a_tenant_without_conditions() {}
#[test] fn an_unavailable_tenant_counts_each_of_its_slots_as_missing_with_store_or_no_session() { /* HoldSlots.missing holds a Missing{part: Tenant, reason: store} (or no_session) for each tenant condition slot, and the 16 primed pairs can all count */ }
#[test] fn the_not_needed_branch_is_treated_as_failed() {}
#[test] fn no_session_misses_the_floor_slots_and_makes_a_conditioned_tenant_unavailable() {}
#[test] fn a_truncated_set_misses_only_its_slots() {}
#[test] fn an_unknown_event_misses_its_slot() {}
#[test] fn any_catalog_tool_with_no_catalog_misses_the_floor_slot() {}
```

In `decider.rs` tests (memory store, `FakeSource` with conditions):

```rust
#[tokio::test] async fn a_sum_cap_denies_the_third_call_of_a_hold() { /* three sends of 2000, sum <= 5000: Allow, Allow, Block(policy) */ }
#[tokio::test] async fn a_denied_call_still_counts_for_the_next_hold() { /* then one send of 100: Block */ }
#[tokio::test] async fn a_store_error_under_open_observes_every_tenant_policy_for_that_hold() {}
#[tokio::test] async fn a_store_error_under_closed_blocks_the_hold() {}
#[tokio::test] async fn a_floor_slot_forbid_denies_on_a_store_error_in_every_mode() {}
#[tokio::test] async fn the_slots_missing_metric_is_primed_and_counts_part_and_reason() {}
#[tokio::test] async fn a_call_named_in_another_case_counts_for_its_name_key() { /* "Send" and "send" both count for name_key "send" */ }
#[tokio::test] async fn the_hold_intent_is_recorded_before_the_reserve_and_forgotten_after_the_decision() {}
```

In `tests/tool_history.rs` (memory backend, full request path): `mode_off_with_a_floor_condition_links_and_counts`, `a_lookup_error_gives_no_session_and_misses_the_floor_slot`, `a_decide_timeout_under_allow_counts_the_call_once`, `an_interceptor_block_after_the_reserve_settles_deny` (both use a decider that sleeps past `decide_timeout` after the reserve).

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway --lib tool_policy:: && cargo nextest run -p paigasus-gateway --test tool_history`
Expected: FAIL to compile (`hold_slots` not found).

- [ ] **Step 3: Implement `hold_slots` in `slots.rs` and the decider wiring**

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway && cargo nextest run -p paigasus-observability`
Expected: PASS.

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/services/paigasus-gateway rs/crates/libs/paigasus-observability docs/ops/RUNBOOK-observability.md
git commit -m "feat(rs): evaluate the history slots in the decider, with the failure rules of P7 (SMA-753)"
```

---

### Task 12: The `forged_history` anchor on the last assistant message with tool calls

**Files:**
- Modify: `rs/crates/libs/paigasus-gateway-core/src/turn.rs` (`chat_latest` L533, `responses_latest` L702, `anthropic_latest` L932)
- Test: `turn.rs` tests

**Interfaces:**
- Consumes: nothing new. `LatestTurn` keeps its fields.

Rules (spec P11):
- Chat: call ids and refs from the last `assistant` message, as in PR 1. Results: `tool` messages after the last `assistant` message whose `tool_calls` is a non-empty array.
- Anthropic: results: `tool_result` blocks after the last `assistant` message with at least one `tool_use` block.
- Responses: the anchor is the last run of assistant, reasoning and call items that holds at least one call item. The results are the result items after the start of that run. Call ids and text stay from the last run, as in PR 1.
- The 128 cap applies to the result list.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn a_trailing_text_message_does_not_hide_the_results_of_the_tool_turn_chat() {
    // assistant(tool_calls c1), tool(c1), assistant("done"): result_call_ids == ["c1"], call_ids == []
}
#[test] fn a_trailing_text_message_does_not_hide_the_results_of_the_tool_turn_anthropic() {}
#[test] fn a_trailing_text_message_does_not_hide_the_results_of_the_tool_turn_responses() {}
#[test] fn the_refs_still_come_from_the_last_assistant_message() {}
#[test] fn the_result_cap_still_applies_with_the_new_anchor() { /* 130 results: 128 kept, dropped_results == 2 */ }
```

The PR 1 tests that assert the old anchor change with the rule; keep their names and fix their expected values.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway-core turn::`
Expected: FAIL (the three trailing-text tests).

- [ ] **Step 3: Implement the anchor in the three functions**

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway-core && cargo nextest run -p paigasus-gateway --test tool_history`
Expected: PASS.

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/libs/paigasus-gateway-core/src/turn.rs
git commit -m "feat(rs): anchor forged_history on the last assistant message with tool calls (SMA-753)"
```

---

### Task 13: The `iam.proto` comments

**Files:**
- Modify: `contracts/proto/paigasus/iam/v1/iam.proto:891-950` (comments of `CompiledToolPolicySet` and `HistoryCondition`)
- Regenerate: `rs/crates/libs/paigasus-proto/src/generated/**`, and the Python and TypeScript generated trees that `contracts:generate` writes

Rules (spec P14), comment text only, no field change:
- `CompiledToolPolicySet`: the retention rule becomes "The gateway checks the retention for each condition. When the retention is shorter than the `window_ms` of a condition, only the slots of that condition are missing. The other policies of the set still decide."
- `HistoryCondition`: add "An event with a time after now is inside every window."; the three-valued rule of spec P1 (true, false, unknown; Kleene logic; unknown makes the slot missing where it can change the result); the decided-call rule as spec P1 states it; "A string, bool or decimal value is compared as a keyed digest. Only `EQ` exists for these types."; replace the `sum_within` line about "no value adds 0" with the rule of plan decision D-C.

- [ ] **Step 1: Edit the comments, then format**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd contracts && buf format -w && buf lint`
Expected: no output from `buf lint`.

- [ ] **Step 2: Regenerate and check the drift**

Run: `moon run contracts:generate && git status --short`
Expected: only comment lines change in the generated files (`git diff --stat` shows the generated Rust, Python and TS files). If `contracts:generate` fails on a BSR rate limit, restore any deleted generated file and run it again (memory note "BSR rate limit deletes generated file").

- [ ] **Step 3: Build**

Run: `cd rs && cargo build --workspace && cargo nextest run -p paigasus-proto`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add contracts rs/crates/libs/paigasus-proto   # and each generated path that `git status --short` lists
git commit -m "docs(contracts): state the per-condition retention, the unknown rule and the digest comparison (SMA-753)"
```

---

### Task 14: The IAM setting `tool_policies.history_conditions`

**Files:**
- Modify: `rs/crates/services/paigasus-iam/src/config.rs:351, ~1086, ~1126` (field, defaults, `Default`)
- Modify: `rs/crates/services/paigasus-iam/src/application/tool_policies/sets.rs:15-50` (remove the constant)
- Modify: `rs/crates/services/paigasus-iam/src/application/tool_policies/mod.rs:67-77, 489-496` (`ToolPolicyLimits.history_conditions`, the compile helper)
- Modify: `rs/crates/services/paigasus-iam/src/adapters/http/mod.rs:360` (fill the limit from the config), every `ToolPolicyLimits { .. }` literal in tests
- Test: `config.rs` tests, `sets.rs` tests, the IAM tool-policy service tests

**Interfaces:**
- Produces: `pub history_conditions: bool` on `ToolPoliciesConfig` and `ToolPolicyLimits`; `pub(crate) fn history_rejections(out: &CompiledOutput, enabled: bool) -> Vec<ToolPolicyRejection>` (the old private helper, now the only one). `HISTORY_CONDITIONS_ENFORCED_BY_GATEWAY` and `reject_history_until_gateway_supports_it` are removed.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn history_conditions_defaults_to_false() { assert!(!ToolPoliciesConfig::default().history_conditions); }
#[test] fn history_conditions_comes_from_the_environment() { /* IAM_TOOL_POLICIES__HISTORY_CONDITIONS=true -> true */ }
```

In the service tests: `a_save_with_a_condition_is_refused_when_history_conditions_is_off` (reason `tool-policy-history-unsupported`) and `a_save_with_a_condition_is_accepted_when_history_conditions_is_on`. The existing history-rejection tests run with `false`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-iam --lib tool_policies config::`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

The rejection message drops "yet": "policy {id} uses a history rule (formerly, count_within or sum_within). History rules are off in this IAM (tool_policies.history_conditions = false), so this set cannot be saved. Remove the temporal block, or ask the operator to turn history rules on."

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-iam`
Expected: PASS (Docker suites need Docker).

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/services/paigasus-iam
git commit -m "feat(rs): add the IAM setting tool_policies.history_conditions (SMA-753)"
```

---

### Task 15: The chart value `toolPolicies.historyConditions`

**Files:**
- Modify: `charts/paigasus/values.yaml:72-83` (new `toolPolicies` map after `dpop`)
- Modify: `charts/paigasus/templates/_iam-backend.tpl` (`iamReservedEnv` line 17, new `paigasus.iamHistoryConditions`, new `paigasus.validateIamToolPolicies`, called from the validate block at line 109)
- Modify: `charts/paigasus/templates/backend-deployment.yaml:165-172` (env entry)
- Modify: `charts/paigasus/tests/env.sh`, `tests/refusals.sh`, `tests/render.sh`; create `charts/paigasus/tests/golden/iam-history-conditions.yaml`
- Modify: `charts/paigasus/README.md`

Rules (spec P13, the DPoP pattern):
- `values.yaml`: `toolPolicies: { historyConditions: false }` with a "NOT required" comment that says: turn history on in the gateway first, then in IAM; turn it off in IAM first.
- Read with `dig "toolPolicies" dict` and `dig "historyConditions" false`. The env entry `IAM_TOOL_POLICIES__HISTORY_CONDITIONS: "true"` renders only when the value is `true`. Its comment goes inside the `{{- if }}` block, so the default render and the existing goldens do not change.
- `validateIamToolPolicies` refuses a `toolPolicies` that is not a map and a `historyConditions` that is not a bool, with the message form of `validateIamDpop`.
- `iamReservedEnv` gets `IAM_TOOL_POLICIES__HISTORY_CONDITIONS`, so `extraEnv` with that name is refused.

- [ ] **Step 1: Write the failing test rows**

`env.sh`: a row that renders `historyConditions: true` and finds the env entry, a row that renders the default and finds no entry; update the row counter. `refusals.sh`: rows for `extraEnv: [{name: IAM_TOOL_POLICIES__HISTORY_CONDITIONS, value: "x"}]`, `toolPolicies: "x"` and `historyConditions: "yes"`. `render.sh`: the new golden case.

- [ ] **Step 2: Run the chart tests to verify they fail**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && for t in charts/paigasus/tests/*.sh; do bash "$t" || echo "FAIL $t"; done`
Expected: FAIL in `env.sh`, `refusals.sh` and `render.sh`.

- [ ] **Step 3: Implement, then write the golden file**

Run: render with the new values, as `render.sh` does for `iam-dpop.yaml`, into `tests/golden/iam-history-conditions.yaml`.

- [ ] **Step 4: Run the chart gate**

Run: `for t in charts/paigasus/tests/*.sh; do bash "$t" || echo "FAIL $t"; done && moon run repo:helm-render`
Expected: no `FAIL`; `repo:helm-render` passes. The existing goldens do not change (`git status charts/paigasus/tests/golden`). If a new chart script were added, the floor pins of `charts/CLAUDE.md` would apply; this task adds none.

- [ ] **Step 5: Commit**

```bash
git add charts/paigasus
git commit -m "feat(charts): expose the IAM setting toolPolicies.historyConditions (SMA-753)"
```

---

### Task 16: The decision corpus cases (compiler workspace)

**Files:**
- Modify: `rs/tools/paigasus-policy-compiler/tests/corpus.rs`
- Create: new files under `rs/tools/paigasus-policy-compiler/corpus/` (written with `PAIGASUS_WRITE_FIXTURES=1`)

**Interfaces:**
- Produces (the corpus file format, read by Task 17): each file keeps the PR 1 keys. Each event gets `"arguments"` (the exact argument text the gateway reads) and `"kind"` (`"request"` or `"response"`). A response event copies the `arguments` of its call. `decisions` holds `null` at a response event.

Rules:
- `Case` gets `name: &'static str` (the file name; the three existing cases keep `formerly`, `count_within`, `sum_within`) and each `Call` gets `kind`, `arguments: &'static str` and an explicit Dogwood value tree.
- `to_dogwood` maps nested objects to `DwValue::Object` (nested paths) and a `{"__decimal": "<text>"}` object to `DwValue::Decimal`.
- A response event goes to Dogwood with `Authorizer::submit` (a history event, `Ok(None)`).
- Dogwood gets the value that the JSON text names: an integer text in the `i64` range is `Int`; a text with a fraction and no exponent is `Decimal(text)`; another number text (a big integer, `1e19`) is `String(text)`. The gateway reads the same `arguments` with the kernel.
- New cases, one file each: `predicate_event_kind`, `predicate_all_of`, `predicate_any_of`, `predicate_negation`, `ops_long` (all six ops), `response_copies_fields`, `any_catalog_tool`, `scope_principal`, `scope_key`, `nested_path`, `field_string`, `field_bool`, `field_decimal`, `decimal_edges` (`5.0`, `-0.0`, `1.5000`), `big_integer`, `exponent_1e19`, `missing_field`, `negative_value`, `unknown_value` (a string amount in a sum: Paigasus denies, `paigasus_differs`), `decided_call_unparsed` (free text of the summed tool: `paigasus_differs`).
- Every event of a case uses one principal and, for `scope_key`, one API key. A difference that the run shows and that the spec does not state is a defect: do not mark it. Mark only the differences of spec P1, P3 and D-C, each with its rule text.

- [ ] **Step 1: Add the cases, then run the test to verify it fails**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && cd rs/tools/paigasus-policy-compiler && cargo nextest run --test corpus`
Expected: FAIL ("corpus/<name>.json: No such file").

- [ ] **Step 2: Write the fixtures and read every new file**

Run: `PAIGASUS_WRITE_FIXTURES=1 cargo nextest run --test corpus && git diff --stat corpus/`
Expected: PASS. The three old files change only by the new `arguments` and `kind` members. Read each `paigasus_differs` entry and check it against the rule list above.

- [ ] **Step 3: Run the compiler gates**

Run: `cargo nextest run && cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add rs/tools/paigasus-policy-compiler
git commit -m "test(rs): extend the history decision corpus for the gateway evaluator (SMA-753)"
```

---

### Task 17: `condition_to_pb` in iam-core, and the parity replay in gateway-core

**Files:**
- Create: `rs/crates/libs/paigasus-iam-core/src/tool_policy/proto.rs` (`condition_to_pb`, `condition_from_pb`, and their helpers, moved from `paigasus-iam/src/application/tool_policies/sets.rs:115-240`)
- Modify: `rs/crates/libs/paigasus-iam-core/Cargo.toml` (`paigasus-proto = { workspace = true }`), `moon.yml` (`dependsOn` `paigasus-proto-rs`; upstreams `paigasus-proto` and `paigasus-proto-derive` sources and `Cargo.toml`)
- Modify: `rs/crates/services/paigasus-iam/src/application/tool_policies/sets.rs` (use the moved functions)
- Create: `rs/crates/libs/paigasus-gateway-core/tests/history_parity.rs`
- Modify: `rs/crates/libs/paigasus-gateway-core/Cargo.toml` (`[dev-dependencies] paigasus-iam-core = { workspace = true }`), `moon.yml` (`dependsOn` `paigasus-iam-core-rs`; upstreams of iam-core; test input `/rs/tools/paigasus-policy-compiler/corpus/**/*`)

**Interfaces:**
- Produces: `pub fn condition_to_pb(c: &HistoryCondition) -> pb::HistoryCondition` and `pub fn condition_from_pb(c: pb::HistoryCondition) -> Result<HistoryCondition, ProtoError>` in `paigasus_iam_core::tool_policy`. `ProtoError` is a `thiserror` enum with one `Malformed(&'static str)` variant; IAM maps it to `corrupt(..)` as today.

Rules (spec P15):
- The replay reads every `corpus/*.json` as data. Chain: `compiled.conditions` → `serde_json` into `paigasus_iam_core::tool_policy::protocol::HistoryCondition` → `condition_to_pb` → `map_conditions(Part::Tenant, .., DAY_MS, ∅)` → `HistoryPolicy::build` with a fixed test digester (a local `HistoryDigester` over `sha2` with a fixed key) → for each request event `i`: the window holds the earlier events (request and response, with `extract_fields` of their `arguments`), the hold is event `i`, `now_ms` is its time × 1000 → `evaluate` → `decide_with_slots` on `ToolPolicies::compile(empty floor, cedar_policies, catalog)`. The catalog is the action names of `cedar_schema_json` other than `InvokeTool`.
- The replay requires `decisions[i]`, or `paigasus_differs[..].paigasus` where an entry names event `i`.
- A test asserts `paigasus_gateway_core::MAX_PREDICATE_DEPTH == paigasus_iam_core::tool_policy::MAX_PREDICATE_DEPTH`.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn every_corpus_decision_replays_through_the_production_chain() {
    let files = corpus_files();
    assert!(files.len() >= 23, "the corpus of Task 16");
    for f in files { /* per event: assert_eq!(gateway_decision, expected, "{name} event {i}") */ }
}
#[test]
fn the_predicate_depth_limit_matches_iam() {}
```

In iam-core: `condition_round_trips_through_the_proto` (every node, every literal kind), and the IAM `sets.rs` tests keep passing.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cd rs && cargo nextest run -p paigasus-gateway-core --test history_parity`
Expected: FAIL to compile (`condition_to_pb` not found in `paigasus_iam_core`).

- [ ] **Step 3: Move the functions and write the replay**

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cd rs && cargo nextest run -p paigasus-gateway-core -p paigasus-iam-core && cargo nextest run -p paigasus-iam --lib tool_policies && moon run repo:affected-smoke`
Expected: PASS. Run `repo:affected-smoke` under system `/bin/bash` 3.2 (root `CLAUDE.md`, "This development Mac only"); it checks the new `dependsOn` and `upstreams` entries (A6).

- [ ] **Step 5: Lint, format, commit**

Run: `cd rs && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/libs/paigasus-iam-core rs/crates/libs/paigasus-gateway-core rs/crates/services/paigasus-iam
git commit -m "test(rs): replay the decision corpus through the production chain in gateway-core (SMA-753)"
```

---

### Task 18: The cap test on real Redis

**Files:**
- Create: `rs/crates/services/paigasus-gateway/tests/history_cap_redis.rs`
- Modify: `rs/.config/nextest.toml:125-137` (a new override block)

Rules (design D8, spec P15):
- Two `RedisSessionStore` clients on one Redis container (two replicas), each in its own `ToolHistory` and `CedarDecider`. One `Clock` is 50 ms ahead in the skew variant.
- One tenant set: `permit(principal, action == Pgs::Gateway::Action::"send", resource) when { context.p_cap };` with one condition `p_cap`: `sum_within(input.amount, 24h, Tool(send)) <= 5000`, scope principal.
- Three concurrent holds of one `send` of 2,000 each, spread over the two deciders. Exactly two are allowed and one is blocked. A fourth call of 100 is blocked (6,100 with the denied 2,000).
- Two test functions: `three_concurrent_sends_pass_the_cap_only_twice` and `the_cap_holds_with_a_50_ms_clock_skew`.

nextest override (before the general gateway Redis block, so first match wins):

```toml
[[profile.default.overrides]]
# SMA-753 PR 2: the cap test and the p99 test. No retry: a retry can hide a race (design § 9).
filter = 'package(=paigasus-gateway) and (binary(history_cap_redis) or binary(perf))'
retries = 0
test-group = 'docker-containers'
```

- [ ] **Step 1: Write the test**

- [ ] **Step 2: Run it**

Run: `cd rs && cargo nextest run -p paigasus-gateway --test history_cap_redis`
Expected: PASS, both functions, with Docker.

- [ ] **Step 3: Prove that it bites**

Temporarily make `ToolHistory::reserve` read no window (return `WindowRead::NotNeeded`). Run the test. Expected: FAIL. Delete the marked change (do not use `git checkout --` on a file that holds other work).

- [ ] **Step 4: Lint, format, commit**

Run: `cd rs && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/services/paigasus-gateway/tests/history_cap_redis.rs rs/.config/nextest.toml
git commit -m "test(rs): prove the history cap across two replicas on real Redis (SMA-753)"
```

---

### Task 19: The p99 test with the Redis store

**Files:**
- Modify: `rs/crates/services/paigasus-gateway/tests/perf.rs` (two new `#[ignore]` tests)

Rules (spec P15, design § 9):
- `p99_policy_overhead_with_history_conditions`: a Redis container; a 1,000-event principal history of `send` and `search`; a hold of three calls; one `sum_within` and one `formerly` condition. Assert `p99 < 15 ms` of `GATEWAY_TOOL_POLICY_OVERHEAD_SECONDS`. Print the p50, p99 and the mean Redis time per hold (the reserve region).
- `history_read_at_the_read_limits` (no assertion): 10,000 events in each of three tools, an `Every` read. Print the same numbers.
- The existing cases keep `history: None`.

- [ ] **Step 1: Write the two tests**

- [ ] **Step 2: Run them in release mode**

Run: `cd rs && cargo nextest run -p paigasus-gateway --release --test perf --run-ignored only -E 'test(history)' --no-capture`
Expected: the first test passes and prints its numbers. Copy both outputs into the PR description (spec P15: the PR records both results).

- [ ] **Step 3: Lint, format, commit**

Run: `cd rs && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`

```bash
git add rs/crates/services/paigasus-gateway/tests/perf.rs
git commit -m "test(rs): measure the p99 overhead with history conditions on Redis (SMA-753)"
```

---

### Task 20: The runbooks

**Files:**
- Modify: `docs/ops/RUNBOOK-tool-policies.md` § 5 ("History rules") and § 10 ("Tool history")

Content (each item one short paragraph in STE):
- § 5: history rules work when the gateway has `[tool_history]` and IAM has `tool_policies.history_conditions = true`. The order: gateway first, then IAM; to turn off: IAM first (spec P13). A denied call counts (design R5). A free-text or mistyped call makes a dependent slot missing for up to 24 hours (R-new-2).
- § 10, Configuration: `digest_key` and `previous_digest_key`; no key gives missing slots for string, bool and decimal conditions (R-new-3); the two-phase rotation with the 25-hour wait (spec P3).
- § 10, Failure rules: the P7 table; under a tenant `fail_mode = open`, a store error makes every tenant policy only observed for that hold, caps included (R-new-5).
- § 10, Redis: every history key has a TTL, so an evicting `maxmemory-policy` can remove any of them; the missing-body metric (spec P10). Sizing: the read limits, and the advice to split the history Redis from the limits Redis when the hold read time grows (R-new-4).
- § 10, Metrics: `gateway_tool_history_slots_missing_total{part, reason}` with its 8 reasons and `gateway_tool_history_events_missing_total`.
- § 10: replace "Known limits of PR 1" with the limits of spec § 6 (R2 revised, R10 revised, R-new-1 to R-new-5).

- [ ] **Step 1: Write the sections**

- [ ] **Step 2: Check the drift gate and the links**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run repo:observability-drift`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add docs/ops/RUNBOOK-tool-policies.md
git commit -m "docs(ops): document history rules, the history key and the slot failure rules (SMA-753)"
```

---

### Task 21: The full gate graph and the mutation checks

**Files:** none (verification only). A fix that a gate or a mutation needs goes into the task that owns the code, as a new commit.

- [ ] **Step 1: Run the full gate graph**

Run (root `CLAUDE.md`, "Before you push"):

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :moon-diagnosis-exec :wasm-lockstep :test-e2e \
  --base origin/main --include-relations
```

Expected: every task passes. On this Mac no single bash runs every gate (root `CLAUDE.md`, "This development Mac only"). Run the graph with system `/bin/bash` 3.2 for `repo:affected-smoke`. Then re-run the bash-4+ gates directly with `/opt/homebrew/bin/bash ci/<gate>/run.sh`: `ruff`, `next-public`, `publish-metadata`; and `/opt/homebrew/bin/bash ops/nats/check-subjects.sh` plus its nextest half for `repo:nats-permissions`. Run `repo:actionlint` only when its preflight reports a pipe of 8192 bytes or more; else run it in a Linux container (memory note "Small pipe"). Read those direct results, not the `moon ci` rows for them. On a failure, follow the root `CLAUDE.md` diagnosis procedure, Step 0 first.

- [ ] **Step 2: Run the mutation checks (spec P15)**

For each row: make the change, run the command, confirm FAIL, then delete the marked change and confirm PASS. Each mutation must compile (use `--no-fail-fast`; a mutation that dies at `rustc` proves nothing). After any fix, run the whole battery again.

| # | Mutation | Test that must fail |
|---|---|---|
| M1 | The reserve reads no window (`WindowRead::NotNeeded`) | `history_cap_redis::three_concurrent_sends_pass_the_cap_only_twice`, `decider::a_sum_cap_denies_the_third_call_of_a_hold` |
| M2 | `settle_plan` gives no append | `tool_history::a_denied_call_with_no_reserved_event_is_appended_as_deny` |
| M3 | The window is half-open (`ts_ms > start`) | `history_evaluate::the_window_is_closed_and_a_future_event_is_inside_every_window` |
| M4 | Restore the `now` upper bound, or remove the future-event rule (the same mutation; the row M10 is merged here) | the same test, and `history_cap_redis::the_cap_holds_with_a_50_ms_clock_skew` |
| M5 | A store error allows (`hold_slots` gives no missing slot on `Failed`) | `slots::a_store_failure_misses_the_floor_slots_and_makes_a_conditioned_tenant_unavailable` |
| M6 | Remove the reserve link (P8) | `history_contract::the_reserve_writes_the_call_links_of_each_call` (both adapters) |
| M7 | Ignore the append verdict (always `allow`) | `history_contract::an_appended_denied_call_counts_and_writes_no_link` |
| M8 | Fail the read on a missing body | `history_contract::a_missing_body_is_skipped_and_counted` |
| M9 | Remove the `Key` to principal alias | `tool_history::a_key_scope_without_an_api_key_reads_the_principal_sets` |
| M11 | Keep the hold's own events in the window | `tool_history::the_hold_own_events_are_removed_from_the_window` |
| M11b | The evaluator does not dedup events by id | `history_evaluate::the_events_of_a_window_and_the_hold_are_counted_once_by_id` |
| M12 | Remove the truncation mark (no `x:` write) | `history_contract::a_cap_trim_writes_a_mark_that_a_later_read_of_that_set_returns` |
| M13 | Unknown counts as false | `history_evaluate::count_within_takes_the_value_only_when_every_possible_count_agrees`, `history_parity::every_corpus_decision_replays_through_the_production_chain` (`unknown_value`) |

Run per row: `cd rs && cargo nextest run --no-fail-fast -p <crate> <filter>` (the Redis rows with `--test history_store_redis` or `--test history_cap_redis`). Each kernel row (M3, M4, M13 `history_evaluate`) adds `--features tool-policy`: `cd rs && cargo nextest run --no-fail-fast -p paigasus-kernel --features tool-policy --test history_evaluate <filter>`. Without the feature the test file compiles to zero tests and proves nothing. Check that the failing output names the test.

- [ ] **Step 3: Record the results**

Write the mutation table with the observed FAIL for each row, the two perf outputs of Task 19 and the list of gates run outside `moon ci` (with their bash) into the PR description draft. Do not commit it.

---

## Self-review

**Spec coverage.**

| Spec item | Task |
|---|---|
| S1, P1 (types, three-valued rules, shapes, missing reasons) | 2 |
| S1, P2 (`decide_with_slots`, reserved names, response and request side) | 3, 5, 11 |
| S2, P5 (load validation, every malformed form with a test) | 4, 7 |
| S2, P6 (conditions travel: source, cache entry, snapshot `Arc`, `floor_only`) | 5, 7 |
| S3, P3 (key, key id, no key, rotation, digest, decimal parse, one walker, path walk, encoding, result events, appended calls) | 1, 2, 4, 5, 6, 8, 9 |
| S4, P4 (`ReadPlan`, `from_ms`, Key alias, tool sets, limits, `t:` cap, `x:` mark, reply order and length, hold events removed) | 5, 10 |
| S5, P7 (failure table, floor-only hold, catalog of a floor condition, mode `off` linking, slots-missing metric) | 7, 11 |
| S6, P8 (reserve link, no replacement, settle keep and delete guard, R-new-1) | 9 |
| S7, P9 (`AppendCall` verdict, hold intent, settle of blocked and of `allow`-forwarded calls) | 8, 9, 11 |
| S8, P10 (missing body skipped, metric, runbook) | 10, 20 |
| S9, P11 (anchor, Responses run, 128 cap) | 12 |
| S10, P12 (remove `refused`, `store_refused`, their tests) | 7 |
| S11, P13 (IAM setting, both values tested, chart value, runbook order) | 14, 15, 20 |
| S12, P14 (proto comments, regenerate, drift) | 13 |
| S13, P15 (corpus, parity replay, proto mapping cases, contract cases, cap test, p99 test, mutation checks) | 4, 9, 10, 16, 17, 18, 19, 21 |
| S14 (runbook and metric catalog) | 10, 11, 20 |

**Type consistency.** Checked across tasks: `Condition`, `Part`, `Scope`, `SetTool`, `Window`, `WindowSet`, `Event`, `Slots`, `Missing`, `MissingReason`, `Catalog` (Task 2) are the names used in Tasks 5, 10, 11 and 17. `FieldPlan::paths_for` (Task 5) is the call in Tasks 8 and 10. `HistoryPolicy::{conditions, has_part, fields, read, catalog}` (Task 5) are the calls in Tasks 7, 10, 11. `AppendCall` (Task 9) replaces `ForwardedCall` everywhere. `ReserveOutcome` and `WindowRead` (Task 10) are consumed in Task 11. `decide_calls_with` has the `slots` argument from Task 5 on.

**Review Focus.** Each of the five lines has its test in its owning task: 1 and 2 in Task 1, 4 in Task 2, 5 in Task 8, 3 in Task 11.

**Proportion.** The plan holds signatures, rules, test names and assertions. The function bodies are left to the implementer, except the fixed values of the spec.
