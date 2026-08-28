# Codegen-config freshness and input-affectedness residue — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close three files that influence a build but are in no compiling task's `inputs`, so a
generator bump, a macOS link-flag edit and a PyO3 stub edit each stop replaying a cached result.

**Architecture:** Every change is an `inputs` addition plus the assertion that keeps it from
rotting. No new Moon task and no new CI job. Two assertions are extended (`WORKSPACE_LINT_INPUTS`
feeding A4/A5; A7's derived `want` set) and one is added (`check_contracts_generate_inputs`).

**Tech Stack:** Moon 2.3.2, buf 1.70.0, Python 3.12 (the `ci/affected-graph` gates), bash.

**Spec:** `docs/superpowers/specs/2026-08-28-sma-592-codegen-and-input-affectedness-residue-design.md`

## Global Constraints

- Branch `feature/sma-592-codegen-and-input-affectedness-residue`, worktree
  `.claude/worktrees/sma-592`. It is already provisioned; do not re-provision.
- Every shell step must start with
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` (shims FIRST) or `moon`/`buf`/`uv`
  resolve to the wrong versions.
- Run `cargo` and `buf` from inside their workspace dir (`rs/`, `contracts/`), never the repo root.
- Conventional commits with a workspace scope. Subject lowercase after the scope, ≤100 chars.
  Never put a bare `#NNN` or a `token: value` line in the commit BODY — it fails
  `footer-leading-blank`. Write "SMA-592", not "#592".
- Do NOT bypass git hooks. No `--no-verify`, no `core.hooksPath=/dev/null`.
- `ci/affected-graph/run.sh` is strict-equality. Any expected-set movement is REPORTED in the
  commit message, never silently re-baselined.
- Two peer sessions are live in this repo. `paigasus-core-2b` (SMA-593) edits
  `SELF_TASK_EXPECTED_GLOBS["publish-metadata"]` in `ci_targets.py`; `paigasus-core-b2` (SMA-579)
  edits `SELF_SCHEDULED_GATES` and `ACTIONLINT_SH_CALL_SITES`. Touch none of those three.

---

### Task 1: SMA-592 — prove the codegen-drift hole, then close it

**Files:**
- Modify: `contracts/moon.yml:8-13` (the `generate` task's `inputs`)
- Modify: `ci/affected-graph/ci_targets.py` (new constant, new function, `main()` wiring, self-test)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `CONTRACTS_GENERATE_INPUTS: tuple[str, ...]` and
  `check_contracts_generate_inputs(projects: dict) -> list[str]`, both module-level in
  `ci/affected-graph/ci_targets.py`. Task 4 re-runs them; no other task imports them.

- [ ] **Step 1: Prove the hole is real (the failing test)**

This is the spec's §7 step 5 and the acceptance evidence for SMA-592. It is the one claim the
spec derived by reading rather than running. Do it FIRST — if it does not reproduce, stop and
report, because the rest of this task would then be fixing nothing.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-592

# Warm the cache so contracts:generate has a recorded hash to hit.
moon run contracts:generate
git status --short -- '*/generated' '*/generated/**'   # expect: clean

# Bump a generator pin that is NOT currently an input. `.prototools` pins buf itself.
sed -i '' 's/^buf = "1.70.0"$/buf = "1.70.1"/' .prototools

# The drift gate's own commands, lifted verbatim from ci.yml:249-262.
moon run contracts:generate
git add --intent-to-add -- \
    rs/crates/libs/paigasus-proto/src/generated \
    py/packages/paigasus-proto/src/paigasus_proto/generated \
    ts/packages/paigasus-proto/src/generated
git diff --exit-code -- \
    rs/crates/libs/paigasus-proto/src/generated \
    py/packages/paigasus-proto/src/paigasus_proto/generated \
    ts/packages/paigasus-proto/src/generated
echo "diff exit: $?"
```

Expected: `moon run contracts:generate` reports a **cached** run (no `buf generate` output), and
the diff exits **0**. That zero is the bug: a generator pin moved and the gate said nothing.

Record the exact `moon` line that shows the cache hit — it goes in the commit message.

**If `buf 1.70.1` does not exist in the proto plugin index** and `moon run` fails at toolchain
install rather than serving a cache hit, use `py/uv.lock` instead: run
`(cd py && uv lock --upgrade-package basedpyright)` and repeat from the second `moon run`. Either
file proves the same thing; the spec measured both as absent from the inputs.

- [ ] **Step 2: Restore the tree**

```bash
git checkout -- .prototools py/uv.lock
git status --short   # expect: clean apart from untracked plan/spec files
```

Do not skip this. A leftover pin bump silently changes every later measurement.

- [ ] **Step 3: Add the two inputs**

In `contracts/moon.yml`, the `generate` task's `inputs` list becomes:

```yaml
  generate:
    command: 'buf generate'
    toolchain: 'system'
    # SMA-592. The three REMOTE plugin versions live in buf.gen.yaml, already listed below. The
    # other two generators do not, and without them this task's cache key is a lie:
    #   /.prototools   pins buf ITSELF (1.70.0). buf's own version changes its output.
    #   /py/uv.lock    pins protoc-gen-python_betterproto2 — the `local:` plugin in buf.gen.yaml
    #                  is run through `uv run --project ../py`, so the py workspace lock is what
    #                  selects the compiler version.
    # This matters because ci.yml:249-262's codegen-drift gate DELEGATES its freshness to this
    # task: it runs `moon run contracts:generate` and diffs. On a cache hit buf never runs, and
    # the diff compares the committed output against itself. `.moon/cache` is restored across CI
    # runs (ci.yml:115-121), so that vacuous pass happens in CI, not just locally.
    inputs:
      - 'proto/**/*'
      - 'buf.yaml'
      - 'buf.gen.yaml'
      - 'buf.lock'
      - '/.prototools'
      - '/py/uv.lock'
```

- [ ] **Step 4: Prove the hole is closed**

Repeat Step 1 verbatim. Expected NOW: `moon run contracts:generate` actually runs `buf generate`,
and either the diff exits non-zero (if buf 1.70.1 changes output) or exits 0 having genuinely
regenerated. Either is a pass — the assertion is that **buf ran**, not that output changed. Then
restore the tree as in Step 2.

- [ ] **Step 5: Write the failing self-test for the new pin**

Append to `self_test()` in `ci/affected-graph/ci_targets.py`, immediately after the existing
`check_gate_inputs` rows (the block ending near `:1912`):

```python
    # SMA-592. contracts:generate is not a repo:* gate, so check_gate_inputs cannot reach it —
    # that function hardcodes projects.get("repo"). This pin exists because the ci.yml
    # codegen-drift gate delegates its freshness to this task's cache key: drop an input and the
    # gate goes vacuous while still printing green.
    cg_ok = {"contracts": {"generate": {
        "inputGlobs": {"proto/**/*": {}, ".moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}": {}},
        "inputFiles": {"buf.gen.yaml": {}, "buf.lock": {}, "buf.yaml": {},
                       ".prototools": {}, "py/uv.lock": {}},
    }}}
    if check_contracts_generate_inputs(cg_ok):
        failures.append("contracts:generate pin reported drift on the clean fixture")

    # The dangerous direction: a dropped input.
    cg_broken = json.loads(json.dumps(cg_ok))
    del cg_broken["contracts"]["generate"]["inputFiles"]["py/uv.lock"]
    if not any("py/uv.lock" in row for row in check_contracts_generate_inputs(cg_broken)):
        failures.append("contracts:generate pin did not fire on a dropped input")

    # ...and an ADDED one, so the pin is equality and not containment.
    cg_broken = json.loads(json.dumps(cg_ok))
    cg_broken["contracts"]["generate"]["inputFiles"]["stray.txt"] = {}
    if not check_contracts_generate_inputs(cg_broken):
        failures.append("contracts:generate pin did not fire on an added input")

    # The whole project going missing must FIRE, never skip — a moon reshape would otherwise
    # turn this pin into a vacuous pass, the exact failure mode it exists to prevent.
    if not check_contracts_generate_inputs({"repo": {}}):
        failures.append("contracts:generate pin did not fire when the project was absent")

    # A wrong-typed bucket is a moon output shape change, not authored drift: rc 2, not rc 1.
    _expect_raises(
        failures,
        lambda: check_contracts_generate_inputs(
            {"contracts": {"generate": {"inputGlobs": ["proto/**/*"]}}}),
        MoonOutputError,
        "contracts:generate pin on a list-typed inputGlobs",
    )
```

`_expect_raises` is the existing helper used by the `check_gate_inputs` rows near `:1908`. Reuse
it; do not write a second one.

- [ ] **Step 6: Run the self-test and watch it fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-592
python3 ci/affected-graph/ci_targets.py --self-test
```

Expected: `NameError: name 'check_contracts_generate_inputs' is not defined`.

- [ ] **Step 7: Add the constant and the function**

In `ci/affected-graph/ci_targets.py`, beside `SELF_TASK_EXPECTED_GLOBS`:

```python
# SMA-592. contracts:generate's inputs, pinned to exact equality. This task is not a repo:* gate,
# but ci.yml:249-262's codegen-drift step delegates its freshness to it: the step runs
# `moon run contracts:generate` and diffs the three generated dirs, so a dropped input makes the
# step compare the committed output against itself and pass having asserted nothing.
#
# Globs first, then files, each alphabetically — the order check_contracts_generate_inputs
# compares in, mirroring check_gate_inputs. The injected .moon/* glob is filtered before
# comparison and is deliberately absent here.
#
# The trade-off is accepted deliberately: a legitimate edit to these inputs reds the gate until
# this constant is updated. An edit to how the repo's codegen is keyed SHOULD stop a human.
CONTRACTS_GENERATE_INPUTS = (
    "proto/**/*",
    ".prototools",
    "buf.gen.yaml",
    "buf.lock",
    "buf.yaml",
    "py/uv.lock",
)
```

And beside `check_gate_inputs`:

```python
def check_contracts_generate_inputs(projects, expected=CONTRACTS_GENERATE_INPUTS):
    """SMA-592. Rows when contracts:generate's authored inputs have drifted.

    A sibling of check_gate_inputs rather than a generalisation of it. That function hardcodes
    `projects.get("repo")` and carries a default-table assertion plus self-test rows that name
    SELF_TASK_EXPECTED_GLOBS explicitly (:1831-1841, :1895); widening its signature would put the
    guard-the-guard machinery at risk for no gain. The comparison logic below is deliberately
    identical — globs then files, each sorted, injected glob filtered.
    """
    entry = ((projects.get("contracts") or {}).get("generate"))
    if not isinstance(entry, dict):
        return ["contracts:generate is absent from the graph, so its inputs cannot be checked"]
    globs_raw, files_raw = entry.get("inputGlobs"), entry.get("inputFiles")
    for name, raw in (("inputGlobs", globs_raw), ("inputFiles", files_raw)):
        if raw is not None and not isinstance(raw, dict):
            raise MoonOutputError(
                f"`moon query tasks` reported contracts:generate's `{name}` as "
                f"{type(raw).__name__}, expected an object"
            )
    got = tuple(g for g in sorted(globs_raw or {})
                if g != ".moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}")
    files = tuple(sorted(files_raw or {}))
    if got + files == tuple(expected):
        return []
    return [
        f"contracts:generate's authored inputs are {list(got) + list(files)}, "
        f"expected exactly {list(expected)} — dropping one makes ci.yml's codegen-drift gate "
        "compare the committed output against itself and pass vacuously (SMA-592)"
    ]
```

- [ ] **Step 8: Run the self-test and watch it pass**

```bash
python3 ci/affected-graph/ci_targets.py --self-test
```

Expected: `ci-targets self-test OK`.

If the clean fixture fires, the likely cause is ordering: confirm `.prototools` sorts into the
FILE bucket (it is a literal path, so moon resolves it into `inputFiles`), not the glob bucket.
Prove it with `moon query tasks --affected` rather than reasoning about it.

- [ ] **Step 9: Wire it into `main()`**

In `main()`, inside the existing `try:` block, beside `bad_gate_inputs` (`:1953`):

```python
        bad_gate_inputs = check_gate_inputs(raw_tasks)
        bad_generate_inputs = check_contracts_generate_inputs(raw_tasks)
```

It must live INSIDE that `try:`. It raises `MoonOutputError`, which belongs to `INFRA_ERRORS` and
must exit 2, not 1 — the same reasoning the existing comment gives for `_scripts` and
`check_gate_inputs`.

Add `bad_generate_inputs` to the all-clear condition:

```python
    if not (floor or missing or unexpected or bad_exempt or stale_exempt or dead or doc_problems
            or missing_sites or bad_invocation or bad_gate_inputs or bad_generate_inputs):
```

And add a reporting row to the `for rows, title in (...)` tuple:

```python
        (bad_generate_inputs,
         "contracts:generate's inputs have drifted, so ci.yml's codegen-drift gate can serve a\n"
         "    cached pass and compare the committed generated code against itself (SMA-592).\n"
         "    Fix: restore the inputs in contracts/moon.yml, or update\n"
         "    CONTRACTS_GENERATE_INPUTS in ci/affected-graph/ci_targets.py if the change is\n"
         "    intended."),
```

- [ ] **Step 10: Prove the wired gate reds**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-592

# Remove one input for real, and confirm the LIVE gate (not just the self-test) reports it.
sed -i '' "/^      - '\/py\/uv.lock'$/d" contracts/moon.yml
python3 ci/affected-graph/ci_targets.py; echo "exit: $?"
```

Expected: exit 1, and the output names `py/uv.lock` and `CONTRACTS_GENERATE_INPUTS`.

```bash
git checkout -- contracts/moon.yml   # NO, this reverts step 3 too — see below
```

**Do not run that checkout.** Step 3's edit is uncommitted at this point, so a checkout would
discard it. Restore the deleted line by hand with the exact text from Step 3, then re-run
`python3 ci/affected-graph/ci_targets.py` and confirm exit 0.

- [ ] **Step 11: Commit**

```bash
git add contracts/moon.yml ci/affected-graph/ci_targets.py
git commit -m "ci(contracts): key contracts:generate on the generator pins it reads (SMA-592)"
```

Body must state: the measured cache-hit line from Step 1, that `.moon/cache` is CI-restored, and
that the new pin is exact-equality with the stated trade-off.

---

### Task 2: SMA-594 — `rs/.cargo/config.toml`

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py:75` (`WORKSPACE_LINT_INPUTS`), `:656`
  (`complete_inputs`), the A4 and A5 fixtures
- Modify: `.moon/tasks/rust.yml` (`build`, `build-release`, `test`, `lint` — NOT `fmt`)
- Modify: `ts/packages/paigasus-kernel/moon.yml` (`build`, `test`)
- Modify: `py/packages/paigasus-kernel/moon.yml` (`test`)

**Interfaces:**
- Consumes: nothing from Task 1.
- Produces: `WORKSPACE_LINT_INPUTS` gains a fourth entry, `"rs/.cargo/config.toml"`.
  `FFI_TASK_INPUTS` inherits it through the existing `(*WORKSPACE_LINT_INPUTS, ".prototools")`
  splat — do NOT edit `FFI_TASK_INPUTS`. Task 3 depends on neither.

- [ ] **Step 1: Make the fixture derive from the constant, so it cannot drift again**

`cargo_moon_parity.py:656` hardcodes the three files:

```python
    complete_inputs = ["rs/Cargo.lock", "rs/Cargo.toml", "rs/rust-toolchain.toml"]
```

Replace it with:

```python
    # Derived, never hand-listed: this fixture is what A4's CLEAN row asserts against, so a
    # hardcoded copy silently breaks every A4 self-test row the day WORKSPACE_LINT_INPUTS grows
    # (it did, on SMA-594). Deriving it means a new workspace input is covered on day one.
    complete_inputs = list(WORKSPACE_LINT_INPUTS)
```

- [ ] **Step 2: Write the failing state — add the entry**

`cargo_moon_parity.py:75`:

```python
# SMA-534 — the workspace-level files `lint` must key on. `rs/` has no Moon project, so without
# these declared on the inherited lint task a Cargo.lock-only change (every Dependabot Cargo PR)
# schedules no crate task at all. Paths are workspace-relative, exactly as Moon RESOLVES them:
# the YAML says `/rs/Cargo.lock`, `moon query projects` reports `rs/Cargo.lock`.
#
# SMA-594 adds the fourth. Cargo finds `.cargo/config.toml` by walking up from the WORKING
# DIRECTORY, and every cargo invocation in this repo runs with cwd inside `rs/`, so the file is
# read by all of them. It sets `rustflags` for the two *-apple-darwin targets. The criterion for
# a cache input is "does this influence the output", not "is it strictly required" — which is why
# it goes on every cargo-from-rs/ task and not only the two that need the flags today. This
# REVERSES SMA-546's deliberate exclusion; see the design doc's D1 and §3.4 for the argument.
WORKSPACE_LINT_INPUTS = (
    "rs/Cargo.lock",
    "rs/Cargo.toml",
    "rs/rust-toolchain.toml",
    "rs/.cargo/config.toml",
)
```

- [ ] **Step 3: Run the self-test and watch A4/A5 fail against the real tree**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-592
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "self-test exit: $?"
python3 ci/affected-graph/cargo_moon_parity.py; echo "live exit: $?"
```

Expected: self-test PASSES (Step 1 made the fixtures derive), and the LIVE run FAILS with
thirteen `a4-lint` rows and three `a5` rows naming `rs/.cargo/config.toml`. That live failure is
the failing test for Steps 4-6.

- [ ] **Step 4: Sharpen the two "broken" fixtures**

Both now omit two files where they used to omit one, which blunts what they prove. Make each
drop exactly one.

`cargo_moon_parity.py:1047` (A4):

```python
    broken["a-rs"]["task_inputs"]["lint"] = [
        f for f in WORKSPACE_LINT_INPUTS if f != "rs/rust-toolchain.toml"
    ]
```

`cargo_moon_parity.py:~1093` (A5) — the row that asserts `.prototools` is named:

```python
    broken["paigasus-kernel-ts"]["task_inputs"]["build"] = list(WORKSPACE_LINT_INPUTS)
```

- [ ] **Step 5: Declare it on the shared Rust tasks**

In `.moon/tasks/rust.yml`, add `'/rs/.cargo/config.toml'` to the `inputs` of `build`,
`build-release`, `test` and `lint`. Add this comment above the `build` task's inputs:

```yaml
    # `/rs/.cargo/config.toml` (SMA-594) sets rustflags for the two *-apple-darwin targets, and
    # cargo reads it by walking up from the cwd — which for every task here is inside `rs/`. It is
    # on build/build-release/test/lint and deliberately NOT on `fmt`: `cargo fmt --check` neither
    # compiles nor links, and rustflags cannot change formatting. That mirrors how `fmt` already
    # omits @group(upstreams) and the workspace lock. CI is Linux, where the flags are inert, so
    # this is a cache-correctness fix for macOS developers, not a CI-correctness one.
```

Do NOT add it to `fmt`. `FMT_TASK_INPUTS` does not list it, so a4-fmt would red.

- [ ] **Step 6: Declare it on the three FFI wrapper tasks**

A5 requires `FFI_TASK_INPUTS` on all three. Add `- '/rs/.cargo/config.toml'` beside the existing
`- '/.prototools'` in each of:

- `ts/packages/paigasus-kernel/moon.yml`, task `build`
- `ts/packages/paigasus-kernel/moon.yml`, task `test`
- `py/packages/paigasus-kernel/moon.yml`, task `test`

In the ts file add: `#   /rs/.cargo/config.toml  the -undefined dynamic_lookup flags napi's cdylib
link needs on macOS; the `--cwd` above exists to bring this file into cargo's upward walk`

In the py file add: `#   /rs/.cargo/config.toml  read by maturin's cargo (cwd is inside rs/).
maturin injects these link args ITSELF (SMA-578, measured), so the wheel does not NEED them — but
the file still influences any darwin build from rs/, which is the criterion for a cache input.`

- [ ] **Step 7: Run both checks and watch them pass**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "self-test exit: $?"
python3 ci/affected-graph/cargo_moon_parity.py; echo "live exit: $?"
```

Expected: both exit 0.

- [ ] **Step 8: Prove A4 and A5 each red**

```bash
# A4: drop it from the shared lint task.
sed -i '' "/^      - '\/rs\/.cargo\/config.toml'$/d" .moon/tasks/rust.yml
python3 ci/affected-graph/cargo_moon_parity.py 2>&1 | grep -c "a4-lint"   # expect 13 rows
```

Restore by hand (a `git checkout` would also discard Step 5). Then:

```bash
# A5: drop it from one FFI task only.
sed -i '' "/^      - '\/rs\/.cargo\/config.toml'$/d" py/packages/paigasus-kernel/moon.yml
python3 ci/affected-graph/cargo_moon_parity.py 2>&1 | grep "a5"           # expect paigasus-kernel-py:test
```

Restore by hand, re-run, confirm exit 0.

- [ ] **Step 9: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py .moon/tasks/rust.yml \
        ts/packages/paigasus-kernel/moon.yml py/packages/paigasus-kernel/moon.yml
git commit -m "ci(rs): key every cargo-from-rs task on rs/.cargo/config.toml (SMA-594)"
```

Body must state that this REVERSES SMA-546's deliberate exclusion and why, and that the fix is
cache correctness on macOS, not CI correctness.

---

### Task 3: SMA-594′ — the PyO3 stub

**Files:**
- Modify: `py/packages/paigasus-kernel/moon.yml` (task `test` inputs)
- Modify: `ci/affected-graph/cargo_moon_parity.py:540-548` (A7's `want` derivation) and the A7-h
  self-test block near `:968-991`

**Interfaces:**
- Consumes: Task 2 already edited `py/packages/paigasus-kernel/moon.yml`. Both edits land in the
  same `inputs` list; apply this one after Task 2 is committed to keep the diffs separable.
- Produces: nothing later tasks consume.

- [ ] **Step 1: Write the failing self-test (A7-i)**

In `cargo_moon_parity.py`, immediately after the A7-h block that ends with
`failures.append("A7 demanded a build.rs for an upstream that has none on disk")`, add a new
tempdir block:

```python
    # A7-i: the .pyi half (SMA-594'). Same disk-conditional shape as A7-h's build.rs, and it needs
    # its own tree for the same reason — the `is_file()`/glob branch is only live when a stub
    # actually exists under an upstream's source_dir.
    with tempfile.TemporaryDirectory() as tmp:
        stubbed = Path(tmp)
        (stubbed / "rs" / "crates" / "bindings" / "nb").mkdir(parents=True)
        (stubbed / "rs" / "crates" / "bindings" / "nb" / "nb.pyi").write_text("def f() -> int: ...\n")
        rows = check_wrapper_upstream_inputs(wrap, stubbed, floor=wrap_floor)
        if not any("inputs omit rs/crates/bindings/nb/nb.pyi" in row for row in rows):
            failures.append(
                "A7 did not demand an upstream's .pyi stub that exists on disk — the stub half "
                "of the check is not asserting anything"
            )
        # Demanded of EVERY examined task, not just the first.
        for task in ("build", "test"):
            if not any(
                f"k-ts:{task} inputs omit rs/crates/bindings/nb/nb.pyi" == row for row in rows
            ):
                failures.append(f"A7 did not demand nb's .pyi of k-ts:{task}")
        # An upstream with NO stub must not be demanded one, or every wrapper gains an
        # unsatisfiable row the day this branch is written wrong.
        if any("rs/crates/libs/kern" in row and ".pyi" in row for row in rows):
            failures.append("A7 demanded a .pyi for an upstream that has none on disk")
```

- [ ] **Step 2: Run the self-test and watch it fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-592
python3 ci/affected-graph/cargo_moon_parity.py --self-test
```

Expected: FAIL, naming `A7 did not demand an upstream's .pyi stub that exists on disk`.

- [ ] **Step 3: Add the clause to A7's `want` derivation**

In `check_wrapper_upstream_inputs`, the per-upstream loop currently reads:

```python
        for upstream in sorted(rust_closure(projects, pid)):
            src = projects[upstream]["source_dir"]
            want.add(f"{src}/src/**/*")
            want.add(f"{src}/Cargo.toml")
            if (root / src / "build.rs").is_file():
                want.add(f"{src}/build.rs")
```

Append:

```python
            # SMA-594'. A hand-written .pyi is the interface contract between a PyO3 cdylib and
            # every Python consumer, and it is what basedpyright reads INSTEAD of the Rust. It
            # lives at the crate ROOT, so `{src}/src/**/*` does not match it and nothing that
            # validates it keyed on it. Disk-conditional and globbed, mirroring build.rs above:
            # conditional so the twelve crates without a stub gain no dead demand, globbed so a
            # second stub is covered the day it appears rather than needing a hand-maintained list.
            for stub in sorted((root / src).glob("*.pyi")):
                want.add(f"{src}/{stub.name}")
```

`(root / src).glob(...)` returns an empty iterator when the directory does not exist, so no
`is_file()`-style existence guard is needed around it.

- [ ] **Step 4: Run the self-test and watch it pass**

```bash
python3 ci/affected-graph/cargo_moon_parity.py --self-test
```

Expected: `OK`. Then run the live check:

```bash
python3 ci/affected-graph/cargo_moon_parity.py; echo "exit: $?"
```

Expected: FAIL with one `a7` row —
`paigasus-kernel-py:test inputs omit rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi`.
That is the real gap, now reported.

- [ ] **Step 5: Declare the input**

In `py/packages/paigasus-kernel/moon.yml`, task `test`, beside the binding's other manifests:

```yaml
      # The hand-written PyO3 stub (SMA-594'). It is the interface contract this test exercises,
      # and basedpyright reads it INSTEAD of the Rust — but it sits at the crate ROOT, so the
      # `src/**/*` glob above does not match it. Before this line, editing the stub selected only
      # repo:actionlint, repo:input-liveness and repo:publish-metadata (all three broad or
      # packaging gates); the FFI smoke test that exercises those very symbols did not run.
      - '/rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi'
```

- [ ] **Step 6: Run the live check and watch it pass**

```bash
python3 ci/affected-graph/cargo_moon_parity.py; echo "exit: $?"
```

Expected: exit 0.

- [ ] **Step 7: Prove it reds, and prove the clause is not vacuous**

```bash
sed -i '' "/paigasus_py_bindings.pyi'$/d" py/packages/paigasus-kernel/moon.yml
python3 ci/affected-graph/cargo_moon_parity.py 2>&1 | grep "a7"
```

Expected: the one named row. Restore by hand and re-run to exit 0.

Then confirm the clause creates no false demand: A7 must still pass for `paigasus-kernel-ts`,
whose closure is the kernel plus the node and wasm bindings, none of which carries a `.pyi`.
Grep the passing output for `paigasus-kernel-ts.*pyi` and expect zero matches.

- [ ] **Step 8: Commit**

```bash
git add py/packages/paigasus-kernel/moon.yml ci/affected-graph/cargo_moon_parity.py
git commit -m "ci(py): key the FFI smoke test on the PyO3 stub it exercises (SMA-594)"
```

---

### Task 4: Measure, document, and run the graph as CI does

**Files:**
- Modify: `CLAUDE.md` (one new gotcha bullet; correct the codegen-drift description)
- Modify: `docs/superpowers/specs/2026-08-28-sma-592-codegen-and-input-affectedness-residue-design.md`
  (§9 open questions, now answered)

- [ ] **Step 1: Re-run the five baseline measurements**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-592
for f in "rs/crates/bindings/paigasus-py-bindings/paigasus_py_bindings.pyi" \
         "rs/.cargo/config.toml" ".prototools" "py/uv.lock" "contracts/buf.gen.yaml"; do
  echo "=== $f ==="
  echo "$f" | moon query tasks --affected 2>/dev/null \
    | grep -o '"target": "[^"]*"' | sed 's/"target": //' | tr -d '"' | sort | tr '\n' ' '
  echo; echo
done
```

Expected movement against the spec's §1 table, and NOTHING else:
- the `.pyi` gains `paigasus-kernel-py:test`
- `rs/.cargo/config.toml` gains 13 `*:lint` + `paigasus-kernel-py:test` +
  `paigasus-kernel-ts:{build,test}` (and `*:build`/`*:test` for every crate)
- `.prototools` and `py/uv.lock` each gain `contracts:generate`
- `contracts/buf.gen.yaml` unchanged

Any other movement is a finding. Report it; do not absorb it — with ONE known exception,
below.

**Known concurrent change.** `paigasus-core-2b` (SMA-593) is adding a new gate,
`repo:workflow-credentials`, whose inputs include `.github/workflows/*.y*ml`, `ci/workflow-
credentials/**/*` and py lock inputs. If that branch lands before this one, the `.prototools` and
`py/uv.lock` rows above legitimately gain `repo:workflow-credentials`, and the graph carries one
more task overall. That is their change, not drift from this branch. Confirm it against their
merged diff rather than assuming — and confirm it is the ONLY extra row, since anything else is
still a finding.

Nothing in this branch asserts the task count or `T`-array membership, so their gate needs no
accommodation here beyond this note. `ci/affected-graph/run.sh`'s cases all key on Rust or proto
source files, none on a workflow file, so their gate should not move any expected set either.

- [ ] **Step 2: Run the affected-graph guard and check for expected-set movement**

```bash
bash ci/affected-graph/run.sh; echo "exit: $?"
```

Expected: exit 0 with NO expected-set changes. The spec's §7 step 6 predicts this: no `run.sh`
case edits any of the four files, and adding an input changes what THAT FILE selects, not what a
source edit selects. **If any set moves, stop and report it rather than re-baselining** — that
would mean one of the new inputs is broader than intended.

- [ ] **Step 3: Time `buf generate` and answer the spec's open question 1**

```bash
time (cd contracts && buf generate)
git checkout -- rs/crates/libs/paigasus-proto/src/generated \
                py/packages/paigasus-proto/src/paigasus_proto/generated \
                ts/packages/paigasus-proto/src/generated
```

Record the wall-clock. Then update §9 of the spec with the answer, including the churn figures
already measured on `main`'s 162 commits: `contracts/proto/**` 21 commits (13%), `py/uv.lock` 21
(13%), `.prototools` 13 (8%). State the conclusion: the drift step already runs
`moon run contracts:generate` on every CI run, so the marginal cost is only the runs where the
cache would otherwise have hit — roughly 20% more PRs paying one `buf generate`.

Answer open question 2 in the same edit: `.prototools` pins twelve tools and only `buf` affects
codegen, but Moon has no sub-file input granularity, so the over-trigger is accepted. Record that
reasoning rather than leaving it implicit.

- [ ] **Step 4: Update CLAUDE.md**

Two edits. First, correct the existing false claim. CLAUDE.md's `repo:version-lockstep` bullet
says "the codegen-drift gate covers only the three `**/generated` proto dirs". The coverage claim
is right; what is missing is WHERE it lives. Add a new bullet to Gotchas:

```markdown
- The **codegen-drift gate is an inline `ci.yml` step** (`.github/workflows/ci.yml:249-262`), NOT
  a `repo:*` Moon task — searching `moon.yml` for it finds nothing. That placement is deliberate
  and load-bearing: the step carries no `if:`, so it runs on EVERY CI run and cannot be
  deselected, where a `T`-array task would run only when affected and a wrong `inputs` list would
  switch it off silently. It delegates its freshness to `moon run contracts:generate`, so that
  task's `inputs` are what make the diff real: they now include `/.prototools` (which pins `buf`
  itself) and `/py/uv.lock` (which pins the `local:` betterproto2 plugin, run via `uv run
  --project ../py`), alongside `buf.gen.yaml` which pins the three REMOTE plugins. Before SMA-592
  the first two were absent, so a generator bump left the hash unchanged, Moon served a cached
  pass, `buf generate` never ran, and the diff compared the committed output against itself —
  vacuously green. `.moon/cache` is restored across CI runs (`ci.yml:115-121`), so that was a real
  CI hole, not a local-only one. `contracts:generate` still declares no `outputs:`; this makes its
  cache KEY honest, not its output restorable, which is the second reason the drift step stays
  unconditional. The inputs are pinned to exact equality by `CONTRACTS_GENERATE_INPUTS` in
  `ci/affected-graph/ci_targets.py` — reachable because `repo:affected-smoke` lists `*/moon.yml`.
- `rs/.cargo/config.toml` is now an input of every task that runs cargo from `rs/` — all thirteen
  crates' `build`/`build-release`/`test`/`lint` and the three FFI wrapper tasks — asserted by A4
  (via `WORKSPACE_LINT_INPUTS`) and A5 (via the `FFI_TASK_INPUTS` splat). It is deliberately NOT
  on `fmt`: `cargo fmt --check` neither compiles nor links, so rustflags cannot change its result.
  This REVERSES SMA-546's deliberate exclusion, which reasoned that CI is Linux and the darwin
  flags are inert there. Both are true; the criterion changed to "does this file influence the
  output" rather than "is it strictly required", because `rustflags` affect every darwin build
  from `rs/`. Note maturin injects the `-undefined dynamic_lookup` args ITSELF (SMA-578), so the
  py wheel does not NEED the file — it is keyed on it anyway, under the same one rule, which is
  why `REQUIRED_FFI_TASKS` needs no carve-out.
- A hand-written `.pyi` next to a PyO3 crate is an interface contract that basedpyright reads
  INSTEAD of the Rust, and it lives at the crate ROOT where `src/**/*` does not match it. A7 now
  demands every `{upstream}/*.pyi` found on disk, disk-conditional exactly like its `build.rs`
  clause. Do NOT read this as closing SMA-535: it makes a stub edit re-run the FFI smoke test, it
  does NOT make a stub that disagrees with the Rust fail. That needs a three-set drift gate
  (`#[pyfunction]` idents × `wrap_pyfunction!` registrations × stub `def` names), which is SMA-535
  proper and pairs with SMA-536.
```

- [ ] **Step 5: Run the full graph exactly as CI does**

Use the marker-delimited command from CLAUDE.md verbatim. Do not retype it from memory — copy it
between the `<!-- ci-targets:begin -->` / `<!-- ci-targets:end -->` markers. This matters more than
usual right now: `paigasus-core-2b` is adding `:workflow-credentials` to both `ci.yml`'s `T=(…)`
array and that marker block, so the command below is a snapshot that may already be stale. The
markers are the source of truth; `repo:affected-smoke` asserts the two agree.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-592
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep --base origin/main --include-relations
echo "exit: $?"
```

If Moon reports an unattributed "N failed", read `.moon/cache/ciReport.json` to find which target
went red — the task output style is `buffer-only-failure`.

Expect the Docker-backed `paigasus-iam` suites to be slow or to red if Docker is unreachable;
`tests/docker_preflight.rs` is the canary that makes that visible. That is environmental, not a
regression from this branch. Baseline against unmodified `origin/main` before blaming the diff.

- [ ] **Step 6: Commit**

```bash
git add CLAUDE.md docs/superpowers/specs/2026-08-28-sma-592-codegen-and-input-affectedness-residue-design.md
git commit -m "docs(ci): record the codegen-config and cargo-config input rules (SMA-592, SMA-594)"
```

- [ ] **Step 7: Report, do not close**

Report to the user: the Step 1 measurement table, whether Step 2 moved any expected set, the
`buf generate` timing, and the full-graph result. Note that SMA-535 remains open and why, and
that the `.pyi` fix must not be read as closing it in part.

---

## Self-Review

**Spec coverage:**

| Spec section | Task |
| -- | -- |
| §2.4 the two inputs | Task 1 Step 3 |
| §2.5 the pin, and the new function rather than `check_gate_inputs` | Task 1 Steps 5-10 |
| §3.2 D1 one rule via `WORKSPACE_LINT_INPUTS` | Task 2 Steps 2, 5, 6 |
| §3.3 D2 not on `fmt` | Task 2 Step 5 (explicit prohibition) |
| §4 the `.pyi`, both halves | Task 3 |
| §5 SMA-535 stays out | Task 4 Step 7 (reported, not closed) |
| §6 D3 gate stays in `ci.yml` | No task — the decision is to change nothing; recorded in Task 4 Step 4 |
| §7 steps 1-7 testing | Task 1 Steps 1/4/10, Task 2 Steps 3/7/8, Task 3 Steps 2/4/6/7, Task 4 Steps 1/2/5 |
| §9 open questions 1 and 2 | Task 4 Step 3 |

No gaps.

**Type consistency:** `CONTRACTS_GENERATE_INPUTS` and `check_contracts_generate_inputs` are
spelled identically in Task 1 Steps 5, 7, 9 and in Task 4. `WORKSPACE_LINT_INPUTS` is edited in
one place (Task 2 Step 2) and consumed by name in Steps 1 and 4. `FFI_TASK_INPUTS` is never
edited — the splat carries the change, stated in Task 2's Interfaces block.

**Two ordering hazards, called out where they bite:** Task 1 Step 10 and Task 2 Step 8 both warn
against `git checkout --` to undo a probe, because the file also carries an uncommitted fix from
an earlier step. Task 3's Interfaces block notes that Task 2 already edited
`py/packages/paigasus-kernel/moon.yml`.
