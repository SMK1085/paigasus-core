# SPDX-License-Identifier: Apache-2.0
# SMA-553 — repo:* task input-liveness gate.
#
# SMA-541 proves a repo:* gate is WIRED into CI. This is the layer below: a gate that is wired,
# resolvable, and stops firing on the changes it exists to catch, because its `inputs` no longer
# match anything. Moon cannot tell you this itself — `moon query` reports declared inputs VERBATIM,
# unresolved (spec E1), so a glob pointing at a deleted directory reads back exactly as it was
# written. This gate matches every declared pattern against git's tracked set instead.
#
# usage: task_inputs.py [--self-test]
import json
import re
import subprocess
import sys
from pathlib import Path


class GateAssertionError(RuntimeError):
    """An AUTHORIAL mistake -> rc 1, with a message naming what to edit.

    Kept distinct from MoonOutputError so a dead glob (someone moved a directory) can never be
    reported as "re-run the job", which is how a reader triages rc 2 (SMA-541 D2).
    """


class MoonOutputError(RuntimeError):
    """Moon's or git's output did not have the shape this gate requires -> rc 2.

    Raised, never returned as a violation row. A moon upgrade that reshapes the task object must
    fail LOUDLY rather than quietly stop asserting — the drift class this gate exists to close.
    """


INFRA_ERRORS = (
    subprocess.CalledProcessError,
    json.JSONDecodeError,
    OSError,
    MoonOutputError,
)

# Moon injects this onto EVERY task in the graph — all 119 across all 28 projects, including a task
# declaring literally `inputs: []` (spec E2). So a "resolved input set" is never empty, and the
# empty-inputs check (I3) asserts nothing until this is subtracted. D4.
INJECTED_GLOB = ".moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}"

# The conservative charset a pattern must fall inside before it is handed to git. Doubles as the
# pathspec-injection guard: a pattern starting with ':' would be read by git as pathspec magic, and
# the `--` separator plus quoting is necessary but NOT sufficient.
SAFE_CHARS_RE = re.compile(r"[A-Za-z0-9._/*-]+")

# D7 — live-fire canaries, run on EVERY real invocation, not only under --self-test. This is the
# one failure the fixture table cannot catch: a matcher stuck returning "live" passes I1, I2 and I4
# vacuously while every check still prints PASS. Following ci/actionlint/run.sh:1449-1459 ("the
# self-tests, invoked for real"), which calls its fixture tables unconditionally so they are not
# dead code in CI. Costs one extra `git` call each.
CANARY_DEAD = "zz-no-such-directory-sma553/**/*"
CANARY_LIVE = "ci/affected-graph/*.py"


def classify(pattern):
    """One pattern's syntactic verdict. PURE — no filesystem, no subprocess.

    Deliberately separate from liveness: this answers "will this gate evaluate the pattern at all",
    check() answers "does it match anything". Splitting them is what lets --self-test drive the
    whole vocabulary without a tree, and it is why a `rejected-*` verdict is a FAILURE rather than
    a skip — a skip is the silent hole this whole gate exists to close.

    The vocabulary is pattern_verdict's (ci/actionlint/run.sh:919-973), deliberately. This is a
    REIMPLEMENTATION, not a reuse: that one is bash, and it answers a different question — whether
    a pattern is legal under GITHUB ACTIONS filter semantics. Keeping the token names identical
    means a reader moving between the two files is not learning a second vocabulary.
    """
    # An exclusion must not be required to match anything; requiring it would be simply wrong.
    # pattern_verdict:928 has this verdict for the same reason. Zero negated globs exist in the
    # graph today, which is exactly why omitting this would have been invisible.
    if pattern.startswith("!"):
        return "negated"
    # git pathspec has NO brace expansion — measured: `:(glob).moon/*.{yml,...}` matches 0 files
    # (spec E4). Expanding braces here would be hand-rolled parsing, "exactly the kind of thing
    # that silently does the wrong thing" (ci/actionlint/run.sh:263-266, about hand-rolled YAML).
    if "{" in pattern or "}" in pattern:
        return "rejected-braces"
    # git and wax equivalence for these is UNMEASURED (spec E4 covers none of them). Unlike
    # pattern_verdict, which rejects them because GitHub's semantics differ, this gate rejects them
    # because nobody has checked. Measuring them is the sanctioned way to lift the restriction.
    if any(ch in pattern for ch in "?+[]"):
        return "rejected-charclass"
    if not SAFE_CHARS_RE.fullmatch(pattern):
        return "rejected-charset"
    segments = pattern.split("/")
    # git normalises './x', 'a/../b' and 'a//b' away when resolving a pathspec. Whether Moon does
    # is unmeasured, so the gate refuses to guess rather than risk disagreeing with it.
    if any(seg in ("", ".", "..") for seg in segments):
        return "rejected-dotty"
    # '**' must be a whole path component; git only honours it as one, and 'a**b' would otherwise
    # be silently downgraded to a single '*'.
    if any("**" in seg and seg != "**" for seg in segments):
        return "rejected-globstar"
    return "ok"


def authored(globs):
    """The globs a human wrote, i.e. everything but Moon's injected one (D4).

    MUST run before classify(): the injected glob contains braces, so classifying first would give
    every repo task a rejected-braces violation.
    """
    return [g for g in globs if g != INJECTED_GLOB]


def _repo_tasks(projects):
    """moon's parsed `{pid: {task: {...}}}` -> `{task: (inputGlobs, inputFiles)}` for `repo`.

    A PURE function, split out of moon_tasks() so --self-test can drive every MoonOutputError
    without a subprocess. The rc-2 paths are the ones a fixture table most needs: they are what a
    moon upgrade trips, and an unexercised raise is indistinguishable from an absent one.

    Keyed by EXACT project id rather than `moon query tasks --project repo`: moon's query filters
    are unanchored regexes — measured, `--id epo` returns `repo` and `--id paigasus-kernel` returns
    four projects (spec E7) — so a future project named e.g. `paigasus-repo-ts` would silently join
    this set.
    """
    if not isinstance(projects, dict):
        raise MoonOutputError(
            f"`moon query tasks` reported `tasks` as {type(projects).__name__}, expected an object"
        )
    repo = projects.get("repo")
    if not isinstance(repo, dict) or not repo:
        raise MoonOutputError(
            "`moon query tasks` reported no tasks for the `repo` project. Either moon's output "
            "shape changed or the root moon.yml lost its tasks — either way this gate would "
            "compare empty sets and assert nothing."
        )
    rows = {}
    for name, task in repo.items():
        if not isinstance(task, dict):
            raise MoonOutputError(
                f"`moon query tasks` reported task repo:{name} as {type(task).__name__}, "
                "expected an object"
            )
        globs, files = task.get("inputGlobs") or {}, task.get("inputFiles") or {}
        # An ABSENT key is fine (spec E8 — five repo tasks declare globs only). A key present with
        # the WRONG TYPE is a shape change and must be loud.
        if not isinstance(globs, dict) or not isinstance(files, dict):
            raise MoonOutputError(
                f"`moon query tasks` reported repo:{name}'s inputGlobs/inputFiles as "
                f"{type(globs).__name__}/{type(files).__name__}, expected objects"
            )
        rows[name] = (sorted(globs), sorted(files))

    # D4 — the composition guard. Presence alone is the weaker half: it catches the injected glob
    # disappearing or being renamed, but NOT a second member appearing, which would leave every
    # task with zero authored inputs and I3 passing vacuously forever.
    common = None
    for globs, files in rows.values():
        combined = set(globs) | set(files)
        common = combined if common is None else (common & combined)
    if common != {INJECTED_GLOB}:
        raise MoonOutputError(
            f"the inputs common to every `repo` task are {sorted(common)}, expected exactly "
            f"[{INJECTED_GLOB!r}]. Moon's injected input set has changed shape, so subtracting it "
            "to find the AUTHORED inputs no longer means what this gate assumes (SMA-553 D4). "
            "Check .moon/tasks.yml's implicitInputs and moon's release notes before adjusting "
            "INJECTED_GLOB."
        )
    return rows


def moon_tasks():
    """Moon's own resolved task graph, for the `repo` project only.

    ONE subprocess call. The subprocess + json.loads shell around _repo_tasks(), which holds every
    shape rule — the same split ci_targets.py uses (`moon_tasks`/`_eligibility`).
    """
    out = subprocess.run(
        ["moon", "query", "tasks"], capture_output=True, text=True, check=True
    ).stdout
    payload = json.loads(out)
    if not isinstance(payload, dict):
        raise MoonOutputError(
            f"`moon query tasks` returned {type(payload).__name__}, expected a JSON object"
        )
    return _repo_tasks(payload.get("tasks") or {})


def _git(args, root):
    """One git invocation, with the two settings that make its output trustworthy.

    `cwd=root` is load-bearing: `ls-files` only lists paths BELOW its working directory, so running
    this script from inside ci/affected-graph/ would make every pattern in the repo read `dead`.
    `core.quotePath=false` keeps a non-ASCII path from being returned C-quoted, which would miss an
    exact match and report a false `not-exact`.

    A non-zero rc is rc 2 (infrastructure), never "no matches" and never a skip. Note this fires
    only when git is genuinely broken: a MALFORMED pattern exits 0 with no output (measured), which
    reads as `dead` — a false red, the safe direction. classify() is the real defense there.
    """
    proc = subprocess.run(
        ["git", "-c", "core.quotePath=false", *args],
        cwd=root, capture_output=True, text=True,
    )
    if proc.returncode != 0:
        raise MoonOutputError(
            f"`git {' '.join(args)}` failed with rc {proc.returncode}: {proc.stderr.strip()}"
        )
    return [line for line in proc.stdout.splitlines() if line]


def tracked_files(root):
    """Every git-tracked path, as an exact-membership set.

    TRACKED rather than on-disk, deliberately. Moon's input collection does not honour .gitignore —
    that is the entire reason .moon/workspace.yml carries `hasher.ignorePatterns`, which records
    that removing it makes repo:actionlint ~8x slower "because the walk descends into pnpm's
    symlinked content-addressable store". A path under an ignored tree is therefore collected but
    never HASHED: it contributes nothing to any cache key, so it can never invalidate the task.
    git's tracked set is the cheapest available proxy for "can this path ever schedule this task".
    """
    files = set(_git(["ls-files"], root))
    if not files:
        raise MoonOutputError(
            "`git ls-files` reported no tracked files at all — this gate would call every declared "
            "input dead. Check that it is running inside the repository."
        )
    return files


def git_matcher(root):
    """pattern -> number of tracked files it matches."""
    return lambda pattern: len(_git(["ls-files", "--", f":(glob){pattern}"], root))


def check_canaries(matcher):
    """D7. Rows describing a matcher that is not actually discriminating."""
    rows = []
    if matcher(CANARY_DEAD) != 0:
        rows.append(
            f"the dead canary {CANARY_DEAD!r} reported matches — the matcher is not "
            "discriminating, so every liveness verdict below is meaningless"
        )
    if matcher(CANARY_LIVE) == 0:
        rows.append(
            f"the live canary {CANARY_LIVE!r} reported no matches — the matcher cannot see the "
            "tree, so every input would be reported dead"
        )
    return rows


def self_test():
    """Negative control: every assertion must FIRE on a synthetic violation.

    Drives the PURE functions, so no verdict depends on the tree happening to be aligned.
    """
    failures = []

    # --- classify (D6) ------------------------------------------------------------------------
    # Each row is (pattern, expected verdict). Ordering inside classify is load-bearing and these
    # rows pin it: a brace pattern is ALSO outside SAFE_CHARS_RE, and '?' is too, so a
    # reordered implementation would report the generic charset message for both and tell the
    # author nothing actionable.
    for pattern, want in (
        ("ops/observability/prometheus/**/*", "ok"),
        ("rs/**/Cargo.toml", "ok"),
        ("**/*", "ok"),
        ("moon.yml", "ok"),
        ("!ops/scratch/**", "negated"),
        (INJECTED_GLOB, "rejected-braces"),
        ("ts/**/*.{ts,tsx}", "rejected-braces"),
        ("rs/**/*.rs?", "rejected-charclass"),
        ("rs/[abc]/**", "rejected-charclass"),
        ("rs/**/*.jsx+", "rejected-charclass"),
        ("ops/$HOME/**", "rejected-charset"),
        (":(glob)ops/**", "rejected-charset"),
        ("", "rejected-charset"),
        ("./ops/**/*", "rejected-dotty"),
        ("ops/../ops/**/*", "rejected-dotty"),
        ("ops//nats/**", "rejected-dotty"),
        ("rs/a**b/*", "rejected-globstar"),
        ("rs/**x/*", "rejected-globstar"),
    ):
        got = classify(pattern)
        if got != want:
            failures.append(f"classify({pattern!r}) -> {got!r}, expected {want!r}")

    # --- _repo_tasks shape rules and the D4 composition guard (rc 2) --------------------------
    def raises_moon(label, projects):
        try:
            _repo_tasks(projects)
        except MoonOutputError:
            return
        failures.append(f"_repo_tasks: no MoonOutputError for {label}")

    good = {
        "repo": {
            "promtool": {
                "inputGlobs": {"ops/observability/prometheus/**/*": {}, INJECTED_GLOB: {}},
                "inputFiles": {".prototools": {}},
            },
            "actionlint": {"inputGlobs": {"**/*": {}, INJECTED_GLOB: {}}},
        }
    }
    rows = _repo_tasks(good)
    if sorted(rows) != ["actionlint", "promtool"]:
        failures.append(f"_repo_tasks: parsed {sorted(rows)}, expected both repo tasks")
    if rows["actionlint"] != (["**/*", INJECTED_GLOB], []):
        failures.append(f"_repo_tasks: actionlint row is {rows['actionlint']!r}")
    # An ABSENT inputFiles key is legitimate, not a violation (spec E8). Five repo tasks declare
    # globs only; A4's "absent key is a violation" rule in cargo_moon_parity.py does NOT transfer,
    # and copying it verbatim would red five clean gates on day one.
    if rows["actionlint"][1] != []:
        failures.append("_repo_tasks: an absent inputFiles key must parse as empty, not raise")

    raises_moon("a non-dict payload", [])
    raises_moon("no repo project", {"ts": {"lint": {"inputGlobs": {INJECTED_GLOB: {}}}}})
    raises_moon("an empty repo project", {"repo": {}})
    raises_moon("a non-dict task", {"repo": {"promtool": "nope"}})
    raises_moon("a non-dict inputGlobs", {"repo": {"promtool": {"inputGlobs": []}}})
    # D4: the guard is on COMPOSITION, not presence. A second shared input means "authored" no
    # longer means what this gate thinks it means — and if that second member were LIVE, every task
    # would satisfy I3 with zero real inputs while a presence check still passed. That is a false
    # green, the one outcome worse than nothing. .moon/tasks.yml already carries a seven-entry
    # implicitInputs block (spec E13) that is one Moon-behaviour change away from doing this.
    raises_moon("a second shared input", {
        "repo": {
            "a": {"inputGlobs": {"x/**": {}, INJECTED_GLOB: {}, ".moon/**/*": {}}},
            "b": {"inputGlobs": {"y/**": {}, INJECTED_GLOB: {}, ".moon/**/*": {}}},
        }
    })
    raises_moon("the injected glob missing from one task", {
        "repo": {
            "a": {"inputGlobs": {"x/**": {}, INJECTED_GLOB: {}}},
            "b": {"inputGlobs": {"y/**": {}}},
        }
    })

    if authored(["**/*", INJECTED_GLOB]) != ["**/*"]:
        failures.append("authored: did not subtract the injected glob")
    if authored([INJECTED_GLOB]) != []:
        failures.append("authored: a task with only the injected glob must have no authored inputs")

    # --- canaries (D7) ------------------------------------------------------------------------
    if check_canaries(lambda p: 0 if p == CANARY_DEAD else 3):
        failures.append("check_canaries: fired on a healthy matcher")
    # The failure this exists for: a matcher that says everything is live. Every other check passes
    # vacuously under it.
    if not check_canaries(lambda p: 3):
        failures.append("check_canaries: missed a matcher stuck returning live")
    if not check_canaries(lambda p: 0):
        failures.append("check_canaries: missed a matcher stuck returning dead")

    if failures:
        print("task-inputs self-test FAILED:", file=sys.stderr)
        for f in failures:
            print(f"  {f}", file=sys.stderr)
        return 1
    print("task-inputs self-test OK")
    return 0


def main():
    raise NotImplementedError


if __name__ == "__main__":
    sys.exit(self_test() if "--self-test" in sys.argv[1:] else main())
