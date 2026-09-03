# SMA-610 — `py:test` passes silently when one package's `tests/` directory moves

## Problem

`py/pyproject.toml` sets `testpaths = ["packages/*/tests"]`. pytest expands each entry with
`glob.iglob` and **concatenates** the results (`_pytest/config/__init__.py:1411-1438`, read
against the pinned pytest 9.1.1). If ONE package's `tests/` directory is deleted, moved or
renamed, the glob still resolves to the survivors, pytest collects them, and the run is green.

`py:test` therefore cannot distinguish "the suite passed" from "the suite is gone". This is the
SMA-553 class — a gate silently switching off when a directory moves — on the py side, where
`repo:input-liveness` structurally cannot reach: `ci/affected-graph/task_inputs.py`'s
`_repo_tasks` is keyed to `projects.get("repo")` by exact project id, so it liveness-checks
`repo:*` tasks and nothing else.

## Measurements

Re-derived on this branch, not carried over from the issue. Host: uv 0.11.16, pytest 9.1.1,
CPython 3.12.13, `uv run pytest -q` from `py/`. The tree was restored between rows and
`git status` was empty afterwards.

| # | tree state | `filterwarnings` | exit | collected |
| -- | -- | -- | -- | -- |
| M1 | intact | no | 0 | 134 passed |
| M2 | `packages/paigasus-kernel/tests` moved aside | no | **0** | **7 passed** |
| M3 | both `tests/` dirs moved aside | no | 5 | 0 — `PytestConfigWarning` emitted |
| M4 | both moved aside | yes | **1** | 0 — hard config-time error |
| M5 | `packages/paigasus-kernel/tests` moved aside | yes | **0** | **7 passed** |
| M6 | intact | yes | 0 | 134 passed |

M2 is the defect. M5 is the load-bearing measurement for this design: it proves that
`filterwarnings` — the issue's own headline recommendation — **cannot** satisfy the issue's
AC 1, because no warning is issued on partial loss at all. M6 shows `filterwarnings` causes no
regression on the intact tree.

## What does NOT already cover this

* **The removed conftest shim never did.** Its guard fired only when NO `packages/*/tests`
  existed (total loss).
* **Total loss is covered, and only accidentally.** M3's exit 5 arises because pytest falls back
  to recursive collection from `py/` and nothing test-shaped survives there (`.venv` is excluded
  by `norecursedirs`' `.*`). That is a property of the current tree, not a guarantee.
* **`assert_typecheck_coverage.py` does not.** Its `INCLUDE_GLOBS` cover `packages/*/tests`, but
  it compares a disk-derived count against basedpyright's `filesAnalyzed` — delete a tests dir
  and BOTH numbers drop, so it stays green. Stated explicitly so a future reader does not assume
  the SMA-436 guard has this.
* **Moon affectedness does not.** `py:test` keys on `packages/*/tests/**/*`, so a deletion does
  select the task — it just passes.

## Design

Two independent changes. Neither subsumes the other: M4 shows `filterwarnings` is what makes
total loss loud at config time, and M5 shows only the floor can see partial loss.

### 1. Error on `PytestConfigWarning`

In `py/pyproject.toml`'s `[tool.pytest.ini_options]`:

```toml
filterwarnings = ["error::pytest.PytestConfigWarning"]
```

`issue_config_time_warning` applies ini `filterwarnings` (`_pytest/config/__init__.py:1649-1654`),
so the total-loss case becomes a hard error (M3 exit 5 → M4 exit 1) with a message naming
`testpaths`, instead of an exit code that happens to be non-zero for an unrelated reason. The
entry is appended to pytest's default filters, so only `PytestConfigWarning` is promoted; M6
confirms no other warning changes behaviour on the intact tree.

### 2. A per-package floor: `py/scripts/assert_test_floor.py`

A committed, ruff-linted, pure-stdlib helper chained into `py:test`, exactly the shape
`py/scripts/assert_typecheck_coverage.py` takes on `py:typecheck` (SMA-436). It is deliberately
**not** a `repo:*` gate — see "Placement" below.

**Input.** `pytest --collect-only -q` on stdin.

**Derived sets.**

* `COLLECTED` — packages whose emitted node ids match `^packages/([^/]+)/tests/`.
* `DISK` — directories under `py/packages` containing a `pyproject.toml`.

**Pinned tables.**

```python
EXPECTED_TEST_PACKAGES = frozenset({"paigasus-kernel", "paigasus-proto"})

NO_TESTS_EXPECTED = {
    "paigasus-ml": "stub package, no public API yet (README: 'Status: stub'; ADR-0011 dormant-until-real)",
    "paigasus-workflows": "stub package, no public API yet (README: 'Status: stub'; ADR-0011 dormant-until-real)",
}
```

**Assertions.**

1. **Registry.** The two tables are disjoint and their union equals `DISK`. A new package must
   be classified deliberately as either test-bearing or reasoned-stub; a package cannot silently
   ship untested forever, and a removed package cannot leave a stale pin behind.
2. **Floor.** `EXPECTED_TEST_PACKAGES == COLLECTED`, as **strict equality**. This is what makes
   the guard bidirectional with one comparison: a moved `tests/` dir drops a package out of
   `COLLECTED` and reds; a package that gains tests without a pin edit also reds.

A disk-derived expectation is deliberately rejected. It is exactly the circularity that makes
`assert_typecheck_coverage.py` blind to this defect: move the directory and both the expected and
the observed value drop together.

**Exit codes**, mirroring the sibling guard:

| code | meaning |
| -- | -- |
| 0 | floor and registry both hold |
| 1 | floor breach — a pinned package contributed no tests, or an unpinned one contributed some |
| 2 | unreadable stdin — no parseable node ids at all |
| 3 | registry mismatch — `DISK` disagrees with the union of the two tables |

**Fail-closed.** Empty or unparseable stdin yields 2, never 0. Because `EXPECTED_TEST_PACKAGES`
is non-empty, a silent change to `--collect-only -q`'s format under the `pytest<10` pin makes
`COLLECTED` empty and reds at assertion 2 rather than passing vacuously. The guard has no path
that reports success on absent input.

### 3. Moon wiring

In `.moon/tasks/python.yml`, `test` becomes a `script:` chaining the real run and the guard,
mirroring `typecheck` directly above it:

```yaml
test:
  script: 'uv run pytest && uv run pytest --collect-only -q | uv run python scripts/assert_test_floor.py'
  inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', '/py/uv.lock', 'scripts/**']
```

The suite runs first and short-circuits on failure, so a red suite is not obscured by a floor
message. The guard is last in the pipe, so its status is the pipeline's. The second invocation
is `--collect-only`, measured at 0.45s cold.

Two input edits ride along, both in the task being rewritten:

* **Drop `'conftest.py'`.** SMA-379 removed `py/conftest.py`; the input declaration outlived the
  file. It is a live instance of this issue's own defect class, sitting in this very task, and
  invisible for the same structural reason (`repo:input-liveness` reaches only `repo:*`).
* **Add `'scripts/**'`.** Without it, editing the guard would not re-run `py:test` — the
  cache-input completeness bug that would let a broken guard serve a cached pass. `typecheck`
  already carries this entry.

### Placement: why not a `repo:*` gate

The issue assumed closing this meant a new `repo:*` gate carrying all seven registration
obligations (`ci.yml`'s `T=(…)` array, the CLAUDE.md marker-delimited command,
`SELF_SCHEDULED_GATES`, `SELF_TASK_EXPECTED_GLOBS`, a script pin, `REQUIRED_REPO_TASKS`, and a
`T_AFFECTED_SMOKE_REQUIRED_INPUTS` entry). That premise is what made deferring the floor look
reasonable.

SMA-436 establishes the cheaper shape: a `py/scripts/` helper chained into the task it guards,
with no registration obligations at all. The guard lives beside the thing it protects, runs
whenever that task runs, and needs nothing in the `repo:*` registries. AC 3 is therefore
satisfied vacuously — no `repo:*` gate is added.

## Residual risks

Stated rather than closed, in each case because closing it costs more than the defect it would
catch.

1. **Nothing pins `py:test`'s script line.** Deleting the guard invocation from
   `.moon/tasks/python.yml` reds nothing. This residual is inherited verbatim from the SMA-436
   guard, which has carried it since 2026-06-20. Closing it means the `repo:*` shape and its
   seven obligations, for a file whose only edits are deliberate. Accepted.
2. **Presence, not counts.** Moving 126 of `paigasus-kernel`'s 127 tests aside leaves one
   collected test and stays green. A per-package count pin would catch it, at the cost of
   re-baselining an assertion on every test deletion — churn on a pin whose value is stability.
   Rejected.
3. **The guard is not type-checked.** `tool.basedpyright.include` is
   `["packages/*/src/**", "packages/*/tests/**"]` and does not reach `py/scripts`. It is
   ruff-linted by `py:lint` (`uv run ruff check .` from `py/`). Same posture as its sibling,
   recorded in SMA-436's plan as "pure stdlib, tooling only — not type-checked, but ruff-linted".
4. **`repo:input-liveness` still cannot reach py.** This spec fixes one dead input instance, not
   the class. A future `py:*` task can still declare a dead input and nothing reds.
5. **No unit tests for the guard.** Verified by a recorded exit-code table driven by synthetic
   stdin, matching SMA-436's verification approach. The table proves the guard worked once, not
   that it keeps working.

## Out of scope

* Any change to `repo:input-liveness`'s project scoping (residual 4).
* A `repo:*` gate of any kind, and therefore any of the seven registration obligations.
* Restoring `py/conftest.py`. A `pytest_collection_finish` hook was considered and rejected:
  measured, the scoped `paigasus-kernel-py:test` invocation (`uv run pytest tests` from
  `py/packages/paigasus-kernel`) resolves rootdir to `py/` — node ids come back as
  `packages/paigasus-kernel/tests/...` — so a py-root conftest is loaded there too and a floor
  hook would red that task. Avoiding it requires a condition that silently no-ops the guard when
  it mis-evaluates, which is the defect class being closed.
* Adding tests to `paigasus-ml` or `paigasus-workflows`. They are reasoned entries in
  `NO_TESTS_EXPECTED`.

## Acceptance criteria

1. Moving or renaming ONE `py/packages/*/tests` directory reds `py:test`, with the failing output
   recorded in the PR.
2. The measurement table above is re-derived on the fixed tree, including the guard's effect on
   M2/M5.
3. The guard's exit codes 0/1/2/3 are each demonstrated against synthetic stdin, with output
   recorded.
4. The intact tree is green: `moon run py:test` passes, and `moon run paigasus-kernel-py:test`
   (the scoped invocation) is unaffected.
5. `py:lint` and `py:fmt` pass over the new script.
6. The reasoning for rejecting the disk-derived expectation, the count-based pin, the conftest
   hook, and the `repo:*` shape is written down in this spec, where a future reader finds it.

## Files touched

| File | Change |
| -- | -- |
| `py/pyproject.toml` | Add `filterwarnings = ["error::pytest.PytestConfigWarning"]` |
| `py/scripts/assert_test_floor.py` | Create — the per-package floor guard |
| `.moon/tasks/python.yml` | `test`: `command` → `script` with the guard; drop dead `conftest.py` input; add `scripts/**` |
| `py/README.md` | Note the floor and its pinned tables under the commands section |
