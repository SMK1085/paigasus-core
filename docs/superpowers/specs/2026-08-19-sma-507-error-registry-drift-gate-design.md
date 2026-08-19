# SMA-507 — a drift gate for the canonical error-code registry

**Issue:** [SMA-507](https://linear.app/smaschek/issue/SMA-507/repo-two-way-drift-gate-for-the-canonical-error-code-registry)
**ADR:** ADR-0019 — decision E7
**Date:** 2026-08-19
**Status:** draft for GATE 1.

Two of the issue's four acceptance criteria are re-scoped by this design, both deliberately and
both with the issue's own permission or a named successor. See "Departures".

## Problem

Error codes are strings on the wire — `google.rpc.ErrorInfo.reason`, deliberately not a proto
enum, so a new code in one service does not force a bindings regen everywhere. `buf breaking`
therefore cannot see the vocabulary at all: it does not read kebab strings, cannot tell whether a
code is still emitted, and never runs against Rust.

SMA-504 shipped five exhaustive membership tests over that vocabulary. The gap is not that the
assertions are missing — it is **when they run** and **what forces a new site to have one**.

## Evidence

Six findings shaped this design. Each was verified against the tree at `d7a2ccd`, not assumed.

### E1 — a contracts-only change never schedules the tests that would catch it

`.moon/tasks/rust.yml` gives `test` project-relative inputs and **no `deps: ['^:build']`**. Only
`lint` carries that edge (added by SMA-526). So a PR that edits `error.proto` and regenerates
`paigasus-proto/src/generated/**`:

| Task | Scheduled? | Consequence |
|---|---|---|
| `paigasus-iam-rs:lint` | yes, via `^:build` | *compiles* the tests |
| `paigasus-iam-rs:test` | **no** | never *runs* them |

Compilation catches a removed code only where it is referenced as `ErrorReason::Variant`. Every
literal site — all 26 of `TenancyError::code()`, both of `system_retirement.rs` — compiles
happily against a registry that no longer declares the code. **AC 3 is unguarded for exactly the
sites that carry the most codes.**

The one-line fix (give `test` a `^:build`) is rejected by name in that file's own comment: it
would put IAM's Docker-gated container suites on every Dependabot PR.

### E2 — `system_retirement.rs` is already an uncovered emission site

`rs/crates/services/paigasus-iam/src/adapters/http/system_retirement.rs:110,118` emit
`"grants-survive"` and `"decision-change-unacknowledged"` as bare literals in production code,
with **no** membership assertion anywhere. SMA-504's rename inventory noted both were already
canonical and needed no rename — and, having no rename, they got no test either.

This is not hypothetical drift risk. It is the failure mode already present in the tree, and it
is what "a new site relies on its author remembering" looks like in practice.

### E3 — the literal census

46 declared codes (excluding the `UNSPECIFIED` sentinel). Matching a code wrapped in either
`"…"` or `\"…\"`, over production regions only (everything above the first column-0
`#[cfg(test)]`), comment lines filtered:

| prod | comment | test | File |
|---|---|---|---|
| 26 | 0 | 8 | `paigasus-iam/src/application/error.rs` |
| 10 | 0 | 2 | `paigasus-gateway/src/adapters/http/error.rs` |
| 8 | 0 | 9 | `paigasus-iam/src/adapters/http/authn.rs` |
| 2 | 0 | 6 | `paigasus-iam/src/adapters/http/system_retirement.rs` |
| 1 | 0 | 1 | `paigasus-gateway/src/adapters/http/chat.rs` |
| 1 | 0 | 0 | `paigasus-observability/src/grpc.rs` — **false positive** |
| 0 | 47 | 0 | `paigasus-proto/src/generated/…/paigasus.common.v1.rs` |
| 0 | 1 | 49 | `paigasus-proto/src/error.rs` |

Three things follow.

**The comment filter is load-bearing, not cosmetic.** The generated registry module repeats every
kebab spelling in a doc comment carried over from `error.proto`. Without the filter it alone
contributes 47 offenders and the gate is unusable.

**The false positive is real and unavoidable.** `paigasus-observability/src/grpc.rs:62` is
`Internal => "internal"` inside `grpc_code_name`, mapping `tonic::Code` to a **metric label**. It
has nothing to do with the registry; the collision exists because single-word codes (`internal`,
`forbidden`, `not-found`) are ordinary English. It needs one documented exclusion.

**Files hosting a membership test need not contain literals.** `convert.rs` has 13 test-region
hits and zero production hits, yet hosts three of the six tests. The allowlist axis (which files
may spell a code) and the test axis (which assertions must run) are genuinely different.

### E4 — the escaped-quote form, and why one arm cannot be enough

`chat.rs:61` is

```rust
const TERMINAL_SSE_ERROR: &str = "data: {\"error\":{…,\"code\":\"upstream-error\"}}\n\n";
```

The code is embedded in a larger string, so the characters around it are `\"`, not `"`. A grep
anchored on plain quotes does not match it. Widening the anchor to `\\?"<code>\\?"` does, and
this design uses the widened form — but the general lesson stands: **a code composed into a
larger string can evade a literal scan**, and only a membership test covers that site. The two
arms are complementary by construction, not redundant.

### E5 — the exact-name nextest filter works, and its control has a hole

Measured on this worktree:

```
cargo nextest run -p paigasus-iam -p paigasus-gateway --lib \
  -E 'test(=…) or test(=…) or …'
  → Starting 6 tests across 2 binaries (630 tests skipped)
  → Summary [0.013s] 6 tests run: 6 passed
```

Test execution costs **13 milliseconds**. Compiling the two crates under the test profile is the
entire real cost.

The zero-match control fires:

```
-E 'test(=…::this_test_does_not_exist)'
  → error: no tests to run   (rc=4)
```

So this task must **not** pass `--no-tests=pass` — inverting the advice in CLAUDE.md, which
exists for whole-workspace runs. But the control only fires at *zero* matches: rename one of
seven and the other six still run, exit 0, and coverage silently drops. A count assertion is
required on top.

### E6 — `:affected-smoke` does not need re-baselining

`ci/affected-graph/run.sh`'s `assert_case` compares "the affected set **minus `repo`**", and
`assert_task_case` filters to task names `build`/`test`/`lint`. Two new `repo` tasks named
`error-registry-drift` and `error-code-single-site` enter neither set. No expected-set edits.

### E7 — there is no consumer to check

`ts/packages/paigasus-console` and `paigasus-docs` do not exist. `@paigasus/sdk` is
`export {};`. `ErrorReason` *is* generated into TypeScript
(`ts/packages/paigasus-proto/src/generated/paigasus/common/v1/error_pb.ts`), but nothing branches
on it. SMA-508 (`@paigasus/sdk`, Backlog, Frontend milestone) already carries the consumed-side
check as its own **AC 3**: *"Error mapping is a table test driven off the registry, so an unmapped
code fails a test rather than rendering an empty toast."*

## Design

### 1. Two Moon tasks on the root `repo` project

The arms want opposite input scopes, so they are separate tasks with separate names — mirroring
the split the repo already draws between drift gates (`:observability-drift`,
`:parity-corpus-drift`) and single-site gates (`:redis-connect-single-site`,
`:iam-docker-policy-single-site`).

| Task | Arm | Cost | Inputs |
|---|---|---|---|
| `repo:error-registry-drift` | runs the membership tests | compiles 2 crates | narrow: `error.proto`, the covered files, the script, `rs/.config/nextest.toml` |
| `repo:error-code-single-site` | greps for unlisted code literals | a grep | broad: `rs/crates/**/src/**/*.rs`, `error.proto`, the script |

Arm 2's inputs **must** be broad. Narrow inputs would schedule it only when an already-covered
file changes, so a brand-new emission site in a brand-new file would be invisible to the arm whose
entire purpose is finding brand-new emission sites. It stays cheap for the same reason
`repo:actionlint` does: `.moon/workspace.yml`'s `hasher.ignorePatterns` keeps gitignored trees out
of the hash walk.

### 2. `ci/error-registry/check.py` — one script, two modes

Modelled on `ci/affected-graph/cargo_moon_parity.py`: stdlib only, `toolchain: 'system'`, a
`--self-test` mode, and exit codes `0` pass / `1` assertion failure / `2` infrastructure error, so
a broken script aborts rather than folding into a green.

```
check.py --drift        # emit the nextest filter + assert the run count
check.py --single-site  # derive codes, scan, compare against the manifest
check.py --self-test    # exercise the manifest invariants and the parser
```

Codes are derived from `contracts/proto/paigasus/common/v1/error.proto` by the mapping rule that
file states normatively: strip `ERROR_REASON_`, lowercase, `_` → `-`, drop `UNSPECIFIED`. Parsing
the proto rather than the generated Rust keeps the source of truth singular.

### 3. The manifest

Lives in `check.py` as reviewed module-level data — no second file, no TOML parser, and it
diffs legibly. Three tables:

**`SITES`** — production files permitted to spell a code, each naming the test that proves the
site's codes are all declared. Drives arm 2's allowlist *and* contributes to arm 1's filter.

| Site | Guarding test |
|---|---|
| `paigasus-iam/src/application/error.rs` | `adapters::grpc::convert::tests::every_tenancy_code_is_declared_in_the_canonical_registry` |
| `paigasus-gateway/src/adapters/http/error.rs` | `adapters::http::error::tests::every_gateway_code_is_declared_in_the_canonical_registry` |
| `paigasus-iam/src/adapters/http/authn.rs` | `adapters::http::authn::tests::every_authn_http_code_is_in_the_registry` |
| `paigasus-iam/src/adapters/http/system_retirement.rs` | *(new — see §5)* |
| `paigasus-gateway/src/adapters/http/chat.rs` | `adapters::http::chat::tests::the_terminal_sse_frame_carries_a_registered_code` |

**`EXTRA_TESTS`** — membership tests guarding sites that carry no literal, so have no `SITES` row,
but must still run:

- `adapters::grpc::convert::tests::the_bare_status_sites_carry_registered_reasons`
- `adapters::grpc::convert::tests::every_authn_status_carries_a_registered_reason_and_its_original_message`

Both guard sites that already route through `ErrorReason` via `LazyLock` statics (SMA-504).

**`EXCLUSIONS`** — file → reason. One entry today:
`paigasus-observability/src/grpc.rs`, because `grpc_code_name` maps `tonic::Code` to a metric
label and its `"internal"` is not a registry code (E3).

A single manifest, rather than a list per arm, is what stops the two arms drifting: arm 2 asserts
a file is covered, arm 1 proves the covering test actually ran and passed.

### 4. Scanning rules for arm 2

- **Pattern:** each code anchored as `\\?"<code>\\?"`, matching both the plain and escaped-quote
  forms (E4).
- **Scope:** `rs/crates/*/*/src/**/*.rs`.
- **Production region:** everything above the first column-0 `#[cfg(test)]`. Inline test modules
  legitimately assert literal wire values — those assertions are the point, and banning them
  would make the tests tautological.
- **Comment lines** (`^\s*//`, which covers `///`) are filtered (E3).
- Paths are passed explicitly and never anchored on `^\./` — GNU grep emits that prefix and ugrep
  strips it, the portability trap `redis-connect-single-site` documents.

### 5. The seventh test

`system_retirement.rs` gets a membership test in its existing inline `mod tests`, asserting both
codes resolve via `ErrorReason::from_wire_reason`.

It reads the code out of `response_for`'s rendered body rather than restating the literal — a
comparison against the same literal the code is built from would pass even if the code were never
registered (the trap `the_terminal_sse_frame_carries_a_registered_code` documents).

Exhaustiveness is enforced by an `match` over `RetireOutcome` inside the test, **not** by
`strum::EnumIter`: its variants are struct variants carrying `PolicyKind` and `Vec<GrantRef>`, and
`EnumIter` requires `Default` for every field type, which `PolicyKind` does not provide. A new
`RetireOutcome` variant therefore fails to compile the test rather than silently escaping it —
the same guarantee the other four get, reached by the mechanism this type actually supports.

### 6. Controls — the vacuity budget

Every gate here can fail open. Each mode gets a named control; without these the task passes while
guarding nothing, which is the failure `promtool`'s all-firing fixture and the Prometheus
`# TYPE` assertions both taught.

| Failure mode | Control |
|---|---|
| proto parse breaks → empty code set → arm 2 scans for nothing | derived set must be non-empty **and** contain a known anchor code |
| pattern or path wrong → zero hits anywhere → arm 2 passes | hits **inside** `SITES` files must be non-empty |
| a covered file's literals vanish (moved, or hidden below an early `#[cfg(test)]`) | each `SITES` file's production-hit count must be > 0 |
| every membership test renamed | nextest `rc=4`; **never** `--no-tests=pass` |
| *one* membership test renamed | tests-run count must equal `len(SITES) + len(EXTRA_TESTS)` (E5) |
| a `SITES` row names a test that no longer exists | folded into the count control above |
| an `EXCLUSIONS` entry outlives its reason | self-test asserts every excluded file still contains a hit; a stale exclusion reds |

### 7. CI wiring (AC 4)

Both targets appended to `.github/workflows/ci.yml`'s `T=(…)` array and to the full-graph command
documented in CLAUDE.md.

**Known collision:** SMA-541 (in flight, `feature/sma-541-ci-target-coverage-gate`) parses that
same array and asserts it against CLAUDE.md. Whichever lands second rebases onto the other; if
SMA-541 lands first, this change must satisfy its parity assertion rather than only editing the
two files by hand.

## Verification

Negative controls, each run by deliberately breaking the tree and asserting red, then reverting.
`--self-test` covers the parser and manifest invariants in-process; the rest are manual and
recorded in the PR body.

| # | Injected defect | Must red |
|---|---|---|
| 1 | spell a declared code in a **new** production file not on `SITES` | arm 2 |
| 2 | add an undeclared code to `system_retirement.rs` | arm 1 (new test) |
| 3 | delete `ERROR_REASON_SLUG_CONFLICT` from `error.proto` (still emitted as a literal) | arm 1 |
| 4 | rename one membership test | arm 1 (count control) |
| 5 | rename all membership tests | arm 1 (`rc=4`) |
| 6 | corrupt the `ERROR_REASON_` prefix so the parse yields nothing | arm 2 (non-empty control) |
| 7 | remove `paigasus-observability/src/grpc.rs`'s `"internal"` | `--self-test` (stale exclusion) |

Control 3 is the one that fails on `main` today, and is the direct proof that E1's gap was real.

Beyond the controls: the full graph as CI runs it, per CLAUDE.md, including both new targets.

## Limitations

Stated rather than papered over, matching the posture of the two existing single-site gates.

1. **Arm 2 is vocabulary-scoped: it detects a *declared* code in an *unlisted* file, not an
   *undeclared* code anywhere.** It greps for the 46 strings the registry declares, so a new site
   inventing `"widget-jammed"` — a code absent from `error.proto` — produces no hit and passes.
   This is the load-bearing limitation of the whole design and is worth stating precisely:

   | New site emits | Caught by | Why |
   |---|---|---|
   | a declared code, in a file not on `SITES` | arm 2 | it is in the derived set |
   | an undeclared code, added to a covered enum | arm 1 | the membership test enumerates that enum's variants |
   | an undeclared code, at a wholly new site | **nothing** | not in the derived set, and no membership test exists yet |

   Closing row 3 mechanically would mean flagging kebab-case literals *by shape* rather than by
   vocabulary, which collides with ordinary strings (`"content-type"`,
   `"application/json"`, `"paigasus-retryable"`) and would drown the gate in false positives.
   Accepted deliberately. The residual risk is bounded by what such a code is worth: a reason
   absent from the registry does not resolve through `ErrorReason::from_wire_reason` on any
   consumer, which SMA-504's design already treats as the defect — so the code is dead on the wire
   whether or not this gate names it.

2. **Codes composed at runtime are invisible to arm 2.** `format!("{prefix}-conflict")` matches
   nothing. Arm 1 covers such a site only if someone wrote a test for it.
3. **An early column-0 `#[cfg(test)]` in a *new, uncovered* file hides literals below it.** The
   per-file control catches this for `SITES` files only.
4. **Arm 2 proves membership of a file on a list, not the quality of the test named beside it.** A
   `SITES` row whose test asserts nothing useful passes both arms. Review is the control there.
5. **The exclusion list is hand-maintained.** A second genuine `tonic::Code`-style collision needs
   a new entry with a stated reason; the self-test only proves existing entries still match
   something.
6. **`registry ⊆ emitted` is not checked** — see Out of scope.

## Out of scope

- **The consumed side (AC 2).** No consumer exists (E7) and SMA-508's AC 3 already owns it. A
  vacuous TS arm that passes today while guarding nothing was considered and rejected: it is the
  exact "green gate proving nothing" shape this design spends §6 defending against.
- **`registry ⊆ emitted`** — dead registry entries are legal by design. The registry is
  append-only; retracting a code requires reserving name and number, so unused entries are the
  expected steady state, not drift.
- **Refactoring the 47 production literals to `ErrorReason::`.** Considered; it would make an
  undeclared code unwritable and a removed one a compile error. Rejected as disproportionate:
  `TenancyError::code() -> &'static str` would have to change shape across 26 sites and its
  callers, for a guarantee the allowlist already delivers at the granularity that matters.

## Departures

**AC 2 is deferred**, with the issue's explicit permission ("if this lands before the SDK, ship
the emitted-side check first and add the consumed side with `@paigasus/sdk`"). SMA-507's AC 2 is
amended to say so and the handoff recorded on SMA-508, whose AC 3 is already the same
requirement. Nothing is silently dropped.

**AC 1 is delivered by two mechanisms rather than one.** The issue's wording — "adding an
undeclared code to a Rust error enum reds the gate" — is already true today for the three enums
with membership tests. The design reads AC 1 as the stronger claim it must have meant: a new
emission site should not be able to escape unguarded. E2 shows the weaker reading was already
violated in the tree.

That stronger reading is delivered **partially, not fully**, and the boundary is Limitation 1: a
new site reusing declared vocabulary is caught, a new site inventing vocabulary is not. This is a
deliberate trade against a false-positive explosion, not an oversight, and it is called out here
rather than left for a reader to discover from the code.

## Acceptance criteria mapping

| AC | Where | Verified by |
|---|---|---|
| 1 — undeclared code in a Rust error enum reds | arm 1: the membership tests enumerate each covered enum's variants, so an undeclared code fails one. Arm 2 additionally forces a new *site* to register. **Not** covered: an undeclared code at a wholly new site (Limitation 1) | controls 1, 2 |
| 2 — undeclared code in the console copy map reds | **deferred to SMA-508 AC 3** (E7) | — |
| 3 — removing a still-emitted code reds | arm 1, which is the only thing that runs the tests on a contracts-only PR (E1) | control 3 |
| 4 — wired into the `moon ci` target list | §7 | full-graph run |
