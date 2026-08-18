# SPDX-License-Identifier: Apache-2.0
# SMA-524 — Cargo <-> Moon dependency-graph parity gate.
#
# The affected-graph guard (SMA-409/429) asserts only the edges someone remembered to write a CASE
# for. SMA-505 added a crate with no case, so its three missing edges survived a full review cycle.
# This gate is generic: it compares every crate's Cargo dependencies against Moon's OWN RESOLVED
# graph, so a new crate cannot repeat that failure.
#
# It never parses moon.yml (formatting-proof) and never shells out to cargo (repo:affected-smoke is
# toolchain: 'system', so cargo is not a dependency this script may take). Cargo.toml is parsed with
# tomllib, which — unlike the regex this replaced — handles dotted keys
# (`paigasus-kernel.workspace = true`), inline tables, and `package =` renames. That regex reported
# five sound edges as phantom.
#
# It also carries A4 (SMA-534), which is about task INPUTS rather than edges: every crate's `lint`
# must key on the workspace-level files (Cargo.lock, Cargo.toml, rust-toolchain.toml), since `rs/`
# has no Moon project for a dependency edge to point at. A4 reads moon's RESOLVED `inputFiles`, so
# it stays inside the "never parse YAML" rule above.
#
# usage: cargo_moon_parity.py [--self-test]
import json
import subprocess
import sys
import tempfile
import tomllib
from pathlib import Path


class MoonOutputError(RuntimeError):
    """Moon's query output did not have the shape this gate requires.

    Raised — never returned as a violation row — when moon reports a task with none of a `command`,
    a `script`, or any `args` (moon_projects() joins all three into the text A5 marker-matches
    against, so only the absence of all three counts). That is "moon told us nothing", which must
    abort as an infrastructure error (rc 2) rather than be folded into an assertion failure, exactly
    as A4 treats an absent `inputFiles` key. A moon upgrade that reshapes the task object must fail
    loudly, not quietly stop asserting.
    """


# Exceptions meaning "the inputs or environment are broken", NOT "the graph regressed". main() maps
# these to rc 2 so run.sh aborts, instead of folding them into SUITE_RC as an assertion failure.
# tomllib.TOMLDecodeError subclasses ValueError, not OSError, so it has to be named explicitly — the
# self-test pins that by asserting cargo_crates' real failure is a member of this tuple.
INFRA_ERRORS = (
    subprocess.CalledProcessError,
    json.JSONDecodeError,
    tomllib.TOMLDecodeError,
    OSError,
    MoonOutputError,
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

# SMA-534 — the workspace-level files `lint` must key on. `rs/` has no Moon project, so without
# these declared on the inherited lint task a Cargo.lock-only change (every Dependabot Cargo PR)
# schedules no crate task at all. Paths are workspace-relative, exactly as Moon RESOLVES them:
# the YAML says `/rs/Cargo.lock`, `moon query projects` reports `rs/Cargo.lock`.
WORKSPACE_LINT_INPUTS = ("rs/Cargo.lock", "rs/Cargo.toml", "rs/rust-toolchain.toml")

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
# `maturin` is FORWARD-LOOKING and matches no RESOLVED moon task invocation today: the string
# shows up elsewhere in the tree (pyproject.toml, moon.yml comments, the python template), but
# paigasus-kernel-py:test's actual resolved script is
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
    return a1, a2, a3


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


def check_ffi_inputs(projects, required=FFI_TASK_INPUTS, floor=REQUIRED_FFI_TASKS):
    """Return the A5 violation list: FFI-compiling tasks that do not key on the workspace files.

    Two halves, and both are load-bearing:

    * DERIVED — any task whose resolved invocation matches an FFI marker must declare `required`.
      This is what covers a future fourth binding task on the day it is added.
    * FLOOR — every task in `floor` must appear in the derived set. Without this a derivation that
      silently stops matching (a renamed flag, an invocation moved behind a wrapper script, a moon
      upgrade dropping `script`) degrades to an empty set and a vacuous PASS.

    Raises MoonOutputError if a task exposes none of a command, a script, or any args.
    """
    matched, a5 = set(), []
    for pid in sorted(projects):
        invocations = projects[pid].get("invocations") or {}
        declared = projects[pid].get("task_inputs") or {}
        for name in sorted(invocations):
            blob = invocations[name]
            if blob is None:
                raise MoonOutputError(
                    f"{pid}:{name} reported none of a `command`, a `script`, or any `args` — "
                    f"moon's output shape changed, so A5 cannot be evaluated"
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
        task_inputs = {}
        invocations = {}
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
            # A5 (SMA-546): the text A5 marker-matches against. Joined from all three fields
            # because moon splits an invocation differently per task form — command-form puts the
            # verb in `args` (command='cargo', args=['clippy', ...]), script-form puts it in
            # `script` (command='touch', script='touch ... && napi build ...'). None when moon
            # reported neither, which check_ffi_inputs escalates to an infra error.
            parts = [task.get("command") or "", task.get("script") or ""]
            parts += [str(a) for a in (task.get("args") or [])]
            joined = " ".join(p for p in parts if p)
            invocations[name] = joined or None
        projects[p["id"]] = {
            "source_dir": p["source"],
            "deps": {d["id"]: d.get("source") for d in (p.get("dependencies") or [])},
            "tasks": tasks,
            "task_inputs": task_inputs,
            "invocations": invocations,
        }
    return projects


def cargo_crates(root):
    """Every workspace crate and its in-tree deps, via tomllib.

    tomllib handles all three declaration forms the repo uses — inline table
    (`paigasus-proto = { workspace = true }`), dotted key
    (`paigasus-proto-derive.workspace = true`), and `package =` renames.
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


def self_test():
    """Negative control: each assertion must FIRE on a synthetic violation (SMA-524 D6).

    A gate whose whole value is catching a silent hole must not be able to pass vacuously.
    """
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
    broken["a-rs"]["tasks"] = {"build": [], "test": [], "lint": []}
    if not check(broken, crates)[2]:
        failures.append("A3 did not fire on an unscheduled upstream build")

    # An ABSENT task key is a different defect from a task that exists but omits the dep, and the
    # violation text has to say which — otherwise the first crate to drop or rename `lint` is told
    # to "add '^:build'" to a task it does not have (SMA-526).
    broken = json.loads(json.dumps(ok))
    del broken["a-rs"]["tasks"]["lint"]
    if not any("has no `lint` task" in row for row in check(broken, crates)[2]):
        failures.append("A3 did not distinguish an absent task from a missing dep")

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

    if check_ffi_inputs(ffi_ok):
        failures.append("A5 reported violations on the clean fixture")

    # Fires when a matched task omits one of the required files.
    broken = json.loads(json.dumps(ffi_ok))
    broken["paigasus-kernel-ts"]["task_inputs"]["build"] = [
        "rs/Cargo.lock", "rs/Cargo.toml", "rs/rust-toolchain.toml"
    ]
    rows = check_ffi_inputs(broken)
    if not rows:
        failures.append("A5 did not fire on a missing FFI workspace input")
    elif not any(".prototools" in row for row in rows):
        failures.append("A5 fired but did not name the missing file")

    # Moon emitting no `inputFiles` for a MATCHED FFI task must FIRE, never silently skip.
    broken = json.loads(json.dumps(ffi_ok))
    broken["paigasus-kernel-ts"]["task_inputs"]["build"] = None
    if not any("inputFiles" in row for row in check_ffi_inputs(broken)):
        failures.append("A5 did not fire when moon reported no inputFiles")

    # THE ANTI-VACUITY ROW. Neuter the marker match (as a package.json indirection or a renamed
    # uv flag would) and A5's derived set empties. Without the floor this reports PASS while
    # asserting nothing — the exact silent-degradation mode the floor exists to stop.
    broken = json.loads(json.dumps(ffi_ok))
    for task in ("build", "test"):
        broken["paigasus-kernel-ts"]["invocations"][task] = "pnpm run build:native"
    rows = check_ffi_inputs(broken)
    if not any("not matched by any FFI marker" in row for row in rows):
        failures.append("A5 did not fire when a required FFI task stopped matching the markers")

    # A task exposing NEITHER a command NOR a script is moon telling us nothing — infra (rc 2),
    # not an assertion failure. Mirrors A4's absent-inputFiles rule.
    broken = json.loads(json.dumps(ffi_ok))
    broken["paigasus-kernel-ts"]["invocations"]["build"] = None
    try:
        check_ffi_inputs(broken)
    except MoonOutputError:
        pass
    else:
        failures.append("A5 did not raise infra on a task with no command and no script")

    # A matched task that is NOT in the floor is still asserted — this is the derived half.
    broken = json.loads(json.dumps(ffi_ok))
    broken["unrelated-ts"]["invocations"]["build"] = "wasm-pack build ."
    if not any("unrelated-ts:build" in row for row in check_ffi_inputs(broken)):
        failures.append("A5 did not assert a newly-matched task outside the floor")

    # A malformed Cargo.toml must surface as INFRA (rc 2), not as an assertion failure. Exercise the
    # whole chain on a throwaway workspace — parser raises, cargo_crates propagates, INFRA_ERRORS
    # catches — so narrowing that tuple fails here instead of silently relabelling a broken manifest
    # as a graph regression. Done without Moon on purpose: `moon query projects` parses Cargo.toml
    # too and fails first, which masks this path end-to-end.
    with tempfile.TemporaryDirectory() as tmp:
        bad = Path(tmp) / "rs" / "crates" / "bad"
        bad.mkdir(parents=True)
        (bad / "Cargo.toml").write_text("[package\nname = ")
        try:
            cargo_crates(Path(tmp))
        except INFRA_ERRORS:
            pass
        else:
            failures.append("a malformed Cargo.toml did not raise out of cargo_crates()")

    for f in failures:
        print(f"  FAIL {f}", file=sys.stderr)
    if failures:
        print("negative-control FAILED: the parity gate can pass vacuously", file=sys.stderr)
        return 1
    print("  OK   [parity] all five assertions fire on synthetic violations")
    return 0


def main():
    root = Path(__file__).resolve().parents[2]
    try:
        projects = moon_projects()
        crates = cargo_crates(root)
        a5 = check_ffi_inputs(projects)
    except INFRA_ERRORS as exc:
        # Mirror run.sh's infra-vs-assertion split: a broken `moon` — or an unparseable Cargo.toml —
        # must never be mistaken for a graph regression. See INFRA_ERRORS.
        print(f"FATAL [parity] could not build the graphs: {exc}", file=sys.stderr)
        return 2

    a1, a2, a3 = check(projects, crates)
    a4 = check_lint_inputs(projects, crates)
    if not (a1 or a2 or a3 or a4 or a5):
        print(
            f"PASS  {'cargo-moon-parity':<18} -> "
            f"{len(crates)} crates: every Cargo dep has a Moon edge that schedules its build, "
            f"every lint keys on the workspace files, and every FFI build task does too"
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
             "    Fix: for `build`/`test`, add '^:build' to the task's `deps` in the consumer's\n"
             "    moon.yml. For `lint` the dep is declared once for ALL crates in\n"
             "    .moon/tasks/rust.yml — restore it there, not per-crate (SMA-526)."),
        (a4, "`lint` does not key on the workspace-level files, so a dependency bump, a\n"
             "    [workspace.lints] edit or a toolchain drift schedules NOTHING for this crate\n"
             "    (SMA-534).\n"
             "    Fix: the inputs are declared once for ALL crates in .moon/tasks/rust.yml —\n"
             "    restore them there, not per-crate. Expected: /rs/Cargo.lock, /rs/Cargo.toml,\n"
             "    /rs/rust-toolchain.toml."),
        (a5, "An FFI build task does not key on the workspace-level files, so a dependency bump\n"
             "    replays a CACHED artifact built from a different resolution — and clippy cannot\n"
             "    cover it, because it never links a cdylib and never targets wasm32 (SMA-546).\n"
             "    Fix: add /rs/Cargo.lock, /rs/Cargo.toml, /rs/rust-toolchain.toml and\n"
             "    /.prototools to that task's `inputs`. A `not matched by any FFI marker` row\n"
             "    means the opposite — the task stopped looking like a Rust build to A5; either\n"
             "    restore the invocation or update FFI_MARKERS."),
    ):
        if rows:
            print(f"  {title}", file=sys.stderr)
            for row in rows:
                print(f"      {row}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(self_test() if "--self-test" in sys.argv[1:] else main())
