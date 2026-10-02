# SMA-673: one `prnParse` kernel binding, so the console reads a PRN once

- Linear: SMA-673 (milestone "Frontend", priority Low, labels `area:ffi`, Improvement)
- Related: SMA-634 (PR 291), which found the cost. ADR-0005 (one kernel, bound to every language).
  ADR-0022 (the console reads PRNs through `@paigasus/kernel`).
- Status: APPROVED by Sven on 2026-10-02. The spec was written unattended on 2026-09-27. The
  spec-challenger reviewed it (verdict APPROVE WITH CHANGES), and this version folds its findings
  (section 11). Section 10 records Sven's answer to each open question. Section 12 lists the
  changes from the re-check against `origin/main` on 2026-10-02 (`e5f31a56`).
- Follow-up: SMA-725 (deprecate the six single-field TypeScript accessors, Q3).

## 1. The problem

`ts/packages/paigasus-console-core/src/prn-tenancy.ts` reads a PRN in `readKernelFields`. It calls
six kernel accessors: `prnErrorKind`, `prnService`, `prnRegion`, `prnResourceType`,
`prnResourceId` and `prnOrg`. Each accessor calls `Prn::parse` again
(`rs/crates/bindings/paigasus-wasm/src/lib.rs:41-83`). Each call also copies the string into wasm
linear memory and copies the result back out.

So one `parseTenancyPrn` call parses the same PRN six times for a valid `iam` PRN with no region.
`ROOT_PRN` also costs six calls. Four calls would decide it, but the three field reads stay together
on purpose: that keeps IAM's tenancy rule outside the `try` that makes the function total (SMA-634
review round 1). An invalid PRN costs one call. A PRN of another service costs two calls, because
the `||` at `prn-tenancy.ts:66` stops after `prnService`. An `iam` PRN with a region costs three
calls.

It is a cost, not a leak: the glue frees every allocation. Eight modules outside
`prn-tenancy.ts` call `parseTenancyPrn` or `parsePrincipalPrn` (10 call sites outside tests,
measured on `e5f31a56`): `ts/packages/paigasus-console-core/src/scopes.ts`, four iam-console files
(`orgs/node-ref.ts`, `orgs/load.ts`, `orgs/[org]/load.ts`, `orgs/[org]/teams/[team]/load.ts`) and
three gateway-console files (`people-model-access/commands.ts`, `orgs/[org]/load.ts`,
`orgs/[org]/projects/[project]/load.ts`). Some of these call sites run inside a loop over a list
(for example one call per team). Every such page pays the cost.

## 2. Acceptance

- A1. `paigasus-kernel` has one new function that parses a PRN once and gives every field, or the
  error kind, in the wire form (org `""` when absent, lower-case hyphenated UUIDs). It is the only
  place for this new function. The six existing accessors keep their own marshalling in each
  binding (A7).
- A2. The wasm and napi bindings each export that function as a one-line wrapper. Their TypeScript
  signatures are identical, and `src/binding-parity.types.ts` pins that. PyO3 gets no new binding
  (D3).
- A3. A new parity corpus `vectors/prn_parse.json` holds valid AND invalid inputs. Rust, napi, wasm
  (vitest and `wasm-probe.mjs`) and Python replay it. Python replays it through its six existing
  accessors (D3). `repo:parity-corpus-drift` covers it, and the gate is changed so that an
  UNTRACKED vector file also fails it (4.4).
- A4. `prn-tenancy.ts` makes exactly ONE kernel call per `parseTenancyPrn` and per
  `parsePrincipalPrn`, for every input that passes the length check. A test proves this.
- A5. The public surface of `prn-tenancy.ts` does not change: the same exports, the same types, the
  same results for every row of the existing corpus replay (`tests/unit/prn-tenancy.test.ts`).
  `parseTenancyPrn` and `parsePrincipalPrn` still never throw. IAM's tenancy rule stays outside
  the `try`, and a source-text test pins that (5.3).
- A6. The PR and a Linear comment record a before/after number: kernel calls per parse, time per
  parse on the wasm entry, and an estimate for one console page render (section 5.5). A6 has a
  stop rule: if the saving is below the threshold in 5.5, the work stops before merge.
- A7. The existing six accessors stay. No other consumer changes. Their deprecation is SMA-725
  (Q3).
- A8. The Notion Development Guidelines get one new entry for the positional multi-value return
  across the bindings (Q4). The entry records the D1 convention: a fixed-length `Vec<String>` wire
  form in `paigasus_kernel::wire`, one typed adapter per language, and no raw re-export. It is NOT
  an ADR-0005 amendment. The PR body links the entry.

## 3. Decisions

### D1. The binding returns a fixed array of six strings, not an object

The binding is total. It never throws. It returns
`[errorKind, service, region, org, resourceType, resourceId]`, always six strings:

- a valid PRN: `errorKind` is `""`, and the five fields are the canonical values (the same values
  the six accessors return today);
- an invalid PRN: `errorKind` is the `PrnError::kind()` token, and the five fields are `""`.

The reason is the type guard. wasm-bindgen maps `Vec<String>` to `string[]`, and napi-rs 3 maps it
to `Array<string>`. The two types are identical, so the `Exact<…>` guard in
`src/binding-parity.types.ts` holds with no new dependency. It also matches the existing
`prnErrorKind` contract: a total function whose error is data, not an exception.

The positional form is an FFI wire convention. It must not become ordinary public API of the
published kernel crate. D2 puts it in a hidden `wire` module, and the typed TypeScript adapter is
the only form that consumers see.

Rejected alternatives:

- A struct: `#[napi(object)]` in napi and a `#[wasm_bindgen]` struct in wasm. wasm-bindgen emits a
  CLASS with a `free()` method and a heap handle, and napi emits a plain interface. The two types
  are not identical, so the guard fails. The wasm class also holds wasm memory until `free()` or a
  finaliser runs, which is the opposite of the goal. `serde-wasm-bindgen` or `tsify` would give a
  plain object, but they add dependencies to a crate that the `:wasm-getrandom-free` gate watches.
- A memo cache in TypeScript (a `Map` from PRN to fields). It does not make the first parse
  cheaper. The input is a URL segment that an attacker controls, so the cache needs a bound and an
  eviction rule. This is more code for less gain.
- A delimited single string. The TypeScript side would then split it, which is PRN grammar
  outside the kernel (ADR-0005).

### D2. The names and the visibility

- Kernel (Rust): `paigasus_kernel::wire::prn_parse_fields(s: &str) -> Vec<String>` (the
  six-element wire form, D1). The module is `#[doc(hidden)] pub mod wire`. Its module doc says
  that it exists only for the bindings under `rs/crates/bindings/`, that consumers must use `Prn`,
  and that its shape is a binding convention. It holds no FFI dependency and no I/O, so the
  `lib.rs` rule "No I/O, no FFI" stays true: the module only fixes one marshalling convention in
  one place, so the bindings do not each copy it. Sven accepted this module in the published
  crate on 2026-10-02 (Q7). No separate unpublished crate. The function calls `Prn::parse` once and
  marshals from the typed accessors (`service()`, `region()`, `org()`, `resource_type()`,
  `resource_id()`).
- There is NO `PrnFields` struct and NO `Prn::fields` method. A struct with `org: String` would
  discard the typed `Option<Uuid>` that `Prn` already has, and it would add a second public
  surface to a published crate for no consumer.
- Bindings: wasm and napi `prnParseFields(s: string): string[]`. No PyO3 binding (D3).
- `@paigasus/kernel` (TypeScript): a typed adapter `prnParse(prn: string): PrnParseResult`, where

  ```ts
  type PrnParseResult =
    | { ok: true; service: string; region: string; org: string; resourceType: string; resourceId: string }
    | { ok: false; errorKind: string };
  ```

  The adapter lives in ONE new module, `ts/packages/paigasus-kernel/src/prn-parse.ts`, as
  `toPrnParseResult(wire: readonly string[])`. `src/index.ts` (napi) and `src/wasm.ts` (wasm) both
  build `prnParse` from it and their own `prnParseFields`. They export `prnParse` and the
  `PrnParseResult` type ONLY. They do NOT re-export the raw `prnParseFields`, so consumers cannot
  index positions. The parity guard does not need that re-export: it reads
  `typeof import('@paigasus/node-bindings')` and `typeof import('@paigasus/wasm')` directly.
- The adapter throws a `TypeError` in two cases only: the array does not hold exactly six strings,
  or `errorKind` is not `""` and one of the five fields is not `""`. Both cases are glue or
  binding defects, and both checks are about the wire convention (D1), not about PRN syntax. The
  adapter does NOT check that a field is non-empty on success: that is grammar, and the corpus
  replays already prove it.

`prnParse` is the name the issue asks for. The raw binding has a different name so that the
adapter does not shadow it and the `Exact<…>` guard can name the raw function.

### D3. Python replays the corpus but gets no new binding

The issue names wasm and napi only. A PyO3 `prn_parse_fields` returning `Vec<String>` fails
`repo:pyo3-stub-drift`. The stub `paigasus_py_bindings.pyi` is hand-written (`moon.yml:855-856`), and
the gate only compares it with the Rust. `RUST_TO_PY` in `ci/pyo3-stub/check.py:171-179` has no
`Vec<String>` row, so `map_rust_type` raises `RefusedError` and the gate exits rc 1. A plain row is
also wrong by the gate's own rule (§3.1): the map applies to parameters too, and `Vec<String>` is
1:1 only as a return. As a parameter, PyO3 accepts any non-`str` sequence. The gate comment says
that a new row "should stop a human". This run is unattended, so it does not make that decision.

Sven confirmed this on 2026-10-02 (Q1): no PyO3 `prn_parse_fields`, and Python keeps its existing
accessors.

The parity README rule "every binding replays every corpus" still holds without a new binding:
`py/packages/paigasus-kernel/tests/test_parity.py` replays `prn_parse.json` through the six
existing accessors (`prn_error_kind`, `prn_service`, `prn_region`, `prn_org`,
`prn_resource_type`, `prn_resource_id`), which `paigasus_kernel/__init__.py` already exports. This
proves that the Python view of every row agrees with the corpus. It does not test a Python wire
function, because there is none.

### D4. `prn-tenancy.ts` keeps its structure

`readKernelFields` keeps its name, its `try`, and its comment's argument. Inside the `try` there is
one call: `prnParse(prn)`. The body then checks `ok`, `service === 'iam'` and `region === ''`, and
returns `{ resourceType, resourceId, org }`. IAM's tenancy rule stays in `parseTenancyPrn` and
`parsePrincipalPrn`, outside the `try`, as today. The `MAX_LEN` bound stays before the kernel call.
The comment that says "an empty `prnErrorKind` happens to imply the other five accessors succeed"
is replaced: with one call there is no cross-call invariant. The `try` stays, because a wasm runtime
failure or the adapter's `TypeError` must still give `null`, not a 500.

### D5. The `catch` logs the failure

Today a glue defect makes the kernel throw, and the `catch` turns it into `null`. The user gets a
404 and nothing is logged. With the adapter, the `TypeError` is the only runtime signal of a glue
defect, so the `catch` logs it. It calls `logger.appEvent('prn.kernel_call_failed', { error })`
from `./logger`, where `error` is the error's `name` (for example `TypeError`) or `'unknown'`. It
NEVER logs the PRN or the error message: the PRN is a URL segment that an attacker controls, and
the logger's redaction is the caller's contract (`logger.ts:7-9`). Add `'prn.kernel_call_failed'`
to `AppEventName`. The function still returns `null`. The event fires only on a defect path, so it
does not need a rate limit. Sven accepted this event on 2026-10-02 (Q6).

## 4. Design

### 4.1 `rs/crates/libs/paigasus-kernel/src/`

- New `wire.rs`, declared in `lib.rs` as `#[doc(hidden)] pub mod wire;`, with the module doc of D2.
- `pub fn prn_parse_fields(s: &str) -> Vec<String>` per D1 and D2. Org is
  `p.org().map(|u| u.as_hyphenated().to_string()).unwrap_or_default()`. Resource id is
  `p.resource_id().as_hyphenated().to_string()`.
- No change to `resource_name.rs` and no new re-export at the crate root.
- Unit tests in `wire.rs`: one valid PRN with an org, one without, one region-ful PRN, and one
  invalid PRN per `PrnError` kind. Each asserts the six elements.

### 4.2 The two bindings

- `rs/crates/bindings/paigasus-wasm/src/lib.rs`: `#[wasm_bindgen(js_name = prnParseFields)] pub fn
  prn_parse_fields(s: String) -> Vec<String> { paigasus_kernel::wire::prn_parse_fields(&s) }`.
- `rs/crates/bindings/paigasus-node-bindings/src/lib.rs`: the same with
  `#[napi(js_name = "prnParseFields")]`.
- Regenerate and commit the artifacts: the five files under `rs/crates/bindings/paigasus-wasm/`
  (`moon run paigasus-kernel-ts:generate-wasm`), and `rs/crates/bindings/paigasus-node-bindings/`
  `index.d.ts` and `index.js`. `index.js` lists every export by name (for example
  `module.exports.prnService` at line 791), so it changes.
- After `generate-wasm`, the console setupFiles check 4 reds until
  `rm -rf ts/node_modules && pnpm -C ts install` (`ts/CLAUDE.md`). The plan must include that step.
- No change to `paigasus-py-bindings` or its `.pyi` (D3).

### 4.3 `@paigasus/kernel` (TypeScript)

- New `src/prn-parse.ts`: `PrnParseResult` and `toPrnParseResult` (D2).
- `src/index.ts` and `src/wasm.ts`: import `prnParseFields` from the binding, and export
  `prnParse = (prn: string) => toPrnParseResult(prnParseFields(prn))` and the `PrnParseResult`
  type. Do NOT re-export `prnParseFields` (D2).
- `src/binding-parity.types.ts`: add `_prnParseFields` and its `void`.
- `tests/committed-wasm.test.ts`: add `'prnParseFields:function'` to `EXPECTED_EXPORTS`, and update
  the "Twelve" comment (line 32) and the "21 names" message (line 119). MEASURE both the export list and the import list
  of the fresh build with `node tests/wasm-probe.mjs --interfaces <dir>`, and write the measured
  lists. In externref mode a `Vec<String>` return can add an `__externref_drop_slice` export and a
  string-conversion import. An import with a hash suffix gets a regex of the existing form
  (`committed-wasm.test.ts:58-63`), not a literal. Do not guess the counts. Update the corpus list
  in check 3 (line 131) to six names.
- `vitest.config.ts`: the `include` lists are EXPLICIT file lists, not globs (lines 31 and 58).
  Add `tests/prn-parse.test.ts` and `tests/prn-parse-adapter.test.ts` to the `node` list, and
  `tests/prn-parse.wasm.test.ts` to the `browser` list. Without this, the three new files never
  run. M5 and M6 (5.4) prove that they run.

### 4.4 The parity corpus

- `rs/crates/libs/paigasus-kernel-parity/src/lib.rs`: add `PrnParseCase { input, error_kind,
  service, region, org, resource_type, resource_id }` and `build_prn_parse_corpus()`. Its inputs are
  the union, in a fixed order, of the `build_prn_canonical_corpus` inputs (they hold the invalid
  cases and an upper-case UUID row) and the `build_prn_fields_corpus` PRNs, with duplicates
  removed. Add one region-ful valid PRN (for example with region `eu-central-1`), because neither
  corpus has one today and `prn-tenancy.ts` branches on region. The expected values come from
  `Prn::parse` and the typed accessors, with the same marshalling code as
  `build_prn_fields_corpus` (share one private helper in the parity crate). They do NOT come from
  `wire::prn_parse_fields`, so the corpus does not replay the function under test against itself.
- `src/bin/gen-parity-vectors.rs`: write `vectors/prn_parse.json`.
- `tests/replay.rs`: `prn_parse_corpus_present_and_fresh`, a replay of every row through
  `paigasus_kernel::wire::prn_parse_fields`, and a cross-check: for each PRN that is in both
  `prn_parse.json` and `prn_fields.json`, the two rows agree field by field.
- `moon.yml` `parity-corpus-drift`: `git diff --exit-code` ignores untracked files, so an
  untracked `prn_parse.json` passes the gate today. Append
  `&& test -z "$(git ls-files --others --exclude-standard -- rs/crates/libs/paigasus-kernel-parity/vectors/)"`
  to the script. Do not use `git status --porcelain`: it also lists a STAGED file, so the gate
  would stay red after `git add`. Prove it: with `prn_parse.json` untracked, the gate reds; after
  `git add`, it is green.
- Update each doc that says "five" corpora to "six": `ts/CLAUDE.md:127`, the headers of
  `tests/wasm-probe.mjs:10` and `tests/committed-wasm.test.ts:9`, the `corpus_path` doc in the
  parity crate, and the parity `README.md` corpus list.

### 4.5 The replays

- `ts/packages/paigasus-kernel/tests/corpus.ts`: `prnParseCases`.
- New `tests/prn-parse.test.ts` (napi, raw `@paigasus/node-bindings` and `@paigasus/kernel/napi`)
  and `tests/prn-parse.wasm.test.ts` (wasm, raw `@paigasus/wasm` and `@paigasus/kernel`). Each
  asserts the raw six-element array AND the `prnParse` result for every row.
- `tests/wasm-probe.mjs`: replay `prn_parse` in `--corpus` mode and report it in `checked`.
- `py/packages/paigasus-kernel/tests/test_parity.py`: `PRN_PARSE_CASES`, the non-empty guard, and
  `test_prn_parse_matches_corpus`, through the six existing accessors (D3).
- Adapter unit tests in `ts/packages/paigasus-kernel/tests/prn-parse-adapter.test.ts`: a wrong
  array length throws; a non-string element throws; an error row with a non-empty field throws; an
  error row maps `errorKind` only; a valid row maps each position to its name (every position has
  a distinct value, so a swap is visible).

### 4.6 `ts/packages/paigasus-console-core/src/`

- `prn-tenancy.ts`: per D4 and D5. The import line becomes
  `import { prnBuild, prnParse } from '@paigasus/kernel';`, plus `import { logger } from './logger';`.
  No export changes.
- `logger.ts`: add `'prn.kernel_call_failed'` to `AppEventName` (D5).

## 5. Tests

### 5.1 Kernel and bindings

The kernel unit tests (4.1), `replay.rs` (4.4), the four replays and the adapter tests (4.5). The
`Exact<…>` guard fails `:typecheck` if the wasm and napi signatures drift.

### 5.2 The delegation test (`tests/unit/prn-tenancy-delegation.test.ts`)

Rewrite the mock around `prnParse`. The mock factory still defines the six old accessors as
`vi.fn()`, so the test can assert they are never called. Mock `./logger` too.

- Every existing case, expressed through `prnParse`'s mock result: a kernel rejection
  (`ok: false`) gives `null`; the fields come from the kernel, not the string; an organization takes
  its org id from the resource id; an organization with an org field gives `null`; another service
  gives `null`; a region gives `null`.
- NEW, the A4 proof: for a tenancy PRN, a principal PRN and `ROOT_PRN`, `prnParse` is called
  exactly once with the PRN, and none of `prnErrorKind`, `prnService`, `prnRegion`, `prnOrg`,
  `prnResourceType`, `prnResourceId` is called.
- NEW: `prnParse` throws (a wasm runtime failure, or the adapter's `TypeError`) → both parsers
  return `null`, and `logger.appEvent` is called once with `'prn.kernel_call_failed'` and the
  error name, and with no field that contains the PRN.
- REPLACE the block "parseTenancyPrn is total when a kernel call fails" (lines 127-159 with its
  doc comment). Today it makes each of the six OLD accessors throw in turn. After the change they
  are never called, so its `it.each` stays green with the `try` deleted. The "prnParse throws"
  case above replaces it. Keep its `prnBuild` case ("does not swallow a failure of the builders")
  unchanged.
- REWRITE the block "parseTenancyPrn bounds the input before the first kernel call" (lines
  98-114 and the boundary case at lines 116-124). Today it asserts that the six OLD accessors are not
  called. After the change they are never called, so the block would stay green with the `MAX_LEN`
  line deleted. The three cases become: an over-long PRN → `prnParse` is not called; an empty PRN
  → `prnParse` is not called; a PRN of exactly `MAX_LEN` characters → `prnParse` is called once
  with that PRN.
- The `prnBuild` cases do not change.

### 5.3 The corpus replay and the source test (`tests/unit/prn-tenancy.test.ts`)

No change to its rows or expectations. It must pass unchanged. That is the A5 proof.

Add to the block "the module holds no PRN grammar of its own": take the source text between
`try {` and `} catch` inside `readKernelFields`, and assert that it contains neither
`TENANCY_KINDS` nor `'principal'`. This pins "IAM's rule stays outside the `try`" as a test, with
the same source-text method the block already uses.

### 5.4 Proof that the tests bite

Per the repo's "delete the feature" rule, run and record in the PR. Each mutation must COMPILE.

- M1. Revert `readKernelFields` to the six accessors. The A4 test must fail (calls counted), and
  the mocked-field tests must fail because the old accessors return `undefined`.
- M2. Swap `org` and `region` in the wire array in `wire::prn_parse_fields`. The Rust, napi and
  wasm corpus replays must fail. (Python does not call the wire function, so it stays green.)
- M3. Remove the `try` in `readKernelFields`. The "prnParse throws" test must fail. (The old
  `it.each` over the six accessors would stay green here, which is why 5.2 replaces it.)
- M4. Delete `prn_parse.json`. The non-empty guards (vitest, `wasm-probe.mjs`, pytest) must fail.
- M5. Break `prnParse` in `src/index.ts` only (the napi entry), for example by passing
  `prn.toUpperCase()`. Only `prn-parse.test.ts` can see this, so a red proves that file runs.
- M6. Swap two names in `toPrnParseResult`. `prn-parse-adapter.test.ts` must red.
- M7. Delete the `MAX_LEN` check in `parseTenancyPrn` and `parsePrincipalPrn`. The rewritten bound
  block (5.2) must red.
- M8. Move the `TENANCY_KINDS` check inside the `try`. The source test (5.3) must red.
- M9. Remove the `logger.appEvent` call from the `catch`. The "prnParse throws" test must red.

Re-run the whole battery after any fix.

### 5.5 The measurement (A6)

The measurement is a one-off. It is not a committed gate, because time varies per host.

- Kernel calls per parse, before and after, for three inputs: a team PRN, `ROOT_PRN`, and a
  gateway PRN. Expected before: 6, 6, 2. Expected after: 1, 1, 1. Take the numbers from the A4
  test's call counts on the old and the new code.
- Raw binding time: a plain node script in the scratchpad loads the wasm glue the same way
  `wasm-probe.mjs` does (`--corpus` mode), and times the six accessors against one
  `prnParseFields` call, 100 000 times per input. This isolates the binding cost. A plain node
  script cannot import `prn-tenancy.ts`: `import 'server-only'` throws without the `react-server`
  condition, and the file is TypeScript source.
- `parseTenancyPrn` time: a bench file run INSIDE the console-core vitest config (so the
  conditions and aliases apply), calling `parseTenancyPrn` 100 000 times on the same three inputs.
  Do NOT commit this file. Report the median of five runs before and after, with the host (the
  development Mac) named.
- Stop rule: if the median `parseTenancyPrn` time for the team PRN does not fall by at least 30%,
  stop before merge. Report the numbers on Linear and let Sven decide. Sven confirmed the 30%
  threshold on 2026-10-02 (Q5). The reason for a threshold: the change adds public API to a published crate and to
  two bindings, and a `Vec<String>` return through wasm-bindgen still makes one JS string and one
  externref slot per element, so the gain is not certain.
- One page render: count the `parseTenancyPrn` and `parsePrincipalPrn` calls that the iam-console
  org page (`orgs/[org]/load.ts` and `node-ref.ts`) makes for a dev-world fixture, and multiply by
  the per-parse saving. State that this is an estimate from the micro-benchmark, not a measured
  render time. Sven confirmed on 2026-10-02 that a micro-benchmark plus a call count is enough
  (Q2). No kind-stack render measurement.

Post the table as a Linear comment on SMA-673 and put it in the PR body.

## 6. Known limits

- L1. The six old accessors stay. Another caller can still read a PRN field by field. No gate stops
  that. Their deprecation is SMA-725 (Q3).
- L2. The wire array is positional. A reorder in the kernel is caught by the corpus replays (M2),
  because the expected values come from the typed accessors, not from the wire function. A
  marshalling defect shared by the wire function and the corpus helper is caught by the
  `replay.rs` cross-check against `prn_fields.json` only for the PRNs that both corpora hold.
- L3. The measurement is a micro-benchmark on one host. It does not prove a page-level gain in
  production.
- L4. `wire` is `#[doc(hidden)]`, but it is still `pub` in a published crate, so semver applies to
  it. A later change to its shape is a breaking change for the crate.
- L5. Python has no one-call parse. A Python caller still reads a PRN field by field (D3, Q1). This
  is Sven's decision, not a gap to close.

## 7. Files to touch

- `rs/crates/libs/paigasus-kernel/src/lib.rs`, new `src/wire.rs`
- `rs/crates/bindings/paigasus-wasm/src/lib.rs` and its five committed artifacts
- `rs/crates/bindings/paigasus-node-bindings/src/lib.rs`, `index.d.ts`, `index.js`
- `rs/crates/libs/paigasus-kernel-parity/src/lib.rs`, `src/bin/gen-parity-vectors.rs`,
  `tests/replay.rs`, new `vectors/prn_parse.json`, `README.md` (list the corpus)
- `moon.yml` (`parity-corpus-drift` script, 4.4)
- `ts/packages/paigasus-kernel/src/index.ts`, `src/wasm.ts`, new `src/prn-parse.ts`,
  `src/binding-parity.types.ts`, `vitest.config.ts`
- `ts/packages/paigasus-kernel/tests/corpus.ts`, `committed-wasm.test.ts`, `wasm-probe.mjs`, new
  `prn-parse.test.ts`, `prn-parse.wasm.test.ts`, `prn-parse-adapter.test.ts`
- `py/packages/paigasus-kernel/tests/test_parity.py`
- `ts/packages/paigasus-console-core/src/prn-tenancy.ts`, `src/logger.ts`,
  `tests/unit/prn-tenancy-delegation.test.ts`, `tests/unit/prn-tenancy.test.ts`
- `ts/CLAUDE.md` ("five" → "six" corpora, lines 125-133)
- Outside the repo: one new entry in the Notion Development Guidelines (A8)

## 8. Gates to run before the push

`:build :test :lint :fmt :typecheck :machete :parity-corpus-drift :wasm-getrandom-free
:version-lockstep :pyo3-stub-drift` at least (`:fmt` includes the ts Prettier gate;
`:pyo3-stub-drift` must stay green with no stub change, which confirms D3). Then the full CLAUDE.md
graph. The kernel crate is published, so `release-plz` will version it; check `:publish-metadata`
and `:release-parity` locally with the right bash (root CLAUDE.md, "This development Mac only").
The host can be in the 512-byte small-pipe state (SMA-612): run a bash-5 gate in a Linux container
when its preflight says so.

Known CI condition on 2026-10-02: `repo:deny` fails on every PR that selects it, because
`yoke-derive` 0.8.3 in `rs/Cargo.lock` is yanked. A separate PR fixes it. If this PR's CI fails
only on `repo:deny` for that reason, report it and do not fix it here.

## 9. Out of scope

- Removing or deprecating the six field accessors. SMA-725 holds the deprecation.
- Changing `prnBuild`, the Cedar accessors, or `MAX_LEN`.
- Any change to IAM's tenancy rule, or to the public surface of `@paigasus/console-core`.
- A committed performance gate.
- A PyO3 `prn_parse_fields` binding and a `Vec<String>` row in `ci/pyo3-stub/check.py` (Q1).
- An ADR-0005 amendment (Q4). The Notion Development Guidelines entry (A8) is in scope instead.

## 10. Open questions (all answered by Sven on 2026-10-02)

- Q1. Does Sven want a PyO3 `prn_parse_fields` too? It needs a gate decision first: a
  RETURN-ONLY `Vec<String>` → `list[str]` mapping in `ci/pyo3-stub/check.py`, with a self-test row
  and a `ci/pyo3-stub/README.md` update, then a hand-edit of the `.pyi`, and `:ruff-ci` in the
  gates. The gate comment says a new row "should stop a human", so this spec does not add it. If
  yes, should Python get a typed wrapper (a NamedTuple or a dataclass), so that Python callers do
  not copy the position map?
  **ANSWER: No.** No PyO3 `prn_parse_fields`. Python keeps its existing accessors (D3, L5).
- Q2. Is a micro-benchmark plus a call count enough for "measure the difference … on a console page
  render", or does Sven want a measured render time of a real page (for example on the kind stack)?
  A kind-stack measurement is much more work and would change the size of this issue.
  **ANSWER: A micro-benchmark plus a call count is enough** (5.5).
- Q3. Should a follow-up issue deprecate the six single-field accessors in the TypeScript package,
  now that `prnParse` exists?
  **ANSWER: Yes.** Filed as SMA-725 (Paigasus Polyglot, milestone Frontend, priority Low,
  `area:frontend`, `area:ffi`, Improvement, related to SMA-673). Not in this issue's scope.
- Q4. This is the first non-scalar return across the bindings, and it sets a positional
  convention. Does it need an ADR-0005 amendment or an entry in the Notion Development Guidelines?
  **ANSWER: A Notion Development Guidelines entry, NOT an ADR-0005 amendment.** It is in scope
  (A8), and the PR body links it.
- Q5. Is a 30% fall in the median `parseTenancyPrn` time (5.5) the right stop threshold?
  **ANSWER: Yes**, a 30% fall in the median `parseTenancyPrn` time.
- Q6. Does Sven accept the new `prn.kernel_call_failed` log event (D5), or does he prefer the
  silent `null` of today?
  **ANSWER: Accepted** (D5).
- Q7. Is a `#[doc(hidden)] pub mod wire` in `paigasus-kernel` acceptable under the `lib.rs` rule
  "No I/O, no FFI", or does he want the wire function in a separate, unpublished crate that the two
  bindings share?
  **ANSWER: Accepted.** `#[doc(hidden)] pub mod wire` in the published `paigasus-kernel` (D2).

## 11. Challenge changelog

Verdict of the spec-challenger: APPROVE WITH CHANGES. Each finding was checked against the repo.

Folded:

- BLOCKER, PyO3 stub gate: confirmed (`check.py:171-179` has no `Vec<String>` row; the stub is
  hand-written, `moon.yml:855-856`). Chose option (a) in a changed form: no PyO3 binding, but Python
  replays the new corpus through its six existing accessors, so no parity exception is needed
  (D3). Option (b) is Q1.
- BLOCKER, vitest includes: confirmed (`vitest.config.ts:31` and `:58` are explicit lists). Added
  the file to section 7, the include edits to 4.3, and mutations M5 and M6.
- MAJOR, vacuous `MAX_LEN` assertions: confirmed (delegation test lines 98-114). 5.2 rewrites the
  block against `prnParse`; mutation M7.
- MAJOR, A6 decision rule and harness: added a stop threshold (Q5), a raw-binding benchmark loaded
  like `wasm-probe.mjs`, and a `parseTenancyPrn` bench inside the console-core vitest config, not
  committed.
- MAJOR, permanent positional API: removed `PrnFields` and `Prn::fields`; the wire function is in a
  `#[doc(hidden)] wire` module (D2, L4, Q7).
- MINOR, A1 against A7: A1 reworded.
- MINOR, wasm export count: 4.3 now measures both lists and uses the hash-regex rule.
- MINOR, factual errors: "6, 6, 3" is now "6, 6, 2" (the `||` short-circuit). The wrong claim that
  the kernel lower-cases fields is removed (upper-case service and region are rejected; only UUIDs
  are lower-cased; `prn_canonical` already has an upper-case UUID row). A region-ful valid row is
  added (confirmed missing from both corpora).
- MINOR, untracked-file gap in `repo:parity-corpus-drift`: the gate script gets a
  `git status --porcelain` check (4.4).
- MINOR, oracle independence: the corpus uses the typed accessors, and `replay.rs` cross-checks
  `prn_parse` rows against `prn_fields` rows (4.4).
- MINOR, silenced defect detector: the `catch` logs a redacted event (D5, Q6). The adapter no longer
  checks non-empty fields (grammar), and it now checks that an error row has five empty fields.
- MINOR, raw re-export: `@paigasus/kernel` exports only `prnParse` (D2). The old Q4 is closed.
- MINOR, "rule outside the `try`" is testable: a source-text test (5.3) and mutation M8.
- MINOR, missing items: the reinstall step after `generate-wasm` (4.2), `index.js` without the
  "if" (4.2), and the "five" → "six" doc edits (4.4).
- QUESTIONS: the ADR question is Q4, the Python wrapper question is in Q1, the threshold is Q5, and
  the Q2 scope note is added.

Rejected:

- The claim that the node-bindings `index.js` and `index.d.ts` change "starts `images.yml` through
  its path filter": the pull-request filter of `images.yml` does not list these files, and its push
  filter holds `rs/**` and `ts/**`, which every PR of this kind matches anyway. No action is needed.
- The citation of `wasm-gate-wiring.test.ts:5-6` as the record of the include-list trap: that file
  does not exist under `ts/packages/paigasus-kernel/tests/`. The trap itself is real and is folded.

## 12. Re-check against `origin/main` on 2026-10-02

The spec was written on 2026-09-27. Every cited path and line was checked again against
`origin/main` at `e5f31a56`. The commits since then touch none of the PRN code. The release
commit `1a45803f` changed `paigasus-node-bindings/index.js`, but line 791 still holds
`module.exports.prnService`.

Changed:

- Section 1: the caller count was "seven modules, 24 call sites". The measured count is eight
  modules and 10 call sites outside tests. The section now lists them.
- D3 and section 11: the hand-written stub note in `moon.yml` is at lines 855-856, not 849.
- 5.2: a stale assumption. The block "parseTenancyPrn is total when a kernel call fails"
  (lines 127-159) makes the six old accessors throw. After the change it is vacuous, so 5.2 now
  replaces it, and M3 says why.
- 5.2: the boundary case of the bound block is at lines 116-124 (the first two cases are still
  lines 98-114).
- 4.3: added the line numbers of the "Twelve" comment (32), the "21 names" message (119) and the
  check 3 corpus list (131).
- Section 7: `ts/CLAUDE.md` "five" is at lines 125-133.
- Sections 2, 6, 7, 9 and 10: Sven's answers to Q1-Q7. New A8 (the Notion entry, Q4).
- Section 8: the small-pipe host note and the known `repo:deny` (`yoke-derive` 0.8.3) CI
  condition.

Confirmed unchanged: `prn-tenancy.ts` (the `||` at line 66, `readKernelFields`, `MAX_LEN`),
`paigasus-wasm/src/lib.rs:41-83`, `ci/pyo3-stub/check.py:171-179`, `logger.ts:7-9` and its
`AppEventName` union, `vitest.config.ts:31` and `:58` (explicit lists), `committed-wasm.test.ts:9`
and `:58-63`, `wasm-probe.mjs:10`, the `parity-corpus-drift` script at `moon.yml:280` (it still
uses `git diff --exit-code` only), the five vector files, and the Python exports of the six
accessors.
