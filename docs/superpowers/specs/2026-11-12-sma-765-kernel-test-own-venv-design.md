# SMA-765 — the kernel test uses its own virtual environment

- Linear: SMA-765
- Date: 2026-11-12
- Status: revision 2, after the builder check and the spec challenge. For approval at GATE 1.

## 1. Intent

`moon ci` can fail at random in `py:test` with `ModuleNotFoundError` during test collection. The
failure has no relation to the change under test. It reached Dependabot PR #455 on 2026-11-12
(CI run 43295812102), and a re-run passed with no code change.

The cause is one shared virtual environment, `py/.venv`, with one writer that removes packages:

- `paigasus-kernel-py:test` (`py/packages/paigasus-kernel/moon.yml`, `test` task) runs
  `uv sync --reinstall-package paigasus-py-bindings` from the member directory. A plain
  `uv sync` is EXACT. It removes every package that this one member does not need. In run
  43295812102 it logged "Uninstalled 18 packages", `grpclib` and `anthropic` included.
- Readers of `py/.venv` that can run at the same time: `py:test`, `py:lint`, `py:fmt`,
  `py:typecheck` (`.moon/tasks/python.yml`), `paigasus-gateway-conformance:test`
  (`uv run pytest tests`), `contracts:generate` (`uv run --project ../py`), `repo:actionlint`
  and `repo:ruff-ci` (`uv run --locked --project py`), and `repo:release-parity-py`
  (`uv run --frozen`). `uv run` syncs INEXACTLY by default, so these tasks only add packages.

When Moon runs the kernel test at the same time as a reader, the reader loses packages in the
middle of its run. `--reinstall-package` adds a second, smaller race: it removes and writes
`paigasus-py-bindings` again, and `py:test` also collects `packages/paigasus-kernel/tests`,
which import that binding.

## 2. Decision

Sven chose option D on 2026-11-12: **the kernel test uses its own virtual environment.**

Revision 2 makes the mechanism concrete (my call after the spec challenge, see §10): the task
runs `uv run --isolated`. uv then makes a temporary environment in its cache directory, uses
it for this one run, and deletes it. The task no longer runs `uv sync`. The kernel test does
not read or write `py/.venv` at all, and no environment directory appears in the repo tree.

## 3. Goals

- G1. The kernel test task does not remove, add or reinstall any package in `py/.venv`.
- G2. `paigasus-kernel-py:test` keeps its current guarantee: it tests a binding that is built
  from the current Rust source (`touch` + `--reinstall-package`, SMA-420 / SMA-430).
- G3. A gate fails if any Moon task mutates a uv project environment in place again.

## 4. Non-goals and residuals

- N1. No change to task ordering, task `deps`, the affected graph, or `.github/workflows/ci.yml`.
- N2. No change to how `py/uv.lock` is written. Today `uv sync` may relock; `uv run` has the same
  default. No `--locked` or `--frozen` is added (`ci/version-lockstep/README.md:25` documents the
  current behavior).
- R-stale. **Residual, a behavior change on a developer machine.** Today the kernel task also
  refreshes the binding in `py/.venv` as a side effect. After this change nothing does: a bare
  `uv run` serves the cached wheel and does not see Rust changes (measured,
  `ci/affected-graph/run.sh:402-406`). So `py:test` and `py:typecheck` can run the kernel tests
  and the stub against an old binding after a Rust edit. Remedy, to run when no Moon run is
  active: `(cd py && uv sync --all-packages --reinstall-package paigasus-py-bindings)`. The PR
  adds this line to `py/README.md` or the nearest Python CLAUDE.md. Rejected fix:
  `[tool.uv] cache-keys` on the Rust sources. With the kernel task's `touch`, every reader's
  `uv run` would then reinstall the binding in `py/.venv`, and the race comes back.
- R-toolchain. Moon's own `InstallDependencies(unstable_python, py)` action writes `py/.venv`.
  Every Python task depends on it, so it runs before them. Readers with the `system` toolchain
  (`repo:actionlint`, `repo:ruff-ci`) do not depend on it. Whether that is a race is not
  measured, and it is out of scope. A14 cannot see toolchain actions.
- R-relock. An inexact `uv run` still changes a package when the lock and the environment
  disagree. That is not a race between two tasks of one run, and it is out of scope.

Anti-goal: a fix that only makes the race less frequent. `--inexact` alone is such a fix (§9).

## 5. Measured facts

- F1. uv is 0.11.16 (`.prototools`).
- F2. `uv run --isolated --reinstall-package paigasus-py-bindings …`, run from
  `py/packages/paigasus-kernel`, uses a temporary environment under the uv cache
  (`~/.cache/uv/builds-v0/.tmp…`). It builds the binding with maturin 1.16.0 (`-v` log shows
  `Building paigasus-py-bindings @ file://…`). Measured on this branch: after a full
  `uv sync --all-packages --locked`, the package list of `py/.venv` was the same before and
  after the run, and `find py/.venv -newer <marker>` found 0 files. Wall time without a Rust
  recompile: 2.5 s.
- F3. The isolated environment contains pytest (9.2.0): the root `dev` dependency group is
  installed. So `uv run --isolated … pytest tests` works without a separate sync.
- F4. Moon 2.5.3 reports a task's `env` in both `moon query tasks` and `moon query projects`
  (measured with a temporary `env:` block on the kernel task). A missing `env:` block gives no
  `env` key.
- F5. The free check number in `repo:affected-smoke` is A14 (A10–A13 exist).
- F6. **Local limit on this Mac.** The binding built on this host does not load:
  `dlopen … mis-aligned LINKEDIT string pool, fileOffset=0x00062514`. This happens in
  `py/.venv` and in the isolated environment, so it is not caused by this design. CI (Linux)
  runs `paigasus-kernel-py:test` green today. So the local proof (§7) measures `py/.venv`
  only, and CI proves that the kernel tests pass.

## 6. Design

### 6.1 The kernel test task

In `py/packages/paigasus-kernel/moon.yml`, task `test`, change the script from

```
touch <kernel lib.rs> <binding lib.rs> && uv sync --reinstall-package paigasus-py-bindings && uv run pytest tests
```

to

```
touch <kernel lib.rs> <binding lib.rs> && uv run --isolated --reinstall-package paigasus-py-bindings pytest tests
```

- Keep the `touch` unchanged (SMA-430).
- Keep the inputs unchanged.
- No `env:` block and no environment directory, so no `.gitignore` line and no
  `hasher.ignorePatterns` entry are needed. (Revision 1 needed both; see §10, BLOCKER 1.)
- Update the comment above the task: why the run is isolated, SMA-765, the measured failure,
  and the stale-binding residual (R-stale).
- `build` does not change.

### 6.2 Gate A14 in `repo:affected-smoke`

A new check, A14 (`check_uv_env_isolation` in `ci/affected-graph/cargo_moon_parity.py`,
findings key `a14`). It reds when a Moon task can change a uv project environment in place.

**Data.** Extend `moon_projects()` to keep each task's `env` (F4). A missing key means "no env".
Existing fixtures without `env` stay valid. The key must survive the json round-trip deep copy
that the self-test uses.

**Text scanned.** The same resolved command/script blob that A5 uses, plus the scripts that the
task calls. Follow the same script references that A10 follows (`ci/**/*.sh`), and also
project-relative `scripts/*.sh` references (`py:test` runs `bash scripts/run_tests.sh`). Add
`py/scripts/**` to `repo:affected-smoke`'s inputs, and to `T_AFFECTED_SMOKE_REQUIRED_INPUTS` if
that registry lists inputs.

**Strings are not stripped (fail-closed),** in the same way as A8 (`cargo_moon_parity.py:835`).
A line that only prints `uv sync` inside a message counts. The two known message lines
(`ci/ruff/run.sh:92`, `ci/actionlint/run.sh:5982`) go into an allowlist keyed by
(script path, exact line text), each with a reason, in the same form as
`ALLOW_UNLOCKED_CARGO_SCRIPT`. An allowlist entry that no longer matches a line is itself a
finding, so the list cannot rot.

**In scope (a finding).** Allow uv global options between `uv` and the verb
(`uv -q sync`, `uv --project py sync`, `uv --directory x sync`), and any whitespace:
space, tab, or a `\` line continuation.

- `uv sync`, `uv pip sync`, `uv pip install`, `uv pip uninstall`, `uv venv`, `uv add`,
  `uv remove`.
- `uv run` with `--exact`, `--reinstall`, `--reinstall-package` or `--refresh-package`, unless
  the same invocation has `--isolated`.
- Any executing text that sets or clears `UV_PROJECT_ENVIRONMENT` or `VIRTUAL_ENV`, or passes
  `--active`. (A task `env:` that sets `UV_PROJECT_ENVIRONMENT` also counts. Revision 2 has no
  legitimate use for it, so the blunt rule is the safe rule.)

**Floor.** `REQUIRED_UV_ISOLATED_TASKS = ("paigasus-kernel-py:test",)`, in the same form as
`REQUIRED_FFI_TASKS`. Each named task must exist and must run `uv run --isolated` with
`--reinstall-package paigasus-py-bindings`. If the kernel task drops either flag, A14 reds.

**Self-test rows** (vary the spacing, so the fixtures do not inherit the implementer's
assumption):

- red: `uv sync`; `uv  sync` (two spaces); `uv\tsync`; `uv -q sync`; `uv --project py sync`;
  `uv sync --inexact`; `uv pip install x`; `uv venv`.
- red: `uv run --reinstall-package p pytest` (no `--isolated`); `uv run --exact pytest`.
- red: `UV_PROJECT_ENVIRONMENT=.venv uv run pytest`; `export VIRTUAL_ENV=x`; `uv run --active x`.
- red: `echo "run uv sync"` (strings are not stripped) when it is not in the allowlist.
- red: a `uv sync` in a followed script file.
- red: an allowlist entry that matches no line.
- red: the floor task without `--isolated`; the floor task missing.
- green: `uv run --isolated --reinstall-package p pytest`; `uv run pytest`; `uv run --locked x`.
- green: an allowlisted message line.

**Real-tree negative control.** In `main()`, after the real findings, apply A14 to a deep copy of
the real `projects` with `--isolated` removed from `paigasus-kernel-py:test`. If A14 does not go
red, exit 2. (`run.sh --negative-control` runs only the synthetic self-test, `run.sh:872`, so this
is a new, explicit step.)

**Registries.** Add `a14` to `EXPECTED_FINDING_KEYS` and to the PASS text in `main()`. Add an
**A14** entry to `ci/affected-graph/README.md` after A13, with the scan scope, the string policy,
the allowlist and the named limit: A14 does not see Moon toolchain actions (R-toolchain).

## 7. Proof (red-first)

The local proof measures `py/.venv` itself. It does not need a loading binding (F6).

Before each measurement: `(cd py && uv sync --all-packages --locked)`, then assert that the
package list contains `grpclib` and `anthropic`.

1. Marker test. `touch "$m"; moon run paigasus-kernel-py:test --force; find py/.venv -newer "$m"`.
   Before the fix: the output is not empty. After the fix: it is empty.
2. Reader test. After the kernel task: `(cd py && uv run --no-sync python -c 'import grpclib,
   anthropic')`. Before the fix: it fails. After the fix: it passes.
3. A14: `python3 ci/affected-graph/cargo_moon_parity.py --self-test` passes. On the tree BEFORE
   the moon.yml change, A14 reds on the kernel task. After it, A14 is green. Run
   `/bin/bash ci/affected-graph/run.sh` directly (system bash 3.2; Moon can pick Homebrew bash
   5.3.15, which deadlocks on this Mac).
4. CI: `paigasus-kernel-py:test` and `py:test` both pass. CI is the authority for the kernel
   tests themselves (F6).

The kernel task's pytest step fails locally in both states because of F6. Steps 1 and 2 still
work, because `uv sync` / `uv run --isolated` run before pytest. Record the numbers in the PR
body.

## 8. Risks and rollback

- Cost: an isolated environment per kernel test run. Measured 2.5 s without a Rust recompile
  (F2). The maturin build costs the same as today.
- The isolated environment is in the uv cache, which CI restores and saves. uv deletes the
  temporary environment after the run.
- Rollback: revert the commit. No directory to delete.

## 9. Options that were considered

| Option | Result | Why not |
|---|---|---|
| A. `uv sync --inexact` in the shared venv | Removes the package loss | The binding reinstall still races `py:test`. The race becomes rarer, not gone (anti-goal). |
| B. A Moon `deps` edge that orders the two tasks | The pair never overlaps | It protects only the named pair. Other readers stay exposed. CI gets slower. |
| B2. A Moon `options.mutex` | Serializes the tasks that opt in | Every reader must opt in (same weakness as B), and it serializes the readers too. |
| C. One step that builds the binding for all Python tasks | Every task sees a fresh binding | Every Python task then waits for a maturin build. Task inputs and the affected graph change. |
| D1. Own venv via `UV_PROJECT_ENVIRONMENT=.venv-kernel-test` (revision 1) | Isolation | A directory in the tree: Moon hashes it (`hasher.ignorePatterns` does not match it), ruff can lint it, and it needs a `.gitignore` line. |
| **D2. `uv run --isolated` (revision 2)** | Isolation, nothing in the tree | Chosen. |

## 10. Changelog of revision 2

From the spec challenge (verdict: APPROVE WITH CHANGES) and the builder check.

Folded in:

- BLOCKER 1 (Moon hashes the new venv; `hasher.ignorePatterns` has only `**/.venv/**`):
  removed by D2. There is no directory in the tree. This also removes the ruff and `.gitignore`
  findings.
- BLOCKER 2 (A14 match rule contradicted the helpers): §6.2 now states the scan scope, a
  fail-closed string policy and an allowlist.
- MAJOR stale binding: added as R-stale with the remedy and the rejected `cache-keys` fix.
- MAJOR "dedicated" not enforced: no longer needed. `--isolated` makes one fresh environment
  per run, so two tasks cannot share one. A14 reds any `UV_PROJECT_ENVIRONMENT` use.
- MAJOR other mutating verbs, global options, overrides, script indirection: in §6.2.
- MAJOR vacuous proof: §7 now restores `py/.venv` first, uses a marker file and a deterministic
  reader.
- MINOR: `env` in `moon_projects()` (measured, F4); named floor; explicit real-tree negative
  control; `EXPECTED_FINDING_KEYS`; mutex in §9; complete reader list in §1; G1 scoped to the
  kernel task; residuals R-toolchain and R-relock; local bash note; rollback.
- Builder questions 1-7: answered by F4, §6.2, §7 and F6.

Not folded in:

- `--locked` on the kernel sync: out of scope (N2). It changes the lockfile behavior that
  `repo:version-lockstep` documents.
- A measurement of whether Moon fails on a file that disappears during the hash walk: no longer
  needed with D2.
