# SPDX-License-Identifier: Apache-2.0
# SMA-541 — CI target-array coverage gate.
#
# `.github/workflows/ci.yml` runs `moon ci` over a HAND-WRITTEN target array. Nothing asserted that
# array was complete, so a new `repo:*` gate could be added to moon.yml, be perfectly correct, pass
# locally via `moon run repo:<name>`, and never run in CI. There was no red check — the gate simply
# did not exist. That is the SMA-525 silent-omission class, one level up.
#
# Measured, and the reason the reverse check (C2) exists: `moon ci` exits **0** on a target that
# resolves to nothing, including the MIXED case where real targets surround one dead entry
# (`moon ci :promtool :bogus-target :actionlint` -> "Resolved targets: 1", rc 0). So a typo'd or
# renamed entry in `T` was a silent no-op on every PR. (`moon run` does exit 1, but the only
# `moon run "${T[@]}"` path is the initial-push fallback nobody exercises.)
#
# Follows ci/affected-graph/cargo_moon_parity.py's conventions: rc 0/1/2, a `--self-test` negative
# control wired into run.sh's `--negative-control` branch, and never parsing moon.yml.
#
# usage: ci_targets.py [--self-test]
import json
import re
import subprocess
import sys
from itertools import zip_longest
from pathlib import Path


class GateAssertionError(RuntimeError):
    """An AUTHORIAL mistake -> rc 1, never rc 2.

    A missing `T=(...)` line, two of them, an absent CLAUDE.md marker: all of these mean someone
    edited a file into a shape this gate cannot read, which is a red with a fix, not a broken tool.
    Routing them to rc 2 would make run.sh `exit 2` the WHOLE affected-graph guard, destroying the
    diagnostics of all eight cascade cases, A1-A5 and assert_include_relations for that run — and
    labelling "you added a second example" as something that triages as "re-run the job" (D2).
    """


class MoonOutputError(RuntimeError):
    """Moon's query output did not have the shape this gate requires -> rc 2.

    Same contract as cargo_moon_parity.py's class of the same name: "moon told us nothing" must
    abort as infrastructure, so a moon upgrade that reshapes the task object fails loudly rather
    than quietly stopping the assertion.
    """


INFRA_ERRORS = (
    subprocess.CalledProcessError,
    json.JSONDecodeError,
    OSError,
    MoonOutputError,
)

# Any `T=` / `T+=` assignment line. Deliberately BROADER than T_ARRAY_RE so that an append
# (`T+=(:new-gate)`) or a second conditional array is REJECTED rather than silently unexamined:
# C1 would still pass while C2 never saw the appended entries.
T_ASSIGN_RE = re.compile(r"^[ \t]*T[ \t]*\+?=", re.MULTILINE)

# The canonical single-line array. `[ \t]*$` and NOT `\s*$`: in Python `\s` matches newlines, so
# `\)\s*$` can consume one and anchor at a later line's end, quietly accepting a multi-line array
# the rest of this parser is not written for.
T_ARRAY_RE = re.compile(r"^[ \t]*T=\((.*?)\)[ \t]*$", re.MULTILINE)

# The docs command is delimited EXPLICITLY, not recognised by prose shape. Prose-shape matching was
# fragile in both directions against ordinary doc edits: converting the command to a fenced code
# block zero-matches it, and CLAUDE.md already carries two neighbouring `moon ci …` spans that a
# reword could turn into a second match (D7). Markers also make the contract visible to whoever
# edits the file, and keep the illustrative gate list in the same bullet safely outside.
MARKER_BEGIN = "<!-- ci-targets:begin -->"
MARKER_END = "<!-- ci-targets:end -->"

# task name -> why this CI-eligible `repo` task is deliberately absent from ci.yml's `T`.
# SHIPS EMPTY, and that is the point: it is the sanctioned escape, not a live exemption.
#
# It exists because `runInCI: false` — the only exemption C1 would otherwise honour — is documented
# in this repo as BROKEN for this purpose: "Do NOT set `runInCI: false`: Moon also excludes such
# tasks from `moon run` whenever CI=true, which would make the CI gate resolve zero tasks and exit
# 1" (ts/moon.yml:31-32, repeated at :45-46). CI-eligible-but-not-in-`T` tasks already exist one
# project over — `build-release` on all 13 Rust crates, `contracts:generate`, `ts:commitlint`,
# `ts:check-config-only` — so the day a `repo:*` gate needs its own workflow step, the alternative
# to this table is someone deleting the assertion.
#
# An entry is a RECORDED DECISION, not a silent exemption: the reason string is required and a
# blank one is itself an assertion failure, mirroring cargo_moon_parity.py's ALLOW_NO_CARGO_BACKING.
T_EXEMPT = {}

# The floor. C1 compares two derived sets, and two EMPTY sets compare equal — so a project-id
# filter that stops matching, or a moon output shape change, would print PASS while asserting
# nothing. Every task named here must be present and CI-eligible in the parsed `repo` set.
# Same role as cargo_moon_parity.py's REQUIRED_FFI_TASKS.
REQUIRED_REPO_TASKS = ("affected-smoke", "promtool", "publish-metadata")

# C3 checks the flag tail too. The first spec draft omitted it on the stated grounds that
# assert_include_relations "already owns the flag question" — it does not: that function greps
# ci.yml only (run.sh:126) and never opens CLAUDE.md. Without this, the documented command could
# lose --include-relations and silently under-build, which is the very behaviour that makes
# checking the docs worth doing (D6).
REQUIRED_DOC_FLAGS = ("--base origin/main", "--include-relations")

# C4 — this gate's own two call sites in run.sh. Placing the gate inside repo:affected-smoke rather
# than making it a repo:* task of its own (D1) means C1 does NOT cover it: its execution depends on
# these two lines, and deleting either leaves everything green. Matched WITH their bash suffixes
# because the bare name `assert_ci_targets` also appears in the function definition, so a
# name-only match would survive deleting the call.
#
# A PARTIAL mitigation, not a closure: deleting the `assert_ci_targets` call removes C4 along with
# it. SMA-542 is the general fix for this class (spec L6).
RUN_SH_CALL_SITES = (
    "assert_ci_targets || SUITE_RC=1",
    '"$HERE/ci_targets.py" --self-test',
)


def parse_t(text):
    """The `T=(...)` array from ci.yml, as BARE task names (no leading colon).

    Bare names because that is what they are compared against: moon's task-name keys (C1/C2) and
    the doc's tokens (C3). Messages re-add the colon so they name what the reader sees in the file.
    """
    arrays = T_ARRAY_RE.findall(text)
    if len(arrays) != 1:
        raise GateAssertionError(
            f"expected exactly one `T=(...)` line in .github/workflows/ci.yml, found {len(arrays)}. "
            "This gate parses the array with a single-line regex, so it must stay on one line with "
            "nothing after the closing paren (SMA-541 L1)."
        )
    assignments = T_ASSIGN_RE.findall(text)
    if len(assignments) != 1:
        raise GateAssertionError(
            f"found {len(assignments)} `T=`/`T+=` assignment lines in .github/workflows/ci.yml, "
            "expected exactly one. An appended or conditional second array would leave its entries "
            "unexamined by the reverse check while the forward check still passed."
        )
    targets = []
    for token in arrays[0].split():
        if not token.startswith(":"):
            raise GateAssertionError(
                f"`T` entry {token!r} is not a `:name` shorthand target. A project-scoped entry "
                "such as `repo:promtool` would be silently ignored by this gate — the array would "
                "contain something never examined — so it is rejected rather than skipped "
                "(SMA-541 D10). Use the `:name` form, or extend this parser deliberately."
            )
        targets.append(token[1:])
    if not targets:
        raise GateAssertionError(
            "`T=()` is empty — `moon ci` would run nothing at all."
        )
    return targets


def parse_doc_targets(text):
    """CLAUDE.md's documented full-graph command: (bare task names, normalised region text).

    Deliberately ASYMMETRIC with parse_t: a non-`:` token here is ignored, not fatal. The region
    legitimately contains prose punctuation, backticks, `moon`, `ci` and the flag tail, whereas
    every token of `T` is a target and an unrecognised one there means the array holds something
    unexamined (D10).
    """
    begins, ends = text.count(MARKER_BEGIN), text.count(MARKER_END)
    if begins != 1 or ends != 1:
        raise GateAssertionError(
            f"CLAUDE.md must contain exactly one {MARKER_BEGIN} and one {MARKER_END} "
            f"(found {begins} and {ends}). They delimit the documented full-graph command that this "
            "gate compares against ci.yml's `T=(...)` array (SMA-541 D7)."
        )
    start = text.index(MARKER_BEGIN) + len(MARKER_BEGIN)
    end = text.index(MARKER_END)
    if end < start:
        raise GateAssertionError(
            f"CLAUDE.md's markers are inverted — {MARKER_END} appears before {MARKER_BEGIN}."
        )
    region = " ".join(text[start:end].split())
    if not region:
        raise GateAssertionError(
            "CLAUDE.md's ci-targets region is empty — the documented full-graph command is gone."
        )
    targets = []
    for token in region.split():
        token = token.strip("`.,")
        if token.startswith(":"):
            targets.append(token[1:])
    return targets, region


def _eligibility(projects):
    """moon's parsed `{pid: {task: {...}}}` -> `{pid: {task: CI-eligible}}`.

    A PURE function, split out of moon_tasks() so `--self-test` can drive both of its
    MoonOutputError raises without a subprocess. The rc-2 paths are the ones a fixture table most
    needs: they are what a moon upgrade trips, and an unexercised raise is indistinguishable from
    an absent one — which is the drift class this whole gate exists to close.

    Eligibility polarity is deliberately `is not False`: an absent `runInCI`, or an absent
    `options` object, means ELIGIBLE. Defaulting toward inclusion means a moon output change
    cannot silently exempt a gate — it can only over-require, which is a loud red.
    """
    if not projects:
        raise MoonOutputError("`moon query tasks` reported no projects at all")
    saw_options = False
    result = {}
    for pid, tasks in projects.items():
        row = {}
        for name, task in (tasks or {}).items():
            options = task.get("options")
            if options is not None:
                saw_options = True
            row[name] = (options or {}).get("runInCI") is not False
        result[pid] = row
    if not saw_options:
        # Not one task carried `options` — moon's shape changed and runInCI can no longer be read.
        # Escalate rather than treat every task as eligible: a silent shape change is how a gate
        # starts asserting something other than what it claims.
        raise MoonOutputError(
            "no task in `moon query tasks` output carries an `options` key — moon's output shape "
            "changed, so `runInCI` can no longer be read (SMA-541 D8)"
        )
    return result


def moon_tasks():
    """Moon's own resolved task graph: project id -> task name -> CI-eligible.

    ONE subprocess call, filtered by project id in Python rather than with `--project repo`:
    moon's query filters are regex-based and unanchored, so a future project named e.g.
    `paigasus-repo-ts` would silently join the "repo task set" and false-red C1 (D8).

    The subprocess + `json.loads` shell around _eligibility(), which holds the shape rules.
    """
    out = subprocess.run(
        ["moon", "query", "tasks"], capture_output=True, text=True, check=True
    ).stdout
    return _eligibility(json.loads(out).get("tasks") or {})


def check_floor(tasks, floor=REQUIRED_REPO_TASKS):
    """Floor members absent from the parsed CI-eligible `repo` set."""
    repo = tasks.get("repo") or {}
    eligible = {name for name, ok in repo.items() if ok}
    return sorted(set(floor) - eligible)


def check_forward(tasks, t_targets, exempt=None):
    """(missing, unexpected, bad_exempt) — strict equality over `T`'s repo-owned partition.

    `got` deliberately counts every `T` entry that names ANY `repo` task, eligible or not. That is
    what makes flipping a gate to `runInCI: false` while leaving it in `T` show up as `unexpected`
    instead of passing three green checks (D3).
    """
    exempt = T_EXEMPT if exempt is None else exempt
    repo = tasks.get("repo")
    if repo is None:
        raise MoonOutputError("`moon query tasks` reported no `repo` project")
    eligible = {name for name, ok in repo.items() if ok}
    want = eligible - set(exempt)
    got = {name for name in t_targets if name in repo}
    bad_exempt = sorted(name for name, reason in exempt.items() if not (reason or "").strip())
    return sorted(want - got), sorted(got - want), bad_exempt


def check_reverse(tasks, t_targets):
    """`T` entries that resolve to no CI-ELIGIBLE task anywhere in the graph.

    Eligibility, not mere existence: plain resolvability would let `:typecheck` pass while every
    task it names had been turned off. `moon ci` exits 0 on an unresolvable target — including in
    the mixed case — so nothing else in CI reports this (D4).
    """
    live = {name for row in tasks.values() for name, ok in row.items() if ok}
    return sorted(name for name in t_targets if name not in live)


def check_docs(t_targets, doc_targets, region):
    """Problems with CLAUDE.md's documented command: ordered mirror of `T`, plus the flag tail."""
    problems = []
    if doc_targets != t_targets:
        for i, (doc, want) in enumerate(zip_longest(doc_targets, t_targets)):
            if doc != want:
                problems.append(
                    f"first divergence at position {i}: CLAUDE.md has "
                    f"{':' + doc if doc else '<end of list>'}, ci.yml's T has "
                    f"{':' + want if want else '<end of list>'}"
                )
                break
        problems.append("CLAUDE.md: " + " ".join(":" + name for name in doc_targets))
        problems.append("ci.yml  T: " + " ".join(":" + name for name in t_targets))
    for flag in REQUIRED_DOC_FLAGS:
        if flag not in region:
            problems.append(f"the documented command is missing `{flag}`")
    return problems


def check_self_invocation(run_sh_text):
    """Call sites of this gate that are missing from run.sh."""
    return [site for site in RUN_SH_CALL_SITES if site not in run_sh_text]


def self_test():
    """Negative control: every assertion must FIRE on a synthetic violation.

    Drives the PARSERS as well as the checks. The parsers are the component this gate cannot
    self-detect a fault in — a total match failure hits the rc-1 path, but a PARTIAL mis-parse is
    silent — and hand-rolled text extraction "is exactly the kind of thing that silently does the
    wrong thing" (ci/actionlint/run.sh:265, which backs that claim with ~35 extractor fixtures).
    """
    failures = []

    def expect_targets(label, text, want):
        try:
            got = parse_t(text)
        except GateAssertionError as exc:
            failures.append(f"parse_t[{label}]: unexpected red: {exc}")
            return
        if got != want:
            failures.append(f"parse_t[{label}]: got {got}, want {want}")

    def expect_red(label, text):
        try:
            parse_t(text)
        except GateAssertionError:
            return
        failures.append(f"parse_t[{label}]: accepted input that should have been rejected")

    expect_targets("canonical", "          T=(:build :test :deny)\n", ["build", "test", "deny"])
    expect_targets(
        "indented-in-yaml",
        "jobs:\n  ci:\n    run: |\n      T=(:a :b)\n      moon ci \"${T[@]}\"\n",
        ["a", "b"],
    )
    expect_targets("hash-comment-is-not-an-assignment", "# T=(:ghost)\nT=(:real)\n", ["real"])
    expect_red("no-array", "moon ci --base origin/main\n")
    expect_red("two-arrays", "T=(:a)\nT=(:b)\n")
    expect_red("append", "T=(:a)\nT+=(:b)\n")
    expect_red("empty-array", "T=()\n")
    expect_red("trailing-comment", "T=(:a :b)  # note\n")
    expect_red("project-scoped-entry", "T=(:a repo:promtool)\n")
    expect_red("bare-token", "T=(:a build)\n")

    def expect_doc(label, text, want_targets, want_region_contains=()):
        try:
            got, region = parse_doc_targets(text)
        except GateAssertionError as exc:
            failures.append(f"parse_doc_targets[{label}]: unexpected red: {exc}")
            return
        if got != want_targets:
            failures.append(f"parse_doc_targets[{label}]: got {got}, want {want_targets}")
        for needle in want_region_contains:
            if needle not in region:
                failures.append(f"parse_doc_targets[{label}]: region lost {needle!r}")

    def expect_doc_red(label, text):
        try:
            parse_doc_targets(text)
        except GateAssertionError:
            return
        failures.append(f"parse_doc_targets[{label}]: accepted input that should have been rejected")

    wrapped = (
        "intro (e.g. `:deny`, `:osv`) prose\n"
        f"  {MARKER_BEGIN}\n"
        "  `moon ci :build :test\n"
        "  :deny :promtool\n"
        "  --base origin/main --include-relations`\n"
        f"  {MARKER_END}\n"
        "trailing prose with `moon ci :other --include-relations`\n"
    )
    expect_doc(
        "wrapped-span",
        wrapped,
        ["build", "test", "deny", "promtool"],
        ("--base origin/main", "--include-relations"),
    )
    expect_doc_red("no-markers", "`moon ci :build --include-relations`\n")
    expect_doc_red("only-begin", f"{MARKER_BEGIN}\n`moon ci :build`\n")
    expect_doc_red("duplicate-begin", f"{MARKER_BEGIN}\n{MARKER_BEGIN}\nx\n{MARKER_END}\n")
    expect_doc_red("inverted", f"{MARKER_END}\n`moon ci :build`\n{MARKER_BEGIN}\n")
    expect_doc_red("empty-region", f"{MARKER_BEGIN}\n\n{MARKER_END}\n")

    def expect_infra(label, call):
        try:
            call()
        except MoonOutputError:
            return
        except Exception as exc:  # any other exception type is itself the failure
            failures.append(
                f"{label}: raised {type(exc).__name__} instead of MoonOutputError: {exc}"
            )
            return
        failures.append(f"{label}: accepted an output shape that must abort as infrastructure")

    # The rc-2 raises, driven directly. moon_tasks() is a subprocess shell around _eligibility()
    # precisely so these are reachable from a fixture: an unexercised raise is indistinguishable
    # from an absent one, which is the drift class this gate exists to close.
    expect_infra("_eligibility[no-projects]", lambda: _eligibility({}))
    expect_infra("_eligibility[no-options-anywhere]", lambda: _eligibility({"repo": {"deny": {}}}))

    # ...and the POLARITY itself, pinned in both directions so the default-toward-inclusion rule
    # (D8) is asserted rather than assumed: only an explicit `runInCI: false` is ineligible.
    polarity = _eligibility(
        {
            "repo": {
                "install-hooks": {"options": {"runInCI": False}},
                "deny": {"options": {}},
                "promtool": {},
            }
        }
    )
    want_polarity = {"repo": {"install-hooks": False, "deny": True, "promtool": True}}
    if polarity != want_polarity:
        failures.append(f"_eligibility[polarity]: got {polarity}, want {want_polarity}")

    # project id -> task name -> CI-eligible. Mirrors moon_tasks()'s return shape.
    tasks_fixture = {
        "repo": {"deny": True, "promtool": True, "affected-smoke": True,
                 "publish-metadata": True, "install-hooks": False},
        "some-crate-rs": {"build": True, "test": True, "build-release": True},
    }
    aligned_t = ["build", "test", "deny", "promtool", "affected-smoke", "publish-metadata"]

    def forward(label, tasks, t, exempt, want_missing, want_unexpected, want_bad_exempt=()):
        missing, unexpected, bad = check_forward(tasks, t, exempt)
        if (missing, unexpected, bad) != (list(want_missing), list(want_unexpected), list(want_bad_exempt)):
            failures.append(
                f"check_forward[{label}]: got {missing}/{unexpected}/{bad}, want "
                f"{list(want_missing)}/{list(want_unexpected)}/{list(want_bad_exempt)}"
            )

    forward("aligned", tasks_fixture, aligned_t, {}, [], [])
    # AC #3: a runInCI:false task absent from T must not trip the gate — asserted with SEVERAL of
    # them, so the exclusion is a rule and not an accident of install-hooks happening to be alone.
    # (A fixture identical to "aligned" would restate that case without testing anything new.)
    two_disabled = {**tasks_fixture,
                    "repo": {**tasks_fixture["repo"], "install-hooks": False, "second-hook": False}}
    forward("runInCI-false-absent", two_disabled, aligned_t, {}, [], [])
    # A new repo gate that nobody added to T.
    forward("missing-gate", {**tasks_fixture, "repo": {**tasks_fixture["repo"], "new-gate": True}},
            aligned_t, {}, ["new-gate"], [])
    # THE BLOCKER: a gate flipped to runInCI:false but LEFT in T. A subset test passes this.
    forward("disabled-but-still-in-T",
            {**tasks_fixture, "repo": {**tasks_fixture["repo"], "promtool": False}},
            aligned_t, {}, [], ["promtool"])
    # A task in T_EXEMPT with a reason may be absent from T...
    forward("exempt-absent", tasks_fixture,
            [t for t in aligned_t if t != "promtool"], {"promtool": "runs in its own step"}, [], [])
    # ...but present-AND-exempt is contradictory and must be reported.
    forward("exempt-but-present", tasks_fixture, aligned_t,
            {"promtool": "runs in its own step"}, [], ["promtool"])
    # A bare-membership exemption with no reason is unreviewable — reject it.
    forward("exempt-without-reason", tasks_fixture,
            [t for t in aligned_t if t != "promtool"], {"promtool": "  "}, [], [], ["promtool"])
    # An output with no `repo` project at all is moon telling us nothing -> infra, never a
    # comparison against an empty set.
    expect_infra("check_forward[no-repo-project]",
                 lambda: check_forward({"other-project": {"build": True}}, aligned_t, {}))

    if check_floor(tasks_fixture) != []:
        failures.append("check_floor: fired on a fixture containing every floor member")
    thin = {"repo": {"deny": True}}
    if check_floor(thin) != ["affected-smoke", "promtool", "publish-metadata"]:
        failures.append(f"check_floor: did not name every absent floor member: {check_floor(thin)}")

    def reverse(label, tasks, t, want):
        got = check_reverse(tasks, t)
        if got != list(want):
            failures.append(f"check_reverse[{label}]: got {got}, want {list(want)}")

    # A generic target owned by another project resolves — it must NOT be reported.
    reverse("generic-resolves", tasks_fixture, aligned_t, [])
    reverse("dead-entry", tasks_fixture, aligned_t + ["ghost"], ["ghost"])
    # A name whose every task is runInCI:false is present but would run NOTHING (D4).
    reverse("resolves-only-to-disabled", tasks_fixture, aligned_t + ["install-hooks"],
            ["install-hooks"])

    def docs(label, t, doc, region, want_empty):
        got = check_docs(t, doc, region)
        if bool(got) == want_empty:
            failures.append(f"check_docs[{label}]: got {got}, want_empty={want_empty}")

    full_flags = "moon ci --base origin/main --include-relations"
    docs("aligned", aligned_t, list(aligned_t), full_flags, True)
    docs("doc-missing-target", aligned_t, aligned_t[:-1], full_flags, False)
    docs("doc-extra-target", aligned_t, aligned_t + ["extra"], full_flags, False)
    docs("doc-reordered", aligned_t, list(reversed(aligned_t)), full_flags, False)
    docs("doc-missing-include-relations", aligned_t, list(aligned_t),
         "moon ci --base origin/main", False)
    docs("doc-missing-base", aligned_t, list(aligned_t), "moon ci --include-relations", False)

    wired = (
        'assert_ci_targets() {\n  :\n}\n'
        '  assert_ci_targets || SUITE_RC=1\n'
        '  python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1\n'
    )
    if check_self_invocation(wired):
        failures.append(f"check_self_invocation: fired on wired run.sh: {check_self_invocation(wired)}")
    no_call = wired.replace("  assert_ci_targets || SUITE_RC=1\n", "")
    if not check_self_invocation(no_call):
        failures.append("check_self_invocation: missed a deleted run_suite call")
    no_selftest = wired.replace('  python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1\n', "")
    if not check_self_invocation(no_selftest):
        failures.append("check_self_invocation: missed a deleted --self-test call")

    if failures:
        print("ci-targets self-test FAILED:", file=sys.stderr)
        for f in failures:
            print(f"  {f}", file=sys.stderr)
        return 1
    print("ci-targets self-test OK")
    return 0


def main():
    root = Path(__file__).resolve().parents[2]
    try:
        tasks = moon_tasks()
        t_targets = parse_t((root / ".github" / "workflows" / "ci.yml").read_text())
        doc_targets, region = parse_doc_targets((root / "CLAUDE.md").read_text())
        run_sh = (root / "ci" / "affected-graph" / "run.sh").read_text()
        floor = check_floor(tasks)
        missing, unexpected, bad_exempt = check_forward(tasks, t_targets)
    except GateAssertionError as exc:
        # An authorial mistake, NOT a broken tool: rc 1 so run.sh records a red suite instead of
        # aborting the whole affected-graph guard and losing every other assertion's output (D2).
        print(f"FAIL  [ci-targets] {exc}", file=sys.stderr)
        return 1
    except INFRA_ERRORS as exc:
        print(f"FATAL [ci-targets] could not read the inputs: {exc}", file=sys.stderr)
        return 2

    dead = check_reverse(tasks, t_targets)
    doc_problems = check_docs(t_targets, doc_targets, region)
    missing_sites = check_self_invocation(run_sh)

    if not (floor or missing or unexpected or bad_exempt or dead or doc_problems or missing_sites):
        print(
            f"PASS  {'ci-targets':<18} -> {len(t_targets)} targets: every CI-eligible repo task is "
            "in ci.yml's T, every entry resolves, CLAUDE.md mirrors it"
        )
        return 0

    print("FAIL  [ci-targets] ci.yml's moon ci target array is out of sync", file=sys.stderr)
    for rows, title in (
        (floor,
         "A task this gate REQUIRES to be present is absent from the parsed `repo` set, so the\n"
         "    comparison below may be between two empty sets and assert nothing.\n"
         "    Fix: if the task was genuinely renamed or removed, update REQUIRED_REPO_TASKS in\n"
         "    ci/affected-graph/ci_targets.py. Otherwise the project filter or moon's output\n"
         "    shape has changed — investigate before touching anything else."),
        (missing,
         "A CI-eligible `repo:*` task is NOT in ci.yml's `T=(...)` array, so it does not run in\n"
         "    CI at all — it passes locally and silently does not exist on any PR (SMA-541).\n"
         "    Fix: append `:<name>` to `T` in .github/workflows/ci.yml AND to the command\n"
         "    between the <!-- ci-targets:begin/end --> markers in CLAUDE.md."),
        (unexpected,
         "`T` contains a `repo` task that is NOT CI-eligible (runInCI: false) or is listed in\n"
         "    T_EXEMPT. `moon ci` will resolve nothing for it and still exit 0, so the gate reads\n"
         "    as running while it is off.\n"
         "    Fix: remove the entry from `T` and from CLAUDE.md, or drop the `runInCI: false` /\n"
         "    the T_EXEMPT entry if the task is meant to run."),
        (bad_exempt,
         "A T_EXEMPT entry has no reason string. An exemption is a recorded decision, so the\n"
         "    record is what earns it.\n"
         "    Fix: give it a non-empty reason in ci/affected-graph/ci_targets.py, or delete it."),
        (dead,
         "A `T` entry resolves to no CI-eligible task anywhere in the graph — a typo, or a task\n"
         "    that was renamed, deleted or turned off. `moon ci` exits 0 on such a target, even\n"
         "    when real targets surround it, so nothing else in CI reports this.\n"
         "    Fix: correct the entry in .github/workflows/ci.yml and CLAUDE.md, or delete it."),
        (doc_problems,
         "CLAUDE.md's documented full-graph command no longer mirrors `T`, so the documented way\n"
         "    to reproduce CI locally does not reproduce it.\n"
         "    Fix: copy `T` verbatim between the <!-- ci-targets:begin/end --> markers, keeping\n"
         "    the `--base origin/main --include-relations` tail."),
        (missing_sites,
         "This gate's own call site is missing from ci/affected-graph/run.sh, so it (or its\n"
         "    negative control) would not run at all.\n"
         "    Fix: restore the exact line; see RUN_SH_CALL_SITES in this file."),
    ):
        if rows:
            print(f"  {title}", file=sys.stderr)
            for row in rows:
                print(f"      {row}", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(self_test() if "--self-test" in sys.argv[1:] else main())
