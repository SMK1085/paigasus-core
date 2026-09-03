# SMA-524 — `paigasus-service-info` Moon edges + Cargo↔Moon parity gate — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire the three missing Moon graph edges around `paigasus-service-info`, and add a
Cargo↔Moon parity gate that makes the entire class of missing-edge defects impossible.

**Architecture:** Three hand-declared edges in `moon.yml` files restore Cargo/Moon parity. A new
Python gate, invoked from the existing `ci/affected-graph/run.sh`, compares Cargo's dependency graph
against Moon's *own resolved* graph — it never parses `moon.yml`. Two new assertion cases extend the
existing strict-equality guard.

**Tech Stack:** Moon 2.3.2, Bash, Python 3.12 (`tomllib` stdlib), Cargo (edition 2024).

**Spec:** `docs/superpowers/specs/2026-08-16-sma-524-service-info-moon-edges-design.md`

## Global Constraints

- Every source file opens with an SPDX header: `# SPDX-License-Identifier: Apache-2.0` for Python and
  Bash. `moon.yml`/config files are carved out (SMA-383).
- Prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` — the Bash
  tool PATH lacks the proto-managed CLIs, and **shims must come first** so `moon` resolves to the
  repo-pinned 2.3.2, not a global pin.
- Work in the worktree: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-524` on
  branch `feature/sma-524-service-info-moon-edge`. Do **not** work in the main checkout — two other
  sessions are active there.
- The gate must depend only on `moon`, `python3`, and standard Unix tools. **Never** shell out to
  `cargo`: `repo:affected-smoke` is `toolchain: 'system'`.
- **Never** parse `moon.yml` for edges or task deps. Read `moon query projects` JSON instead.
- **Never** use a regex to find Cargo dependencies. Use `tomllib`. A regex prototype produced five
  false positives by missing TOML dotted keys (`paigasus-kernel.workspace = true`).
- Commit messages: conventional commits with a workspace scope, subject **lowercase** after the
  prefix, header ≤100 chars. Never put a bare `#NNN` in the body (breaks commitlint
  `footer-leading-blank`). Do **not** use `--no-verify`.
- Moon 2.3.2 field names: `layer:` (not `type:`), `vcs.client`, `codeowners.sync`.

## Baseline (already verified — do not re-derive)

At `origin/main` (`4546c6a`), `bash ci/affected-graph/run.sh` passes all 8 cases and
`--negative-control` reports red correctly. The parity violations that exist today:

| Assertion | Count | Rows |
| --- | --- | --- |
| A1 edge missing | 3 | `gateway → service-info`, `iam → service-info`, `service-info → proto` |
| A2 unbacked explicit edge | 0 (1 allowlisted) | `gateway → kernel` |
| A3 upstream `:build` not scheduled | 6 | `{gateway,iam,service-info}` × `{build,test}` |

After Task 2, all three must be empty.

## File Structure

| File | Responsibility |
| --- | --- |
| `ci/affected-graph/cargo_moon_parity.py` | **new** — the parity gate: A1/A2/A3 + allowlist + self-test |
| `ci/affected-graph/run.sh` | invokes the gate; gains 2 assertion cases; loses SMA-438's note |
| `ci/affected-graph/README.md` | documents the gate + new cases; 3 stale bullets corrected |
| `rs/crates/libs/paigasus-service-info/moon.yml` | gains `dependsOn` + `^:build` |
| `rs/crates/services/paigasus-{iam,gateway}/moon.yml` | each gains `paigasus-service-info-rs` |
| `rs/crates/libs/paigasus-{proto,kernel-parity}/moon.yml` | corrected premise in comments |
| `CLAUDE.md` | corrected premise + the new convention |

The gate lives in its own file rather than inline in `run.sh` because it is ~120 lines of Python with
its own self-test; `run.sh`'s existing inline `python3 -c` snippets are one-liners.

---

### Task 1: The parity gate, test-first

**Files:**
- Create: `ci/affected-graph/cargo_moon_parity.py`

**Interfaces:**
- Produces: `python3 ci/affected-graph/cargo_moon_parity.py` → exit 0 clean / 1 violations / 2 infra
  error. `--self-test` runs the built-in negative control. Both are consumed by Task 3.

- [ ] **Step 1: Write the failing self-test**

Create `ci/affected-graph/cargo_moon_parity.py` containing **only** the SPDX header, the imports, the
constants, and the self-test — no detection logic yet.

```python
# SPDX-License-Identifier: Apache-2.0
# SMA-524 — Cargo <-> Moon dependency-graph parity gate.
#
# The affected-graph guard (SMA-409/429) asserts only the edges someone remembered to write a CASE
# for. SMA-505 added a crate with no case, so its three missing edges survived a full review cycle.
# This gate is generic: it compares every crate's Cargo dependencies against Moon's OWN RESOLVED
# graph, so a new crate cannot repeat that failure.
#
# It never parses moon.yml (formatting-proof) and never shells out to cargo (repo:affected-smoke is
# toolchain: 'system'). Cargo.toml is parsed with tomllib, which — unlike the regex this replaced —
# handles dotted keys (`paigasus-kernel.workspace = true`), inline tables, and `package =` renames.
#
# usage: cargo_moon_parity.py [--self-test]
import json
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path

# Exceptions meaning "the inputs or environment are broken", NOT "the graph regressed". main() maps
# these to rc 2 so run.sh aborts, instead of folding them into SUITE_RC as an assertion failure.
# tomllib.TOMLDecodeError subclasses ValueError, not OSError, so it has to be named explicitly — the
# self-test pins that by asserting cargo_crates' real failure is a member of this tuple.
INFRA_ERRORS = (
    subprocess.CalledProcessError,
    json.JSONDecodeError,
    tomllib.TOMLDecodeError,
    OSError,
)

# (consumer, upstream) -> why this hand-declared Moon edge has no Cargo backing.
# An allowlisted edge is a RECORDED DECISION, not a silent exemption: the reason string is required.
ALLOW_NO_CARGO_BACKING = {
    ("paigasus-gateway-rs", "paigasus-kernel-rs"): (
        "Over-approximation, not a defect: the gateway has no Cargo dep on the kernel. Over-building "
        "costs CI time but can never under-build, and removing the edge would change the "
        "kernel->bindings expected set that SMA-409 owns (SMA-524 D4)."
    ),
}

# Build-scope parents injected by a task dep (e.g. `contracts:generate`), never Cargo deps.
NON_CARGO_PARENTS = {"contracts"}


def _allowlisted(allow, consumer, upstream):
    """True only if the entry exists AND carries a non-empty reason.

    Bare membership would let `("a", "b"): ""` silence A2 unreviewably, which defeats the point of
    the table: an allowlisted edge is a recorded decision, so the record is what earns the exemption.
    """
    return bool(allow.get((consumer, upstream), "").strip())


def check(projects, crates, allow=None):
    """Return (a1, a2, a3) violation lists.

    projects: {moon_id: {"source_dir": str,
                         "deps": {dep_id: "explicit"|"implicit"},
                         "tasks": {task_name: [resolved dep target, ...]}}}
    crates:   {crate_name: {"source_dir": str, "deps": {crate_name, ...}}}
    allow:    allowlist table; defaults to ALLOW_NO_CARGO_BACKING (injectable for the self-test).
    """
    allow = ALLOW_NO_CARGO_BACKING if allow is None else allow
    by_dir = {p["source_dir"]: mid for mid, p in projects.items()}
    a1, a2, a3 = [], [], []
    for _crate, info in sorted(crates.items()):
        mid = by_dir.get(info["source_dir"])
        if mid is None:
            continue
        want = {
            by_dir[crates[d]["source_dir"]]
            for d in info["deps"]
            if d in crates and crates[d]["source_dir"] in by_dir
        }
        have = projects[mid]["deps"]
        tasks = projects[mid]["tasks"]

        for upstream in sorted(want - set(have)):
            a1.append(f"{mid} -> {upstream}")
        for dep, src in sorted(have.items()):
            if (
                src == "explicit"
                and dep not in want
                and dep not in NON_CARGO_PARENTS
                and not _allowlisted(allow, mid, dep)
            ):
                a2.append(f"{mid} -> {dep}")
        for upstream in sorted(want):
            for task in ("build", "test"):
                if f"{upstream}:build" not in tasks.get(task, []):
                    a3.append(f"{mid}:{task} does not schedule {upstream}:build")
    return a1, a2, a3


def self_test():
    """Negative control: each assertion must FIRE on a synthetic violation (SMA-524 D6).

    A gate whose whole value is catching a silent hole must not be able to pass vacuously.
    """
    ok = {
        "a-rs": {
            "source_dir": "rs/crates/libs/a",
            "deps": {"b-rs": "explicit"},
            "tasks": {"build": ["b-rs:build"], "test": ["b-rs:build"]},
        },
        "b-rs": {"source_dir": "rs/crates/libs/b", "deps": {}, "tasks": {"build": [], "test": []}},
    }
    crates = {
        "a": {"source_dir": "rs/crates/libs/a", "deps": {"b"}},
        "b": {"source_dir": "rs/crates/libs/b", "deps": set()},
    }
    failures = []

    a1, a2, a3 = check(ok, crates)
    if (a1, a2, a3) != ([], [], []):
        failures.append(f"clean fixture reported violations: {a1} {a2} {a3}")

    # A1: drop the project edge, keep the Cargo dep.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["deps"] = {}
    if not check(broken, crates)[0]:
        failures.append("A1 did not fire on a missing project edge")

    # A2: a hand-declared edge with no Cargo backing.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["deps"]["ghost-rs"] = "explicit"
    if not check(broken, crates)[1]:
        failures.append("A2 did not fire on an unbacked explicit edge")

    # A3: the edge exists but the upstream build is not scheduled — the exact hole the
    # project-level affected-graph guard is structurally blind to (SMA-429 F3).
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["tasks"] = {"build": [], "test": []}
    if not check(broken, crates)[2]:
        failures.append("A3 did not fire on an unscheduled upstream build")

    # An implicit edge is just as valid as an explicit one — A2 must not flag it.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["deps"] = {"b-rs": "implicit"}
    if check(broken, crates)[1]:
        failures.append("A2 wrongly flagged an implicit (toolchain-inferred) edge")

    # The allowlist exempts on the strength of its REASON, not on bare membership: a blank reason is
    # an unreviewable exemption and must not silence A2.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["deps"]["ghost-rs"] = "explicit"
    if not check(broken, crates, allow={("a-rs", "ghost-rs"): "   "})[1]:
        failures.append("A2 did not fire on an allowlist entry with a blank reason")
    if check(broken, crates, allow={("a-rs", "ghost-rs"): "a documented reason"})[1]:
        failures.append("A2 fired despite a properly-reasoned allowlist entry")

    # A malformed Cargo.toml must surface as INFRA (rc 2), not as an assertion failure. tomllib's
    # TOMLDecodeError subclasses ValueError, not OSError, so main() has to name it explicitly.
    try:
        tomllib.loads("[dependencies\nbroken =")
    except tomllib.TOMLDecodeError:
        pass
    except Exception as exc:  # pragma: no cover - guards an upstream behaviour change
        failures.append(f"malformed TOML raised {type(exc).__name__}, not TOMLDecodeError")
    else:
        failures.append("malformed TOML did not raise at all")

    for f in failures:
        print(f"  FAIL {f}", file=sys.stderr)
    if failures:
        print("negative-control FAILED: the parity gate can pass vacuously", file=sys.stderr)
        return 1
    print("  OK   [parity] all three assertions fire on synthetic violations")
    return 0


if __name__ == "__main__":
    sys.exit(self_test() if "--self-test" in sys.argv[1:] else 0)
```

- [ ] **Step 2: Run the self-test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-524
python3 ci/affected-graph/cargo_moon_parity.py --self-test
```

Expected: **FAIL** — `NameError: name 'check' is not defined` is *not* acceptable (the function is
defined above). The real expected failure is that `check` returns empty lists for every fixture, so
you get three `did not fire` lines and exit 1. If it passes, the fixtures are wrong — fix them before
continuing.

> Note: `check()` above is already complete. If you prefer strict red-green, temporarily stub its body
> to `return [], [], []`, watch the three `did not fire` failures, then restore it.

- [ ] **Step 3: Add the data-collection functions**

Append to `cargo_moon_parity.py`, **above** the `if __name__` block:

```python
def moon_projects():
    """Moon's own resolved graph. Never parse moon.yml — Moon already resolved it.

    `moon query projects` expands `^:build` into concrete targets, so task deps report
    `paigasus-iam-rs:build -> ['paigasus-proto-rs:build', ...]`. Asserting that expansion is
    strictly stronger than grepping for the literal `^:build`: it also catches a hand-written
    per-dependency list that omits an upstream.
    """
    out = subprocess.run(
        ["moon", "query", "projects"], capture_output=True, text=True, check=True
    ).stdout
    projects = {}
    for p in json.loads(out)["projects"]:
        tasks = {}
        for name, task in (p.get("tasks") or {}).items():
            tasks[name] = [
                d if isinstance(d, str) else d.get("target")
                for d in (task.get("deps") or [])
            ]
        projects[p["id"]] = {
            "source_dir": p["source"],
            "deps": {d["id"]: d.get("source") for d in (p.get("dependencies") or [])},
            "tasks": tasks,
        }
    return projects


def cargo_crates(root):
    """Every workspace crate and its in-tree deps, via tomllib.

    tomllib handles all three declaration forms the repo uses — inline table
    (`paigasus-proto = { workspace = true }`), dotted key
    (`paigasus-proto-derive.workspace = true`), and `package =` renames. The regex this replaced
    saw only the first, and reported five sound edges as phantom.
    """
    manifests = {}
    for path in sorted((root / "rs" / "crates").rglob("Cargo.toml")):
        data = tomllib.loads(path.read_text())
        package = data.get("package")
        if not package or "name" not in package:
            continue
        manifests[package["name"]] = (path, data)

    crates = {}
    for name, (path, data) in manifests.items():
        deps = set()
        for kind in ("dependencies", "dev-dependencies", "build-dependencies"):
            for key, value in (data.get(kind) or {}).items():
                real = value.get("package", key) if isinstance(value, dict) else key
                if real in manifests and real != name:
                    deps.add(real)
        crates[name] = {
            "source_dir": str(path.parent.relative_to(root)),
            "deps": deps,
        }
    return crates
```

- [ ] **Step 4: Add the main entry point**

Replace the `if __name__` block with:

```python
def main():
    root = Path(__file__).resolve().parents[2]
    try:
        projects = moon_projects()
        crates = cargo_crates(root)
    except INFRA_ERRORS as exc:
        # Mirror run.sh's infra-vs-assertion split: a broken `moon` — or an unparseable Cargo.toml —
        # must never be mistaken for a graph regression. See INFRA_ERRORS.
        print(f"FATAL [parity] could not build the graphs: {exc}", file=sys.stderr)
        return 2

    a1, a2, a3 = check(projects, crates)
    if not (a1 or a2 or a3):
        print(
            f"PASS  {'cargo-moon-parity':<18} -> "
            f"{len(crates)} crates: every Cargo dep has a Moon edge that schedules its build"
        )
        return 0

    print("FAIL  [cargo-moon-parity] Cargo and Moon disagree", file=sys.stderr)
    for rows, title in (
        (a1, "Cargo dep with NO Moon edge (under-builds — CI stays green while skipping work).\n"
             "    Fix: add the upstream to `dependsOn` in the consumer's moon.yml."),
        (a2, "Hand-declared Moon edge with NO Cargo backing (over-builds).\n"
             "    Fix: delete it, or add it to ALLOW_NO_CARGO_BACKING with a reason."),
        (a3, "Moon edge exists but the upstream's build is NOT scheduled — the affected-graph\n"
             "    guard CANNOT see this (SMA-429 F3).\n"
             "    Fix: add '^:build' to the task's `deps` in the consumer's moon.yml."),
    ):
        if rows:
            print(f"  {title}", file=sys.stderr)
            for row in rows:
                print(f"      {row}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(self_test() if "--self-test" in sys.argv[1:] else main())
```

- [ ] **Step 5: Run the self-test to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py --self-test
```
Expected: `OK   [parity] all three assertions fire on synthetic violations`, exit 0.

- [ ] **Step 6: Run the gate against the real repo — it MUST fail with exactly the baseline set**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py; echo "exit=$?"
```

Expected: exit 1, listing **exactly** 3 A1 rows and 6 A3 rows, and **zero** A2 rows (the phantom
`gateway → kernel` edge is allowlisted). If A2 is non-empty, the allowlist key is wrong. If any count
differs from the table in "Baseline" above, **stop** — the parser is wrong, not the repo.

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -m "feat(ci): add a Cargo/Moon dependency-graph parity gate (SMA-524)" \
  -m "Compares every crate's Cargo deps against Moon's own resolved graph, asserting that each
edge exists AND that it schedules the upstream's build. The task-level assertion covers the
half the affected-graph guard is structurally blind to (SMA-429 F3). Currently RED: it
reports the three missing paigasus-service-info edges, fixed in the next commit."
```

---

### Task 2: Wire the three edges — turn the gate green

**Files:**
- Modify: `rs/crates/libs/paigasus-service-info/moon.yml`
- Modify: `rs/crates/services/paigasus-iam/moon.yml:7-12`
- Modify: `rs/crates/services/paigasus-gateway/moon.yml:7-11`

**Interfaces:**
- Consumes: the gate from Task 1 as the failing test.
- Produces: a green parity gate; `paigasus-service-info-rs` reachable from `paigasus-proto`, and
  `paigasus-{iam,gateway}-rs` reachable from `paigasus-service-info`.

- [ ] **Step 1: Confirm the gate is red (this is the failing test)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py; echo "exit=$?"
```
Expected: exit 1, 3 A1 rows + 6 A3 rows.

- [ ] **Step 2: Wire `paigasus-service-info`**

Replace the whole of `rs/crates/libs/paigasus-service-info/moon.yml` with:

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-service-info-rs'
layer: 'library'
language: 'rust'

# This crate depends on paigasus-proto via `workspace = true` (Cargo.toml:13). Moon 2.3.2's Rust
# toolchain resolves `path = "..."` deps into the project graph automatically but does NOT resolve
# workspace-inherited ones, so a `workspace = true` dep MUST be hand-declared here (SMA-524).
#
# BOTH declarations are required, and they do different jobs:
#   * `dependsOn`      creates the project-graph edge (what `moon query projects --affected` follows);
#   * task `^:build`   schedules the upstream's build (what `moon ci --include-relations` runs).
# Neither implies the other. `repo:affected-smoke` asserts both (SMA-524 A1/A3).
dependsOn:
  - 'paigasus-proto-rs'

tasks:
  build:
    deps: ['contracts:generate', '^:build']
  test:
    deps: ['contracts:generate', '^:build']
```

- [ ] **Step 3: Wire `paigasus-iam`**

In `rs/crates/services/paigasus-iam/moon.yml`, add one line to `dependsOn` so the block reads:

```yaml
dependsOn:
  - 'paigasus-iam-core-rs'
  - 'paigasus-logging-rs'
  - 'paigasus-observability-rs'
  - 'paigasus-kernel-rs'
  - 'paigasus-proto-rs'
  - 'paigasus-service-info-rs'
```

Leave the `tasks` block alone — it already carries `^:build` on `build` and `test`.

- [ ] **Step 4: Wire `paigasus-gateway`**

In `rs/crates/services/paigasus-gateway/moon.yml`, replace the `dependsOn` block with:

```yaml
dependsOn:
  - 'paigasus-proto-rs'
  # No Cargo dep backs this edge — the gateway does not use the kernel. Kept deliberately as an
  # over-approximation: over-building costs CI time but can never under-build, and removing it would
  # change the `kernel->bindings` expected set that SMA-409 owns. Allowlisted in
  # ci/affected-graph/cargo_moon_parity.py (SMA-524 D4).
  - 'paigasus-kernel-rs'
  - 'paigasus-logging-rs'
  - 'paigasus-observability-rs'
  - 'paigasus-service-info-rs'
```

- [ ] **Step 5: Run the gate — it must now be green**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py; echo "exit=$?"
```
Expected: exit 0, `PASS  cargo-moon-parity  -> 13 crates: …`.

- [ ] **Step 6: Prove the fix changed the affected graph**

```bash
printf '%s\n' rs/crates/libs/paigasus-service-info/src/lib.rs \
  | moon query projects --affected --downstream deep \
  | python3 -c 'import sys,json; print(", ".join(sorted(p["id"] for p in json.load(sys.stdin)["projects"] if p["id"]!="repo")))'
```
Expected: `paigasus-gateway-rs, paigasus-iam-rs, paigasus-service-info-rs` (was
`paigasus-service-info-rs` alone).

- [ ] **Step 7: Confirm the existing guard now fails, and understand why**

```bash
bash ci/affected-graph/run.sh; echo "exit=$?"
```
Expected: **exit 1**, `FAIL [proto-derive->proto]` with `unexpected: paigasus-service-info-rs`. This is
correct — the expected set is updated in Task 3. Do not "fix" it here.

- [ ] **Step 8: Commit**

```bash
git add rs/crates/libs/paigasus-service-info/moon.yml \
        rs/crates/services/paigasus-iam/moon.yml \
        rs/crates/services/paigasus-gateway/moon.yml
git commit -m "fix(rs): wire the missing paigasus-service-info Moon graph edges (SMA-524)" \
  -m "paigasus-service-info consumes paigasus-proto, and both services consume
paigasus-service-info, but none of the three edges was declared — so a proto change never
retested the crate and a ServiceInfoDto change retested nothing at all. Turns the parity
gate green; the affected-graph expected sets follow in the next commit."
```

---

### Task 3: Extend the affected-graph guard

**Files:**
- Modify: `ci/affected-graph/run.sh` — `run_suite()`, the negative-control block, and `assert_case`'s
  neighbourhood

**Interfaces:**
- Consumes: `cargo_moon_parity.py` (Task 1), the wired edges (Task 2).
- Produces: `assert_task_case`, the `service-info->services` project case, the
  `proto->service-info-tasks` behavioral case, and the parity gate wired into both the suite and the
  negative control.

- [ ] **Step 1: Update the `proto-derive->proto` expected set and delete SMA-438's note**

In `run_suite()`, delete the **entire** comment paragraph beginning
`# paigasus-service-info-rs is deliberately ABSENT` **including the bare `#` separator line directly
above it**, and add the crate to the expected set. The result:

```bash
  # derive-crate edit -> the derive crate + paigasus-proto and everything downstream of it
  # (SMA-438). One-directional w.r.t. contracts: the derive crate is strictly UPSTREAM of
  # paigasus-proto, so a proto edit must NOT reach it — enforced implicitly by the strict
  # equality of the contracts->proto case above, which lists no derive crate.
  # paigasus-service-info-rs is here via its paigasus-proto edge, wired in SMA-524.
  run_case "proto-derive->proto" "rs/crates/libs/paigasus-proto-derive/src/lib.rs" \
    "paigasus-proto-derive-rs,paigasus-proto-rs,paigasus-gateway-rs,paigasus-iam-rs,paigasus-service-info-rs"
```

- [ ] **Step 2: Add the `service-info->services` project case**

Immediately after the `proto-derive->proto` case, add:

```bash
  # service-info edit -> the crate + both services that serve the descriptor (SMA-524). Guards the
  # DOWNSTREAM direction, which no case covered before: paigasus-service-info was a graph LEAF, so an
  # edit to ServiceInfoDto — the wire body both services return — retested nothing.
  # One-directional: paigasus-proto-rs is deliberately absent (a consumer edit must not rebuild the
  # contract crate), enforced implicitly by strict equality.
  run_case "service-info->services" "rs/crates/libs/paigasus-service-info/src/lib.rs" \
    "paigasus-service-info-rs,paigasus-iam-rs,paigasus-gateway-rs"
```

- [ ] **Step 3: Run the suite — all cases must pass again**

```bash
bash ci/affected-graph/run.sh; echo "exit=$?"
```
Expected: exit 0, 9 PASS lines, `== affected-graph cascade intact ==`.

- [ ] **Step 4: Add `assert_task_case`**

Insert directly **after** the `assert_case` function (before the `assert_include_relations` comment
block):

```bash
# assert_task_case LABEL FILE EXPECTED_CSV
#   Same strict-equality contract as assert_case, but over the TASK graph: the set of `build` and
#   `test` targets scheduled by the touched file must EQUAL the expected set.
#
#   Why a second query: `moon query projects --affected` follows `dependsOn` ONLY and is structurally
#   blind to a task-level `^:build` (SMA-429 F3). Delete the `^:build` from a moon.yml and every
#   project case above stays GREEN while `moon ci --include-relations` silently under-builds — the
#   exact hole SMA-524 exists to close. This case sees it.
#
#   Scoped to build/test because those are the two tasks that carry `^:build`. Including
#   fmt/lint/build-release/repo:* would couple the case to unrelated task config without adding any
#   assurance about the invariant under test.
# returns 0 pass / 1 assertion fail / 2 infrastructure error
assert_task_case() {
  local label="$1" file="$2" expected_csv="$3" got want missing unexpected
  [ -n "$expected_csv" ] || { echo "FATAL [$label]: EXPECTED_CSV is empty (harness bug)" >&2; return 2; }
  got="$(printf '%s\n' "$file" \
    | moon query tasks --affected --downstream deep \
    | python3 -c '
import sys, json
d = json.load(sys.stdin)
out = []
for pid, tasks in (d.get("tasks") or {}).items():
    for name in tasks:
        if name in ("build", "test"):
            out.append(f"{pid}:{name}")
print("\n".join(sorted(out)))')" \
    || { echo "FATAL [$label]: moon query tasks failed" >&2; return 2; }
  want="$(tr ',' '\n' <<<"$expected_csv" | sort)"
  if [ "$got" = "$want" ]; then
    printf 'PASS  %-18s -> %s\n' "$label" "$(tr '\n' ' ' <<<"$got")"
    return 0
  fi
  missing="$(comm -23 <(printf '%s\n' "$want") <(printf '%s\n' "$got"))"
  unexpected="$(comm -13 <(printf '%s\n' "$want") <(printf '%s\n' "$got"))"
  echo "FAIL  [$label] affected TASK set != expected set" >&2
  if [ -n "$missing" ]; then
    echo "  missing  (expected but not scheduled — likely a dropped task-level '^:build'):" >&2
    sed 's/^/    /' <<<"$missing" >&2
  fi
  if [ -n "$unexpected" ]; then
    echo "  unexpected (scheduled but not expected — if the new edge is intended, add it here):" >&2
    sed 's/^/    /' <<<"$unexpected" >&2
  fi
  return 1
}
```

- [ ] **Step 5: Add the behavioral task case and the parity gate to `run_suite`**

In `run_suite()`, immediately before the `assert_include_relations` line, add:

```bash
  # A proto edit must SCHEDULE paigasus-service-info's build and test, not merely mark the project
  # affected. This is the behavioral half of SMA-524: the parity gate asserts `^:build` is DECLARED,
  # this asserts it takes EFFECT.
  run_task_case "proto->service-info-tasks" "rs/crates/libs/paigasus-proto/src/lib.rs" \
    "paigasus-proto-rs:build,paigasus-proto-rs:test,paigasus-service-info-rs:build,paigasus-service-info-rs:test,paigasus-iam-rs:build,paigasus-iam-rs:test,paigasus-gateway-rs:build,paigasus-gateway-rs:test"
  # Generic Cargo<->Moon parity: catches a MISSING case, which is how SMA-524's bug survived review.
  assert_cargo_moon_parity || SUITE_RC=1
```

Add the two helpers next to `run_case` (after it, before `run_suite`):

```bash
# Task-graph twin of run_case — same 3-way return-code folding.
run_task_case() {
  local ec=0
  assert_task_case "$@" || ec=$?
  case "$ec" in
    0) ;;
    1) SUITE_RC=1 ;;
    *) echo "== affected-graph guard ABORTED: infrastructure error (rc=$ec) ==" >&2; exit 2 ;;
  esac
}

# Generic Cargo<->Moon parity gate. rc 2 (infra) aborts, mirroring run_case.
assert_cargo_moon_parity() {
  local ec=0
  python3 "$HERE/cargo_moon_parity.py" || ec=$?
  case "$ec" in
    0) return 0 ;;
    1) return 1 ;;
    *) echo "== affected-graph guard ABORTED: parity gate infrastructure error (rc=$ec) ==" >&2; exit 2 ;;
  esac
}
```

- [ ] **Step 6: Run the suite**

```bash
bash ci/affected-graph/run.sh; echo "exit=$?"
```
Expected: exit 0, with `PASS  proto->service-info-tasks` and `PASS  cargo-moon-parity` among the lines.

- [ ] **Step 7: Wire the parity self-test into the negative control**

In the `if [ "$NEGATIVE" = 1 ]` block, directly **before** the final `if [ "$NEG_RC" = 0 ]`, add:

```bash
  # 3) the parity gate must fire on synthetic violations of each of its three assertions — a gate
  #    that can pass vacuously reproduces the very bug it exists to prevent (SMA-524 D6).
  python3 "$HERE/cargo_moon_parity.py" --self-test || NEG_RC=1
```

- [ ] **Step 8: Run the negative control**

```bash
bash ci/affected-graph/run.sh --negative-control; echo "exit=$?"
```
Expected: exit 0, ending `negative-control OK: harness reported red on all wrong expectations`, and
including `OK   [parity] all three assertions fire on synthetic violations`.

- [ ] **Step 9: Bite check — the whole point of the task case**

Temporarily remove **only** the `^:build` entries from
`rs/crates/libs/paigasus-service-info/moon.yml` (keep `dependsOn`), then:

```bash
bash ci/affected-graph/run.sh 2>&1 | grep -E 'proto-derive->proto|service-info->services|service-info-tasks|cargo-moon-parity'
```

Expected — this is the finding that justifies the whole design:
- `PASS  proto-derive->proto` and `PASS  service-info->services` (project cases stay **green**)
- `FAIL  [proto->service-info-tasks]` with `missing: paigasus-service-info-rs:build/:test`
- `FAIL  [cargo-moon-parity]` with 2 A3 rows

Then restore the file:

```bash
git checkout -- rs/crates/libs/paigasus-service-info/moon.yml
bash ci/affected-graph/run.sh   # back to green
```

> **Gotcha:** restore with `git checkout --`, never by moving a `.bak` file back — `mv` rolls mtime
> backwards and downstream tooling then reuses stale artifacts.

- [ ] **Step 10: Commit**

```bash
git add ci/affected-graph/run.sh
git commit -m "test(ci): assert the service-info cascade at project and task level (SMA-524)" \
  -m "Adds the service-info->services project case, an assert_task_case helper with a behavioral
case proving a proto edit SCHEDULES service-info's build, and wires the parity gate plus its
self-test into the suite and the negative control. Removes the SMA-438 note recording the
gap this issue closed."
```

---

### Task 4: Correct the premise everywhere the repo records it

**Files:**
- Modify: `CLAUDE.md` (the "Gotchas" section)
- Modify: `rs/crates/libs/paigasus-proto/moon.yml:7-10`
- Modify: `rs/crates/libs/paigasus-kernel-parity/moon.yml:7-10`
- Modify: `ci/affected-graph/README.md:11-26`

This task ships no behavior. It is separated because a reviewer could reasonably approve the code and
reject the wording, or vice versa.

- [ ] **Step 1: Correct `CLAUDE.md`**

In the "Gotchas" section, add this bullet directly after the
`A new crate that dependsOn paigasus-kernel-rs …` bullet:

```markdown
- Moon 2.3.2's Rust toolchain resolves `path = "…"` Cargo deps into the project graph **automatically**
  (`moon query projects` labels them `source=implicit`), but does **not** resolve `workspace = true`
  inheritance. So a `{ workspace = true }` in-tree dep — the repo's default form — **must** be
  hand-declared in `dependsOn`, while a `path` dep needs nothing. This is why the drift was scattered
  rather than systematic, and it is the opposite of the "Cargo path deps are NOT auto-synced" claim
  that SMA-389 recorded and SMA-524 disproved. Either way the project edge alone is **not enough**:
  `dependsOn` is what `moon query projects --affected` follows, and a task-level `^:build` is what
  actually schedules the upstream's build under `moon ci --include-relations` — neither implies the
  other. `repo:affected-smoke` now asserts both generically for every crate
  (`ci/affected-graph/cargo_moon_parity.py`), so a new in-tree dep that forgets either one reds CI
  instead of silently under-building (SMA-524).
```

- [ ] **Step 2: Correct the two `moon.yml` comment blocks**

In `rs/crates/libs/paigasus-proto/moon.yml`, replace lines 7-10 with:

```yaml
# paigasus-proto-derive is a `workspace = true` dep (Cargo.toml:24), and Moon 2.3.2 does not resolve
# workspace-inherited deps into the project graph — only `path = "..."` ones — so this edge must be
# hand-declared. `dependsOn` creates the project edge; the task-level `^:build` is what schedules the
# derive crate's build under `moon ci --include-relations`. Both are required; neither implies the
# other, and `repo:affected-smoke` asserts both (SMA-389, corrected by SMA-524).
```

In `rs/crates/libs/paigasus-kernel-parity/moon.yml`, replace lines 7-10 with the same explanation,
substituting `paigasus-kernel` for the crate name and its own `Cargo.toml:12` citation.

> Verify the exact current line ranges with `sed -n '1,20p' <file>` before editing — do not trust
> these numbers blindly.

- [ ] **Step 3: Correct and extend `ci/affected-graph/README.md`**

Fix the three stale bullets and add the new ones. The contracts bullet must read:

```markdown
- **contracts edit** → `contracts` + `paigasus-proto-{rs,py,ts}` + `paigasus-gateway-rs` +
  `paigasus-iam-rs` + `paigasus-service-info-rs`.
```

The kernel bullet must gain `paigasus-iam-core-rs` + `paigasus-iam-rs`. Then add:

```markdown
- **derive-crate edit** → `paigasus-proto-derive-rs` + `paigasus-proto-rs` + `paigasus-gateway-rs` +
  `paigasus-iam-rs` + `paigasus-service-info-rs` (SMA-438/SMA-524). One-directional w.r.t. contracts.
- **service-info edit** → `paigasus-service-info-rs` + `paigasus-iam-rs` + `paigasus-gateway-rs`
  (SMA-524). One-directional w.r.t. `paigasus-proto`.

It also runs two checks that the per-case sets structurally cannot make:

- **`proto->service-info-tasks`** asserts the affected *task* set. `moon query projects --affected`
  follows `dependsOn` only and is blind to a task-level `^:build`, so a deleted `^:build` keeps every
  project case green while CI under-builds (SMA-429 F3, closed by SMA-524).
- **`cargo-moon-parity`** (`cargo_moon_parity.py`) compares every crate's Cargo deps against Moon's own
  resolved graph — asserting each edge exists and schedules the upstream's build. The per-case sets
  only assert edges someone wrote a case for; this catches a crate added with **no** case, which is
  how SMA-524's bug survived review.
```

- [ ] **Step 4: Verify nothing broke**

```bash
bash ci/affected-graph/run.sh && bash ci/affected-graph/run.sh --negative-control
```
Expected: both exit 0. (Docs-only task, but the README sits inside `ci/affected-graph/**/*`, which is a
task input.)

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md ci/affected-graph/README.md \
        rs/crates/libs/paigasus-proto/moon.yml \
        rs/crates/libs/paigasus-kernel-parity/moon.yml
git commit -m "docs(repo): correct the Cargo/Moon edge-inference rule (SMA-524)" \
  -m "Moon 2.3.2 DOES resolve path deps into the project graph and does NOT resolve
workspace-inherited ones — the opposite of what SMA-389 recorded in CLAUDE.md and two
moon.yml comment blocks. Also destales three affected-graph README bullets that drifted
from the live expected sets and documents the two new checks."
```

---

### Task 5: Full-graph verification

**Files:** none modified.

- [ ] **Step 1: Run the complete CI gate graph**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-524
moon ci :build :test :lint :fmt :deny :osv :machete :typecheck :breaking :affected-smoke \
  :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :promtool \
  :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  --base origin/main --include-relations
```

Expected: exit 0.

If Moon reports an unattributed failure, diagnose it with:

```bash
jq '.actions[] | select(.status=="failed") | {label, status}' .moon/cache/ciReport.json
```

- [ ] **Step 2: Confirm the diff matches the plan**

```bash
git diff --stat origin/main
```

Expected exactly 11 files: `cargo_moon_parity.py`, `run.sh`, `README.md` (affected-graph),
3 × service/lib `moon.yml` wired, 2 × `moon.yml` comment-corrected, `CLAUDE.md`, plus the spec and
this plan. Verify no stray debug code:

```bash
git diff origin/main | grep -nE '^\+.*(print\("DEBUG|TODO|FIXME|XXX|breakpoint\(\))' || echo "clean"
```

- [ ] **Step 3: Confirm the Docker-gated suites were not silently skipped**

`paigasus-iam`'s container tests `return` early — reporting PASS in ~0.7s having run nothing — unless
`CI=1` makes them panic. `nextest`'s `0 skipped` does **not** catch this.

```bash
cd rs && CI=1 cargo nextest run -p paigasus-iam --no-tests=pass --retries 2 2>&1 | tail -20
```

Expected: real test execution (tens of seconds), not a sub-second pass. If Docker is unavailable
locally, note it explicitly in the PR rather than claiming the suite passed.

---

## Self-Review

**Spec coverage:**

| Spec section | Task |
| --- | --- |
| § 6 parity gate A1/A2/A3 + allowlist | Task 1 |
| § 7.1–7.2 the three edges | Task 2 |
| § 7.3 `run.sh` cases, note deletion, task case | Task 3 |
| § 7.4 README destaling | Task 4 |
| § 7.5–7.6 CLAUDE.md + 2 moon.yml comments | Task 4 |
| § 10.1–10.2 suite + negative control | Tasks 3.6, 3.8 |
| § 10.3 bite checks | Task 3.9 |
| § 10.4 full gate graph | Task 5.1 |
| § 12 Docker cost | Task 5.3 |
| D6 negative control per assertion | Task 1.5, Task 3.7 |

Covered. § 8 (unchanged expected sets) needs no task — it is asserted implicitly by strict equality in
Task 3.3/3.6. D3, D7 and § 14 are explicitly out of scope; the follow-ups are filed at PR time.

**Placeholder scan:** no TBD/TODO; every code step carries literal content; no "similar to Task N".

**Type consistency:** `check(projects, crates)` returns `(a1, a2, a3)` and is called with that shape in
`self_test` and `main`. `moon_projects()` produces exactly the `{"source_dir", "deps", "tasks"}` shape
`check` destructures. `assert_task_case`/`run_task_case` mirror `assert_case`/`run_case`'s 0/1/2
contract. `$HERE` is defined at `run.sh:19` and is in scope for both new helpers.

**One residual risk, flagged not hidden:** Task 3.5 places `run_task_case`/`assert_cargo_moon_parity`
after `run_case`; Bash resolves functions at call time, so definition order relative to `run_suite`
does not matter — but both must be defined before `run_suite` is *invoked* at line ~180. They are.

<!-- moon-diagnosis:superseded -->
> **Superseded (SMA-597).** The `ciReport.json` diagnosis advice above does not work as written:
> there is no action-level `exitCode` key, and the file carries no stdout/stderr at all. The
> measured procedure is in CLAUDE.md between the `moon-diagnosis` markers. This document is left
> otherwise unedited as a record of what was believed when it was written.
