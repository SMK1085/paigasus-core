# SPDX-License-Identifier: Apache-2.0
"""Release guard (SMA-579) — assert every job that can reach a registry is gated.

WHY A REAL PARSER, NOT grep. The verdict must tell a JOB-level `if:` from the EIGHT identical
STEP-level ones release.yml already carries, and must walk `needs:` chains. Neither is a
line-oriented question. SMA-593 exists because ci/publish-metadata hand-rolled a partial YAML
scanner and 14 spellings evaded it; a second hand-rolled scanner would recreate that defect class
in a guard whose whole job is structural.

PyYAML is a YAML 1.1 parser and GitHub's schema collides with it in five measured places — see
COERCIONS in the fixture table. The `on:` key parsing as the boolean True is the one that bites
first.

FAIL-CLOSED. Every abnormal condition exits 2 (infra). Never a skip, never a pass.
"""

from __future__ import annotations

import contextlib
import io
import os
import re
import sys
import tempfile
from pathlib import Path
from typing import NoReturn

try:
    import yaml
except ImportError:  # pragma: no cover - exercised by the missing-interpreter path
    print("release-guard: PyYAML is not importable. This gate needs the pinned pyyaml from "
          "py/pyproject.toml — invoke via `uv run --project py`.", file=sys.stderr)
    raise SystemExit(2)

# --- The pinned vocabulary ---------------------------------------------------------------------

# V2: the gate expression is pinned as a LITERAL, accepted in exactly two forms. A substring test
# would admit `!= 'disabled'` and `== 'true' || github.actor == 'x'`, both of which always run.
GATE_EXPR = "vars.PAIGASUS_RELEASE_ENABLED == 'true'"
ACCEPTED_GATE_FORMS = frozenset({GATE_EXPR, "${{ " + GATE_EXPR + " }}"})

# V1: the subject is EVERY job; the exemption is the pin. Inverted from a detection-derived set,
# which could not see a publish step using an unrecognised mechanism (SMA-579 review).
#
# Fix round 3, Critical 2: an exemption here is an exemption from the GATING rule only. It was an
# exemption from EVERYTHING, and that was measured: a `release-pr` job whose three steps ran
# `cargo publish`, `npm publish` and `pypa/gh-action-pypi-publish` exited 0 with no violations,
# while the identical steps in any other job correctly exited 1. `release-pr` runs on every push
# to `main` with an App installation token, so that hole was live. V7 below now applies the V6
# publish DETECTOR to every member of this set: a member may skip the gate, but must never
# contain a publish step. The exemption's premise ("release-pr cannot reach a registry") is now
# asserted rather than assumed.
UNGATED_JOBS = frozenset({"release-pr"})

# V8: the approval gate is the ONE human checkpoint in release.yml, and everything downstream of
# it is irreversible. Two directions, and BOTH are needed: V8b says nothing upstream may publish,
# V8c says every publisher must be downstream. Without V8c, deleting `approve-release` from the
# `release` job's needs: removes the only gate in the file and passes V1, V3, V4, V7 and V8a/b.
APPROVAL_JOB = "approve-release"

# V3: the real bypass class is any status-check function, not two literal spellings.
# `success() || failure()`, `!failure()` and `${{ ! cancelled() }}` all evade a two-string test.
STATUS_FUNCS = ("always", "cancelled", "success", "failure")

# V6/V7: detection. Used for called workflows (where UNGATED_JOBS has no meaning) and, since fix
# round 3, for UNGATED_JOBS members in the main workflow too.
#
# REGEXES, not substrings, and the first entry is why. `release-plz release` is a strict PREFIX of
# `release-plz release-pr`, which is exactly what the real, correct `release-pr` job runs. Measured:
#
#   'release-plz release' in 'release-plz release-pr --output json'            -> True   (false red)
#   re.search(r'release-plz\s+release(?![-\w])', <the same string>)            -> False  (correct)
#   re.search(r'release-plz\s+release(?![-\w])', 'release-plz release --out')  -> True   (correct)
#
# A naive substring test here would red the real repository on the very PR that added V7. The
# `(?![-\w])` boundary is what keeps `release-pr` distinct from `release`.
#
# Fix round 3, Important 3: `maturin publish`, `maturin upload`, `uv publish` and `yarn publish`
# were missing, and this repo's own Python publishing IS maturin — `wheels.yml` carries both
# `pull_request` and `push` triggers, and `maturin publish` is a one-word edit from the
# `maturin build` already in it. Spec §8.3 leaves detection as the sole rule for called workflows,
# so a list omitting the repo's own tooling was the weakest point of that rule.
#
# `npm\s+publish` deliberately also matches `pnpm publish` (the substring is contained in it).
# That is a superset in the safe direction: it detects more publish mechanisms, never fewer.
PUBLISH_MARKERS = (
    r"release-plz\s+release(?![-\w])",
    r"npm\s+publish",
    r"yarn\s+publish",
    r"napi\s+prepublish",
    r"twine\s+upload",
    r"gh-action-pypi-publish",
    r"cargo\s+publish",
    r"maturin\s+publish",
    r"maturin\s+upload",
    r"uv\s+publish",
)
_PUBLISH_RE = re.compile("|".join(PUBLISH_MARKERS))

# V5: matches V6's own whitespace tolerance (`napi\s+prepublish` in PUBLISH_MARKERS above). V5 used
# to test the literal substring "napi prepublish", so `napi  prepublish` (two spaces) or a tab
# between the words was recognised as a publish step by V6 while V5's --no-gh-release assertion
# never fired on it — the tagging boundary went unasserted for a command the guard otherwise knew
# about (CodeRabbit round 1 finding 2).
_NAPI_PREPUBLISH_RE = re.compile(r"napi\s+prepublish")


def infra(msg: str) -> NoReturn:
    print(f"release-guard: {msg}", file=sys.stderr)
    raise SystemExit(2)


# --- Parsing -----------------------------------------------------------------------------------

def load_workflow(path: Path) -> dict:
    """Parse one workflow. Fail-closed on every abnormal condition."""
    if not path.is_file():
        infra(f"{path}: not a readable file")
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError) as exc:
        # Fix round 1, Important 5: `path.is_file()` proves the file exists, not that it can be
        # read. A permission-denied or invalid-UTF-8 file used to raise an unhandled traceback
        # that `main()` would report as a Python exit status, blurring the 1-vs-2 distinction
        # spec §8.5 exists to preserve. Both must be infra (2), never treated as "violations found".
        infra(f"{path}: unreadable: {exc}")
    try:
        docs = [d for d in yaml.safe_load_all(text) if d is not None]
    except yaml.YAMLError as exc:
        infra(f"{path}: unparseable YAML: {exc}")
    if len(docs) != 1:
        infra(f"{path}: expected exactly 1 YAML document, found {len(docs)}")
    doc = docs[0]
    if not isinstance(doc, dict):
        infra(f"{path}: top level is {type(doc).__name__}, expected a mapping")
    if not isinstance(doc.get("jobs"), dict):
        infra(f"{path}: 'jobs:' is missing or not a mapping")
    return doc


def triggers_of(doc: dict) -> dict:
    """`on:` parses as the BOOLEAN True under YAML 1.1. Measured on release.yml: top-level keys
    come back as ['name', True, 'concurrency', 'permissions', 'jobs']."""
    raw = doc.get("on", doc.get(True))
    if isinstance(raw, str):
        return {raw: None}
    if isinstance(raw, list):
        return {k: None for k in raw}
    if isinstance(raw, dict):
        return raw
    infra("'on:' is missing or of an unexpected type")


def needs_of(job: dict) -> list[str]:
    """`needs:` may be a scalar string. Iterating a str yields CHARACTERS and silently walks
    nothing — which would make the transitive half of V1 vacuous, and that half carries every job
    but `plan`."""
    raw = job.get("needs")
    if raw is None:
        return []
    if isinstance(raw, str):
        return [raw]
    if isinstance(raw, list):
        return [str(x) for x in raw]
    infra(f"'needs:' is {type(raw).__name__}, expected a string or list")


def if_text(job: dict) -> str | None:
    """`if: false` parses to the BOOLEAN False, not the string 'false'."""
    raw = job.get("if")
    if raw is None:
        return None
    if isinstance(raw, bool):
        return "true" if raw else "false"
    return str(raw).strip()


def coe_is_false(job_or_step: dict) -> bool:
    """V4. `continue-on-error: false` parses to the BOOLEAN False; `"false"` stays a STRING and
    GitHub treats it as false too. Accept both; reject everything else."""
    raw = job_or_step.get("continue-on-error")
    if raw is None:
        return True
    if isinstance(raw, bool):
        return raw is False
    return str(raw).strip() == "false"


# --- The verdict -------------------------------------------------------------------------------

def is_gated(job_id: str, jobs: dict, seen: frozenset[str] = frozenset()) -> bool:
    """V1. Gated directly, or through an unbroken `needs:` chain from a gated job."""
    if job_id in seen:          # a needs: cycle is not a gate
        return False
    job = jobs.get(job_id)
    if not isinstance(job, dict):
        return False
    if if_text(job) in ACCEPTED_GATE_FORMS:
        return True
    deps = needs_of(job)
    if not deps:
        return False
    # EVERY dependency must be gated: one ungated path is an ungated job.
    return all(is_gated(d, jobs, seen | {job_id}) for d in deps)


def gated_path_jobs(job_id: str, jobs: dict, seen: frozenset[str] = frozenset()) -> set[str]:
    """The job plus every job on its needs: path — the set V3/V4 apply to."""
    if job_id in seen or job_id not in jobs:
        return set()
    out = {job_id}
    for d in needs_of(jobs[job_id]):
        out |= gated_path_jobs(d, jobs, seen | {job_id})
    return out


_COMMAND_SEPS = re.compile(r"&&|\|\||;|\|")


def command_segments(line: str) -> list[str]:
    """Split ONE `run:` line into the shell command segments chained by `&&`, `||`, `;` or `|`,
    then strip a trailing `#` comment from each segment.

    Fix round 2, Important 1: round 1's fix moved from a whole-BLOCK substring test to a
    whole-LINE one, which closed every fixture reported at the time — but "does this line
    contain the flag anywhere" is the same class of test, one granularity narrower, and shell
    chaining on the SAME line defeats it: `napi prepublish --npm-dir npm && echo
    "--no-gh-release"` reads the flag as present even though the real invocation never received
    it. Evaluating the SEGMENT that actually contains the invocation closes that, and a comment
    like `napi prepublish --npm-dir npm  # remember --no-gh-release` must not satisfy the check
    either.

    NOT a shell parser (Ruling 10, fix round 2): this is a bare regex split with no quote/escape
    awareness — a `&&`, `|` or `#` inside a quoted string is mishandled, and there is no
    flag-adjacency analysis. That is a deliberate scope limit: a full tokeniser would close a
    bypass nobody has demonstrated at the cost of new parsing surface in the one file where a bug
    is most expensive. The residual is recorded in the spec's limitations, not chased here.
    """
    return [seg.split("#", 1)[0] for seg in _COMMAND_SEPS.split(line)]


def job_publishes(job: dict) -> bool:
    """V6 detection. Used ONLY for called workflows.

    Fix round 1, Important 3: evaluated per LINE, not per whole `run:` block. A `--dry-run`
    occurrence reaches no registry — `napi prepublish --dry-run --no-gh-release` (prebuild.yml)
    must not trip this, or the gate is unpassable on a correct repository. Checking the whole
    block would let a `--dry-run` anywhere in a multi-line script silence a REAL invocation on
    another line; per-line scoping is the same fix shape as V5's Important 4.

    Fix round 2, Important 1: evaluated per COMMAND SEGMENT (see command_segments), not per whole
    line — `npm publish && echo "not --dry-run"` must still count as registry-reaching, since the
    marker and the `--dry-run` flag are in different chained commands on the same line.
    """
    for step in job.get("steps") or []:
        if not isinstance(step, dict):
            continue
        blob = f"{step.get('run', '')}\n{step.get('uses', '')}"
        for line in blob.splitlines():
            for segment in command_segments(line):
                if "--dry-run" in segment:
                    continue
                if _PUBLISH_RE.search(segment):
                    return True
    return False


def napi_violations(job: dict, job_id: str, name: str) -> list[str]:
    """V5: the tagging boundary (spec §2), enforced rather than documented.

    Ruling 8 (fix round 1): applies to EVERY job, including UNGATED_JOBS members — `napi
    prepublish` cuts a git tag regardless of whether the job that ran it was gated, and release-plz
    must own every tag (ADR-0011 S3).

    Fix round 1, Important 4: evaluated per LINE, not per whole `run:` block — a comment or
    unrelated line mentioning the flag must not satisfy a check over the real invocation.

    Fix round 2, Important 1: evaluated per COMMAND SEGMENT (see command_segments), not per whole
    line — `napi prepublish --npm-dir npm && echo "--no-gh-release"` and a trailing
    `# remember --no-gh-release` comment must still red, since the flag never reaches the real
    invocation in either case.

    Fix round 3, Important 4: FACTORED OUT of check_main so check_called can apply it too. It was
    inlined in check_main, and `main()` runs check_main on argv[0] ONLY — every callee got
    check_called, which had no V5 at all. So `prebuild.yml:295`, the invocation whose own comment
    says the flag "IS REQUIRED", was unguarded, while CLAUDE.md claimed V5 asserts *every*
    invocation carries it. Calling this from both sites is what makes that sentence true.
    """
    out: list[str] = []
    for step in job.get("steps") or []:
        if not isinstance(step, dict):
            continue
        run = str(step.get("run") or "")
        for line in run.splitlines():
            for segment in command_segments(line):
                # The flag must appear AFTER the invocation, not merely somewhere in the segment.
                # `NOTE=--no-gh-release napi prepublish --npm-dir npm` is a shell ENVIRONMENT
                # ASSIGNMENT followed by the command: the flag never reaches napi, and a plain
                # `in segment` test accepts it. Comparing against the match END is enough here and
                # stops short of tokenising the command line — see Ruling 10 / the module docstring
                # for why this guard deliberately does not embed a shell parser.
                m = _NAPI_PREPUBLISH_RE.search(segment)
                if m and "--no-gh-release" not in segment[m.end() :]:
                    out.append(
                        f"{name}: job '{job_id}' runs `napi prepublish` without "
                        f"--no-gh-release. release-plz owns every tag (ADR-0011 S3); napi "
                        f"must never cut one."
                    )
    return out


def approval_boundary_violations(jobs: dict, name: str) -> list[str]:
    """V8. The approval gate is the ONE human checkpoint; everything downstream of it is
    irreversible. V8a is the floor (without it the rest of V8 passes vacuously); V8b asserts
    nothing upstream of the gate may publish; V8c asserts every publisher IS downstream of it —
    the direction V8b alone cannot cover, since deleting `approve-release` from a publishing
    job's needs: satisfies V8b trivially (there is nothing left upstream of the gate to check)."""
    out: list[str] = []
    gate = jobs.get(APPROVAL_JOB)
    if not isinstance(gate, dict):
        return [f"{name}: V8a: no job named '{APPROVAL_JOB}' exists. Every other clause of V8 is "
                f"defined relative to it, so without it this verdict would pass vacuously."]
    if not gate.get("environment"):
        out.append(f"{name}: V8a: job '{APPROVAL_JOB}' declares no environment:. The pause that "
                   f"makes it a gate comes from the environment's required reviewers; without "
                   f"the key it is an ordinary job that always succeeds.")

    for jid in sorted(gated_path_jobs(APPROVAL_JOB, jobs)):
        job = jobs.get(jid)
        if isinstance(job, dict) and job_publishes(job):
            out.append(f"{name}: V8b: job '{jid}' runs upstream of '{APPROVAL_JOB}' and contains "
                       f"a step that can reach a registry. That publishes before any human "
                       f"approves. Add --dry-run, or move the step downstream of the gate.")

    for jid, job in jobs.items():
        if not isinstance(job, dict) or not job_publishes(job):
            continue
        if APPROVAL_JOB not in gated_path_jobs(jid, jobs):
            out.append(f"{name}: V8c: job '{jid}' can reach a registry, but '{APPROVAL_JOB}' is "
                       f"not on its needs: path. It would publish without passing the gate.")
    return out


def check_main(doc: dict, name: str) -> list[str]:
    """V1-V5 over the release workflow."""
    out: list[str] = []
    jobs = doc["jobs"]

    if not jobs:
        # Fix round 1, Minor 9: an empty `jobs: {}` mapping is a valid dict, so it sailed through
        # load_workflow's isinstance check, then the loop below examined zero jobs and returned
        # a false-clean []. A check that asserts nothing must not report success.
        infra(f"{name}: 'jobs:' is empty; nothing to assert")

    for job_id, job in jobs.items():
        if not isinstance(job, dict):
            infra(f"{name}: job '{job_id}' is not a mapping")

        # V5: the tagging boundary. Applies to EVERY job, UNGATED_JOBS members included, so it
        # runs BEFORE the `continue` below. See napi_violations for the full rationale.
        out += napi_violations(job, job_id, name)

        if job_id in UNGATED_JOBS:
            # V7 (fix round 3, Critical 2). The exemption above is from V1 (the gating rule) and
            # nothing else. Assert the premise that justifies it: a job allowed to run ungated on
            # every push to `main` must not be able to reach a registry. Without this, a
            # `release-pr` job carrying `cargo publish` + `npm publish` +
            # `pypa/gh-action-pypi-publish` passed the whole guard clean — measured, exit 0.
            if job_publishes(job):
                out.append(
                    f"{name}: job '{job_id}' is exempt from the gate (UNGATED_JOBS) but contains a "
                    f"step that can reach a registry. An exempt job runs on every push to main, "
                    f"ungated — it may never publish. Remove the publish step, or remove the job "
                    f"from UNGATED_JOBS and gate it."
                )
            continue

        if not is_gated(job_id, jobs):
            out.append(
                f"{name}: job '{job_id}' is not gated on PAIGASUS_RELEASE_ENABLED, directly or "
                f"through an unbroken needs: chain. Add the gate, extend the chain, or add it to "
                f"UNGATED_JOBS with a stated reason."
            )
            continue

        # V3/V4 apply to the job AND every job on its needs: path — an always() upstream
        # un-gates everything downstream of it. Scoped to gated paths only (Ruling 8): these
        # verdicts are about defeating a GATE, which has no meaning for a job that cannot publish.
        for pid in gated_path_jobs(job_id, jobs):
            pjob = jobs[pid]
            # Fix round 1, Critical 1: GitHub Actions expression function names are
            # case-insensitive (confirmed against the repo's pinned actionlint 1.7.12), so
            # `Always()`, `ALWAYS()` and `!Cancelled()` are all working bypasses. Normalise case
            # AND strip ALL whitespace (not only U+0020, closing Minor 7's tab case for free).
            norm = "".join((if_text(pjob) or "").split()).lower()
            for fn in STATUS_FUNCS:
                if f"{fn}(" in norm:
                    out.append(
                        f"{name}: job '{pid}' (on '{job_id}'s gated path) uses the status "
                        f"function {fn}() in its if:. That defeats the gate for every job "
                        f"downstream of it."
                    )
            if not coe_is_false(pjob):
                out.append(
                    f"{name}: job '{pid}' carries continue-on-error: "
                    f"{pjob.get('continue-on-error')!r}. A failed job then counts as success for "
                    f"needs:, so a failed publish still releases downstream."
                )
            for step in pjob.get("steps") or []:
                if isinstance(step, dict) and not coe_is_false(step):
                    out.append(
                        f"{name}: job '{pid}' has a step with continue-on-error: "
                        f"{step.get('continue-on-error')!r}. That hides a failed publish inside a "
                        f"job that still reports success."
                    )

    # V8: the approval boundary, both directions. Called once, outside the per-job loop above —
    # that loop has `continue` statements that would skip a call placed inside it.
    out += approval_boundary_violations(jobs, name)
    return out


def check_called(doc: dict, name: str) -> list[str]:
    """V6. A workflow the release path CALLS may publish only if it is workflow_call-ONLY.

    Revision 1 of the spec claimed such a workflow inherits the caller's gate. It does not:
    wheels.yml and prebuild.yml carry their own push: and pull_request: triggers, so a publish
    step added to one would run ungated on every PR while the caller's gate stayed green.
    """
    out: list[str] = []

    # V5 also applies here (fix round 3, Important 4). It used to be inlined in check_main, which
    # main() runs on argv[0] only, so every CALLED workflow escaped it — including prebuild.yml's
    # own `napi prepublish` invocation, the one whose comment says the flag "IS REQUIRED".
    for jid, j in doc["jobs"].items():
        if isinstance(j, dict):
            out += napi_violations(j, jid, name)

    publishing = [jid for jid, j in doc["jobs"].items() if isinstance(j, dict) and job_publishes(j)]
    if not publishing:
        return out
    trigs = set(triggers_of(doc))
    if trigs != {"workflow_call"}:
        out.append(
            f"{name}: jobs {sorted(publishing)} can reach a registry, but this workflow's "
            f"triggers are {sorted(str(t) for t in trigs)}. A workflow called from the release "
            f"path may publish only if it is workflow_call-ONLY — otherwise the publish runs "
            f"ungated on its own triggers while the caller's gate stays green."
        )
    return out


def pre_approval_callees(doc: dict) -> list[Path]:
    """Local reusable workflows called from a job upstream of the approval gate."""
    jobs = doc.get("jobs") or {}
    if APPROVAL_JOB not in jobs:
        return []
    out = []
    for jid in gated_path_jobs(APPROVAL_JOB, jobs):
        job = jobs.get(jid)
        uses = str(job.get("uses") or "") if isinstance(job, dict) else ""
        if uses.startswith("./"):
            out.append(Path(uses.removeprefix("./")))
    return out


def check_called_pre_approval(doc: dict, name: str) -> list[str]:
    """V8d. V6 permits a publish step in a workflow_call-ONLY workflow. That permission predates
    the approval gate and is unsafe for a callee invoked from upstream of it."""
    return [f"{name}: V8d: job '{jid}' can reach a registry, and this workflow is called from a "
            f"job upstream of '{APPROVAL_JOB}'. V6's workflow_call-only permission does not "
            f"apply here — it would publish before any human approves."
            for jid, j in doc["jobs"].items() if isinstance(j, dict) and job_publishes(j)]


# --- Fixtures ----------------------------------------------------------------------------------
# Each row: (name, kind, yaml, expected_substring | None). None means "must be clean".
# The arity floor in ci/actionlint/run.sh pins len(FIXTURES) — emptying this table would
# otherwise be invisible to check 7's bash-only definition counter.

_OK_MAIN = """
on:
  push:
    branches:
      - main
jobs:
  release-pr:
    runs-on: ubuntu-latest
    steps: [{run: echo hi}]
  plan:
    if: vars.PAIGASUS_RELEASE_ENABLED == 'true'
    runs-on: ubuntu-latest
    outputs:
      nothing_to_release: ${{ steps.decide.outputs.nothing_to_release }}
    steps:
      - id: decide
        run: ci/release-plan/run.sh --github-output
  build:
    needs: [plan]
    if: needs.plan.outputs.nothing_to_release != 'true'
    runs-on: ubuntu-latest
    steps: [{run: echo build}]
  approve-release:
    needs: [build]
    environment: release-approval
    runs-on: ubuntu-latest
    steps: [{run: echo approved}]
  release:
    needs: [build, approve-release]
    runs-on: ubuntu-latest
    steps: [{run: release-plz release}]
"""

FIXTURES: list[tuple[str, str, str, str | None]] = [
    ("healthy control", "main", _OK_MAIN, None),
    ("ungated job", "main", _OK_MAIN.replace("    if: vars.PAIGASUS_RELEASE_ENABLED == 'true'\n", ""),
     "is not gated"),
    ("gate expression weakened to !=", "main",
     _OK_MAIN.replace("vars.PAIGASUS_RELEASE_ENABLED == 'true'",
                      "vars.PAIGASUS_RELEASE_ENABLED != 'disabled'"), "is not gated"),
    ("gate expression widened with ||", "main",
     _OK_MAIN.replace("vars.PAIGASUS_RELEASE_ENABLED == 'true'",
                      "vars.PAIGASUS_RELEASE_ENABLED == 'true' || github.actor == 'x'"),
     "is not gated"),
    ("wrapped gate form is accepted", "main",
     _OK_MAIN.replace("if: vars.PAIGASUS_RELEASE_ENABLED == 'true'",
                      "if: ${{ vars.PAIGASUS_RELEASE_ENABLED == 'true' }}"), None),
    ("always() on the gated job", "main",
     _OK_MAIN.replace("    needs: [build, approve-release]",
                      "    needs: [build, approve-release]\n    if: always()"), "status function"),
    ("!cancelled() with spacing", "main",
     _OK_MAIN.replace("    needs: [build, approve-release]",
                      "    needs: [build, approve-release]\n    if: ${{ ! cancelled() }}"),
     "status function"),
    ("success() || failure()", "main",
     _OK_MAIN.replace("    needs: [build, approve-release]",
                      "    needs: [build, approve-release]\n    if: success() || failure()"),
     "status function"),
    ("job-level continue-on-error: true", "main",
     _OK_MAIN.replace("    needs: [build, approve-release]",
                      "    needs: [build, approve-release]\n    continue-on-error: true"),
     "continue-on-error"),
    ("step-level continue-on-error: true", "main",
     _OK_MAIN.replace("steps: [{run: release-plz release}]",
                      "steps: [{run: release-plz release, continue-on-error: true}]"),
     "step with continue-on-error"),
    ("continue-on-error: false (bool) is accepted", "main",
     _OK_MAIN.replace("    needs: [build, approve-release]",
                      "    needs: [build, approve-release]\n    continue-on-error: false"), None),
    ('continue-on-error: "false" (str) is accepted', "main",
     _OK_MAIN.replace("    needs: [build, approve-release]",
                      '    needs: [build, approve-release]\n    continue-on-error: "false"'), None),
    # These two anchor on `build`'s "    needs: [plan]" line (the release job's own needs is now
    # the two-item "[build, approve-release]", which a scalar cannot represent losslessly).
    # `build` still carries a genuine single-item needs list, so retargeting these rows onto
    # `release` would either drop a dependency or require inventing new semantics; leaving them on
    # `build` preserves the original list-to-scalar test exactly, and adding no `if:`/`needs:`
    # collision (build's own `if:` line is untouched by either mutation).
    ("needs: as a SCALAR string still walks", "main",
     _OK_MAIN.replace("    needs: [plan]", "    needs: plan"), None),
    ("needs: scalar pointing at an ungated job reds", "main",
     _OK_MAIN.replace("    needs: [plan]", "    needs: release-pr"), "is not gated"),
    ("napi prepublish without --no-gh-release", "main",
     _OK_MAIN.replace("run: release-plz release", "run: napi prepublish --npm-dir npm"),
     "without --no-gh-release"),
    ("napi prepublish with --no-gh-release is clean", "main",
     _OK_MAIN.replace("run: release-plz release",
                      "run: napi prepublish --no-gh-release --npm-dir npm"), None),
    ("job-level if: false is MORE restrictive, so clean", "main",
     _OK_MAIN.replace("    needs: [build, approve-release]",
                      "    needs: [build, approve-release]\n    if: false"), None),
    ("called workflow that is workflow_call-only may publish", "called",
     "on:\n  workflow_call:\njobs:\n  build:\n    steps: [{run: twine upload dist/*}]\n", None),
    ("called workflow with pull_request may NOT publish", "called",
     ("on:\n  workflow_call:\n  pull_request:\n    branches:\n      - main\n"
      "jobs:\n  build:\n    steps: [{run: twine upload dist/*}]\n"), "workflow_call-ONLY"),
    ("called workflow with no publish step is clean", "called",
     ("on:\n  workflow_call:\n  pull_request:\n    branches:\n      - main\n"
      "jobs:\n  build:\n    steps: [{run: maturin build}]\n"), None),

    # --- Fix round 1 additions -------------------------------------------------------------
    ("Critical 1: case-insensitive Always() bypass", "main",
     _OK_MAIN.replace("    needs: [build, approve-release]",
                      "    needs: [build, approve-release]\n    if: Always()"),
     "status function"),
    ("Critical 1: case-insensitive ALWAYS() wrapped form", "main",
     _OK_MAIN.replace("    needs: [build, approve-release]",
                      "    needs: [build, approve-release]\n    if: ${{ ALWAYS() }}"),
     "status function"),
    ("Critical 1: case-insensitive !Cancelled() bypass", "main",
     _OK_MAIN.replace("    needs: [build, approve-release]",
                      "    needs: [build, approve-release]\n    if: ${{ !Cancelled() }}"),
     "status function"),
    ("Important 3: napi prepublish --dry-run in a called workflow is not a publish", "called",
     ("on:\n  workflow_call:\n  pull_request:\n    branches:\n      - main\n"
      "jobs:\n  build:\n    steps: [{run: pnpm exec napi prepublish --dry-run --no-gh-release "
      "--npm-dir npm}]\n"), None),
    ("Important 4: V5 is not fooled by a decoy --no-gh-release mention on another line", "main",
     _OK_MAIN.replace(
         "steps: [{run: release-plz release}]",
         "steps:\n      - run: |\n          "
         "echo \"decoy mention of --no-gh-release here\"\n          "
         "napi prepublish --npm-dir npm"),
     "without --no-gh-release"),
    ("Important 6: V5 still applies to an UNGATED_JOBS member", "main",
     _OK_MAIN.replace("steps: [{run: echo hi}]", "steps: [{run: napi prepublish --npm-dir npm}]", 1),
     "without --no-gh-release"),
    ("Ruling 8: continue-on-error on an UNGATED_JOBS member is not V3/V4's concern", "main",
     _OK_MAIN.replace(
         "  release-pr:\n    runs-on: ubuntu-latest\n    steps: [{run: echo hi}]",
         "  release-pr:\n    runs-on: ubuntu-latest\n    continue-on-error: true\n"
         "    steps: [{run: echo hi}]"),
     None),
    ("Minor 10: explicit !!str tag on the gate expression parses correctly", "main",
     _OK_MAIN.replace("if: vars.PAIGASUS_RELEASE_ENABLED == 'true'",
                      "if: !!str vars.PAIGASUS_RELEASE_ENABLED == 'true'"), None),
    ("Minor 10: folded >- scalar gate form is accepted", "main",
     _OK_MAIN.replace("    if: vars.PAIGASUS_RELEASE_ENABLED == 'true'",
                      "    if: >-\n      vars.PAIGASUS_RELEASE_ENABLED == 'true'"), None),

    # --- Fix round 2 additions (Important 1: command-segment scoping) ----------------------
    ("R2 Important 1: chained && hides the missing flag from a whole-line test", "main",
     _OK_MAIN.replace(
         "steps: [{run: release-plz release}]",
         "steps:\n      - run: |\n          "
         'napi prepublish --npm-dir npm && echo "always pass --no-gh-release"'),
     "without --no-gh-release"),
    ("R2 Important 1 CONTROL: the flag IS in the invocation's own segment, so this stays clean",
     "main",
     _OK_MAIN.replace(
         "steps: [{run: release-plz release}]",
         "steps:\n      - run: |\n          "
         "napi prepublish --no-gh-release --npm-dir npm && echo done"),
     None),
    ("R2 Important 1: a trailing '# remember --no-gh-release' comment does not count", "main",
     _OK_MAIN.replace(
         "steps: [{run: release-plz release}]",
         "steps:\n      - run: |\n          "
         "napi prepublish --npm-dir npm  # remember --no-gh-release"),
     "without --no-gh-release"),
    ("R2 Important 1: job_publishes sees a real publish chained with a decoy --dry-run", "called",
     ("on:\n  workflow_call:\n  pull_request:\n    branches:\n      - main\n"
      "jobs:\n  build:\n    steps:\n      - run: |\n          "
      'npm publish && echo "not --dry-run"\n'),
     "workflow_call-ONLY"),

    # --- V8 additions (the approval boundary, both directions) -----------------------------
    ("V8a: no approve-release job at all", "main",
     _OK_MAIN.replace("  approve-release:\n    needs: [build]\n"
                      "    environment: release-approval\n    runs-on: ubuntu-latest\n"
                      "    steps: [{run: echo approved}]\n", ""),
     "V8a"),
    ("V8a: approve-release without an environment", "main",
     _OK_MAIN.replace("    environment: release-approval\n", ""), "V8a"),
    ("V8b: a real publish upstream of approval", "main",
     _OK_MAIN.replace("steps: [{run: echo build}]", "steps: [{run: cargo publish}]"), "V8b"),
    ("V8b CONTROL: a --dry-run publish upstream of approval is clean", "main",
     _OK_MAIN.replace("steps: [{run: echo build}]",
                      "steps: [{run: cargo publish --dry-run}]"), None),
    ("V8b: a uses:-shaped publish upstream of approval", "main",
     _OK_MAIN.replace("steps: [{run: echo build}]",
                      "steps: [{uses: pypa/gh-action-pypi-publish@v1}]"), "V8b"),
    ("V8c: approve-release dropped from release's needs", "main",
     _OK_MAIN.replace("    needs: [build, approve-release]", "    needs: [build]"), "V8c"),

    # --- Fix round 3 additions (Critical 2, Important 3, Important 4) ----------------------
    # Critical 2, both directions. The RED direction is the defect: this exact three-step job was
    # measured passing the whole guard at exit 0 before V7 existed. The two CLEAN directions are
    # the false-red controls that keep V7 usable against the real repository.
    ("R3 Critical 2: an UNGATED_JOBS member containing publish steps reds", "main",
     _OK_MAIN.replace(
         "steps: [{run: echo hi}]",
         "steps:\n      - run: cargo publish -p paigasus-kernel\n"
         "      - run: npm publish --provenance --access public\n"
         "      - uses: pypa/gh-action-pypi-publish@v1", 1),
     "exempt from the gate"),
    ("R3 Critical 2 CONTROL: `release-plz release-pr` in an UNGATED_JOBS member stays clean",
     "main",
     _OK_MAIN.replace("steps: [{run: echo hi}]",
                      "steps: [{run: release-plz release-pr --output json}]", 1),
     None),
    ("R3 Critical 2: the BOUNDED marker still catches a real `release-plz release` when exempt",
     "main",
     _OK_MAIN.replace("steps: [{run: echo hi}]",
                      "steps: [{run: release-plz release --output json}]", 1),
     "exempt from the gate"),

    # Important 3 — the four publish verbs this repo's own tooling uses, which PUBLISH_MARKERS
    # omitted. `wheels.yml` IS a maturin workflow carrying pull_request and push.
    ("R3 Important 3: maturin publish in a non-workflow_call-only callee reds", "called",
     ("on:\n  workflow_call:\n  pull_request:\n    branches:\n      - main\n"
      "jobs:\n  build:\n    steps: [{run: maturin publish --skip-existing}]\n"),
     "workflow_call-ONLY"),
    ("R3 Important 3: maturin upload reds the same way", "called",
     ("on:\n  workflow_call:\n  push:\n    branches:\n      - main\n"
      "jobs:\n  build:\n    steps: [{run: maturin upload dist/*}]\n"),
     "workflow_call-ONLY"),
    ("R3 Important 3: uv publish reds the same way", "called",
     ("on:\n  workflow_call:\n  pull_request:\n    branches:\n      - main\n"
      "jobs:\n  build:\n    steps: [{run: uv publish --trusted-publishing always}]\n"),
     "workflow_call-ONLY"),
    ("R3 Important 3: yarn publish reds the same way", "called",
     ("on:\n  workflow_call:\n  pull_request:\n    branches:\n      - main\n"
      "jobs:\n  build:\n    steps: [{run: yarn publish --access public}]\n"),
     "workflow_call-ONLY"),

    # Important 4 — V5 on a CALLED workflow. `--dry-run` is present on purpose: it keeps V6/V7
    # silent, so the only thing this row can be reporting is V5 itself.
    ("R3 Important 4: V5 now reaches a CALLED workflow's napi prepublish", "called",
     ("on:\n  workflow_call:\n  pull_request:\n    branches:\n      - main\n"
      "jobs:\n  build:\n    steps: [{run: pnpm exec napi prepublish --dry-run --npm-dir npm}]\n"),
     "without --no-gh-release"),
    ("R3 Important 4 CONTROL: a called workflow carrying the flag stays clean", "called",
     ("on:\n  workflow_call:\n  pull_request:\n    branches:\n      - main\n"
      "jobs:\n  build:\n    steps: [{run: pnpm exec napi prepublish --dry-run --no-gh-release "
      "--npm-dir npm}]\n"),
     None),

    # --- CodeRabbit round 1 additions (finding 2: V5 whitespace tolerance) -----------------
    ("CR1 finding 2: V5 catches a two-space `napi  prepublish` without --no-gh-release", "main",
     _OK_MAIN.replace("run: release-plz release", "run: napi  prepublish --npm-dir npm"),
     "without --no-gh-release"),
    # CR2: a shell ENVIRONMENT ASSIGNMENT is not an argument. The flag never reaches napi here,
    # so the invocation keeps napi's default GitHub-release behaviour and must RED.
    ("CR2: `NOTE=--no-gh-release` before the command does not satisfy V5", "main",
     _OK_MAIN.replace("run: release-plz release",
                      "run: NOTE=--no-gh-release napi prepublish --npm-dir npm"),
     "without --no-gh-release"),
    # ...and the control, so the position check cannot be "fixed" into rejecting every real
    # invocation: the flag AFTER the command is what a correct call looks like.
    ("CR2 control: the flag after the command still passes", "main",
     _OK_MAIN.replace("run: release-plz release",
                      "run: napi prepublish --no-gh-release --npm-dir npm"),
     None),
]


def _critical2_end_to_end() -> str | None:
    """Regression test for Critical 2 (fix round 1): drives main() through a real `./`-prefixed
    local `uses:`, so the `uses.removeprefix("./")` fix is exercised end to end.

    self_test()'s FIXTURES loop calls check_main/check_called DIRECTLY and never goes through
    main() at all — that is exactly how the original `str.lstrip("./")` bug survived undetected;
    a (name, kind, yaml, want) row cannot express this, since it never touches the filesystem or
    main()'s callee-resolution loop. The callee directory name starts with a dot (".wf/") on
    purpose: `lstrip("./")` only mis-strips when a second dot follows the leading "./" — a plain
    "./callee.yml" would not have reproduced the original bug.
    """
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        hidden = tmp_path / ".wf"
        hidden.mkdir()
        (hidden / "callee.yml").write_text(
            "on:\n  workflow_call:\n  pull_request:\n"
            "jobs:\n  build:\n    steps: [{run: twine upload dist/*}]\n"
        )
        main_yaml = _OK_MAIN.replace(
            "steps: [{run: release-plz release}]", "uses: ./.wf/callee.yml"
        )
        (tmp_path / "main.yml").write_text(main_yaml)

        prev_cwd = Path.cwd()
        out_buf, err_buf = io.StringIO(), io.StringIO()
        rc: int
        try:
            os.chdir(tmp_path)
            try:
                with contextlib.redirect_stdout(out_buf), contextlib.redirect_stderr(err_buf):
                    rc = main(["main.yml"])
            except SystemExit as exc:
                rc = exc.code if isinstance(exc.code, int) else 2
        finally:
            os.chdir(prev_cwd)

    out = out_buf.getvalue()
    if rc != 1 or "workflow_call-ONLY" not in out:
        return (f"expected exit 1 with 'workflow_call-ONLY' in output, got exit {rc!r}: "
                f"stdout={out!r} stderr={err_buf.getvalue()!r}")
    return None


def _v8d_pre_approval_callee_publish() -> str | None:
    """Regression test for V8d: check_called's V6 deliberately permits a publish step in a
    workflow_call-ONLY workflow, but that permission is unsafe for a callee invoked from a job
    UPSTREAM of the approval gate. A (name, kind, yaml, want) row cannot express this — it needs
    a two-file tree (a main workflow plus a real local callee) driven through main() end to end,
    the same shape as _critical2_end_to_end above. `build` sits on approve-release's needs: path
    (plan -> build -> approve-release -> release), so replacing its steps with a local `uses:`
    puts the callee squarely upstream of the gate.
    """
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        (tmp_path / "called.yml").write_text(
            "on:\n  workflow_call:\njobs:\n  build:\n    steps: [{run: cargo publish}]\n"
        )
        main_yaml = _OK_MAIN.replace("steps: [{run: echo build}]", "uses: ./called.yml")
        (tmp_path / "main.yml").write_text(main_yaml)

        prev_cwd = Path.cwd()
        out_buf, err_buf = io.StringIO(), io.StringIO()
        rc: int
        try:
            os.chdir(tmp_path)
            try:
                with contextlib.redirect_stdout(out_buf), contextlib.redirect_stderr(err_buf):
                    rc = main(["main.yml"])
            except SystemExit as exc:
                rc = exc.code if isinstance(exc.code, int) else 2
        finally:
            os.chdir(prev_cwd)

    out = out_buf.getvalue()
    if rc != 1 or "V8d" not in out:
        return (f"expected exit 1 with 'V8d' in output, got exit {rc!r}: "
                f"stdout={out!r} stderr={err_buf.getvalue()!r}")
    return None


def _minor9_empty_jobs_floor() -> str | None:
    """Regression test for Minor 9: `jobs: {}` must infra (exit 2 via SystemExit), never return
    a false-clean [] having examined zero jobs. Expressed here, not as a FIXTURES row, because a
    FIXTURES row expects check_main to RETURN a list — this scenario must instead raise.

    infra() always prints to stderr before raising — that's the wanted, correct behaviour for
    every OTHER caller, but here it would make a clean `--self-test` run noisy for a check that
    passed, so stderr is captured and only surfaced on failure.
    """
    err_buf = io.StringIO()
    try:
        with contextlib.redirect_stderr(err_buf):
            check_main({"jobs": {}}, "fixture")
    except SystemExit as exc:
        if exc.code == 2:
            return None
        return f"expected SystemExit(2), got SystemExit({exc.code!r}): {err_buf.getvalue()!r}"
    return f"check_main returned normally on an empty jobs mapping instead of infra(2): {err_buf.getvalue()!r}"


def _important5_regressions() -> list[str]:
    """Regression tests for Important 5: a file that IS a readable path (`is_file()` True) but
    cannot actually be read must still infra (exit 2), never surface an unhandled traceback that
    `main()`'s caller would otherwise see as a bare nonzero exit indistinguishable from exit 1.
    Stderr is captured per call for the same reason as `_minor9_empty_jobs_floor`."""
    errs: list[str] = []
    with tempfile.TemporaryDirectory() as tmp:
        p1 = Path(tmp) / "noperm.yml"
        p1.write_text("on: push\njobs:\n  a:\n    steps: [{run: echo hi}]\n")
        p1.chmod(0o000)
        try:
            err_buf = io.StringIO()
            try:
                with contextlib.redirect_stderr(err_buf):
                    rc = main([str(p1)])
            except SystemExit as exc:
                rc = exc.code if isinstance(exc.code, int) else 2
            if rc != 2:
                if os.access(p1, os.R_OK):
                    # Running as root (or similar): permission bits don't block the read, so this
                    # scenario cannot be reproduced here. Not a guard defect — note and move on.
                    print("NOTE: mode-000 fixture unreadable-file check skipped — the read "
                          "succeeded anyway (likely running as root/uid 0)", file=sys.stderr)
                else:
                    errs.append(f"unreadable-file (mode 000): expected exit 2, got {rc!r}: "
                                f"{err_buf.getvalue()!r}")
        finally:
            p1.chmod(0o644)

        p2 = Path(tmp) / "badutf8.yml"
        p2.write_bytes(b"on: push\njobs:\n  a:\n    steps: [{run: \xff\xfe bad}]\n")
        err_buf2 = io.StringIO()
        try:
            with contextlib.redirect_stderr(err_buf2):
                rc = main([str(p2)])
        except SystemExit as exc:
            rc = exc.code if isinstance(exc.code, int) else 2
        if rc != 2:
            errs.append(f"invalid UTF-8: expected exit 2, got {rc!r}: {err_buf2.getvalue()!r}")
    return errs


def self_test() -> int:
    rc = 0
    for name, kind, text, want in FIXTURES:
        docs = [d for d in yaml.safe_load_all(text) if d is not None]
        doc = docs[0]
        if not isinstance(doc.get("jobs"), dict):
            print(f"FIXTURE BROKEN '{name}': no jobs mapping", file=sys.stderr)
            rc = 1
            continue
        found = (check_main if kind == "main" else check_called)(doc, "fixture")
        blob = " | ".join(found)
        if want is None and found:
            print(f"FAIL '{name}': expected clean, got: {blob}", file=sys.stderr)
            rc = 1
        elif want is not None and want not in blob:
            print(f"FAIL '{name}': expected a violation containing {want!r}, got: "
                  f"{blob or '(clean)'}", file=sys.stderr)
            rc = 1

    # Regression tests that cannot be expressed as a (name, kind, yaml, want) row: they drive
    # main() end to end (Critical 2), touch the filesystem (Important 5), or must observe an
    # infra() SystemExit rather than a returned violation list (Minor 9).
    for check_name, fn in (
        ("critical-2 uses: ./ prefix resolution", _critical2_end_to_end),
        ("v8d pre-approval callee publish", _v8d_pre_approval_callee_publish),
        ("minor-9 empty jobs: {} floor", _minor9_empty_jobs_floor),
    ):
        err = fn()
        if err:
            print(f"FAIL '{check_name}': {err}", file=sys.stderr)
            rc = 1

    for err in _important5_regressions():
        print(f"FAIL 'important-5 fail-closed unreadable file': {err}", file=sys.stderr)
        rc = 1

    return rc


def main(argv: list[str]) -> int:
    if argv == ["--fixture-count"]:
        print(len(FIXTURES))
        return 0
    if argv == ["--self-test"]:
        return self_test()
    if not argv:
        infra("usage: release_guard.py <workflow.yml> [...] | --self-test | --fixture-count")

    violations: list[str] = []
    main_path = Path(argv[0])
    main_doc = load_workflow(main_path)
    violations += check_main(main_doc, main_path.name)

    # Follow local reusable-workflow calls out of the MAIN workflow only (one level; a called
    # workflow cannot itself call another local one in this repo, and V6 keeps the callees honest).
    for job in main_doc["jobs"].values():
        uses = str(job.get("uses") or "") if isinstance(job, dict) else ""
        if uses.startswith("./"):
            # Fix round 1, Critical 2: `str.lstrip` strips a CHARACTER SET, not a prefix.
            # "./.github/workflows/wheels.yml".lstrip("./") == "github/workflows/wheels.yml" —
            # it keeps eating '.' and '/' past the intended two-character prefix, so V6 could
            # never run against any callee path with a leading dot in its own directory name.
            # `removeprefix` strips exactly the literal "./" and nothing more.
            p = Path(uses.removeprefix("./"))
            violations += check_called(load_workflow(p), p.name)

    # V8d: a local callee invoked from a job UPSTREAM of the approval gate must never publish,
    # even though V6 permits a publish step in a workflow_call-ONLY workflow generally.
    for p in pre_approval_callees(main_doc):
        violations += check_called_pre_approval(load_workflow(p), p.name)

    for v in violations:
        print(v)
    return 1 if violations else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
