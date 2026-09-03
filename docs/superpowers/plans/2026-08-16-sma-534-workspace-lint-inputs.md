# SMA-534 Workspace-level `lint` Inputs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a `rs/Cargo.lock`, `rs/Cargo.toml` or `rs/rust-toolchain.toml` change schedule every
Rust crate's `lint`, so a dependency bump's clippy break cannot ship green — and guard the new
invariant behaviourally and generically so it cannot silently reopen.

**Architecture:** Three workspace-relative paths are added to the *inherited* `lint` task in
`.moon/tasks/rust.yml` (one declaration site, so a new crate cannot forget it), and the command
gains `--locked` so the lint runs against the resolution the PR actually ships. Two guard layers
follow the repo's existing split: a hand-written strict-equality behavioural case in
`ci/affected-graph/run.sh` proves the inputs take *effect*; a new generic assertion **A4** in
`ci/affected-graph/cargo_moon_parity.py` proves they are *declared* for every crate, reading Moon's
own resolved `inputFiles` rather than parsing YAML. A third change makes CI actually execute the
negative control that proves those gates can bite.

**Tech Stack:** Moon 2.3.2 (`moon query projects|tasks --affected --downstream deep`), bash + Python
3 (stdlib only: `json`, `subprocess`, `tomllib`, `tempfile`, `pathlib`), GitHub Actions,
`cargo clippy`.

## Global Constraints

- **Spec:** `docs/superpowers/specs/2026-08-16-sma-534-cargo-lock-lint-inputs-design.md`. Read it
  before starting; every design decision below is justified there.
- **Moon is 2.3.2.** Expected affected sets are a snapshot of this exact version.
- **PATH:** the Bash tool's PATH lacks the proto-managed CLIs. Prefix every command with
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` (shims **first** — repo-pinned version).
- **Working directory:** `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-534`.
  This is a git worktree. Do **not** `cd` to the main checkout. Branch:
  `feature/sma-534-rust-cargo-lock-lints-nothing`.
- **`cargo_moon_parity.py` invariants (do not break):** never parse a `moon.yml` or
  `.moon/tasks/*.yml` — read Moon's resolved output. Never shell out to `cargo` — `repo:affected-smoke`
  is `toolchain: 'system'`. Python **stdlib only**.
- **`self_test()` deep-copies fixtures with `json.loads(json.dumps(ok))`.** Anything you add to a
  fixture must be JSON-serialisable — use **lists**, never `set`s.
- **SPDX header** on every source file: `// SPDX-License-Identifier: Apache-2.0` (`#` for
  Python/bash). All files touched here already have one; do not remove it.
- **Conventional commits with a workspace scope**, subject **lowercase**, header **≤100 chars**.
  Never write `#NNN` in a commit body (commitlint `footer-leading-blank`) — write "owner/repo PR NNN".
- **Do not use `--no-verify`.** The worktree is provisioned; the `commit-msg` hook works.
- **The three required workspace paths, verbatim** (workspace-relative, as Moon resolves them):
  `rs/Cargo.lock`, `rs/Cargo.toml`, `rs/rust-toolchain.toml`. In YAML `inputs:` they are written
  with a leading slash: `/rs/Cargo.lock`, `/rs/Cargo.toml`, `/rs/rust-toolchain.toml`.
- **The thirteen Rust crate projects** (Moon ids), used verbatim in the expected set:
  `paigasus-gateway-rs`, `paigasus-iam-core-rs`, `paigasus-iam-rs`, `paigasus-kernel-parity-rs`,
  `paigasus-kernel-rs`, `paigasus-logging-rs`, `paigasus-node-bindings-rs`,
  `paigasus-observability-rs`, `paigasus-proto-derive-rs`, `paigasus-proto-rs`,
  `paigasus-py-bindings-rs`, `paigasus-service-info-rs`, `paigasus-wasm-rs`.

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `.moon/tasks/rust.yml` | The fix: three workspace inputs + `--locked` on the inherited `lint` task | 1 |
| `ci/affected-graph/run.sh` | Behavioural guard: the `lockfile->all-lint` strict-equality case | 1 |
| `ci/affected-graph/cargo_moon_parity.py` | Generic guard: A4 (declared inputs, every crate) + its self-tests | 2 |
| `moon.yml` | `repo:affected-smoke` runs the negative control before the real suite | 3 |
| `.github/workflows/ci.yml` | `rs/Cargo.toml` joins the `rs/target` primary cache key | 4 |
| `ci/affected-graph/README.md` | Documents the new case, A4, the negative-control change | 5 |
| `CLAUDE.md` | Gotcha: a new crate now changes two expected sets, not one | 5 |

Tasks 1 and 2 each land a working guard plus the thing it guards. Tasks 3–5 are independent and
individually rejectable. Task 6 is verification only and writes no product code.

---

### Task 1: The fix, with its behavioural guard

**Files:**
- Modify: `.moon/tasks/rust.yml:25-35` (the `lint` task)
- Modify: `ci/affected-graph/run.sh` (add a case inside `run_suite()`, after `proto->service-info-tasks`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: the invariant *"a `rs/Cargo.lock` touch schedules exactly the thirteen `*-rs:lint`
  targets and no `build`/`test`"*. Task 2's A4 asserts the declaration side of the same change;
  Task 6 verifies it end-to-end through `moon ci`.

- [ ] **Step 1: Write the failing test — add the behavioural case to `run.sh`**

In `ci/affected-graph/run.sh`, inside `run_suite()`, immediately **after** the existing
`run_task_case "proto->service-info-tasks" …` block and **before** the
`assert_cargo_moon_parity || SUITE_RC=1` line, insert:

```bash
  # A workspace-level change must schedule EVERY crate's lint. `rs/` has no Moon project, so these
  # files belong to `repo`; affectedness reaches the crates through `lint`'s task INPUTS, not
  # through `dependsOn` — which is why no project case above changes. Before SMA-534 a
  # Cargo.lock-only touch (i.e. every Dependabot Cargo PR) scheduled no crate task at all, so a
  # dependency bump that tripped `-D warnings` merged green and redded main later.
  #
  # No `build`/`test` row is expected: the workspace files are inputs to `lint` ONLY (SMA-534
  # weighed and rejected build/test on cost — see the spec).
  #
  # SAFETY OF THE NAME FILTER: `assert_task_case` matches the task NAMES build/test/lint across
  # every project. Two things make that safe here, and only these two — state them narrowly:
  #   1. `repo` declares no task named build/test/lint (verify: `moon query tasks`).
  #   2. No py/ts task lists `rs/Cargo.lock` TODAY. It is NOT true that py/ts are unreachable from
  #      `rs/` paths: ts/packages/paigasus-kernel:build, ts/packages/paigasus-kernel:test and
  #      py/packages/paigasus-kernel:test all declare `/rs/crates/**` inputs. Adding the lockfile
  #      to any of those three — a legitimate cache-input-completeness fix — puts a `build`/`test`
  #      row into THIS case's observed set. Add it here when that happens; do not widen the filter.
  run_task_case "lockfile->all-lint" "rs/Cargo.lock" \
    "paigasus-gateway-rs:lint,paigasus-iam-core-rs:lint,paigasus-iam-rs:lint,paigasus-kernel-parity-rs:lint,paigasus-kernel-rs:lint,paigasus-logging-rs:lint,paigasus-node-bindings-rs:lint,paigasus-observability-rs:lint,paigasus-proto-derive-rs:lint,paigasus-proto-rs:lint,paigasus-py-bindings-rs:lint,paigasus-service-info-rs:lint,paigasus-wasm-rs:lint"
```

Keep the expected CSV on **one line**. `assert_task_case` splits on commas with `tr` and sorts, so
order does not matter, but an embedded newline would become part of a target name and fail.

- [ ] **Step 2: Run it to make sure it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/affected-graph/run.sh
```

Expected: **FAIL**, exit 1, with a `FAIL [lockfile->all-lint] affected TASK set != expected set`
block listing all thirteen `*-rs:lint` targets under `missing` (the observed set is empty because
the lockfile is in no crate task's inputs yet). Every other case must still say `PASS` — if any
other case moved, stop and investigate before continuing.

- [ ] **Step 3: Implement — add the three inputs and `--locked`**

In `.moon/tasks/rust.yml`, replace the `lint` task's `command` and `inputs` lines. **Keep SMA-526's
seven-line rationale comment above `deps` exactly as it is** — only the `command:` line and the
`inputs:` line change, and a new comment block is added above `inputs:`:

```yaml
  lint:
    # `--locked` so clippy lints the resolution the PR actually SHIPS. Without it cargo silently
    # re-resolves and rewrites an inconsistent Cargo.lock, and the thirteen lints below would
    # compile against newest-compatible versions instead — which would make the whole point of the
    # workspace inputs unprovable. No other gate in the repo passes `--locked`, and a Dependabot
    # Cargo PR has already shipped a lockfile resolved from 3 of 11 workspace members here (SMA-534).
    command: 'cargo clippy --locked --all-targets -- -D warnings'
    # `^:build` is what makes a task AFFECTED when an upstream changes. Without it clippy
    # propagated across no edge at all, so an upstream change that tripped `-D warnings` in a
    # CONSUMER shipped green and redded main later (SMA-526). Declared here rather than per-crate
    # so a new crate has no per-crate `lint` declaration to forget. A `workspace = true` in-tree
    # dep still needs its own hand-written `dependsOn` (SMA-524) — the parity gate's A1 catches
    # that separately.
    # `repo:affected-smoke` asserts this expansion for every crate (cargo_moon_parity.py A3).
    deps: ['^:build']
    # The three WORKSPACE-level inputs (SMA-534). `rs/` has no Moon project, so without these a
    # Cargo.lock-only change — every Dependabot Cargo PR — scheduled no crate task at all.
    # Leading `/` = workspace-relative. Declared here, not per-crate, for the same reason as `deps`.
    #   /rs/Cargo.lock          the resolved dependency versions. The issue's motivating case.
    #   /rs/Cargo.toml          [workspace.dependencies] AND [workspace.lints.{rust,clippy}] — the
    #                           clippy posture itself — AND feature flips, which never reach the lock.
    #   /rs/rust-toolchain.toml which clippy-driver runs. Defence-in-depth: a CORRECT toolchain bump
    #                           also touches .moon/toolchains.yml, which is an implicitInput
    #                           (.moon/tasks.yml:17) and already schedules all thirteen lints. This
    #                           input catches the bump that drifts the two files apart, which
    #                           rust-toolchain.toml's own lockstep comment warns about.
    # Deliberately NOT on build/test/fmt: `clippy --all-targets` is a superset of
    # `check --all-targets`, so it already type-checks libs, bins and tests; adding `test` would put
    # the Docker-gated IAM container suites on every Dependabot PR. See the spec for the residual
    # risk this leaves open (clippy neither links the cdylibs nor compiles wasm32).
    inputs:
      - '@group(sources)'
      - '@group(tests)'
      - 'Cargo.toml'
      - '/rs/Cargo.toml'
      - '/rs/Cargo.lock'
      - '/rs/rust-toolchain.toml'
```

- [ ] **Step 4: Run it to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/affected-graph/run.sh
```

Expected: `PASS  lockfile->all-lint  -> paigasus-gateway-rs:lint …` (thirteen targets) and
`== affected-graph cascade intact ==`, exit 0. Every pre-existing case still `PASS`, with its
expected set **unmodified** — if you had to edit an existing expected set, stop: the spec asserts
none should move, so something else is wrong.

- [ ] **Step 5: Confirm `--locked` and the lint itself are actually green**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run :lint
```

Expected: all thirteen `*-rs:lint` tasks pass. This takes ~2 minutes from a cold `rs/target`. If
cargo reports *"the lock file … needs to be updated but --locked was passed"*, do **not** run
`cargo update` — that means the committed lockfile is out of sync with the manifests, which is a
real finding: stop and report it.

- [ ] **Step 6: Commit**

```bash
git add .moon/tasks/rust.yml ci/affected-graph/run.sh
git commit -m "fix(rs): lint on the workspace lockfile, manifest and toolchain (SMA-534)"
```

Use this commit body (note: no `#NNN` anywhere):

```
A rs/Cargo.lock-only change — the exact shape of every Dependabot Cargo PR — scheduled
repo:deny, repo:nats-permissions, repo:publish-metadata and repo:wasm-getrandom-free and
nothing else: no crate build, test or lint, not even for a crate that directly consumes
the bumped dependency. A bump that deprecates an API therefore merged green and redded
main later. SMA-526 closed the in-tree half of this shape and left this half open.

Adds /rs/Cargo.lock, /rs/Cargo.toml and /rs/rust-toolchain.toml to the inherited lint
task's inputs — one declaration site, so a new crate has nothing to forget. Measured: a
lockfile touch goes from zero crate tasks to all thirteen lints. build/test/fmt are
deliberately untouched; clippy --all-targets already type-checks libs, bins and tests,
while adding test would put the Docker-gated IAM suites on every dependency bump.

The command also gains --locked: without it cargo silently re-resolves an inconsistent
lockfile, so the lints would cover a resolution the PR does not ship.

Guards the new invariant with a strict-equality lockfile->all-lint case whose comment
names the three py/ts tasks that are one input line from entering its observed set.
```

---

### Task 2: A4 — the generic declared-inputs assertion

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` (module docstring, `moon_projects()`,
  a new `check_lint_inputs()`, `self_test()`, `main()`)

**Interfaces:**
- Consumes: the three input paths added in Task 1 (A4 fails without them — that is its point).
- Produces: `check_lint_inputs(projects, crates, required=WORKSPACE_LINT_INPUTS) -> list[str]`, and
  a `projects[mid]["task_inputs"]` mapping of `{task_name: list[str] | None}` — `None` meaning Moon
  emitted no `inputFiles` key for that task.

**Why A4 is a separate function, not a fourth branch of `check()`:** the module contract in its
docstring is *Cargo↔Moon dependency-graph parity*, and A4 uses none of the `crates` dependency
data. It also must **not** inherit `check()`'s `if want:` guard, which only reaches crates that have
in-tree dependencies — `paigasus-kernel`, `paigasus-logging`, `paigasus-observability` and
`paigasus-proto-derive` have none, so four of thirteen would go unasserted.

- [ ] **Step 1: Write the failing tests — new `self_test()` rows**

First, extend the two fixture projects in `self_test()`'s `ok` dict with a `task_inputs` key. Use
**lists** — `self_test` deep-copies with `json.loads(json.dumps(ok))` and a `set` is not
JSON-serialisable:

```python
    complete_inputs = ["rs/Cargo.lock", "rs/Cargo.toml", "rs/rust-toolchain.toml"]
    ok = {
        "a-rs": {
            "source_dir": "rs/crates/libs/a",
            "deps": {"b-rs": "explicit"},
            "tasks": {
                "build": ["b-rs:build"],
                "test": ["b-rs:build"],
                "lint": ["b-rs:build"],
            },
            "task_inputs": {"build": [], "test": [], "lint": list(complete_inputs)},
        },
        "b-rs": {
            "source_dir": "rs/crates/libs/b",
            "deps": {},
            "tasks": {"build": [], "test": [], "lint": []},
            "task_inputs": {"build": [], "test": [], "lint": list(complete_inputs)},
        },
    }
```

Then add these rows to `self_test()`, after the existing A3 rows and before the malformed-manifest
block:

```python
    # A4 (SMA-534): the workspace-level lint inputs must be DECLARED for every crate. Distinct from
    # A1-A3, which are about dependency edges — a crate can have a perfect edge set and still be
    # blind to a Cargo.lock bump.
    if check_lint_inputs(ok, crates):
        failures.append("A4 reported violations on the clean fixture")

    # Fires when a required file is missing from the declared inputs.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["task_inputs"]["lint"] = ["rs/Cargo.lock", "rs/Cargo.toml"]
    rows = check_lint_inputs(broken, crates)
    if not rows:
        failures.append("A4 did not fire on a missing workspace lint input")
    elif not any("rs/rust-toolchain.toml" in row for row in rows):
        failures.append("A4 fired but did not name the missing file")

    # Fires for a crate with NO in-tree deps. A3 is guarded by `if want:` and never reaches such a
    # crate; A4 must not copy that shape, or four of the thirteen real crates go unasserted while
    # the negative control stays green.
    broken = json.loads(json.dumps(ok))
    broken["b-rs"]["task_inputs"]["lint"] = []
    if not any(row.startswith("b-rs") for row in check_lint_inputs(broken, crates)):
        failures.append("A4 did not fire for a dep-free crate (it inherited A3's `if want:` guard)")

    # An ABSENT lint task is a different defect from a lint task with incomplete inputs.
    broken = json.loads(json.dumps(ok))
    del broken["a-rs"]["task_inputs"]["lint"]
    if not any("has no `lint` task" in row for row in check_lint_inputs(broken, crates)):
        failures.append("A4 did not distinguish an absent lint task from incomplete inputs")

    # Moon emitting no `inputFiles` for the task must FIRE, never silently skip: a skip would turn a
    # moon-version change into a vacuous pass, which is the failure mode this whole gate exists for.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["task_inputs"]["lint"] = None
    if not any("inputFiles" in row for row in check_lint_inputs(broken, crates)):
        failures.append("A4 did not fire when moon reported no inputFiles")
```

- [ ] **Step 2: Run it to make sure it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py --self-test
```

Expected: **fails** with `NameError: name 'check_lint_inputs' is not defined` (exit 1). That is the
correct red — the function does not exist yet.

- [ ] **Step 3: Implement A4**

Three edits to `ci/affected-graph/cargo_moon_parity.py`.

**3a.** Add the constant next to `NON_CARGO_PARENTS` near the top of the file:

```python
# SMA-534 — the workspace-level files `lint` must key on. `rs/` has no Moon project, so without
# these declared on the inherited lint task a Cargo.lock-only change (every Dependabot Cargo PR)
# schedules no crate task at all. Paths are workspace-relative, exactly as Moon RESOLVES them:
# the YAML says `/rs/Cargo.lock`, `moon query projects` reports `rs/Cargo.lock`.
WORKSPACE_LINT_INPUTS = ("rs/Cargo.lock", "rs/Cargo.toml", "rs/rust-toolchain.toml")
```

**3b.** In `moon_projects()`, collect resolved per-task input files alongside the existing task
deps, and add them to the returned dict:

```python
    for p in json.loads(out)["projects"]:
        tasks = {}
        task_inputs = {}
        for name, task in (p.get("tasks") or {}).items():
            tasks[name] = [
                d if isinstance(d, str) else d.get("target")
                for d in (task.get("deps") or [])
            ]
            # `inputFiles` is a path-keyed OBJECT of resolved workspace-relative paths. Preserve the
            # absent-key case as None rather than collapsing it to []: "moon told us nothing" and
            # "moon told us there are none" are different defects, and A4 must fire loudly on the
            # first instead of reporting a confusing missing-file list. Sorted list, not a set, so
            # self_test()'s json round-trip deep-copy keeps working.
            raw = task.get("inputFiles")
            task_inputs[name] = None if raw is None else sorted(raw.keys())
        projects[p["id"]] = {
            "source_dir": p["source"],
            "deps": {d["id"]: d.get("source") for d in (p.get("dependencies") or [])},
            "tasks": tasks,
            "task_inputs": task_inputs,
        }
    return projects
```

**3c.** Add the function itself, immediately after `check()`:

```python
def check_lint_inputs(projects, crates, required=WORKSPACE_LINT_INPUTS):
    """Return the A4 violation list: crates whose `lint` does not key on the workspace files.

    A1-A3 are about dependency EDGES. A4 is about task INPUTS, and the two are independent: a crate
    can have a flawless edge set and still be structurally blind to a `rs/Cargo.lock` bump, because
    `rs/` has no Moon project for an edge to point at (SMA-534).

    Iterates EVERY crate unconditionally. It deliberately does not reuse `check()`'s `if want:`
    guard, which is only reached by crates that have in-tree dependencies: paigasus-kernel,
    paigasus-logging, paigasus-observability and paigasus-proto-derive have none, so copying that
    shape would leave four of thirteen unasserted with a green negative control.
    """
    by_dir = {p["source_dir"]: mid for mid, p in projects.items()}
    a4 = []
    for _crate, info in sorted(crates.items()):
        mid = by_dir.get(info["source_dir"])
        if mid is None:
            continue
        declared = projects[mid].get("task_inputs") or {}
        if "lint" not in declared:
            a4.append(f"{mid} has no `lint` task (nothing can key on the workspace files)")
            continue
        resolved = declared["lint"]
        if resolved is None:
            a4.append(
                f"{mid}:lint reported no `inputFiles` — moon's output shape changed, so this "
                f"assertion cannot be evaluated (treated as a violation, never skipped)"
            )
            continue
        missing = [f for f in required if f not in resolved]
        if missing:
            a4.append(f"{mid}:lint inputs omit {', '.join(missing)}")
    return a4
```

**3d.** Wire it into `main()`. Replace the result block:

```python
    a1, a2, a3 = check(projects, crates)
    a4 = check_lint_inputs(projects, crates)
    if not (a1 or a2 or a3 or a4):
        print(
            f"PASS  {'cargo-moon-parity':<18} -> "
            f"{len(crates)} crates: every Cargo dep has a Moon edge that schedules its build, "
            f"and every lint keys on the workspace files"
        )
        return 0
```

and add a fourth row to the report loop, after the `a3` row:

```python
        (a4, "`lint` does not key on the workspace-level files, so a dependency bump, a\n"
             "    [workspace.lints] edit or a toolchain drift schedules NOTHING for this crate\n"
             "    (SMA-534).\n"
             "    Fix: the inputs are declared once for ALL crates in .moon/tasks/rust.yml —\n"
             "    restore them there, not per-crate. Expected: /rs/Cargo.lock, /rs/Cargo.toml,\n"
             "    /rs/rust-toolchain.toml."),
```

**3e.** Update the module docstring (lines 1–14) so the file header stays honest — it currently
describes only dependency-graph parity. Append after the existing prose:

```python
# It also carries A4 (SMA-534), which is about task INPUTS rather than edges: every crate's `lint`
# must key on the workspace-level files (Cargo.lock, Cargo.toml, rust-toolchain.toml), since `rs/`
# has no Moon project for a dependency edge to point at. A4 reads moon's RESOLVED `inputFiles`, so
# it stays inside the "never parse YAML" rule above.
```

- [ ] **Step 4: Run the self-test to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py --self-test
```

Expected: `OK   [parity] all three assertions fire on synthetic violations`, exit 0.

- [ ] **Step 5: Prove A4 is non-vacuous against the REAL tree**

The self-test proves A4 fires on fixtures. This proves it fires on the actual repository.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py            # expect PASS, exit 0
cp .moon/tasks/rust.yml /tmp/rust.yml.sma534
# remove the three '/rs/...' input lines from the lint task, then:
python3 ci/affected-graph/cargo_moon_parity.py            # expect 13 named violations, exit 1
cp /tmp/rust.yml.sma534 .moon/tasks/rust.yml
touch .moon/tasks/rust.yml                                # see the warning below
python3 ci/affected-graph/cargo_moon_parity.py            # expect PASS, exit 0 again
```

Expected in the middle run: thirteen `*-rs:lint inputs omit rs/Cargo.lock, rs/Cargo.toml,
rs/rust-toolchain.toml` rows. Record the count — it is evidence for the PR body.

**Warning:** restoring a file with `cp` rolls its mtime *backwards*, which can make a cached
consumer serve stale output. The `touch` above is not optional. Prefer editing the file back with
an editor rather than restoring a copy.

- [ ] **Step 6: Run the whole guard both ways**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/affected-graph/run.sh --negative-control   # expect: negative-control OK, exit 0
ci/affected-graph/run.sh                      # expect: cascade intact, exit 0
```

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -m "test(rs): assert every crate's lint keys on the workspace files (SMA-534)"
```

Body:

```
A1-A3 assert dependency EDGES. A crate can have a flawless edge set and still be blind to
a rs/Cargo.lock bump, because rs/ has no Moon project for an edge to point at — so the
lockfile inputs added alongside are guarded only behaviourally, by one hand-written case
that a future crate could quietly fall out of.

A4 reads moon's own resolved inputFiles and asserts every crate's lint keys on
rs/Cargo.lock, rs/Cargo.toml and rs/rust-toolchain.toml. Self-maintaining: a new crate
inherits the inputs and is asserted with no CSV to update, and a crate that overrides
lint's inputs is caught.

Deliberately iterates every crate rather than reusing check()'s `if want:` guard, which
only reaches crates that have in-tree deps — kernel, logging, observability and
proto-derive have none, so copying that shape would leave four of thirteen unasserted
with a green negative control. A self-test row pins that difference.

Proven non-vacuous against the real tree, not just fixtures: thirteen named violations
with the inputs reverted, zero with them applied.
```

---

### Task 3: Make CI actually run the negative control

**Files:**
- Modify: `moon.yml` (the `repo:affected-smoke` task, around line 117-121)

**Interfaces:**
- Consumes: the self-test rows added in Task 2 — this is what makes CI execute them.
- Produces: nothing later tasks depend on.

- [ ] **Step 1: Observe the gap**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
grep -A3 "^  affected-smoke:" moon.yml
```

Expected: `script: 'ci/affected-graph/run.sh'` — bare, so CI never runs `--negative-control` and the
self-tests that prove the gates can bite are dead code in CI. SMA-526 hit exactly this. Compare with
the precedent at `repo:publish-metadata` (`grep -B2 -A4 "negative-control" moon.yml`).

- [ ] **Step 2: Implement**

Replace the `script:` line of `affected-smoke` with:

```yaml
    # Run the negative control FIRST, mirroring repo:publish-metadata. Without this CI runs only the
    # real suite, so the self-tests that prove these assertions can FIRE are never executed — a
    # rotted self-test ships green, taking the repo's only proof-that-the-gate-bites with it
    # (SMA-526 hit this; SMA-534 adds A4's rows, which are worth exactly nothing unexecuted).
    # Moon does not enable errexit for `script:` blocks — same latent defect the nats-permissions,
    # promtool and publish-metadata tasks document — hence the explicit `set -euo pipefail`.
    script: |
      set -euo pipefail
      ci/affected-graph/run.sh --negative-control
      ci/affected-graph/run.sh
```

- [ ] **Step 3: Run it to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:affected-smoke --force
```

Expected: exit 0, with **both** `negative-control OK: harness reported red on all wrong expectations`
and `== affected-graph cascade intact ==` in the output. If you see only the second, the `script:`
block did not take — check the YAML block indentation.

- [ ] **Step 4: Prove the errexit guard works**

Temporarily break the negative control to confirm a red first command actually fails the task:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 - <<'PY'
p = 'ci/affected-graph/run.sh'
s = open(p).read()
open(p, 'w').write(s.replace('expect_red "neg-wrong-expect"     "rs/crates/libs/paigasus-kernel/src/lib.rs" "paigasus-proto-py"',
                             'expect_red "neg-wrong-expect"     "rs/crates/libs/paigasus-kernel/src/lib.rs" "paigasus-kernel-rs,paigasus-py-bindings-rs,paigasus-gateway-rs,paigasus-kernel-py,paigasus-node-bindings-rs,paigasus-kernel-ts,paigasus-wasm-rs,paigasus-kernel-parity-rs,paigasus-iam-core-rs,paigasus-iam-rs"'))
PY
moon run repo:affected-smoke --force ; echo "exit=$?"
git checkout ci/affected-graph/run.sh
```

Expected: `exit=1` — the task fails because the negative control failed, proving `set -euo pipefail`
is doing its job. If it prints `exit=0`, the errexit line is missing or the block is not a real
multi-line script. Restore `run.sh` afterwards (the `git checkout` above does it) and re-run
`moon run repo:affected-smoke --force` to confirm exit 0.

- [ ] **Step 5: Commit**

```bash
git add moon.yml
git commit -m "ci(repo): run the affected-graph negative control in CI, not just by hand (SMA-534)"
```

Body:

```
repo:affected-smoke ran ci/affected-graph/run.sh bare, so the --negative-control pass —
the only thing that proves these assertions can report red — was a manual step the README
documents and nothing executes. A rotted self-test therefore shipped green, which is
exactly what SMA-526 hit, and SMA-534 has just added five more self-test rows that would
be worth nothing unexecuted.

Runs the control first, mirroring repo:publish-metadata, with the explicit
set -euo pipefail those sibling tasks document as necessary because Moon does not enable
errexit for script blocks. Verified the guard bites: a deliberately green-lit negative
control fails the task.
```

---

### Task 4: `rs/Cargo.toml` joins the `rs/target` cache key

**Files:**
- Modify: `.github/workflows/ci.yml:88-96` (the `Cache Rust (cargo + target)` step)

**Interfaces:**
- Consumes: the widened build surface from Task 1.
- Produces: nothing later tasks depend on.

**Why:** `rs/Cargo.lock` and `rs/rust-toolchain.toml` are already in the key, so a change to either
misses it and `actions/cache` saves the enlarged `rs/target`. `rs/Cargo.toml` is not, and it does not
imply a lockfile change: enabling a feature on an existing workspace dependency leaves `Cargo.lock`
byte-identical. The primary key then **hits exactly**, `actions/cache` skips its save, and cargo
recompiles that dependency and everything above it on every run, permanently. That is the SMA-520
failure mode, and per SMA-520 a verification run cannot reveal it — the fix has to be reasoned, not
observed.

- [ ] **Step 1: Implement — extend the key**

Change the `key:` line from:

```yaml
          key: rust-${{ runner.os }}-${{ hashFiles('rs/rust-toolchain.toml') }}-line-tables-only-lint-deps-${{ hashFiles('rs/Cargo.lock') }}
```

to:

```yaml
          key: rust-${{ runner.os }}-${{ hashFiles('rs/rust-toolchain.toml') }}-line-tables-only-lint-deps-${{ hashFiles('rs/Cargo.lock', 'rs/Cargo.toml') }}
```

and append to the comment block directly above it:

```yaml
          # rs/Cargo.toml joins the hash (SMA-534): lint now keys on it, and a FEATURE flip on an
          # existing workspace dep changes no resolution — Cargo.lock stays byte-identical. The
          # primary key would hit exactly, actions/cache would skip its save, and the extra units
          # that flip compiles would be rebuilt cold every run, forever. Hashing both files instead
          # of adding a literal discriminator segment keeps the existing restore-keys prefixes
          # warm, so there is no one-time cold-churn of the whole cache.
```

Leave `restore-keys` untouched — both prefixes end before the hash, so they still match.

- [ ] **Step 2: Verify the workflow still parses and the key is well-formed**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 -c "import yaml,sys; d=yaml.safe_load(open('.github/workflows/ci.yml')); print('parsed OK')" 2>/dev/null \
  || python3 -c "
import re
s=open('.github/workflows/ci.yml').read()
m=re.search(r'^\s*key: (rust-.*)$', s, re.M)
print('key line:', m.group(1) if m else 'NOT FOUND')
assert m and \"hashFiles('rs/Cargo.lock', 'rs/Cargo.toml')\" in m.group(1)
print('key OK')"
grep -c "line-tables-only-lint-deps-" .github/workflows/ci.yml
```

Expected: the key line contains `hashFiles('rs/Cargo.lock', 'rs/Cargo.toml')`, and the
`line-tables-only-lint-deps-` count is **3** (one key + two restore-key prefixes — unchanged).

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci(repo): key the rs/target cache on rs/Cargo.toml too (SMA-534)"
```

Body:

```
lint now keys on rs/Cargo.toml, and that file does not imply a lockfile change: flipping a
feature on an existing workspace dependency changes no resolution, so Cargo.lock stays
byte-identical and the primary cache key hits exactly. actions/cache skips its save on an
exact hit, so the extra compilation units the flip produces would be rebuilt cold on every
run, forever — the SMA-520 failure mode, which a verification run cannot reveal.

Hashing both files rather than adding a literal discriminator keeps the existing
restore-keys prefixes warm, so there is no one-time cold churn of the whole cache.
```

---

### Task 5: Documentation

**Files:**
- Modify: `ci/affected-graph/README.md`
- Modify: `CLAUDE.md:69-71`

**Interfaces:**
- Consumes: everything from Tasks 1–3.
- Produces: nothing.

- [ ] **Step 1: Update `ci/affected-graph/README.md`**

**1a.** In the bulleted list of checks that "the per-case project sets structurally **cannot** make"
(the section beginning `It also runs two checks that`), change the lead-in `two checks` to
`three checks` and add a bullet after the `proto->service-info-tasks` one:

```markdown
- **`lockfile->all-lint`** asserts that a `rs/Cargo.lock` touch schedules **every** crate's `lint`.
  `rs/` has no Moon project, so the workspace files belong to `repo` and affectedness reaches the
  crates through `lint`'s task **inputs**, not through `dependsOn` — which is why no *project* case
  changes and this one is needed at all. Before SMA-534 that touch scheduled no crate task
  whatsoever, so every Dependabot Cargo PR was unlinted. The case's comment names the three py/ts
  tasks that are one input line away from entering its observed set.
```

**1b.** Add a bullet after the `cargo-moon-parity` one:

```markdown
- **A4** (in `cargo_moon_parity.py`) is the generic twin of `lockfile->all-lint`: for every crate,
  moon's **resolved** `lint` `inputFiles` must contain `rs/Cargo.lock`, `rs/Cargo.toml` and
  `rs/rust-toolchain.toml`. The behavioural case proves the inputs take effect; A4 proves they are
  declared for crates no case names. It iterates every crate unconditionally — unlike A1-A3, which
  are guarded by `if want:` and so never reach the four crates with no in-tree dependencies.
```

**1c.** Replace the two "Run locally / Prove it can fail" lines with:

```markdown
Run locally: `moon run repo:affected-smoke` (or `ci/affected-graph/run.sh`).
`repo:affected-smoke` runs `--negative-control` first and then the real suite, so the proof that
these assertions can report red is executed by CI rather than left as a manual step (SMA-534).
Run the control alone: `ci/affected-graph/run.sh --negative-control`.
```

**1d.** In the `## Maintenance` section, after the bullet beginning `A **task** case
(assert_task_case…`, add:

```markdown
- `lockfile->all-lint` lists **every** Rust crate, so **adding a Rust crate always changes it** —
  unlike the project cases, which only change when the new crate joins a specific dependency chain.
  A4 needs no update in that situation: the new crate inherits `lint`'s inputs from
  `.moon/tasks/rust.yml`, which is the point of declaring them there.
```

**1e.** Update the closing paragraph's version-snapshot note to mention task inputs:

```markdown
The expected sets are a snapshot of `moon query --affected --downstream deep` output at the
**pinned moon version** (currently 2.3.2), and A4 additionally depends on `moon query projects`
emitting per-task `inputFiles` as a path-keyed object. A moon upgrade that changes either — even
benignly — will fail the guard, so re-grounding is a known step of any moon bump. A4 treats an
absent `inputFiles` key as a violation rather than skipping, precisely so such a change cannot
turn into a silent pass.
```

- [ ] **Step 2: Update the `CLAUDE.md` gotcha**

Replace the sentence at `CLAUDE.md:69-71` that currently reads *"A new crate that `dependsOn`
`paigasus-kernel-rs` reds `:affected-smoke` until it's added to the `kernel->bindings` expected set
in `ci/affected-graph/run.sh` (strict-equality guard, SMA-409)."* with:

```markdown
- A new Rust crate reds `:affected-smoke` until it's added to the `lockfile->all-lint` expected set
  in `ci/affected-graph/run.sh` — that case lists **every** crate, so **every** new crate changes it
  (SMA-534) — and, if it `dependsOn` `paigasus-kernel-rs`, to the `kernel->bindings` set as well
  (strict-equality guard, SMA-409). The parity gate's A4 needs no update: a new crate inherits
  `lint`'s workspace inputs from `.moon/tasks/rust.yml`.
```

Keep the rest of that bullet (the `rs/deny.toml` / advisory / machete guidance) unchanged.

- [ ] **Step 3: Verify the docs match reality**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
grep -n "lockfile->all-lint" ci/affected-graph/README.md CLAUDE.md ci/affected-graph/run.sh
grep -n "three checks" ci/affected-graph/README.md
```

Expected: `lockfile->all-lint` appears in all three files; `three checks` appears once. Every claim
in the new prose must be true of the code you actually wrote — re-read `run.sh`'s new case and A4
side by side with the README bullets.

- [ ] **Step 4: Commit**

```bash
git add ci/affected-graph/README.md CLAUDE.md
git commit -m "docs(repo): document the lockfile lint case, A4 and the CI negative control (SMA-534)"
```

Body:

```
Records the three things a future contributor cannot infer from the code: that
lockfile->all-lint lists every crate so every new crate changes it, that A4 needs no
update in that case because the inputs are inherited, and that repo:affected-smoke now
runs the negative control itself rather than leaving it as a README suggestion.
```

---

### Task 6: End-to-end verification

**Files:** none modified. This task produces evidence for the PR body.

**Interfaces:**
- Consumes: Tasks 1–5.
- Produces: the measured numbers and the `ciReport.json` proof quoted in the PR description.

- [ ] **Step 1: Prove `moon ci` — not just `moon query` — schedules the lints**

The guard uses `moon query tasks --affected` as a proxy for what `moon ci` actually runs. That proxy
has never been exercised for a task-input-only affectedness class, so prove it:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
printf '\n' >> rs/Cargo.lock
git add rs/Cargo.lock && git commit -q -m "chore(rs): scratch lockfile touch, do not keep"
moon ci :lint --base HEAD~1 --include-relations
python3 -c "
import json
d = json.load(open('.moon/cache/ciReport.json'))
acts = [a for a in d['actions'] if ':lint' in (a.get('label') or '')]
print('lint actions:', len(acts))
for a in sorted(acts, key=lambda x: x['label']):
    print(' ', a['label'], a['status'])
builds = [a for a in d['actions'] if a.get('label','').endswith(':build')]
print('build actions:', len(builds), '->', sorted({a['status'] for a in builds}))
"
git reset --hard HEAD~1
```

Expected: **13 lint actions**, all with a ran/passed status. The `build` actions pulled in by
`^:build` should be **cached replays**, not runs — their inputs exclude the lockfile. A `build` that
actually *runs* means an input is wider than intended: investigate before proceeding.

If `cargo clippy --locked` rejects the scratch commit, that is itself informative — report it — and
re-run this step with `--locked` temporarily removed from `.moon/tasks/rust.yml` to get the
scheduling evidence, restoring it (and `touch`ing the file) afterwards.

- [ ] **Step 2: Confirm the tree is clean after the scratch commit**

```bash
git status --short
git log --oneline -6
```

Expected: no modification to `rs/Cargo.lock`, and no scratch commit in the log — only the five
commits from Tasks 1–5.

- [ ] **Step 3: Run the full CI gate graph, exactly as CI does**

Per-project Moon tasks do **not** run the repo-level gates. Run the whole list:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :promtool \
  :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata --base origin/main --include-relations
```

Expected: exit 0. If moon reports an unattributed failure count, diagnose with:

```bash
python3 -c "
import json
d = json.load(open('.moon/cache/ciReport.json'))
for a in d['actions']:
    if a.get('status') == 'failed':
        print(a['label'])
"
```

Note `paigasus-iam`'s container tests are Docker-gated and genuinely flaky under parallel load — a
different random subset can fail per run. Before blaming this diff, re-run with `--retries 2` and
compare against a baseline run on unmodified `origin/main`.

- [ ] **Step 4: Record the cost evidence**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
du -sh rs/target
```

Note the value alongside the timings from Task 1 Step 5. The spec's dev-machine figures are
1m47s wall / 5m04s CPU / 3.1 GB; CI numbers must be read off the actual PR run in Stage 6, not
extrapolated. Flag it explicitly if `rs/target` materially exceeds 3.1 GB — the runner has ~14 GB and
SMA-444 documents a mid-link `ld` SIGBUS when it fills.

- [ ] **Step 5: File the follow-up for the residual FFI risk**

The spec records an accepted residual risk: `cargo clippy` never links, and the three
`crate-type = ["cdylib"]` crates fail at link time, while `wasm32-unknown-unknown` is never compiled
at all. Create a Linear issue in team **Sven Maschek**, project **Paigasus Polyglot**, priority
**Medium**, titled:

> `rust: a dep bump still ships unlinked — clippy neither links the cdylibs nor compiles wasm32`

Body must state: the three cdylib crates; that `prebuild.yml`'s pull-request trigger deliberately
excludes `rs/**`; that `ts/packages/paigasus-kernel:{build,test}` and
`py/packages/paigasus-kernel:test` list `/rs/crates/**` inputs but not `rs/Cargo.lock`; the
`wasm-bindgen` 0.2.z ↔ wasm-pack invariant at `rs/Cargo.toml:90-96` as the motivating scenario; and
that closing it costs a `wasm-pack` release build plus a `napi build` on every Dependabot Cargo PR
**and** re-baselines the `lockfile->all-lint` expected set, since those tasks are named
`build`/`test`. Mark it related to SMA-534.

- [ ] **Step 6: Final diff review**

```bash
git diff origin/main --stat
git diff origin/main
```

Confirm: exactly the seven files from the File Structure table, no scratch commits, no debug output,
no `.bak` files, and no change to any pre-existing expected set in `run.sh`.

---

## Self-Review

**Spec coverage:** every scope item 1–6 maps to a task — `.moon/tasks/rust.yml` + `--locked` → T1;
`cargo_moon_parity.py` (all four sub-changes: `moon_projects()` carrying inputs, the A4 function,
the clean-fixture update, the new self-test rows) → T2; `run.sh` case with the py/ts comment → T1;
`moon.yml` negative control → T3; `ci.yml` cache key → T4; docs → T5. All six spec verification
steps map to T1 S4/S5, T2 S4/S5/S6, T3 S3/S4 and T6 S1/S3/S4. The spec's "file as a follow-up" for
the residual FFI risk → T6 S5. The spec's rollback section needs no task (it is `git revert`).

**Placeholder scan:** no TBD/TODO, no "add error handling", no "similar to Task N"; every code step
carries the literal content to write.

**Type consistency:** `check_lint_inputs(projects, crates, required=WORKSPACE_LINT_INPUTS)` is
defined in T2 S3c and called with that exact name in T2 S1 (five self-test rows) and T2 S3d
(`main()`). `projects[mid]["task_inputs"]` is written in T2 S3b as `{task_name: list[str] | None}`
and read in T2 S3c and the T2 S1 fixtures, all as lists — matching the JSON-round-trip constraint in
Global Constraints. `WORKSPACE_LINT_INPUTS` is defined in T2 S3a and referenced in T2 S3c and S3d.
The thirteen crate ids in T1's CSV match the Global Constraints list exactly.

<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-597).** The `ciReport.json` diagnosis advice above does not work as written:
> there is no action-level `exitCode` key, and the file carries no stdout/stderr at all. The
> measured procedure is in CLAUDE.md between the `moon-diagnosis` markers. This document is left
> otherwise unedited as a record of what was believed when it was written.
