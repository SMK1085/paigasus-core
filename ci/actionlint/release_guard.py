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
import sys
import tempfile
from pathlib import Path

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
UNGATED_JOBS = frozenset({"release-pr"})

# V3: the real bypass class is any status-check function, not two literal spellings.
# `success() || failure()`, `!failure()` and `${{ ! cancelled() }}` all evade a two-string test.
STATUS_FUNCS = ("always", "cancelled", "success", "failure")

# V6: detection, retained ONLY for called workflows where UNGATED_JOBS has no meaning.
PUBLISH_MARKERS = (
    "release-plz release",
    "npm publish",
    "napi prepublish",
    "twine upload",
    "gh-action-pypi-publish",
    "cargo publish",
)


def infra(msg: str) -> "NoReturn":  # type: ignore[valid-type]
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


def job_publishes(job: dict) -> bool:
    """V6 detection. Used ONLY for called workflows.

    Fix round 1, Important 3: evaluated per LINE, not per whole `run:` block. A `--dry-run`
    occurrence reaches no registry — `napi prepublish --dry-run --no-gh-release` (prebuild.yml)
    must not trip this, or the gate is unpassable on a correct repository. Checking the whole
    block would let a `--dry-run` anywhere in a multi-line script silence a REAL invocation on
    another line; per-line scoping is the same fix shape as V5's Important 4.
    """
    for step in job.get("steps") or []:
        if not isinstance(step, dict):
            continue
        blob = f"{step.get('run', '')}\n{step.get('uses', '')}"
        for line in blob.splitlines():
            if "--dry-run" in line:
                continue
            if any(m in line for m in PUBLISH_MARKERS):
                return True
    return False


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

        # V5: the tagging boundary (spec §2), enforced rather than documented. Fix round 1,
        # Ruling 8: V5 applies to EVERY job, including UNGATED_JOBS members — `napi prepublish`
        # cuts a git tag regardless of whether the job that ran it was gated, and release-plz must
        # own every tag (ADR-0011 S3). So this runs BEFORE the UNGATED_JOBS `continue` below.
        # Fix round 1, Important 4: evaluated per LINE, not per whole `run:` block — a comment or
        # unrelated line mentioning the flag must not satisfy a check over the real invocation.
        for step in job.get("steps") or []:
            if not isinstance(step, dict):
                continue
            run = str(step.get("run") or "")
            for line in run.splitlines():
                if "napi prepublish" in line and "--no-gh-release" not in line:
                    out.append(
                        f"{name}: job '{job_id}' runs `napi prepublish` without --no-gh-release. "
                        f"release-plz owns every tag (ADR-0011 S3); napi must never cut one."
                    )

        if job_id in UNGATED_JOBS:
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
    return out


def check_called(doc: dict, name: str) -> list[str]:
    """V6. A workflow the release path CALLS may publish only if it is workflow_call-ONLY.

    Revision 1 of the spec claimed such a workflow inherits the caller's gate. It does not:
    wheels.yml and prebuild.yml carry their own push: and pull_request: triggers, so a publish
    step added to one would run ungated on every PR while the caller's gate stayed green.
    """
    out: list[str] = []
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
    steps: [{run: echo plan}]
  release:
    needs: [plan]
    runs-on: ubuntu-latest
    steps: [{run: release-plz release}]
"""

FIXTURES: list[tuple[str, str, str, str | None]] = [
    ("healthy control", "main", _OK_MAIN, None),
    ("ungated job", "main", _OK_MAIN.replace("    if: vars.PAIGASUS_RELEASE_ENABLED == 'true'\n", ""),
     "is not gated"),
    ("gate expression weakened to !=", "main",
     _OK_MAIN.replace("== 'true'", "!= 'disabled'"), "is not gated"),
    ("gate expression widened with ||", "main",
     _OK_MAIN.replace("== 'true'", "== 'true' || github.actor == 'x'"), "is not gated"),
    ("wrapped gate form is accepted", "main",
     _OK_MAIN.replace("if: vars.PAIGASUS_RELEASE_ENABLED == 'true'",
                      "if: ${{ vars.PAIGASUS_RELEASE_ENABLED == 'true' }}"), None),
    ("always() on the gated job", "main",
     _OK_MAIN.replace("    needs: [plan]", "    needs: [plan]\n    if: always()"), "status function"),
    ("!cancelled() with spacing", "main",
     _OK_MAIN.replace("    needs: [plan]", "    needs: [plan]\n    if: ${{ ! cancelled() }}"),
     "status function"),
    ("success() || failure()", "main",
     _OK_MAIN.replace("    needs: [plan]", "    needs: [plan]\n    if: success() || failure()"),
     "status function"),
    ("job-level continue-on-error: true", "main",
     _OK_MAIN.replace("    needs: [plan]", "    needs: [plan]\n    continue-on-error: true"),
     "continue-on-error"),
    ("step-level continue-on-error: true", "main",
     _OK_MAIN.replace("steps: [{run: release-plz release}]",
                      "steps: [{run: release-plz release, continue-on-error: true}]"),
     "step with continue-on-error"),
    ("continue-on-error: false (bool) is accepted", "main",
     _OK_MAIN.replace("    needs: [plan]", "    needs: [plan]\n    continue-on-error: false"), None),
    ('continue-on-error: "false" (str) is accepted', "main",
     _OK_MAIN.replace("    needs: [plan]", '    needs: [plan]\n    continue-on-error: "false"'), None),
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
     _OK_MAIN.replace("    needs: [plan]", "    needs: [plan]\n    if: false"), None),
    ("called workflow that is workflow_call-only may publish", "called",
     "on:\n  workflow_call:\njobs:\n  build:\n    steps: [{run: twine upload dist/*}]\n", None),
    ("called workflow with pull_request may NOT publish", "called",
     "on:\n  workflow_call:\n  pull_request:\n    branches:\n      - main\n"
     "jobs:\n  build:\n    steps: [{run: twine upload dist/*}]\n", "workflow_call-ONLY"),
    ("called workflow with no publish step is clean", "called",
     "on:\n  workflow_call:\n  pull_request:\n    branches:\n      - main\n"
     "jobs:\n  build:\n    steps: [{run: maturin build}]\n", None),

    # --- Fix round 1 additions -------------------------------------------------------------
    ("Critical 1: case-insensitive Always() bypass", "main",
     _OK_MAIN.replace("    needs: [plan]", "    needs: [plan]\n    if: Always()"),
     "status function"),
    ("Critical 1: case-insensitive ALWAYS() wrapped form", "main",
     _OK_MAIN.replace("    needs: [plan]", "    needs: [plan]\n    if: ${{ ALWAYS() }}"),
     "status function"),
    ("Critical 1: case-insensitive !Cancelled() bypass", "main",
     _OK_MAIN.replace("    needs: [plan]", "    needs: [plan]\n    if: ${{ !Cancelled() }}"),
     "status function"),
    ("Important 3: napi prepublish --dry-run in a called workflow is not a publish", "called",
     "on:\n  workflow_call:\n  pull_request:\n    branches:\n      - main\n"
     "jobs:\n  build:\n    steps: [{run: pnpm exec napi prepublish --dry-run --no-gh-release "
     "--npm-dir npm}]\n", None),
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

    for v in violations:
        print(v)
    return 1 if violations else 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
