# Cross-stack input affectedness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close two Rust task-input gaps and add assertion A7, so the py/ts wrappers' hand-written Rust globs — the ADR-0005 cross-binding guarantee — are held to a derived, floored, file-granular containment check.

**Architecture:** Two one-line edits to `.moon/tasks/rust.yml` close SMA-537. In `ci/affected-graph/cargo_moon_parity.py`, A4 is generalized into `check_task_inputs` (spanning both of moon's input buckets), the FFI task derivation is split out of A5 for reuse, `main()` is restructured so a check cannot be computed and silently dropped, and A7 is added alongside A6.

**Tech Stack:** Moon 2.3.2, Python 3.12 (stdlib only), bash. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-28-sma-560-cross-stack-input-affectedness-design.md` (rev 3, commit `a0d6933`)

## Global Constraints

- **Branch:** `feature/sma-560-assert-cross-stack-input-affectedness`, base `main` @ `2f37378`. Do not create a new branch.
- **Every Bash invocation of moon/uv/buf/nextest** must be prefixed with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` — the Bash tool's PATH lacks the proto-managed CLIs, and shims must come first.
- **No new `repo:*` task is created.** Therefore: no `.github/workflows/ci.yml` `T=(…)` change, no CLAUDE.md marker-block change, no `SELF_SCHEDULED_GATES` entry, and **no `repo:affected-smoke` `inputs` change** (which would drag in a four-site pin update across `ci/actionlint/run.sh` and `ci/affected-graph/ci_targets.py`). The target count stays **27**.
- **No `ci/affected-graph/run.sh` expected set moves.** This is *determined*, not assumed: no case touches `rs/rustfmt.toml`, `rs/rust-toolchain.toml`, the kernel's `Cargo.toml`, or any `build.rs`, and `fmt` is filtered out of `_assert_task_case_impl` (`run.sh:94-97`). Task 6 confirms it. If a set *does* move, stop and report — it means an assumption in the spec was wrong.
- **SPDX headers** on every source file: `// SPDX-License-Identifier: Apache-2.0` (`#` for Python). All files touched here already have one; do not remove it.
- **Conventional commits** with a scope from the enum `rs|py|ts|contracts|ci|docs|deps|release|repo|claude|workspace`. Subject starts lowercase, header ≤100 chars.
- **Commit body lines must never start with `word:`** — commitlint parses that as a footer token and fails `footer-leading-blank`. Rephrase rather than fight it.
- **Do not run `cargo fmt`, `cargo clippy` or any formatter over the tree** as a side effect. This plan changes no Rust source.

---

### Task 1: Close SMA-537's two Rust input gaps

**Files:**
- Modify: `.moon/tasks/rust.yml:8-13` (fileGroups.sources), `.moon/tasks/rust.yml:83-85` (fmt task)

**Interfaces:**
- Consumes: nothing.
- Produces: `@group(sources)` now resolves to `['src/**/*', 'build.rs']` for every Rust crate; `fmt` keys on four inputs. Task 5's A7 relies on `build.rs` being a real input concept but does not read this file.

- [ ] **Step 1: Record today's behaviour, so the fix is provably a change**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
echo "rs/crates/bindings/paigasus-node-bindings/build.rs" | moon query tasks --affected
echo "--- rustfmt.toml ---"
echo "rs/rustfmt.toml" | moon query tasks --affected
```

Expected, and the whole point of SMA-537: **neither** prints any `paigasus-node-bindings-rs:*` or `*:fmt` task. Save both outputs; Step 5 diffs against them.

- [ ] **Step 2: Add `build.rs` to the shared `sources` fileGroup**

In `.moon/tasks/rust.yml`, replace the `fileGroups.sources` block:

```yaml
fileGroups:
  sources:
    - 'src/**/*'
    # SMA-537 — `cargo build` RUNS a crate-root build.rs and `cargo clippy --all-targets`
    # COMPILES it, but `src/**/*` does not match it, so editing one re-keyed no crate task.
    # Declared in the shared group rather than per-crate so a new crate cannot forget it — the
    # same argument .moon/tasks/rust.yml makes for declaring `deps: ['^:build']` centrally.
    # Reaches build/test/lint/build-release AND fmt, all of which consume @group(sources).
    # Twelve of thirteen crates have no build.rs; the entry simply matches nothing for them.
    # It does NOT disturb A6: a crate's own build.rs is `own/`-prefixed and excluded from
    # `observed` (cargo_moon_parity.py:400-401).
    - 'build.rs'
  tests:
    - 'tests/**/*'
    - '**/*_test.rs'
```

- [ ] **Step 3: Widen the `fmt` task's inputs**

In `.moon/tasks/rust.yml`, replace the `fmt` task:

```yaml
  fmt:
    command: 'cargo fmt --check'
    # SMA-537. `cargo fmt --check` formats EVERY target in the package — src/, tests/, benches/
    # and build.rs — but this task keyed on @group(sources) alone, so a misformatted integration
    # test could merge green and red `main` on an unrelated later src edit.
    #   @group(sources)         now includes build.rs (see fileGroups above)
    #   @group(tests)           tests/ is formatted too; an empty group matches nothing, which is
    #                           already true for `test`/`lint` on crates without a tests/ dir
    #   /rs/rustfmt.toml        the format config itself (max_width = 200); changing it must
    #                           invalidate every crate's fmt, or the tree drifts silently
    #   /rs/rust-toolchain.toml selects WHICH rustfmt runs — the same argument `lint` already
    #                           makes for this file
    # Deliberately NOT @group(upstreams): `cargo fmt --check` reads only the crate's own files,
    # so it cannot be broken by an upstream crate edit (SMA-526 rejected that propagation, and
    # that rejection was correct). This is a CONFIG-edit hole, which is a different defect.
    inputs: ['@group(sources)', '@group(tests)', '/rs/rustfmt.toml', '/rs/rust-toolchain.toml']
```

- [ ] **Step 4: Verify the YAML still loads**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query projects > /dev/null && echo "graph loads"
```

Expected: `graph loads`. A malformed fileGroup is a hard graph-load error for every moon command, so this fails loudly if the edit is wrong.

- [ ] **Step 5: Verify both holes are closed**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
echo "rs/crates/bindings/paigasus-node-bindings/build.rs" | moon query tasks --affected
echo "--- rustfmt.toml ---"
echo "rs/rustfmt.toml" | moon query tasks --affected
```

Expected now: the first prints `paigasus-node-bindings-rs:build`, `:test`, `:lint`, `:fmt` (and the downstream tasks that key on that crate). The second prints **thirteen** `*:fmt` tasks.

This is an **ad-hoc probe, not a `run.sh` case**. Do not add it to `run.sh`: `_assert_task_case_impl` filters to `build`/`test`/`lint` names (`run.sh:94-97`), so a `fmt` case there would assert nothing and pass vacuously.

- [ ] **Step 6: Confirm the parity gate still passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py --self-test && \
python3 ci/affected-graph/cargo_moon_parity.py
```

Expected: both exit 0. The second prints `PASS  cargo-moon-parity  -> 13 crates: …`.

- [ ] **Step 7: Commit**

```bash
git add .moon/tasks/rust.yml
git commit -F - <<'MSG'
fix(rs): key crate tasks on build.rs and the format config (SMA-537)

cargo build runs a crate-root build.rs and clippy --all-targets compiles it, but
src/**/* never matched it, so editing one re-keyed no crate task. Adding it to the
shared sources fileGroup reaches build, test, lint, build-release and fmt at once,
and a new crate cannot forget it.

Separately, cargo fmt --check formats every target in the package while fmt keyed
on sources alone. It now also keys on tests, the rustfmt config and the toolchain
pin, so a misformatted integration test cannot merge green and red main later on an
unrelated edit.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
```

---

### Task 2: Generalize A4 into `check_task_inputs`, spanning both input buckets

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — `check_lint_inputs` (`:196-227`), `self_test()`, `main()` (`:894`)

**Interfaces:**
- Consumes: `WORKSPACE_LINT_INPUTS` (existing module constant).
- Produces: `check_task_inputs(projects, crates, task, required) -> list[str]` and a new module constant `FMT_TASK_INPUTS`. Task 4 folds both calls into `main()`'s findings list.

**Why both buckets:** `check_lint_inputs` reads `task_inputs` only — moon's `inputFiles`. Literals like `rs/rustfmt.toml` land there, but `@group(sources)` and `@group(tests)` resolve to **globs** and land in `inputGlobs` (`moon_projects()`, `:441-448`). A one-bucket check cannot see `@group(tests)` at all, so removing it later would restore Task 1's bug with nothing red.

- [ ] **Step 1: Write the failing self-test rows**

In `self_test()`, immediately after the existing `if not REQUIRED_FFI_TASKS:` block, add:

```python
    if not FMT_TASK_INPUTS:
        failures.append("FMT_TASK_INPUTS is empty — the fmt half of A4 would assert nothing")

    # A4-fmt: the fmt call must span BOTH buckets. `rs/rustfmt.toml` is a literal (inputFiles)
    # and `rs/crates/libs/b/tests/**/*` is a glob (inputGlobs), so a one-bucket check passes
    # while blind to half its own required set — the split A6 already exists because of.
    fmt_ok = json.loads(json.dumps(ok))
    for pid in ("a-rs", "b-rs"):
        fmt_ok[pid]["task_inputs"]["fmt"] = ["rs/rustfmt.toml", "rs/rust-toolchain.toml"]
        src = fmt_ok[pid]["source_dir"]
        fmt_ok[pid]["task_input_globs"]["fmt"] = [f"{src}/src/**/*", f"{src}/tests/**/*"]
    if check_task_inputs(fmt_ok, crates, "fmt", FMT_TASK_INPUTS) != []:
        failures.append("A4-fmt reported violations on a complete fixture")

    broken = json.loads(json.dumps(fmt_ok))
    broken["a-rs"]["task_input_globs"]["fmt"] = ["rs/crates/libs/a/src/**/*"]
    if not any(
        "tests/**/*" in row
        for row in check_task_inputs(broken, crates, "fmt", FMT_TASK_INPUTS)
    ):
        failures.append("A4-fmt did not fire on a fmt task missing @group(tests)")

    broken = json.loads(json.dumps(fmt_ok))
    broken["a-rs"]["task_inputs"]["fmt"] = ["rs/rust-toolchain.toml"]
    if not any(
        "rustfmt.toml" in row
        for row in check_task_inputs(broken, crates, "fmt", FMT_TASK_INPUTS)
    ):
        failures.append("A4-fmt did not fire on a fmt task missing the rustfmt config")
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
python3 ci/affected-graph/cargo_moon_parity.py --self-test
```

Expected: FAIL with `NameError: name 'check_task_inputs' is not defined`.

- [ ] **Step 3: Add the constant**

Directly below the existing `FFI_TASK_INPUTS` definition, add:

```python
# SMA-537 — what every crate's `fmt` must key on. The two globs come from the shared fileGroups
# and land in moon's `inputGlobs`; the two literals land in `inputFiles`. check_task_inputs spans
# both, which is the whole reason it does not simply reuse check_lint_inputs' one-bucket read.
FMT_TASK_INPUTS = (
    "rs/rustfmt.toml",
    "rs/rust-toolchain.toml",
    "src/**/*",
    "tests/**/*",
)
```

- [ ] **Step 4: Replace `check_lint_inputs` with `check_task_inputs`**

Replace the whole `check_lint_inputs` function with:

```python
def check_task_inputs(projects, crates, task, required):
    """Return the A4 violation list: crates whose `task` does not key on `required`.

    A1-A3 are about dependency EDGES. A4 is about task INPUTS, and the two are independent: a crate
    can have a flawless edge set and still be structurally blind to a `rs/Cargo.lock` bump, because
    `rs/` has no Moon project for an edge to point at (SMA-534).

    Iterates EVERY crate unconditionally. It deliberately does not reuse `check()`'s `if want:`
    guard, which is only reached by crates that have in-tree dependencies: paigasus-kernel,
    paigasus-logging, paigasus-observability and paigasus-proto-derive have none, so copying that
    shape would leave four of thirteen unasserted with a green negative control.

    Spans BOTH input buckets (SMA-537). moon splits resolved inputs by kind: plain paths go to
    `inputFiles`, globs to `inputGlobs`. `lint`'s required set is all literals, but `fmt`'s is half
    globs (`@group(sources)`, `@group(tests)`), so a one-bucket read would silently assert nothing
    about them. An entry is matched if it appears in EITHER bucket, or as the tail of a
    workspace-relative path in either — `src/**/*` resolves per-crate to
    `rs/crates/libs/<crate>/src/**/*`, while `rs/rustfmt.toml` resolves verbatim.
    """
    by_dir = {p["source_dir"]: mid for mid, p in projects.items()}
    a4 = []
    for _crate, info in sorted(crates.items()):
        mid = by_dir.get(info["source_dir"])
        if mid is None:
            continue
        declared = projects[mid].get("task_inputs") or {}
        declared_globs = projects[mid].get("task_input_globs") or {}
        if task not in declared:
            a4.append(f"{mid} has no `{task}` task (nothing can key on {', '.join(required)})")
            continue
        files, globs = declared[task], declared_globs.get(task)
        if files is None or globs is None:
            a4.append(
                f"{mid}:{task} reported no `inputFiles`/`inputGlobs` — moon's output shape "
                f"changed, so this assertion cannot be evaluated (treated as a violation, "
                f"never skipped)"
            )
            continue
        observed = set(files) | set(globs)
        missing = [
            f for f in required
            if f not in observed and not any(o.endswith(f"/{f}") for o in observed)
        ]
        if missing:
            a4.append(f"{mid}:{task} inputs omit {', '.join(missing)}")
    return a4
```

- [ ] **Step 5: Update the single existing call site**

In `main()`, replace `a4 = check_lint_inputs(projects, crates)` with:

```python
    a4 = check_task_inputs(projects, crates, "lint", WORKSPACE_LINT_INPUTS)
    a4_fmt = check_task_inputs(projects, crates, "fmt", FMT_TASK_INPUTS)
```

and add `a4_fmt` to the aggregate guard `if not (a1 or a2 or a3 or a4 or a5 or a6):` → `... or a4 or a4_fmt or a5 or a6):`, and add this entry to the report tuple immediately after the existing `a4` entry:

```python
        (a4_fmt, "`fmt` does not key on everything `cargo fmt --check` actually reads, so a\n"
                 "    rustfmt.toml edit, a toolchain bump or a misformatted tests/ file schedules\n"
                 "    NOTHING for this crate (SMA-537).\n"
                 "    Fix: the inputs are declared once for ALL crates in .moon/tasks/rust.yml —\n"
                 "    restore them there, not per-crate. Expected: @group(sources), @group(tests),\n"
                 "    /rs/rustfmt.toml, /rs/rust-toolchain.toml."),
```

*(Task 4 replaces this hand-maintained guard/report pairing entirely. Doing it in two steps keeps each task independently reviewable and each commit green.)*

- [ ] **Step 6: Run the self-test to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py --self-test && \
python3 ci/affected-graph/cargo_moon_parity.py
```

Expected: both exit 0. If A4-fmt fires against the real graph, Task 1 was not applied — stop and fix that first.

- [ ] **Step 7: Prove it bites against the real tree**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
python3 - <<'PY'
import io, subprocess
p = ".moon/tasks/rust.yml"
s = io.open(p, encoding="utf-8").read()
io.open(p + ".bak", "w", encoding="utf-8").write(s)
io.open(p, "w", encoding="utf-8").write(
    s.replace(
        "inputs: ['@group(sources)', '@group(tests)', '/rs/rustfmt.toml', '/rs/rust-toolchain.toml']",
        "inputs: ['@group(sources)', '/rs/rustfmt.toml', '/rs/rust-toolchain.toml']",
    )
)
PY
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py; echo "EXIT=$?  (expected 1, rows naming tests/**/*)"
mv .moon/tasks/rust.yml.bak .moon/tasks/rust.yml
touch .moon/tasks/rust.yml
python3 ci/affected-graph/cargo_moon_parity.py; echo "EXIT=$?  (expected 0)"
```

Expected: 13 rows reading `…:fmt inputs omit tests/**/*`, then a clean pass after restore. The `touch` is required — restoring a file by `mv` rolls its mtime backwards, and moon's hasher can serve a stale result.

- [ ] **Step 8: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -F - <<'MSG'
ci(repo): assert the fmt task's inputs, spanning both moon buckets (SMA-537)

A4 asserted lint's workspace files and nothing asserted fmt's, so the inputs added
for SMA-537 could be removed again with nothing red. Generalizes the check to take
a task name and a required set.

The check now reads inputFiles and inputGlobs together. moon splits resolved inputs
by kind, and half of fmt's required set are globs from the shared fileGroups, so a
one-bucket read would have asserted nothing about exactly the entries most likely to
be dropped.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
```

---

### Task 3: Split `derive_ffi_tasks` out of `check_ffi_inputs`

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — `check_ffi_inputs` (`:233-276`), `self_test()`

**Interfaces:**
- Consumes: `FFI_MARKERS`, `MoonOutputError` (existing).
- Produces: `derive_ffi_tasks(projects) -> set[str]` returning `"<pid>:<task>"` targets. **Task 5's A7 consumes this**; it is the entire reason A7 needs no hand-maintained task list.

This is a **pure refactor**: A5's behaviour must not change.

- [ ] **Step 1: Add a self-test row asserting the derivation is shared**

In `self_test()`, after the `FMT_TASK_INPUTS` non-emptiness row from Task 2, add:

```python
    # A5/A7 share one derivation. If they ever diverge, A7 silently stops examining a wrapper
    # while A5 keeps passing — so assert the split function returns exactly what A5 matches.
    ffi_fixture = {
        "w-ts": {
            "source_dir": "ts/packages/w", "deps": {}, "language": "typescript",
            "tasks": {"build": []}, "task_inputs": {"build": []},
            "task_input_globs": {"build": []},
            "invocations": {"build": "touch ../x && pnpm exec napi build --platform"},
        },
        "q-rs": {
            "source_dir": "rs/crates/libs/q", "deps": {}, "language": "rust",
            "tasks": {"build": []}, "task_inputs": {"build": []},
            "task_input_globs": {"build": []},
            "invocations": {"build": "cargo build"},
        },
    }
    if derive_ffi_tasks(ffi_fixture) != {"w-ts:build"}:
        failures.append(
            f"derive_ffi_tasks did not match exactly the FFI-marked task: "
            f"{sorted(derive_ffi_tasks(ffi_fixture))}"
        )
```

- [ ] **Step 2: Run it to verify it fails**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test
```

Expected: FAIL with `NameError: name 'derive_ffi_tasks' is not defined`.

- [ ] **Step 3: Extract the function**

Insert directly above `check_ffi_inputs`:

```python
def derive_ffi_tasks(projects):
    """Every `<pid>:<task>` whose resolved invocation shells out to a Rust build.

    Shared by A5 (which asserts those tasks key on the workspace files) and A7 (which asserts the
    non-Rust ones key on their upstream crates' sources). Sharing it is deliberate: a wrapper that
    A5 covers and A7 does not — or the reverse — is a hole neither check can see.

    Raises MoonOutputError if a task exposes none of a command, a script, or any args.
    """
    matched = set()
    for pid in sorted(projects):
        invocations = projects[pid].get("invocations") or {}
        for name in sorted(invocations):
            blob = invocations[name]
            if blob is None:
                raise MoonOutputError(
                    f"{pid}:{name} reported none of a `command`, a `script`, or any `args` — "
                    f"moon's output shape changed, so the FFI derivation cannot be evaluated"
                )
            if any(marker in blob for marker in FFI_MARKERS):
                matched.add(f"{pid}:{name}")
    return matched
```

- [ ] **Step 4: Rewrite `check_ffi_inputs` to consume it**

Replace the body of `check_ffi_inputs` (keep its docstring, adding a line that the derivation now lives in `derive_ffi_tasks`):

```python
    matched, a5 = derive_ffi_tasks(projects), []
    for target in sorted(matched):
        pid, _, name = target.partition(":")
        resolved = (projects[pid].get("task_inputs") or {}).get(name)
        if resolved is None:
            a5.append(
                f"{target} reported no `inputFiles` — moon's output shape changed, so this "
                f"assertion cannot be evaluated (treated as a violation, never skipped)"
            )
            continue
        missing = [f for f in required if f not in resolved]
        if missing:
            a5.append(f"{target} inputs omit {', '.join(missing)}")
    for target in sorted(set(floor) - matched):
        a5.append(
            f"{target} is not matched by any FFI marker — the derived set no longer covers it, "
            f"so A5 would assert nothing about it (see FFI_MARKERS)"
        )
    return a5
```

- [ ] **Step 5: Run the full self-test and the real check**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py --self-test && \
python3 ci/affected-graph/cargo_moon_parity.py
```

Expected: both exit 0, and **every pre-existing A5 self-test row still passes** — this is a refactor, so any A5 row that now fails means behaviour changed.

- [ ] **Step 6: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -F - <<'MSG'
ci(repo): split the FFI task derivation out of A5 for reuse (SMA-560)

A5 matches each task's resolved invocation against the FFI markers to find the tasks
that shell out to a Rust build. A7 needs exactly that set, filtered to the non-Rust
projects, so extracting it avoids a second hand-maintained list that could drift out
of agreement with A5's.

Pure refactor. A5's behaviour is unchanged and its existing self-test rows still
pass.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
```

---

### Task 4: Make a forgotten check structurally impossible in `main()`

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — `main()` (`:882-947`), `self_test()`

**Interfaces:**
- Consumes: every `check_*` function.
- Produces: a `findings` list of `(rows, title)` pairs that is the single source for both the pass/fail guard and the report. Task 5 adds A7 by appending one entry to it.

**Why:** SMA-542's lesson is that a check's own call site goes unguarded. Here there are *two* unguarded sites — the aggregate guard `if not (a1 or … or a6)` and the report tuple. Calling a check and folding it into only one of them is a green no-op. Building both from one list removes the possibility rather than detecting it.

- [ ] **Step 1: Write the failing self-test row**

In `self_test()`, after the `derive_ffi_tasks` row from Task 3, add:

```python
    # Guard the guard (SMA-542). A check that is defined but never invoked by main() asserts
    # nothing, and no fixture here would notice — self_test calls the check functions directly.
    # This is generic on purpose: it covers a future A8 on the day it is written.
    main_src = inspect.getsource(main)
    unreferenced = sorted(
        name for name in globals()
        if name.startswith("check_") and f"{name}(" not in main_src
    )
    if unreferenced:
        failures.append(
            f"main() never calls {', '.join(unreferenced)} — a check that is defined but not "
            f"invoked asserts nothing (SMA-542)"
        )
```

Add `import inspect` to the module's imports if it is not already present.

- [ ] **Step 2: Run it to verify it passes for the wrong reason, then make it meaningful**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test
```

Expected: **PASS** — every check is currently called. That is correct but proves nothing yet, so verify the row can fire:

```bash
python3 - <<'PY'
import io
p = "ci/affected-graph/cargo_moon_parity.py"
s = io.open(p, encoding="utf-8").read()
io.open(p + ".bak", "w", encoding="utf-8").write(s)
io.open(p, "w", encoding="utf-8").write(
    s.replace("    a6 = check_upstream_inputs(projects)\n", "    a6 = []\n")
)
PY
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "EXIT=$? (expected 1, naming check_upstream_inputs)"
mv ci/affected-graph/cargo_moon_parity.py.bak ci/affected-graph/cargo_moon_parity.py
```

Expected: exit 1 with a row naming `check_upstream_inputs`.

- [ ] **Step 3: Restructure `main()`**

Replace everything in `main()` from `a1, a2, a3 = check(projects, crates)` to the end of the report loop with:

```python
    a1, a2, a3 = check(projects, crates)
    # ONE list, used for BOTH the pass/fail verdict and the report. Previously the two were
    # written separately, so a new check folded into one and not the other was a green no-op —
    # the SMA-542 shape, where the guard around a check is as unguarded as its call site.
    # Appending here is now the only way to add a check, and forgetting to means it is never
    # called at all, which is loud rather than silent.
    findings = [
        (a1, "Cargo dep with NO Moon edge (under-builds — CI stays green while skipping work).\n"
             "    Fix: add the upstream to `dependsOn` in the consumer's moon.yml."),
        (a2, "Hand-declared Moon edge with NO Cargo backing (over-builds).\n"
             "    Fix: delete it, or add it to ALLOW_NO_CARGO_BACKING with a reason."),
        (a3, "Moon edge exists but the upstream's build is NOT scheduled — the affected-graph\n"
             "    guard CANNOT see this (SMA-429 F3).\n"
             "    Fix: for `build`/`test`, add '^:build' to the task's `deps` in the consumer's\n"
             "    moon.yml. For `lint` the dep is declared once for ALL crates in\n"
             "    .moon/tasks/rust.yml — restore it there, not per-crate (SMA-526)."),
        (check_task_inputs(projects, crates, "lint", WORKSPACE_LINT_INPUTS),
             "`lint` does not key on the workspace-level files, so a dependency bump, a\n"
             "    [workspace.lints] edit or a toolchain drift schedules NOTHING for this crate\n"
             "    (SMA-534).\n"
             "    Fix: the inputs are declared once for ALL crates in .moon/tasks/rust.yml —\n"
             "    restore them there, not per-crate. Expected: /rs/Cargo.lock, /rs/Cargo.toml,\n"
             "    /rs/rust-toolchain.toml."),
        (check_task_inputs(projects, crates, "fmt", FMT_TASK_INPUTS),
             "`fmt` does not key on everything `cargo fmt --check` actually reads, so a\n"
             "    rustfmt.toml edit, a toolchain bump or a misformatted tests/ file schedules\n"
             "    NOTHING for this crate (SMA-537).\n"
             "    Fix: the inputs are declared once for ALL crates in .moon/tasks/rust.yml —\n"
             "    restore them there, not per-crate. Expected: @group(sources), @group(tests),\n"
             "    /rs/rustfmt.toml, /rs/rust-toolchain.toml."),
        (a5, "An FFI build task does not key on the workspace-level files, so a dependency bump\n"
             "    replays a CACHED artifact built from a different resolution — and clippy cannot\n"
             "    cover it, because it never links a cdylib and never targets wasm32 (SMA-546).\n"
             "    Fix: add /rs/Cargo.lock, /rs/Cargo.toml, /rs/rust-toolchain.toml and\n"
             "    /.prototools to that task's `inputs`. A `not matched by any FFI marker` row\n"
             "    means the opposite — the task stopped looking like a Rust build to A5; either\n"
             "    restore the invocation or update FFI_MARKERS."),
        (check_upstream_inputs(projects),
             "A crate's build/test/lint does not key on its upstream crates' sources, so an\n"
             "    upstream change SELECTS NOTHING for this crate and its cached PASS replays\n"
             "    against a different upstream (SMA-528).\n"
             "    Fix: the list lives in that crate's own moon.yml under `fileGroups.upstreams` —\n"
             "    two entries per upstream, `/<src_dir>/src/**/*` and `/<src_dir>/Cargo.toml`,\n"
             "    for its TRANSITIVE dependsOn closure. A `not in its closure` row is the\n"
             "    opposite: delete the entry, or add it to ALLOW_OVER_APPROXIMATION with a reason.\n"
             "    A `FLOOR:` row means the check itself cannot be trusted — the crate is missing\n"
             "    from the graph, it dropped out of A6's examined set (e.g. stopped reporting\n"
             "    `language: rust`), or its dependsOn closure derivation is broken — fix that\n"
             "    first, every other A6 row is meaningless until it passes."),
    ]

    if not any(rows for rows, _ in findings):
        print(
            f"PASS  {'cargo-moon-parity':<18} -> "
            f"{len(crates)} crates: every Cargo dep has a Moon edge that schedules its build, "
            f"every lint and fmt keys on the files its command reads, every FFI build task does "
            f"too, and every crate keys on its upstream sources"
        )
        return 0

    print("FAIL  [cargo-moon-parity] Cargo and Moon disagree", file=sys.stderr)
    for rows, title in findings:
        if rows:
            print(f"  {title}", file=sys.stderr)
            for row in rows:
                print(f"      {row}", file=sys.stderr)
    return 1
```

- [ ] **Step 4: Run the self-test and the real check**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py --self-test && \
python3 ci/affected-graph/cargo_moon_parity.py
```

Expected: both exit 0.

- [ ] **Step 5: Verify the FAIL path still reports correctly**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
python3 - <<'PY'
import io
p = ".moon/tasks/rust.yml"
s = io.open(p, encoding="utf-8").read()
io.open(p + ".bak", "w", encoding="utf-8").write(s)
io.open(p, "w", encoding="utf-8").write(s.replace("      - 'build.rs'\n", ""))
PY
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py; echo "EXIT=$? (expected 1 with a readable fmt section)"
mv .moon/tasks/rust.yml.bak .moon/tasks/rust.yml
touch .moon/tasks/rust.yml
python3 ci/affected-graph/cargo_moon_parity.py; echo "EXIT=$? (expected 0)"
```

- [ ] **Step 6: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -F - <<'MSG'
ci(repo): build the parity verdict and report from one list (SMA-560)

The pass/fail guard and the failure report were written separately, so a check
folded into one and not the other was a green no-op. That is the SMA-542 shape one
level up, where the guard around a check goes as unguarded as its call site.

Both now derive from a single findings list, so appending is the only way to add a
check and forgetting to means it is never called at all. A self-test row also
asserts that every check function defined in the module is referenced by main,
which covers a future assertion on the day it is written.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
```

---

### Task 5: Add assertion A7 and fix the two wrapper under-declarations

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` (new constant, new check, `main()` findings list, `self_test()`)
- Modify: `py/packages/paigasus-kernel/moon.yml` (`test.inputs`)
- Modify: `ts/packages/paigasus-kernel/moon.yml` (`build.inputs`, `test.inputs`)

**Interfaces:**
- Consumes: `derive_ffi_tasks` (Task 3), `rust_closure` (existing, unmodified), the `findings` list (Task 4).
- Produces: `REQUIRED_WRAPPER_CLOSURE` and `check_wrapper_upstream_inputs(projects, root=None, floor=REQUIRED_WRAPPER_CLOSURE) -> list[str]`.

> **Amended after this plan was executed.** `root` is now a **required** positional parameter —
> `check_wrapper_upstream_inputs(projects, root, floor=REQUIRED_WRAPPER_CLOSURE)`. The final
> whole-branch review measured that with `root` defaulting to `None`, deleting BOTH
> `build.rs` lines from `ts/packages/paigasus-kernel/moon.yml` left the real run **green** — the
> default silently un-asserted A7's `build.rs` half, unpinning this plan's own headline fix.
> The `root is not None` guard is gone with it. Every call below that omits `root` now raises
> `TypeError`, which is the point: the omission is loud rather than silent. The plan text is
> left as written elsewhere, as the record of what was planned.

**One commit.** A7 must red on the pre-existing under-declarations before they are fixed — that is the genuine failing test — but the two must land together so history never contains a state where A7 fails on `main`.

- [ ] **Step 1: Write the failing self-test rows**

In `self_test()`, after Task 4's `unreferenced` row, add:

```python
    if not REQUIRED_WRAPPER_CLOSURE:
        failures.append("REQUIRED_WRAPPER_CLOSURE is empty — A7's floor would assert nothing")

    # A7 fixture: a ts wrapper depending on a binding crate that depends on the kernel. The
    # wrapper declares the manifest as a literal and the sources as a glob — the same two-bucket
    # split A6 spans — plus one legitimate extra outside its closure (the parity corpus), which
    # containment must ALLOW and strict equality would have wrongly flagged.
    wrap = {
        "k-ts": {
            "source_dir": "ts/packages/k", "deps": {"nb-rs": "explicit"}, "language": "typescript",
            "tasks": {"build": []},
            "task_inputs": {"build": ["rs/crates/libs/kern/Cargo.toml",
                                      "rs/crates/bindings/nb/Cargo.toml"]},
            "task_input_globs": {"build": ["rs/crates/libs/kern/src/**/*",
                                           "rs/crates/bindings/nb/src/**/*",
                                           "rs/crates/libs/parity/vectors/**/*"]},
            "invocations": {"build": "pnpm exec napi build --platform"},
        },
        "nb-rs": {
            "source_dir": "rs/crates/bindings/nb", "deps": {"kern-rs": "explicit"},
            "language": "rust", "tasks": {"build": []}, "task_inputs": {"build": []},
            "task_input_globs": {"build": []}, "invocations": {"build": "cargo build"},
        },
        "kern-rs": {
            "source_dir": "rs/crates/libs/kern", "deps": {}, "language": "rust",
            "tasks": {"build": []}, "task_inputs": {"build": []},
            "task_input_globs": {"build": []}, "invocations": {"build": "cargo build"},
        },
    }
    wrap_floor = {"k-ts": {"kern-rs", "nb-rs"}}
    if check_wrapper_upstream_inputs(wrap, floor=wrap_floor) != []:
        failures.append(
            f"A7 reported violations on a complete fixture: "
            f"{check_wrapper_upstream_inputs(wrap, floor=wrap_floor)}"
        )

    # A7-a: a MISSING upstream glob is the dangerous direction and must fire.
    broken = json.loads(json.dumps(wrap))
    broken["k-ts"]["task_input_globs"]["build"] = ["rs/crates/libs/kern/src/**/*"]
    if not any(
        "rs/crates/bindings/nb/src/**/*" in row
        for row in check_wrapper_upstream_inputs(broken, floor=wrap_floor)
    ):
        failures.append("A7 did not fire on a wrapper task missing an upstream's sources")

    # A7-b: the manifest half, which lives in the OTHER bucket. A one-bucket A7 passes this.
    broken = json.loads(json.dumps(wrap))
    broken["k-ts"]["task_inputs"]["build"] = ["rs/crates/bindings/nb/Cargo.toml"]
    if not any(
        "rs/crates/libs/kern/Cargo.toml" in row
        for row in check_wrapper_upstream_inputs(broken, floor=wrap_floor)
    ):
        failures.append("A7 did not fire on a wrapper task missing an upstream's Cargo.toml")

    # A7-c: Rust projects belong to A6, never A7. Double-covering them would make A6's strict
    # equality and A7's containment disagree on the same task.
    rusty = json.loads(json.dumps(wrap))
    rusty["k-ts"]["language"] = "rust"
    if check_wrapper_upstream_inputs(rusty, floor={}) != []:
        failures.append("A7 examined a Rust project, which is A6's job")

    # A7-d: the FLOOR must fire when the closure derivation degrades to empty. Emptying `deps`
    # also empties `want`, so the per-task loop goes quiet by itself — this MUST match the
    # `FLOOR:` prefix or it passes with the whole floor block deleted (A6-e's lesson).
    broken = json.loads(json.dumps(wrap))
    broken["k-ts"]["deps"] = {}
    if not any(
        row.startswith("FLOOR:")
        for row in check_wrapper_upstream_inputs(broken, floor=wrap_floor)
    ):
        failures.append("A7 floor did not fire on a neutered closure derivation")

    # A7-e: a floor entry naming a project that is not examined at all is a FLOOR violation,
    # never a silent skip — the wrapper's task could have stopped matching an FFI marker.
    broken = json.loads(json.dumps(wrap))
    broken["k-ts"]["invocations"]["build"] = "echo nothing"
    if not any(
        row.startswith("FLOOR:")
        for row in check_wrapper_upstream_inputs(broken, floor=wrap_floor)
    ):
        failures.append("A7 floor did not fire when a wrapper stopped matching any FFI marker")

    # A7-f: a floor entry naming an absent project is a FLOOR violation.
    if not any(
        row.startswith("FLOOR:")
        for row in check_wrapper_upstream_inputs(wrap, floor={"ghost-ts": {"kern-rs"}})
    ):
        failures.append("A7 floor did not fire on a floor entry naming an absent project")
```

- [ ] **Step 2: Run it to verify it fails**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test
```

Expected: FAIL with `NameError: name 'REQUIRED_WRAPPER_CLOSURE' is not defined`.

- [ ] **Step 3: Add the floor constant**

Directly below `REQUIRED_CLOSURE_EDGES`, add:

```python
# A7's anti-vacuity floor (SMA-560), and the reason it is EDGE-based rather than a task list.
# A7 asserts CONTAINMENT (`want <= observed`), and a containment check whose `want` empties is
# VACUOUSLY SATISFIED — it prints PASS having asserted nothing. A moon rename, a `dependencies`
# reshape or a `language` field change on a binding crate would each do that. A task-name floor
# cannot see it, because the tasks are still examined; only these edges can.
# The task SET needs no floor of its own: A7 derives it from derive_ffi_tasks(), whose own floor
# is REQUIRED_FFI_TASKS, already asserted by A5.
REQUIRED_WRAPPER_CLOSURE = {
    "paigasus-kernel-py": {"paigasus-kernel-rs", "paigasus-py-bindings-rs"},
    "paigasus-kernel-ts": {
        "paigasus-kernel-rs", "paigasus-node-bindings-rs", "paigasus-wasm-rs",
    },
}
```

- [ ] **Step 4: Add A7**

Insert directly after `check_upstream_inputs`:

```python
def check_wrapper_upstream_inputs(projects, root=None, floor=REQUIRED_WRAPPER_CLOSURE):
    """Return the A7 violation list: py/ts wrappers that do not key on their upstream crates.

    The cross-stack half of A6. A6 iterates `language == "rust"` only, so the py/ts wrappers —
    whose hand-written `/rs/...` globs ARE the ADR-0005 cross-binding guarantee — were asserted
    by nothing. Note the kernel->wrapper edge specifically IS covered, by one hand-written
    `run.sh` case (`kernel->consumer-tasks`); what was uncovered is every OTHER upstream, any new
    wrapper, and the under-declarations this check's first run found.

    Three deliberate differences from A6:

    * DERIVED TASK SET, not a hand-written one. `derive_ffi_tasks` already finds exactly these
      tasks for A5, so a new wrapper's `napi build` is examined on day one even if it declares no
      inputs at all — which is precisely the bug a hand-written list could not detect.
    * CONTAINMENT, not strict equality. A6's strict equality is right for `fileGroups.upstreams`,
      a mechanical mirror of the closure where anything extra is waste. The wrapper globs are
      hand-written per task and legitimately mixed with non-closure inputs under `rs/crates/` —
      the SMA-433 parity vectors, and each binding's `package.json` / `pyproject.toml`. Strict
      equality would report those correct entries as violations.
    * BOTH BUCKETS, PER TASK. `Cargo.toml` is a path (`inputFiles`), `src/**/*` is a glob
      (`inputGlobs`), and a wrapper's `build` and `test` declare different sets — so a
      one-bucket read, or one that unions across a wrapper's tasks, passes the very mutations
      this check exists to catch.
    """
    a7 = []
    examined = {}
    for target in sorted(derive_ffi_tasks(projects)):
        pid, _, task = target.partition(":")
        proj = projects.get(pid)
        if proj is None or proj.get("language") == "rust":
            continue
        examined.setdefault(pid, []).append(task)

    # FLOOR first: if the derivation broke, every per-wrapper check below is vacuous. Rows are
    # `FLOOR:`-prefixed so a control can tell a floor failure from a per-wrapper one.
    for pid, required in sorted((floor or {}).items()):
        if pid not in projects:
            a7.append(f"FLOOR: {pid} is not in the graph at all")
            continue
        if pid not in examined:
            a7.append(
                f"FLOOR: {pid} has no task matched by an FFI marker, so A7 examines nothing for "
                f"it — either restore the invocation or update FFI_MARKERS"
            )
            continue
        derived = rust_closure(projects, pid)
        for missing in sorted(required - derived):
            a7.append(f"FLOOR: {pid}'s dependsOn closure no longer derives {missing}")

    for pid, tasks in sorted(examined.items()):
        want = set()
        for upstream in sorted(rust_closure(projects, pid)):
            src = projects[upstream]["source_dir"]
            want.add(f"{src}/src/**/*")
            want.add(f"{src}/Cargo.toml")
            # A build script is compiled by the wrapper's own `napi build`/`maturin` invocation,
            # so a change to it changes what the wrapper links. Only demanded when one exists.
            if root is not None and (root / src / "build.rs").is_file():
                want.add(f"{src}/build.rs")
        for task in sorted(tasks):
            files = (projects[pid].get("task_inputs") or {}).get(task)
            globs = (projects[pid].get("task_input_globs") or {}).get(task)
            if files is None or globs is None:
                a7.append(
                    f"{pid}:{task} reported no `inputFiles`/`inputGlobs` — moon's output shape "
                    f"changed, so this assertion cannot be evaluated (treated as a violation, "
                    f"never skipped)"
                )
                continue
            observed = set(files) | set(globs)
            for entry in sorted(want - observed):
                a7.append(f"{pid}:{task} inputs omit {entry}")
    return a7
```

- [ ] **Step 5: Fold A7 into `main()`'s findings list**

In `main()`, pass `root` through and append one entry after the A6 entry:

```python
        (check_wrapper_upstream_inputs(projects, root=root),
             "A py/ts wrapper's FFI task does not key on an upstream Rust crate's sources, so a\n"
             "    change there SELECTS NOTHING for that wrapper and the ADR-0005 parity replay\n"
             "    silently stops running on it (SMA-560).\n"
             "    Fix: add the missing entry to that task's `inputs` in the wrapper's own\n"
             "    moon.yml — `/<src_dir>/src/**/*` and `/<src_dir>/Cargo.toml` for every crate in\n"
             "    its TRANSITIVE dependsOn closure, plus `/<src_dir>/build.rs` where one exists.\n"
             "    Extra inputs beyond the closure are ALLOWED (this is containment, unlike A6).\n"
             "    A `FLOOR:` row means the check itself cannot be trusted — the wrapper is\n"
             "    missing, its closure derivation broke, or its task stopped matching an FFI\n"
             "    marker — fix that first, every other A7 row is meaningless until it passes."),
```

- [ ] **Step 6: Run A7 against the real graph and watch it red on the real defects**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py --self-test && echo "--- self-test OK ---"
python3 ci/affected-graph/cargo_moon_parity.py; echo "EXIT=$?"
```

Expected: self-test exits 0; the real run exits **1** with exactly these five rows:

```
paigasus-kernel-py:test inputs omit rs/crates/libs/paigasus-kernel/Cargo.toml
paigasus-kernel-ts:build inputs omit rs/crates/bindings/paigasus-node-bindings/build.rs
paigasus-kernel-ts:build inputs omit rs/crates/libs/paigasus-kernel/Cargo.toml
paigasus-kernel-ts:test inputs omit rs/crates/bindings/paigasus-node-bindings/build.rs
paigasus-kernel-ts:test inputs omit rs/crates/libs/paigasus-kernel/Cargo.toml
```

**This is the genuine failing test.** If any other row appears, stop and report — the spec's measurement was incomplete. Do not commit in this state.

- [ ] **Step 7: Fix the py wrapper**

In `py/packages/paigasus-kernel/moon.yml`, in `test.inputs`, directly after the
`/rs/crates/libs/paigasus-kernel/src/**/*` entry, add:

```yaml
      # The kernel's manifest, not just its sources (SMA-560). A `[features]` toggle there can
      # change what this wheel links without any src diff — the same hazard
      # repo:parity-corpus-drift already lists this file for. A6 demands the src+manifest PAIR of
      # every Rust crate; A7 now demands it here too.
      - '/rs/crates/libs/paigasus-kernel/Cargo.toml'
```

- [ ] **Step 8: Fix the ts wrapper**

In `ts/packages/paigasus-kernel/moon.yml`, add to **both** `build.inputs` and `test.inputs`,
after each task's `/rs/crates/libs/paigasus-kernel/src/**/*` entry:

```yaml
      # The kernel's manifest, not just its sources (SMA-560) — a `[features]` toggle there can
      # change what this task links with no src diff.
      - '/rs/crates/libs/paigasus-kernel/Cargo.toml'
      # The napi crate's build script. `napi build` COMPILES it (it is `napi_build::setup()`,
      # which emits the addon's link args), so a change to it changes this task's output — but
      # nothing keyed on it. This is where SMA-537 and SMA-560 meet.
      - '/rs/crates/bindings/paigasus-node-bindings/build.rs'
```

- [ ] **Step 9: Verify green**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py --self-test && \
python3 ci/affected-graph/cargo_moon_parity.py; echo "EXIT=$? (expected 0)"
```

- [ ] **Step 10: Prove A7 bites with a mutation nothing else can see**

The headline mutation is the **wasm** glob, not the kernel glob — `run.sh:343` already reds on
the kernel one, so a red there would prove nothing about A7.

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core
python3 - <<'PY'
import io
p = "ts/packages/paigasus-kernel/moon.yml"
s = io.open(p, encoding="utf-8").read()
io.open(p + ".bak", "w", encoding="utf-8").write(s)
# Remove the wasm sources glob from the `test` task only; `build` keeps its copy, which is what
# makes a union-across-tasks A7 pass this mutation and a per-task A7 catch it.
head, sep, tail = s.partition("  test:")
tail = tail.replace("      - '/rs/crates/bindings/paigasus-wasm/src/**/*'\n", "", 1)
io.open(p, "w", encoding="utf-8").write(head + sep + tail)
PY
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py; echo "EXIT=$? (expected 1)"
bash ci/affected-graph/run.sh > /tmp/a7probe.log 2>&1; echo "run.sh EXIT=$? (expected 1 — via A7 only)"
grep -c "paigasus-wasm" /tmp/a7probe.log
mv ts/packages/paigasus-kernel/moon.yml.bak ts/packages/paigasus-kernel/moon.yml
touch ts/packages/paigasus-kernel/moon.yml
python3 ci/affected-graph/cargo_moon_parity.py; echo "EXIT=$? (expected 0)"
```

Expected: `paigasus-kernel-ts:test inputs omit rs/crates/bindings/paigasus-wasm/src/**/*`, and
`run.sh` red **only** through the parity gate — no expected-set case reports it, which is what
makes this hole A7's to catch.

- [ ] **Step 11: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py py/packages/paigasus-kernel/moon.yml ts/packages/paigasus-kernel/moon.yml
git commit -F - <<'MSG'
ci(repo): assert the py/ts wrappers key on their upstream crates (SMA-560)

A6 holds every Rust crate to its dependsOn closure but iterates rust projects only,
so the py and ts wrappers, whose hand-written globs are the ADR-0005 cross-binding
guarantee, were asserted by nothing beyond one hand-written run.sh case covering the
kernel edge alone.

A7 derives its task set from the same FFI-marker matching A5 uses, so a new wrapper
is examined on day one even when it declares no inputs. It asserts containment
rather than A6's strict equality, because the wrapper globs legitimately carry
non-closure entries under rs/crates such as the parity corpus and each binding's
package manifest. It reads both input buckets per task, since the manifest and the
source glob land in different ones and the two ts tasks declare different sets.

Its first run found two live under-declarations, fixed here. No wrapper task keyed
on the kernel's manifest, so a features toggle could change linked behaviour with no
src diff, and neither ts task keyed on the napi build script even though napi build
compiles it.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
```

---

### Task 6: Full-graph verification

**Files:** none modified. This task produces evidence, not code.

**Interfaces:**
- Consumes: everything from Tasks 1-5.
- Produces: a verification record for the PR body.

- [ ] **Step 1: Confirm no `run.sh` expected set moved**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core
bash ci/affected-graph/run.sh; echo "EXIT=$? (expected 0)"
```

Expected: exit 0 with **no** `missing`/`unexpected` rows. The spec determined no set should move; if one does, **stop and report** — an assumption was wrong, and re-baselining silently would hide it.

- [ ] **Step 2: Confirm the target count is unchanged**

```bash
grep -c "repo:" .github/workflows/ci.yml > /dev/null
sed -n '214p' .github/workflows/ci.yml | grep -o ':[a-z0-9-]*' | wc -l
```

Expected: `27`. No new `repo:*` task was created, so this must not have changed.

- [ ] **Step 3: Run the full graph exactly as CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep --base origin/main --include-relations
echo "EXIT=$?"
```

Expected: exit 0. Note `.moon/tasks/rust.yml` changed, which schedules the **entire** Rust graph — expect a long run. If `paigasus-iam-rs:test` fails on Docker, check the daemon is up rather than assuming a regression; a Docker-less run yields exactly one red from `tests/docker_preflight.rs`.

- [ ] **Step 4: Record the evidence**

Capture for the PR body: the five A7 rows from Task 5 Step 6 (the real defects it found), the wasm mutation result from Step 10, the `run.sh` exit 0, and the full-graph exit 0.

- [ ] **Step 5: No commit**

This task adds no files. If Steps 1-3 all pass, the branch is ready for the local review stage.

---

## Self-Review

**Spec coverage:** D1 → Task 1; D2 → Task 1; D3 → Task 2; §4 D4 (derived task set) → Tasks 3 + 5; §4 D5 (containment, file-granular, both buckets, closure floor, floored floor, absent-entry rule) → Task 5 Steps 1/3/4; §4.3 (wrapper fixes) → Task 5 Steps 7-8; §4.4 (aggregate guard) → Task 4; §5 tests 1-10 → distributed across Tasks 1-6; §2.3 (no re-baseline) → Task 6 Step 1. No spec section is unimplemented.

**Open question resolved:** §7's single open question — restructure `main()` versus an `inspect.getsource` pin — is decided in Task 4 in favour of the restructure, *plus* a generic `check_*`-referenced self-test row, which is strictly stronger than the pin the spec offered as a fallback and covers a future A8.

**Type consistency:** `check_task_inputs(projects, crates, task, required)` is defined in Task 2 and called with that signature in Tasks 2 and 4. `derive_ffi_tasks(projects)` is defined in Task 3 and called in Tasks 3 and 5. `check_wrapper_upstream_inputs(projects, root=None, floor=REQUIRED_WRAPPER_CLOSURE)` is defined in Task 5 Step 4 and called with `root=root` in Step 5 and with `floor=` overrides throughout the self-test rows. `FMT_TASK_INPUTS` and `REQUIRED_WRAPPER_CLOSURE` are each defined once and referenced consistently.

**One ordering note:** Task 2 Step 5 adds a hand-maintained `a4_fmt` entry to the old guard/report pairing that Task 4 then replaces. That is deliberate — it keeps each commit independently green rather than leaving `main()` broken between two tasks.
