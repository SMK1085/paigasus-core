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

Four further measurements, taken during the adversarial review of this spec, each of which changed
the design:

| # | question | result |
| -- | -- | -- |
| M7a-c | does a non-node-id line match `^packages/<pkg>/tests/`? | **yes**, three shapes — see §3 |
| M7c | does `pipefail` change the outcome? | `false \| true` exits **0** without, **1** with |
| M8a | `moon run py:test -- -k parity` under `command:` | 124 passed, **10 deselected** |
| M8b | the same under `script:` | **134 passed** — the filter is silently dropped |
| M9 | the same under a `command: 'bash scripts/run_tests.sh'` wrapper | `argc=2`, 10 deselected — passthrough preserved |

M8b is why the wiring is a wrapper rather than the `script:` form the first draft proposed: a
filtered run that quietly executes the full suite and reports success is the same species of
silent lie this issue exists to close.

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

Three artefacts. The floor is the fix; `filterwarnings` is a cheap independent hardening; the
wrapper exists because Moon's `script:` form silently discards passthrough args (M8b).

### 1. Error on `PytestConfigWarning`

In `py/pyproject.toml`'s `[tool.pytest.ini_options]`:

```toml
filterwarnings = ["error::pytest.PytestConfigWarning"]
```

`issue_config_time_warning` applies ini `filterwarnings` (`_pytest/config/__init__.py:1649-1654`),
so total loss becomes a hard error naming `testpaths` (M3 exit 5 -> M4 exit 1) instead of an exit
code that happens to be non-zero for an unrelated reason.

**This promotes seven emission sites, not one.** In pytest 9.1.1 `PytestConfigWarning` is raised
at `config/__init__.py`:563 (deprecated external plugin), :1431 (`testpaths` — the intended one),
:1506 via `_warn_or_fail_if_strict`, :1613 (conftest load failure under `--help`/`--version`),
:2058 (assertion rewriting disabled, e.g. under `PYTHONOPTIMIZE`), :2065 (`_warn_about_skipped_plugins`),
and :2235/:2244 (filter module import failure). Two carry real collateral and are accepted
deliberately:

* **:1506** is reached by `_validate_config_options`, so `Unknown config option: X` becomes a hard
  error — for the whole py workspace *and* for the scoped `paigasus-kernel-py:test`, which reads
  the same inipath. A pytest minor bump that renames or drops an ini key, or a `pyproject.toml`
  typo, reds every py task instead of warning. Accepted: that is a real misconfiguration, and a
  loud failure is the desired response.
* **:2065** turns a self-skipping third-party plugin into a hard failure. Accepted: the py
  workspace pins its plugins through `uv.lock`.

M6 measures today's tree only. **A pytest bump re-opens this measurement**, the same caveat
SMA-603 attaches to the release-plz 0.3.158 measurements.

Mechanism note, since this repo pins mechanism claims: the ini entry does not "append to" pytest's
defaults. `apply_warning_filters` (`config/__init__.py:2229-2237`) calls `warnings.filterwarnings`,
which **inserts at the front**, and `issue_config_time_warning` applies
`simplefilter("always", ...)` before it (`:1651-1653`), so the ini entry takes precedence.

### 2. `py/scripts/run_tests.sh` — the wrapper

```bash
set -euo pipefail
uv run pytest "$@"
if [ "$#" -eq 0 ]; then
  uv run pytest --collect-only -q | uv run python scripts/assert_test_floor.py
fi
```

Three jobs, each forced by a measurement:

* **`set -euo pipefail` (M7c).** Moon does not enable errexit for `script:` blocks and a pipeline's
  status is its last command's — `moon.yml:68-74` documents this trap verbatim, and
  `repo:promtool`, `repo:nats-permissions` and `repo:publish-metadata` each carry the same note.
  Without `pipefail`, `uv run pytest --collect-only`'s exit status is discarded and the guard's
  status alone decides. Measured: `false | true` exits 0 without `pipefail`, 1 with it. That matters
  concretely, because `_validate_config_options` runs in the `pytest_collection` hookwrapper's
  `finally` (`config/__init__.py:1440-1447, 1462-1464`) — **after** `pytest_collection_finish` has
  already printed every node id (`_pytest/terminal.py:905-919`). Under the new `filterwarnings`,
  run 2 can emit a complete node-id list and still exit non-zero. Same shape for an OOM kill after
  the node-id burst.
* **Preserving passthrough (M8).** `command:` forwards `moon run py:test -- -k parity` to pytest;
  `script:` silently drops it and runs the full suite while reporting success. A wrapper invoked as
  `command:` keeps forwarding, measured at `argc=2` with 10 deselected.
* **Skipping the floor under passthrough.** A filtered run legitimately collects from one package,
  so the floor must not apply. `[ "$#" -eq 0 ]` is the condition; CI never passes args. This is the
  design's one deliberate no-op branch — see residual 6.

### 3. `py/scripts/assert_test_floor.py` — the floor

A committed, ruff-linted, pure-stdlib helper, opening with the SPDX header
`# SPDX-License-Identifier: Apache-2.0`. It is deliberately **not** a `repo:*` gate — see
"Placement".

**Input.** `pytest --collect-only -q` on stdin.

**Parsing.** A node id must match `^packages/([^/]+)/tests/[^:]+\.py::` — the `::` is load-bearing.
Measured, three non-node-id shapes match a bare `^packages/([^/]+)/tests/` prefix and none contain
`::`:

| shape | emitted line |
| -- | -- |
| module-level warning, `stacklevel=1` | `packages/paigasus-proto/tests/test_zz_canary.py:2` |
| unregistered-mark warning | `packages/paigasus-proto/tests/test_zz_canary.py:2` |
| collection-error traceback | `packages/paigasus-proto/tests/test_zz_canary.py:1: in <module>` |

`WarningReport.get_location` (`_pytest/terminal.py:367-375`) returns `f"{relpath}:{linenum}"`, and
warnings are reported by default under `--collect-only` because `ExitCode.OK` is in
`summary_exit_codes` (`terminal.py:963-972`). Without the `::` requirement, a package whose tests
were all renamed to `check_*` would still be credited as collected on the strength of any warning
mentioning a file inside it — a false green reached by a rename, which AC 1's move test would miss.

**Integrity cross-check.** The guard also parses pytest's own summary line (`^(\d+) tests? collected`)
and requires it to equal the number of parsed node ids. A truncated stream — the OOM case above —
therefore reds rather than under-counting silently.

**Derived sets.**

* `COLLECTED` — packages contributing at least one parsed node id.
* `DISK` — packages under `py/packages` with a **git-tracked** `pyproject.toml`, via `git ls-files`.
  Tracked rather than on-disk, so an untracked scratch package does not red locally while CI is
  green; `ci/affected-graph/task_inputs.py:276-292` chooses the tracked set for exactly this reason.

**Pinned tables.**

```python
EXPECTED_TEST_PACKAGES = frozenset({"paigasus-kernel", "paigasus-proto"})

NO_TESTS_EXPECTED = {
    "paigasus-ml": "stub package, no public API yet (README 'Status: stub'; ADR-0011 dormant-until-real)",
    "paigasus-workflows": "stub package, no public API yet (README 'Status: stub'; ADR-0011 dormant-until-real)",
}
```

Both key on the **directory name** under `py/packages`, not `[project].name`. They happen to
coincide today.

**Assertions, in this precedence order.**

1. **Input integrity** -> exit 2. Zero bytes, no summary line, or node-id count disagreeing with
   the summary count.
2. **Root sanity** -> exit 4. `DISK` is empty — the guard ran from the wrong cwd, or `py/packages`
   moved. A dedicated code, because surfacing this as a registry mismatch misdiagnoses it
   (`assert_typecheck_coverage.py:59-64` carries the same idea as `EXIT_NO_PACKAGES`).
3. **Registry** -> exit 3. The two tables are disjoint; their union equals `DISK`; every
   `NO_TESTS_EXPECTED` reason is non-blank after `.strip()`; and the **disk cross-check**: every
   `EXPECTED_TEST_PACKAGES` member has a tracked `tests/` directory and every `NO_TESTS_EXPECTED`
   member has none.
4. **Floor** -> exit 1. `EXPECTED_TEST_PACKAGES == COLLECTED`, strict equality, so a lost `tests/`
   dir and an unpinned package that gained tests both red.

The disk cross-check in assertion 3 exists to close a one-line silent disable: moving
`paigasus-kernel` from `EXPECTED_TEST_PACKAGES` into `NO_TESTS_EXPECTED` and deleting its tests
would otherwise satisfy both the union and the floor, and the guard would report green having
asserted nothing about the package it was built for. With the cross-check, that reclassification
also requires deleting a tracked directory — a reviewable event. The non-blank reason requirement
mirrors `ALLOW_DEAD_INPUT` (`task_inputs.py:374`, `:406`).

**Fail-closed.** No path reports success on absent input: zero bytes yields 2, and because
`EXPECTED_TEST_PACKAGES` is non-empty, a format change under the `pytest<10` pin that empties
`COLLECTED` reds at assertion 4.

**`--self-test`.** The guard exposes pure functions — `parse_collected(text)` and
`check(collected, disk, expected, no_tests, tests_dirs)` — and a `--self-test` flag driving them
with fixtures, as `ci/affected-graph/task_inputs.py:434-658` does. This is what makes all five exit
codes demonstrable: codes 2/3/4 cannot be produced from stdin alone, since `DISK` is read from git.

### 4. Moon wiring

In `.moon/tasks/python.yml`:

```yaml
lint:
  command: 'uv run ruff check .'
  inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', '/py/uv.lock', 'scripts/**']
fmt:
  command: 'uv run ruff format --check .'
  inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', '/py/uv.lock', 'scripts/**']
test:
  command: 'bash scripts/run_tests.sh'
  inputs: ['@group(sources)', '@group(tests)', 'pyproject.toml', '/py/uv.lock', 'scripts/**', 'packages/*/pyproject.toml']
```

`test` stays `command:` so passthrough survives (M8). Four input edits:

* **Drop `'conftest.py'` from `test`.** SMA-379 removed `py/conftest.py`; the declaration outlived
  the file. A live instance of this issue's own defect class, in this very task, invisible for the
  same structural reason.
* **Add `'scripts/**'` to `test`.** Without it, editing the guard would not re-run `py:test`.
* **Add `'scripts/**'` to `lint` and `fmt`.** Residual 3 claims the guard is ruff-linted; measured,
  neither task keys on `py/scripts`, so a PR touching only the guard selects neither. Without this,
  AC 5 is satisfiable once by hand and never again.
* **Add `'packages/*/pyproject.toml'` to `test`.** The registry assertion reads it, so a PR adding
  a scaffold package with only a `pyproject.toml` must select `py:test` — otherwise the assertion
  never runs on the PR that breaks it and reds an unrelated PR later. `repo:affected-smoke` already
  carries this path (`moon.yml:185`) for the same reason.

### Placement: why not a `repo:*` gate, and why no pin

The issue assumed closing this meant a `repo:*` gate with all seven registration obligations, and
that premise is what made deferring the floor look reasonable. The SMA-436 shape — a `py/scripts/`
helper chained into the task it guards — carries none of them, which is what makes the floor cheap
enough to build now.

A third shape exists and is **deliberately declined**: `check_contracts_generate_inputs`
(`ci/affected-graph/ci_targets.py:1609-1621`, constant at `:339-346`) pins a **non-`repo:*`** task's
configuration from inside `repo:affected-smoke`, and a sibling could pin `py:test`'s `command:` line
and `run_tests.sh`'s load-bearing lines the same way. It is declined here because it is not free:
`repo:affected-smoke` would have to list `py/scripts/**/*` among its own `inputs`, floored by a
`T_AFFECTED_SMOKE_REQUIRED_INPUTS` entry in `ci/actionlint/run.sh` — the reachability pair CLAUDE.md
names for `repo:ruff-ci` and `repo:workflow-credentials` — or the pin stays green on exactly the PR
that breaks it. Two registrations, three extra files, and a second gate's inputs widened, to guard a
file whose only edits are deliberate. See residual 1, which states the cost of declining.

AC 3 of the issue is therefore satisfied vacuously — no `repo:*` gate is added.

## Residual risks

1. **Nothing pins the invocation.** Deleting the floor call from `py/scripts/run_tests.sh`, or
   reverting `py:test` to a bare `uv run pytest`, reds nothing. This residual is inherited from the
   SMA-436 guard, which has carried it since 2026-06-20 — but it is **worse here**, and that should
   be recorded plainly: deleting SMA-436's guard invocation degrades a second-order vacuity check,
   whereas deleting this one restores exactly the SMA-610 defect this spec exists to close. The
   cheap `ci_targets.py` pin described under "Placement" would close it; it is declined on cost, not
   because the risk is small. Revisit if `run_tests.sh` ever acquires a second editor or an `args:`
   entry.
2. **Presence, not counts.** Moving 126 of `paigasus-kernel`'s 127 tests aside leaves one collected
   test and stays green. A count pin would catch it, at the cost of re-baselining on every test
   deletion. Rejected. A per-file variant — every tracked `packages/*/tests/**/test_*.py`
   contributes at least one node id — was considered as a middle ground and is **deferred, not
   rejected**: it is disk-derived, so it is circular for the deleted-file case in the same way the
   count pin is, and it earns its keep only once a package has enough test files for partial
   in-package loss to be plausible.
3. **The guard is not type-checked.** `tool.basedpyright.include` does not reach `py/scripts`. It is
   ruff-linted by `py:lint`, which this spec makes continuously true by adding `scripts/**` to its
   inputs.
4. **`repo:input-liveness` still cannot reach py.** This fixes one dead-input instance, not the
   class. A future `py:*` task can still declare a dead input and nothing reds.
5. **No unit tests for the guard beyond `--self-test`.** The self-test proves the pure functions
   behave; it does not prove the wrapper wires them correctly.
6. **`filterwarnings` is not itself guarded.** Once the floor exists, deleting the ini line would
   still leave total loss red (via the floor), so nothing would notice the loss of the earlier,
   clearer config-time error. Accepted as cosmetic.
7. **The `[ "$#" -eq 0 ]` branch is a deliberate no-op path.** A task definition that grew a
   permanent `args:` entry would silently switch the floor off. Nothing pins that, per residual 1;
   a reviewer is what catches it. The condition is one line and carries a comment saying so.

## Out of scope

* Any change to `repo:input-liveness`'s project scoping (residual 3).
* A `repo:*` gate of any kind.
* A `ci_targets.py` pin over `py:test`'s invocation, and the widening of
  `repo:affected-smoke`'s inputs it would require (see "Placement" and residual 1).
* Restoring `py/conftest.py`. A `pytest_collection_finish` hook was considered and rejected:
  measured, the scoped `paigasus-kernel-py:test` invocation resolves rootdir to `py/` — node ids
  come back as `packages/paigasus-kernel/tests/...` — so a py-root conftest is loaded there too and
  a floor hook would red that task. Avoiding it needs a condition that silently no-ops the guard
  when it mis-evaluates.
* ~~Fixing the identical missing `pipefail` in `py:typecheck` (`.moon/tasks/python.yml:38`).~~
  **Amended after review:** folded into this PR at the reviewer's direction. Measured while doing
  so — a producer emitting valid, sufficient JSON while exiting 3 gives pipeline exit 0 without
  `pipefail` and 3 with it — but **no live false green is claimed**: `assert_typecheck_coverage.py`
  parses JSON, so a truncated or empty stream already fails `json.loads` and returns
  `EXIT_UNREADABLE`. The line removes the dependence on that happy accident rather than closing a
  demonstrated hole.
* Adding tests to `paigasus-ml` or `paigasus-workflows`.

## Acceptance criteria

1. Moving `py/packages/paigasus-kernel/tests` aside reds `moon run py:test` on its **exit status**,
   with the output recorded. Repeated for `py/packages/paigasus-proto/tests` — the two have
   different shapes (7 surviving tests vs 127).
2. The measurement table is re-derived on the fixed tree, including the floor's effect on M2/M5.
3. All five guard exit codes (0/1/2/3/4) are demonstrated — 1 and the success path from real stdin,
   2/3/4 via `--self-test` — with output recorded.
4. The intact tree is green: `moon run py:test` passes, and `moon run paigasus-kernel-py:test`
   passes with its node-id shape unchanged.
5. `moon run py:test -- -k parity` still deselects, proving passthrough survived.
6. A warm-cache `moon run py:test` after touching only `py/scripts/assert_test_floor.py` re-runs
   the task, proving the `scripts/**` input works.
7. `py:lint` and `py:fmt` pass over both new files and select on a `py/scripts` edit.

## Files touched

| File | Change |
| -- | -- |
| `py/pyproject.toml` | Add `filterwarnings = ["error::pytest.PytestConfigWarning"]` |
| `py/scripts/run_tests.sh` | Create — `pipefail` wrapper, passthrough, conditional floor |
| `py/scripts/assert_test_floor.py` | Create — the floor guard, with `--self-test` |
| `.moon/tasks/python.yml` | `test` -> wrapper; input edits on `test`, `lint`, `fmt` |
| `py/README.md` | Document the floor and its tables; fix `moon run py:format` -> `fmt` |
