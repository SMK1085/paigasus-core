# SMA-546 — Workspace inputs for the FFI build tasks: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a `rs/Cargo.lock` / `rs/Cargo.toml` / `rs/rust-toolchain.toml` / `.prototools` change
schedule the three Moon tasks that actually link the FFI cdylibs and compile `wasm32`, and guard that
wiring so it cannot silently regress.

**Architecture:** Four workspace-level files become `inputs` of
`paigasus-kernel-ts:{build,test}` and `paigasus-kernel-py:test`; the same three builds gain
`--locked` so the artifact provably comes from the resolution the PR ships. Two guard layers follow
the repo's existing split — a behavioural strict-equality case in `ci/affected-graph/run.sh`, and a
new generic assertion **A5** in `ci/affected-graph/cargo_moon_parity.py` that derives its own target
list from each task's resolved invocation but is anchored by a required floor so it cannot pass
vacuously.

**Tech Stack:** Moon 2.3.2 (task graph, `moon query projects` / `moon query tasks`), Bash
(`ci/affected-graph/run.sh`), Python 3 stdlib only (`cargo_moon_parity.py` — `tomllib`, `json`,
`subprocess`; **no third-party imports**, the gate runs under `toolchain: 'system'`), wasm-pack
0.15.0, `@napi-rs/cli` 3, maturin via uv 0.11.16.

**Spec:** `docs/superpowers/specs/2026-08-17-sma-546-ffi-workspace-inputs-design.md`

## Global Constraints

- Every source file opens with an SPDX header: `// SPDX-License-Identifier: Apache-2.0` (`#` for
  Python/Bash). All files touched here already have one — do not add a second.
- `cargo_moon_parity.py` must **never parse `moon.yml`** and must **never shell out to cargo**. It
  reads Moon's own resolved output only. `repo:affected-smoke` is `toolchain: 'system'`.
- The exact workspace-relative paths Moon **resolves** (no leading slash) are
  `rs/Cargo.lock`, `rs/Cargo.toml`, `rs/rust-toolchain.toml`, `.prototools`. The YAML that declares
  them **does** carry a leading slash: `/rs/Cargo.lock` etc. Both forms appear in this plan; do not
  mix them up.
- Prefix every shell command with
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` — the proto-managed CLIs (`moon`, `uv`,
  `wasm-pack`, `nextest`) are not on the default PATH, and shims must come first so the
  repo-pinned versions win.
- Run everything from the worktree root
  `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-546`. Do **not** `cd` to the
  main checkout.
- Commit messages: conventional commit with a workspace scope, subject **starting lowercase**,
  header ≤100 chars. **No line in the body may start with `word:`** and no bare `#NNN` — commitlint
  parses either as a trailer and fails `footer-leading-blank`. Keep one contiguous footer.
- An absent key from Moon's query output is always a **violation or infra error, never a skip**.
  "Moon told us nothing" must not be reported as "the graph is fine".

---

### Task 1: A5 — the generic FFI-inputs assertion (guard first; ends RED against the real graph)

This task adds the assertion **before** the inputs exist, so the guard's red *is* the failing test
for Task 2. Expect `ci/affected-graph/run.sh` to fail at the end of this task. That is correct.

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` (constants near `:56`; `check_lint_inputs` ends
  `:149`; `moon_projects` `:152-185`; `self_test` `:218-352`; `main` `:355-398`)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `check_ffi_inputs(projects, required=FFI_TASK_INPUTS, floor=REQUIRED_FFI_TASKS) -> list[str]`
  returning A5 violation strings; the constants `FFI_MARKERS`, `REQUIRED_FFI_TASKS`,
  `FFI_TASK_INPUTS`; and a new `"invocations"` key inside each project dict returned by
  `moon_projects()`, mapping task name → the joined `command + args + script` string (or `None`
  when Moon reported neither a `command` nor a `script`).

- [ ] **Step 1: Add the A5 constants**

In `ci/affected-graph/cargo_moon_parity.py`, immediately after the existing
`WORKSPACE_LINT_INPUTS` definition (currently ending line 56), add:

```python
# SMA-546 — A5. The tasks that COMPILE the FFI cdylibs live in the ts/py stacks, so A4's
# per-crate loop cannot reach them: `moon query projects` lists them under their own project ids,
# not under any Rust crate. They must key on the same workspace files as `lint`, plus `.prototools`
# — which pins `wasm-pack` and is therefore the OTHER half of the rs/Cargo.toml:90-97 invariant
# ("the pinned wasm-pack must support that 0.2.z — bump the two together").
FFI_TASK_INPUTS = WORKSPACE_LINT_INPUTS + (".prototools",)

# Substrings that mean "this task shells out to a Rust build". Matched against the task's resolved
# `command` + `args` + `script` joined — NOT `command` alone: measured on moon 2.3.2, a
# command-form task reports command='cargo' with the verb in args (paigasus-kernel-rs:lint ->
# args=['clippy', '--locked', ...], script=None), so a `command: 'napi'` + `args: ['build', ...]`
# task would be invisible to a command-only scan.
#
# `maturin` is FORWARD-LOOKING and matches nothing today: the string appears only in
# py/packages/paigasus-kernel/moon.yml COMMENTS, and the resolved script is
# `uv sync --reinstall-package paigasus-py-bindings`. It is kept so a future direct maturin
# invocation is covered on day one. Do not mistake it for measured coverage.
FFI_MARKERS = ("napi build", "wasm-pack", "maturin", "--reinstall-package")

# The floor. A5's derived set is its strength (a fourth FFI task is covered the day it is added —
# SMA-524's "a MISSING case is how the bug survived" lesson) and also its weakness: a derived set
# that shrinks to EMPTY asserts nothing while still printing PASS. Moving an invocation behind a
# package.json script, `--reinstall-package` becoming `--refresh-package`, or a moon upgrade
# renaming the `script` key would each do that silently. A4's "absent inputFiles is a violation"
# rule does NOT protect against it — when nothing matches, inputFiles is never consulted.
# So: every task named here MUST be in the derived set, or A5 fails.
REQUIRED_FFI_TASKS = (
    "paigasus-kernel-py:test",
    "paigasus-kernel-ts:build",
    "paigasus-kernel-ts:test",
)
```

- [ ] **Step 2: Write the failing self-test rows**

In `self_test()`, immediately before the `# A malformed Cargo.toml must surface as INFRA` block
(currently line 330), insert:

```python
    # A5 (SMA-546): the FFI build tasks must key on the workspace files. Fixture mirrors the real
    # shape — a ts project whose `build` shells out to napi + wasm-pack.
    ffi_ok = {
        "paigasus-kernel-ts": {
            "source_dir": "ts/packages/paigasus-kernel",
            "deps": {},
            "tasks": {"build": [], "test": []},
            "task_inputs": {
                "build": list(FFI_TASK_INPUTS),
                "test": list(FFI_TASK_INPUTS),
            },
            "invocations": {
                "build": "touch ... && napi build --platform && wasm-pack build .",
                "test": "touch ... && napi build --platform && wasm-pack build .",
            },
        },
        "paigasus-kernel-py": {
            "source_dir": "py/packages/paigasus-kernel",
            "deps": {},
            "tasks": {"test": []},
            "task_inputs": {"test": list(FFI_TASK_INPUTS)},
            "invocations": {"test": "uv sync --reinstall-package paigasus-py-bindings"},
        },
        "unrelated-ts": {
            "source_dir": "ts/packages/unrelated",
            "deps": {},
            "tasks": {"build": []},
            "task_inputs": {"build": []},
            "invocations": {"build": "tsc --noEmit"},
        },
    }
    ffi_floor = ("paigasus-kernel-py:test", "paigasus-kernel-ts:build", "paigasus-kernel-ts:test")

    if check_ffi_inputs(ffi_ok, floor=ffi_floor):
        failures.append("A5 reported violations on the clean fixture")

    # Fires when a matched task omits one of the required files.
    broken = json.loads(json.dumps(ffi_ok))
    broken["paigasus-kernel-ts"]["task_inputs"]["build"] = [
        "rs/Cargo.lock", "rs/Cargo.toml", "rs/rust-toolchain.toml"
    ]
    rows = check_ffi_inputs(broken, floor=ffi_floor)
    if not rows:
        failures.append("A5 did not fire on a missing FFI workspace input")
    elif not any(".prototools" in row for row in rows):
        failures.append("A5 fired but did not name the missing file")

    # THE ANTI-VACUITY ROW. Neuter the marker match (as a package.json indirection or a renamed
    # uv flag would) and A5's derived set empties. Without the floor this reports PASS while
    # asserting nothing — the exact silent-degradation mode the floor exists to stop.
    broken = json.loads(json.dumps(ffi_ok))
    for task in ("build", "test"):
        broken["paigasus-kernel-ts"]["invocations"][task] = "pnpm run build:native"
    rows = check_ffi_inputs(broken, floor=ffi_floor)
    if not any("not matched by any FFI marker" in row for row in rows):
        failures.append("A5 did not fire when a required FFI task stopped matching the markers")

    # A task exposing NEITHER a command NOR a script is moon telling us nothing — infra (rc 2),
    # not an assertion failure. Mirrors A4's absent-inputFiles rule.
    broken = json.loads(json.dumps(ffi_ok))
    broken["paigasus-kernel-ts"]["invocations"]["build"] = None
    try:
        check_ffi_inputs(broken, floor=ffi_floor)
    except MoonOutputError:
        pass
    else:
        failures.append("A5 did not raise infra on a task with no command and no script")

    # A matched task that is NOT in the floor is still asserted — this is the derived half.
    broken = json.loads(json.dumps(ffi_ok))
    broken["unrelated-ts"]["invocations"]["build"] = "wasm-pack build ."
    if not any("unrelated-ts:build" in row for row in check_ffi_inputs(broken, floor=ffi_floor)):
        failures.append("A5 did not assert a newly-matched task outside the floor")
```

- [ ] **Step 3: Run the self-test to verify it fails**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py --self-test
```
Expected: **FAIL** with `NameError: name 'check_ffi_inputs' is not defined` (or
`MoonOutputError` undefined). This confirms the new rows actually execute rather than being
skipped.

- [ ] **Step 4: Add the infra exception type**

Directly above the existing `INFRA_ERRORS` tuple (currently line 32), add:

```python
class MoonOutputError(RuntimeError):
    """Moon's query output did not have the shape this gate requires.

    Raised — never returned as a violation row — when moon reports a task with neither a `command`
    nor a `script`. That is "moon told us nothing", which must abort as an infrastructure error
    (rc 2) rather than be folded into an assertion failure, exactly as A4 treats an absent
    `inputFiles` key. A moon upgrade that reshapes the task object must fail loudly, not quietly
    stop asserting.
    """
```

Then add `MoonOutputError` to the `INFRA_ERRORS` tuple so `main()` maps it to rc 2:

```python
INFRA_ERRORS = (
    subprocess.CalledProcessError,
    json.JSONDecodeError,
    tomllib.TOMLDecodeError,
    OSError,
    MoonOutputError,
)
```

- [ ] **Step 5: Implement `check_ffi_inputs`**

Insert immediately after `check_lint_inputs` (which currently ends at line 149):

```python
def check_ffi_inputs(projects, required=FFI_TASK_INPUTS, floor=REQUIRED_FFI_TASKS):
    """Return the A5 violation list: FFI-compiling tasks that do not key on the workspace files.

    Two halves, and both are load-bearing:

    * DERIVED — any task whose resolved invocation matches an FFI marker must declare `required`.
      This is what covers a future fourth binding task on the day it is added.
    * FLOOR — every task in `floor` must appear in the derived set. Without this a derivation that
      silently stops matching (a renamed flag, an invocation moved behind a wrapper script, a moon
      upgrade dropping `script`) degrades to an empty set and a vacuous PASS.

    Raises MoonOutputError if a task exposes neither a command nor a script.
    """
    matched, a5 = set(), []
    for pid in sorted(projects):
        invocations = projects[pid].get("invocations") or {}
        declared = projects[pid].get("task_inputs") or {}
        for name in sorted(invocations):
            blob = invocations[name]
            if blob is None:
                raise MoonOutputError(
                    f"{pid}:{name} reported neither a `command` nor a `script` — moon's output "
                    f"shape changed, so A5 cannot be evaluated"
                )
            if not any(marker in blob for marker in FFI_MARKERS):
                continue
            target = f"{pid}:{name}"
            matched.add(target)
            resolved = declared.get(name)
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

- [ ] **Step 6: Carry the invocation text through `moon_projects()`**

In `moon_projects()`, inside the `for name, task in ...` loop, alongside the existing
`task_inputs[name] = ...` line, add:

```python
            # A5 (SMA-546): the text A5 marker-matches against. Joined from all three fields
            # because moon splits an invocation differently per task form — command-form puts the
            # verb in `args` (command='cargo', args=['clippy', ...]), script-form puts it in
            # `script` (command='touch', script='touch ... && napi build ...'). None when moon
            # reported neither, which check_ffi_inputs escalates to an infra error.
            parts = [task.get("command") or "", task.get("script") or ""]
            parts += [str(a) for a in (task.get("args") or [])]
            joined = " ".join(p for p in parts if p)
            invocations[name] = joined or None
```

Declare `invocations = {}` beside the existing `tasks = {}` / `task_inputs = {}`, and add
`"invocations": invocations,` to the returned per-project dict.

- [ ] **Step 7: Wire A5 into `main()`**

Replace the `a4 = check_lint_inputs(projects, crates)` line and the condition below it:

```python
    a4 = check_lint_inputs(projects, crates)
    a5 = check_ffi_inputs(projects)
    if not (a1 or a2 or a3 or a4 or a5):
        print(
            f"PASS  {'cargo-moon-parity':<18} -> "
            f"{len(crates)} crates: every Cargo dep has a Moon edge that schedules its build, "
            f"every lint keys on the workspace files, and every FFI build task does too"
        )
        return 0
```

Add a fifth entry to the reporting loop's tuple, after the `a4` entry:

```python
        (a5, "an FFI build task does not key on the workspace-level files, so a dependency bump\n"
             "    replays a CACHED artifact built from a different resolution — and clippy cannot\n"
             "    cover it, because it never links a cdylib and never targets wasm32 (SMA-546).\n"
             "    Fix: add /rs/Cargo.lock, /rs/Cargo.toml, /rs/rust-toolchain.toml and\n"
             "    /.prototools to that task's `inputs`. A `not matched by any FFI marker` row\n"
             "    means the opposite — the task stopped looking like a Rust build to A5; either\n"
             "    restore the invocation or update FFI_MARKERS."),
```

- [ ] **Step 8: Update the self-test's closing line**

Change the success print (currently line 351) from `all four assertions` to:

```python
    print("  OK   [parity] all five assertions fire on synthetic violations")
```

- [ ] **Step 9: Run the self-test to verify it passes**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py --self-test
```
Expected: **PASS**, exit 0, printing `OK   [parity] all five assertions fire on synthetic
violations`. No `FAIL` lines.

- [ ] **Step 10: Confirm A5 reds against the real graph — this is Task 2's failing test**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py
```
Expected: **FAIL**, exit 1, with three rows naming
`paigasus-kernel-py:test`, `paigasus-kernel-ts:build` and `paigasus-kernel-ts:test`, each omitting
`rs/Cargo.lock, rs/Cargo.toml, rs/rust-toolchain.toml, .prototools`. No
`not matched by any FFI marker` row — the markers must already match all three.

If a `not matched` row appears, the marker matching is wrong; fix it before continuing (the three
tasks' scripts genuinely contain `napi build`, `wasm-pack` and `--reinstall-package`).

- [ ] **Step 11: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -F - <<'EOF'
test(repo): add A5, the generic FFI workspace-inputs assertion (SMA-546)

A4 covers every Rust crate's lint, but the tasks that actually link the
cdylibs and compile wasm32 live in the ts/py stacks, where its per-crate loop
cannot reach them. A5 derives its targets from each task's resolved
invocation, anchored by a required floor so a derivation that stops matching
cannot degrade to a vacuous pass.

Reds against the real graph until the inputs land, which is the point.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

### Task 2: Declare the inputs and make the FFI builds `--locked`

Turns Task 1's A5 red green, and reds the behavioural case — which is Task 3's failing test.

**Files:**
- Modify: `ts/packages/paigasus-kernel/moon.yml` (the `build` task's `script` + `inputs`; the
  `test` task's `script` + `inputs`)
- Modify: `py/packages/paigasus-kernel/moon.yml` (the `test` task's `script` + `inputs`)

**Interfaces:**
- Consumes: A5 from Task 1 (its red is this task's failing test).
- Produces: the four resolved inputs `rs/Cargo.lock`, `rs/Cargo.toml`, `rs/rust-toolchain.toml`,
  `.prototools` on all three tasks; Task 3 reads the resulting affected-task set.

- [ ] **Step 1: Add the inputs to the ts `build` task**

In `ts/packages/paigasus-kernel/moon.yml`, in the **`build`** task's `inputs:` list, append after
the existing parity-corpus entry:

```yaml
      # Workspace-level Rust files (SMA-546). This task LINKS two cdylibs and compiles
      # wasm32-unknown-unknown; `cargo clippy` does neither, so SMA-534's thirteen lints cannot
      # cover a dependency bump here. Without these the task replays a CACHED artifact built from
      # a different resolution — a cache-correctness bug independent of scheduling. Moon has no
      # hash-only input, so declaring them also makes a lockfile touch SCHEDULE this task, which
      # is the coverage half of SMA-546.
      #   /rs/Cargo.lock          the resolved versions the PR ships
      #   /rs/Cargo.toml          [workspace.dependencies] + feature flips, which never reach the lock
      #   /rs/rust-toolchain.toml the compiler wasm-pack selects (it runs from INSIDE the crate dir
      #                           precisely so this file's 1.95.0 override wins)
      #   /.prototools            pins wasm-pack itself — the OTHER half of the rs/Cargo.toml:90-97
      #                           invariant, which is bidirectional ("bump the two together")
      - '/rs/Cargo.lock'
      - '/rs/Cargo.toml'
      - '/rs/rust-toolchain.toml'
      - '/.prototools'
```

- [ ] **Step 2: Add the same four inputs to the ts `test` task**

Append the identical four entries to the **`test`** task's `inputs:` list, with this shorter
comment (the long rationale lives on `build`):

```yaml
      # Workspace-level Rust files (SMA-546) — same rationale as `build` above: this task rebuilds
      # both cdylibs before asserting, so its cached vitest RESULT is as resolution-dependent as
      # build's artifact.
      - '/rs/Cargo.lock'
      - '/rs/Cargo.toml'
      - '/rs/rust-toolchain.toml'
      - '/.prototools'
```

- [ ] **Step 3: Add `--locked` to both ts scripts**

In the **`build`** task's `script`, change the two build invocations so cargo cannot silently
re-resolve. The `--` separator forwards the flag to cargo; verified on this worktree by sending a
bogus flag through each tool and observing `cargo build`'s own usage error.

- `pnpm exec napi build --platform --cwd ../../../rs/crates/bindings/paigasus-node-bindings`
  becomes
  `pnpm exec napi build --platform --cwd ../../../rs/crates/bindings/paigasus-node-bindings -- --locked`
- `wasm-pack build . --target bundler --release --no-pack --out-dir .wasmpack-out --out-name paigasus_wasm`
  becomes
  `wasm-pack build . --target bundler --release --no-pack --out-dir .wasmpack-out --out-name paigasus_wasm -- --locked`

Apply the same two changes in the **`test`** task's `script`, where the wasm out-dir is
`.wasmpack-test-out` instead of `.wasmpack-out`. **Do not change the out-dir names** — they are
deliberately different so the concurrent build and test wasm-pack runs do not race (SMA-427).

Add this comment above the `build` task's `script:` line:

```yaml
  # `--locked` on both cargo shell-outs (SMA-546): without it cargo silently re-resolves and
  # REWRITES rs/Cargo.lock — which is now a declared input of this task, so the hash Moon recorded
  # before execution would describe a tree that no longer exists, and the rewrite would race
  # repo:deny / repo:wasm-getrandom-free / repo:nats-permissions reading the same file in the same
  # graph. It also makes the artifact provably come from the resolution the PR ships, which is the
  # guarantee SMA-534 established for `lint` and the reason its clippy is `--locked` too.
```

- [ ] **Step 4: Add the inputs and `--locked` to the py `test` task**

In `py/packages/paigasus-kernel/moon.yml`, append to the **`test`** task's `inputs:`:

```yaml
      # Workspace-level Rust files (SMA-546) — see ts/packages/paigasus-kernel/moon.yml's `build`
      # for the full rationale. `--reinstall-package` forces maturin to relink the cdylib, so this
      # task's result depends on the resolution these files pin.
      - '/rs/Cargo.lock'
      - '/rs/Cargo.toml'
      - '/rs/rust-toolchain.toml'
      - '/.prototools'
```

Change the `script` so maturin's cargo runs `--locked`. maturin takes it via a PEP 517 env var,
not a trailing `--` (verified: a bogus value surfaces as
`command ['maturin', 'pep517', 'build-wheel', ..., '--this-flag-does-not-exist'] returned non-zero exit status 2`):

`uv sync --reinstall-package paigasus-py-bindings` becomes
`MATURIN_PEP517_ARGS=--locked uv sync --reinstall-package paigasus-py-bindings`

with this comment above the `script:` line:

```yaml
  # MATURIN_PEP517_ARGS=--locked (SMA-546): the py path reaches cargo through uv's PEP 517 build
  # backend, so it takes no trailing `--` the way wasm-pack and napi do. Same rationale as the ts
  # side — build from the shipped resolution, and never rewrite a file that is now a declared input.
```

- [ ] **Step 5: Verify A5 now passes against the real graph**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py
```
Expected: **PASS**, exit 0, ending
`every lint keys on the workspace files, and every FFI build task does too`.

If it still reds, print what Moon actually resolved for one task and compare against the four
expected paths (note: **no leading slash** in resolved output):
```bash
moon query projects | python3 -c "import sys,json;d=json.load(sys.stdin);print([p['tasks']['build']['inputFiles'].keys() for p in d['projects'] if p['id']=='paigasus-kernel-ts'])"
```

- [ ] **Step 6: Verify the three tasks still actually run**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-kernel-ts:build paigasus-kernel-ts:test paigasus-kernel-py:test
```
Expected: all tasks succeed. This is what proves `--locked` did not break the three builds —
`napi build`, `wasm-pack` and maturin each reach cargo with the flag.

- [ ] **Step 7: Confirm the behavioural case now reds — Task 3's failing test**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/affected-graph/run.sh
```
Expected: **FAIL** on `lockfile->all-lint` with exactly three `unexpected` rows:
`paigasus-kernel-py:test`, `paigasus-kernel-ts:build`, `paigasus-kernel-ts:test`. Every other case
still passes.

- [ ] **Step 8: Commit**

```bash
git add ts/packages/paigasus-kernel/moon.yml py/packages/paigasus-kernel/moon.yml
git commit -F - <<'EOF'
fix(repo): key the FFI build tasks on the workspace files and lock their builds (SMA-546)

These three tasks compile Rust but kept no cache key on the resolution that
Rust is compiled against, so a dependency bump replayed an artifact built
from a different graph. Moon has no hash-only input, so the same four
declarations also make a lockfile touch schedule them, which is what covers
the cdylib link and the wasm32 target that clippy structurally cannot.

The builds now pass --locked, so cargo can neither re-resolve behind the
gate nor rewrite a file that is now a declared input of the running task.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

### Task 3: Re-baseline the behavioural case from measured output

**Files:**
- Modify: `ci/affected-graph/run.sh` (the `lockfile->all-lint` comment block `:230-245` and its
  `run_task_case` invocation `:246-247`)

**Interfaces:**
- Consumes: the inputs from Task 2.
- Produces: a green `ci/affected-graph/run.sh` suite.

- [ ] **Step 1: Measure the real affected-task set — do not hand-write the CSV**

The spec predicts exactly three new rows, but Moon's JS toolchain can synthesise implicit project
edges from `package.json`, so this is measured rather than reasoned about:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
printf 'rs/Cargo.lock\n' | moon query tasks --affected --downstream deep \
  | python3 -c '
import sys, json
d = json.load(sys.stdin)
out = []
for pid, tasks in (d.get("tasks") or {}).items():
    for name in tasks:
        if name in ("build", "test", "lint"):
            out.append(f"{pid}:{name}")
print(",".join(sorted(out)))'
```
Expected: sixteen entries — the thirteen `*:lint` rows plus `paigasus-kernel-py:test`,
`paigasus-kernel-ts:build`, `paigasus-kernel-ts:test`. **Use this exact output as the CSV.** If it
differs from sixteen, stop and reconcile against the spec before editing.

- [ ] **Step 2: Replace the case's comment block**

Replace the existing comment above `run_task_case "lockfile->all-lint"` (the block starting
`# A workspace-level change must schedule EVERY crate's lint.`) with:

```bash
  # A workspace-level change must schedule EVERY crate's lint, AND the three tasks that compile the
  # FFI cdylibs. `rs/` has no Moon project, so these files belong to `repo`; affectedness reaches
  # both sets through task INPUTS, not through `dependsOn` — which is why no project case above
  # changes. Before SMA-534 a Cargo.lock-only touch (i.e. every Dependabot Cargo PR) scheduled no
  # crate task at all, so a dependency bump that tripped `-D warnings` merged green and redded main
  # later.
  #
  # The three build/test rows are SMA-546. `cargo clippy` emits metadata and never LINKS, and runs
  # on the host target only — so the thirteen lints cannot cover the three `crate-type = ["cdylib"]`
  # bindings, for which linking IS the failure mode, nor wasm32-unknown-unknown, which they never
  # compile. paigasus-kernel-ts:{build,test} and paigasus-kernel-py:test are the tasks that do.
  #
  # The case name still says `all-lint`. It is now a slight misnomer, kept deliberately: it is
  # referenced by CLAUDE.md and ci/affected-graph/README.md, and renaming it would break those
  # greps for no functional gain.
  #
  # SAFETY OF THE NAME FILTER: `assert_task_case` matches the task NAMES build/test/lint across
  # every project, so a same-named task elsewhere would enter this set. One premise makes that safe
  # and it must be stated narrowly: `repo` declares no task named build/test/lint (verify:
  # `moon query tasks`). The py/ts side is no longer a premise but an ASSERTION — the three tasks
  # that key on `rs/Cargo.lock` are listed below, so a fourth one appearing shows up here as an
  # `unexpected` row rather than passing silently. Add it if intended; do not widen the filter.
  #
  # The py CONFIGURATION ROOT's tasks (py:test/lint/fmt/typecheck) are deliberately absent. They do
  # not key on these files: `uv run` alone serves a CACHED wheel and cannot observe a Rust change
  # (measured for SMA-546 — a kernel edit that made `--reinstall-package` fail 67 tests left plain
  # `uv run pytest` reporting 124 passed), so giving them these inputs would buy cost with no
  # coverage.
```

- [ ] **Step 3: Replace the expected CSV**

Replace the `run_task_case "lockfile->all-lint" ...` invocation's CSV with the measured output from
Step 1, keeping the existing two-line continuation style:

```bash
  run_task_case "lockfile->all-lint" "rs/Cargo.lock" \
    "paigasus-gateway-rs:lint,paigasus-iam-core-rs:lint,paigasus-iam-rs:lint,paigasus-kernel-parity-rs:lint,paigasus-kernel-py:test,paigasus-kernel-rs:lint,paigasus-kernel-ts:build,paigasus-kernel-ts:test,paigasus-logging-rs:lint,paigasus-node-bindings-rs:lint,paigasus-observability-rs:lint,paigasus-proto-derive-rs:lint,paigasus-proto-rs:lint,paigasus-py-bindings-rs:lint,paigasus-service-info-rs:lint,paigasus-wasm-rs:lint"
```

- [ ] **Step 4: Run the suite to verify it passes**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/affected-graph/run.sh
```
Expected: **PASS** on all twelve assertions, ending `== affected-graph cascade intact ==`, with
`lockfile->all-lint` listing sixteen targets.

- [ ] **Step 5: Run the negative control**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/affected-graph/run.sh --negative-control
```
Expected: **PASS**, exit 0, ending `negative-control OK: harness reported red on all wrong
expectations`, and including `OK   [parity] all five assertions fire on synthetic violations`.

- [ ] **Step 6: Commit**

```bash
git add ci/affected-graph/run.sh
git commit -F - <<'EOF'
test(repo): re-baseline the lockfile case for the three FFI build tasks (SMA-546)

The case's own comment predicted these rows and told the next reader to add
them here. The set is taken from measured moon query output rather than
hand-written, and the name is kept because CLAUDE.md and the README grep for
it.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

### Task 4: Documentation

**Files:**
- Modify: `ci/affected-graph/README.md:44-49` (case description), `:55-59` (add an A5 bullet after
  A4's), `:85-88` (maintenance note), `:90-95` (moon-version re-grounding paragraph)
- Modify: `CLAUDE.md:69-73` (the new-crate re-baselining gotcha)

**Interfaces:**
- Consumes: the case name and A5 semantics from Tasks 1-3. Produces nothing consumed later.

- [ ] **Step 1: Correct the case description in the README**

In `ci/affected-graph/README.md`, the `lockfile->all-lint` bullet currently claims the touch
"schedules **every** crate's `lint`" and ends by saying the comment "names the three py/ts tasks
that are one input line away from entering its observed set". Both are now stale. Replace that
bullet's first sentence and last sentence so it reads:

```markdown
- **`lockfile->all-lint`** asserts that a `rs/Cargo.lock` touch schedules **every** crate's `lint`
  **and** the three tasks that compile the FFI cdylibs (`paigasus-kernel-ts:{build,test}`,
  `paigasus-kernel-py:test`). `rs/` has no Moon project, so the workspace files belong to `repo`
  and affectedness reaches both sets through task **inputs**, not through `dependsOn` — which is
  why no *project* case changes and this one is needed at all. Before SMA-534 that touch scheduled
  no crate task whatsoever, so every Dependabot Cargo PR was unlinted; before SMA-546 it still
  scheduled nothing that LINKS a cdylib or compiles `wasm32`, which clippy never does. The name is
  a deliberate misnomer — renaming it would break the `CLAUDE.md` procedure that greps for it.
```

- [ ] **Step 2: Add the A5 bullet**

Immediately after the existing **A4** bullet (currently ending line 59), add:

```markdown
- **A5** (in `cargo_moon_parity.py`) is A4's cross-stack twin (SMA-546): the tasks that COMPILE the
  FFI cdylibs live in the ts/py stacks, where A4's per-crate loop cannot reach them. A5 **derives**
  its targets — any task whose resolved `command` + `args` + `script` mentions `napi build`,
  `wasm-pack`, `maturin` or `--reinstall-package` — and requires each to declare `rs/Cargo.lock`,
  `rs/Cargo.toml`, `rs/rust-toolchain.toml` and `.prototools`. Deriving covers a future fourth
  binding task on day one; a `REQUIRED_FFI_TASKS` **floor** stops the derivation degrading to a
  vacuous PASS if a task ever stops matching the markers. A task with neither a `command` nor a
  `script` aborts as infra (rc 2), never as a silent skip.
```

- [ ] **Step 3: Update the maintenance note**

The `lockfile->all-lint` maintenance bullet (currently `:85-88`) says A4 needs no update when a
crate is added. Append one sentence so the FFI rows are not mistaken for per-crate rows:

```markdown
  A4 needs no update in that situation: the new crate inherits `lint`'s inputs from
  `.moon/tasks/rust.yml`, which is the point of declaring them there. The case's three
  `build`/`test` rows are the FFI tasks (SMA-546) and are unaffected by adding a Rust crate; A5
  covers them, and likewise needs no update unless a *new* FFI-compiling task appears.
```

- [ ] **Step 4: Record A5's moon-version dependency**

The re-grounding paragraph (currently `:90-95`) names only `inputFiles`. Extend it:

```markdown
The expected sets are a snapshot of `moon query --affected --downstream deep` output at the
**pinned moon version** (currently 2.3.2). A4 additionally depends on `moon query projects`
emitting per-task `inputFiles` as a path-keyed object, and A5 on it emitting per-task `command`,
`args` and `script`. A moon upgrade that changes either — even benignly — will fail the guard, so
re-grounding is a known step of any moon bump. Both treat a missing key as a violation or an
infrastructure error rather than skipping, precisely so such a change cannot turn into a silent
pass.
```

- [ ] **Step 5: Update the CLAUDE.md gotcha**

In `CLAUDE.md`, the bullet beginning "A new Rust crate reds `:affected-smoke`" ends with the A4
sentence. Append after it:

```markdown
  That case now also carries three non-lint rows — `paigasus-kernel-ts:{build,test}` and
  `paigasus-kernel-py:test`, the tasks that link the cdylibs and compile `wasm32` (SMA-546) — so
  keep them when re-baselining; a new Rust crate does not change them.
```

- [ ] **Step 6: Verify no stale references remain**

Run:
```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-546
grep -rn "lockfile->all-lint" . --exclude-dir=.git --exclude-dir=node_modules \
  --exclude-dir=target --exclude-dir=.venv | grep -v "docs/superpowers/plans/2026-08-16"
```
Expected: hits only in `CLAUDE.md`, `ci/affected-graph/README.md`, `ci/affected-graph/run.sh` and
this plan/spec — and every prose hit now describes a set containing both lints and FFI rows. The
SMA-534 plan document is a historical record and must **not** be edited.

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/README.md CLAUDE.md
git commit -F - <<'EOF'
docs(repo): describe A5 and the widened lockfile case (SMA-546)

The case no longer schedules only lints, and A5 adds a second moon-version
dependency alongside A4's inputFiles — the per-task command, args and script
fields. Both are recorded where a contributor re-baselining the guard or
bumping moon will actually look.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

---

### Task 5: End-to-end verification — prove the wiring bites through `moon ci`

Nothing here changes committed source. It produces evidence for the PR body. Every experimental
edit is reverted, and the task ends with the working tree clean.

**Files:**
- Temporary only: `rs/Cargo.lock` (edited then restored via `git checkout`)

**Interfaces:**
- Consumes: everything from Tasks 1-4.

- [ ] **Step 1: V2 — confirm the project graph is untouched**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
printf 'rs/Cargo.lock\n' | moon query projects --affected --downstream deep \
  | python3 -c 'import sys,json;print(sorted(p["id"] for p in json.load(sys.stdin)["projects"]))'
```
Expected: `['repo']` alone. This is the load-bearing premise of the whole design — task inputs do
not create project edges — so it is observed, not assumed.

- [ ] **Step 2: V4 — prove a lockfile-only change SCHEDULES the FFI tasks through `moon ci`**

`moon query tasks` is a proxy; this proves the real scheduler agrees. Make a lockfile-only commit,
then read Moon's own CI report to distinguish **ran** from **replayed**:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-546
# A no-op whitespace touch is enough — the file's CONTENT hash is what Moon keys on.
printf '\n' >> rs/Cargo.lock
git add rs/Cargo.lock && git commit -m "chore(repo): temporary lockfile touch for SMA-546 verification"
moon ci :build :test :lint --base HEAD~1 --include-relations
python3 -c '
import json
d = json.load(open(".moon/cache/ciReport.json"))
for a in d["actions"]:
    label = a.get("label", "")
    if any(k in label for k in ("paigasus-kernel-ts", "paigasus-kernel-py", "paigasus-wasm", "paigasus-node-bindings")):
        print(f"{a.get(\"status\"):12} {label}")'
```
Expected: `paigasus-kernel-ts:build`, `paigasus-kernel-ts:test` and `paigasus-kernel-py:test`
appear with a *ran* status (`passed`), **not** `cached`/`skipped`. Record the output verbatim — it
is the core evidence for the PR body.

- [ ] **Step 3: Undo the temporary commit**

```bash
git reset --hard HEAD~1
git status --short   # expected: empty
```

- [ ] **Step 4: V5 — prove a BAD lockfile reds the FFI build**

Downgrading `wasm-bindgen` is **not** available in this workspace: `js-sys` pins it with
`= 0.2.126` and `reqwest`'s tree pins `js-sys`, so `cargo update --precise` refuses (verified while
planning). Use the deterministic lockfile corruption instead — it is still a lockfile-only change
to the `wasm-bindgen` entry, and it makes cargo refuse to build:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-546
python3 - <<'PY'
import pathlib, re
p = pathlib.Path("rs/Cargo.lock")
s = p.read_text()
# Corrupt ONLY the wasm-bindgen package's checksum.
block = re.search(r'\[\[package\]\]\nname = "wasm-bindgen"\n.*?\n\n', s, re.S).group(0)
bad = re.sub(r'checksum = "[0-9a-f]{64}"', 'checksum = "' + "0" * 64 + '"', block)
assert bad != block, "checksum line not found in the wasm-bindgen block"
p.write_text(s.replace(block, bad))
print("corrupted the wasm-bindgen checksum")
PY
moon run paigasus-kernel-ts:build
```
Expected: **FAIL**, with cargo reporting
`unable to verify that 'wasm-bindgen v0.2.126' is the same as when the lockfile was generated`.
Before this change the same edit would have replayed a cached green, because the task did not key
on the lockfile at all.

- [ ] **Step 5: Restore and confirm green**

```bash
git checkout rs/Cargo.lock
git status --short          # expected: empty
touch rs/crates/libs/paigasus-kernel/src/lib.rs
moon run paigasus-kernel-ts:build
```
Expected: PASS. The `touch` is mandatory: `git checkout` can leave a source file OLDER than an
artifact built during the experiment, and cargo's mtime-based incrementality would then reuse the
wrong build. Never restore such an experiment with `mv file.bak file`.

- [ ] **Step 6: V6 — record the interleaved cost**

The spec's `+21s` is a sequential lower bound. Measure the real thing, cold:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
rm -rf .moon/cache && (cd rs && cargo clean)
printf '\n' >> rs/Cargo.lock
git add rs/Cargo.lock && git commit -m "chore(repo): temporary lockfile touch for SMA-546 cost measurement"
/usr/bin/time -l moon ci :build :test :lint --base HEAD~1 --include-relations 2>&1 | tail -20
du -sh rs/target
du -sh rs/target/wasm32-unknown-unknown 2>/dev/null || echo "no wasm32 dir"
git reset --hard HEAD~1
```
Record wall time, the `rs/target` total, and the wasm32 subtotal separately — `CARGO_PROFILE_*_DEBUG:
line-tables-only` shrinks the dev-profile baseline on CI but does **not** apply to
`wasm-pack --release`, and SMA-444 makes runner disk a real constraint. Also note whether wasm-pack
printed `⬇️ Installing wasm-bindgen...` and how long that took.

- [ ] **Step 7: V7 — run the full repo gate graph as CI does**

Per-project tasks do not run the repo-level gates. Run the whole list:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :promtool :observability-drift :nats-permissions \
  :release-parity :release-parity-py :release-parity-ts :publish-metadata \
  --base origin/main --include-relations
```
Expected: all green. If Moon reports an unattributed failure, identify it with:
```bash
python3 -c 'import json;print([a["label"] for a in json.load(open(".moon/cache/ciReport.json"))["actions"] if a.get("status")=="failed"])'
```
Note: `paigasus-iam`'s Docker-backed suites silently pass without Docker. If they are in the
affected set, re-run that crate with `CI=1 cargo nextest run -p paigasus-iam` from `rs/` to make a
missing daemon a hard failure.

- [ ] **Step 8: Confirm the tree is clean**

```bash
git status --short              # expected: empty
git log --oneline origin/main..HEAD
```
Expected: six commits (two spec/plan, four implementation). No `chore(repo): temporary` commit may
survive — if one does, `git reset --hard` back to the last real commit.

---

## Self-Review

**Spec coverage.** Decision §inputs → Task 2 Steps 1/2/4. §`--locked` → Task 2 Steps 3/4.
§`rust-toolchain.toml`+`.prototools` → Task 2, and asserted by A5's `FFI_TASK_INPUTS` (Task 1
Step 1). §py-root exemption → recorded in Task 3 Step 2's comment; enforced by the behavioural
case's strict equality. Guard Layer 1 → Task 3. Layer 2 (A5, derived ∪ floor, three self-test
directions, `command`+`args`+`script`, infra-on-absent) → Task 1 Steps 1/2/4/5/6. Scope items 1-6 →
Tasks 2, 3, 4. V1 → Task 3 Steps 4/5. V2 → Task 5 Step 1. V3 → Task 3 Step 1. V4 → Task 5 Step 2.
V5 → Task 5 Steps 4/5. V6 → Task 5 Step 6. V7 → Task 5 Step 7.

**Deliberately not in this plan** (spec §Scope, out of scope): committed-glue drift, caching
wasm-pack's cache dir, `repo:parity-corpus-drift`'s missing lockfile input, `prebuild.yml` triggers,
consolidating the duplicated ts `build`/`test` work. Do not implement these; the glue-drift item is
to be filed as a follow-up issue after merge.

**Type consistency.** `check_ffi_inputs(projects, required, floor)` is defined in Task 1 Step 5 and
called in Task 1 Step 7 (`check_ffi_inputs(projects)`) and in the self-test rows (Task 1 Step 2,
`check_ffi_inputs(fixture, floor=ffi_floor)`) — the `floor` keyword matches. `MoonOutputError` is
defined in Step 4 and referenced in Steps 2 and 5. The `"invocations"` key is produced in Step 6 and
consumed in Step 5, and every self-test fixture in Step 2 supplies it. `FFI_TASK_INPUTS` is defined
in Step 1 and used in Steps 2 and 5.

**One ordering constraint.** Task 1 Step 2 (self-test rows) references `check_ffi_inputs` and
`MoonOutputError` before Steps 4-6 define them. That is intentional TDD — Step 3 exists to observe
the `NameError`. Do not reorder.
