# SMA-436 — Fix `py:typecheck` vacuous pass (basedpyright checks zero files)

**Status:** Design approved · **Date:** 2026-06-20 · **Linear:** SMA-436 · Relates to SMA-433

## Problem

The `py:typecheck` CI gate runs bare `uv run basedpyright` from `py/` (per
`.moon/tasks/python.yml`). It reports `0 errors, 0 warnings, 0 notes` and exits `0`, but it
analyzes **zero files** — it prints `No source files found.` The entire Python type gate has
been asserting nothing; type errors under `py/packages/**` ship green. SMA-433's `test_parity.py`
was the first to expose it (a `reportAny` caught only by a *direct* basedpyright run in review,
not by the gate).

## Root cause (verified empirically)

basedpyright **does** load `py/pyproject.toml`; the four `packages/*/src` dirs even appear in its
search paths. The fault is the `include` glob form. Measuring `summary.filesAnalyzed` via
`--outputjson`:

| `tool.basedpyright.include` | files analyzed |
| --- | --- |
| `["packages/*/src", "packages/*/tests"]` ← **current** | **0** |
| `["packages/*/src/**", "packages/*/tests/**"]` | 6 |
| `["packages/**/src", "packages/**/tests"]` | 6 |
| `["packages"]` (+ existing `exclude`) | 6 |
| no-arg config + an explicit path arg | 6 |

pyright's include semantics: a single-`*` glob that **terminates at a directory** (`…/src`)
matches the directory but pulls in none of its `.py` files. It needs a recursive `/**` suffix
(or a `**` in the path, or a plain literal directory pyright walks). The current globs match
nothing, so the gate passes vacuously.

The 6 files that *should* be checked are the four `packages/*/src/<pkg>/__init__.py` plus
`test_parity.py` and `test_health_smoke.py`. Under the **real** config (`typeCheckingMode = "all"`
plus the `report*` overrides), those 6 files currently produce **0 errors** — so fixing the
include yields a green-but-*real* gate, with no hidden backlog of errors to chase. `generated/**`
stays correctly excluded in every working form.

### Confirmed mechanics

- `basedpyright --outputjson` emits `summary.filesAnalyzed` and `summary.errorCount`.
- Exit code is `0` on a clean run **and** on a zero-files run (the trap); `1` on real type errors.
- Injecting a deliberate type error into a tracked `src` file flips the explicit-path run to exit `1`.

## Design

Three separable pieces: the include fix, a durable coverage guard, and one-shot verification.

### 1. Fix the include glob

In `py/pyproject.toml`:

```toml
# Each entry needs a recursive /** suffix: a single-* glob terminating at a directory
# (packages/*/src) matches the dir but collects none of its .py files — that is what made
# py:typecheck vacuous (SMA-436). Do not "simplify" the /** off.
include = ["packages/*/src/**", "packages/*/tests/**"]
```

Chosen over `["packages"]` because it preserves the original intent — type-check **only** `src`
and `tests`, not stray package-root `.py` files. New packages are still covered automatically
(the glob is per-package). `exclude` is unchanged.

### 2. Durable coverage guard (Approach A — double run)

Switch the `typecheck` task in `.moon/tasks/python.yml` from `command:` to `script:`:

```yaml
typecheck:
  # First run: basedpyright's native human output; fails on real type errors.
  # Second run: JSON only, fed to a coverage guard that fails if basedpyright analyzed fewer
  # files than the source tree actually contains — so a future include/layout change can never
  # silently re-darken the gate, totally OR partially (SMA-436).
  script: 'uv run basedpyright && uv run basedpyright --outputjson | python3 scripts/assert_typecheck_coverage.py'
  inputs:
    - '@group(sources)'
    - '@group(tests)'
    - 'pyproject.toml'
    - '/py/uv.lock'
    - 'scripts/**'
```

The guard is a committed, ruff-linted helper `py/scripts/assert_typecheck_coverage.py`. It asserts a
**derived floor**, not merely non-zero:

1. Read `summary.filesAnalyzed` from stdin (the `--outputjson` payload).
2. Compute the *expected* count by globbing the source tree with the same intent as the include —
   `packages/*/src/**/*.py` and `packages/*/tests/**/*.py`, minus the `exclude` basenames
   (`generated`, `__pycache__`, …). Verified to equal `filesAnalyzed` exactly today (6 == 6).
3. Fail (exit 1) if `filesAnalyzed < expected`, with a message naming both counts and pointing at
   `tool.basedpyright.include`.

**Why a floor, not `== 0`:** `== 0` only catches *total* darkness. The likelier future
recurrence is *partial* darkness — a new package whose layout doesn't fit `packages/*/src/**`, or an
over-narrow edit — which leaves `filesAnalyzed > 0` and would pass a non-zero check. A package-count
floor (`>= number of packages`) doesn't fix it either: a broken new package still leaves the count
above that floor (verified). A floor derived from the actual on-disk file count is the form that
trips on partial drops, and because it's computed from the tree it never churns when files are
legitimately added (no hardcoded number to bump, no temptation to lower it). The guard re-globbing
the tree is deliberate — cross-checking basedpyright's resolution against filesystem truth is the
entire mechanism. `--outputjson` exposes only the *count*, not the analyzed-file list, so a count
floor is the practical durable ceiling; the floor's patterns/excludes mirror
`tool.basedpyright.{include,exclude}` and must move with them (a deliberate two-place coupling).

**Fail-closed, legibly:** on empty stdin, non-JSON, or a missing key (basedpyright crash or a
future `--outputjson` schema change under the `basedpyright<2` pin), the guard exits with a
*distinct* code and message (e.g. *"couldn't read basedpyright coverage JSON — schema changed?"*)
rather than an opaque traceback indistinguishable from the vacuous-gate signal. Both directions are
red; the operator can tell them apart. The guard is the last command in the pipe, so its exit status
propagates without `pipefail`.

The guard's complete exit-code contract:

| code | condition | meaning |
| --- | --- | --- |
| `0` | `filesAnalyzed >= expected` | gate saw the full source tree (pass) |
| `1` | `filesAnalyzed < expected` | total or partial coverage darkening — vacuous gate |
| `2` | empty stdin / non-JSON / missing key | unreadable `--outputjson` (fail-closed) |
| `3` | `expected == 0` | source tree appears empty (`packages/*` layout moved) — the guard refuses to pass vacuously on its own broken view |

(Exit `3`, `EXIT_NO_PACKAGES`, was added during the code-review loop as a further fail-closed
hardening: a guard whose job is to detect "zero files analyzed" must not itself pass when its own
glob finds nothing.)

**Why double-run over the alternatives:** keeps basedpyright's native (grouped, colored) output
verbatim and keeps the guard single-purpose. A separate `typecheck-coverage` Moon task was rejected
(extra CI-graph node, decoupled from the gate it protects). Note the double-run *also* parses
`--outputjson` for the guard, so it carries the same JSON dependency a single-run design would — the
honest reason to prefer it is native output on the error path, not schema-coupling avoidance.

**Cost:** the gate runs two full `typeCheckingMode = "all"` passes; the guard's expected-count
step is a filesystem glob (no third analysis). Sub-second today, Moon-cached and affected-gated, but
it scales ~linearly with the py tree. If it ever bites, the single-run-parse-both shape reclaims the
2× at the cost of re-rendering diagnostics.

The guard script is ruff-linted (`uv run ruff check .` from `py/` covers `scripts/`) but is **not**
type-checked by the include globs — acceptable; it is tooling, not package code.

### 3. One-shot verification (canary) — during the PR, not committed

Two proofs, each injected then removed:

1. **Include fix catches real errors:** add a deliberate type error to a tracked `src` file →
   `moon run py:typecheck` goes **red** → remove it.
2. **Coverage floor catches darkening:** temporarily revert the include to the broken single-`*`
   glob → the guard fires **red** because `filesAnalyzed` drops below the derived floor → restore
   the fix. (A total revert drops it to 0; an over-narrow edit drops it below the expected count —
   both trip the floor.)

This discharges the issue's "prove the gate's effectiveness rather than assume it" requirement.

### Residual risk: potency is not durably guarded

A type gate can go vacuous two ways: it analyzes too few files (closed by the coverage floor above),
or it analyzes the right files under a *toothless ruleset* — a future downgrade of `typeCheckingMode`,
`report* = "none"`, or a broad `# pyright: ignore`. The durable guard does **not** close the second;
this is an accepted residual risk for now, because:

- Potency regressions are deliberate, PR-visible config edits — categorically different from the
  silent, honest glob mistake this issue fixes; code review is the proportionate control.
- `reportUnnecessaryTypeIgnoreComment = "error"` (already enabled, verified) is a weak indirect
  tripwire: if checking silently went toothless, previously-necessary ignores would start erroring.
- The §3 canary proves potency once, at PR time.

A durable potency guard (a committed known-bad fixture asserted to exit 1, isolated from the coverage
set) is deferred: it needs a third basedpyright pass and a deliberately-broken committed file, not
justified at the current py size. Revisit if the py stack grows substantial typed code.

## Out of scope

- The repo-root `py/conftest.py` is not under `packages/**` and remains outside the type gate;
  widening coverage to it is a separate concern.
- No change to `lint` / `fmt` / `test` tasks, or to `typeCheckingMode` / `report*` settings.

## Acceptance criteria

- `moon run py:typecheck` analyzes the 6 source/test files (verifiable via `--outputjson`
  `filesAnalyzed == 6`) and passes with 0 errors.
- The task fails red if basedpyright analyzes fewer files than the source tree contains
  (coverage floor) — catching total *and* partial darkening.
- The guard fails with a distinct message/exit code on unreadable `--outputjson` (fail-closed).
- Both canary proofs demonstrated in the PR description, then reverted.
- `generated/**` remains excluded.
- Gate *potency* (the ruleset still has teeth) is an accepted, documented residual risk — not
  durably guarded.

## Files touched

- `py/pyproject.toml` — `include` glob fix + comment.
- `.moon/tasks/python.yml` — `typecheck` task → `script:` with guard + `scripts/**` input.
- `py/scripts/assert_typecheck_coverage.py` — new guard helper (derived coverage floor +
  fail-closed JSON parsing).
