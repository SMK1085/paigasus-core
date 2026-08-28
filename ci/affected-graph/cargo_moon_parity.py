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
# must key on the workspace-level files (Cargo.lock, Cargo.toml, rust-toolchain.toml and, since
# SMA-594, .cargo/config.toml), since `rs/` has no Moon project for a dependency edge to point at. A4 reads moon's RESOLVED `inputFiles`, so
# it stays inside the "never parse YAML" rule above.
#
# usage: cargo_moon_parity.py [--self-test]
import inspect
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
        "Over-approximation, not a defect: the gateway has no Cargo dep on the kernel. Removing the "
        "edge would change the kernel->bindings expected set that SMA-409 owns (SMA-524 D4). NOTE "
        "(SMA-528): this is no longer free. The edge now feeds @group(upstreams), so every kernel "
        "edit runs the gateway's full build+test+lint for a dependency that does not exist. Revisit "
        "if kernel PRs approach the CI budget — that is the first thing to drop."
    ),
}

# Build-scope parents injected by a task dep (e.g. `contracts:generate`), never Cargo deps.
# Used twice: A2 must not report them as unbacked Moon edges, and A6's closure walk must not demand
# `src/**/*` + `Cargo.toml` globs for them — they have neither tree (SMA-528).
NON_CARGO_PARENTS = {"contracts"}

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

# SMA-546 — A5. The tasks that COMPILE the FFI cdylibs live in the ts/py stacks, so A4's
# per-crate loop cannot reach them: `moon query projects` lists them under their own project ids,
# not under any Rust crate. They must key on the same workspace files as `lint`, plus `.prototools`
# — which pins `wasm-pack` and is therefore the OTHER half of the rs/Cargo.toml:90-97 invariant
# ("the pinned wasm-pack must support that 0.2.z — bump the two together").
FFI_TASK_INPUTS = (*WORKSPACE_LINT_INPUTS, ".prototools")

# SMA-537 — what every crate's `fmt` must key on. The two globs come from the shared fileGroups
# and land in moon's `inputGlobs`; the two literals land in `inputFiles`. check_task_inputs spans
# both, which is the whole reason it does not read a single bucket the way its lint-only ancestor did.
FMT_TASK_INPUTS = (
    "rs/rustfmt.toml",
    "rs/rust-toolchain.toml",
    "Cargo.toml",
    "src/**/*",
    "tests/**/*",
)

# Substrings that mean "this task shells out to a Rust build". Matched against the task's resolved
# `command` + `args` + `script` joined — NOT `command` alone: measured on moon 2.5.3, a
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

# SMA-528 — the tasks that must key on their crate's upstream sources. `fmt` is crate-local by
# construction and `build-release` never runs in CI; neither carries `^:build` either.
UPSTREAM_INPUT_TASKS = ("build", "test", "lint")

# Consumer -> upstream pairs deliberately declared in `fileGroups.upstreams` WITHOUT being in the
# crate's Moon closure. A6 is strict-equality (SMA-429's default-deny model), so an intentional
# over-approximation needs an entry here with a non-empty reason. Empty today.
#
# MIND THE KEY SHAPE: it is (moon id, upstream SOURCE DIR) — e.g.
# ("paigasus-iam-rs", "rs/crates/libs/paigasus-kernel") — NOT the (moon id, moon id) shape
# ALLOW_NO_CARGO_BACKING uses twelve lines above. A6 recovers the upstream half STRUCTURALLY, from
# the first four `/`-separated segments of a resolved input path (`rs/crates/<layer>/<crate>`),
# not by stripping a known suffix — an over-approximating entry can carry any suffix now, not just
# `/src/**/*` or `/Cargo.toml`. A moon id never appears in a resolved path. A waiver written in the
# neighbouring shape is silently inert.
ALLOW_OVER_APPROXIMATION = {}

# A6's anti-vacuity floor, mirroring REQUIRED_FFI_TASKS. A6 DERIVES each crate's closure from
# moon's `dependencies` key; a moon rename or JSON reshape would empty every closure and A6 would
# print PASS for thirteen crates while asserting nothing. These edges must survive the derivation.
REQUIRED_CLOSURE_EDGES = {
    "paigasus-iam-rs": {"paigasus-kernel-rs", "paigasus-proto-rs"},
    "paigasus-kernel-parity-rs": {"paigasus-kernel-rs"},
}

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

# "moon reported no such task key at all", distinct from both None and []. A unique object, never a
# string: a string default flows into `set(declared or [])` and is iterated CHARACTER-WISE, turning
# a half-reported task into eight bogus single-letter entries instead of one honest violation.
_ABSENT = object()


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
    about them. An entry is matched if it appears in EITHER bucket verbatim; a crate-relative entry
    (`src/**/*`, `tests/**/*` — the group-derived globs) is also matched against the crate's OWN
    `source_dir`, because moon resolves those groups to exactly `<source_dir>/src/**/*` (SMA-560 M3).

    That anchoring replaced a `not f.startswith("rs/")` test plus an unanchored tail match, which
    was weak in two directions at once. A required entry outside `rs/` (`.prototools`, a
    `contracts/...` path) would have silently gained tail matching on the day it was added; and an
    unanchored tail let ANOTHER crate's glob satisfy this crate — `a-rs:fmt` counted as covered by
    `rs/crates/libs/b/src/**/*`. Anchoring drops the `rs/`-prefix special case entirely: a
    workspace-relative entry still has to match verbatim, since `<source_dir>/rs/rustfmt.toml` is
    not a path moon ever resolves.
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
        missing = []
        for f in required:
            if f in observed:
                continue
            # Crate-relative entries (`src/**/*`, `tests/**/*`) resolve to exactly
            # `<source_dir>/<entry>`, so ANCHOR the fallback to this crate's own source_dir rather
            # than accepting any tail match (SMA-560 M3). A workspace-relative entry (`rs/...`)
            # never matches this form, so it keeps its verbatim-only rule with no special case.
            if f"{info['source_dir']}/{f}" in observed:
                continue
            missing.append(f)
        if missing:
            a4.append(f"{mid}:{task} inputs omit {', '.join(missing)}")
    return a4


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


def check_ffi_inputs(projects, required=FFI_TASK_INPUTS, floor=REQUIRED_FFI_TASKS):
    """Return the A5 violation list: FFI-compiling tasks that do not key on the workspace files.

    Two halves, and both are load-bearing:

    * DERIVED — any task whose resolved invocation matches an FFI marker must declare `required`.
      This is what covers a future fourth binding task on the day it is added.
    * FLOOR — every task in `floor` must appear in the derived set. Without this a derivation that
      silently stops matching (a renamed flag, an invocation moved behind a wrapper script, a moon
      upgrade dropping `script`) degrades to an empty set and a vacuous PASS.

    The derivation itself lives in `derive_ffi_tasks`, shared with A7.

    Raises MoonOutputError if a task exposes none of a command, a script, or any args.
    """
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


def rust_closure(projects, pid, seen=None, origin=None):
    """Transitive dependsOn closure of `pid`, restricted to Rust projects.

    Restricted because Moon injects non-Rust build-scope parents into `dependencies`: `contracts`
    arrives via the `contracts:generate` task dep (which is what NON_CARGO_PARENTS already exists
    for) and has neither a `src/` tree nor a Cargo.toml, so an unfiltered closure would demand
    globs matching nothing for four crates.

    Transitive, not direct: affectedness does NOT propagate through `^:build` (SMA-528 F1), so with
    A -> B -> C an edit to C would otherwise reach B and stop.

    `origin` is the crate the walk started from and is excluded at EVERY depth, not just the first.
    Comparing against `pid` instead would only skip a direct self-edge: in a cycle A -> B -> A the
    walk from A would return {B, A}, putting A's own pair in `want` while `observed` deliberately
    excludes it — an `inputs omit <own>/src/**/*` row no declaration could ever satisfy. A self-edge
    deeper in is already covered by `dep in seen`, since `seen` gains `dep` before the recursion.
    """
    seen = set() if seen is None else seen
    origin = pid if origin is None else origin
    for dep in sorted((projects.get(pid) or {}).get("deps") or {}):
        if dep == origin or dep in NON_CARGO_PARENTS or dep not in projects:
            continue
        if projects[dep].get("language") != "rust" or dep in seen:
            continue
        seen.add(dep)
        rust_closure(projects, dep, seen, origin)
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

    # The crates the per-crate loop below will actually examine. Computed here because the floor
    # has to assert MEMBERSHIP as well as derivation: `rust_closure` filters on each DEPENDENCY's
    # language but never on the ROOT's, so a crate that stopped reporting `language: rust` — a moon
    # toolchain reshuffle, a hand-edited `language:` — drops silently out of the loop while its
    # closure still derives perfectly and the floor still passes.
    examined = {pid for pid, proj in projects.items() if proj.get("language") == "rust"}

    # FLOOR first: if the derivation broke, every per-crate check below is vacuous. Every row here
    # is prefixed `FLOOR:` so the negative control can tell a floor failure from a per-crate one.
    for consumer, required in sorted((floor or {}).items()):
        if consumer not in projects:
            a6.append(f"FLOOR: {consumer} is not in the graph at all")
            continue
        if consumer not in examined:
            a6.append(
                f"FLOOR: {consumer} is not among the {len(examined)} crates A6 examines — it "
                f"stopped reporting `language: rust`, so the per-crate loop skips it entirely"
            )
            continue
        derived = rust_closure(projects, consumer)
        for upstream in sorted(set(required) - derived):
            a6.append(
                f"FLOOR: {consumer}'s derived closure omits {upstream} — the dependsOn "
                f"derivation is broken, so A6 is asserting nothing"
            )

    for pid, proj in sorted(projects.items()):
        if pid not in examined:
            continue
        own = proj["source_dir"]
        want = set()
        for upstream in rust_closure(projects, pid):
            src = projects[upstream]["source_dir"]
            want.add(f"{src}/src/**/*")
            want.add(f"{src}/Cargo.toml")
        for task in tasks:
            declared_files = (proj.get("task_inputs") or {}).get(task, _ABSENT)
            declared_globs = (proj.get("task_input_globs") or {}).get(task, _ABSENT)
            if declared_files is _ABSENT and declared_globs is _ABSENT:
                a6.append(f"{pid} has no `{task}` task (nothing can key on its upstreams)")
                continue
            # One bucket present and the other missing entirely is moon half-reporting the task —
            # folded in with the None case, since both mean "this assertion cannot be evaluated".
            # Never fall through: `_ABSENT` in the union below would be a TypeError at best, and a
            # string sentinel there would silently iterate character-wise.
            if _ABSENT in (declared_files, declared_globs) or None in (
                declared_files,
                declared_globs,
            ):
                a6.append(
                    f"{pid}:{task} reported no inputFiles/inputGlobs — moon's output shape "
                    f"changed, so this assertion cannot be evaluated (treated as a violation, "
                    f"never skipped)"
                )
                continue
            resolved = set(declared_files or []) | set(declared_globs or [])
            # Observed = every entry pointing INTO another crate's tree. The crate's own
            # `src/**/*`, `tests/**/*`, `**/*_test.rs`, and `Cargo.toml` come from
            # .moon/tasks/rust.yml and are not upstreams — excluded below by the `own/` prefix
            # check, not by suffix, so this needs no updating when rust.yml grows a new own-crate
            # glob shape.
            #
            # Deliberately WIDE: any entry under `rs/crates/` that is not the crate's own enters
            # `observed`, regardless of suffix. A prior version matched only `/src/**/*` or
            # `/Cargo.toml`, so a broad `.../paigasus-kernel/**/*` or a
            # `.../paigasus-kernel/tests/**/*` sitting beside the correct pair never entered
            # `observed` and could never surface in the `observed - want` diff below — silently
            # widening what a crate's build/test/lint keys on, the exact direction strict equality
            # was chosen to catch (SMA-429). What this filter still CANNOT see: an over-declared
            # entry living OUTSIDE `rs/crates/**` entirely (e.g. a stray `contracts/**/*`) — such
            # an entry can never be in `want` either, so it is invisible to strict equality in
            # both directions regardless of this filter. Under-declaration is unaffected either
            # way: a missing correct-shaped pair still fails `want - observed`.
            observed = {
                e for e in resolved if e.startswith("rs/crates/") and not e.startswith(f"{own}/")
            }
            for entry in sorted(want - observed):
                a6.append(f"{pid}:{task} inputs omit {entry}")
            for entry in sorted(observed - want):
                # Structural derivation, not suffix-stripping: `entry` may now carry any suffix
                # (`/src/**/*`, `/Cargo.toml`, `/**/*`, `/tests/**/*`, ...), but the first four
                # `/`-separated segments are always `rs/crates/<layer>/<crate>`.
                upstream = "/".join(entry.split("/")[:4])
                if not _allowlisted(allow, pid, upstream):
                    a6.append(f"{pid}:{task} inputs include {entry}, which is not in its closure")
    return a6


def check_wrapper_upstream_inputs(projects, root, floor=REQUIRED_WRAPPER_CLOSURE):
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

    `root` is POSITIONAL AND REQUIRED, never defaulted (SMA-560 I3). It gates the `build.rs`
    half — the branch that catches this branch's own headline under-declaration — and a
    `root=None` default made that half opt-in: a call site that simply stopped passing it went
    on printing PASS while the two `paigasus-node-bindings/build.rs` lines could be deleted from
    `ts/packages/paigasus-kernel/moon.yml` for free. Required means the same mistake is now a
    TypeError at the call site instead of a silent downgrade.
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
            # so a change to it changes what the wrapper links. Only demanded when one exists on
            # disk — which is why `root` is required rather than defaulted (see the docstring).
            if (root / src / "build.rs").is_file():
                want.add(f"{src}/build.rs")
            # SMA-594'. A hand-written .pyi is the interface contract between a PyO3 cdylib and
            # every Python consumer, and it is what basedpyright reads INSTEAD of the Rust. It
            # lives at the crate ROOT, so `{src}/src/**/*` does not match it and nothing that
            # validates it keyed on it. Disk-conditional and globbed, mirroring build.rs above:
            # conditional so the twelve crates without a stub gain no dead demand, globbed so a
            # second stub is covered the day it appears rather than needing a hand-maintained list.
            for stub in sorted((root / src).glob("*.pyi")):
                want.add(f"{src}/{stub.name}")
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
        task_input_globs = {}
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
            # SMA-528 — A6 needs BOTH buckets. moon splits resolved inputs by kind: plain paths go
            # to `inputFiles`, globs to `inputGlobs` (measured, SMA-534). `src/**/*` is a glob and
            # `Cargo.toml` is a path, so an A6 that read only one of them could never fire on half
            # of every upstream pair. Same absent-key-is-None contract as `task_inputs`.
            raw_globs = task.get("inputGlobs")
            task_input_globs[name] = None if raw_globs is None else sorted(raw_globs.keys())
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
            "task_input_globs": task_input_globs,
            "invocations": invocations,
            "language": p.get("language"),
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
    # Derived, never hand-listed: this fixture is what A4's CLEAN row asserts against, so a
    # hardcoded copy silently breaks every A4 self-test row the day WORKSPACE_LINT_INPUTS grows
    # (it did, on SMA-594). Deriving it means a new workspace input is covered on day one.
    complete_inputs = list(WORKSPACE_LINT_INPUTS)
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
    # A6 (SMA-528) additions to the SAME fixture, so A1-A5 keep being asserted against it. Sorted
    # lists rather than sets throughout: the negative controls below deep-copy via a json round
    # trip, which a set cannot survive.
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
    crates = {
        "a": {"source_dir": "rs/crates/libs/a", "deps": {"b"}},
        "b": {"source_dir": "rs/crates/libs/b", "deps": set()},
    }
    failures = []

    # Floor the floor: REQUIRED_CLOSURE_EDGES, UPSTREAM_INPUT_TASKS and REQUIRED_FFI_TASKS are
    # themselves module constants that can be silently emptied (a bad merge, an over-eager
    # refactor). Every A6 self-test call below passes an explicit `floor=`, so the real
    # REQUIRED_CLOSURE_EDGES is never exercised by anything else here — `REQUIRED_CLOSURE_EDGES =
    # {}` unasserts A6's floor entirely with `--self-test` still exiting 0 (measured). Every A6
    # call also relies on the default `tasks=UPSTREAM_INPUT_TASKS`, and A5's calls rely on the
    # default `floor=REQUIRED_FFI_TASKS`; emptying either is caught TODAY only incidentally, by
    # unrelated row-text assertions elsewhere in this function, which is not a guarantee — so all
    # three get a direct, explicit non-emptiness check here instead.
    if not REQUIRED_CLOSURE_EDGES:
        failures.append("REQUIRED_CLOSURE_EDGES is empty — A6's floor would assert nothing")
    if not UPSTREAM_INPUT_TASKS:
        failures.append("UPSTREAM_INPUT_TASKS is empty — A6's per-crate loop would assert nothing")
    if not REQUIRED_FFI_TASKS:
        failures.append("REQUIRED_FFI_TASKS is empty — A5's floor would assert nothing")

    if not FMT_TASK_INPUTS:
        failures.append("FMT_TASK_INPUTS is empty — the fmt half of A4 would assert nothing")

    # A4-fmt: the fmt call must span BOTH buckets. `rs/rustfmt.toml` is a literal (inputFiles)
    # and `rs/crates/libs/b/tests/**/*` is a glob (inputGlobs), so a one-bucket check passes
    # while blind to half its own required set — the split A6 already exists because of.
    fmt_ok = json.loads(json.dumps(ok))
    for pid in ("a-rs", "b-rs"):
        fmt_ok[pid]["task_inputs"]["fmt"] = [
            "rs/rustfmt.toml", "rs/rust-toolchain.toml", f"{fmt_ok[pid]['source_dir']}/Cargo.toml",
        ]
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

    # A4-fmt: the manifest half. `cargo fmt` reads Cargo.toml for its target list and edition, so
    # a fmt task blind to it can serve a cached PASS across a [[bin]] addition (CodeRabbit, PR 174).
    broken = json.loads(json.dumps(fmt_ok))
    broken["a-rs"]["task_inputs"]["fmt"] = ["rs/rustfmt.toml", "rs/rust-toolchain.toml"]
    if not any(
        "Cargo.toml" in row
        for row in check_task_inputs(broken, crates, "fmt", FMT_TASK_INPUTS)
    ):
        failures.append("A4-fmt did not fire on a fmt task missing its crate's Cargo.toml")

    broken = json.loads(json.dumps(fmt_ok))
    broken["a-rs"]["task_inputs"]["fmt"] = ["rs/rust-toolchain.toml"]
    if not any(
        "rustfmt.toml" in row
        for row in check_task_inputs(broken, crates, "fmt", FMT_TASK_INPUTS)
    ):
        failures.append("A4-fmt did not fire on a fmt task missing the rustfmt config")

    # A4's crate-relative fallback is ANCHORED to the crate's own source_dir (SMA-560 M3). The
    # previous unanchored tail match let ANOTHER crate's globs satisfy this one: `a-rs:fmt` counted
    # as covered by `rs/crates/libs/b/src/**/*`, so a crate that lost @group(sources) entirely was
    # still reported green as long as some other crate declared its own.
    broken = json.loads(json.dumps(fmt_ok))
    src_b = fmt_ok["b-rs"]["source_dir"]
    broken["a-rs"]["task_input_globs"]["fmt"] = [f"{src_b}/src/**/*", f"{src_b}/tests/**/*"]
    if not any(
        row == "a-rs:fmt inputs omit src/**/*, tests/**/*"
        for row in check_task_inputs(broken, crates, "fmt", FMT_TASK_INPUTS)
    ):
        failures.append("A4 let one crate's fmt be satisfied by ANOTHER crate's source globs")

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

    # Guard the guard (SMA-542). A check that is defined but never invoked on the real run asserts
    # nothing, and no fixture here would notice — self_test calls the check functions directly.
    # This is generic on purpose: it covers a future A8 on the day it is written. It scans BOTH
    # `main` and `collect_findings`, since SMA-560 moved every invocation into the latter.
    real_run_src = inspect.getsource(main) + inspect.getsource(collect_findings)
    unreferenced = sorted(
        name for name in globals()
        if name.startswith("check_") and f"{name}(" not in real_run_src
    )
    if unreferenced:
        failures.append(
            f"the real run never calls {', '.join(unreferenced)} — a check that is defined but "
            f"not invoked asserts nothing (SMA-542)"
        )

    # ...and the name scan above is only HALF the guard (SMA-560 I4). It cannot separate two call
    # sites of the same function, and it never sees `check` at all (no `check_` prefix), so three
    # measured deletions from the findings list left `--self-test` green with a real assertion
    # gone: the `a5` tuple, either `check_task_inputs` tuple, and the `a1`/`a2`/`a3` tuples. Pin
    # the LIST itself — arity first, so a shrunk list says so plainly, then the exact key sequence.
    if not EXPECTED_FINDING_KEYS:
        failures.append("EXPECTED_FINDING_KEYS is empty — the findings floor would assert nothing")
    with tempfile.TemporaryDirectory() as tmp:
        collected = collect_findings(ok, crates, Path(tmp))
    if len(collected) != len(EXPECTED_FINDING_KEYS):
        failures.append(
            f"collect_findings returned {len(collected)} entries, expected "
            f"{len(EXPECTED_FINDING_KEYS)} — a check was added or dropped without updating "
            f"EXPECTED_FINDING_KEYS"
        )
    got_keys = tuple(key for key, _, _ in collected)
    if got_keys != EXPECTED_FINDING_KEYS:
        failures.append(
            f"collect_findings reported {got_keys}, expected {EXPECTED_FINDING_KEYS} — a check "
            f"was dropped, added or reordered in the findings list"
        )

    if not REQUIRED_WRAPPER_CLOSURE:
        failures.append("REQUIRED_WRAPPER_CLOSURE is empty — A7's floor would assert nothing")

    # A7 fixture: a ts wrapper depending on a binding crate that depends on the kernel. The
    # wrapper declares the manifest as a literal and the sources as a glob — the same two-bucket
    # split A6 spans — plus one legitimate extra outside its closure (the parity corpus), which
    # containment must ALLOW and strict equality would have wrongly flagged.
    #
    # TWO tasks, and that is load-bearing (SMA-560 I2). A one-task wrapper cannot tell a per-task
    # read from one that unions across the wrapper's tasks, so the per-(project, task) read the
    # docstring names as one of A7's three deliberate differences would be held by nothing. `build`
    # and `test` therefore declare DIFFERENT complete sets, exactly as the real wrappers do, and
    # A7-g below drops an entry from `test` alone that `build` still carries.
    wrap = {
        "k-ts": {
            "source_dir": "ts/packages/k", "deps": {"nb-rs": "explicit"}, "language": "typescript",
            "tasks": {"build": [], "test": []},
            "task_inputs": {
                "build": ["rs/crates/libs/kern/Cargo.toml",
                          "rs/crates/bindings/nb/Cargo.toml"],
                "test": ["rs/crates/libs/kern/Cargo.toml",
                         "rs/crates/bindings/nb/Cargo.toml",
                         "rs/crates/bindings/nb/package.json"],
            },
            "task_input_globs": {
                "build": ["rs/crates/libs/kern/src/**/*",
                          "rs/crates/bindings/nb/src/**/*",
                          "rs/crates/libs/parity/vectors/**/*"],
                "test": ["rs/crates/libs/kern/src/**/*",
                         "rs/crates/bindings/nb/src/**/*"],
            },
            "invocations": {
                "build": "pnpm exec napi build --platform",
                "test": "touch ../x && pnpm exec napi build --platform && vitest run",
            },
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
    # A7's `root` is required, so every call below passes one. This tempdir holds NO build.rs, so
    # `want` stays at the two-entry-per-upstream shape these rows are written against; the
    # build.rs half gets its own fixture tree in A7-h.
    with tempfile.TemporaryDirectory() as no_build_rs:
        bare = Path(no_build_rs)

        if check_wrapper_upstream_inputs(wrap, bare, floor=wrap_floor) != []:
            failures.append(
                f"A7 reported violations on a complete fixture: "
                f"{check_wrapper_upstream_inputs(wrap, bare, floor=wrap_floor)}"
            )

        # A7-a: a MISSING upstream glob is the dangerous direction and must fire.
        broken = json.loads(json.dumps(wrap))
        broken["k-ts"]["task_input_globs"]["build"] = ["rs/crates/libs/kern/src/**/*"]
        if not any(
            "rs/crates/bindings/nb/src/**/*" in row
            for row in check_wrapper_upstream_inputs(broken, bare, floor=wrap_floor)
        ):
            failures.append("A7 did not fire on a wrapper task missing an upstream's sources")

        # A7-b: the manifest half, which lives in the OTHER bucket. A one-bucket A7 passes this.
        broken = json.loads(json.dumps(wrap))
        broken["k-ts"]["task_inputs"]["build"] = ["rs/crates/bindings/nb/Cargo.toml"]
        if not any(
            "rs/crates/libs/kern/Cargo.toml" in row
            for row in check_wrapper_upstream_inputs(broken, bare, floor=wrap_floor)
        ):
            failures.append("A7 did not fire on a wrapper task missing an upstream's Cargo.toml")

        # A7-c: Rust projects belong to A6, never A7. Double-covering them would make A6's strict
        # equality and A7's containment disagree on the same task.
        #
        # The flipped project is UNDER-DECLARED on purpose (SMA-560 I1). With a CLEAN one this row
        # was vacuous: `!= []` cannot tell "filtered out by the language test" from "examined and
        # found satisfied", so deleting `or proj.get("language") == "rust"` from the examined-set
        # filter kept --self-test green (measured). Emptying both glob buckets means an examined
        # k-ts MUST emit rows, so the empty result now proves the filter ran.
        rusty = json.loads(json.dumps(wrap))
        rusty["k-ts"]["language"] = "rust"
        rusty["k-ts"]["task_input_globs"] = {"build": [], "test": []}
        rusty["k-ts"]["task_inputs"] = {"build": [], "test": []}
        if check_wrapper_upstream_inputs(rusty, bare, floor={}) != []:
            failures.append("A7 examined a Rust project, which is A6's job")

        # A7-d: the FLOOR must fire when the closure derivation degrades to empty. Emptying `deps`
        # also empties `want`, so the per-task loop goes quiet by itself — this MUST match the
        # `FLOOR:` prefix or it passes with the whole floor block deleted (A6-e's lesson).
        broken = json.loads(json.dumps(wrap))
        broken["k-ts"]["deps"] = {}
        if not any(
            row.startswith("FLOOR:") and "k-ts's dependsOn closure no longer derives kern-rs" in row
            for row in check_wrapper_upstream_inputs(broken, bare, floor=wrap_floor)
        ):
            failures.append("A7 floor did not fire on a neutered closure derivation")

        # A7-e: a floor entry naming a project that is not examined at all is a FLOOR violation,
        # never a silent skip — the wrapper's task could have stopped matching an FFI marker.
        # The assertion names THIS branch's own message, never the bare `FLOOR:` prefix: with only
        # the prefix asserted, deleting the branch lets `k-ts` fall through to the
        # closure-derivation branch, which emits a `FLOOR:` row of its own and keeps this control
        # green with the very code it names removed (SMA-542, measured).
        broken = json.loads(json.dumps(wrap))
        broken["k-ts"]["invocations"] = {"build": "echo nothing", "test": "echo nothing"}
        if not any(
            row.startswith("FLOOR:") and "k-ts has no task matched by an FFI marker" in row
            for row in check_wrapper_upstream_inputs(broken, bare, floor=wrap_floor)
        ):
            failures.append("A7 floor did not fire when a wrapper stopped matching any FFI marker")

        # A7-f: a floor entry naming an absent project is a FLOOR violation.
        # Same discrimination as A7-e, in the other direction: an absent project deleted from the
        # `pid not in projects` branch falls straight through to the `pid not in examined` one.
        if not any(
            row.startswith("FLOOR:") and "ghost-ts is not in the graph at all" in row
            for row in check_wrapper_upstream_inputs(wrap, bare, floor={"ghost-ts": {"kern-rs"}})
        ):
            failures.append("A7 floor did not fire on a floor entry naming an absent project")

        # A7-g: the read is PER (project, task) — SMA-560 I2. `test` loses an upstream glob that
        # `build` still declares, so an A7 that unioned a wrapper's tasks would see a complete set
        # and report nothing. The row must NAME `k-ts:test`: asserting only that some row mentions
        # the glob would also pass if the union leaked it out under `k-ts:build`.
        broken = json.loads(json.dumps(wrap))
        broken["k-ts"]["task_input_globs"]["test"] = ["rs/crates/bindings/nb/src/**/*"]
        rows = check_wrapper_upstream_inputs(broken, bare, floor=wrap_floor)
        if not any("k-ts:test inputs omit rs/crates/libs/kern/src/**/*" == row for row in rows):
            failures.append(
                "A7 did not report a per-task under-declaration against the task that carries it "
                "— it may be unioning inputs across the wrapper's tasks"
            )
        if any(row.startswith("k-ts:build inputs omit") for row in rows):
            failures.append("A7 blamed `build` for a shortfall that lives on `test`")

    # A7-h: the build.rs half, which is A7's headline assertion and was previously unreachable
    # from --self-test at all — `root` defaulted to None, so no self-test row could exercise it
    # (SMA-560 I3). A real file on disk under a fixture crate's source_dir is what turns the
    # `is_file()` branch on, so this row needs its own tree.
    with tempfile.TemporaryDirectory() as tmp:
        rooted = Path(tmp)
        (rooted / "rs" / "crates" / "bindings" / "nb").mkdir(parents=True)
        (rooted / "rs" / "crates" / "bindings" / "nb" / "build.rs").write_text("fn main() {}\n")
        rows = check_wrapper_upstream_inputs(wrap, rooted, floor=wrap_floor)
        if not any("inputs omit rs/crates/bindings/nb/build.rs" in row for row in rows):
            failures.append(
                "A7 did not demand an upstream's build.rs that exists on disk — the `root` half "
                "of the check is not asserting anything"
            )
        # ...and it must be demanded of EVERY examined task, not just the first one.
        for task in ("build", "test"):
            if not any(
                f"k-ts:{task} inputs omit rs/crates/bindings/nb/build.rs" == row for row in rows
            ):
                failures.append(f"A7 did not demand nb's build.rs of k-ts:{task}")
        # A crate with NO build.rs on disk must not be demanded one — otherwise every wrapper
        # gains an unsatisfiable row the day this branch is written wrong.
        if any("rs/crates/libs/kern/build.rs" in row for row in rows):
            failures.append("A7 demanded a build.rs for an upstream that has none on disk")

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
    if check_task_inputs(ok, crates, "lint", WORKSPACE_LINT_INPUTS):
        failures.append("A4 reported violations on the clean fixture")

    # Fires when a required file is missing from the declared inputs.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["task_inputs"]["lint"] = [
        f for f in WORKSPACE_LINT_INPUTS if f != "rs/rust-toolchain.toml"
    ]
    rows = check_task_inputs(broken, crates, "lint", WORKSPACE_LINT_INPUTS)
    if not rows:
        failures.append("A4 did not fire on a missing workspace lint input")
    elif not any("rs/rust-toolchain.toml" in row for row in rows):
        failures.append("A4 fired but did not name the missing file")

    # Fires for a crate with NO in-tree deps. A3 is guarded by `if want:` and never reaches such a
    # crate; A4 must not copy that shape, or four of the thirteen real crates go unasserted while
    # the negative control stays green.
    broken = json.loads(json.dumps(ok))
    broken["b-rs"]["task_inputs"]["lint"] = []
    if not any(
        row.startswith("b-rs")
        for row in check_task_inputs(broken, crates, "lint", WORKSPACE_LINT_INPUTS)
    ):
        failures.append("A4 did not fire for a dep-free crate (it inherited A3's `if want:` guard)")

    # An ABSENT lint task is a different defect from a lint task with incomplete inputs.
    broken = json.loads(json.dumps(ok))
    del broken["a-rs"]["task_inputs"]["lint"]
    if not any(
        "has no `lint` task" in row
        for row in check_task_inputs(broken, crates, "lint", WORKSPACE_LINT_INPUTS)
    ):
        failures.append("A4 did not distinguish an absent lint task from incomplete inputs")

    # Moon emitting no `inputFiles` for the task must FIRE, never silently skip: a skip would turn a
    # moon-version change into a vacuous pass, which is the failure mode this whole gate exists for.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["task_inputs"]["lint"] = None
    if not any(
        "inputFiles" in row
        for row in check_task_inputs(broken, crates, "lint", WORKSPACE_LINT_INPUTS)
    ):
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
    broken["paigasus-kernel-ts"]["task_inputs"]["build"] = list(WORKSPACE_LINT_INPUTS)
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

    # A6 (SMA-528): every crate's build/test/lint must key on its TRANSITIVE upstreams' sources.
    # Independent of A1-A5 again — and uniquely unprotected elsewhere, because a crate's own
    # moon.yml is not an input to its own tasks, so a wrong `fileGroups.upstreams` reds nothing.
    #
    # EVERY control below matches a row KIND, never mere non-emptiness. A6 emits five distinct
    # kinds and most mutations trip several at once, so a bare `if not check_upstream_inputs(...)`
    # passes on a row from a DIFFERENT sub-assertion than the one it claims to prove. Measured, not
    # theorised: with the floor's derivation rows deleted, the first draft of these controls still
    # printed `all six assertions fire` and exited 0 — emptying `deps` also empties `want`, so the
    # per-crate loop supplied six `inputs include ...` rows and the floor control read them as its
    # own. Same discipline as A5's `unrelated-ts:build` case.
    #
    # A6 clean baseline — and it is a REAL control, not a smoke test. A6-a/A6-b below prove a row
    # appears when the DECLARATION lacks an entry; only this proves no row appears when it has one.
    # Neither alone pins a bucket: drop the `inputFiles` half of `resolved` entirely and A6-b still
    # passes (its Cargo.toml row shows up for the wrong reason) while THIS reds, because the clean
    # fixture declares that manifest in `inputFiles`. The pair is what holds both buckets wired.
    rows = check_upstream_inputs(ok, allow={}, floor={})
    if rows:
        failures.append(f"clean fixture reported A6 violations: {rows}")

    # A6-a: the GLOB half missing. Must name the upstream SRC entry on the task that lost it.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["task_input_globs"]["test"] = []
    if "a-rs:test inputs omit rs/crates/libs/b/src/**/*" not in check_upstream_inputs(
        broken, allow={}, floor={}
    ):
        failures.append("A6 did not fire on a missing upstream src glob")

    # A6-b: the MANIFEST half missing. This is the half an inputGlobs-only A6 could never see —
    # the exact defect the pre-review draft of the SMA-528 spec shipped. Naming the entry pins
    # WHICH row fired, so the control cannot be satisfied by an unrelated one. Note what it still
    # cannot do alone: dropping the `inputFiles` bucket from `resolved` produces this same row, so
    # A6-b passes and it is the CLEAN BASELINE above that reds. Do not weaken either one.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["task_inputs"]["build"] = []
    if "a-rs:build inputs omit rs/crates/libs/b/Cargo.toml" not in check_upstream_inputs(
        broken, allow={}, floor={}
    ):
        failures.append("A6 did not fire on a missing upstream Cargo.toml")

    # A6-c: over-approximation with no allowlist entry — the direction a subset check cannot see.
    ghost = "rs/crates/libs/ghost/src/**/*"
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["task_input_globs"]["lint"].append(ghost)
    if f"a-rs:lint inputs include {ghost}, which is not in its closure" not in (
        check_upstream_inputs(broken, allow={}, floor={})
    ):
        failures.append("A6 did not fire on an upstream outside the closure")

    # A6-c2: a BROAD glob into a crate that IS a real upstream — the hole the `observed` filter
    # was widened to close (SMA-528 CodeRabbit finding). Unlike A6-c's `ghost` entry, `b-rs` IS in
    # `a-rs`'s closure — but a `**/*` glob is still not the `src/**/*`-or-`Cargo.toml` pair `want`
    # expects, so strict equality must still fire on the literal mismatch. Before the widened
    # filter this entry matched neither suffix, so it never entered `observed` and the violation
    # was invisible regardless of what the fixture declared.
    broad = "rs/crates/libs/b/**/*"
    broad_broken = json.loads(json.dumps(ok))
    broad_broken["a-rs"]["task_input_globs"]["lint"].append(broad)
    if f"a-rs:lint inputs include {broad}, which is not in its closure" not in (
        check_upstream_inputs(broad_broken, allow={}, floor={})
    ):
        failures.append("A6 did not fire on a broad glob into a real upstream's tree")

    # A6-d: an allowlisted over-approximation must be accepted, and only WITH a reason. `broken`
    # carries exactly one defect, so a working allowlist empties the list outright.
    if check_upstream_inputs(
        broken, allow={("a-rs", "rs/crates/libs/ghost"): "deliberate"}, floor={}
    ):
        failures.append("A6 rejected an allowlisted over-approximation")
    if not any(
        ghost in row
        for row in check_upstream_inputs(
            broken, allow={("a-rs", "rs/crates/libs/ghost"): ""}, floor={}
        )
    ):
        failures.append("A6 accepted an allowlist entry with an empty reason")

    # A6-e: the FLOOR must fire when the derivation degrades to empty. Emptying `deps` also empties
    # `want`, which makes the per-crate loop emit six `inputs include ...` rows all by itself — so
    # this MUST match the `FLOOR:` prefix, or it passes with the entire floor block deleted.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["deps"] = {}
    if not any(
        row.startswith("FLOOR:")
        for row in check_upstream_inputs(broken, allow={}, floor={"a-rs": {"b-rs"}})
    ):
        failures.append("A6 floor did not fire on a neutered closure derivation")

    # The floor's OTHER half: a crate that stops reporting `language: rust` drops out of the
    # per-crate loop entirely. Its closure still derives, so the derivation half of the floor is
    # blind to it and the whole run goes green with that crate asserted about nothing.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["language"] = "unknown"
    if not any(
        "A6 examines" in row
        for row in check_upstream_inputs(broken, allow={}, floor={"a-rs": {"b-rs"}})
    ):
        failures.append("A6 floor did not fire on a crate that dropped out of the examined set")

    # A6-f: an absent inputGlobs key is an infra-shaped violation, never a silent skip.
    broken = json.loads(json.dumps(ok))
    broken["a-rs"]["task_input_globs"]["build"] = None
    if not any(
        "a-rs:build reported no inputFiles/inputGlobs" in row
        for row in check_upstream_inputs(broken, allow={}, floor={})
    ):
        failures.append("A6 did not fire on an absent inputGlobs key")

    # ...and so is ONE bucket missing the task key while the other has it. Unreachable from
    # moon_projects() today, which writes both keys together. With a STRING sentinel this fell
    # through to `set("absent") | set(globs)`: the single letters are filtered out by the
    # `rs/crates/` prefix test, so the visible symptom is not junk rows but a CONFIDENT WRONG one —
    # `inputs omit .../Cargo.toml`, blaming the declaration for moon's half-report. Both halves are
    # asserted: the honest row present, the misleading row absent.
    broken = json.loads(json.dumps(ok))
    del broken["a-rs"]["task_input_globs"]["build"]
    rows = check_upstream_inputs(broken, allow={}, floor={})
    if not any("a-rs:build reported no inputFiles/inputGlobs" in row for row in rows):
        failures.append("A6 did not fire on a half-reported task (one bucket missing its key)")
    if any(row.startswith("a-rs:build inputs omit") for row in rows):
        failures.append("A6 blamed the declaration for a half-reported task")

    # A CYCLE must not make a crate its own upstream. `rust_closure` excludes the ORIGIN at every
    # depth; comparing against the current node instead would return {b-rs, a-rs} here, and `want`
    # would demand a-rs's own pair that `observed` deliberately excludes — an unsatisfiable row.
    broken = json.loads(json.dumps(ok))
    broken["b-rs"]["deps"] = {"a-rs": "explicit"}
    if any(
        row.startswith("a-rs:") and "rs/crates/libs/a/" in row
        for row in check_upstream_inputs(broken, allow={}, floor={})
    ):
        failures.append("A6 made a crate its own upstream on a dependency cycle")

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
    print("  OK   [parity] all seven assertions fire on synthetic violations")
    return 0


# SMA-560 I4 — the findings list's own floor. `collect_findings` is the ONLY place a check is
# invoked for the real run, so a check dropped from that list is never called; but "never called"
# was, until now, only LOUD for a check whose name appears nowhere else in the list. Three shapes
# were measured passing `--self-test` with a real assertion silently removed: deleting the `a5`
# tuple (the name `check_ffi_inputs` still appears, on the line that pre-computes `a5`), deleting
# either `check_task_inputs` tuple (the name still appears on the other one), and deleting the
# `a1`/`a2`/`a3` tuples (`check` does not even carry the `check_` prefix the name guard scans for).
# A name-based guard structurally cannot separate two call sites of the same function, so this pins
# the LIST: `collect_findings` returns `(key, rows, title)` triples and `self_test` asserts both the
# arity and the exact key sequence, the way ci_targets.py's SELF_SCHEDULED_GATES pins a membership
# rather than a bare count.
#
# Adding a check means adding its key here AND its tuple there, in the same order.
EXPECTED_FINDING_KEYS = ("a1", "a2", "a3", "a4-lint", "a4-fmt", "a5", "a6", "a7")


def collect_findings(projects, crates, root):
    """Every assertion's rows, as `(key, rows, title)`, in report order.

    ONE list, used for BOTH the pass/fail verdict and the report. Before SMA-542 the two were
    written separately, so a new check folded into one and not the other was a green no-op.
    That restructure is necessary but NOT sufficient on its own: what makes the list itself
    hard to shrink is `EXPECTED_FINDING_KEYS` above, asserted by `self_test`.

    Raises the INFRA_ERRORS members its checks raise (`MoonOutputError` from the FFI derivation),
    so `main` keeps them inside its try and maps them to rc 2.
    """
    a1, a2, a3 = check(projects, crates)
    a5 = check_ffi_inputs(projects)
    # SMA-594. Derived, never hand-listed, for the same reason `self_test`'s `complete_inputs` is:
    # these two hints named three files while the checks already demanded four, so a developer who
    # followed the advice verbatim was left with a still-red gate. `/`-prefixed because that is the
    # form the YAML `inputs` take (the checks compare the resolved, slash-free form).
    #
    # Do NOT extend this to the `a4-fmt` hint below. Every member of these two constants is
    # WORKSPACE-relative, which is what makes a blanket `/` prefix right. FMT_TASK_INPUTS is mixed:
    # `rs/rustfmt.toml` is workspace-relative but `Cargo.toml`, `src/**/*` and `tests/**/*` are
    # PROJECT-relative, so the same one-liner would emit `/Cargo.toml` and `/src/**/*` and send the
    # reader to paths that do not exist. That hint stays hand-listed on purpose.
    want_lint_inputs = ", ".join(f"/{f}" for f in WORKSPACE_LINT_INPUTS)
    want_ffi_inputs = ", ".join(f"/{f}" for f in FFI_TASK_INPUTS)
    findings = [
        ("a1", a1,
             "Cargo dep with NO Moon edge (under-builds — CI stays green while skipping work).\n"
             "    Fix: add the upstream to `dependsOn` in the consumer's moon.yml."),
        ("a2", a2,
             "Hand-declared Moon edge with NO Cargo backing (over-builds).\n"
             "    Fix: delete it, or add it to ALLOW_NO_CARGO_BACKING with a reason."),
        ("a3", a3,
             "Moon edge exists but the upstream's build is NOT scheduled — the affected-graph\n"
             "    guard CANNOT see this (SMA-429 F3).\n"
             "    Fix: for `build`/`test`, add '^:build' to the task's `deps` in the consumer's\n"
             "    moon.yml. For `lint` the dep is declared once for ALL crates in\n"
             "    .moon/tasks/rust.yml — restore it there, not per-crate (SMA-526)."),
        ("a4-lint", check_task_inputs(projects, crates, "lint", WORKSPACE_LINT_INPUTS),
             "`lint` does not key on the workspace-level files, so a dependency bump, a\n"
             "    [workspace.lints] edit or a toolchain drift schedules NOTHING for this crate\n"
             "    (SMA-534).\n"
             "    Fix: the inputs are declared once for ALL crates in .moon/tasks/rust.yml —\n"
             "    restore them there, not per-crate.\n"
             f"    Expected: {want_lint_inputs}."),
        ("a4-fmt", check_task_inputs(projects, crates, "fmt", FMT_TASK_INPUTS),
             "`fmt` does not key on everything `cargo fmt --check` actually reads, so a\n"
             "    rustfmt.toml edit, a toolchain bump or a misformatted tests/ file schedules\n"
             "    NOTHING for this crate (SMA-537).\n"
             "    Fix: the inputs are declared once for ALL crates in .moon/tasks/rust.yml —\n"
             "    restore them there, not per-crate. Expected: @group(sources), @group(tests),\n"
             "    /rs/rustfmt.toml, /rs/rust-toolchain.toml."),
        ("a5", a5,
             "An FFI build task does not key on the workspace-level files, so a dependency bump\n"
             "    replays a CACHED artifact built from a different resolution — and clippy cannot\n"
             "    cover it, because it never links a cdylib and never targets wasm32 (SMA-546).\n"
             f"    Fix: add {want_ffi_inputs}\n"
             "    to that task's `inputs`. A `not matched by any FFI marker` row\n"
             "    means the opposite — the task stopped looking like a Rust build to A5; either\n"
             "    restore the invocation or update FFI_MARKERS."),
        ("a6", check_upstream_inputs(projects),
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
        ("a7", check_wrapper_upstream_inputs(projects, root),
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
    ]

    return findings


def main():
    root = Path(__file__).resolve().parents[2]
    try:
        projects = moon_projects()
        crates = cargo_crates(root)
        findings = collect_findings(projects, crates, root)
    except INFRA_ERRORS as exc:
        # Mirror run.sh's infra-vs-assertion split: a broken `moon` — or an unparseable Cargo.toml —
        # must never be mistaken for a graph regression. See INFRA_ERRORS.
        print(f"FATAL [parity] could not build the graphs: {exc}", file=sys.stderr)
        return 2

    if not any(rows for _, rows, _ in findings):
        print(
            f"PASS  {'cargo-moon-parity':<18} -> "
            f"{len(crates)} crates: every Cargo dep has a Moon edge that schedules its build, "
            f"every lint and fmt keys on the files its command reads, every FFI build task does "
            f"too, every crate keys on its upstream sources, and every py/ts wrapper keys on the "
            f"Rust crates it builds"
        )
        return 0

    print("FAIL  [cargo-moon-parity] Cargo and Moon disagree", file=sys.stderr)
    for _, rows, title in findings:
        if rows:
            print(f"  {title}", file=sys.stderr)
            for row in rows:
                print(f"      {row}", file=sys.stderr)
    return 1

if __name__ == "__main__":
    sys.exit(self_test() if "--self-test" in sys.argv[1:] else main())
