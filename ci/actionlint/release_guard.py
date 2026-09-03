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
    raise SystemExit(2) from None

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
# The flag the decision step must pass. `ci/release-plan/run.sh --self-test` in the decision step
# would satisfy a bare "does it invoke the script" test while writing no output at all.
PLAN_SCRIPT_FLAG = "--github-output"
# Literal pinning, exactly as V2 pins GATE_EXPR, and for the same reason: a structural test would
# admit `== 'false'`, which is NOT equivalent — it fails closed on an unset output.
PLAN_GATE_EXPR = f"needs.{PLAN_JOB}.outputs.{PLAN_OUTPUT} != 'true'"
ACCEPTED_PLAN_FORMS = frozenset({PLAN_GATE_EXPR, "${{ " + PLAN_GATE_EXPR + " }}"})
# SMA-603 fix wave, 2d. FULL-MATCH, not `search`. The old regex only had to occur SOMEWHERE in
# the outputs expression, so any expression that merely CONTAINED the step reference passed —
# including ones that resolve to a constant. The measured shape is
# `${{ steps.decide.outputs.nothing_to_release || 'true' }}`: GitHub's `||` yields its right
# operand when the left is the empty string, so an unset step output becomes the literal 'true'
# and EVERY consumer skips. That inverts the branch's central fail-safe property in one edit and
# reds nothing. Literal pinning, exactly as ACCEPTED_PLAN_FORMS pins the consumer `if:` above:
# the expression must be the reference and nothing else. Surrounding whitespace is tolerated
# because YAML and the `${{ }}` syntax both allow it; anything else is not.
_PLAN_STEP_RE = re.compile(
    r"^\$\{\{\s*steps\.([A-Za-z0-9_-]+)\.outputs\." + re.escape(PLAN_OUTPUT) + r"\s*\}\}$")


# SMA-603 fix wave, 2c. V9d used to be `PLAN_SCRIPT not in run_text` — a raw substring test over
# the decision step's whole `run:` block. A COMMENT naming the script satisfied it, so a step
# whose real command hardcodes the answer read as clean:
#     run: |
#       # ci/release-plan/run.sh --github-output decides this
#       echo "nothing_to_release=true" >> "$GITHUB_OUTPUT"
# This tests the COMMAND WORD instead, per command segment, reusing command_segments (which
# strips a trailing `#` comment from each segment, so a comment leaves an empty segment behind).
# It also demands PLAN_SCRIPT_FLAG in the SAME segment: a step running the script with any other
# flag writes no output.
#
# NOT a shell parser, the same deliberate scope limit command_segments records: leading
# `VAR=value` assignments and one `env`/`bash`/`sh` prefix are skipped, and nothing else is
# modelled. A more exotic invocation (`eval`, a variable holding the path) reports missing, which
# fails CLOSED — the gate reds and a human looks.
_ENV_ASSIGN_RE = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")
_CMD_PREFIXES = ("env", "bash", "sh", "/bin/bash", "/bin/sh", "/usr/bin/env")



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

# V10 (SMA-602): no registry publish credential anywhere in the release path. PyPI and npm
# authenticate through OIDC trusted publishing; crates.io already did. A reintroduced token
# publishes SILENTLY — npm's oidc.js never throws (its own doc comment says so), so a failed
# exchange falls through to whatever credential is configured, and the publish succeeds having
# used the token. Nothing else in this repository catches that: ci/workflow-credentials only
# inspects pull_request-triggered workflows, and release.yml is not one.
#
# This bans PUBLISH credentials BY NAME, never the `secrets` context as a whole.
# PAIGASUS_BOT_APP_ID and PAIGASUS_BOT_PRIVATE_KEY are legitimate and must keep working: an App
# installation token cannot come from a registry trusted publisher. A blanket ban would also red
# ci/workflow-credentials/run.sh's control row, which asserts release.yml still reads A secret.
BANNED_PUBLISH_CREDENTIALS = ("PYPI_API_TOKEN", "NPM_TOKEN", "NODE_AUTH_TOKEN")

# An `_authToken` written into any npmrc masks a broken OIDC exchange: npm's oidc.js sets its
# exchanged token at the 'user' config level, and a file token at that level is what publish.js
# falls back to when the exchange fails.
NPMRC_AUTH_TOKEN = "_authToken"

# Final-review Important 1. `_auth` is the OTHER live npm credential key at that same config
# level: `getCredentialsByURI` honours `//registry/:_auth=<base64 user:pass>` exactly as it
# honours `_authToken`, so it masks a failed exchange the same way. MEASURED as a V10 bypass
# before this rule existed.
#
# The lookahead is load-bearing twice over. `_auth` is a PREFIX of `_authToken`, so a bare
# substring test would report both rules on one string; `(?![A-Za-z0-9_])` keeps the two
# messages disjoint. It also excludes `NODE_AUTH_TOKEN` (`_AUTH_` — an underscore follows),
# which BANNED_PUBLISH_CREDENTIALS already covers by name.
#
# Case-insensitive on purpose: npm reads every npmrc key from the environment too, so
# `NPM_CONFIG__AUTH` is the same credential spelled for a step `env:` block — the fourth
# measured bypass.
#
# SMA-602 fix wave, F6. The LEADING boundary `(?<![A-Za-z0-9])` is as load-bearing as the
# trailing one, and it was missing. MEASURED false positives against the bare `_auth(?![A-Za-z0-9_])`
# form: `GIT_AUTH="x"`, `${{ steps.app_auth.outputs.token }}` and `CRATES_AUTH: 1` all matched,
# and release.yml already carries a near-miss at `AUTH_REMOTE=` that escapes only because no
# underscore precedes it. Each would have red this gate with a message about npm credentials on
# a step touching no npm registry — blocking every PR behind a message naming the wrong
# subsystem. The boundary rejects all three, because in every one the character before `_auth`
# is a LETTER.
#
# It must NOT be `(?<![A-Za-z0-9_])`: an underscore is exactly what precedes the key in the
# environment spelling `NPM_CONFIG__AUTH` (npm's own `NPM_CONFIG_` prefix plus the `_auth` key),
# which is the measured bypass this rule was added for. `:` (the npmrc `//registry/:_auth=`
# form) and start-of-string (a bare `_auth=` line in an npmrc) are both admitted too.
#
# Deliberately NOT tightened further with a trailing `[:=]` requirement: `npm config set _auth
# "$X"` is a real credential write and carries neither.
_NPMRC_AUTH_RE = re.compile(r"(?<![A-Za-z0-9])_auth(?![A-Za-z0-9_])", re.IGNORECASE)

# Final-review Important 1, rule 2. A KEY-based rule, not a value-based one: red a `password:`
# inside the `with:` of any pypa/gh-action-pypi-publish step whatever the value is. The two
# measured bypasses were `${{ secrets.PYPI_PROJECT_TOKEN }}` (a NEW secret name — which is
# exactly what design §9's rollback plan would mint) and `${{ env.PY_CRED }}` (no secret
# reference at all). Neither can be caught by a denylist of names; the presence of the key is
# the violation, because under Trusted Publishing there is nothing legitimate to put there.
PYPI_PUBLISH_ACTION = "pypa/gh-action-pypi-publish"

# Final-review Important 1, rule 1. The secret names `release.yml` may reference, pinned by
# STRICT EQUALITY — the same shape, and for the same reason, as
# ci/workflow-credentials/workflow_credentials.py's EXPECTED_PR_SUBJECTS. A denylist can only
# ever ban names someone already thought of; this reds on EVERY new secret name until a human
# adds it here on purpose.
#
# Both members are GitHub App credentials for the per-run installation token. An App token
# cannot come from a registry trusted publisher, so neither is replaceable by OIDC, and
# ci/workflow-credentials/run.sh's control row asserts release.yml still reads A secret.
#
# IF THIS REDS: re-baseline it DELIBERATELY — add the new name here with a comment saying why
# that secret cannot be an OIDC exchange. Do not loosen the comparison to a subset test, and do
# not delete the pin; either turns a strict gate into a decorative one.
EXPECTED_RELEASE_SECRETS = (
    "PAIGASUS_BOT_APP_ID",
    "PAIGASUS_BOT_PRIVATE_KEY",
)

# Matched against PARSED scalars, never the raw file, so the YAML comments in release.yml that
# NAME the removed tokens stay invisible here (the same property publish_credential_violations
# relies on).
#
# SMA-602 fix wave, F1. The old form was `re.compile(r"secrets\.([A-Za-z_][A-Za-z0-9_]*)")`, one
# spelling of four that GitHub Actions accepts. MEASURED against the real release.yml — each of
# these returned `[]`, so each was a live V10 rule-1 bypass at guard exit 0:
#
#     ${{ secrets['PYPI_PROJECT_TOKEN'] }}     ${{ Secrets.PYPI_PROJECT_TOKEN }}
#     ${{ secrets["PYPI_PROJECT_TOKEN"] }}     ${{ SECRETS.PYPI_PROJECT_TOKEN }}
#
# The machinery below is REUSED from ci/workflow-credentials/workflow_credentials.py rather than
# reinvented weaker: EXPR_SPAN / STRING_LITERAL / SECRETS_CTX are transcribed from that file,
# where all four spellings are already pinned as live fixtures. Every comment there applies here
# — the literal-aware span (a `}}` can sit inside a string literal), the possessive `*+` for
# cost, and the `(?<![\w.-])` boundary that rejects `inputs.secrets-file` and
# `steps.x.outputs.secrets`.
#
# WHY SPAN-SCOPED, not a scan of every scalar. release.yml's own `run:` blocks contain the WORD
# "secrets" in prose (`::notice::… App secrets (see release.yml)`), which the bare context regex
# matches. Scanning only inside `${{ }}` — plus a bare `if:`, which GitHub evaluates as an
# expression without the wrapper — is what keeps that prose invisible.
_EXPR_SPAN = re.compile(r"\$\{\{((?:'[^']*'|\"[^\"]*\"|(?!\}\}).)*+)\}\}", re.S)
_STRING_LITERAL = re.compile(r"'[^']*'|\"[^\"]*\"")
_SECRETS_CTX = re.compile(r"(?<![\w.-])secrets(?![\w-])", re.IGNORECASE)
# The bracket-index forms. The NAME sits INSIDE the literal here, so these must be extracted
# BEFORE literals are stripped — the opposite order from workflow_credentials.py, which only
# needs to know THAT the context was read, never which name.
_SECRET_INDEX_RE = re.compile(
    r"(?<![\w.-])secrets(?![\w-])\s*\[\s*(?:'([^']*)'|\"([^\"]*)\")\s*\]", re.IGNORECASE)
# The property form, applied AFTER literals are stripped — which is what stops
# `hashFiles('secrets.txt')` yielding the name "txt".
_SECRET_DOT_RE = re.compile(
    r"(?<![\w.-])secrets(?![\w-])\s*\.\s*([A-Za-z_][A-Za-z0-9_]*)", re.IGNORECASE)


def secret_refs(text: str, *, bare_expression: bool = False) -> tuple[set[str], bool]:
    """Every secret NAME `text` references, and whether some reference resolved to no name.

    The second element is the FAIL-CLOSED half: `${{ toJSON(secrets) }}` or
    `${{ secrets[format('{0}', x)] }}` reads the context without naming anything a
    strict-equality allowlist can compare. Reporting that as a violation is the only honest
    answer — a name-based pin cannot judge a reference whose name does not exist until run time.

    `bare_expression=True` additionally evaluates the text OUTSIDE any `${{ }}` wrapper. GitHub
    evaluates an `if:` value as an expression with or without the wrapper, so
    `if: secrets.TOKEN != ''` references the context with no span to extract.
    """
    spans = list(_EXPR_SPAN.findall(text))
    if bare_expression:
        # The wrapped part is already in `spans`; blank it out so it is not counted twice.
        spans.append(_EXPR_SPAN.sub(" ", text))

    names: set[str] = set()
    unresolved = False
    for span in spans:
        for m in _SECRET_INDEX_RE.finditer(span):
            names.add(m.group(1) if m.group(1) is not None else m.group(2))
        rest = _STRING_LITERAL.sub("", _SECRET_INDEX_RE.sub(" ", span))
        for m in _SECRET_DOT_RE.finditer(rest):
            names.add(m.group(1))
        if _SECRETS_CTX.search(_SECRET_DOT_RE.sub(" ", rest)):
            unresolved = True
    return names, unresolved

# The one workflow EXPECTED_RELEASE_SECRETS is a pin OF. The "unexpected name" half of the rule
# applies to every workflow check_main sees; the "pinned name went missing" half can only apply
# to the real file, because a synthetic fixture legitimately references no secret at all.
RELEASE_WORKFLOW_NAME = "release.yml"

# V11 (SMA-602 fix wave, F2). V10 bans the OLD mechanism; NOTHING asserted the NEW one is still
# wired. `grep -n "id-token" release_guard.py` matched a docstring and nothing else, so deleting
# `id-token: write` from either publish job — or adding a narrower job-level `permissions:` block
# that simply omits it — left every gate in this repository green.
#
# The run-time consequence is the one this branch exists to prevent. With no OIDC grant the
# runner sets no ACTIONS_ID_TOKEN_REQUEST_* variables; npm's lib/utils/oidc.js returns undefined
# WITHOUT throwing (its own doc comment says so), and, the token now being gone too, the publish
# dies ENEEDAUTH — after crates.io has published and the tags are cut, in the one job a fresh
# dispatch cannot repair.
#
# Scoped to RELEASE_WORKFLOW_NAME. A CALLED workflow legitimately declares no `id-token: write`
# (wheels.yml builds, it does not publish), and `repo:workflow-credentials` actively BANS the
# grant in any pull_request-triggered workflow — so applying this rule file-wide would red a
# correct repository.
OIDC_PUBLISH_JOBS = ("publish-pypi", "publish-npm")
ID_TOKEN_SCOPE = "id-token"

# V12 (SMA-602 fix wave, F3). The npm OIDC floor is duplicated across release.yml's `publish-npm`
# job and prebuild.yml's `assemble` job, and NOTHING cross-pinned the two: `grep -rn '11.5.1' ci/`
# found nothing, so deleting both steps — or lowering only ONE copy — kept `moon ci` fully green.
#
# WHY THESE SIX LINES, each verified to occur EXACTLY ONCE in EACH workflow. They are the
# distinct single-edit bypasses, not one span:
#   1. `node_bin=…proto --reporter text bin node` — what RESOLVES the pinned Node. Drop it and
#      the step falls back to the runner image's npm 10.x, which has no OIDC code path at all.
#   2. `echo "$node_dir" >> "$GITHUB_PATH"` — the PATH move itself. Without it the resolution
#      above is computed and discarded, and `napi prepublish` still spawns the runner's npm.
#   3. `if [ ! -x "$node_dir/npm" ]` — the assertion that the pinned Node ships an npm at all.
#   4. `v="$(npm --version)"` — the measurement the floor is compared against.
#   5. `floor=11.5.1` — the floor VALUE. Lowering one copy is F3's exact reported bypass.
#   6. the unparseable-version arm, which is what makes "cannot parse" fail rather than pass.
#   7. the version comparison itself. Delete it and the step prints OK unconditionally.
#
# Matched as STRIPPED WHOLE LINES against the parsed `run:` scalars, the same rule
# T_CARGO_LOCK_SH_CALL_SITES and RELEASE_PARITY_SH_CALL_SITES use, and for both of their
# reasons: the real lines are indented inside a YAML block scalar, so a column-0 rule would
# reject them, while a substring rule would let a COMMENTED-OUT copy satisfy the pin.
#
# WHY HERE AND NOT ci/affected-graph/ci_targets.py. Same reasoning ci/actionlint/run.sh's check 8f
# records for its own choice: `repo:actionlint` carries `inputs: ['**/*']`, so it is scheduled on
# every PR and this pin needs NO new input registration and no `SELF_TASK_EXPECTED_GLOBS` entry.
# release_guard.py additionally already READS both files — check 10 runs it on release.yml, whose
# `prebuild` job carries `uses: ./.github/workflows/prebuild.yml`, so check_called reaches the
# second copy for free. A pin in ci_targets.py would need a seventh haystack parameter threaded
# through check_self_invocation and its whole self-test matrix, for a strictly weaker guarantee.
NPM_OIDC_FLOOR_SUBJECTS = ("release.yml", "prebuild.yml")
NPM_OIDC_FLOOR_LINES = (
    'node_bin="$(proto --reporter text bin node)"',
    'echo "$node_dir" >> "$GITHUB_PATH"',
    'if [ ! -x "$node_dir/npm" ]; then',
    'v="$(npm --version)"',
    "floor=11.5.1",
    "''|*[!0-9.]*|.*|*.|*..*) below ;;",
    'if [ "$vmaj" -lt "$fmaj" ] \\',
)


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
        return dict.fromkeys(raw)
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


def steps_of(job: dict, where: str) -> list:
    """A job's `steps:`, FAIL-CLOSED on any shape that is not a list (SMA-602 fix wave, F7).

    `for step in job.get("steps") or []` iterates a STRING's CHARACTERS. Each character is not a
    dict, so the `continue` in every step loop fires, and the step `env:`/`run:`/`with:` scans,
    the pypa `password:` rule, the `napi prepublish` rule and the publish DETECTOR all skip
    silently — the job reads clean. MEASURED on `steps: publish`:
    `publish_credential_violations` returned `[]`, `job_publishes` returned False and
    `napi_violations` returned `[]`.

    That is the PyYAML string-iteration pitfall the `needs:` scalar case already documents
    (SMA-579), and it contradicts this file's stated convention: every abnormal condition exits
    2, never a skip and never a pass. `env:`, `container:` and `services:` already fail closed
    the same way in publish_credential_violations.

    `None` (no `steps:` at all) is a legitimate shape — a reusable-workflow-call job carries
    `uses:` instead — so it returns an empty list, not an infra abort.
    """
    raw = job.get("steps")
    if raw is None:
        return []
    if not isinstance(raw, list):
        infra(f"{where} has steps: that is not a list "
              f"(got {type(raw).__name__}: {raw!r}). A non-list steps: makes every step-level "
              f"rule skip in silence, which would read as clean.")
    return raw


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


def job_publishes(job: dict, where: str = "a job") -> bool:
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

    SMA-602 fix wave, F7: `steps:` is read through `steps_of`, which fails closed on a non-list
    shape rather than iterating a string's characters and reporting False.
    """
    for step in steps_of(job, where):
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

    SMA-602 fix wave, F7: `steps:` is read through `steps_of`, which fails closed on a non-list
    shape — a `steps: publish` string used to make this rule skip in silence.
    """
    out: list[str] = []
    for step in steps_of(job, f"{name}: job '{job_id}'"):
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


def publish_credential_violations(job: dict, job_id: str, name: str) -> list[str]:
    """V10: no registry publish credential anywhere in the release path.

    Invoked from BOTH check_main and check_called. Scoping it to check_main would repeat the
    SMA-579 V5 mistake, where a check living only in check_main left every CALLED workflow
    unguarded — main() runs check_main on argv[0] alone.

    Scans job env, job container:/services: env (fix round 1, Minor 4), job-level secrets:/with:
    (fix round 1, Minor 5 — the reusable-workflow-call shape, where a credential travels via the
    job itself rather than a step), step env, step run: bodies and step with: blocks. YAML
    comments are not in the parsed doc, so the explanatory comments in release.yml that NAME
    these tokens are invisible here — which is what lets those comments keep explaining the
    history.

    Callers may pass a SYNTHETIC job carrying only an `env` key — check_main/check_called each do
    this once, before their per-job loop, to scan the WORKFLOW-level `env:` block (fix round 1,
    Important 3): that scope is reachable from every step via the `secrets` context, so a
    credential lifted from a step env: to the workflow root would otherwise pass this check clean.

    Fail-closed (fix round 1, Minor 6; refined fix round 2): a present-but-non-mapping env:/
    services:/services.<id>: value is invalid GitHub Actions YAML this guard must not silently
    pass through as clean, or crash on — every such shape calls infra() (SystemExit(2)), the
    file's own convention, rather than raising an uncaught AttributeError from calling .items()
    on a scalar. `container:` is the ONE exception: GitHub Actions permits a bare string there as
    an image-only shorthand (confirmed against the SchemaStore workflow schema, which types
    `container` as `oneOf [string, object]` but types each `services.<id>` entry as an object
    only, no string form) — fix round 2, Important. MEASURED before this fix: a bare-string
    `container:` value aborted the entire guard at exit 2 on a workflow shape GitHub Actions
    accepts. A string carries no `env:` to scan, so it is skipped rather than treated as
    malformed.

    `secrets: inherit` (a STRING, not a mapping) is a genuine GitHub Actions shape for a
    reusable-workflow-call job and is deliberately NOT fail-closed here — a name-based check
    cannot see what it forwards. Documented as a limitation (README L23), not fixed.
    """
    out: list[str] = []

    def mapping_pairs(value: object, desc: str) -> list[tuple[str, object]]:
        if value is None:
            return []
        if not isinstance(value, dict):
            infra(f"{name}: job '{job_id}' has {desc} that is not a mapping "
                  f"(got {type(value).__name__}: {value!r})")
        return list(value.items())

    def scan(text: str, where: str) -> None:
        for banned in BANNED_PUBLISH_CREDENTIALS:
            if banned in text:
                out.append(
                    f"{name}: job '{job_id}' references {banned} in {where}. PyPI and npm "
                    f"publish through OIDC trusted publishing (SMA-602). A token here would "
                    f"silently mask a broken exchange rather than fail. Remove it."
                )
        if NPMRC_AUTH_TOKEN in text:
            out.append(
                f"{name}: job '{job_id}' writes an npm {NPMRC_AUTH_TOKEN} in {where}. npm reads "
                f"that at the 'user' config level, which is exactly what masks a failed OIDC "
                f"exchange (SMA-602). Remove it."
            )
        if _NPMRC_AUTH_RE.search(text):
            out.append(
                f"{name}: job '{job_id}' sets an npm _auth credential in {where}. "
                f"getCredentialsByURI honours `_auth` exactly as it honours {NPMRC_AUTH_TOKEN}, "
                f"so it masks a failed OIDC exchange the same way (SMA-602). Remove it."
            )

    for key, value in mapping_pairs(job.get("env"), "an env:"):
        scan(f"{key}: {value}", "the job env:")

    # fix round 2: `container:` accepts a bare string (image-only shorthand) as well as a
    # mapping — see the docstring above. A string carries no env: to scan; skip it. Only a
    # value that is NEITHER a string NOR a mapping is malformed.
    container = job.get("container")
    if isinstance(container, dict):
        for key, value in mapping_pairs(container.get("env"), "a container env:"):
            scan(f"{key}: {value}", "the job container env:")
    elif container is not None and not isinstance(container, str):
        infra(f"{name}: job '{job_id}' has container: that is not a mapping or a string "
              f"(got {type(container).__name__}: {container!r})")

    # fix round 2: unlike `container:`, a `services.<id>` entry has NO string shorthand — the
    # SchemaStore workflow schema types `services` as an object whose values are ALWAYS the
    # object-only `serviceContainer` definition, never a bare string. A non-mapping value here
    # (or a non-mapping `services:` itself) is therefore genuinely malformed, and stays
    # fail-closed via infra(), unchanged from fix round 1.
    services = job.get("services")
    if services is not None:
        if not isinstance(services, dict):
            infra(f"{name}: job '{job_id}' has services: that is not a mapping "
                  f"(got {type(services).__name__}: {services!r})")
        for svc_id, svc in services.items():
            if not isinstance(svc, dict):
                infra(f"{name}: job '{job_id}' has services.{svc_id} that is not a mapping "
                      f"(got {type(svc).__name__}: {svc!r})")
            for key, value in mapping_pairs(svc.get("env"), f"service '{svc_id}' env:"):
                scan(f"{key}: {value}", f"the job services.{svc_id} env:")

    # Minor 5: a reusable-workflow-call job (`uses: ./...`) passes credentials via its OWN
    # job-level secrets:/with: mapping, not steps: — job_publishes and this scan's step-level
    # code are both blind to that shape (V8d's own docstring names the same job_publishes gap).
    # `secrets: inherit` is excluded deliberately — see the docstring above and README L23.
    job_secrets = job.get("secrets")
    if job_secrets != "inherit":
        for key, value in mapping_pairs(job_secrets, "a secrets:"):
            scan(f"{key}: {value}", "the job secrets:")
    for key, value in mapping_pairs(job.get("with"), "a with:"):
        scan(f"{key}: {value}", "the job with:")

    # SMA-602 fix wave, F7: fail closed on a non-list `steps:`, matching the env:/container:/
    # services: convention above. `steps: publish` iterates the string's characters, every
    # `continue` fires, and this whole scan — plus the pypa `password:` rule below — skips in
    # silence (MEASURED).
    for step in steps_of(job, f"{name}: job '{job_id}'"):
        if not isinstance(step, dict):
            continue
        for key, value in mapping_pairs(step.get("env"), "a step env:"):
            scan(f"{key}: {value}", "a step env:")
        scan(str(step.get("run") or ""), "a step run:")
        step_with = step.get("with")
        for key, value in mapping_pairs(step_with, "a step with:"):
            scan(f"{key}: {value}", "a step with:")

        # Final-review Important 1, rule 2: the KEY, not the value. See PYPI_PUBLISH_ACTION.
        # `startswith` (not equality) because the real steps carry an `@<sha>` pin, and the
        # action may legitimately move to a new ref.
        uses = str(step.get("uses") or "")
        if uses.startswith(PYPI_PUBLISH_ACTION) and isinstance(step_with, dict) \
                and "password" in step_with:
            out.append(
                f"{name}: job '{job_id}' passes a password: to {PYPI_PUBLISH_ACTION}. Under "
                f"PyPI Trusted Publishing there is nothing legitimate to put there, and any "
                f"value — a NEW secret name, an env: reference — masks a broken OIDC exchange "
                f"(SMA-602). Remove the key."
            )

    return out


def secret_reference_violations(doc: dict, name: str) -> list[str]:
    """V10 rule 1 (final-review Important 1): the secret NAMES release.yml may reference,
    pinned by strict equality against EXPECTED_RELEASE_SECRETS.

    Strictly stronger than extending BANNED_PUBLISH_CREDENTIALS. The measured bypass was
    `password: ${{ secrets.PYPI_PROJECT_TOKEN }}` — a name no denylist held, and precisely the
    name design §9's rollback plan would create when it mints a fresh project-scoped token.

    Walks the PARSED document, so YAML comments naming the removed tokens are invisible here.

    THE WHOLE-DOCUMENT WALK IS THE RULE, not an implementation detail (SMA-602 fix wave, F4).
    `publish_credential_violations` enumerates scopes; this one deliberately does not, because a
    `secrets` reference can sit in a job `if:`, a `concurrency: group:`, a `run-name:`, a
    `strategy:` matrix or a `defaults:` block, none of which that enumeration covers. MEASURED
    before the F4 fixtures existed: replacing this walk with one over `jobs[*].env`,
    `jobs[*].steps[*].{env,with,run}` and the workflow-level `env:` left `--self-test` at rc 0
    and the real release.yml at rc 0 — every rule-1 fixture put its secret in a scope the
    enumeration already covered. Three rows now pin the walk-only scopes; narrow it and they red.

    Called from both check_main and check_called (fix round 2). The `missing` half runs only for
    RELEASE_WORKFLOW_NAME: every other document — a check_main self-test fixture, or a real called
    workflow like prebuild.yml or wheels.yml — legitimately references no secret at all. That half
    is a liveness check on the pin itself, not a security rule — ci/workflow-credentials/run.sh's
    control row independently asserts release.yml still reads a secret.
    """
    found: set[str] = set()
    unresolved: list[str] = []

    def walk(node: object, key: object = None) -> None:
        if isinstance(node, dict):
            for k, value in node.items():
                walk(k)
                walk(value, k)
        elif isinstance(node, list):
            for item in node:
                # A list inherits its parent key, so `on.workflow_call.inputs` and a
                # block-sequence `if:` are scanned under the same rule as a scalar one.
                walk(item, key)
        elif node is not None:
            names, bad = secret_refs(str(node), bare_expression=(key == "if"))
            found.update(names)
            if bad:
                unresolved.append(str(node).strip()[:120])

    walk(doc)

    out: list[str] = []
    # FAIL-CLOSED, and it comes first: a reference that names no secret cannot be compared
    # against a strict-equality allowlist at all, so reporting "clean" would be a lie.
    for expr in sorted(set(unresolved)):
        out.append(
            f"{name}: reads the `secrets` context in a form that names no secret: {expr!r}. "
            f"EXPECTED_RELEASE_SECRETS is a strict-equality pin of NAMES, and a dynamic or "
            f"whole-context read (`toJSON(secrets)`, `secrets[format(...)]`) cannot be checked "
            f"against it (SMA-602). Write `secrets.NAME` or `secrets['NAME']` instead."
        )
    for unexpected in sorted(found - set(EXPECTED_RELEASE_SECRETS)):
        out.append(
            f"{name}: references the secret {unexpected}, which is not in "
            f"EXPECTED_RELEASE_SECRETS. PyPI and npm authenticate through OIDC trusted "
            f"publishing (SMA-602); a secret here would mask a broken exchange rather than "
            f"fail. If this credential genuinely cannot be an OIDC exchange, add it to "
            f"EXPECTED_RELEASE_SECRETS deliberately, with a comment saying why."
        )
    if name == RELEASE_WORKFLOW_NAME:
        for missing in sorted(set(EXPECTED_RELEASE_SECRETS) - found):
            out.append(
                f"{name}: no longer references {missing}, which EXPECTED_RELEASE_SECRETS pins. "
                f"The pin has gone stale — re-baseline it deliberately."
            )
    return out


def _grants_scope(block: object, scope: str) -> bool | None:
    """Whether a `permissions:` block grants `scope` at `write`. None means "no block here".

    Three shapes GitHub accepts, and one deliberate omission. `write-all` grants everything.
    `read-all`, or any other string, grants nothing. A mapping grants exactly what it names —
    and, crucially, sets every scope it does NOT name to `none`, which is the whole reason V11
    exists. Anything else (a list, a number) is not a shape GitHub honours, so it grants
    nothing and the caller reds: fail-closed, this file's convention.
    """
    if block is None:
        return None
    if isinstance(block, str):
        return block.strip() == "write-all"
    if isinstance(block, dict):
        return str(block.get(scope, "")).strip() == "write"
    return False


def id_token_violations(doc: dict, name: str) -> list[str]:
    """V11: both OIDC publish jobs must still hold `id-token: write` (SMA-602 fix wave, F2).

    Scoped to RELEASE_WORKFLOW_NAME — see OIDC_PUBLISH_JOBS for why a called workflow must not
    inherit this rule. A job-level `permissions:` block WINS over the workflow-level one and sets
    every scope it omits to `none`, so the job block is consulted first and the workflow block is
    only the fallback for a job that declares none at all.
    """
    if name != RELEASE_WORKFLOW_NAME:
        return []
    out: list[str] = []
    jobs = doc.get("jobs") or {}
    workflow_grant = _grants_scope(doc.get("permissions"), ID_TOKEN_SCOPE)
    for jid in OIDC_PUBLISH_JOBS:
        job = jobs.get(jid)
        if not isinstance(job, dict):
            out.append(
                f"{name}: V11: no job named '{jid}' exists. V11 keys on that literal name, so "
                f"without this floor a rename would leave the OIDC grant unasserted."
            )
            continue
        grant = _grants_scope(job.get("permissions"), ID_TOKEN_SCOPE)
        if grant is None:
            grant = bool(workflow_grant)
        if not grant:
            out.append(
                f"{name}: V11: job '{jid}' does not grant `{ID_TOKEN_SCOPE}: write`. That is the "
                f"credential OIDC trusted publishing runs on (SMA-602). Without it the runner "
                f"sets no ACTIONS_ID_TOKEN_REQUEST_* variables, npm's oidc.js returns undefined "
                f"without throwing, and the publish dies ENEEDAUTH AFTER crates.io has "
                f"published. A job-level permissions: block sets every scope it omits to none, "
                f"so adding a narrower block is the same defect as deleting the grant."
            )
    return out


def npm_floor_violations(doc: dict, name: str) -> list[str]:
    """V12: the npm >= 11.5.1 OIDC floor, pinned in BOTH workflows that carry it.

    See NPM_OIDC_FLOOR_LINES for what each pinned line closes and why this pin lives here rather
    than in ci/affected-graph/ci_targets.py. Applies only to NPM_OIDC_FLOOR_SUBJECTS: no other
    workflow provisions npm for a trusted-publishing exchange, and demanding these lines of one
    that does not would red a correct repository.

    Reads every job's step `run:` bodies from the PARSED document, so the pin is on EXECUTING
    text: a copy of a pinned line living only in a YAML comment cannot satisfy it.
    """
    if name not in NPM_OIDC_FLOOR_SUBJECTS:
        return []
    present: set[str] = set()
    for jid, job in (doc.get("jobs") or {}).items():
        if not isinstance(job, dict):
            continue
        for step in steps_of(job, f"{name}: job '{jid}'"):
            if not isinstance(step, dict):
                continue
            for line in str(step.get("run") or "").splitlines():
                present.add(line.strip())
    return [
        f"{name}: V12: the npm OIDC floor line {site!r} is gone. Both {' and '.join(
            NPM_OIDC_FLOOR_SUBJECTS)} carry this provisioning block, and nothing else pins "
        f"them to each other — lowering or deleting one copy alone used to keep `moon ci` "
        f"fully green (SMA-602). npm below 11.5.1 has no lib/utils/oidc.js at all, so the "
        f"publish would die ENEEDAUTH after crates.io has published. Restore the line in BOTH "
        f"workflows, or re-baseline NPM_OIDC_FLOOR_LINES deliberately."
        for site in NPM_OIDC_FLOOR_LINES if site not in present
    ]


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
        if not isinstance(job, dict) or not job_publishes(job, f"{name}: job '{jid}'"):
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
        if not isinstance(job, dict) or not job_publishes(job, f"{name}: job '{jid}'"):
            continue
        if APPROVAL_JOB not in gated_path_jobs(jid, jobs):
            out.append(f"{name}: V8c: job '{jid}' can reach a registry, but '{APPROVAL_JOB}' is "
                       f"not on its needs: path. It would publish without passing the gate.")
    return out


def plan_run_segments(run_text: str) -> list[str]:
    """Every non-empty command segment of a `run:` block, comments already stripped."""
    return [seg.strip()
            for line in run_text.splitlines()
            for seg in command_segments(line)
            if seg.strip()]


def _is_plan_invocation(segment: str) -> bool:
    tokens = segment.split()
    i = 0
    while i < len(tokens) and _ENV_ASSIGN_RE.match(tokens[i]):
        i += 1
    if i < len(tokens) and tokens[i] in _CMD_PREFIXES:
        i += 1
        while i < len(tokens) and _ENV_ASSIGN_RE.match(tokens[i]):
            i += 1
    if i >= len(tokens):
        return False
    word = tokens[i]
    if word != PLAN_SCRIPT and not word.endswith("/" + PLAN_SCRIPT):
        return False
    return PLAN_SCRIPT_FLAG in tokens[i + 1:]


def invokes_plan_script(run_text: str) -> bool:
    """True when some command segment of `run_text` really RUNS PLAN_SCRIPT with its flag."""
    return any(_is_plan_invocation(seg) for seg in plan_run_segments(run_text))

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
        m = _PLAN_STEP_RE.match(expr.strip())
        if not m:
            out.append(f"{name}: V9c: outputs.{PLAN_OUTPUT} is {expr!r}, which names no "
                       f"steps.<id>.outputs.{PLAN_OUTPUT} — or names one inside a LARGER "
                       f"expression. It must be exactly "
                       f"'${{{{ steps.<id>.outputs.{PLAN_OUTPUT} }}}}': anything else can "
                       f"resolve to a constant, and "
                       f"'${{{{ steps.<id>.outputs.{PLAN_OUTPUT} || \\'true\\' }}}}' resolves "
                       f"to 'true' on an unset output, which SKIPS every consumer.")
        else:
            steps_by_id = {s.get("id"): s
                           for s in steps_of(plan, f"{name}: job '{PLAN_JOB}'")
                           if isinstance(s, dict)}
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
        # SMA-603 fix wave, 2c: a COMMAND-WORD test, not a substring one. See
        # invokes_plan_script's own comment for the comment-satisfies-the-pin shape it closes.
        segments = plan_run_segments(run_text)
        if not invokes_plan_script(run_text):
            out.append(f"{name}: V9d: step {decision.get('id')!r} in job '{PLAN_JOB}' never "
                       f"invokes {PLAN_SCRIPT} {PLAN_SCRIPT_FLAG} as a command. Without this, "
                       f"V9c passes on an inline `echo {PLAN_OUTPUT}=true` — and a COMMENT "
                       f"naming the script does not count.")
        # V9e (SMA-603 fix wave, 2g — the RE-JUDGED parked finding B2). V9d asks whether the
        # decision step invokes the checker; it cannot ask what ELSE the step does. The measured
        # bypass is one appended line:
        #     run: |
        #       ci/release-plan/run.sh --github-output
        #       echo "nothing_to_release=true" > "$GITHUB_OUTPUT"
        # The checker really runs, V9c and V9d both pass, and the second line overwrites the
        # verdict — every release silently dropped. B2 was parked as "out of reach for any
        # structural workflow guard". That was WRONG: the step can be required to be EXACTLY the
        # invocation, which is a strict-equality pin of the same family as ACCEPTED_PLAN_FORMS,
        # and which the real step already satisfies (release.yml's own comment calls it ONE
        # PHYSICAL LINE, for command_segments' sake).
        #
        # SCOPE, stated honestly. This bounds what the DECISION STEP may do; it does not bound
        # the job. A LATER step in `plan` can still overwrite $GITHUB_OUTPUT, and no structural
        # check in this file forbids that — the residual stays recorded, one step narrower than
        # B2 was. The cost of the rule if it is ever wrong is small and visible: a plan job that
        # legitimately needs setup work does it in its OWN steps, which is what the spec asks for
        # anyway.
        elif len(segments) != 1:
            out.append(f"{name}: V9e: step {decision.get('id')!r} in job '{PLAN_JOB}' runs "
                       f"{len(segments)} commands; it must run EXACTLY one, the "
                       f"{PLAN_SCRIPT} {PLAN_SCRIPT_FLAG} invocation. A second command in the "
                       f"same step can overwrite $GITHUB_OUTPUT after the checker wrote it, "
                       f"which passes V9c and V9d and silently drops every release. Move setup "
                       f"work into its own step.")
    return out


def check_main(doc: dict, name: str) -> list[str]:
    """V1-V5, V7, V8a-c and V9 over the release workflow. V6 applies to CALLED workflows (see
    check_called) and V8d to every job's local callee (see callee_boundary_violations) — both
    need the filesystem, which this function, driven purely off a parsed doc, deliberately does
    not touch."""
    out: list[str] = []
    jobs = doc["jobs"]

    # V10 fix round 1, Important 3: the WORKFLOW-level env: block, scanned via the same helper
    # with a synthetic job carrying only `env`. That scope reaches every step through the
    # `secrets` context, so a credential lifted from a step env: up to the workflow root would
    # otherwise pass V10 clean.
    out += publish_credential_violations({"env": doc.get("env") or {}}, "<workflow>", name)

    # V10 rule 1 (final-review Important 1): the whole-document secret-name allowlist. Runs on
    # the DOCUMENT, not per job, because a `secrets` reference can sit anywhere — a job `if:`,
    # a `with:`, a `concurrency: group:` — and a per-job walk would have to enumerate scopes the
    # way publish_credential_violations does. This one does not need to: it bans by name.
    out += secret_reference_violations(doc, name)

    # V11/V12 (SMA-602 fix wave, F2/F3). Both are scoped BY WORKFLOW NAME inside the functions
    # themselves — V11 to release.yml, V12 to release.yml and prebuild.yml — so calling them
    # unconditionally here and in check_called is what gives prebuild.yml its V12 coverage,
    # while a fixture (named "fixture") and wheels.yml stay untouched by either.
    out += id_token_violations(doc, name)
    out += npm_floor_violations(doc, name)

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

        # V10: applies to EVERY job, UNGATED_JOBS members included, so it runs BEFORE the
        # `continue` below. An exempt job with a publish token is the worst case, not an
        # excused one.
        out += publish_credential_violations(job, job_id, name)

        if job_id in UNGATED_JOBS:
            # V7 (fix round 3, Critical 2). The exemption above is from V1 (the gating rule) and
            # nothing else. Assert the premise that justifies it: a job allowed to run ungated on
            # every push to `main` must not be able to reach a registry. Without this, a
            # `release-pr` job carrying `cargo publish` + `npm publish` +
            # `pypa/gh-action-pypi-publish` passed the whole guard clean — measured, exit 0.
            if job_publishes(job, f"{name}: job '{job_id}'"):
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
            for step in steps_of(pjob, f"{name}: job '{pid}'"):
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

    # V10 fix round 1, Important 3: same workflow-level env: scan as check_main, for the same
    # reason V5 and V10's per-job call both apply here — a called workflow escaping check_main
    # must not escape this scan either.
    out += publish_credential_violations({"env": doc.get("env") or {}}, "<workflow>", name)

    # V10 rule 1 (fix round 2, CodeRabbit + independent branch review): the unexpected-secret-name
    # half of secret_reference_violations must reach a called workflow too, for the same reason
    # every other V10 check does — prebuild.yml and wheels.yml are exactly the files a fresh
    # PYPI_PROJECT_TOKEN-shaped credential would land in first. This was the one V10 rule that
    # `check_main` ran and `check_called` did not; it is the same class of gap SMA-579's V5 left
    # once, inlined in `check_main` alone. The `missing`/liveness half of the same function stays
    # inert here: it fires only when `name == RELEASE_WORKFLOW_NAME`, and every called workflow's
    # name is its own filename, never that constant — so a legitimate callee that references no
    # secret at all still passes clean.
    out += secret_reference_violations(doc, name)

    # V11/V12 also apply here, for the same reason V5 and V10 do — a called workflow escaping
    # check_main must not escape them either. V11 is inert on every callee (it keys on
    # RELEASE_WORKFLOW_NAME); V12 is what reaches prebuild.yml's copy of the npm floor, which is
    # the second half of the F3 pin and the only reason the two copies are held to each other.
    out += id_token_violations(doc, name)
    out += npm_floor_violations(doc, name)

    # V5 also applies here (fix round 3, Important 4). It used to be inlined in check_main, which
    # main() runs on argv[0] only, so every CALLED workflow escaped it — including prebuild.yml's
    # own `napi prepublish` invocation, the one whose comment says the flag "IS REQUIRED".
    for jid, j in doc["jobs"].items():
        if isinstance(j, dict):
            out += napi_violations(j, jid, name)
            # V10 also applies here, for the same reason as V5 above: a called workflow that
            # escapes check_main must not escape this check either.
            out += publish_credential_violations(j, jid, name)

    publishing = [jid for jid, j in doc["jobs"].items()
                  if isinstance(j, dict) and job_publishes(j, f"{name}: job '{jid}'")]
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
            if job_publishes(cjob, f"{name}: job '{jid}' callee '{uses}' job '{cjid}'"):
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
    # SMA-603 fix wave, 2c. V9d was a raw substring test over the decision step's `run:` text, so
    # a COMMENT naming the script satisfied it while the step hardcoded the answer. This is the
    # measured bypass, in the exact shape a reader would write it.
    ("V9d fix-wave: a COMMENT naming the script does not count as invoking it", "main",
     _OK_MAIN.replace(
         "      - id: decide\n        run: ci/release-plan/run.sh --github-output\n",
         "      - id: decide\n"
         "        run: |\n"
         "          # ci/release-plan/run.sh --github-output decides this\n"
         "          echo \"nothing_to_release=true\" >> \"$GITHUB_OUTPUT\"\n"),
     "V9d"),
    # ...and the same class one step further: the script really RUNS, but with a flag that writes
    # no output at all. `--self-test` exits 0 and appends nothing, so the job output stays unset.
    ("V9d fix-wave: the decision step runs the script with the wrong flag", "main",
     _OK_MAIN.replace("run: ci/release-plan/run.sh --github-output",
                      "run: ci/release-plan/run.sh --self-test"), "V9d"),
    # ...and a trailing-comment CONTROL: a real invocation carrying a comment must stay accepted,
    # or the fix above would red the legitimate form it is meant to allow.
    ("V9d fix-wave CONTROL: a real invocation with a trailing comment is accepted", "main",
     _OK_MAIN.replace("run: ci/release-plan/run.sh --github-output",
                      "run: ci/release-plan/run.sh --github-output  # the decision"), None),
    # ...and an env-prefixed CONTROL, the shape a `GITHUB_EVENT_NAME=... ` prefix would take.
    ("V9d fix-wave CONTROL: a leading env assignment and a bash prefix are accepted", "main",
     _OK_MAIN.replace("run: ci/release-plan/run.sh --github-output",
                      "run: GITHUB_EVENT_NAME=push bash ./ci/release-plan/run.sh --github-output"),
     None),
    # SMA-603 fix wave, 2d. V9c used `_PLAN_STEP_RE.search`, so ANY expression merely CONTAINING
    # the step reference passed — including one that resolves to a constant. GitHub's `||` yields
    # its right operand when the left is the empty string, so an unset step output becomes the
    # literal 'true' and every consumer SKIPS. That is the branch's central property, inverted in
    # one edit, and nothing red before this row.
    ("V9c fix-wave: the outputs expression defaults to 'true' via ||", "main",
     _OK_MAIN.replace(
         "nothing_to_release: ${{ steps.decide.outputs.nothing_to_release }}",
         "nothing_to_release: ${{ steps.decide.outputs.nothing_to_release || 'true' }}"),
     "V9c"),
    # ...and the concatenation shape, which the `search` form also accepted.
    ("V9c fix-wave: the outputs expression is a concatenation, not the bare reference", "main",
     _OK_MAIN.replace(
         "nothing_to_release: ${{ steps.decide.outputs.nothing_to_release }}",
         "nothing_to_release: true${{ steps.decide.outputs.nothing_to_release }}"),
     "V9c"),
    # SMA-603 fix wave, 2g — the re-judged parked finding B2, verbatim. The checker really runs;
    # the next line in the SAME step overwrites its verdict. V9c and V9d both pass on this shape.
    ("V9e fix-wave: the decision step overwrites $GITHUB_OUTPUT after running the checker", "main",
     _OK_MAIN.replace(
         "      - id: decide\n        run: ci/release-plan/run.sh --github-output\n",
         "      - id: decide\n"
         "        run: |\n"
         "          ci/release-plan/run.sh --github-output\n"
         "          echo \"nothing_to_release=true\" > \"$GITHUB_OUTPUT\"\n"),
     "V9e"),
    # ...and the same class on ONE line, via a `;` chain, which command_segments splits.
    ("V9e fix-wave: a `;`-chained overwrite in the decision step's one line", "main",
     _OK_MAIN.replace(
         "run: ci/release-plan/run.sh --github-output",
         "run: ci/release-plan/run.sh --github-output; "
         "echo nothing_to_release=true > \"$GITHUB_OUTPUT\""),
     "V9e"),
    # ...and the CONTROL: a block-scalar `run:` holding ONLY the invocation stays clean, so V9e
    # pins the command COUNT rather than the YAML scalar style.
    ("V9e fix-wave CONTROL: a block scalar holding only the invocation is accepted", "main",
     _OK_MAIN.replace(
         "      - id: decide\n        run: ci/release-plan/run.sh --github-output\n",
         "      - id: decide\n"
         "        run: |\n"
         "          # the decision\n"
         "          ci/release-plan/run.sh --github-output\n"),
     None),
    # ...and the CONTROL for the whitespace tolerance the strict form deliberately keeps.
    ("V9c fix-wave CONTROL: extra whitespace inside ${{ }} is accepted", "main",
     _OK_MAIN.replace(
         "nothing_to_release: ${{ steps.decide.outputs.nothing_to_release }}",
         "nothing_to_release: ${{   steps.decide.outputs.nothing_to_release   }}"),
     None),

    # --- V10 (SMA-602): no registry publish credential in the release path -----------------
    ("V10 npm token in a step env", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - env:\n"
         "          NODE_AUTH_TOKEN: ${{ secrets.NPM_TOKEN }}\n"
         "        run: npm publish\n"),
     "references NODE_AUTH_TOKEN"),
    ("V10 pypi token in a step with:", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - uses: pypa/gh-action-pypi-publish@v1\n"
         "        with:\n"
         "          password: ${{ secrets.PYPI_API_TOKEN }}\n"),
     "references PYPI_API_TOKEN"),
    ("V10 npmrc authToken written in a run:", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         '    steps: [{run: \'echo "//registry.npmjs.org/:_authToken=x" > "$HOME/.npmrc"\'}]'),
     "writes an npm _authToken"),
    # NEGATIVE CONTROL, and the most important row here. Without it, a future edit could ban the
    # whole `secrets` context and every other V10 row would still pass — while breaking the App
    # token mint and reding ci/workflow-credentials/run.sh's control row.
    ("V10 App secrets stay clean", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - env:\n"
         "          APP_ID: ${{ secrets.PAIGASUS_BOT_APP_ID }}\n"
         "          KEY: ${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}\n"
         "        run: release-plz release\n"),
     None),

    # --- V10 fix round 1, Important 1: the check_called call site had no fixture coverage ------
    # (all four original V10 rows were kind "main"), so deleting that call site left --self-test
    # green — the exact SMA-579 V5 failure shape the brief warned about.
    ("V10 called workflow with a step env NPM_TOKEN reds too", "called",
     ("on:\n  workflow_call:\n  pull_request:\n    branches:\n      - main\n"
      "jobs:\n  build:\n    steps:\n      - env:\n"
      "          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}\n        run: echo hi\n"),
     "references NPM_TOKEN"),

    # --- V10 fix round 1, Important 2: NPM_TOKEN was an unasserted member of the ban list -------
    # (the only fixture containing it also contained NODE_AUTH_TOKEN in the same scanned text, so
    # `want` never actually pinned on the NPM_TOKEN verdict). Isolated so NPM_TOKEN is the ONLY
    # banned string present.
    ("V10 npm token alone in a step with:", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - uses: some-org/some-publish-action@v1\n"
         "        with:\n"
         "          token: ${{ secrets.NPM_TOKEN }}\n"),
     "references NPM_TOKEN"),

    # --- V10 fix round 1, Important 3: the WORKFLOW-level env: block was unscanned -------------
    # (the spec, §5.4, is unqualified — "no PYPI_API_TOKEN, NPM_TOKEN or NODE_AUTH_TOKEN
    # reference" — so a credential lifted from a step env: to the workflow root must still red).
    ("V10 workflow-level env: is scanned too", "main",
     _OK_MAIN.replace(
         "      - main\njobs:\n",
         "      - main\nenv:\n  NPM_TOKEN: ${{ secrets.NPM_TOKEN }}\njobs:\n"),
     "references NPM_TOKEN"),
    ("V10 called workflow-level env: is scanned too", "called",
     ("on:\n  workflow_call:\n  pull_request:\n    branches:\n      - main\n"
      "env:\n  NPM_TOKEN: ${{ secrets.NPM_TOKEN }}\n"
      "jobs:\n  build:\n    steps: [{run: echo hi}]\n"),
     "references NPM_TOKEN"),

    # --- V10 fix round 1, Minor 4: job container:/services: env: was unscanned -----------------
    # (both accept the `secrets` context the same way a step env: does).
    ("V10 job container env: is scanned", "main",
     _OK_MAIN.replace(
         "  release:\n    needs: [build, approve-release]\n    runs-on: ubuntu-latest\n"
         "    steps: [{run: release-plz release}]\n",
         "  release:\n    needs: [build, approve-release]\n    runs-on: ubuntu-latest\n"
         "    container:\n      image: node:20\n      env:\n"
         "        NPM_TOKEN: ${{ secrets.NPM_TOKEN }}\n"
         "    steps: [{run: release-plz release}]\n"),
     "references NPM_TOKEN"),
    ("V10 job services.<id>.env: is scanned", "main",
     _OK_MAIN.replace(
         "  release:\n    needs: [build, approve-release]\n    runs-on: ubuntu-latest\n"
         "    steps: [{run: release-plz release}]\n",
         "  release:\n    needs: [build, approve-release]\n    runs-on: ubuntu-latest\n"
         "    services:\n      registry:\n        image: verdaccio/verdaccio\n        env:\n"
         "          NPM_TOKEN: ${{ secrets.NPM_TOKEN }}\n"
         "    steps: [{run: release-plz release}]\n"),
     "references NPM_TOKEN"),

    # --- V10 fix round 1, Minor 5: job-level secrets:/with: on a reusable-workflow-call job -----
    # was unscanned (job_publishes and this scan's step-level code are both blind to that shape —
    # V8d's own docstring names the same job_publishes gap for a different check).
    ("V10 job-level secrets: on a reusable workflow call reds", "main",
     _OK_MAIN.replace(
         "  release:\n    needs: [build, approve-release]\n    runs-on: ubuntu-latest\n"
         "    steps: [{run: release-plz release}]\n",
         "  release:\n    needs: [build, approve-release]\n"
         "    uses: ./.github/workflows/does-not-exist.yml\n"
         "    secrets:\n      PYPI_API_TOKEN: ${{ secrets.PYPI_API_TOKEN }}\n"),
     "references PYPI_API_TOKEN"),
    ("V10 job-level with: on a reusable workflow call reds", "main",
     _OK_MAIN.replace(
         "  release:\n    needs: [build, approve-release]\n    runs-on: ubuntu-latest\n"
         "    steps: [{run: release-plz release}]\n",
         "  release:\n    needs: [build, approve-release]\n"
         "    uses: ./.github/workflows/does-not-exist.yml\n"
         "    with:\n      npm-token: ${{ secrets.NPM_TOKEN }}\n"),
     "references NPM_TOKEN"),
    # CONTROL, and the reason Minor 5 does not try to close `secrets: inherit` too: it is a
    # STRING, not a mapping, and names nothing a name-based check can catch. Documented as a
    # limitation (README L23), not fixed — this row proves it neither crashes nor false-positives.
    ("V10 job-level secrets: inherit stays clean (documented limitation, README L23)", "main",
     _OK_MAIN.replace(
         "  release:\n    needs: [build, approve-release]\n    runs-on: ubuntu-latest\n"
         "    steps: [{run: release-plz release}]\n",
         "  release:\n    needs: [build, approve-release]\n"
         "    uses: ./.github/workflows/does-not-exist.yml\n"
         "    secrets: inherit\n"),
     None),

    # --- V10 fix round 2, Important: `container:` accepts a bare image-reference STRING as a
    # documented GitHub Actions shorthand (SchemaStore schema: oneOf [string, object]) — a string
    # carries no env: to scan, so the scan must SKIP it rather than infra() on it. MEASURED before
    # this fix: `container: <image>` aborted the whole guard at exit 2 on this valid shape.
    ("V10 job-level container: string shorthand is accepted, not aborted", "main",
     _OK_MAIN.replace(
         "  release:\n    needs: [build, approve-release]\n    runs-on: ubuntu-latest\n"
         "    steps: [{run: release-plz release}]\n",
         "  release:\n    needs: [build, approve-release]\n    runs-on: ubuntu-latest\n"
         "    container: postgres:16\n"
         "    steps: [{run: release-plz release}]\n"),
     None),

    # --- V10 final review, Important 1: three rules strictly stronger than the denylist -------
    # Every row below was MEASURED as a V10 bypass (guard exit 0) against the real release.yml
    # before the rule existed.

    # Rule 1 — the secret-name allowlist. This is the rollback shape from design §9: a NEW
    # project-scoped PyPI token gets a NEW secret name and sails past BANNED_PUBLISH_CREDENTIALS.
    ("V10 rule 1: an unpinned secret name reds even though no denylist holds it", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - uses: pypa/gh-action-pypi-publish@v1\n"
         "        with:\n"
         "          packages-dir: dist\n"
         "          password: ${{ secrets.PYPI_PROJECT_TOKEN }}\n"),
     "references the secret PYPI_PROJECT_TOKEN"),
    # Rule 1 reaches scopes publish_credential_violations never walks — here a job-level `if:`.
    # SMA-602 fix wave, F4: this row's LABEL said "job if:" and its BODY inserted a job `env:`,
    # a scope publish_credential_violations already covers — so the walk-only claim it was
    # written to pin was never exercised. It now uses a real job `if:`, in the BARE (unwrapped)
    # form GitHub also evaluates as an expression, which no `${{ }}` span extraction can see.
    ("V10 rule 1: a secret referenced from a bare job if: reds too", "main",
     _OK_MAIN.replace(
         "  release:\n    needs: [build, approve-release]\n",
         "  release:\n    needs: [build, approve-release]\n"
         "    if: secrets.SOME_OTHER_TOKEN != ''\n"),
     "references the secret SOME_OTHER_TOKEN"),
    # ...and a second walk-only scope, so narrowing the walk cannot survive by covering `if:`
    # alone. A job-level `concurrency: group:` is reachable from no scope
    # publish_credential_violations enumerates.
    ("V10 rule 1: a secret in a job concurrency: group: reds (walk-only scope)", "main",
     _OK_MAIN.replace(
         "  release:\n    needs: [build, approve-release]\n",
         "  release:\n    needs: [build, approve-release]\n"
         "    concurrency:\n      group: ${{ secrets.CONCURRENCY_TOKEN }}\n"),
     "references the secret CONCURRENCY_TOKEN"),
    # ...and a third, at the WORKFLOW level rather than inside a job: `run-name:`.
    ("V10 rule 1: a secret in the workflow run-name: reds (walk-only scope)", "main",
     _OK_MAIN.replace(
         "on:\n  push:",
         "run-name: ${{ secrets.RUN_NAME_TOKEN }}\non:\n  push:"),
     "references the secret RUN_NAME_TOKEN"),
    # Rule 2 — the KEY, whatever the value. No secret is referenced at all here, so rule 1
    # cannot see it; this is the second measured bypass.
    ("V10 rule 2: password: from an env: reference on the PyPI action reds", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - uses: pypa/gh-action-pypi-publish@v1\n"
         "        with:\n"
         "          password: ${{ env.PY_CRED }}\n"),
     "passes a password: to pypa/gh-action-pypi-publish"),
    # Rule 2 must survive the `@<sha>` pin the real steps carry, hence startswith, not equality.
    ("V10 rule 2: a sha-pinned PyPI action is matched too", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - uses: pypa/gh-action-pypi-publish"
         "@dc37677b2e1c63e2034f94d8a5b11f265b73ba33\n"
         "        with:\n"
         "          password: hunter2\n"),
     "passes a password: to pypa/gh-action-pypi-publish"),
    # Rule 2 CONTROL: the real shape. A PyPI publish step with NO password: is the whole point
    # of SMA-602 and must stay clean, or the rule would ban trusted publishing itself.
    ("V10 rule 2 control: the PyPI action without a password: stays clean", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - uses: pypa/gh-action-pypi-publish@v1\n"
         "        with:\n"
         "          packages-dir: dist\n"
         "          skip-existing: true\n"),
     None),
    # Rule 3 — `_auth`, the other live npmrc credential key.
    ("V10 rule 3: an npmrc _auth written in a run: reds", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         '    steps: [{run: \'echo "//registry.npmjs.org/:_auth=$LEGACY" > "$HOME/.npmrc"\'}]'),
     "sets an npm _auth credential"),
    # Rule 3, the environment spelling of the same key. Case-insensitive matching is what makes
    # this row work; `${{ secrets.NPM_LEGACY_AUTH }}` additionally trips rule 1, and the `want`
    # substring test is satisfied by either — so assert the _auth message specifically.
    ("V10 rule 3: NPM_CONFIG__AUTH in a step env: reds", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - env:\n"
         "          NPM_CONFIG__AUTH: ${{ secrets.NPM_LEGACY_AUTH }}\n"
         "        run: release-plz release\n"),
     "sets an npm _auth credential"),
    # Rule 3 CONTROL: `_authToken` must keep producing its OWN message and not also trip rule 3,
    # or the two rules would double-report every npmrc token. `_auth` is a prefix of
    # `_authToken`; the negative lookahead is what keeps them disjoint.
    ("V10 rule 3 control: _authToken does not also trip the _auth rule", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         '    steps: [{run: \'echo "//registry.npmjs.org/:_authToken=x" > "$HOME/.npmrc"\'}]'),
     "writes an npm _authToken"),
    # Rule 3 CONTROL: NODE_AUTH_TOKEN keeps its BANNED_PUBLISH_CREDENTIALS message. `_AUTH_` is
    # followed by an underscore, so the lookahead excludes it — one violation, not two.
    ("V10 rule 3 control: NODE_AUTH_TOKEN stays a denylist hit", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - env:\n"
         "          NODE_AUTH_TOKEN: ${{ secrets.PAIGASUS_BOT_APP_ID }}\n"
         "        run: release-plz release\n"),
     "references NODE_AUTH_TOKEN"),
    # THE CLEAN CONTROL for all three rules together. The two legitimate App secrets, referenced
    # exactly as release.yml references them, must still pass. Without this row a future edit
    # could ban the `secrets` context wholesale and every red row above would still pass — while
    # breaking the App token mint and reding ci/workflow-credentials/run.sh's control row.
    ("V10 rules 1-3 control: the two PAIGASUS_BOT_* secrets stay clean", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - uses: actions/create-github-app-token@v2\n"
         "        with:\n"
         "          app-id: ${{ secrets.PAIGASUS_BOT_APP_ID }}\n"
         "          private-key: ${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}\n"
         "      - uses: pypa/gh-action-pypi-publish@v1\n"
         "        with:\n"
         "          packages-dir: dist\n"
         "          skip-existing: true\n"),
     None),

    # --- V10 rule 1 fix round 2 (CodeRabbit + independent branch review): secret_reference_
    # violations must reach check_called too --------------------------------------------------
    ("V10 rule 1: a called workflow referencing an unpinned secret name reds", "called",
     ("on:\n  workflow_call:\n  pull_request:\n    branches:\n      - main\n"
      "jobs:\n  build:\n    steps:\n      - uses: pypa/gh-action-pypi-publish@v1\n"
      "        with:\n          packages-dir: dist\n"
      "          password: ${{ secrets.PYPI_PROJECT_TOKEN }}\n"),
     "references the secret PYPI_PROJECT_TOKEN"),
    # CONTROL: a legitimate callee that references no secret at all — the shape prebuild.yml and
    # wheels.yml actually have — must stay clean. Without this row, calling
    # secret_reference_violations from check_called with the wrong `name` (RELEASE_WORKFLOW_NAME,
    # say, instead of the callee's own filename) would silently turn on the `missing`/liveness
    # half for every callee and red this file on nothing.
    ("V10 rule 1 control: a called workflow with no secrets stays clean", "called",
     ("on:\n  workflow_call:\n  pull_request:\n    branches:\n      - main\n"
      "jobs:\n  build:\n    steps: [{run: maturin build}]\n"), None),

    # --- SMA-602 fix wave, F1: the secret-name allowlist matched ONE spelling of four ---------
    # Every row below was MEASURED as a live V10 rule-1 bypass (guard exit 0) against a copy of
    # the real release.yml before `secret_refs` replaced the old
    # `re.compile(r"secrets\.([A-Za-z_][A-Za-z0-9_]*)")`. GitHub Actions accepts all four, and
    # ci/workflow-credentials/workflow_credentials.py already pinned the same four as live
    # fixtures — which is why that file's EXPR_SPAN / STRING_LITERAL / SECRETS_CTX machinery is
    # reused here rather than a second, weaker regex being invented.
    ("F1: secrets['NAME'] bracket index reds", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - env:\n"
         "          CRED: ${{ secrets['PYPI_PROJECT_TOKEN'] }}\n"
         "        run: echo hi\n"),
     "references the secret PYPI_PROJECT_TOKEN"),
    ('F1: secrets["NAME"] double-quoted bracket index reds', "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - env:\n"
         '          CRED: ${{ secrets["PYPI_PROJECT_TOKEN"] }}\n'
         "        run: echo hi\n"),
     "references the secret PYPI_PROJECT_TOKEN"),
    ("F1: a capitalised Secrets. context reds (function/context names are case-insensitive)",
     "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - env:\n"
         "          CRED: ${{ Secrets.PYPI_PROJECT_TOKEN }}\n"
         "        run: echo hi\n"),
     "references the secret PYPI_PROJECT_TOKEN"),
    ("F1: an uppercased SECRETS. context reds", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - env:\n"
         "          CRED: ${{ SECRETS.PYPI_PROJECT_TOKEN }}\n"
         "        run: echo hi\n"),
     "references the secret PYPI_PROJECT_TOKEN"),
    # F12 — rule 2 is bound to ONE action name (`uses.startswith(PYPI_PUBLISH_ACTION)`), so a
    # hand-rolled twine upload matches NO rule of its own. Fixing F1 is what closes it in
    # practice: the unexpected secret name reds, whatever tool consumes it. This is the exact
    # shape the reviewer measured passing, and it is NOT a claim that every upload tool is
    # enumerated — see README L24.
    ("F12: a hand-rolled twine upload reds through the NAME, not the action", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - env:\n"
         "          PYPI_CRED: ${{ secrets['PYPI_NEW'] }}\n"
         '        run: uv run twine upload -u __token__ -p "$PYPI_CRED" dist/*\n'),
     "references the secret PYPI_NEW"),
    # FAIL-CLOSED: a read of the whole context names nothing a strict-equality pin can compare,
    # so it must red rather than read clean.
    ("F1: a whole-context toJSON(secrets) read names nothing and fails closed", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - env:\n"
         "          ALL: ${{ toJSON(secrets) }}\n"
         "        run: echo hi\n"),
     "names no secret"),
    # CONTROL for the literal stripping the extraction depends on: the context name in
    # `hashFiles('secrets.txt')` sits INSIDE a string literal and is not a context read at all.
    ("F1 control: hashFiles('secrets.txt') is not a secrets read", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - env:\n"
         "          H: ${{ hashFiles('secrets.txt') }}\n"
         "        run: echo hi\n"),
     None),
    # CONTROL for the context BOUNDARY: `steps.<id>.outputs.secrets` and `inputs.secrets-file`
    # are ordinary expressions, not context reads. Both were measured false positives against a
    # bare \bsecrets\b in ci/workflow-credentials, and the same boundary is reused here.
    ("F1 control: an expression merely ENDING in .secrets is not a context read", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - env:\n"
         "          S: ${{ steps.decide.outputs.secrets }}\n"
         "        run: echo hi\n"),
     None),

    # --- SMA-602 fix wave, F6: _NPMRC_AUTH_RE matched ordinary identifiers -------------------
    # All three below were MEASURED matching the old `_auth(?![A-Za-z0-9_])` form. Each would
    # have red this gate with a message about npm credentials on a step touching no npm
    # registry, blocking every PR behind a message naming the wrong subsystem. release.yml
    # already carries a near-miss (`AUTH_REMOTE=`), included in the first row.
    ("F6: GIT_AUTH / AUTH_REMOTE in a run: are not npm credentials", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         '    steps: [{run: \'GIT_AUTH="x"; AUTH_REMOTE=origin; echo "$GIT_AUTH$AUTH_REMOTE"\'}]'),
     None),
    ("F6: a steps.app_auth.outputs reference is not an npm credential", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - env:\n"
         "          TOKEN: ${{ steps.app_auth.outputs.token }}\n"
         "        run: release-plz release\n"),
     None),
    ("F6: a CRATES_AUTH env key is not an npm credential", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         "    steps:\n"
         "      - env:\n"
         "          CRATES_AUTH: 1\n"
         "        run: release-plz release\n"),
     None),
    # ...and the TRUE positives must survive the tightening. Both spellings, both directions.
    ("F6 control: //registry/:_auth= still reds after the boundary is tightened", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         '    steps: [{run: \'echo "//registry.npmjs.org/:_auth=$LEGACY" >> "$HOME/.npmrc"\'}]'),
     "sets an npm _auth credential"),
    ("F6 control: a bare `npm config set _auth` still reds", "main",
     _OK_MAIN.replace(
         "    steps: [{run: release-plz release}]",
         '    steps: [{run: \'npm config set _auth "$LEGACY"\'}]'),
     "sets an npm _auth credential"),
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


def _v10_minor6_scalar_env_fails_closed() -> str | None:
    """Regression test for V10 fix round 1, Minor 6: a scalar `env:` (job level or step level)
    must infra (exit 2 via SystemExit), never crash with an uncaught AttributeError from calling
    .items() on a str. Expressed here, not as a FIXTURES row, because a FIXTURES row expects
    check_main to RETURN a list — this scenario must instead raise.

    Before this fix, `env: NODE_AUTH_TOKEN` failed safe only by accident, via run.sh's own
    unreadable-file/bad-exit-code handling — not because release_guard.py itself failed closed.
    """
    cases = [
        ("job-level env: scalar",
         {"jobs": {"release": {"env": "NODE_AUTH_TOKEN", "steps": [{"run": "echo hi"}]}}}),
        ("step-level env: scalar",
         {"jobs": {"release": {"steps": [{"env": "NODE_AUTH_TOKEN", "run": "echo hi"}]}}}),
    ]
    for label, doc in cases:
        err_buf = io.StringIO()
        try:
            with contextlib.redirect_stderr(err_buf):
                check_main(doc, "fixture")
        except SystemExit as exc:
            if exc.code != 2:
                return (f"{label}: expected SystemExit(2), got SystemExit({exc.code!r}): "
                        f"{err_buf.getvalue()!r}")
            continue
        return f"{label}: check_main returned normally on a scalar env: instead of infra(2)"
    return None


def _v10_rule1_strict_equality() -> str | None:
    """V10 rule 1, the `missing` half (final-review Important 1): EXPECTED_RELEASE_SECRETS is a
    STRICT-EQUALITY pin, so a pinned name that release.yml stops referencing must red too.

    Expressed here rather than as a FIXTURES row because self_test() calls check_main with the
    name "fixture", and that half of the rule deliberately fires only for
    RELEASE_WORKFLOW_NAME — every synthetic fixture legitimately references no secret at all.
    Without this test the `missing` direction would be unasserted, and the pin could rot into a
    subset test without anything reporting it.
    """
    doc = {"jobs": {"release": {"runs-on": "ubuntu-latest", "steps": [{"run": "echo hi"}]}}}

    found = check_main(doc, RELEASE_WORKFLOW_NAME)
    for pinned in EXPECTED_RELEASE_SECRETS:
        if not any(f"no longer references {pinned}" in line for line in found):
            return (f"a release.yml referencing no secret at all did not red for the pinned "
                    f"{pinned}: {found or '(clean)'}")

    # The same document under any OTHER name must stay clean on this rule — otherwise every
    # fixture and every called workflow would inherit release.yml's pin.
    other = [line for line in check_main(doc, "fixture") if "no longer references" in line]
    if other:
        return f"the missing-name half leaked past RELEASE_WORKFLOW_NAME: {other}"

    # And the real referencing shape must satisfy it exactly.
    ok = {"jobs": {"release": {"runs-on": "ubuntu-latest", "steps": [{
        "uses": "actions/create-github-app-token@v2",
        "with": {"app-id": "${{ secrets.PAIGASUS_BOT_APP_ID }}",
                 "private-key": "${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}"},
    }]}}}
    leftover = [line for line in check_main(ok, RELEASE_WORKFLOW_NAME)
                if "EXPECTED_RELEASE_SECRETS" in line or "no longer references" in line]
    if leftover:
        return f"the exact pinned secret set did not satisfy strict equality: {leftover}"
    return None


def _v11_id_token_write_required() -> str | None:
    """V11 (SMA-602 fix wave, F2): both OIDC publish jobs must still grant `id-token: write`.

    Expressed here rather than as a FIXTURES row because self_test() calls check_main with the
    name "fixture", and V11 fires only for RELEASE_WORKFLOW_NAME — a called workflow legitimately
    declares no OIDC grant, and repo:workflow-credentials actively BANS one in any
    pull_request-triggered workflow. The same reason `_v10_rule1_strict_equality` lives here.

    Both directions, and the NARROWED-block direction as well: a job-level `permissions:` block
    sets every scope it omits to `none`, so ADDING one that names only `contents: read` is the
    same defect as deleting the grant, and a rule that only tested for the literal absence of a
    `permissions:` key would miss it.
    """
    def doc_with(pypi_perms, npm_perms, workflow_perms=None):
        out = {"jobs": {
            "publish-pypi": {"permissions": pypi_perms, "steps": [{"run": "echo hi"}]},
            "publish-npm": {"permissions": npm_perms, "steps": [{"run": "echo hi"}]},
        }}
        for jid in ("publish-pypi", "publish-npm"):
            if out["jobs"][jid]["permissions"] is None:
                del out["jobs"][jid]["permissions"]
        if workflow_perms is not None:
            out["permissions"] = workflow_perms
        return out

    grant = {"id-token": "write", "contents": "read"}
    narrowed = {"contents": "read"}

    def v11(doc):
        return [ln for ln in id_token_violations(doc, RELEASE_WORKFLOW_NAME) if ": V11:" in ln]

    if v11(doc_with(grant, grant)):
        return f"the correct shape red: {v11(doc_with(grant, grant))}"
    for jid, bad in (("publish-pypi", (None, grant)), ("publish-npm", (grant, None))):
        found = v11(doc_with(*bad))
        if not any(f"job '{jid}'" in line for line in found):
            return f"deleting {jid}'s permissions: block did not red: {found or '(clean)'}"
    found = v11(doc_with(narrowed, grant))
    if not any("publish-pypi" in line for line in found):
        return f"a narrower permissions: block omitting id-token did not red: {found or '(clean)'}"
    # A workflow-level grant covers a job that declares no block of its own; a job block always
    # wins over it.
    if v11(doc_with(None, None, workflow_perms=grant)):
        return "a workflow-level id-token: write did not satisfy a job declaring no block"
    if not v11(doc_with(narrowed, None, workflow_perms=grant)):
        return "a job-level block omitting id-token was masked by the workflow-level grant"
    # The floor: a renamed job must red rather than pass vacuously.
    renamed = doc_with(grant, grant)
    renamed["jobs"]["publish-pypi-v2"] = renamed["jobs"].pop("publish-pypi")
    if not any("no job named 'publish-pypi'" in line for line in v11(renamed)):
        return "renaming publish-pypi did not red the V11 floor"
    # And the scoping: no other document may inherit release.yml's rule.
    if id_token_violations(doc_with(None, None), "fixture"):
        return "V11 leaked past RELEASE_WORKFLOW_NAME onto a fixture document"
    return None


def _v12_npm_floor_pinned() -> str | None:
    """V12 (SMA-602 fix wave, F3): every NPM_OIDC_FLOOR_LINES entry, in BOTH subjects.

    Here rather than in FIXTURES for the same reason as V11: the rule is scoped by workflow NAME,
    and self_test() names every fixture "fixture". Driven against a synthetic document holding
    exactly the pinned lines, so this asserts the RULE; the real two-file proof is check 10
    running the guard on release.yml, which follows `uses: ./.github/workflows/prebuild.yml` into
    the second copy.
    """
    def doc_with(lines):
        return {"jobs": {"publish-npm": {"steps": [{"run": "\n".join(lines) + "\n"}]}}}

    all_lines = list(NPM_OIDC_FLOOR_LINES)
    for subject in NPM_OIDC_FLOOR_SUBJECTS:
        found = npm_floor_violations(doc_with(all_lines), subject)
        if found:
            return f"{subject}: the complete floor block red: {found}"
        for site in all_lines:
            missing = [ln for ln in all_lines if ln != site]
            found = npm_floor_violations(doc_with(missing), subject)
            if not any(repr(site) in line for line in found):
                return f"{subject}: deleting {site!r} did not red: {found or '(clean)'}"
    # Indented lines must still satisfy the pin: the real ones sit inside a YAML block scalar.
    if npm_floor_violations(doc_with([f"          {ln}" for ln in all_lines]),
                            NPM_OIDC_FLOOR_SUBJECTS[0]):
        return "an indented but complete floor block was reported missing"
    # A workflow that is not a subject must not inherit the requirement.
    if npm_floor_violations({"jobs": {"build": {"steps": [{"run": "echo hi"}]}}}, "wheels.yml"):
        return "V12 leaked onto a workflow that is not in NPM_OIDC_FLOOR_SUBJECTS"
    return None


def _non_list_steps_fails_closed() -> str | None:
    """SMA-602 fix wave, F7: a non-list `steps:` must infra (exit 2), never read as clean.

    Here rather than as a FIXTURES row because a row expects check_main to RETURN a list; this
    scenario must raise, exactly like `_v10_minor6_scalar_env_fails_closed`.

    MEASURED before `steps_of` existed, on `steps: publish`: `publish_credential_violations`
    returned `[]`, `job_publishes` returned False and `napi_violations` returned `[]` — the
    string's characters were iterated, every `continue` fired, and the job read clean. That is
    the PyYAML string-iteration pitfall the `needs:` scalar case already documents (SMA-579),
    and it contradicts this file's own fail-closed convention.
    """
    for label, raw in (("string", "publish"), ("mapping", {"run": "echo hi"}), ("int", 3)):
        err_buf = io.StringIO()
        try:
            with contextlib.redirect_stderr(err_buf):
                check_main({"jobs": {"release": {"steps": raw}}}, "fixture")
        except SystemExit as exc:
            if exc.code != 2:
                return (f"{label} steps:: expected SystemExit(2), got SystemExit({exc.code!r}): "
                        f"{err_buf.getvalue()!r}")
            continue
        return f"{label} steps:: check_main returned normally instead of infra(2)"
    # The two shapes that are NOT malformed must still be accepted: a real list, and a job with
    # no `steps:` at all (a reusable-workflow-call job carries `uses:` instead).
    if steps_of({"uses": "./x.yml"}, "fixture") != []:
        return "a job with no steps: did not yield an empty list"
    if steps_of({"steps": [{"run": "echo hi"}]}, "fixture") != [{"run": "echo hi"}]:
        return "a genuine list steps: was not returned unchanged"
    return None


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
        ("v10 minor-6 scalar env: fails closed", _v10_minor6_scalar_env_fails_closed),
        ("v10 rule 1 strict equality (the missing-name half)", _v10_rule1_strict_equality),
        ("v11 id-token: write on both OIDC publish jobs", _v11_id_token_write_required),
        ("v12 npm OIDC floor pinned in both workflows", _v12_npm_floor_pinned),
        ("f7 non-list steps: fails closed", _non_list_steps_fails_closed),
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
