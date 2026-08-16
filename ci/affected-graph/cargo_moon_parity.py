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
    print("  OK   [parity] all three assertions fire on synthetic violations")
    return 0


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
             "    Fix: for `build`/`test`, add '^:build' to the task's `deps` in the consumer's\n"
             "    moon.yml. For `lint` the dep is declared once for ALL crates in\n"
             "    .moon/tasks/rust.yml — restore it there, not per-crate (SMA-526)."),
    ):
        if rows:
            print(f"  {title}", file=sys.stderr)
            for row in rows:
                print(f"      {row}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(self_test() if "--self-test" in sys.argv[1:] else main())
