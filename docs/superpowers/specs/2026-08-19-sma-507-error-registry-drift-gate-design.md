# SMA-507 — a drift gate for the canonical error-code registry

**Issue:** [SMA-507](https://linear.app/smaschek/issue/SMA-507/repo-two-way-drift-gate-for-the-canonical-error-code-registry)
**ADR:** ADR-0019 — decision E7
**Date:** 2026-08-19
**Status:** revised after adversarial challenge (round 1). The first draft's central premise was
measured to be false and its main deliverable has been deleted — see "Departures".

## Problem

Error codes are strings on the wire — `google.rpc.ErrorInfo.reason`, deliberately not a proto
enum, so a new code in one service does not force a bindings regen everywhere. `buf breaking`
therefore cannot see the vocabulary at all: it does not read kebab strings, cannot tell whether a
code is still emitted, and never runs against Rust.

The issue assumes two things are unguarded — new codes and removed codes. Measurement says only
one is. This design guards that one and records why the other needs nothing.

## Evidence

Seven findings. Each was verified against the tree at `d7a2ccd` by running the command shown, not
by reading configuration and inferring.

### E1 — the scheduling gap does not exist; the membership tests already run

The first draft claimed `test` carries no `^:build`, citing `.moon/tasks/rust.yml`. That file is
not the whole story: **both service crates declare the edge per-crate** —
`rs/crates/services/paigasus-iam/moon.yml:30` and
`rs/crates/services/paigasus-gateway/moon.yml:22` each carry `test: deps: ['^:build']`. Measured:

```
$ printf 'contracts/proto/paigasus/common/v1/error.proto\n' \
    | moon query tasks --affected --downstream deep
contracts:                ['breaking', 'fmt', 'generate', 'lint']
paigasus-gateway-rs:      ['build', 'lint', 'test']
paigasus-iam-rs:          ['build', 'lint', 'test']
paigasus-proto-rs:        ['build', 'lint', 'test']
paigasus-proto-ts:        ['build', 'test', 'typecheck']
paigasus-service-info-rs: ['build', 'lint', 'test']
repo:                     ['actionlint']
```

A contracts-only change schedules `paigasus-iam-rs:test` and `paigasus-gateway-rs:test`. All six
membership tests therefore run on exactly the PRs that could remove a code. **AC 3 is already
satisfied by machinery that exists**, and the first draft's `repo:error-registry-drift` task — a
nextest filter, a count control, a manifest of test names — would have been pure duplication.

This is also why the structure is guaranteed rather than incidental:
`ci/affected-graph/cargo_moon_parity.py` (A3) fails any crate whose `build`/`test`/`lint` does not
schedule every upstream's `:build`, and `ci/affected-graph/run.sh`'s `proto->service-info-tasks`
case asserts these exact task rows. The edge cannot silently disappear.

### E2 — `system_retirement.rs` is an uncovered emission site, today

`rs/crates/services/paigasus-iam/src/adapters/http/system_retirement.rs:110,118` emit
`"grants-survive"` and `"decision-change-unacknowledged"` as bare literals, with **no** membership
assertion anywhere. SMA-504's rename inventory noted both were already canonical and needed no
rename — and, having no rename, they got no test.

This is not hypothetical. It is what "a new site relies on its author remembering" already looks
like in this tree, and it is the gap that justifies this issue.

### E3 — a second uncovered branch, inside a file that *is* covered

`adapters/http/authn.rs:87,91` — `envelope_rejection` — emits `"request-too-large"` and
`"invalid-request-body"` as literals chosen by an `if`, not by an enum. Its membership test
(`authn.rs:274-285`) enumerates `AuthnError` variants and then **hand-restates those same two
strings** at `:277`:

```rust
let mut codes = vec!["request-too-large".to_owned(), "invalid-request-body".to_owned()];
```

Add a third branch to `envelope_rejection` and nothing catches it: the file is covered, and the
test enumerates an enum the branch is not in. Per-file coverage is therefore weaker than
"every code this file emits is checked", and the design must not claim otherwise.

### E4 — the literal census

46 declared codes (excluding the `UNSPECIFIED` sentinel). Matching a code wrapped in either
`"…"` or `\"…\"` across `rs/crates/*/*/src/**/*.rs`. Complete — every file with a hit is listed:

| emits | asserts | File | Note |
|---|---|---|---|
| 26 | 8 | `paigasus-iam/src/application/error.rs` | `TenancyError::code()` |
| 10 | 2 | `paigasus-gateway/src/adapters/http/error.rs` | `GatewayError::parts()` |
| 8 | 9 | `paigasus-iam/src/adapters/http/authn.rs` | authn funnel + `envelope_rejection` (E3) |
| 2 | 6 | `paigasus-iam/src/adapters/http/system_retirement.rs` | uncovered (E2) |
| 1 | 1 | `paigasus-gateway/src/adapters/http/chat.rs` | terminal SSE frame (E5) |
| 1 | 0 | `paigasus-observability/src/grpc.rs` | **false positive** |
| 0 | 13 | `paigasus-iam/src/adapters/grpc/convert.rs` | hosts three membership tests |
| 0 | 49 | `paigasus-proto/src/error.rs` | `EXPECTED_REASONS` |
| 0 | 3 | `paigasus-iam/src/adapters/http/error.rs` | test assertions only |
| 0 | 1 | `paigasus-gateway/src/adapters/http/auth.rs` | test assertion only |
| 0 | 1 | `paigasus-iam/src/application/create_user.rs` | test assertion only |
| 0 | 47 comments | `paigasus-proto/src/generated/…/paigasus.common.v1.rs` | generated |

**The false positive is real and unavoidable.** `paigasus-observability/src/grpc.rs:62` is
`Internal => "internal"` inside `grpc_code_name`, mapping `tonic::Code` to a **metric label**. The
collision exists because single-word codes (`internal`, `forbidden`, `not-found`) are ordinary
English.

**The generated module needs a path exclusion, not a comment filter.** Its 47 hits are all `///`
doc comments carried over from `error.proto`, and it contains no `#[cfg(test)]` at all. Relying on
a comment filter makes the gate hostage to prost's doc style: were prost to emit `#[doc = "…"]`,
those 47 lines become offenders overnight in a file nobody here authors.

**Files hosting a membership test need not emit.** `convert.rs` has zero emissions and hosts three
of the six tests. Emission and assertion are different axes.

### E5 — the escaped-quote form

`chat.rs:61` is

```rust
const TERMINAL_SSE_ERROR: &str = "data: {\"error\":{…,\"code\":\"upstream-error\"}}\n\n";
```

The code sits inside a larger string, so the surrounding characters are `\"`, not `"`. An anchor
on plain quotes misses it; `\\?"<code>\\?"` catches it, with zero additional false positives
across the scan scope (independently reproduced during the challenge: 187 hits → 189, both new
ones in `chat.rs`). The general lesson stands — a code composed into a larger string can evade a
literal scan.

### E6 — "everything above the first `#[cfg(test)]`" is an unsound production/test split

The first draft proposed cutting each file at its first column-0 `#[cfg(test)]`. Audited across
the scan scope, **seven files** have a first column-0 `#[cfg(test)]` that is not a test module:

| File | cut at | of | production lines lost |
|---|---|---|---|
| `paigasus-iam/src/config.rs` | 1316 | 3318 | 2002 |
| `paigasus-iam/src/adapters/redis_conn.rs` | 138 | 1197 | 1059 |
| `paigasus-iam-core/src/authz/roles.rs` | 84 | 749 | 665 |
| `paigasus-iam/src/adapters/events/relay.rs` | 33 | 539 | 506 |
| `paigasus-gateway/src/config.rs` | 284 | 538 | 254 |
| `paigasus-iam/src/adapters/retryable.rs` | 35 | 100 | 65 |
| `paigasus-iam/src/application/mod.rs` | 15 | 25 | 10 |

Roughly **4,560 production lines** would be silently exempt — including `relay.rs`, an adapter and
a plausible future emission site. The heuristic is deleted from this design (§2).

### E7 — there is a console, but nothing consumes the vocabulary

Correcting the first draft, which claimed the console did not exist. It does:
`ts/apps/paigasus-console/` is a Moon project (`.moon/workspace.yml`) with `next.config.ts`,
`app/layout.tsx` and `app/page.tsx` — a scaffold with no error handling. `@paigasus/sdk` is
`export {};`. `ErrorReason` **is** generated into TypeScript
(`ts/packages/paigasus-proto/src/generated/paigasus/common/v1/error_pb.ts`) but is imported
nowhere.

So there is no error-copy map to check. SMA-508 (`@paigasus/sdk`, Backlog, Frontend milestone)
already carries the consumed-side check as its own **AC 3**: *"Error mapping is a table test
driven off the registry, so an unmapped code fails a test rather than rendering an empty toast."*

## Design

### 1. One Moon task: `repo:error-code-single-site`

The scheduling arm is gone (E1). What remains is discovery: **a file that spells registry
vocabulary must be on a reviewed list.** That is the shape of `repo:redis-connect-single-site` and
`repo:iam-docker-policy-single-site`, and the name says so.

| | |
|---|---|
| Script | `set -euo pipefail`; `python3 ci/error-registry/check.py --self-test`; then `--single-site` |
| Toolchain | `system` (no cargo, no `cd rs` needed — this task never invokes cargo) |
| Inputs | `rs/crates/**/src/**/*.rs`, `contracts/proto/paigasus/common/v1/error.proto`, `ci/error-registry/**` |

Inputs **must** be broad. Narrow inputs would schedule the gate only when an already-listed file
changes, so a new emission site in a new file would be invisible to the one gate whose purpose is
finding new emission sites. It stays cheap for the same reason `repo:actionlint` does:
`.moon/workspace.yml`'s `hasher.ignorePatterns` keeps gitignored trees out of the hash walk.

`--self-test` runs **first, in the same script block**, following `moon.yml`'s `affected-smoke`
and `publish-metadata` precedent, so a rotted checker cannot ship green. `set -euo pipefail` is
mandatory: Moon does not enable errexit for `script:` blocks, so without it a failing `--self-test`
would be masked by a passing `--single-site`.

### 2. No production/test split

E6 kills the heuristic, and nothing replaces it: **the whole file is scanned, and every file that
spells a code is listed.** This is simpler and strictly sounder — there is no region logic left to
be wrong.

The cost is that files containing only *test* assertions are on the list too. That is acceptable
and arguably right: adding a code literal to a new file becomes a conscious act either way, and
the manifest's `role` field records which kind each file is.

### 3. `ci/error-registry/check.py`

Modelled on `ci/affected-graph/cargo_moon_parity.py`: stdlib only, never shells out to cargo, and
exit codes `0` pass / `1` assertion failure / `2` infrastructure error. The Moon script does not
interpret `1` vs `2` (Moon reds on any non-zero); the distinction exists so a future caller can,
and so the failure text is unambiguous.

**Code derivation.** Anchored on `^\s*ERROR_REASON_([A-Z0-9_]+)\s*=\s*\d+;\s*$` within the
`enum ErrorReason` block, dropping `UNSPECIFIED`, then applying the rule `error.proto` states
normatively: lowercase, `_` → `-`. The anchor matters: a bare `ERROR_REASON_[A-Z0-9_]+` scan also
matches the prefix and the value names mentioned in that file's own prose comments.

**Scan.** `rs/crates/*/*/src/**/*.rs`, excluding `**/src/generated/**` by path (E4). Pattern
`\\?"<code>\\?"` (E5).

### 4. The manifest

Module-level data in `check.py` — no second file and no TOML parser, and it diffs legibly. One
row per file, with a role and a reason:

| Role | File | Guard / reason |
|---|---|---|
| `emits` | `paigasus-iam/src/application/error.rs` | `every_tenancy_code_is_declared_in_the_canonical_registry` |
| `emits` | `paigasus-gateway/src/adapters/http/error.rs` | `every_gateway_code_is_declared_in_the_canonical_registry` |
| `emits` | `paigasus-iam/src/adapters/http/authn.rs` | `every_authn_http_code_is_in_the_registry` (partial — E3) |
| `emits` | `paigasus-iam/src/adapters/http/system_retirement.rs` | *new — §5* |
| `emits` | `paigasus-gateway/src/adapters/http/chat.rs` | `the_terminal_sse_frame_carries_a_registered_code` |
| `asserts` | `paigasus-iam/src/adapters/grpc/convert.rs` | hosts three membership tests |
| `asserts` | `paigasus-proto/src/error.rs` | `EXPECTED_REASONS`, the registry's own mirror |
| `asserts` | `paigasus-iam/src/adapters/http/error.rs` | test assertions only |
| `asserts` | `paigasus-gateway/src/adapters/http/auth.rs` | test assertion only |
| `asserts` | `paigasus-iam/src/application/create_user.rs` | test assertion only |
| `excluded` | `paigasus-observability/src/grpc.rs` | `grpc_code_name` maps `tonic::Code` to a metric label; its `"internal"` is not a registry code |
| `excluded` | `**/src/generated/**` | prost output, doc comments only, not authored here |

For every `emits` row the named guard must still exist. `check.py` asserts this by grepping the
crate for `fn <name>`. That is cheap, and it recovers the only part of the deleted arm 1 that was
ever load-bearing: deleting a membership test now reds this gate, even though nothing here runs it.

### 5. The seventh membership test

`system_retirement.rs` gets a membership test in its existing inline `mod tests`, closing E2.

It reads the code out of `response_for`'s rendered body rather than restating the literal — a
comparison against the same literal the code is built from passes even if the code was never
registered (the trap `the_terminal_sse_frame_carries_a_registered_code` documents).

Exhaustiveness comes from a `match` over `RetireOutcome` inside the test, **not** from
`strum::EnumIter`: its variants are struct variants carrying `PolicyKind` and `Vec<GrantRef>`, and
`EnumIter` requires `Default` for every field type. `RetireOutcome` is not `#[non_exhaustive]`
(`paigasus-iam-core/src/authz/retirement.rs:71-87`), so a cross-crate `match` is genuinely
exhaustive and a new variant fails to compile the test. The `Retired` arm asserts **no** `code` is
present, so a future `Retired` that grows one cannot escape.

### 6. Fixing `envelope_rejection`'s coverage (E3)

`every_authn_http_code_is_in_the_registry` stops hand-restating the two literals and instead drives
real `JsonRejection`s through `envelope_rejection`, reading the codes out of the rendered bodies.
A third branch is then covered automatically.

### 7. Controls — the vacuity budget

Every gate can fail open. Each mode gets a named control; without these the task passes while
guarding nothing.

| Failure mode | Control |
|---|---|
| proto parse breaks → few or no codes derived → scan finds nothing | derived set must equal `EXPECTED_REASONS` in `paigasus-proto/src/error.rs:154` **as a set**, and number 46, matching that file's own `:217` anchor |
| pattern or path wrong → zero hits anywhere | total hits across listed files must be non-empty |
| a listed file stops containing any code (moved, renamed, deleted) | each `emits`/`asserts`/`excluded` row must still match ≥ 1 hit; a stale row reds |
| a membership test is deleted | each `emits` row's named guard must still be found as `fn <name>` |
| the checker itself rots | `--self-test` runs first in the same script block |

The first control is the important one, and it is a genuine cross-check rather than a smoke test.
The first draft proposed "non-empty and contains a known anchor code", which a parser returning 3
of 46 would pass. Comparing against `EXPECTED_REASONS` makes the Python parser and the Rust
registry mutually validating: they are independent transcriptions of the same proto, so they can
only agree if both are right. It also answers the "third copy of the mapping rule" objection —
the copy exists, but it is now checked against the second copy on every run.

### 8. CI wiring (AC 4)

`:error-code-single-site` appended to `.github/workflows/ci.yml`'s `T=(…)` array and to the
full-graph command documented in CLAUDE.md. One target, not two.

**Known collision:** SMA-541 (in flight, `feature/sma-541-ci-target-coverage-gate`) parses that
same array and asserts it against CLAUDE.md. Whichever lands second rebases onto the other; if
SMA-541 lands first, this change must satisfy its parity assertion rather than only editing the
two files by hand.

`:affected-smoke` needs no re-baselining: `ci/affected-graph/run.sh:30`'s `assert_case` compares
the affected set **minus `repo`**, and `assert_task_case` filters to task names `build`/`test`/
`lint`. A `repo` task named `error-code-single-site` enters neither, and
`cargo_moon_parity.py`'s FFI markers do not match its script.

## Verification

Each control is exercised by injecting the defect, observing which task reds, and reverting. The
task that reds is recorded — a control that cannot attribute its red is not a control.

| # | Injected defect | Must red | Attribution risk |
|---|---|---|---|
| 1 | spell a declared code in a new `src/` file not on the manifest | `repo:error-code-single-site` | none — no other gate reads that file |
| 2 | add an undeclared code to `system_retirement.rs` | `paigasus-iam-rs:test` (new test, §5) | none |
| 3 | add a third `envelope_rejection` branch with an undeclared code | `paigasus-iam-rs:test` (§6) | none |
| 4 | delete a manifest row whose file still contains a hit | `repo:error-code-single-site` | none |
| 5 | delete `every_tenancy_code_is_declared_in_the_canonical_registry` | `repo:error-code-single-site` (guard control) | also reds `paigasus-iam-rs:test`? no — deleting a test cannot fail it |
| 6 | remove `ERROR_REASON_SLUG_CONFLICT` from `error.proto`, regenerate, **and** update `EXPECTED_REASONS` and the `46` anchor | `paigasus-iam-rs:test` | the extra edits are required precisely so the red is attributable to the membership test rather than to `paigasus-proto-rs:test` |
| 7 | corrupt the derivation so it yields a subset | `repo:error-code-single-site` (set-equality control) | exercised in `--self-test` against an in-process fixture, not by mutating the real proto |

Control 6 is the AC 3 proof. It must be run in its full form: the naive version (delete the value
and stop) reds `paigasus-proto-rs:test` and `contracts:lint` first and proves nothing about the
membership tests.

Beyond the controls: the full graph as CI runs it, per CLAUDE.md, including the new target.

## Limitations

Stated rather than papered over, matching the posture of the two existing single-site gates.

1. **The gate is vocabulary-scoped: it detects a *declared* code in an *unlisted* file, not an
   *undeclared* code anywhere.** It greps for the 46 strings the registry declares, so a new site
   inventing `"widget-jammed"` produces no hit and passes.

   | New site emits | Caught by | Why |
   |---|---|---|
   | a declared code, in an unlisted file | this gate | it is in the derived set |
   | an undeclared code, added to a covered enum | that enum's membership test | the test enumerates variants |
   | an undeclared code, in a listed file but outside the guarded enum | **nothing** | E3's shape |
   | an undeclared code, at a wholly new site | **nothing** | not in the derived set, no test exists yet |

   Closing rows 3 and 4 mechanically would mean flagging kebab literals *by shape* rather than by
   vocabulary, which collides with ordinary strings (`"content-type"`, `"application/json"`,
   `"paigasus-retryable"`) and would drown the gate in false positives. Accepted deliberately. The
   residual risk is bounded by what such a code is worth: a reason absent from the registry
   resolves through `ErrorReason::from_wire_reason` on no consumer, which SMA-504 already treats as
   the defect — so the code is dead on the wire whether or not this gate names it.

2. **Codes composed at runtime are invisible.** `format!("{prefix}-conflict")` matches nothing.

3. **Block comments are not filtered.** There is no comment filter at all now (§2), so a
   `/* … */` or `///` mention of a code in an unlisted file reds the gate. Same class of gap
   `redis-connect-single-site` documents, inverted: here it produces a false positive, not a false
   negative, so it fails safe and is fixed by listing the file.

4. **A manifest row asserts a file was reviewed, not that its guard is good.** A row whose named
   test asserts nothing useful passes. Review is the control there.

5. **The exclusion rows are hand-maintained.** A second `tonic::Code`-style collision needs a new
   row with a stated reason.

6. **Scope is `rs/crates/*/*/src/**`.** `tests/`, `benches/`, `build.rs` and the `py/`/`ts/`
   workspaces are out. Emissions live in `src/`; integration tests under `tests/` assert against
   the wire and are not emission sites. A future non-Rust service that emits codes needs the scan
   widened.

7. **`registry ⊆ emitted` is not checked** — see Out of scope.

## Out of scope

- **The consumed side (AC 2).** Nothing consumes the vocabulary (E7) and SMA-508's AC 3 already
  owns it. A vacuous TS arm that passes today while guarding nothing was considered and rejected:
  it is the exact "green gate proving nothing" shape §7 exists to prevent.
- **`registry ⊆ emitted`** — dead registry entries are legal by design. The registry is
  append-only; retracting a code requires reserving name and number, so unused entries are the
  expected steady state, not drift.
- **Refactoring the 47 production literals to `ErrorReason::`.** Considered; it would make an
  undeclared code unwritable. Rejected as disproportionate: `TenancyError::code() -> &'static str`
  would have to change shape across 26 sites and its callers.

## Departures

**`repo:error-registry-drift` is deleted.** The first draft's premise — that a contracts-only
change never schedules the membership tests — was measured false (E1). AC 3 needs no new code.

**AC 2 is deferred**, with the issue's explicit permission ("if this lands before the SDK, ship
the emitted-side check first and add the consumed side with `@paigasus/sdk`"). SMA-507's AC 2 is
amended to say so and the handoff recorded on SMA-508, whose AC 3 is the same requirement.

**AC 1 is read as the stronger claim.** Its literal wording — "adding an undeclared code to a Rust
error enum reds the gate" — is already true for the three enums with membership tests. The design
reads it as: a new emission site should not escape unguarded. E2 and E3 show the weaker reading was
already violated twice in the tree. The stronger reading is delivered **partially**, and the
boundary is Limitation 1.

## Acceptance criteria mapping

| AC | Where | Verified by |
|---|---|---|
| 1 — undeclared code in a Rust error enum reds | existing membership tests, plus the new one (§5) and the widened one (§6); the gate forces a new *site* to register | controls 1–3 |
| 2 — undeclared code in the console copy map reds | **deferred to SMA-508 AC 3** (E7) | — |
| 3 — removing a still-emitted code reds | **already satisfied** — a contracts change schedules both services' `:test` (E1) | control 6 |
| 4 — wired into the `moon ci` target list | §8 | full-graph run |
