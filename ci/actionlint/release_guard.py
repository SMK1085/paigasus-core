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

# V9. The plan job decides whether a release happens at all, and it sits upstream of the approval
# gate — so a wrong polarity here fails GREEN, silently dropping every release. The producer side
# is covered by ci/release-plan's fixture table; this pins the WIRING, which no fixture can reach.
PLAN_JOB = "plan"
PLAN_OUTPUT = "nothing_to_release"
PLAN_SCRIPT = "ci/release-plan/run.sh"
# Literal pinning, exactly as V2 pins GATE_EXPR, and for the same reason: a structural test would
# admit `== 'false'`, which is NOT equivalent — it fails closed on an unset output.
PLAN_GATE_EXPR = f"needs.{PLAN_JOB}.outputs.{PLAN_OUTPUT} != 'true'"
ACCEPTED_PLAN_FORMS = frozenset({PLAN_GATE_EXPR, "${{ " + PLAN_GATE_EXPR + " }}"})
_PLAN_STEP_RE = re.compile(r"steps\.([A-Za-z0-9_-]+)\.outputs\." + re.escape(PLAN_OUTPUT))

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


# V8 fix round 4, Important 2. The anchor was `\b`, and `-` is a NON-word character, so
# `--dry-run-x` satisfied `--dry-run\b` and read as an effective dry-run flag. Measured on npm
# 11.11.0: `--dry-run-x` warns `Unknown cli config`, sets `dry-run-x`, leaves `dry-run` UNSET,
# and really publishes. The idiom PUBLISH_MARKERS uses for the same job (`release-plz
# release(?![-\w])`) is not enough here: `.` is neither `-` nor a word character, so
# `--dry-run.x` still matched under it (measured EXEMPT). The anchor is `(?!\S)` — "not
# immediately followed by a Python \S character" — which subsumes `(?![-\w])` and also rejects
# `--dry-run.x`. This is a description of what the code does, not a claim that it matches shell
# word-splitting in general: Python's `\S` is Unicode-aware and excludes some whitespace a shell
# does NOT treat as a token separator. KNOWN LIMITATION: `--dry-run` followed by U+00A0 (a
# non-breaking space) satisfies `(?!\S)` here and reads exempt, but bash does not split on NBSP,
# so the real invocation receives one token, `--dry-run` immediately followed by an NBSP
# and more text, that the underlying tool does not recognize as a dry-run flag: it really
# publishes. Not closed here; see the spec's limitations. The `=` form is NOT reachable here:
# the value scan below returns before this regex is ever applied.
_DRY_RUN_FLAG_RE = re.compile(r"--dry-run(?!\S)")

# V8 fix round 4, Important 1. The general "any `=` value is unverified, so fail closed" rule,
# hoisted out of _dry_run_exempts' per-occurrence loop and applied to the WHOLE tail beside the
# negation scan. Inside the loop it was unreachable behind an earlier bare `--dry-run`, because
# the loop returns True on the first occurrence it accepts. Deliberately NOT `--dry-run\s*=`:
# `--dry-run = $DRY_RUN` is a bare flag followed by an unrelated positional argument, which npm
# resolves to dry-run=true (see _dry_run_exempts' docstring), so a space-tolerant regex here
# would produce a false red on a segment that really is dry.
_DRY_RUN_VALUE_RE = re.compile(r"--dry-run=")

# V8 fix round 2, Important 2 (N2). A negating form ANYWHERE after the publish marker's match end
# — not only immediately after the FIRST `--dry-run` token — since every tool this guard has
# measured (npm 11.11.0, clipanion 4.0.0-rc.4) is last-flag-wins: `--dry-run --no-dry-run` and
# `--dry-run --dry-run=false` both really publish, but a per-occurrence scan that returns on the
# first qualifying `--dry-run` never sees the later negation. Covers `--no-dry-run`,
# `--dry-run=false`, `--dry-run false`, `--dry-run=0`, `--dry-run 0` — the space form needs its
# own alternative here because `\s*=\s*` requires a literal `=`. This is deliberately NOT the
# general `=`-value rule (_DRY_RUN_VALUE_RE below): it only recognises the two LITERAL falsey
# values this guard can prove are unsafe. A bare `=` with any OTHER value (a shell variable, a
# `${{ }}` workflow expression, `no`, `off`, ...) is caught by _DRY_RUN_VALUE_RE, which since fix
# round 4 scans the whole tail alongside this one. Before that hoist the two rules were NOT
# interchangeable: the general rule ran per occurrence, so it saw an `=` value only when no
# earlier bare `--dry-run` had already ended the scan.
_DRY_RUN_NEGATION_RE = re.compile(
    r"--no-dry-run\b|--dry-run\s*=\s*(?:false|0)\b|--dry-run\s+(?:false|0)\b"
)


def _dry_run_exempts(segment: str, after: int) -> bool:
    """True if the text of `segment` AT OR AFTER position `after` (the publish marker's own
    match end) carries an EFFECTIVE `--dry-run` that disables the call.

    V8 fix round 1, Important 2. Two measured false-cleans that genuinely publish:
    `FLAGS=--dry-run cargo publish` (a shell ENVIRONMENT ASSIGNMENT — the flag never reaches
    `cargo`, since it sits BEFORE the marker match, not after it) and `npm publish
    --dry-run=false` / `npm publish --dry-run false` (npm honours either as NOT dry). The
    position check mirrors napi_violations' `--no-gh-release` comparison against the match END,
    applied in the opposite direction; the value check additionally refuses to exempt a
    `--dry-run` immediately followed by `=` or a `false`/`0` argument, since this guard does not
    parse arbitrary flag values and a `--dry-run=true` is rare enough that failing closed on any
    `=` form costs nothing real.

    V8 fix round 2, Important 2 (N2). The round-1 shape scanned occurrences in ORDER and RETURNED
    on the first one that qualified as safe — so a LATER negating token, appended after an
    earlier bare `--dry-run`, was never examined. Measured, both real publishes: `npm publish
    --dry-run --no-dry-run` and `npm publish --dry-run --dry-run=false` (npm 11.11.0 and
    clipanion 4.0.0-rc.4 are both last-flag-wins). This does not attempt true last-token-wins
    ordering for three or more flags (e.g. `--dry-run --no-dry-run --dry-run`, where the final
    token really does mean dry); it fails closed (NOT exempt) on any negation present, which is
    the safe direction and not a shape either measurement covered.

    V8 fix round 3, Critical 1. The round-2 rewrite replaced the whole per-occurrence loop —
    including the round-1 `if tail.startswith("="): continue` fail-closed rule — with only the
    negation scan above, which recognises exactly two LITERAL values (`false`/`0`). Every OTHER
    `=` value — a shell variable (`--dry-run=$DRY_RUN`), a `${{ }}` workflow expression
    (`--dry-run=${{ inputs.dry_run }}`), or a string this guard has no reason to trust (`=no`,
    `=off`, a bare `=`) — fell through to the bare positive check and read as EXEMPT, reopening
    the exact hole fix round 1 closed. Restored: refuse to exempt a `--dry-run` followed by `=`
    (any value, not only `false`/`0`) or by a bare `false`/`0` on the space form — mirroring the
    negation regex's space-form check, since `\\s*=\\s*` only matches an `=`, not whitespace.

    V8 fix round 4, Important 1. Round 3 put that restored rule INSIDE the per-occurrence loop,
    and the loop returns True on the FIRST occurrence it accepts — so one bare `--dry-run` token
    in front of the `=` form ended the scan before the rule was ever applied, and the negation
    scan sees only the two literal falsey values. Measured through main(): `npm publish
    --dry-run=$DRY_RUN` red, but `npm publish --dry-run --dry-run=$DRY_RUN` read EXEMPT, and on
    npm 11.11.0 `--dry-run --dry-run=false` resolves dry-run=false and really publishes. The `=`
    rule is therefore now a WHOLE-TAIL scan (_DRY_RUN_VALUE_RE) beside the negation scan, not a
    per-occurrence one, so the loop below no longer tests for `=` at all. Important 2 of the same
    round narrowed the flag anchor; see _DRY_RUN_FLAG_RE.

    V8 fix round 4, Minor 4 — a behaviour change round 3 shipped as a "restore" without saying
    so. Fix round 1 lstripped the tail before testing `startswith("=")`, so it ALSO failed closed
    on the space-separated `npm publish --dry-run = $DRY_RUN`; round 3 did not lstrip, which
    moved that form from red to exempt. The round-3 behaviour is the CORRECT one and is kept:
    measured on npm 11.11.0, `--dry-run = $DRY_RUN` resolves dry-run=true and the `=` is an
    unrelated positional argument, so round 1's red was a false one. That is why
    _DRY_RUN_VALUE_RE is `--dry-run=` and not `--dry-run\\s*=`.
    """
    tail = segment[after:]
    if _DRY_RUN_NEGATION_RE.search(tail):
        return False
    if _DRY_RUN_VALUE_RE.search(tail):
        return False
    for m in _DRY_RUN_FLAG_RE.finditer(tail):
        if re.match(r"\s+(false|0)\b", tail[m.end() :]):
            continue
        return True
    return False


def job_publishes(job: dict) -> bool:
    """V6 detection. Used for called workflows, and (fix round 1, Critical 1) as the shared
    step-level primitive `approval_boundary_violations` and `callee_boundary_violations` both
    build V8 on.

    Fix round 1, Important 3: evaluated per LINE, not per whole `run:` block. A `--dry-run`
    occurrence reaches no registry — `napi prepublish --dry-run --no-gh-release` (prebuild.yml)
    must not trip this, or the gate is unpassable on a correct repository. Checking the whole
    block would let a `--dry-run` anywhere in a multi-line script silence a REAL invocation on
    another line; per-line scoping is the same fix shape as V5's Important 4.

    Fix round 2, Important 1: evaluated per COMMAND SEGMENT (see command_segments), not per whole
    line — `npm publish && echo "not --dry-run"` must still count as registry-reaching, since the
    marker and the `--dry-run` flag are in different chained commands on the same line.

    Fix round 1, Important 2: the `--dry-run` exemption now requires ADJACENCY (see
    _dry_run_exempts) — a bare `"--dry-run" in segment` test accepted a flag that never reached
    the command at all.
    """
    for step in job.get("steps") or []:
        if not isinstance(step, dict):
            continue
        blob = f"{step.get('run', '')}\n{step.get('uses', '')}"
        for line in blob.splitlines():
            for segment in command_segments(line):
                m = _PUBLISH_RE.search(segment)
                if not m:
                    continue
                if _dry_run_exempts(segment, m.end()):
                    continue
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


def _environment_name(job: dict) -> str | None:
    """The environment's NAME — the field GitHub Actions actually gates on via required
    reviewers. `environment:` may be a plain string (the name itself) or a mapping with `name:`
    and `url:`. V8 fix round 1, Minor 4: a mapping with a `url:` but no `name:` (or an empty one)
    is not a named environment at all — GitHub rejects it outright — so it must not satisfy V8a."""
    env = job.get("environment")
    if isinstance(env, str):
        env = env.strip()
        return env or None
    if isinstance(env, dict):
        raw_name = env.get("name")
        if isinstance(raw_name, str) and raw_name.strip():
            return raw_name.strip()
    return None


def approval_boundary_violations(jobs: dict, name: str) -> list[str]:
    """V8a/b/c. The approval gate is the ONE human checkpoint; everything downstream of it is
    irreversible. V8a is the floor (without it the rest of V8 passes vacuously); V8b asserts
    nothing upstream of the gate may publish; V8c asserts every publisher IS downstream of it —
    the direction V8b alone cannot cover, since deleting `approve-release` from a publishing
    job's needs: satisfies V8b trivially (there is nothing left upstream of the gate to check).

    Both V8b and V8c here are STEPS-shaped only (via job_publishes, which reads `steps:`). A
    job-level `uses:` publisher — the shape `wheels` and `prebuild` already use — is invisible to
    job_publishes and is covered separately by callee_boundary_violations (V8d), which needs the
    filesystem this function deliberately does not touch (fix round 1, Critical 1)."""
    out: list[str] = []
    gate = jobs.get(APPROVAL_JOB)
    if not isinstance(gate, dict):
        return [f"{name}: V8a: no job named '{APPROVAL_JOB}' exists. Every other clause of V8 is "
                f"defined relative to it, so without it this verdict would pass vacuously."]
    if not _environment_name(gate):
        out.append(f"{name}: V8a: job '{APPROVAL_JOB}' declares no NAMED environment:. The pause "
                   f"that makes it a gate comes from that named environment's required "
                   f"reviewers; a missing environment:, or one with no name:, is an ordinary job "
                   f"that always succeeds.")

    for jid in sorted(gated_path_jobs(APPROVAL_JOB, jobs)):
        job = jobs.get(jid)
        if not isinstance(job, dict) or not job_publishes(job):
            continue
        if jid == APPROVAL_JOB:
            # Fix round 1, Minor 1: gated_path_jobs(APPROVAL_JOB, jobs) includes APPROVAL_JOB
            # itself (trivially, on its own needs: path), so "runs upstream of 'approve-release'"
            # about approve-release itself read as nonsense. Word this case on its own terms.
            out.append(f"{name}: V8b: job '{APPROVAL_JOB}' IS the approval gate and contains a "
                       f"step that can reach a registry. The gate itself must never publish — "
                       f"move the step to a job downstream of it.")
        else:
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


def plan_contract_violations(jobs: dict, name: str) -> list[str]:
    plan = jobs.get(PLAN_JOB)
    if not isinstance(plan, dict):
        return [f"{name}: V9a: no job named '{PLAN_JOB}' exists. V9 keys on that literal name, so "
                f"without this floor a rename would leave it asserting nothing."]
    out: list[str] = []

    # V9b's subject is DIRECT consumers only — jobs naming PLAN_JOB in their OWN needs:. A
    # transitive reader (a job two hops downstream that references needs.plan.outputs... without
    # PLAN_JOB on its own needs: path) is a different bug, and one actionlint itself already
    # catches: a `needs.X...` expression referencing a job X not in that job's own needs: reds
    # actionlint on its own. Widening V9b to walk transitively would duplicate a check another
    # tool already owns — fix round 1 review, SMA-603.
    consumers = [jid for jid, j in jobs.items()
                 if isinstance(j, dict) and PLAN_JOB in needs_of(j)]
    if not consumers:
        out.append(f"{name}: V9a: no job names '{PLAN_JOB}' in needs:. The decision is computed "
                   f"and then read by nothing.")
    for jid in sorted(consumers):
        if if_text(jobs[jid]) not in ACCEPTED_PLAN_FORMS:
            out.append(f"{name}: V9b: job '{jid}' needs '{PLAN_JOB}' but its if: is "
                       f"{if_text(jobs[jid])!r}, not {PLAN_GATE_EXPR!r}. Only `!=` fails safe: "
                       f"`== 'true'` inverts the decision and `== 'false'` skips on an unset "
                       f"output. A whitespace variant of an accepted form (an extra space, a "
                       f"different quote style) also reds here — literal pinning, exactly as V2 "
                       f"pins GATE_EXPR.")

    # V9c resolves the DECISION STEP: outputs.nothing_to_release must reference a
    # steps.<id>.outputs... expression naming a step that actually exists in this job. `decision`
    # is set only when every part of that chain resolves — used below by V9d, which must be
    # scoped to THIS step alone (fix round 1, I1: see the comment above V9d).
    outs = plan.get("outputs")
    expr = outs.get(PLAN_OUTPUT) if isinstance(outs, dict) else None
    decision: dict | None = None
    if not isinstance(expr, str):
        out.append(f"{name}: V9c: job '{PLAN_JOB}' declares no outputs.{PLAN_OUTPUT}. A STEP "
                   f"output is not a JOB output, so every consumer would read the empty string.")
    else:
        m = _PLAN_STEP_RE.search(expr)
        if not m:
            out.append(f"{name}: V9c: outputs.{PLAN_OUTPUT} is {expr!r}, which names no "
                       f"steps.<id>.outputs.{PLAN_OUTPUT}.")
        else:
            steps_by_id = {s.get("id"): s for s in (plan.get("steps") or []) if isinstance(s, dict)}
            if m.group(1) not in steps_by_id:
                out.append(f"{name}: V9c: outputs.{PLAN_OUTPUT} names step id {m.group(1)!r}, "
                           f"which does not exist in '{PLAN_JOB}'. A typo here yields '' "
                           f"forever, silently.")
            else:
                decision = steps_by_id[m.group(1)]

    # V9d, fix round 1 (I1). This used to join every step's `run:` in the job and search THAT for
    # the checker invocation — decoupled from V9c, which resolves a specific step id. Three
    # measured fail-green shapes: (1) the decision step runs an inline echo while an unrelated
    # LATER step happens to invoke the script with an unrelated flag; (2) outputs maps to a
    # DIFFERENT step id that hardcodes the answer, while the decision-looking step still runs the
    # script; (3) the decision step is a `uses:` step with no `run:` at all, and the script runs
    # in some other, anonymous step. All three silently drop every release with no red anywhere —
    # exactly the class this verdict's own message claims to prevent. Scoping to the step V9c
    # just resolved closes all three; when V9c could not resolve a decision step at all, there is
    # nothing here to scope to and that failure is already reported by V9c.
    if decision is not None:
        run_text = str(decision.get("run") or "")
        if PLAN_SCRIPT not in run_text:
            out.append(f"{name}: V9d: step {decision.get('id')!r} in job '{PLAN_JOB}' never "
                       f"invokes {PLAN_SCRIPT}. Without this, V9c passes on an inline `echo "
                       f"{PLAN_OUTPUT}=true`.")
    return out


def check_main(doc: dict, name: str) -> list[str]:
    """V1-V5, V7, V8a-c and V9 over the release workflow. V6 applies to CALLED workflows (see
    check_called) and V8d to every job's local callee (see callee_boundary_violations) — both
    need the filesystem, which this function, driven purely off a parsed doc, deliberately does
    not touch."""
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
    # V9: the plan job's output wiring and fail-safe polarity. Same reason as V8: called once,
    # outside the per-job loop, which the loop's `continue` statements would otherwise skip.
    out += plan_contract_violations(jobs, name)
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


# V8d fix round 1, Important 1. `callee_boundary_violations` below resolves exactly ONE level of
# a LOCAL ("./...") `uses:`. A target it cannot resolve that way — a remote `org/repo/path@ref`
# reference, or a second local hop nested inside an already-resolved callee — is reported as
# unverifiable rather than silently treated as clean, fail-closed. An entry here is a reviewed
# exception to that rule; state the reason in a comment beside it. Empty today.
#
# The key for a REMOTE or missing-on-disk target is that `uses:` string verbatim. The key for a
# NESTED (second-level) target is the composite `"{outer uses:} -> {inner uses:}"` form shown in
# the violation message — copy it EXACTLY from that message, not just the inner `uses:` value, or
# the entry will never match.
CALLEE_VERIFICATION_ALLOWLIST: frozenset[str] = frozenset()


def callee_boundary_violations(jobs: dict, name: str) -> list[str]:
    """V8d. `job_publishes` (and therefore V8b/V8c above) reads a job's `steps:` only, so a job
    whose entire body IS a `uses: ./callee.yml` — the exact shape `wheels` and `prebuild` already
    use — is invisible to them. For every job carrying a local `uses:`, resolve the callee and
    apply the SAME boundary a direct publisher must: if anything in the callee can reach a
    registry, the calling job itself must have `APPROVAL_JOB` on its own needs: path.

    Fix round 1, Critical 1. This ONE rule replaces the original V8c/V8d split, and closes the
    critical hole that split left open: a job neither on `APPROVAL_JOB`'s own needs: path (what
    the old V8d walked) NOR carrying a `steps:`-shaped publish (what V8c's job_publishes call
    could see) evaded V8 completely — e.g. a `sneak` job hanging off `build`, gated on V1, never
    needed BY `approve-release` and never needing it either, calling a workflow_call-only local
    callee that runs `cargo publish`. Scanning EVERY job's own `uses:` here, not only ones already
    known to sit on the gate's path, closes that. It also subsumes the pre-approval-only case: a
    caller upstream of the gate can never have the gate on its own needs: path (needs: walks
    upstream, never down), so it still reds under this one rule.

    Fail-closed (Important 1): see CALLEE_VERIFICATION_ALLOWLIST above.
    """
    out: list[str] = []
    seen: set[tuple[str, str]] = set()

    def _unverifiable(jid: str, target: str, reason: str) -> None:
        key = (jid, target)
        if key in seen or target in CALLEE_VERIFICATION_ALLOWLIST:
            return
        seen.add(key)
        out.append(
            f"{name}: V8d: job '{jid}' calls '{target}', {reason} Whether it can reach a "
            f"registry is unknown, so it cannot be proven safe. Add a reviewed "
            f"CALLEE_VERIFICATION_ALLOWLIST entry with a stated reason, or make it verifiable — "
            f"a local './...' workflow, resolved to exactly one level."
        )

    for jid, job in jobs.items():
        if not isinstance(job, dict):
            continue
        uses = str(job.get("uses") or "")
        if not uses:
            continue
        if not uses.startswith("./"):
            _unverifiable(jid, uses, "a reusable workflow this guard cannot read.")
            continue
        local_path = Path(uses.removeprefix("./"))
        if not local_path.is_file():
            _unverifiable(jid, uses, "a local workflow that does not exist on disk.")
            continue

        callee_jobs = load_workflow(local_path).get("jobs") or {}
        publishes = False
        for cjid, cjob in callee_jobs.items():
            if not isinstance(cjob, dict):
                continue
            if job_publishes(cjob):
                publishes = True
            nested = str(cjob.get("uses") or "")
            if nested:
                _unverifiable(
                    jid, f"{uses} -> {nested}",
                    f"whose own job '{cjid}' calls a second workflow this guard resolves only "
                    f"one level past — never two."
                )
        if not publishes:
            continue
        if jid == APPROVAL_JOB:
            # Fix round 2, Minor 4 (N4): gated_path_jobs(APPROVAL_JOB, jobs) trivially contains
            # APPROVAL_JOB itself, so without this case the gate calling a publishing callee read
            # as clean — symmetric with the M1 case already handled in
            # approval_boundary_violations' V8b loop above.
            out.append(f"{name}: V8d: job '{APPROVAL_JOB}' IS the approval gate and calls a "
                       f"local workflow '{uses}' that can reach a registry. The gate itself must "
                       f"never publish — move the call to a job downstream of it.")
        elif APPROVAL_JOB not in gated_path_jobs(jid, jobs):
            out.append(f"{name}: V8d: job '{jid}' calls local workflow '{uses}', which can reach "
                       f"a registry, but '{APPROVAL_JOB}' is not on its needs: path. It would "
                       f"publish without passing the gate.")
    return out


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
  wheels:
    needs: [plan]
    if: ${{ needs.plan.outputs.nothing_to_release != 'true' }}
    runs-on: ubuntu-latest
    steps: [{run: echo wheels}]
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
    # Both rows below now name an explicit `1` count: since V9's `wheels` consumer (added below)
    # shares the identical "    needs: [plan]" text with `build`, an unbounded .replace() would
    # rewrite BOTH jobs' needs: here, which is not what either row is testing. `1` pins these back
    # to their original, single-consumer meaning — `build` is always the first occurrence.
    ("needs: as a SCALAR string still walks", "main",
     _OK_MAIN.replace("    needs: [plan]", "    needs: plan", 1), None),
    ("needs: scalar pointing at an ungated job reds", "main",
     _OK_MAIN.replace("    needs: [plan]", "    needs: release-pr", 1), "is not gated"),
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

    # --- V8 fix round 1 additions (Important 2, Minor 1, Minor 4) --------------------------
    # Important 2: the --dry-run exemption needs adjacency, not a bare substring test. Both
    # measured false-cleans genuinely publish.
    ("V8 fix1 Important 2: FLAGS=--dry-run is a shell assignment, never reaches cargo", "main",
     _OK_MAIN.replace("steps: [{run: echo build}]",
                      "steps: [{run: FLAGS=--dry-run cargo publish}]"), "V8b"),
    ("V8 fix1 Important 2: npm publish --dry-run=false still publishes for real", "main",
     _OK_MAIN.replace("steps: [{run: echo build}]",
                      "steps: [{run: npm publish --dry-run=false}]"), "V8b"),
    # Minor 1: the V8b message for a publish step inside approve-release ITSELF used to read
    # "runs upstream of 'approve-release'" about approve-release, which is nonsense.
    ("V8 fix1 Minor 1: a publish step inside approve-release itself gets its own wording", "main",
     _OK_MAIN.replace("steps: [{run: echo approved}]", "steps: [{run: cargo publish}]"),
     "IS the approval gate"),
    # Minor 4: an environment: mapping with a url: but no name: is not a NAMED environment —
    # GitHub rejects it outright, so it must not satisfy V8a either.
    ("V8 fix1 Minor 4: environment: {url: ...} with no name: is not a named gate", "main",
     _OK_MAIN.replace("environment: release-approval", "environment: {url: 'https://x'}"), "V8a"),
    ("V8 fix1 Minor 4 CONTROL: environment: {name: ..., url: ...} is accepted", "main",
     _OK_MAIN.replace("environment: release-approval",
                      "environment: {name: release-approval, url: 'https://x'}"), None),

    # --- V8 fix round 2 additions (N1, N2) ---------------------------------------------------
    # N1: the SPACE-separated form of the false/0 negation had no dedicated fixture. Measured
    # against real npm 11.11.0: `npm publish --dry-run false` resolves dry-run=false and really
    # publishes.
    ("V8 fix2 N1: npm publish --dry-run false (space form) still publishes for real", "main",
     _OK_MAIN.replace("steps: [{run: echo build}]",
                      "steps: [{run: npm publish --dry-run false}]"), "V8b"),
    # N2: a LATER negating token was never seen — the old code exempted on the FIRST qualifying
    # `--dry-run` and stopped scanning. Both measured against real npm 11.11.0 / clipanion
    # 4.0.0-rc.4 (both last-flag-wins): the tool really publishes in both shapes.
    ("V8 fix2 N2: a later --no-dry-run overrides an earlier --dry-run", "main",
     _OK_MAIN.replace("steps: [{run: echo build}]",
                      "steps: [{run: npm publish --dry-run --no-dry-run}]"), "V8b"),
    ("V8 fix2 N2: a later --dry-run=false overrides an earlier --dry-run", "main",
     _OK_MAIN.replace("steps: [{run: echo build}]",
                      "steps: [{run: npm publish --dry-run --dry-run=false}]"), "V8b"),

    # --- V8 fix round 3 addition (Critical 1) ------------------------------------------------
    # The round-2 rewrite deleted the general `if tail.startswith("="): continue` fail-closed
    # rule along with the per-occurrence loop it lived in, and nothing pinned that rule — the
    # only existing `=`-form row (fix round 1's) uses the literal value `false`, which the
    # negation regex still happens to cover on its own. A NON-LITERAL `=` value — a shell
    # variable, or, worse, a `${{ }}` workflow expression this guard cannot evaluate at scan
    # time — must still fail closed. Without this row, deleting the restored `=`-form check goes
    # undetected exactly as it did the first time.
    ("V8 fix3 C1: --dry-run=$DRY_RUN (a non-literal value) must fail closed, not read exempt",
     "main",
     _OK_MAIN.replace("steps: [{run: echo build}]",
                      "steps: [{run: npm publish --dry-run=$DRY_RUN}]"), "V8b"),

    # --- V8 fix round 4 additions (Important 1, Important 2) ---------------------------------
    # Important 1. The round-3 rule above lived INSIDE the per-occurrence loop, and that loop
    # returns on the FIRST occurrence it accepts. So one bare `--dry-run` token in front of the
    # `=` form made the guard stop scanning before it ever reached the `=` form, and the
    # negation scan sees only the two literal falsey values. Measured on npm 11.11.0:
    # `--dry-run --dry-run=false` resolves dry-run=false, so a `--dry-run=<value>` this guard
    # cannot evaluate must fail closed even when a bare `--dry-run` precedes it. Without this
    # row, hoisting the `=` rule back into the loop goes undetected.
    ("V8 fix4 I1: a bare --dry-run in front of --dry-run=$DRY_RUN must not exempt", "main",
     _OK_MAIN.replace("steps: [{run: echo build}]",
                      "steps: [{run: npm publish --dry-run --dry-run=$DRY_RUN}]"), "V8b"),
    # Important 2. The flag regex used `--dry-run\b`, and `-` is a non-word character, so a
    # SUFFIXED flag satisfied it. Measured on npm 11.11.0: `--dry-run-x` warns `Unknown cli
    # config`, sets `dry-run-x`, leaves `dry-run` UNSET, and really publishes. Nothing pinned the
    # anchor: removing it outright survived the whole self-test.
    ("V8 fix4 I2: --dry-run-x is a different flag and does not exempt", "main",
     _OK_MAIN.replace("steps: [{run: echo build}]",
                      "steps: [{run: npm publish --dry-run-x}]"), "V8b"),

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

    # --- V9 additions (the plan job's output wiring and fail-safe polarity) ----------------
    # `_OK_MAIN` now carries TWO consumers of `plan` — `build` (bare `if:` form) and `wheels`
    # (wrapped `${{ }}` form, added deliberately to disambiguate: it keeps every row below from
    # accidentally mutating both consumers at once via a shared anchor string). Real release.yml
    # has three (wheels, prebuild, proto-dist); two is enough to prove "EVERY consumer", not "SOME
    # consumer".
    ("V9a: the plan job renamed out from under V9", "main",
     _OK_MAIN.replace("  plan:\n", "  planning:\n")
             .replace("needs: [plan]", "needs: [planning]")
             .replace("needs.plan.outputs", "needs.planning.outputs"), "V9a"),
    # Fix round 1, I2. V9a is a conjunction — `plan` exists AND at least one job names it in
    # needs: — and only the first conjunct had a row. `plan` stays present and correctly wired
    # here; both consumers are re-gated DIRECTLY on the release flag instead of via needs: [plan],
    # so V1 stays satisfied and this is the ONLY thing that reds.
    ("V9a: plan exists but nothing reads it (every direct consumer re-gated on the flag instead)",
     "main",
     _OK_MAIN.replace(
         "    needs: [plan]\n    if: needs.plan.outputs.nothing_to_release != 'true'\n",
         "    if: vars.PAIGASUS_RELEASE_ENABLED == 'true'\n")
             .replace(
         "    needs: [plan]\n    if: ${{ needs.plan.outputs.nothing_to_release != 'true' }}\n",
         "    if: vars.PAIGASUS_RELEASE_ENABLED == 'true'\n"),
     "V9a: no job names 'plan' in needs:"),
    ("V9b: an INVERTED consumer condition", "main",
     _OK_MAIN.replace("if: needs.plan.outputs.nothing_to_release != 'true'",
                      "if: needs.plan.outputs.nothing_to_release == 'true'"), "V9b"),
    ("V9b: == 'false' fails closed and is not accepted", "main",
     _OK_MAIN.replace("if: needs.plan.outputs.nothing_to_release != 'true'",
                      "if: needs.plan.outputs.nothing_to_release == 'false'"), "V9b"),
    ("V9b: a consumer with no if: at all", "main",
     _OK_MAIN.replace("    if: needs.plan.outputs.nothing_to_release != 'true'\n", ""), "V9b"),
    ("V9b CONTROL: the ${{ }} wrapping is accepted", "main",
     _OK_MAIN.replace("if: needs.plan.outputs.nothing_to_release != 'true'",
                      "if: ${{ needs.plan.outputs.nothing_to_release != 'true' }}"), None),
    # This is the required addition: `build`'s bare-form `if:` is untouched here, and only
    # `wheels`'s wrapped-form `if:` is inverted (the anchor below matches wheels's exact literal
    # text and nothing else in `_OK_MAIN`). A `plan_contract_violations` that stops at the FIRST
    # bad consumer, rather than checking EVERY one, would pass this row for the wrong reason
    # (build's `if:` alone is fine) — it must still red, naming 'wheels'.
    ("V9b: EVERY consumer must be checked, not just one — wheels alone goes bad", "main",
     _OK_MAIN.replace("if: ${{ needs.plan.outputs.nothing_to_release != 'true' }}",
                      "if: ${{ needs.plan.outputs.nothing_to_release == 'true' }}"), "V9b"),
    ("V9c: the outputs mapping is missing", "main",
     _OK_MAIN.replace("    outputs:\n      nothing_to_release: "
                      "${{ steps.decide.outputs.nothing_to_release }}\n", ""), "V9c"),
    ("V9c: the mapping names a step id that does not exist", "main",
     _OK_MAIN.replace("${{ steps.decide.outputs.nothing_to_release }}",
                      "${{ steps.decdie.outputs.nothing_to_release }}"), "V9c"),
    # Fix round 1, I3. The "names no steps.<id>" branch — the one that catches a HARDCODED
    # `outputs:` mapping, the most direct drop-every-release wiring there is — had no dedicated
    # row. `plan` itself is otherwise untouched (still gated, `decide` still runs the real
    # checker); only the mapping value is replaced with a literal.
    ("V9c: outputs.nothing_to_release is a hardcoded literal, not a steps.<id> reference", "main",
     _OK_MAIN.replace(
         "nothing_to_release: ${{ steps.decide.outputs.nothing_to_release }}",
         "nothing_to_release: 'false'"),
     "which names no steps"),
    ("V9d: the decision step no longer invokes the checker", "main",
     _OK_MAIN.replace("run: ci/release-plan/run.sh --github-output",
                      "run: echo nothing_to_release=true >> \"$GITHUB_OUTPUT\""), "V9d"),
    # Fix round 1, I1. V9d used to search the WHOLE job's `run:` text, not just the DECISION
    # step (the one V9c resolves via outputs.nothing_to_release's steps.<id> reference). This row
    # is the measured fail-green: `decide` runs an inline echo (never touches the real checker),
    # and an unrelated LATER step happens to invoke the script with an unrelated flag. A
    # whole-job scan finds the script text somewhere in the job and reports clean; scoped to the
    # decision step alone, it must still red.
    ("V9d fix1: decide runs an inline echo while an unrelated step invokes the checker", "main",
     _OK_MAIN.replace(
         "      - id: decide\n        run: ci/release-plan/run.sh --github-output\n",
         "      - id: decide\n"
         "        run: echo \"nothing_to_release=true\" >> \"$GITHUB_OUTPUT\"\n"
         "      - run: ci/release-plan/run.sh --help\n"),
     "V9d"),
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


def _run_main_in_tempdir(files: dict[str, str], entry: str = "main.yml") -> tuple[int, str, str]:
    """Write `files` (relative-path -> content) into a fresh tempdir, chdir into it, drive
    main([entry]) end to end, and return (rc, stdout, stderr). Shared by every V8d helper below —
    all of them need main()'s filesystem-resolving callee walk, which check_main/check_called
    (and therefore a plain FIXTURES row) never touch."""
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        for rel, content in files.items():
            (tmp_path / rel).write_text(content)
        prev_cwd = Path.cwd()
        out_buf, err_buf = io.StringIO(), io.StringIO()
        rc: int
        try:
            os.chdir(tmp_path)
            try:
                with contextlib.redirect_stdout(out_buf), contextlib.redirect_stderr(err_buf):
                    rc = main([entry])
            except SystemExit as exc:
                rc = exc.code if isinstance(exc.code, int) else 2
        finally:
            os.chdir(prev_cwd)
    return rc, out_buf.getvalue(), err_buf.getvalue()


_PUBLISHING_CALLEE = "on:\n  workflow_call:\njobs:\n  build:\n    steps: [{run: cargo publish}]\n"


def _v8d_pre_approval_callee_publish() -> str | None:
    """Regression test for V8d (retargeted, fix round 1, Critical 1): check_called's V6
    deliberately permits a publish step in a workflow_call-ONLY workflow, but that permission is
    unsafe for a callee invoked from a job that never passes the approval gate. A
    (name, kind, yaml, want) row cannot express this — it needs a two-file tree (a main workflow
    plus a real local callee) driven through main() end to end, the same shape as
    _critical2_end_to_end above.

    Proves BOTH directions, per the fix-round-1 ruling: without the CLEAN direction, the merged
    callee_boundary_violations rule could be wired to always-red and no fixture would notice.
    """
    # Direction 1: `build` sits on approve-release's needs: path (plan -> build ->
    # approve-release -> release), so replacing its steps with a local `uses:` puts the callee
    # squarely upstream of the gate. Must RED.
    pre_yaml = _OK_MAIN.replace("steps: [{run: echo build}]", "uses: ./called.yml")
    rc, out, err = _run_main_in_tempdir({"called.yml": _PUBLISHING_CALLEE, "main.yml": pre_yaml})
    if rc != 1 or "V8d" not in out:
        return (f"pre-approval caller: expected exit 1 with 'V8d' in output, got exit {rc!r}: "
                f"stdout={out!r} stderr={err!r}")

    # Direction 2: a new job whose OWN needs: chain includes approve-release calls the same
    # publishing callee. It IS downstream of the gate, so it must stay CLEAN.
    post_yaml = _OK_MAIN.replace(
        "    steps: [{run: release-plz release}]\n",
        "    steps: [{run: release-plz release}]\n"
        "  post-publish:\n"
        "    needs: [approve-release]\n"
        "    uses: ./called.yml\n",
    )
    rc2, out2, err2 = _run_main_in_tempdir({"called.yml": _PUBLISHING_CALLEE, "main.yml": post_yaml})
    if rc2 != 0 or out2:
        return (f"post-approval caller: expected exit 0 with no output, got exit {rc2!r}: "
                f"stdout={out2!r} stderr={err2!r}")
    return None


def _v8d_sneak_shape() -> str | None:
    """Regression test for C1 (fix round 1, Critical 1) — the exact CRITICAL hole reported
    against the original brief. A job neither on `APPROVAL_JOB`'s own needs: path NOR itself
    needed by it — `sneak`, hanging off `build` but never merging back into the approval-gated
    chain — evaded V8 entirely under the original split: V8b only walked
    `gated_path_jobs(APPROVAL_JOB, jobs)` (the gate's OWN needs: path, which a sibling branch is
    never on), and V8c/V6 never looked at a job-level `uses:` at all. The merged
    callee_boundary_violations rule catches it because it scans EVERY job's own `uses:`, not only
    ones already known to sit on the gate's path.
    """
    main_yaml = _OK_MAIN.replace(
        "  approve-release:\n",
        "  sneak:\n    needs: [build]\n    uses: ./pub.yml\n"
        "  approve-release:\n",
    )
    rc, out, err = _run_main_in_tempdir({"pub.yml": _PUBLISHING_CALLEE, "main.yml": main_yaml})
    if rc != 1 or "V8d" not in out or "'sneak'" not in out:
        return (f"expected exit 1 with a 'sneak'-naming V8d violation, got exit {rc!r}: "
                f"stdout={out!r} stderr={err!r}")
    return None


def _v8d_unverifiable_remote_uses() -> str | None:
    """Regression test for I1 (fix round 1, Important 1): a job-level `uses:` this guard cannot
    read at all — a remote `org/repo/path@ref` reference — must be reported as an unverifiable
    violation, never silently treated as clean. Fail-closed, not a resolver: this guard has no
    network access and must never assume a remote workflow's shape.
    """
    main_yaml = _OK_MAIN.replace(
        "steps: [{run: echo build}]", "uses: acme/evil/.github/workflows/pub.yml@main"
    )
    rc, out, err = _run_main_in_tempdir({"main.yml": main_yaml})
    if rc != 1 or "V8d" not in out or "cannot be proven safe" not in out:
        return (f"expected exit 1 with an unverifiable-remote V8d violation, got exit {rc!r}: "
                f"stdout={out!r} stderr={err!r}")
    return None


def _v8d_unverifiable_nested_local_callee() -> str | None:
    """Regression test for I1 (fix round 1, Important 1): callee_boundary_violations resolves
    exactly ONE level of a LOCAL './' `uses:`. A second local hop nested inside an
    already-resolved callee — main.yml -> ./a.yml -> ./b.yml, where the SECOND level publishes —
    must be reported unverifiable, not silently treated as clean because the FIRST-level callee
    itself contains no publish step. This is closed by DETECTION, not by building a resolver: the
    guard never loads b.yml at all.
    """
    files = {
        "a.yml": "on:\n  workflow_call:\njobs:\n  inner:\n    uses: ./b.yml\n",
        "b.yml": "on:\n  workflow_call:\njobs:\n  deep:\n    steps: [{run: cargo publish}]\n",
        "main.yml": _OK_MAIN.replace("steps: [{run: echo build}]", "uses: ./a.yml"),
    }
    rc, out, err = _run_main_in_tempdir(files)
    if rc != 1 or "V8d" not in out or "resolves only one level" not in out:
        return (f"expected exit 1 with an unverifiable-nested V8d violation, got exit {rc!r}: "
                f"stdout={out!r} stderr={err!r}")
    return None


def _v8d_dedup_shared_callee() -> str | None:
    """Regression test for M2 (fix round 1, Minor 2): two different callers pointing at the SAME
    local callee must not emit the identical line twice. The old `pre_approval_callees` returned
    a flat, unattributed list of callee paths, so two callers of one target callee doubled the
    identical line; callee_boundary_violations instead names the CALLING job in every message, so
    two distinct callers naturally produce two DISTINCT lines rather than one repeated one.
    """
    main_yaml = (
        "on:\n  push:\n    branches: [main]\n"
        "jobs:\n"
        "  caller-a:\n"
        "    if: vars.PAIGASUS_RELEASE_ENABLED == 'true'\n"
        "    uses: ./shared.yml\n"
        "  caller-b:\n"
        "    if: vars.PAIGASUS_RELEASE_ENABLED == 'true'\n"
        "    uses: ./shared.yml\n"
    )
    rc, out, err = _run_main_in_tempdir({"shared.yml": _PUBLISHING_CALLEE, "main.yml": main_yaml})
    lines = [ln for ln in out.splitlines() if ln.strip()]
    a_lines = [ln for ln in lines if "'caller-a'" in ln]
    b_lines = [ln for ln in lines if "'caller-b'" in ln]
    if rc != 1 or len(a_lines) != 1 or len(b_lines) != 1 or a_lines[0] == b_lines[0]:
        return (f"expected exactly one distinct V8d line per caller, got exit {rc!r}: "
                f"stdout={out!r} stderr={err!r}")
    return None


def _v8d_dedup_shared_nested_target() -> str | None:
    """Regression test for the `seen` de-duplication set inside callee_boundary_violations (fix
    round 2, Minor 6). `_v8d_dedup_shared_callee` above proves that naming the CALLING job
    removes a duplicate line across two DIFFERENT calling jobs — that is what actually does the
    work in that case, not `seen`. `seen` earns its keep on a narrower shape: TWO jobs INSIDE THE
    SAME resolved callee ('a.yml') that both nest the IDENTICAL unresolvable second-level target
    ('./b.yml'). Without `seen`, the single outer caller ('build') would get the identical
    unverifiable-nested line twice, once per inner job.
    """
    files = {
        "a.yml": (
            "on:\n  workflow_call:\njobs:\n"
            "  inner-1:\n    uses: ./b.yml\n"
            "  inner-2:\n    uses: ./b.yml\n"
        ),
        "main.yml": _OK_MAIN.replace(
            "    runs-on: ubuntu-latest\n    steps: [{run: echo build}]", "    uses: ./a.yml"
        ),
    }
    rc, out, err = _run_main_in_tempdir(files)
    matching = [ln for ln in out.splitlines() if "resolves only one level" in ln]
    if rc != 1 or len(matching) != 1:
        return (f"expected exactly one de-duplicated unverifiable-nested line, got exit {rc!r}, "
                f"{len(matching)} matching line(s): stdout={out!r} stderr={err!r}")
    return None


def _v8d_approval_gate_self_case() -> str | None:
    """Regression test for N4 (fix round 2, Minor 4): a job literally named APPROVAL_JOB, itself
    carrying `uses: ./pub.yml` where the callee publishes, used to read clean —
    gated_path_jobs(APPROVAL_JOB, jobs) trivially contains APPROVAL_JOB, so the old
    `APPROVAL_JOB not in gated_path_jobs(jid, jobs)` check could never fire for jid ==
    APPROVAL_JOB itself. Symmetric with the Minor 1 case already handled in
    approval_boundary_violations' V8b loop.

    Not a live hole against the real repository — the pinned actionlint rejects `environment:` on
    a `uses:` job, so such a job would red V8a's syntax instead — but V8d's own safety in this
    corner should not rest on a DIFFERENT gate's check. Driven as a DIRECT call to
    callee_boundary_violations (not through main()), since it only needs a `jobs` dict and the
    filesystem for the one callee file — main()'s own `uses.startswith("./")` V6 loop has nothing
    useful to say about a job named `approve-release` and would only add noise.
    """
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        (tmp_path / "pub.yml").write_text(_PUBLISHING_CALLEE)
        prev_cwd = Path.cwd()
        try:
            os.chdir(tmp_path)
            out = callee_boundary_violations({APPROVAL_JOB: {"uses": "./pub.yml"}}, "fixture")
        finally:
            os.chdir(prev_cwd)
    blob = " | ".join(out)
    if "V8d" not in blob or "IS the approval gate" not in blob:
        return f"expected a self-case V8d violation, got: {blob or '(clean)'}"
    return None


def _v8d_missing_local_callee_direct() -> str | None:
    """Direct-call regression test for the `is_file()` fail-closed arm inside
    callee_boundary_violations (fix round 2, Minor 5). Unreachable through main() end to end:
    main()'s earlier check_called loop already calls load_workflow on every local `./` callee
    first, and load_workflow infra()s at exit 2 on a missing file before
    callee_boundary_violations ever runs at all. Calling callee_boundary_violations directly,
    bypassing that loop, is what actually exercises this arm — chosen over a defence-in-depth
    comment so the arm stays a LIVE, tested rule rather than an assumed one.
    """
    with tempfile.TemporaryDirectory() as tmp:
        prev_cwd = Path.cwd()
        try:
            os.chdir(tmp)
            jobs = {"caller": {"uses": "./does-not-exist.yml"}}
            out = callee_boundary_violations(jobs, "fixture")
        finally:
            os.chdir(prev_cwd)
    blob = " | ".join(out)
    if "V8d" not in blob or "does not exist on disk" not in blob:
        return f"expected a missing-callee V8d violation, got: {blob or '(clean)'}"
    return None


def _v8_fix4_dry_run_boundary_cases() -> str | None:
    """V8 fix round 4. Two properties of _dry_run_exempts that the two new FIXTURES rows do NOT
    pin, kept here rather than as rows so the fixture count stays the reviewed 62. Both were
    MEASURED as mutation survivors before this helper existed: narrowing the flag anchor from
    `(?!\\S)` to PUBLISH_MARKERS' `(?![-\\w])` idiom, and widening the value scan from
    `--dry-run=` to `--dry-run\\s*=`, each left the whole self-test green.

    1. `--dry-run.x` is a DIFFERENT flag, like `--dry-run-x`, and must not exempt. `.` is neither
       `-` nor a word character, so `(?![-\\w])` admits it; only an anchor that ends the token at
       whitespace rejects it.
    2. `--dry-run = $DRY_RUN` IS an effective dry-run and must stay exempt. npm 11.11.0 resolves
       it to dry-run=true, the `=` being an unrelated positional argument. Fix round 1 red it
       (it lstripped before testing for `=`) and round 3 silently stopped doing so; this pins
       the current, correct behaviour so the next reader cannot restore round 1's false red by
       adding `\\s*` to the value scan.
    """
    cases = (
        ("npm publish --dry-run.x", True,
         "a suffixed flag is a different flag and must not exempt"),
        ("npm publish --dry-run = $DRY_RUN", False,
         "a bare flag followed by an unrelated = argument must still exempt"),
    )
    for command, want_publish, why in cases:
        got = job_publishes({"steps": [{"run": command}]})
        if got is not want_publish:
            return (f"job_publishes({command!r}) returned {got}, expected {want_publish}: "
                    f"{why}")
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
        ("v8d callee boundary, both directions", _v8d_pre_approval_callee_publish),
        ("v8d C1 sneak shape (job off the gate's own needs: path)", _v8d_sneak_shape),
        ("v8d I1 unverifiable remote uses:", _v8d_unverifiable_remote_uses),
        ("v8d I1 unverifiable nested local callee", _v8d_unverifiable_nested_local_callee),
        ("v8d M2 no duplicate line for a shared callee", _v8d_dedup_shared_callee),
        ("v8d N6 no duplicate line for a shared nested target", _v8d_dedup_shared_nested_target),
        ("v8d N4 approval gate self-case", _v8d_approval_gate_self_case),
        ("v8d N5 missing local callee (direct call)", _v8d_missing_local_callee_direct),
        ("v8 fix4 dry-run anchor and space-form boundary cases",
         _v8_fix4_dry_run_boundary_cases),
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

    # V8d: every job carrying a `uses:` — not only ones already known to sit on the approval
    # gate's own needs: path — must be checked against the SAME boundary a direct publisher is
    # (fix round 1, Critical 1). Fail-closed on anything it cannot resolve (Important 1).
    violations += callee_boundary_violations(main_doc["jobs"], main_path.name)

    for v in violations:
        print(v)
    return 1 if violations else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
