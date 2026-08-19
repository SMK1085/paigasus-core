# SMA-528 — moon ci downstream cascade Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a change to an upstream Rust crate actually schedule its downstream consumers'
`build`/`test`/`lint` under the `moon ci` command CI runs, and gate that property so it cannot
silently regress.

**Architecture:** Moon 2.3.2 marks a task affected **only** when the task's own declared `inputs`
match a changed file — graph flags (`--include-relations`, `--downstream`) never confer
affectedness. So each Rust crate declares a `fileGroups.upstreams` listing its transitive
`dependsOn` closure's sources, and `.moon/tasks/rust.yml` references `@group(upstreams)` once from
`build`/`test`/`lint`. A new assertion **A6** in `cargo_moon_parity.py` holds the declarations to
strict equality with the closure moon itself reports.

**Tech Stack:** Moon 2.3.2 (pinned via proto), Python 3 (gate scripts, stdlib only), Bash
(`ci/affected-graph/run.sh`), GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-08-19-sma-528-moon-ci-downstream-cascade-design.md`

## Global Constraints

- **Every source file opens with an SPDX header:** `// SPDX-License-Identifier: Apache-2.0`
  (`#` for Python/YAML/Bash). Files edited here already have one — do not remove it.
- **Moon is 2.3.2.** Never parse `moon.yml` in a gate; always read `moon query projects`, which
  reports the *resolved* graph.
- **PATH:** every shell step must be prefixed with
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` — the Bash tool's PATH lacks the
  proto-managed CLIs and would silently resolve a different `moon`.
- **Conventional commits with a workspace scope**, subject **starting lowercase**, **≤100 chars**.
  No `#NNN` issue refs in the body (commitlint reads them as a footer and fails
  `footer-leading-blank`). Write "SMA-528" in the subject's trailing parens.
- **Do not use `git commit --no-verify`.** The worktree is already provisioned.
- **Work in the worktree** `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-528`
  on branch `feature/sma-528-moon-ci-downstream-cascade`. Confirm with `git rev-parse
  --abbrev-ref HEAD` before the first commit.
- **Landing order is load-bearing** (spec §4.8): Task 1 (declare groups, inert) must precede Task 3
  (`@group(upstreams)` reference). Reversing them makes graph load fail for *every* moon command
  repo-wide, and the first visible symptom is `ci.yml`'s earlier `moon run ts:commitlint` step
  failing with `project::unknown_file_group`.
- **Do not push between Task 2 and Task 3.** Task 2 deliberately commits a red gate (TDD
  red-before-green, spec §6.0); Task 3 makes it green.

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `rs/crates/{libs,bindings,services}/*/moon.yml` (13 files) | Each declares `fileGroups.upstreams` — its transitive `dependsOn` closure's sources | 1 |
| `.moon/templates/rust/moon.yml` | Scaffold emits the group so a generated crate loads | 1 |
| `.moon/tasks/rust.yml` | References `@group(upstreams)` once, from `build`/`test`/`lint` | 3 |
| `ci/affected-graph/run.sh` | `assert_task_case_ci`, `expect_red_task`, the new case, re-pointed comments | 2, 4, 7 |
| `ci/affected-graph/cargo_moon_parity.py` | A6 + `moon_projects()` capturing `inputGlobs` and `language` | 5 |
| `.github/workflows/ci.yml` | `rs/target` cache-key discriminator | 6 |
| `ci/affected-graph/README.md`, `CLAUDE.md`, `rs/crates/services/paigasus-iam/moon.yml`, `ci/error-registry/README.md` | Docs + correcting claims that F1 makes false | 7 |

---

## Reference data (computed from `moon query projects`, do not re-derive by hand)

The transitive `dependsOn` closure restricted to `language: rust`, excluding `contracts`
(`NON_CARGO_PARENTS`). **22 edges across 13 crates → 44 input entries.**

| crate | `moon.yml` path | closure |
|---|---|---|
| `paigasus-kernel-rs` | `rs/crates/libs/paigasus-kernel` | *(leaf)* |
| `paigasus-logging-rs` | `rs/crates/libs/paigasus-logging` | *(leaf)* |
| `paigasus-proto-derive-rs` | `rs/crates/libs/paigasus-proto-derive` | *(leaf)* |
| `paigasus-iam-core-rs` | `rs/crates/libs/paigasus-iam-core` | kernel |
| `paigasus-kernel-parity-rs` | `rs/crates/libs/paigasus-kernel-parity` | kernel |
| `paigasus-observability-rs` | `rs/crates/libs/paigasus-observability` | kernel |
| `paigasus-node-bindings-rs` | `rs/crates/bindings/paigasus-node-bindings` | kernel |
| `paigasus-py-bindings-rs` | `rs/crates/bindings/paigasus-py-bindings` | kernel |
| `paigasus-wasm-rs` | `rs/crates/bindings/paigasus-wasm` | kernel |
| `paigasus-proto-rs` | `rs/crates/libs/paigasus-proto` | proto-derive |
| `paigasus-service-info-rs` | `rs/crates/libs/paigasus-service-info` | proto, proto-derive |
| `paigasus-gateway-rs` | `rs/crates/services/paigasus-gateway` | kernel, logging, observability, proto, proto-derive, service-info |
| `paigasus-iam-rs` | `rs/crates/services/paigasus-iam` | iam-core, kernel, logging, observability, proto, proto-derive, service-info |

Source dirs, for building the glob strings:

```
paigasus-kernel-rs         rs/crates/libs/paigasus-kernel
paigasus-logging-rs        rs/crates/libs/paigasus-logging
paigasus-proto-derive-rs   rs/crates/libs/paigasus-proto-derive
paigasus-proto-rs          rs/crates/libs/paigasus-proto
paigasus-service-info-rs   rs/crates/libs/paigasus-service-info
paigasus-iam-core-rs       rs/crates/libs/paigasus-iam-core
paigasus-observability-rs  rs/crates/libs/paigasus-observability
```

Each closure member contributes **exactly two** entries, in this order:

```yaml
    - '/<source_dir>/src/**/*'
    - '/<source_dir>/Cargo.toml'
```

The brace form `{src/**/*,Cargo.toml}` is **forbidden** (spec §3.3): it would move the manifest from
`inputFiles` into `inputGlobs` and change which bucket A6 must read.

---

### Task 1: Declare `fileGroups.upstreams` on every Rust crate (inert)

Nothing consumes the group yet, so this task cannot change any behaviour. That is the point — it is
the half of the landing order that must go first.

**Files:**
- Modify: all 13 `rs/crates/{libs,bindings,services}/*/moon.yml`
- Modify: `.moon/templates/rust/moon.yml`

**Interfaces:**
- Consumes: nothing.
- Produces: a `fileGroups.upstreams` key on every Rust project, readable via
  `moon query projects` → `projects[].fileGroups.upstreams`. Task 3 references it as
  `@group(upstreams)`; Task 5's A6 asserts it.

- [ ] **Step 1: Record the pre-change baseline so Step 4 can prove inertness**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-528
echo "rs/crates/libs/paigasus-kernel/src/lib.rs" \
  | moon query tasks --affected > /tmp/sma528-before.json
python3 -c "
import json;d=json.load(open('/tmp/sma528-before.json'))
print(sorted(f'{p}:{t}' for p,ts in (d.get('tasks') or {}).items() for t in ts))"
```

Expected (this is the broken state being fixed — no consumer tasks):
```
['paigasus-kernel-py:test', 'paigasus-kernel-rs:build', 'paigasus-kernel-rs:build-release',
 'paigasus-kernel-rs:fmt', 'paigasus-kernel-rs:lint', 'paigasus-kernel-rs:test',
 'paigasus-kernel-ts:build', 'paigasus-kernel-ts:test', 'repo:actionlint',
 'repo:error-code-single-site', 'repo:machete', 'repo:parity-corpus-drift',
 'repo:publish-metadata', 'repo:wasm-getrandom-free']
```

- [ ] **Step 2: Add the group to the three leaf crates**

In each of `rs/crates/libs/paigasus-kernel/moon.yml`,
`rs/crates/libs/paigasus-logging/moon.yml`, `rs/crates/libs/paigasus-proto-derive/moon.yml`,
insert directly **above** the existing `tasks:` key (if a file has no `tasks:` key, append at
end-of-file):

```yaml
# SMA-528 — the transitive dependsOn closure's sources, consumed by build/test/lint via
# `@group(upstreams)` in .moon/tasks/rust.yml. Moon confers affectedness ONLY through a task's own
# `inputs`; dependsOn and `^:build` schedule an upstream but never select this crate. Empty because
# this crate has no in-tree dependencies. Asserted to strict equality by repo:affected-smoke's A6.
fileGroups:
  upstreams: []
```

- [ ] **Step 3: Add the group to the ten crates that have a closure**

Use the reference table above. Each entry is two lines, closure members in alphabetical order by
source dir. The comment header is the same as Step 2's minus the "Empty because" sentence.

`rs/crates/libs/paigasus-iam-core/moon.yml`, `rs/crates/libs/paigasus-kernel-parity/moon.yml`,
`rs/crates/libs/paigasus-observability/moon.yml`,
`rs/crates/bindings/paigasus-node-bindings/moon.yml`,
`rs/crates/bindings/paigasus-py-bindings/moon.yml`, `rs/crates/bindings/paigasus-wasm/moon.yml`
all get exactly:

```yaml
fileGroups:
  upstreams:
    - '/rs/crates/libs/paigasus-kernel/src/**/*'
    - '/rs/crates/libs/paigasus-kernel/Cargo.toml'
```

`rs/crates/libs/paigasus-proto/moon.yml`:

```yaml
fileGroups:
  upstreams:
    - '/rs/crates/libs/paigasus-proto-derive/src/**/*'
    - '/rs/crates/libs/paigasus-proto-derive/Cargo.toml'
```

`rs/crates/libs/paigasus-service-info/moon.yml`:

```yaml
fileGroups:
  upstreams:
    - '/rs/crates/libs/paigasus-proto-derive/src/**/*'
    - '/rs/crates/libs/paigasus-proto-derive/Cargo.toml'
    - '/rs/crates/libs/paigasus-proto/src/**/*'
    - '/rs/crates/libs/paigasus-proto/Cargo.toml'
```

`rs/crates/services/paigasus-gateway/moon.yml`:

```yaml
fileGroups:
  upstreams:
    - '/rs/crates/libs/paigasus-kernel/src/**/*'
    - '/rs/crates/libs/paigasus-kernel/Cargo.toml'
    - '/rs/crates/libs/paigasus-logging/src/**/*'
    - '/rs/crates/libs/paigasus-logging/Cargo.toml'
    - '/rs/crates/libs/paigasus-observability/src/**/*'
    - '/rs/crates/libs/paigasus-observability/Cargo.toml'
    - '/rs/crates/libs/paigasus-proto-derive/src/**/*'
    - '/rs/crates/libs/paigasus-proto-derive/Cargo.toml'
    - '/rs/crates/libs/paigasus-proto/src/**/*'
    - '/rs/crates/libs/paigasus-proto/Cargo.toml'
    - '/rs/crates/libs/paigasus-service-info/src/**/*'
    - '/rs/crates/libs/paigasus-service-info/Cargo.toml'
```

`rs/crates/services/paigasus-iam/moon.yml` — the gateway list **plus** iam-core:

```yaml
fileGroups:
  upstreams:
    - '/rs/crates/libs/paigasus-iam-core/src/**/*'
    - '/rs/crates/libs/paigasus-iam-core/Cargo.toml'
    - '/rs/crates/libs/paigasus-kernel/src/**/*'
    - '/rs/crates/libs/paigasus-kernel/Cargo.toml'
    - '/rs/crates/libs/paigasus-logging/src/**/*'
    - '/rs/crates/libs/paigasus-logging/Cargo.toml'
    - '/rs/crates/libs/paigasus-observability/src/**/*'
    - '/rs/crates/libs/paigasus-observability/Cargo.toml'
    - '/rs/crates/libs/paigasus-proto-derive/src/**/*'
    - '/rs/crates/libs/paigasus-proto-derive/Cargo.toml'
    - '/rs/crates/libs/paigasus-proto/src/**/*'
    - '/rs/crates/libs/paigasus-proto/Cargo.toml'
    - '/rs/crates/libs/paigasus-service-info/src/**/*'
    - '/rs/crates/libs/paigasus-service-info/Cargo.toml'
```

- [ ] **Step 4: Update the scaffold template**

`.moon/templates/rust/moon.yml` currently ends after the conditional `dependsOn` block. Append:

```yaml
# SMA-528 — every Rust crate MUST declare this group: .moon/tasks/rust.yml references
# `@group(upstreams)` from build/test/lint, and a missing group is a hard graph-load error
# (`project::unknown_file_group`) for every moon command, repo-wide. A generated crate therefore
# ships one from the start. Keep it in sync with `dependsOn` above — repo:affected-smoke's A6
# asserts the two agree to strict equality.
fileGroups:
{%- if archetype == "service" %}
  upstreams:
    - '/rs/crates/libs/paigasus-kernel/src/**/*'
    - '/rs/crates/libs/paigasus-kernel/Cargo.toml'
    - '/rs/crates/libs/paigasus-proto-derive/src/**/*'
    - '/rs/crates/libs/paigasus-proto-derive/Cargo.toml'
    - '/rs/crates/libs/paigasus-proto/src/**/*'
    - '/rs/crates/libs/paigasus-proto/Cargo.toml'

# A3 requires build and test to schedule each upstream's build. Without these a generated service
# reds repo:affected-smoke on its first run.
tasks:
  build:
    deps: ['^:build']
  test:
    deps: ['^:build']
{%- else %}
  upstreams: []
{%- endif %}
```

Note `paigasus-proto-derive` is in the service closure because it is transitively reachable through
`paigasus-proto-rs`, even though the template's `dependsOn` names only `paigasus-proto-rs` and
`paigasus-kernel-rs`.

- [ ] **Step 5: Verify the graph still loads and behaviour is unchanged**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-528
moon query projects >/dev/null && echo "GRAPH OK"
echo "rs/crates/libs/paigasus-kernel/src/lib.rs" \
  | moon query tasks --affected > /tmp/sma528-after1.json
python3 -c "
import json
b=json.load(open('/tmp/sma528-before.json')); a=json.load(open('/tmp/sma528-after1.json'))
f=lambda d: sorted(f'{p}:{t}' for p,ts in (d.get('tasks') or {}).items() for t in ts)
print('UNCHANGED' if f(b)==f(a) else f'CHANGED:\n  before={f(b)}\n  after={f(a)}')"
```

Expected: `GRAPH OK` then `UNCHANGED`. A `CHANGED` result means the group is being consumed
somewhere it should not be yet — stop and investigate before continuing.

- [ ] **Step 6: Verify the gate suite is still green**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && ci/affected-graph/run.sh`
Expected: `== affected-graph cascade intact ==`

- [ ] **Step 7: Commit**

```bash
git add rs/crates .moon/templates/rust/moon.yml
git commit -m "feat(repo): declare each Rust crate's upstream closure as a fileGroup (SMA-528)"
```

---

### Task 2: RED — pin the kernel cascade before fixing it

Spec §6.0. The failure mode of this whole guard family is "snapshot whatever moon printed", so the
case is written from the closure **first** and must fail on the current tree.

**Files:**
- Modify: `ci/affected-graph/run.sh`

**Interfaces:**
- Consumes: nothing from Task 1 (the case reads moon's graph directly).
- Produces: `assert_task_case_ci LABEL FILE EXPECTED_CSV` and
  `run_task_case_ci LABEL FILE EXPECTED_CSV` (3-way rc folding, mirroring `run_task_case`), plus
  `expect_red_task LABEL FILE EXPECTED_CSV` for the negative-control block. Task 4 reuses all three.

- [ ] **Step 1: Add the CI-traversal helper**

Insert into `ci/affected-graph/run.sh` immediately after the existing `assert_task_case`:

```bash
# assert_task_case_ci LABEL FILE EXPECTED_CSV
#   Same strict-equality contract as assert_task_case, but over the traversal `moon ci` ACTUALLY
#   USES: `moon query tasks --affected` with NO graph flags.
#
#   Why both exist (SMA-528). Moon 2.3.2 confers affectedness ONLY through a task's own `inputs`.
#   `--downstream deep` walks dependents in the QUERY, but `moon ci` never does — measured at the
#   full 24-target ci.yml shape, `moon ci "${T[@]}" --stdin --include-relations` and the same
#   command plus `--downstream deep` produce byte-identical action sets. So the `_deep` cases
#   assert what the task graph WOULD cascade (which is what catches a deleted `^:build`), and these
#   `_ci` cases assert what CI actually selects. Before SMA-528 only the former existed, and it was
#   green for years while no consumer test ran.
#
#   This traversal is a CHARACTERIZED proxy, not `moon ci` itself. Measured relationship:
#       moon ci RunTask set = (query-affected ∩ ci.yml's T array ∩ runInCI) ∪ upstream-dep closure
#   Both differences are benign here — the T filter only REMOVES tasks these cases do not assert
#   (`build-release`), and the upstream-dep closure only ADDS builds. Moon has no dry-run
#   (`--plan`, `--no-actions` and `--cache` all still execute), so grounding a per-run gate in a
#   real `moon ci` would mean running tasks on a cold CI cache. RE-MEASURE THIS ON A MOON BUMP,
#   alongside A4's `inputFiles` shape and A5's command/args/script shape.
# returns 0 pass / 1 assertion fail / 2 infrastructure error
assert_task_case_ci() {
  local label="$1" file="$2" expected_csv="$3" got want missing unexpected
  [ -n "$expected_csv" ] || { echo "FATAL [$label]: EXPECTED_CSV is empty (harness bug)" >&2; return 2; }
  got="$(printf '%s\n' "$file" \
    | moon query tasks --affected \
    | python3 -c '
import sys, json
d = json.load(sys.stdin)
out = []
for pid, tasks in (d.get("tasks") or {}).items():
    for name in tasks:
        if name in ("build", "test", "lint"):
            out.append(f"{pid}:{name}")
print("\n".join(sorted(out)))')" \
    || { echo "FATAL [$label]: moon query tasks failed" >&2; return 2; }
  want="$(tr ',' '\n' <<<"$expected_csv" | sort)"
  if [ "$got" = "$want" ]; then
    printf 'PASS  %-22s -> %s\n' "$label" "$(tr '\n' ' ' <<<"$got")"
    return 0
  fi
  missing="$(comm -23 <(printf '%s\n' "$want") <(printf '%s\n' "$got"))"
  unexpected="$(comm -13 <(printf '%s\n' "$want") <(printf '%s\n' "$got"))"
  echo "FAIL  [$label] CI-traversal TASK set != expected set" >&2
  if [ -n "$missing" ]; then
    echo "  missing  (expected but NOT selected by the traversal moon ci uses — the consumer's" >&2
    echo "  build/test/lint does not key on this upstream's sources; check its fileGroups.upstreams" >&2
    echo "  and that .moon/tasks/rust.yml still references @group(upstreams)):" >&2
    sed 's/^/    /' <<<"$missing" >&2
  fi
  if [ -n "$unexpected" ]; then
    echo "  unexpected (selected but not expected — if the new edge is intended, add it here):" >&2
    sed 's/^/    /' <<<"$unexpected" >&2
  fi
  return 1
}
```

- [ ] **Step 2: Add the rc-folding wrapper**

Insert immediately after the existing `run_task_case`:

```bash
# CI-traversal twin of run_task_case — same 3-way return-code folding.
run_task_case_ci() {
  local ec=0
  assert_task_case_ci "$@" || ec=$?
  case "$ec" in
    0) ;;
    1) SUITE_RC=1 ;;
    *) echo "== affected-graph guard ABORTED: infrastructure error (rc=$ec) ==" >&2; exit 2 ;;
  esac
}
```

- [ ] **Step 3: Add the new case to `run_suite`**

Insert at the end of `run_suite`, immediately **before** the `assert_cargo_moon_parity` line.

The expected set is derived from the closure, not pasted from output: every crate whose
`upstreams` closure contains `paigasus-kernel-rs` contributes `build`, `test` and `lint`
(gateway, iam-core, iam, kernel-parity, node-bindings, observability, py-bindings, wasm = 8 × 3);
the kernel itself contributes its own three (`fmt`/`build-release` are outside the name filter);
and the two language wrappers contribute the tasks that compile the FFI artefacts
(`kernel-ts:{build,test}`, `kernel-py:test`, hand-declared since SMA-420/546). **30 rows.**

```bash
  # SMA-528 — a kernel SOURCE edit must select every consumer's build/test/lint under the traversal
  # `moon ci` uses. This is the case the issue exists for: before SMA-528 a kernel behavioural
  # change ran the kernel's own tests and NOT ONE consumer's, including paigasus-kernel-parity-rs,
  # the ADR-0005 cross-binding harness that exists precisely to catch kernel drift.
  # kernel-ts:{build,test} and kernel-py:test are the FFI tasks; they key on the kernel's sources by
  # hand (SMA-420/546) rather than through @group(upstreams), which is Rust-only.
  run_task_case_ci "kernel->consumer-tasks" "rs/crates/libs/paigasus-kernel/src/lib.rs" \
    "paigasus-gateway-rs:build,paigasus-gateway-rs:test,paigasus-gateway-rs:lint,paigasus-iam-core-rs:build,paigasus-iam-core-rs:test,paigasus-iam-core-rs:lint,paigasus-iam-rs:build,paigasus-iam-rs:test,paigasus-iam-rs:lint,paigasus-kernel-parity-rs:build,paigasus-kernel-parity-rs:test,paigasus-kernel-parity-rs:lint,paigasus-node-bindings-rs:build,paigasus-node-bindings-rs:test,paigasus-node-bindings-rs:lint,paigasus-observability-rs:build,paigasus-observability-rs:test,paigasus-observability-rs:lint,paigasus-py-bindings-rs:build,paigasus-py-bindings-rs:test,paigasus-py-bindings-rs:lint,paigasus-wasm-rs:build,paigasus-wasm-rs:test,paigasus-wasm-rs:lint,paigasus-kernel-rs:build,paigasus-kernel-rs:test,paigasus-kernel-rs:lint,paigasus-kernel-ts:build,paigasus-kernel-ts:test,paigasus-kernel-py:test"
```

- [ ] **Step 4: Run the suite and verify it FAILS with the right diagnosis**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && ci/affected-graph/run.sh; echo "rc=$?"`

Expected: `rc=1`, and a `FAIL [kernel->consumer-tasks]` block whose `missing` list contains
exactly the 24 consumer rows (8 crates × build/test/lint) — kernel-rs, kernel-ts and kernel-py rows
must **not** be missing, because those are already affected today.

If any of the 24 is *not* listed as missing, the expected set is wrong — fix the set, not the
observation. If `unexpected` is non-empty, a project is being selected that the closure does not
explain; stop and investigate.

- [ ] **Step 5: Commit the red gate**

```bash
git add ci/affected-graph/run.sh
git commit -m "test(repo): pin the kernel consumer cascade before fixing it (SMA-528)"
```

**Do not push.** The next task makes this green.

---

### Task 3: GREEN — reference `@group(upstreams)` from the shared task defs

**Files:**
- Modify: `.moon/tasks/rust.yml`

**Interfaces:**
- Consumes: `fileGroups.upstreams` from Task 1; the failing case from Task 2.
- Produces: `build`/`test`/`lint` on every Rust crate now key on their closure's sources.

- [ ] **Step 1: Add the group reference to all three tasks**

In `.moon/tasks/rust.yml`, add `'@group(upstreams)'` as the last entry of each task's `inputs`.
`build` and `test` use the inline list form; `lint` uses the block form.

```yaml
  build:
    command: 'cargo build'
    inputs: ['@group(sources)', 'Cargo.toml', '@group(upstreams)']
  build-release:
    command: 'cargo build --release'
    inputs: ['@group(sources)', 'Cargo.toml']
  test:
    command: 'cargo nextest run --no-tests=pass'
    inputs: ['@group(sources)', '@group(tests)', 'Cargo.toml', '/rs/.config/nextest.toml', '@group(upstreams)']
```

and, appended to `lint`'s existing `inputs:` block:

```yaml
      - '@group(upstreams)'
```

Add this comment directly above the `tasks:` key:

```yaml
# `@group(upstreams)` (SMA-528) is each crate's transitive dependsOn closure, declared per-crate in
# its own moon.yml. It is what makes a CONSUMER affected by an UPSTREAM source change: Moon 2.3.2
# confers affectedness only through a task's own `inputs`, so `dependsOn` and the `^:build` below
# schedule an upstream's build but never SELECT this crate. Without it `moon ci` ran a kernel edit's
# own tests and no consumer's — including paigasus-kernel-parity-rs, the ADR-0005 harness.
# It also fixes a cache-correctness bug that is independent of scheduling: a consumer whose inputs
# omit its upstreams replays a cached PASS built against a DIFFERENT upstream.
# Deliberately NOT on `fmt` (crate-local by construction) or `build-release` (never runs in CI);
# neither carries `^:build` either.
```

- [ ] **Step 2: Verify the previously-red case now passes**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && ci/affected-graph/run.sh; echo "rc=$?"`

Expected: `PASS  kernel->consumer-tasks -> …` listing all 30 rows.

Other cases may now fail — `proto->service-info-tasks` and `lockfile->all-lint` are re-baselined in
Task 4. Record which ones failed; do not fix them here.

- [ ] **Step 3: Verify the issue's own acceptance check**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
echo "rs/crates/libs/paigasus-kernel/src/lib.rs" | moon query tasks --affected \
  | python3 -c "
import sys,json;d=json.load(sys.stdin)
need={'paigasus-kernel-parity-rs:test','paigasus-iam-rs:test','paigasus-iam-core-rs:test','paigasus-gateway-rs:test'}
got={f'{p}:{t}' for p,ts in (d.get('tasks') or {}).items() for t in ts}
print('MISSING:', need-got or 'none — all four consumer test tasks selected')"
```

Expected: `MISSING: none — all four consumer test tasks selected`

- [ ] **Step 4: Verify cache correctness (spec §6.3) — the primary justification**

A consumer's task hash must move when an upstream source changes. This is checked directly because
spec F5 shows hashes can silently fail to move.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-528
moon run paigasus-kernel-parity-rs:build 2>&1 | grep -oE '\(cached, [0-9a-f]+\)|\(completed' | head -1
printf '\n// SMA-528 cache-key probe\n' >> rs/crates/libs/paigasus-kernel/src/lib.rs
moon run paigasus-kernel-parity-rs:build 2>&1 | tail -3
git checkout -- rs/crates/libs/paigasus-kernel/src/lib.rs
touch rs/crates/libs/paigasus-kernel/src/lib.rs
```

Expected: the second run is **not** reported `cached` — it re-runs. A `cached` result means the
group is not reaching the task; stop and investigate.

The trailing `touch` is required: `git checkout --` restores an **older** mtime than the artefact
built from the probe edit, and cargo's mtime-based incrementality would then reuse a binary built
from the temporary edit.

- [ ] **Step 5: Commit**

```bash
git add .moon/tasks/rust.yml
git commit -m "feat(repo): key each crate's build, test and lint on its upstream sources (SMA-528)"
```

---

### Task 4: Re-baseline the existing task cases and give them negative controls

**Files:**
- Modify: `ci/affected-graph/run.sh`

**Interfaces:**
- Consumes: `assert_task_case_ci` / `run_task_case_ci` from Task 2.
- Produces: `expect_red_task`, used by the negative-control block.

- [ ] **Step 1: Rename the existing deep-traversal cases for clarity**

The existing `run_task_case` invocations keep their traversal but gain explicit labels. Change the
two labels only — not the expected sets:

- `"proto->service-info-tasks"` → `"proto->svc-info-deep"`
- `"lockfile->all-lint"` → **keep this name**. `CLAUDE.md` and
  `ci/affected-graph/README.md` grep for it and the README already documents it as a deliberate
  misnomer; renaming it breaks a documented procedure for no functional gain.

Update the header comment of `assert_task_case` to say it asserts the `--downstream deep`
traversal — *what the task graph would cascade* — and that it remains the only behavioural detector
of a deleted `^:build`, since after SMA-528 affectedness comes from inputs and a missing `^:build`
would not change any `_ci` case's output.

- [ ] **Step 2: Add CI-traversal twins**

Insert after each existing deep case:

```bash
  # CI-traversal twin of proto->svc-info-deep: a proto edit must SELECT the consumers under the
  # traversal moon ci uses, not merely cascade in the task graph. Expected to equal the deep set:
  # every consumer reaches paigasus-proto through @group(upstreams) now.
  run_task_case_ci "proto->svc-info-ci" "rs/crates/libs/paigasus-proto/src/lib.rs" \
    "paigasus-proto-rs:build,paigasus-proto-rs:test,paigasus-proto-rs:lint,paigasus-service-info-rs:build,paigasus-service-info-rs:test,paigasus-service-info-rs:lint,paigasus-iam-rs:build,paigasus-iam-rs:test,paigasus-iam-rs:lint,paigasus-gateway-rs:build,paigasus-gateway-rs:test,paigasus-gateway-rs:lint"
  # CI-traversal twin of lockfile->all-lint. A Cargo.lock touch reaches every crate through `lint`'s
  # workspace inputs (SMA-534) and the three FFI tasks through theirs (SMA-546) — through INPUTS,
  # not dependsOn — so this set is expected to equal the deep one.
  run_task_case_ci "lockfile->all-lint-ci" "rs/Cargo.lock" \
    "paigasus-gateway-rs:lint,paigasus-iam-core-rs:lint,paigasus-iam-rs:lint,paigasus-kernel-parity-rs:lint,paigasus-kernel-py:test,paigasus-kernel-rs:lint,paigasus-kernel-ts:build,paigasus-kernel-ts:test,paigasus-logging-rs:lint,paigasus-node-bindings-rs:lint,paigasus-observability-rs:lint,paigasus-proto-derive-rs:lint,paigasus-proto-rs:lint,paigasus-py-bindings-rs:lint,paigasus-service-info-rs:lint,paigasus-wasm-rs:lint"
```

- [ ] **Step 3: Run the suite and reconcile**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && ci/affected-graph/run.sh; echo "rc=$?"`

Both twins are *predicted* to equal their deep counterparts. If a twin fails, do **not** paste the
observed output over the expectation. Diff the two sets and explain the difference from the closure
first; only then adjust, and record the reason in the case's comment.

The deep `proto->svc-info-deep` case may also now report `unexpected` rows if the upstream inputs
widened its cascade — apply the same discipline.

Expected end state: `rc=0`.

- [ ] **Step 4: Add the task-case negative control helper**

Today's `expect_red` calls `assert_case` (the *project* helper), so no task case has ever had a
negative control. Add inside the `if [ "$NEGATIVE" = 1 ]` block, directly after `expect_red`:

```bash
  # expect_red_task LABEL FILE EXPECTED_CSV — task-case twin of expect_red. Until SMA-528 the task
  # cases had NO negative control at all: expect_red calls assert_case, the project helper, so the
  # proof that a task case can report red was never executed.
  expect_red_task() {
    local rc=0
    assert_task_case_ci "$1" "$2" "$3" || rc=$?
    case "$rc" in
      1) echo "  OK   [$1] task harness reported red as expected" ;;
      0) echo "  FAIL [$1] task harness accepted a wrong expectation" >&2; NEG_RC=1 ;;
      *) echo "  INCONCLUSIVE [$1] infrastructure error (rc=$rc)" >&2; exit 2 ;;
    esac
  }
```

- [ ] **Step 5: Add three controls**

Insert after the existing `expect_red "neg-incomplete-expect" …` line:

```bash
  # 3) a task case must reject a WRONG task: a kernel edit does not select paigasus-proto-rs:lint.
  expect_red_task "neg-task-wrong"      "rs/crates/libs/paigasus-kernel/src/lib.rs" "paigasus-proto-rs:lint"
  # 4) default-deny for task cases: an INCOMPLETE expected set must fail on the extras, exactly as
  #    the project cases do. This is the direction that silently unasserts everything left out.
  expect_red_task "neg-task-incomplete" "rs/crates/libs/paigasus-kernel/src/lib.rs" "paigasus-kernel-rs:build"
  # 5) the regression this issue is about: an expectation that OMITS the consumers must not pass.
  #    If this ever goes green, the cascade is broken again and every other case is lying.
  expect_red_task "neg-task-no-cascade" "rs/crates/libs/paigasus-kernel/src/lib.rs" \
    "paigasus-kernel-rs:build,paigasus-kernel-rs:test,paigasus-kernel-rs:lint,paigasus-kernel-ts:build,paigasus-kernel-ts:test,paigasus-kernel-py:test"
```

- [ ] **Step 6: Run the negative control**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && ci/affected-graph/run.sh --negative-control; echo "rc=$?"`

Expected: `rc=0` and `OK [neg-task-wrong]`, `OK [neg-task-incomplete]`,
`OK [neg-task-no-cascade]` all present.

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/run.sh
git commit -m "test(repo): assert the CI traversal and give task cases negative controls (SMA-528)"
```

---

### Task 5: A6 — gate the declarations against moon's own closure

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py`

**Interfaces:**
- Consumes: `moon_projects()`, `NON_CARGO_PARENTS`, `_allowlisted` (all existing).
- Produces: `check_upstream_inputs(projects, allow=None, floor=None) -> list[str]`, wired into
  `main()` as `a6` and into `self_test()`.

- [ ] **Step 1: Capture `inputGlobs` and `language` in `moon_projects()`**

A6 cannot work without both. `inputFiles` alone would make the `Cargo.toml` half of every pair
unable to fire — plain paths land in `inputFiles`, globs in `inputGlobs` (measured, SMA-534).

In `moon_projects()`, next to the existing `task_inputs[name] = …`, add:

```python
            # SMA-528 — A6 needs BOTH buckets. moon splits resolved inputs by kind: plain paths go
            # to `inputFiles`, globs to `inputGlobs` (measured, SMA-534). `src/**/*` is a glob and
            # `Cargo.toml` is a path, so an A6 that read only one of them could never fire on half
            # of every upstream pair. Same absent-key-is-None contract as `task_inputs`.
            raw_globs = task.get("inputGlobs")
            task_input_globs[name] = None if raw_globs is None else sorted(raw_globs.keys())
```

Declare `task_input_globs = {}` beside the existing `task_inputs = {}`, and add both new keys to
the per-project dict:

```python
            "task_input_globs": task_input_globs,
            "language": p.get("language"),
```

- [ ] **Step 2: Add the module-level tables**

Place after `REQUIRED_FFI_TASKS`:

```python
# SMA-528 — the tasks that must key on their crate's upstream sources. `fmt` is crate-local by
# construction and `build-release` never runs in CI; neither carries `^:build` either.
UPSTREAM_INPUT_TASKS = ("build", "test", "lint")

# Consumer -> upstream pairs deliberately declared in `fileGroups.upstreams` WITHOUT being in the
# crate's Moon closure. A6 is strict-equality (SMA-429's default-deny model), so an intentional
# over-approximation needs an entry here with a non-empty reason. Empty today.
ALLOW_OVER_APPROXIMATION = {}

# A6's anti-vacuity floor, mirroring REQUIRED_FFI_TASKS. A6 DERIVES each crate's closure from
# moon's `dependencies` key; a moon rename or JSON reshape would empty every closure and A6 would
# print PASS for thirteen crates while asserting nothing. These edges must survive the derivation.
REQUIRED_CLOSURE_EDGES = {
    "paigasus-iam-rs": {"paigasus-kernel-rs", "paigasus-proto-rs"},
    "paigasus-kernel-parity-rs": {"paigasus-kernel-rs"},
}
```

- [ ] **Step 3: Write the closure helper and A6**

```python
def rust_closure(projects, pid, seen=None):
    """Transitive dependsOn closure of `pid`, restricted to Rust projects.

    Restricted because Moon injects non-Rust build-scope parents into `dependencies`: `contracts`
    arrives via the `contracts:generate` task dep (which is what NON_CARGO_PARENTS already exists
    for) and has neither a `src/` tree nor a Cargo.toml, so an unfiltered closure would demand
    globs matching nothing for four crates.

    Transitive, not direct: affectedness does NOT propagate through `^:build` (SMA-528 F1), so with
    A -> B -> C an edit to C would otherwise reach B and stop.
    """
    seen = set() if seen is None else seen
    for dep in sorted((projects.get(pid) or {}).get("deps") or {}):
        if dep == pid or dep in NON_CARGO_PARENTS or dep not in projects:
            continue
        if projects[dep].get("language") != "rust" or dep in seen:
            continue
        seen.add(dep)
        rust_closure(projects, dep, seen)
    return seen


def check_upstream_inputs(
    projects, allow=None, floor=REQUIRED_CLOSURE_EDGES, tasks=UPSTREAM_INPUT_TASKS
):
    """Return the A6 violation list: crates whose build/test/lint do not key on their upstreams.

    A1-A3 assert dependency EDGES; A4/A5 assert WORKSPACE-level task inputs. A6 asserts per-crate
    UPSTREAM inputs, and it is the only thing standing between a wrong `fileGroups.upstreams` and a
    silent green: a crate's own moon.yml is NOT an input to its tasks (measured, SMA-528 F5 —
    paigasus-kernel-parity-rs:fmt reported hash 12d26cbd before and after a fileGroups edit), so a
    stale or empty group cannot red anything by itself.

    STRICT EQUALITY, not a subset check. A subset check would let a removed dependsOn edge leave
    stale globs in place forever and let a copy-pasted group over-approximate permanently —
    unbounded invisible CI cost, and the positive-superset model SMA-429 deliberately abandoned.
    """
    allow = ALLOW_OVER_APPROXIMATION if allow is None else allow
    a6 = []

    # FLOOR first: if the derivation broke, every per-crate check below is vacuous.
    for consumer, required in sorted((floor or {}).items()):
        if consumer not in projects:
            a6.append(f"floor names {consumer}, which is not in the graph")
            continue
        derived = rust_closure(projects, consumer)
        for upstream in sorted(required - derived):
            a6.append(
                f"FLOOR: {consumer}'s derived closure omits {upstream} — the dependsOn "
                f"derivation is broken, so A6 is asserting nothing"
            )

    for pid, proj in sorted(projects.items()):
        if proj.get("language") != "rust":
            continue
        own = proj["source_dir"]
        want = set()
        for upstream in rust_closure(projects, pid):
            src = projects[upstream]["source_dir"]
            want.add(f"{src}/src/**/*")
            want.add(f"{src}/Cargo.toml")
        for task in tasks:
            declared_files = (proj.get("task_inputs") or {}).get(task, "absent")
            declared_globs = (proj.get("task_input_globs") or {}).get(task, "absent")
            if declared_files == "absent" and declared_globs == "absent":
                a6.append(f"{pid} has no `{task}` task (nothing can key on its upstreams)")
                continue
            if declared_files is None or declared_globs is None:
                a6.append(
                    f"{pid}:{task} reported no inputFiles/inputGlobs — moon's output shape "
                    f"changed, so this assertion cannot be evaluated (treated as a violation, "
                    f"never skipped)"
                )
                continue
            resolved = set(declared_files or []) | set(declared_globs or [])
            # Observed = every entry pointing INTO another crate's tree. The crate's own
            # `src/**/*` and `Cargo.toml` come from .moon/tasks/rust.yml and are not upstreams.
            observed = {
                e
                for e in resolved
                if e.startswith("rs/crates/")
                and not e.startswith(f"{own}/")
                and (e.endswith("/src/**/*") or e.endswith("/Cargo.toml"))
            }
            for entry in sorted(want - observed):
                a6.append(f"{pid}:{task} inputs omit {entry}")
            for entry in sorted(observed - want):
                upstream = entry.rsplit("/src/**/*", 1)[0].rsplit("/Cargo.toml", 1)[0]
                if not _allowlisted(allow, pid, upstream):
                    a6.append(f"{pid}:{task} inputs include {entry}, which is not in its closure")
    return a6
```

- [ ] **Step 4: Wire A6 into `main()`**

Add `a6 = check_upstream_inputs(projects)` beside the existing `a4 = …`, extend both the
`if not (…)` guard and the PASS message, and append the report block:

```python
        (a6, "A crate's build/test/lint does not key on its upstream crates' sources, so an\n"
             "    upstream change SELECTS NOTHING for this crate and its cached PASS replays\n"
             "    against a different upstream (SMA-528).\n"
             "    Fix: the list lives in that crate's own moon.yml under `fileGroups.upstreams` —\n"
             "    two entries per upstream, `/<src_dir>/src/**/*` and `/<src_dir>/Cargo.toml`,\n"
             "    for its TRANSITIVE dependsOn closure. A `not in its closure` row is the\n"
             "    opposite: delete the entry, or add it to ALLOW_OVER_APPROXIMATION with a reason.\n"
             "    A `FLOOR:` row means the derivation itself broke — fix that first, every other\n"
             "    A6 row is meaningless until it passes."),
```

Update the PASS string to end: `…, and every crate keys on its upstream sources`.

- [ ] **Step 5: Extend `self_test()`'s clean fixture**

The fixture's projects need the two new keys or A6 cannot run against them. In both `a-rs` and
`b-rs`, add `"language": "rust"` and `"task_input_globs"`. `a-rs` depends on `b-rs`
(`rs/crates/libs/b`), so its complete upstream set is
`{"rs/crates/libs/b/src/**/*", "rs/crates/libs/b/Cargo.toml"}`:

```python
    upstream_ok = ["rs/crates/libs/b/src/**/*"]
    # a-rs: manifest in inputFiles, glob in inputGlobs — the real split A6 must span.
    ok["a-rs"]["language"] = "rust"
    ok["a-rs"]["task_inputs"] = {
        "build": ["rs/crates/libs/b/Cargo.toml"],
        "test": ["rs/crates/libs/b/Cargo.toml"],
        "lint": [*complete_inputs, "rs/crates/libs/b/Cargo.toml"],
    }
    ok["a-rs"]["task_input_globs"] = {
        "build": list(upstream_ok), "test": list(upstream_ok), "lint": list(upstream_ok),
    }
    ok["b-rs"]["language"] = "rust"
    ok["b-rs"]["task_input_globs"] = {"build": [], "test": [], "lint": []}
```

Existing A1–A5 assertions must keep passing against the extended fixture — run the self-test after
this step, before adding the A6 cases.

- [ ] **Step 6: Add the A6 negative-control cases**

```python
    # A6 clean baseline.
    if check_upstream_inputs(ok, allow={}, floor={}):
        failures.append(f"clean fixture reported A6 violations: {check_upstream_inputs(ok, allow={}, floor={})}")

    # A6-a: the GLOB half missing (upstream src not keyed on).
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["task_input_globs"]["test"] = []
    if not check_upstream_inputs(broken, allow={}, floor={}):
        failures.append("A6 did not fire on a missing upstream src glob")

    # A6-b: the MANIFEST half missing. This is the half an inputGlobs-only A6 could never see —
    # the exact defect the pre-review draft of the SMA-528 spec shipped.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["task_inputs"]["build"] = []
    if not check_upstream_inputs(broken, allow={}, floor={}):
        failures.append("A6 did not fire on a missing upstream Cargo.toml")

    # A6-c: over-approximation with no allowlist entry — the direction a subset check cannot see.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["task_input_globs"]["lint"].append("rs/crates/libs/ghost/src/**/*")
    if not check_upstream_inputs(broken, allow={}, floor={}):
        failures.append("A6 did not fire on an upstream outside the closure")

    # A6-d: an allowlisted over-approximation must be accepted, and only WITH a reason.
    if check_upstream_inputs(broken, allow={("a-rs", "rs/crates/libs/ghost"): "deliberate"}, floor={}):
        failures.append("A6 rejected an allowlisted over-approximation")
    if not check_upstream_inputs(broken, allow={("a-rs", "rs/crates/libs/ghost"): ""}, floor={}):
        failures.append("A6 accepted an allowlist entry with an empty reason")

    # A6-e: the FLOOR must fire when the derivation degrades to empty.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["deps"] = {}
    if not check_upstream_inputs(broken, allow={}, floor={"a-rs": {"b-rs"}}):
        failures.append("A6 floor did not fire on a neutered closure derivation")

    # A6-f: an absent inputGlobs key is an infra-shaped violation, never a silent skip.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["task_input_globs"]["build"] = None
    if not check_upstream_inputs(broken, allow={}, floor={}):
        failures.append("A6 did not fire on an absent inputGlobs key")
```

Note `_allowlisted` is keyed `(consumer, upstream)`; A6 passes the upstream's **source dir**, since
that is what it recovers from the entry string.

- [ ] **Step 7: Run the self-test**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"`
Expected: `rc=0`.

- [ ] **Step 8: Run A6 against the real graph**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && python3 ci/affected-graph/cargo_moon_parity.py; echo "rc=$?"`
Expected: `rc=0` and a PASS line ending `…, and every crate keys on its upstream sources`.

A failure here means Task 1's hand-written groups disagree with moon's closure. Fix the **group**,
not the gate — the gate is the authority.

- [ ] **Step 9: Full suite, then commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/affected-graph/run.sh --negative-control && ci/affected-graph/run.sh
git add ci/affected-graph/cargo_moon_parity.py
git commit -m "feat(repo): gate every crate's upstream inputs against moon's closure (SMA-528)"
```

---

### Task 6: Re-key the `rs/target` cache so the widened build is actually saved

**Files:**
- Modify: `.github/workflows/ci.yml:104-115`

Without this the change is a permanent slowdown: `actions/cache` **skips its save on a primary-key
hit**, and a kernel-source PR changes neither `rs/Cargo.lock` nor `rs/Cargo.toml`, so the key hits
exactly and the newly-built consumer test binaries are never written back — cold rebuild every run,
forever. `ci.yml` already documents this trap for `-lint-deps-` (SMA-534).

- [ ] **Step 1: Add the discriminator to the primary key**

Change `key:` to insert `upstream-inputs-` after `lint-deps-`:

```yaml
          key: rust-${{ runner.os }}-${{ hashFiles('rs/rust-toolchain.toml') }}-line-tables-only-lint-deps-upstream-inputs-${{ hashFiles('rs/Cargo.lock', 'rs/Cargo.toml') }}
```

- [ ] **Step 2: Add it as the first restore-key, keeping the existing two**

```yaml
          restore-keys: |
            rust-${{ runner.os }}-${{ hashFiles('rs/rust-toolchain.toml') }}-line-tables-only-lint-deps-upstream-inputs-
            rust-${{ runner.os }}-${{ hashFiles('rs/rust-toolchain.toml') }}-line-tables-only-lint-deps-
            rust-${{ runner.os }}-${{ hashFiles('rs/rust-toolchain.toml') }}-line-tables-only-
```

- [ ] **Step 3: Replace the stale comment above `key:`**

The existing comment claims a primary-key hit "can no longer happen" because `rs/Cargo.toml` joined
the hash. That is wrong — any PR touching neither manifest nor lock hits it exactly. Replace with:

```yaml
          # SMA-528 adds `upstream-inputs`: a kernel/proto/logging/observability/service-info commit
          # now builds and TESTS the whole downstream graph, but changes neither rs/Cargo.lock nor
          # rs/Cargo.toml — so the previous primary key would HIT exactly, actions/cache would skip
          # its save, and those newly-built test binaries would be rebuilt cold every run, forever.
          # (The SMA-534 comment this replaces claimed a primary hit "can no longer happen" once
          # rs/Cargo.toml joined the hash. It can: every PR that touches neither file hits it.)
          # Three-tier restore: the new prefix goes warm after the first post-merge save; the two
          # older prefixes keep that first run — which builds far more than any before it — from
          # starting fully cold. All three keep the toolchain hash and `-line-tables-only-`, so
          # neither the SMA-389 cross-version guard nor the debuginfo-bloat guard is weakened.
```

- [ ] **Step 4: Verify the workflow still lints**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run repo:actionlint`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "ci(repo): re-key the rust cache so the widened graph is saved (SMA-528)"
```

---

### Task 7: Documentation and correcting the claims F1 makes false

**Files:**
- Modify: `ci/affected-graph/README.md`
- Modify: `CLAUDE.md`
- Modify: `ci/affected-graph/run.sh` (`assert_include_relations`' comment only)
- Modify: `rs/crates/services/paigasus-iam/moon.yml`, `ci/error-registry/README.md`

- [ ] **Step 1: Correct `assert_include_relations`' false premise**

Its comment says "the edges are inert without it". Replace that clause with:

```bash
# NOTE (SMA-528): `--include-relations` was measured to change NOTHING in every probe run —
# including the full 24-target ci.yml shape, where `moon ci "${T[@]}" --stdin --include-relations`
# and the same command WITHOUT it produce identical action sets, and where adding
# `--downstream deep` also changes nothing. No probe was found in which it alters the RunTask set.
# The flag is kept and still asserted because removing it on that evidence is an unforced risk and
# it remains the documented mechanism should moonrepo fix the dependent traversal upstream — but do
# NOT read this gate as evidence that the cascade works. What carries the cascade is
# `@group(upstreams)`, asserted by the *_ci task cases and by cargo_moon_parity.py's A6.
```

The same "inert without it" claim appears near the end of `ci/affected-graph/README.md` — correct
it there too.

- [ ] **Step 2: Update the README**

Add an A6 bullet to the assertion list, in the style of the A4/A5 bullets: what it asserts (per-crate
upstream inputs, strict equality against moon's Rust-restricted transitive `dependsOn` closure), why
the per-case sets cannot make it, its `ALLOW_OVER_APPROXIMATION` escape hatch and its
`REQUIRED_CLOSURE_EDGES` floor, and F5 — that a crate's own `moon.yml` is not an input to its tasks,
which is why A6 is the sole guard.

Add a task-case paragraph documenting the two traversal modes, the measured relationship
`moon ci RunTask set = (query-affected ∩ T ∩ runInCI) ∪ upstream-dep closure`, and that it must be
re-measured on a moon bump — extend the README's existing "a moon upgrade breaks this guard"
paragraph to name `inputGlobs` alongside `inputFiles`, `command`/`args`/`script`.

Correct the framing of the project cases: they prove the `dependsOn` **edge** exists, not that the
cascade runs.

Add to the maintenance section: a new crate must declare `fileGroups.upstreams` (a missing group is
a hard graph-load error), and adding an in-tree dep changes the consumer's closure and therefore
A6's expectation.

- [ ] **Step 3: Extend the CLAUDE.md Moon gotcha**

Append to the existing bullet that ends "…instead of silently under-building (SMA-524)":

```markdown
  Neither is enough on its own either: task `inputs` are the **only** thing that confers
  affectedness in Moon 2.3.2. `dependsOn` and `^:build` schedule an upstream's build but never
  **select** a downstream — a dependent runs only if independently affected, and neither
  `--include-relations` nor `--downstream` changes that for `moon ci` (measured at the full
  24-target shape: identical action sets with and without both, SMA-528). Every Rust crate therefore
  declares its transitive upstream sources in `fileGroups.upstreams`, consumed by build/test/lint via
  `@group(upstreams)` in `.moon/tasks/rust.yml`. Omitting the group is a hard graph-load error
  (`project::unknown_file_group`) for every moon command; mis-declaring it reds
  `repo:affected-smoke`'s A6 — and **nothing else can**, because a crate's own `moon.yml` is not an
  input to its tasks, so a wrong group otherwise serves a cached PASS. `^:build` has a second job
  here: it orders `contracts:generate` before a downstream that keys on
  `paigasus-proto/src/generated/**`, so removing it as "vestigial" would make those cache keys
  nondeterministic.
```

- [ ] **Step 4: Correct the two now-load-bearing claims**

`rs/crates/services/paigasus-iam/moon.yml` and `ci/error-registry/README.md` both state that a
contracts change already schedules the service crates' membership tests via `test: deps: ['^:build']`.
Per F1 that was **false** until this change. Append to each:

```
(Until SMA-528 this was aspirational: `^:build` schedules an upstream's build, it does not make this
crate affected. What makes it true is `@group(upstreams)` — this crate's build/test/lint now key on
paigasus-proto's sources, so a contracts change that regenerates them selects this test.)
```

- [ ] **Step 5: Update `ALLOW_NO_CARGO_BACKING`'s reason string, which this change invalidates**

`cargo_moon_parity.py`'s entry for `("paigasus-gateway-rs", "paigasus-kernel-rs")` says
over-building "costs CI time but can never under-build". That was written when the edge cost
**nothing**: it only widened a project-graph query. After SMA-528 the gateway keys on the kernel's
sources, so every kernel edit runs the gateway's full `build` + `test` + `lint` for a Cargo
dependency that does not exist. Keep the edge — removing it would churn the `kernel->bindings`
project case and the new `kernel->consumer-tasks` case — but make the price visible:

```python
    ("paigasus-gateway-rs", "paigasus-kernel-rs"): (
        "Over-approximation, not a defect: the gateway has no Cargo dep on the kernel. Removing the "
        "edge would change the kernel->bindings expected set that SMA-409 owns (SMA-524 D4). NOTE "
        "(SMA-528): this is no longer free. The edge now feeds @group(upstreams), so every kernel "
        "edit runs the gateway's full build+test+lint for a dependency that does not exist. Revisit "
        "if kernel PRs approach the CI budget — that is the first thing to drop."
    ),
```

- [ ] **Step 6: Verify no marker/gate drift**

`CLAUDE.md`'s `<!-- ci-targets:begin -->` block must be untouched — A6 adds no Moon task, so `T` is
unchanged.

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && python3 ci/affected-graph/ci_targets.py; echo "rc=$?"`
Expected: `rc=0`.

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/README.md CLAUDE.md ci/affected-graph/run.sh \
        ci/affected-graph/cargo_moon_parity.py \
        rs/crates/services/paigasus-iam/moon.yml ci/error-registry/README.md
git commit -m "docs(repo): record that only task inputs confer affectedness (SMA-528)"
```

---

### Task 8: Full-graph verification and the cost measurement the spec requires

**Files:** none modified unless a gate fails.

- [ ] **Step 1: The issue's headline acceptance check — a REAL `moon ci` run**

Probes K/L in the spec are the pre-fix baseline (16 tasks, no consumers). Re-run the same command
and diff.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-528
T=(:build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site :promtool :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata)
echo "rs/crates/libs/paigasus-kernel/src/lib.rs" | moon ci "${T[@]}" --stdin --include-relations
python3 -c "
import json;d=json.load(open('.moon/cache/ciReport.json'))
r=sorted(a['label'].replace('RunTask(','').replace(')','') for a in d['actions'] if a['node']['action']=='run-task')
need={'paigasus-kernel-parity-rs:test','paigasus-iam-rs:test','paigasus-iam-core-rs:test','paigasus-gateway-rs:test'}
print('n actions:', len(r)); print('MISSING:', need-set(r) or 'none')
print('wall secs:', d.get('duration',{}).get('secs'))"
```

Expected: `MISSING: none`, and `n actions` substantially above the pre-fix 16.

This must run with Docker reachable — `paigasus-iam-rs:test`'s suites hard-fail on an unreachable
daemon via `tests/docker_preflight.rs`. Do **not** set `PAIGASUS_SKIP_DOCKER=1` to get past it: a
run that greened under it leaves a cached PASS that replays afterwards.

- [ ] **Step 2: Record the cost datapoint the spec's acceptance threshold needs**

From Step 1's output, note total wall seconds and the slowest tasks:

```bash
python3 -c "
import json;d=json.load(open('.moon/cache/ciReport.json'))
rows=sorted(((a.get('duration') or {}).get('secs') or 0, a['label']) for a in d['actions'] if a['node']['action']=='run-task')
print('total wall:', d.get('duration',{}).get('secs'), 's')
[print(f'  {s:5d}s {l}') for s,l in rows[-8:]]"
df -h . | tail -1
```

Record both in the PR description. The spec's threshold is **25 minutes of CI wall time**; if the
PR's own CI run exceeds it or shows disk pressure, split the job or raise `timeout-minutes` **before
merging**, per spec §5.

- [ ] **Step 3: Run the whole repo gate set as CI does**

This change touches `.moon/tasks/rust.yml`, which schedules the entire Rust graph.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci "${T[@]}" --base origin/main --include-relations
```

Expected: exit 0. Diagnose any unattributed failure via
`python3 -c "import json;d=json.load(open('.moon/cache/ciReport.json'));print([a['label'] for a in d['actions'] if a['status']=='failed'])"`.

- [ ] **Step 4: Confirm the negative control still proves the suite can fail**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && ci/affected-graph/run.sh --negative-control`
Expected: `negative-control OK: harness reported red on all wrong expectations`

- [ ] **Step 5: Confirm no stray artefacts and a clean tree**

```bash
git status --short
```
Expected: empty. In particular `rs/crates/libs/paigasus-kernel/src/lib.rs` must carry no leftover
probe comment from Task 3 Step 4.

- [ ] **Step 6: Commit any gate adjustments this task required**

If Steps 1-4 required no changes, there is nothing to commit — say so rather than creating an empty
commit.

```bash
git commit -m "fix(repo): reconcile the affected-graph baselines after the cascade fix (SMA-528)"
```

---

## Self-Review

**Spec coverage**

| Spec section | Task |
|---|---|
| §3.1 shape (`@group(upstreams)`, per-crate group, empty for leaves) | 1, 3 |
| §3.2 closure = Rust-restricted transitive `dependsOn`; `contracts` excluded | 1 (data), 5 (`rust_closure`) |
| §3.3 two entries per upstream; brace form forbidden; `fmt`/`build-release` excluded | 1, 3 |
| §4.1 A6: both buckets, strict equality + allowlist, anti-vacuity floor | 5 |
| §4.1 no `T`/marker change | 7 Step 5 |
| §4.2 both traversal modes | 2, 4 |
| §4.3 new case, hand-derived 30-row set | 2 |
| §4.4 project cases kept, comments corrected | 7 Step 2 |
| §4.5 `assert_include_relations` kept, comment corrected | 7 Step 1 |
| §4.6 negative controls (A6 + `expect_red_task` + existing cases) | 4, 5 |
| §4.7 invariant documented, re-measure on moon bump | 2 Step 1, 7 Step 2 |
| §4.8 landing order | task order + Global Constraints |
| §4.9 template | 1 Step 4 |
| §5 cache key | 6 |
| §5 acceptance threshold + `df -h` | 8 Step 2 |
| §6.0 red before green | 2 |
| §6.1 gate self-tests | 5 Step 7, 8 Step 4 |
| §6.2 query-level check | 3 Step 3 |
| §6.3 cache correctness | 3 Step 4 |
| §6.4 real run | 8 Step 1 |
| §6.5 full graph | 8 Step 3 |
| §7 docs incl. the two now-load-bearing claims | 7 |

Spec §9 items (upstream moonrepo report, `buf.gen.yaml` codegen changes, cross-stack A6, CI job
splitting) are explicitly out of scope and have no task, by design.

**Placeholder scan:** no TBD/TODO, no "add error handling", no "similar to Task N". Every code step
carries the literal content.

**Type consistency:** `check_upstream_inputs(projects, allow, floor, tasks)` is defined in Task 5
Step 3 and called with that signature in Step 4 (`main()`) and Step 6 (self-tests).
`rust_closure(projects, pid, seen)` likewise. `assert_task_case_ci` / `run_task_case_ci` /
`expect_red_task` are defined in Task 2 Steps 1-2 and Task 4 Step 4, and used with matching
`LABEL FILE EXPECTED_CSV` arity throughout. The new `moon_projects()` keys `task_input_globs` and
`language` are produced in Task 5 Step 1 and consumed in Steps 3, 5 and 6.

**One deliberate ordering dependency:** Task 5 Step 5 extends the self-test fixture *before* Step 6
adds A6 cases to it, so A1–A5 are re-verified against the extended fixture before A6 relies on it.
