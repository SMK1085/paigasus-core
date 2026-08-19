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

    Currently UNRAISED in this file: every violation is returned as a row by check() rather than
    thrown, and every error path raises MoonOutputError. Kept for symmetry with ci_targets.py, which
    does raise it, so the rc-1/rc-2 split reads the same way in both.
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

# Moon injects this onto EVERY task in the graph, across all 28 projects — including a task
# declaring literally `inputs: []` (spec E2; the task count there, 119, predates this file's own
# `repo:input-liveness`, which makes it 120 — deliberately not restated here as a number that would
# just go stale again). So a "resolved input set" is never empty, and the
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

# The floor. check() compares derived sets, and an empty set violates nothing — so a project key
# that stops matching, or a moon output shape change, would print PASS while asserting nothing.
# Same role as cargo_moon_parity.py's REQUIRED_FFI_TASKS and ci_targets.py's REQUIRED_REPO_TASKS.
#
# HONEST SCOPE: this catches a task RENAMED or made `internal: true` while the gate still runs. It
# does NOT catch repo:input-liveness itself vanishing — if that task is gone, nothing executes this
# file at all. SMA-541's C1 is what makes a deleted repo:* task red.
REQUIRED_TASKS = ("affected-smoke", "input-liveness", "promtool", "publish-metadata")

# D13 — this gate's OWN inputs. `inputs: ['**/*']` is load-bearing: the verdict depends on the whole
# tracked tree, because a glob dies when files MOVE, and no narrow input list can observe that.
# Narrowed to e.g. 'ops/**/*' "for cost", this task is still live under I1, still has authored
# inputs under I3, and still passes all of SMA-541's C1-C5 — while silently no longer firing on the
# renames it exists to catch. That is this issue's own failure class, reproduced inside its fix.
# Asserted here AND in ci_targets.py, so a gate is not the sole judge of its own configuration.
SELF_TASK = "input-liveness"
SELF_EXPECTED_GLOBS = ("**/*",)

# (task, pattern) -> why this dead input is tolerated. SHIPS EMPTY, and unlike SMA-541's T_EXEMPT
# there is not even a hypothetical entry: the repo project is measured 100% clean (spec E3).
# `pattern` may name an inputGlobs OR an inputFiles entry — the allowlist covers I1 and I2 alike.
# An entry is a RECORDED DECISION: the reason string is required to be non-blank AFTER STRIPPING
# whitespace — "" and "   " are both an assertion failure, not merely "". Mirrors
# cargo_moon_parity.py's ALLOW_NO_CARGO_BACKING, which uses the same `.strip()` test.
ALLOW_DEAD_INPUT = {}


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
    shape rule — the same split ci_targets.py uses (`moon_payload`/`_eligibility`).
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


def check(tasks, tracked, matcher, allow=ALLOW_DEAD_INPUT):
    """I1-I5. PURE apart from `matcher`, which is injected so fixtures need no tree.

    Returns `(kind, message)` rows. An empty list is a pass.
    """
    rows = []

    # I5 floor, first: if the parsed set is wrong, every row below is about the wrong thing.
    for name in sorted(set(REQUIRED_TASKS) - set(tasks)):
        rows.append((
            "floor",
            f"repo:{name} is REQUIRED to be present but is absent from the parsed task set. Either "
            "it was renamed or removed (update REQUIRED_TASKS), or it was made `internal: true` "
            "(moon omits such tasks from `moon query tasks` entirely), or moon's output shape "
            "changed. Investigate before touching anything else — the checks below may be "
            "comparing empty sets."
        ))

    # I5 / D13 — this gate's own inputs.
    if SELF_TASK in tasks:
        got = tuple(authored(tasks[SELF_TASK][0]))
        if got != SELF_EXPECTED_GLOBS or tasks[SELF_TASK][1]:
            rows.append((
                "self-inputs",
                f"repo:{SELF_TASK}'s authored inputs are {list(got) + tasks[SELF_TASK][1]}, "
                f"expected exactly {list(SELF_EXPECTED_GLOBS)}. This gate's verdict depends on the "
                "WHOLE tracked tree — a glob dies when files move — so a narrower input set makes "
                "it serve a cached PASS on exactly the rename that kills another gate, with "
                "nothing red. Restore `inputs: ['**/*']` in moon.yml (SMA-553 D13)."
            ))

    # I1 / I2 / I4 — per task, per pattern.
    for name in sorted(tasks):
        globs, files = tasks[name]
        for pattern in authored(globs):
            # classify() runs BEFORE the allowlist: ALLOW_DEAD_INPUT is documented as "why this
            # dead INPUT is tolerated", not a third door around classify()'s "fix it or extend the
            # validator deliberately". An allowlisted-but-unevaluable pattern must still be
            # reported rejected — the gate never looked at it, so there is nothing for the
            # allowlist to have exempted.
            verdict = classify(pattern)
            if verdict == "negated":
                continue
            if verdict != "ok":
                rows.append((
                    "rejected",
                    f"repo:{name} declares the glob {pattern!r}, which this gate will not evaluate "
                    f"({verdict}). It is NOT reported as dead — the gate did not look. Either use a "
                    "form the validator accepts, or extend classify() in "
                    "ci/affected-graph/task_inputs.py deliberately (SMA-553 D6)."
                ))
                continue
            # TRUTHINESS after STRIPPING, not membership and not bare truthiness: an entry with a
            # blank OR whitespace-only reason must NOT exempt anything. Bare membership would let
            # `("a", "b"): ""` silence a violation unreviewably, which is the hole
            # cargo_moon_parity.py's _allowlisted helper exists to close — and that helper strips,
            # so `"   "` must be treated the same as `""` here too. The blank/blank-after-strip
            # reason is reported separately below, and the underlying violation still fires.
            if (allow.get((name, pattern)) or "").strip():
                continue
            if matcher(pattern) == 0:
                rows.append((
                    "dead",
                    f"repo:{name}'s input glob {pattern!r} matches no tracked file, so Moon will "
                    "never schedule that task on a change to what it is meant to guard. Usually a "
                    "moved or renamed directory: update the glob in moon.yml. If it is genuinely "
                    "meant to match nothing, add an ALLOW_DEAD_INPUT entry with a reason."
                ))
        for path in files:
            if (allow.get((name, path)) or "").strip():  # truthiness-after-strip, per the note above
                continue
            if path not in tracked:
                rows.append((
                    "not-exact",
                    f"repo:{name}'s input file {path!r} is not tracked by git, so it can never "
                    "invalidate that task. Update the path in moon.yml, or add an ALLOW_DEAD_INPUT "
                    "entry with a reason."
                ))

        # I3 — after the subtraction, not before (spec E2: the resolved set is never empty).
        if not authored(globs) and not files:
            rows.append((
                "no-inputs",
                f"repo:{name} declares no inputs of its own — only Moon's injected "
                f"{INJECTED_GLOB!r}. It would be scheduled solely by a .moon/ config edit, which "
                "means it never runs on a change to its own subject. Give it an `inputs:` list."
            ))

    # D11 — the allowlist's own staleness rules.
    for (name, pattern), reason in sorted(allow.items()):
        if not (reason or "").strip():  # blank OR whitespace-only, matching the loops above
            rows.append((
                "allowlist",
                f"ALLOW_DEAD_INPUT[{(name, pattern)!r}] has no reason string. An exemption is a "
                "recorded decision, so the record is what earns it."
            ))
        if name not in tasks:
            rows.append((
                "allowlist",
                f"ALLOW_DEAD_INPUT names repo:{name}, which is not a repo task — the task it "
                "exempted was renamed or deleted and the exemption outlived it. A typo is loud "
                "(the real pattern shows up as a violation); a leftover is silent, and exempts "
                "nothing forever."
            ))
        # `authored(tasks[name][0])`, NOT the raw glob list: INJECTED_GLOB is present in every
        # task's raw inputGlobs, so comparing against the raw list would treat an entry keyed on
        # INJECTED_GLOB itself as "declared" — it is never iterated in the loops above (which walk
        # authored() output), so such an entry would exempt nothing and never be reported. Same
        # staleness class this rule exists to catch, just via a different door.
        elif pattern not in authored(tasks[name][0]) and pattern not in tasks[name][1]:
            rows.append((
                "allowlist",
                f"ALLOW_DEAD_INPUT exempts {pattern!r} on repo:{name}, which declares no such "
                "input in either inputGlobs or inputFiles. Same staleness class as above."
            ))
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
    def raises_moon(label, projects, match=None):
        # `match` asserts WHICH guard fired, not merely that one did. Without it a fixture aimed at
        # an early guard is satisfied by the D4 composition guard downstream — which raises on any
        # malformed payload — so it cannot detect its own target guard being deleted.
        try:
            _repo_tasks(projects)
        except MoonOutputError as exc:
            if match and match not in str(exc):
                failures.append(f"_repo_tasks: {label} raised the WRONG guard: {exc}")
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
    raises_moon("a non-dict inputGlobs", {"repo": {"promtool": {"inputGlobs": ["x"]}}},
                match="expected objects")
    raises_moon("a non-dict inputFiles", {"repo": {"promtool": {"inputFiles": ["x"]}}},
                match="expected objects")
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

    # --- check (I1-I5) ------------------------------------------------------------------------
    # A minimal well-formed task set. Every fixture below mutates ONE thing away from it, so a row
    # that fires proves the specific rule fired and not a neighbour.
    def task_set(**overrides):
        base = {
            "affected-smoke": (["ci/affected-graph/**/*", INJECTED_GLOB], []),
            "input-liveness": (["**/*", INJECTED_GLOB], []),
            "promtool": (["ops/**/*", INJECTED_GLOB], [".prototools"]),
            "publish-metadata": ([INJECTED_GLOB], ["rs/Cargo.toml"]),
        }
        base.update(overrides)
        return base

    tracked = {".prototools", "rs/Cargo.toml"}
    live = {"ci/affected-graph/**/*", "**/*", "ops/**/*"}
    matcher = lambda p: 1 if p in live else 0

    def kinds(tasks, tracked=tracked, matcher=matcher, allow=None):
        return sorted({k for k, _ in check(tasks, tracked, matcher, allow or {})})

    if kinds(task_set()) != []:
        failures.append(f"check: fired on a clean task set: {check(task_set(), tracked, matcher, {})}")

    # I1 — a glob matching nothing.
    if kinds(task_set(promtool=(["ops-moved/**/*", INJECTED_GLOB], [".prototools"]))) != ["dead"]:
        failures.append("check: I1 missed a dead glob")
    # I2 — a file input that is not tracked.
    if kinds(task_set(promtool=(["ops/**/*", INJECTED_GLOB], ["gone.toml"]))) != ["not-exact"]:
        failures.append("check: I2 missed an untracked file input")
    # I2 — EXACT membership, not a prefix match. `git ls-files -- rs` returns 330 files (spec E14),
    # so an implementation that asked git instead of the tracked SET would pass for any directory.
    if kinds(task_set(promtool=(["ops/**/*", INJECTED_GLOB], ["rs"]))) != ["not-exact"]:
        failures.append("check: I2 accepted a directory path as a tracked file")
    # I3 — nothing but the injected glob.
    if kinds(task_set(promtool=([INJECTED_GLOB], []))) != ["no-inputs"]:
        failures.append("check: I3 missed a task with no authored inputs")
    # I4 — an unevaluable pattern. NOT reported as dead: the gate did not evaluate it at all.
    if kinds(task_set(promtool=(["ops/**/*.{a,b}", INJECTED_GLOB], [".prototools"]))) != ["rejected"]:
        failures.append("check: I4 missed a brace glob")
    # A negated glob is SKIPPED, not failed.
    if kinds(task_set(promtool=(["ops/**/*", "!ops/scratch/**", INJECTED_GLOB], [".prototools"]))) != []:
        failures.append("check: a negated glob must be skipped, not reported")
    # I5 floor.
    missing_floor = task_set()
    del missing_floor["promtool"]
    if "floor" not in kinds(missing_floor):
        failures.append("check: I5 missed an absent REQUIRED_TASKS member")
    # D13 — this gate's own inputs narrowed.
    if kinds(task_set(**{SELF_TASK: (["ops/**/*", INJECTED_GLOB], [])})) != ["self-inputs"]:
        failures.append("check: D13 missed input-liveness narrowed away from '**/*'")
    # ...and widened with an extra glob, which is equally a change to a load-bearing input set.
    if "self-inputs" not in kinds(task_set(**{SELF_TASK: (["**/*", "ops/**/*", INJECTED_GLOB], [])})):
        failures.append("check: D13 missed an extra glob on input-liveness")
    # ...and a file input, which :295's `or tasks[SELF_TASK][1]` clause exists solely to catch —
    # deleting that clause keeps every other fixture in this file green.
    narrowed_by_file = task_set(**{SELF_TASK: (["**/*", INJECTED_GLOB], [".prototools"])})
    if kinds(narrowed_by_file) != ["self-inputs"]:
        failures.append("check: D13 missed a file input on input-liveness")
    # Message text (review round 1, minor 6): kinds() discards messages, so a fixture must read
    # one directly at least once or blanking all seven diagnostics stays invisible.
    self_inputs_rows = check(narrowed_by_file, tracked, matcher, {})
    if not any(k == "self-inputs" and "SMA-553 D13" in m for k, m in self_inputs_rows):
        failures.append("check: the self-inputs message must reference SMA-553 D13")

    # Allowlist (D11).
    dead_globs = task_set(promtool=(["ops-moved/**/*", INJECTED_GLOB], [".prototools"]))
    if kinds(dead_globs, allow={("promtool", "ops-moved/**/*"): "reason"}) != []:
        failures.append("check: an allowlisted dead glob still fired")
    dead_file = task_set(promtool=(["ops/**/*", INJECTED_GLOB], ["gone.toml"]))
    if kinds(dead_file, allow={("promtool", "gone.toml"): "reason"}) != []:
        failures.append("check: the allowlist does not cover inputFiles")
    if kinds(dead_globs, allow={("promtool", "ops-moved/**/*"): ""}) != ["allowlist", "dead"]:
        failures.append("check: an allowlist entry with a blank reason must itself be a violation")
    # Review round 1, minor 4: ALLOW_DEAD_INPUT is documented as exempting LIVENESS only — it must
    # not double as a third way to silence an unevaluable pattern (fix it, or extend classify()
    # deliberately). Allowlisting a brace glob must not swallow its "rejected" row.
    rejected_glob = task_set(promtool=(["ops/**/*.{a,b}", INJECTED_GLOB], [".prototools"]))
    if kinds(rejected_glob, allow={("promtool", "ops/**/*.{a,b}"): "reason"}) != ["rejected"]:
        failures.append("check: allowlisting a rejected glob must not suppress the rejected row")
    # Review round 1, important 1: a WHITESPACE-ONLY reason is blank after stripping and must be
    # treated identically to "" — strictly worse than the "" case above, since a naive `not reason`
    # check finds "   " truthy and suppresses BOTH the allowlist row and the underlying violation.
    if kinds(dead_globs, allow={("promtool", "ops-moved/**/*"): "   "}) != ["allowlist", "dead"]:
        failures.append("check: a whitespace-only reason must not suppress the underlying violation")
    # Review round 1, important 2: the files loop's truthiness-after-strip is unexercised by the
    # glob-loop fixture above. An implementation that wrote `in allow` here instead of `.get()`
    # would ship green without this.
    if kinds(dead_file, allow={("promtool", "gone.toml"): ""}) != ["allowlist", "not-exact"]:
        failures.append("check: a blank-reason allowlist entry on inputFiles must not suppress not-exact")
    if "allowlist" not in kinds(task_set(), allow={("ghost", "x/**"): "reason"}):
        failures.append("check: an allowlist entry naming no repo task must fire")
    if "allowlist" not in kinds(task_set(), allow={("promtool", "never-declared/**"): "reason"}):
        failures.append("check: an allowlist entry naming an undeclared pattern must fire")
    # Review round 1, minor 5: INJECTED_GLOB is present in every task's RAW inputGlobs but is never
    # iterated by the per-pattern loop above (which walks authored() output) — so an entry keyed on
    # it is not really "declared" and must still be reported stale, not treated as legitimate.
    if "allowlist" not in kinds(task_set(), allow={("promtool", INJECTED_GLOB): "reason"}):
        failures.append("check: an allowlist entry keyed on the injected glob must still be stale")

    if failures:
        print("task-inputs self-test FAILED:", file=sys.stderr)
        for f in failures:
            print(f"  {f}", file=sys.stderr)
        return 1
    print("task-inputs self-test OK")
    return 0


def main():
    root = Path(__file__).resolve().parents[2]
    try:
        tasks = moon_tasks()
        tracked = tracked_files(root)
        matcher = git_matcher(root)
        # D7 — BEFORE the checks. A stuck matcher makes every verdict below meaningless, so
        # reporting "3 dead globs" from one would be actively misleading.
        canaries = check_canaries(matcher)
        rows = check(tasks, tracked, matcher)
    except GateAssertionError as exc:
        print(f"FAIL  [task-inputs] {exc}", file=sys.stderr)
        return 1
    except INFRA_ERRORS as exc:
        print(f"FATAL [task-inputs] could not read the inputs: {exc}", file=sys.stderr)
        return 2

    if canaries:
        print("FATAL [task-inputs] the liveness matcher is not working", file=sys.stderr)
        for row in canaries:
            print(f"    {row}", file=sys.stderr)
        return 2

    if not rows:
        print(
            f"PASS  {'task-inputs':<18} -> {len(tasks)} repo tasks: every declared input still "
            f"matches a tracked file ({len(tracked)} tracked)"
        )
        return 0

    print("FAIL  [task-inputs] a repo:* task declares an input that matches nothing", file=sys.stderr)
    for _, message in rows:
        print(f"  - {message}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(self_test() if "--self-test" in sys.argv[1:] else main())
