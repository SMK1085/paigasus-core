# SMA-526 Rust lint propagation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make Rust `lint` propagate across Moon dependency edges so an in-tree upstream change that trips `-D warnings` in a consumer fails the PR that introduces it, and guard that invariant permanently.

**Architecture:** One `deps: ['^:build']` on the inherited `lint` task in `.moon/tasks/rust.yml` fixes all 13 crates and every future crate at once. The SMA-524 parity gate's A3 assertion is widened to `lint` so the single declaration site cannot be deleted silently, the strict-equality affected-graph guard is re-baselined, and the `rs/target` cache key is re-keyed so the added clippy artifacts are actually saved.

**Tech Stack:** Moon 2.3.2, Cargo/clippy (Rust 1.95, edition 2024), Python 3 (stdlib only — `tomllib`, `subprocess`, `json`), Bash, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-08-16-sma-526-rust-lint-propagation-design.md`

## Global Constraints

- **PATH:** Every shell command must be prefixed with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` — proto-managed CLIs (`moon`, `uv`, `buf`, `cargo-nextest`) are not on the default Bash PATH, and shims must come first so the repo-pinned versions win.
- **Working directory:** `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-526`. This is a git worktree — never `cd` to the main checkout.
- **Branch:** `feature/sma-526-rust-lint-propagation`. Do not create or switch branches.
- **SPDX:** Every source file opens with `# SPDX-License-Identifier: Apache-2.0` (`#` for Python/Bash/YAML). All files in this plan already have theirs — do not remove or duplicate them.
- **Commit messages:** Conventional commits. Allowed scopes: `rs`, `py`, `ts`, `contracts`, `ci`, `docs`, `deps`, `release`, `repo`, `claude`, `workspace`. Subject **must start lowercase**, header ≤ 100 chars, body lines ≤ 100 chars. Never put a bare `#NNN` in the body — it makes commitlint fail `footer-leading-blank`. Write "PR NNN" instead.
- **No `--no-verify`.** The worktree is provisioned; the `commit-msg` hook must run.
- **`cargo` is not available to the gate scripts.** `repo:affected-smoke` is `toolchain: 'system'`; `cargo_moon_parity.py` may use only the Python stdlib and `moon query`.
- **Python style:** the repo's Python is formatted by ruff with a 100-column limit. Match the surrounding file exactly.

---

### Task 1: Widen the parity gate's A3 assertion to `lint`

Widen A3 to cover `lint`, repair the self-test fixtures the widening breaks, and make an *absent* task key report differently from a *present-but-incomplete* one. This task deliberately leaves the repo RED — `main()` will report 17 real violations. Task 2 turns it green. That ordering is the point: it proves the assertion bites before the fix exists.

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` — `check()` (~lines 91-94), `self_test()` (~lines 160-195), remediation text (~lines 262-264)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `check(projects, crates, allow=None) -> (a1, a2, a3)` — unchanged signature. A3 rows now use two distinct message shapes: `f"{mid}:{task} does not schedule {upstream}:build"` (dep missing) and `f"{mid} has no `{task}` task (cannot schedule its upstream builds)"` (task key absent). Task 3 relies on neither string; only `self_test()` matches on them.

- [ ] **Step 1: Update the self-test fixtures so the clean fixture stays clean**

In `self_test()`, replace the `ok` fixture:

```python
    ok = {
        "a-rs": {
            "source_dir": "rs/crates/libs/a",
            "deps": {"b-rs": "explicit"},
            "tasks": {
                "build": ["b-rs:build"],
                "test": ["b-rs:build"],
                "lint": ["b-rs:build"],
            },
        },
        "b-rs": {
            "source_dir": "rs/crates/libs/b",
            "deps": {},
            "tasks": {"build": [], "test": [], "lint": []},
        },
    }
```

- [ ] **Step 2: Update the A3 broken fixture to match**

Find the A3 case and add the `lint` key so it still isolates "dep missing" rather than accidentally testing "task absent":

```python
    # A3: the edge exists but the upstream build is not scheduled — the exact hole the
    # project-level affected-graph guard is structurally blind to (SMA-429 F3).
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["tasks"] = {"build": [], "test": [], "lint": []}
    if not check(broken, crates)[2]:
        failures.append("A3 did not fire on an unscheduled upstream build")
```

- [ ] **Step 3: Add a self-test case for the absent-task message**

Insert directly after the A3 case from Step 2:

```python
    # An ABSENT task key is a different defect from a task that exists but omits the dep, and the
    # violation text has to say which — otherwise the first crate to drop or rename `lint` is told
    # to "add '^:build'" to a task it does not have (SMA-526).
    broken = json.loads(json.dumps(ok))
    del broken["a-rs"]["tasks"]["lint"]
    if not any("has no `lint` task" in row for row in check(broken, crates)[2]):
        failures.append("A3 did not distinguish an absent task from a missing dep")
```

- [ ] **Step 4: Run the self-test to verify it FAILS**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py --self-test
```

Expected: FAIL — `A3 did not distinguish an absent task from a missing dep`. The fixtures now cover `lint`, but `check()` has not been widened yet, so it neither asserts `lint` nor emits the absent-task message. This is the failing test.

- [ ] **Step 5: Widen A3 in `check()`**

Replace the A3 block at the end of the `for _crate, info in sorted(crates.items())` loop:

```python
        # `lint` joined build/test in SMA-526: clippy propagates across a Moon edge only if the
        # task carries `^:build`, so a consumer's lint must schedule its upstreams' builds exactly
        # as its build and test do. Unlike build/test, lint's dep is declared ONCE for every crate
        # in .moon/tasks/rust.yml, so this row fires for all crates at once or not at all.
        for task in ("build", "test", "lint"):
            if want and tasks.get(task) is None:
                a3.append(f"{mid} has no `{task}` task (cannot schedule its upstream builds)")
        for upstream in sorted(want):
            for task in ("build", "test", "lint"):
                deps = tasks.get(task)
                if deps is not None and f"{upstream}:build" not in deps:
                    a3.append(f"{mid}:{task} does not schedule {upstream}:build")
```

- [ ] **Step 6: Run the self-test to verify it PASSES**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py --self-test
```

Expected: PASS — `OK   [parity] all three assertions fire on synthetic violations`

- [ ] **Step 7: Run the gate against the real graph to verify the bug is REAL**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py; echo "rc=$?"
```

Expected: **FAIL, rc=1**, with exactly 17 violations, all of the form `<consumer>:lint does not schedule <upstream>:build` — e.g. `paigasus-service-info-rs:lint does not schedule paigasus-proto-rs:build`. No `has no \`lint\` task` rows (every Rust project inherits `lint`). Record the count; Task 2 asserts it drops to zero.

If the count is not 17, stop and report — the graph differs from what the spec measured.

- [ ] **Step 8: Update the remediation text so it does not misdirect for `lint`**

In `main()`, replace the a3 entry of the reporting tuple:

```python
        (a3, "Moon edge exists but the upstream's build is NOT scheduled — the affected-graph\n"
             "    guard CANNOT see this (SMA-429 F3).\n"
             "    Fix: for `build`/`test`, add '^:build' to the task's `deps` in the consumer's\n"
             "    moon.yml. For `lint` the dep is declared once for ALL crates in\n"
             "    .moon/tasks/rust.yml — restore it there, not per-crate (SMA-526)."),
```

- [ ] **Step 9: Re-run both modes to confirm nothing regressed**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py --self-test
python3 ci/affected-graph/cargo_moon_parity.py; echo "rc=$?"
```

Expected: self-test PASS; real graph still FAIL rc=1 with 17 violations and the new remediation wording.

- [ ] **Step 10: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -m "test(ci): assert lint schedules upstream builds in the parity gate (SMA-526)" -m "Widens A3 from (build, test) to (build, test, lint) and repairs the self-test fixtures the
widening breaks: the clean fixture declared no lint key, and A3 reads tasks.get(task, []), so
the clean fixture would have reported a violation and the negative control would exit 1. CI runs
run.sh without --negative-control, so that would have shipped green with the gate's only
proof-that-it-bites dead.

Deliberately red against the real graph: 17 violations, one per real missing edge. The next
commit adds the propagation that turns it green."
```

---

### Task 2: Make `lint` propagate across Moon edges

The fix. Three words in the inherited task file.

**Files:**
- Modify: `.moon/tasks/rust.yml` (the `lint` task, ~lines 25-27)

**Interfaces:**
- Consumes: the widened A3 from Task 1 — this task's verification is that A3 goes from 17 violations to zero.
- Produces: every Rust project's `lint` task now resolves `^:build` into its upstreams' concrete `:build` targets. Task 3 depends on this to re-baseline the affected-graph expected set.

- [ ] **Step 1: Confirm the gate is still red (the failing test from Task 1)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py; echo "rc=$?"
```

Expected: FAIL rc=1, 17 violations.

- [ ] **Step 2: Capture the BEFORE lint propagation, to prove the change does something**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
printf 'rs/crates/libs/paigasus-kernel/src/lib.rs\n' \
  | moon query tasks --affected --downstream deep \
  | python3 -c 'import sys,json;d=json.load(sys.stdin);print(sorted(f"{p}:{t}" for p,ts in (d.get("tasks") or {}).items() for t in ts if t=="lint"))'
```

Expected: exactly one entry — `['paigasus-kernel-rs:lint']`.

- [ ] **Step 3: Add the dep**

In `.moon/tasks/rust.yml`, the `lint` task becomes:

```yaml
  lint:
    command: 'cargo clippy --all-targets -- -D warnings'
    # `^:build` is what makes a task AFFECTED when an upstream changes. Without it clippy
    # propagated across no edge at all, so an upstream change that tripped `-D warnings` in a
    # CONSUMER shipped green and redded main later (SMA-526). Declared here rather than per-crate
    # on purpose: build/test declare theirs in each moon.yml, which is exactly how SMA-505 shipped
    # a crate with three missing edges. One site means a new crate has nothing to forget.
    # `repo:affected-smoke` asserts this expansion for every crate (cargo_moon_parity.py A3).
    deps: ['^:build']
    inputs: ['@group(sources)', '@group(tests)', 'Cargo.toml']
```

- [ ] **Step 4: Run the parity gate to verify it PASSES**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py; echo "rc=$?"
```

Expected: **PASS, rc=0** — `13 crates: every Cargo dep has a Moon edge that schedules its build`.

- [ ] **Step 5: Verify lint now propagates (spec V1)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
printf 'rs/crates/libs/paigasus-kernel/src/lib.rs\n' \
  | moon query tasks --affected --downstream deep \
  | python3 -c 'import sys,json;d=json.load(sys.stdin);r=sorted(f"{p}:{t}" for p,ts in (d.get("tasks") or {}).items() for t in ts if t=="lint");print(len(r));print("\n".join(r))'
```

Expected: **8** lint tasks — `paigasus-kernel-rs`, `paigasus-iam-core-rs`, `paigasus-iam-rs`, `paigasus-kernel-parity-rs`, `paigasus-node-bindings-rs`, `paigasus-py-bindings-rs`, `paigasus-wasm-rs`, `paigasus-gateway-rs`.

Then the proto case:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
printf 'rs/crates/libs/paigasus-proto/src/lib.rs\n' \
  | moon query tasks --affected --downstream deep \
  | python3 -c 'import sys,json;d=json.load(sys.stdin);r=sorted(f"{p}:{t}" for p,ts in (d.get("tasks") or {}).items() for t in ts if t=="lint");print(len(r));print("\n".join(r))'
```

Expected: **4** — `paigasus-proto-rs`, `paigasus-service-info-rs`, `paigasus-iam-rs`, `paigasus-gateway-rs`.

- [ ] **Step 6: Verify clippy actually passes on every crate at the widened scope**

This is the substance behind the change — if any downstream crate has a latent clippy break, this is where it surfaces.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && cargo clippy --workspace --all-targets -- -D warnings; echo "rc=$?"
```

Expected: rc=0.

**Important:** this `--workspace` invocation resolves features differently from the per-crate ones Moon runs and will evict their artifacts from `rs/target` (measured: 69s of rebuild). That is a local-only cost; do not repeat it casually, and expect the next per-crate task to be slow once.

- [ ] **Step 7: Commit**

```bash
git add .moon/tasks/rust.yml
git commit -m "fix(rs): propagate clippy across Moon edges via lint deps (SMA-526)" -m "lint and fmt carried no deps, so clippy propagated across no edge in the workspace: a consumer
that an upstream change broke under -D warnings was never linted on the PR, merged green, and
redded main on the next run that happened to schedule its lint.

Declared in the inherited .moon/tasks/rust.yml rather than per-crate so a new crate has no
per-crate declaration to forget. fmt is deliberately left alone: cargo fmt --check reads only the
crate's own files, so an upstream edit cannot change a downstream crate's formatting.

Turns the parity gate's widened A3 from 17 violations to zero."
```

---

### Task 3: Re-baseline the strict-equality affected-graph guard

`ci/affected-graph/run.sh` asserts an exact task set. Task 2 added four `lint` targets to the `proto->service-info-tasks` case, so the guard is now red until its filter and expected set admit `lint`.

**Files:**
- Modify: `ci/affected-graph/run.sh` — `assert_task_case()` doc comment (~lines 66-78), its task-name filter (~line 90), the missing-row diagnostic (~line 103), the `proto->service-info-tasks` case (~lines 219-222)
- Modify: `ci/affected-graph/README.md`

**Interfaces:**
- Consumes: the propagation from Task 2 and the widened A3 from Task 1.
- Produces: `ci/affected-graph/run.sh` exits 0; `ci/affected-graph/run.sh --negative-control` exits 0.

- [ ] **Step 1: Run the guard to see it fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/affected-graph/run.sh; echo "rc=$?"
```

Expected: **FAIL rc=1** on `proto->service-info-tasks`, reporting four *unexpected* entries — `paigasus-gateway-rs:lint`, `paigasus-iam-rs:lint`, `paigasus-proto-rs:lint`, `paigasus-service-info-rs:lint`. (`assert_cargo_moon_parity` should PASS — Task 2 fixed it.)

Capture the exact `unexpected` list from the output; Step 3's expected set must match it.

- [ ] **Step 2: Widen the task-name filter**

In `assert_task_case()`, change the comprehension filter:

```python
        if name in ("build", "test", "lint"):
```

- [ ] **Step 3: Update the case's expected set**

Replace the `proto->service-info-tasks` case:

```bash
  # A proto edit must SCHEDULE paigasus-service-info's build, test AND lint, not merely mark the
  # project affected. This is the behavioral half of SMA-524 (build/test) and SMA-526 (lint): the
  # parity gate asserts `^:build` is DECLARED, this asserts it takes EFFECT.
  run_task_case "proto->service-info-tasks" "rs/crates/libs/paigasus-proto/src/lib.rs" \
    "paigasus-proto-rs:build,paigasus-proto-rs:test,paigasus-proto-rs:lint,paigasus-service-info-rs:build,paigasus-service-info-rs:test,paigasus-service-info-rs:lint,paigasus-iam-rs:build,paigasus-iam-rs:test,paigasus-iam-rs:lint,paigasus-gateway-rs:build,paigasus-gateway-rs:test,paigasus-gateway-rs:lint"
```

- [ ] **Step 4: Update `assert_task_case`'s doc comment, whose rationale has expired**

Replace the "Scoped to build/test" paragraph:

```bash
#   Scoped to build/test/lint — the three tasks that carry `^:build` (lint joined them in
#   SMA-526). fmt and build-release are excluded because they carry no `^:build`: fmt is
#   crate-local by construction, and build-release does not run in CI at all.
#
#   NOTE: the filter matches task NAMES across every project, not just Rust ones, so a
#   same-named task in another stack could enter a case's observed set. `contracts:lint` exists
#   and does not appear here — contracts is UPSTREAM of paigasus-proto-rs and `--downstream deep`
#   walks dependents — but a future case with a different touched file must re-check that.
```

- [ ] **Step 5: Update the missing-row diagnostic so it does not misdirect for `lint`**

```bash
    echo "  missing  (expected but not scheduled — likely a dropped task-level '^:build'; for" >&2
    echo "  \`lint\` that dep lives once in .moon/tasks/rust.yml, not per-crate):" >&2
```

- [ ] **Step 6: Run the guard to verify it PASSES**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/affected-graph/run.sh; echo "rc=$?"
```

Expected: **rc=0**, ending `== affected-graph cascade intact ==`, with every case PASS including `proto->service-info-tasks` listing all twelve targets.

- [ ] **Step 7: Run the negative control (spec V4)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/affected-graph/run.sh --negative-control; echo "rc=$?"
```

Expected: **rc=0**, ending `negative-control OK: harness reported red on all wrong expectations`, and including `OK   [parity] all three assertions fire on synthetic violations`.

This is the check CI does not run. If it fails, Task 1's fixtures are wrong — fix them before continuing.

- [ ] **Step 8: Update the README**

In `ci/affected-graph/README.md`, update the parity-gate description so it names all three tasks, and note that `lint`'s dep is declared once in `.moon/tasks/rust.yml` rather than per-crate. Keep the existing prose style; do not restructure the document. Also confirm the strict-equality maintenance section still describes how to update an expected set correctly now that a case carries `lint` rows.

- [ ] **Step 9: Commit**

```bash
git add ci/affected-graph/run.sh ci/affected-graph/README.md
git commit -m "ci(repo): re-baseline the affected-graph guard for lint propagation (SMA-526)" -m "The proto->service-info-tasks case is strict-equality and filtered to build/test, so the four new
lint targets read as unexpected entries. Widens the filter and the expected set to twelve targets,
and records that the filter matches task names across all projects — contracts:lint exists but is
upstream of the touched file, so it does not appear.

Also fixes both diagnostics that now misdirect: for lint the missing dep lives once in
.moon/tasks/rust.yml, not in the consumer's moon.yml."
```

---

### Task 4: Order `lint` behind `contracts:generate` for the generated-code crates

`contracts:generate` declares no `outputs:` yet writes into `rs/crates/libs/paigasus-proto/src/generated/**` — the files clippy compiles. `build` and `test` already depend on it; `lint` does not, so Moon may run them concurrently. The ts sibling (`ts/packages/paigasus-proto/moon.yml`) already wires it into `build`, `typecheck` *and* `test`.

**Files:**
- Modify: `rs/crates/libs/paigasus-proto/moon.yml`
- Modify: `rs/crates/libs/paigasus-service-info/moon.yml`

**Interfaces:**
- Consumes: the inherited `lint: deps: ['^:build']` from Task 2.
- Produces: `paigasus-proto-rs:lint` and `paigasus-service-info-rs:lint` resolve deps that include **both** `contracts:generate` and their upstreams' `:build` targets.

- [ ] **Step 1: Record the current resolved lint deps**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query projects --json \
  | python3 -c 'import sys,json;d=json.load(sys.stdin);[print(p["id"],[x if isinstance(x,str) else x.get("target") for x in ((p.get("tasks") or {}).get("lint") or {}).get("deps") or []]) for p in d["projects"] if p["id"] in ("paigasus-proto-rs","paigasus-service-info-rs")]'
```

Expected: `paigasus-proto-rs ['paigasus-proto-derive-rs:build']` and `paigasus-service-info-rs ['paigasus-proto-rs:build']` — inherited `^:build` only, no `contracts:generate`.

- [ ] **Step 2: Add the dep to `paigasus-proto`**

In `rs/crates/libs/paigasus-proto/moon.yml`, add a `lint` task under `tasks:`:

```yaml
  # `contracts:generate` declares no `outputs:` but writes this crate's src/generated/**, so
  # without this dep Moon may run `buf generate` concurrently with the clippy that is compiling
  # those very files. build and test already order against it; lint must too, now that SMA-526
  # gives lint `^:build` and puts more Rust tasks in flight at once. Mirrors the ts sibling,
  # which wires contracts:generate into build, typecheck and test.
  lint:
    deps: ['contracts:generate']
```

- [ ] **Step 3: Add the same to `paigasus-service-info`**

In `rs/crates/libs/paigasus-service-info/moon.yml`, add under `tasks:`:

```yaml
  # Mirrors build/test: this crate consumes paigasus-proto's generated types, so its lint must not
  # race `buf generate` either (SMA-526).
  lint:
    deps: ['contracts:generate']
```

- [ ] **Step 4: Verify Moon MERGED the deps rather than replacing the inherited `^:build`**

This is the step that decides whether the task is done. Moon's default `mergeDeps` is `append`, but it must be confirmed, not assumed — if the project-level `deps` *replaced* the inherited one, the propagation from Task 2 is silently gone for these two crates.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query projects --json \
  | python3 -c 'import sys,json;d=json.load(sys.stdin);[print(p["id"],[x if isinstance(x,str) else x.get("target") for x in ((p.get("tasks") or {}).get("lint") or {}).get("deps") or []]) for p in d["projects"] if p["id"] in ("paigasus-proto-rs","paigasus-service-info-rs")]'
```

Expected: **both** entries present in each list —
`paigasus-proto-rs ['contracts:generate', 'paigasus-proto-derive-rs:build']` and
`paigasus-service-info-rs ['contracts:generate', 'paigasus-proto-rs:build']` (order may differ).

**If `^:build` was dropped**, change both files to declare it explicitly instead:

```yaml
  lint:
    deps: ['contracts:generate', '^:build']
```

then re-run this step and confirm both appear.

- [ ] **Step 5: Confirm the parity gate still passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py; echo "rc=$?"
```

Expected: rc=0. A3 checks that each upstream's `:build` is in lint's deps — adding `contracts:generate` must not have displaced it. If this reds, Step 4's merge check was misread.

- [ ] **Step 6: Confirm the affected-graph guard still passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/affected-graph/run.sh; echo "rc=$?"
```

Expected: rc=0. `contracts` is not a `build`/`test`/`lint` target, so the twelve-target expected set from Task 3 is unchanged.

- [ ] **Step 7: Commit**

```bash
git add rs/crates/libs/paigasus-proto/moon.yml rs/crates/libs/paigasus-service-info/moon.yml
git commit -m "fix(rs): order proto lint behind contracts:generate (SMA-526)" -m "contracts:generate declares no outputs but writes paigasus-proto/src/generated/**, so Moon was
free to run buf generate concurrently with the clippy compiling those files. build and test
already ordered against it and the ts sibling wires it into build, typecheck and test; lint was
the outlier, and giving lint ^:build puts more Rust tasks in flight at once.

Pre-existing race, cheap to close while lint's graph is being changed anyway."
```

---

### Task 5: Re-key the `rs/target` cache so the added clippy artifacts are saved

The cache key hashes only `rs/rust-toolchain.toml` and `rs/Cargo.lock`, neither of which this branch touches. `actions/cache` skips its post-job save on a primary-key hit, so the clippy-driver artifacts the new downstream lint tasks produce would never be persisted — a cold rebuild on every run, indefinitely. A verification dispatch cannot reveal this: feature branches read the base branch's cache scope, hit the same key, and the cold compile reads as ordinary first-run cost (SMA-520).

**Files:**
- Modify: `.github/workflows/ci.yml` (~lines 86-93, the `Cache Rust build artifacts` step)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: a cache key whose literal segment changes with this PR, so the first post-merge run writes a new entry.

- [ ] **Step 1: Read the current step to get the exact text**

```bash
sed -n '78,95p' .github/workflows/ci.yml
```

Confirm the `key:` line ends `-line-tables-only-${{ hashFiles('rs/Cargo.lock') }}` and `restore-keys:` ends `-line-tables-only-`.

- [ ] **Step 2: Add the discriminator to BOTH `key` and `restore-keys`**

Both must change. Changing only `key` leaves the old `restore-keys` prefix matching the stale entry, which restores the pre-change target and defeats the point.

```yaml
          # Toolchain hash in the key + restore-key prefix so a rustc bump (rs/rust-toolchain.toml)
          # can't restore stale cross-version target artifacts (E0514 on prost/tonic rmeta, SMA-389).
          # `line-tables-only` segment so a cache written before the CARGO_PROFILE_*_DEBUG trim
          # above can't restore a stale full-debuginfo rs/target and reintroduce the disk
          # pressure that trim exists to fix.
          # `lint-deps` segment (SMA-526): giving lint `^:build` makes downstream crates run clippy,
          # which writes clippy-driver artifacts `cargo build` never produces. Neither
          # rust-toolchain.toml nor Cargo.lock changed, so without a new literal segment the primary
          # key still hits the pre-change entry — and actions/cache SKIPS its save on a primary-key
          # hit, so the enlarged target would never be written. Cold rebuild every run, forever.
          key: rust-${{ runner.os }}-${{ hashFiles('rs/rust-toolchain.toml') }}-line-tables-only-lint-deps-${{ hashFiles('rs/Cargo.lock') }}
          restore-keys: |
            rust-${{ runner.os }}-${{ hashFiles('rs/rust-toolchain.toml') }}-line-tables-only-lint-deps-
```

- [ ] **Step 3: Verify the YAML still parses and both lines changed**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml')); print('yaml ok')"
grep -n 'lint-deps' .github/workflows/ci.yml
```

Expected: `yaml ok`, and **exactly two** matching lines — one `key:`, one under `restore-keys:`. If only one matches, Step 2 was applied incompletely.

- [ ] **Step 4: Verify the affected-graph guard's `--include-relations` assertion still holds**

The guard greps `ci.yml` for `moon ci "` invocations; confirm the edit did not disturb them.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/affected-graph/run.sh; echo "rc=$?"
```

Expected: rc=0, including `PASS  ci-include-relations`.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci(repo): re-key the rs/target cache for the widened clippy graph (SMA-526)" -m "Giving lint ^:build makes downstream crates run clippy, producing clippy-driver artifacts that
cargo build does not. The cache key hashes only rust-toolchain.toml and Cargo.lock, neither of
which this branch touches, so the primary key still hits the pre-change entry — and actions/cache
skips its post-job save on a primary-key hit. The enlarged rs/target would never be saved: a cold
rebuild on every run until the lockfile happened to move.

A verification dispatch cannot reveal this; feature branches read the base scope, hit the same
key, and the cold compile reads as ordinary first-run cost. Precedent for the literal
discriminator is the existing line-tables-only segment."
```

---

### Task 6: Full-graph verification and follow-up capture

No production change. Runs the gate list CI runs, and records the follow-ups the spec identified so they are not lost.

**Files:**
- None modified unless a gate reds.

**Interfaces:**
- Consumes: Tasks 1-5.
- Produces: a green full graph, and Linear follow-ups.

- [ ] **Step 1: Run the full CI gate list (spec V6)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site \
  :promtool :observability-drift :nats-permissions :release-parity :release-parity-py \
  :release-parity-ts --base origin/main --include-relations
```

Expected: all green.

**Diagnosing a red:**
- Moon reports "N failed" without naming the task. Get the names with:
  `jq '.actions[]|select(.status=="failed")|.label' .moon/cache/ciReport.json`
- `paigasus-iam-rs:test` is Docker-gated and documented as flaky under parallel load — a different random subset fails per run with `postgres did not accept connections within 60s`. Re-run before concluding it is this change. If it persists, compare against a baseline run on unmodified `origin/main`.
- This branch edits `.moon/tasks/rust.yml`, which is in `implicitInputs`, so this is a 70-task / 16-project run — larger than a normal PR, against `timeout-minutes: 30` in CI.

- [ ] **Step 2: Confirm the blast radius matches the spec's V5 figure**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
printf '.moon/tasks/rust.yml\n' | moon query tasks --affected --downstream deep \
  | python3 -c 'import sys,json;d=json.load(sys.stdin);t=d.get("tasks") or {};print(sum(len(v) for v in t.values()),"tasks across",len(t),"projects")'
```

Expected: `70 tasks across 16 projects`. A materially larger number means something beyond the Rust stack was pulled in — investigate before opening the PR.

- [ ] **Step 3: Confirm the working tree is clean and no scratch files leaked**

```bash
git status --short
```

Expected: empty. In particular there must be no `measure.sh`, `cost*.sh` or `verify*.py` at the repo root.

- [ ] **Step 4: Create the follow-up Linear issues**

The spec records four holes it deliberately does not close. Create one Linear issue each, in the `Sven Maschek` team, project `Paigasus Polyglot`, linked as related to SMA-526:

1. **Dependency-bump clippy breaks are unlinted.** A `rs/Cargo.lock`-only touch schedules no crate task at all — only `repo:deny`, `repo:nats-permissions`, `repo:wasm-getrandom-free`. `lint`'s inputs are `@group(sources)`, `@group(tests)` and the project-local `Cargo.toml`; the workspace lockfile is not among them. So a dep bump that deprecates an API is not linted even in the crate that uses it. Fixing it means adding `/rs/Cargo.lock` to lint's inputs, which lints all 13 crates on every Dependabot PR — a spend decision.
2. **`py:typecheck` does not propagate from Rust.** `py:lint`/`py:typecheck` live on the `py` configuration root, which has no `dependsOn` to any Rust project and no `rs/**` inputs, so a kernel edit schedules neither. Only `paigasus-kernel-py:test`'s hand-written `/rs/...` inputs close the gap, and only at runtime: a PyO3 signature change pytest does not exercise is a basedpyright finding that never runs on the introducing PR.
3. **`typecheck` in `typescript-project.yml` carries no `deps`** and structurally cannot propagate. It is currently harmless only because `paigasus-kernel-ts` overrides `build` with a script ending in `tsc` and no other ts package consumes the kernel bindings — but that file itself warns `build` is the override surface, and `paigasus-console-ts` already replaces it with `next build`.
4. **Two input gaps:** `cargo clippy --all-targets` compiles `build.rs`, but `rs/crates/bindings/paigasus-node-bindings/build.rs` is in no task's inputs; and `rs/rustfmt.toml` is in no `fmt` task's inputs, so a global format-config change re-keys nothing.

- [ ] **Step 5: No commit**

This task produces no repo change. If Step 1 required a fix, commit that fix with a `fix(...)` or `ci(...)` message naming the gate it repaired, then re-run Step 1.

---

## Self-Review

**Spec coverage:**

| Spec item | Task |
|---|---|
| Scope 1 — `deps: ['^:build']` on `lint` | Task 2 |
| Scope 2 — A3 widened, self-test fixtures, absent-key message, remediation text | Task 1 |
| Scope 3 — `run.sh` filter, expected set, comment, diagnostic | Task 3 |
| Scope 4 — `contracts:generate` on lint for proto + service-info | Task 4 |
| Scope 5 — `ci.yml` cache key discriminator | Task 5 |
| Scope 6 — `ci/affected-graph/README.md` | Task 3, Step 8 |
| V1 lint propagation measured | Task 2, Step 5 |
| V2 A3 not vacuous (17 violations) | Task 1, Step 7 |
| V3 cost | Measured pre-spec; no task (nothing to re-verify) |
| V4 negative control passes | Task 3, Step 7 |
| V5 blast radius 70/16 | Task 6, Step 2 |
| V6 full CI graph | Task 6, Step 1 |
| Out-of-scope follow-ups | Task 6, Step 4 |
| Rollback | Spec only; config-only single-branch revert, no task needed |

No gaps.

**Placeholder scan:** none — every step carries the literal code, the exact command, and the expected output.

**Type consistency:** `check()` keeps its `(a1, a2, a3)` return across Tasks 1 and 4. The two A3 message shapes introduced in Task 1 Step 5 are matched only by the self-test added in Task 1 Step 3 (`"has no \`lint\` task"`), and that substring is present in the emitted string. Task 3's expected-set strings are `project:task` targets produced by `moon query tasks`, independent of the Python messages.

**Ordering:** Task 1 is deliberately red against the real graph and Task 2 turns it green — the plan's red/green cycle. Task 3 must follow Task 2, because the expected set can only be re-baselined once propagation exists. Task 4 must follow Task 2, because its merge check asserts the inherited `^:build` survived. Tasks 5 and 6 are independent of each other but 6 must run last.

---

## Corrections found during execution

This plan is a historical record of what was planned; the task steps above are left as written.
Execution surfaced three predictions that did not hold:

- **Task 3, Step 1** predicted `ci/affected-graph/run.sh` would FAIL before any edit. It PASSES
  instead: the pre-edit task-name filter strips `lint` rows before the comparison runs, so the
  four new lint targets never reach the diff as `unexpected`. The filter has to be widened
  (Step 2) before the guard can even see them, let alone fail on them.

- **Task 4, Steps 1 and 4** give `moon query projects --json`. On Moon 2.3.2 this errors —
  `unexpected argument '--json' found` — because `moon query projects` already emits JSON
  unconditionally and the flag does not exist. Drop `--json` from both commands.

- **Task 5, Step 3** expects "exactly two" matching lines for
  `grep -n 'lint-deps' .github/workflows/ci.yml`. The correct count is three: the Step 2 comment
  text itself contains the literal string `lint-deps` (in "`lint-deps` segment (SMA-526): ..."),
  in addition to the two structural lines (`key:` and `restore-keys:`) that actually matter.
