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
  # Second run: JSON only, asserts the gate actually saw files — fails if filesAnalyzed == 0,
  # so a future include/layout change can never silently re-darken the gate (SMA-436).
  script: 'uv run basedpyright && uv run basedpyright --outputjson | python3 scripts/assert_typecheck_coverage.py'
  inputs:
    - '@group(sources)'
    - '@group(tests)'
    - 'pyproject.toml'
    - '/py/uv.lock'
    - 'scripts/**'
```

The guard is a committed, ruff-linted helper `py/scripts/assert_typecheck_coverage.py` (~5 lines):
read JSON from stdin, exit non-zero iff `summary.filesAnalyzed == 0`, with a message such as
*"basedpyright analyzed 0 files — the type gate is vacuous; check `tool.basedpyright.include` in
py/pyproject.toml."* It is the last command in the pipe, so its exit status propagates without
`pipefail`.

**Why double-run over the alternatives:** keeps basedpyright's native (grouped, colored) output
verbatim and keeps the guard single-purpose. The rejected alternatives were a single `--outputjson`
run with Python re-rendering diagnostics (worse DX, brittle to JSON-schema drift) and a separate
`typecheck-coverage` Moon task (extra CI-graph node, decoupled from the gate it protects). The
double analysis is sub-second and the whole task is Moon-cached.

The guard script is ruff-linted (`uv run ruff check .` from `py/` covers `scripts/`) but is **not**
type-checked by the include globs — acceptable; it is tooling, not package code.

### 3. One-shot verification (canary) — during the PR, not committed

Two proofs, each injected then removed:

1. **Include fix catches real errors:** add a deliberate type error to a tracked `src` file →
   `moon run py:typecheck` goes **red** → remove it.
2. **Coverage guard catches zero-files:** temporarily revert the include to the broken
   single-`*` glob → the guard fires **red** on `filesAnalyzed == 0` → restore the fix.

This discharges the issue's "prove the gate's effectiveness rather than assume it" requirement.

## Out of scope

- The repo-root `py/conftest.py` is not under `packages/**` and remains outside the type gate;
  widening coverage to it is a separate concern.
- No change to `lint` / `fmt` / `test` tasks, or to `typeCheckingMode` / `report*` settings.

## Acceptance criteria

- `moon run py:typecheck` analyzes the 6 source/test files (verifiable via `--outputjson`
  `filesAnalyzed == 6`) and passes with 0 errors.
- The task fails red if basedpyright ever analyzes 0 files (coverage guard).
- Both canary proofs demonstrated in the PR description, then reverted.
- `generated/**` remains excluded.

## Files touched

- `py/pyproject.toml` — `include` glob fix + comment.
- `.moon/tasks/python.yml` — `typecheck` task → `script:` with guard + `scripts/**` input.
- `py/scripts/assert_typecheck_coverage.py` — new guard helper.
