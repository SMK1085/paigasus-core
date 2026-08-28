# `repo:workflow-credentials` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace P-D6's regex credential scan with a YAML parse in a new, cheap gate that guards every pull-request-triggered workflow, not only `wheels.yml`.

**Architecture:** A Python checker parses each workflow with PyYAML under a duplicate-key-rejecting loader, discovers subjects by trigger, compares them to a strict expected set, and applies four rules to the parsed tree. A bash wrapper supplies PyYAML through a dedicated one-dependency `uv` project and maps the checker's exit codes onto the repo's `0/1/2` contract. P-D6 is then deleted from `repo:publish-metadata`.

**Tech Stack:** bash, Python 3.12+, PyYAML 6, uv 0.11.16, Moon 2.3.2.

**Spec:** `docs/superpowers/specs/2026-08-28-sma-593-p-d6-credential-parse-design.md`

## Global Constraints

- Every new source file opens with `# SPDX-License-Identifier: Apache-2.0`.
- Branch is `feature/sma-593-close-p-d6-credential-spelling-gaps`; commits are conventional with a workspace scope, e.g. `ci(repo): …(SMA-593)`.
- Commit subjects start lowercase and are 100 characters or fewer. Never put a `#NNN` reference or a bare `token: value` line in the commit body — it fails `footer-leading-blank`.
- Exit contract: `0` pass, `1` the repo is wrong, `2` infrastructure. An authorial mistake is **never** rc 2.
- The checker exits `3` for an assertion failure; only `run.sh` maps `3 → 1`.
- `SCAN_GLOB` in the checker and the first `inputs:` entry in `moon.yml` must be the identical string `.github/workflows/*.y*ml`.
- Shell commands need `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` for `moon`/`uv`.
- Two peer sessions edit `ci/affected-graph/ci_targets.py` concurrently. Touch only the entries this plan names; leave every other line byte-identical.

---

### Task 1: The uv project and the wrapper's exit-code contract

Creates the dependency route and proves the `3 → 1` mapping before any rule exists.

**Files:**
- Create: `ci/workflow-credentials/pyproject.toml`
- Create: `ci/workflow-credentials/uv.lock` (generated)
- Create: `ci/workflow-credentials/workflow_credentials.py`
- Create: `ci/workflow-credentials/run.sh`

**Interfaces:**
- Produces: `run.sh` modes `--self-test`, `--negative-control`, and a bare run. `workflow_credentials.py` exits `0|2|3`.

- [ ] **Step 1: Create the uv project**

`ci/workflow-credentials/pyproject.toml`:

```toml
# SPDX-License-Identifier: Apache-2.0
# A DEDICATED one-dependency project, deliberately not the py/ workspace. py/ is a
# [tool.uv.workspace] root whose member paigasus-kernel depends on paigasus-py-bindings by
# path, and that crate builds with maturin — so `uv run --project py` compiles a PyO3 cdylib.
# This gate's inputs are .github/workflows/*.y*ml, so that cost would land on every
# workflow-edit PR, which is exactly what moving the check out of repo:publish-metadata
# avoided. Measured here: 0.073s warm, 0.959s cold. (SMA-593 spec §3 Decision A.)
[project]
name = "paigasus-workflow-credentials"
version = "0.0.0"
description = "Credential guard for pull-request-triggered GitHub Actions workflows"
requires-python = ">=3.12"
dependencies = ["pyyaml>=6.0.3,<7"]
```

- [ ] **Step 2: Generate the lockfile**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ci/workflow-credentials && uv lock && grep -c '^\[\[package\]\]' uv.lock
```

Expected: `2` (the project plus pyyaml).

- [ ] **Step 3: Write a checker stub that exercises all three exit codes**

`ci/workflow-credentials/workflow_credentials.py`:

```python
# SPDX-License-Identifier: Apache-2.0
"""Assert no pull-request-triggered workflow can obtain a repository credential.

Exit codes are DELIBERATELY not the repo's usual 0/1/2. This process exits 3 for an
assertion failure so that `uv`'s own rc 1 — measured on a failed resolution, online and
under UV_OFFLINE=1 — cannot be mistaken for "a workflow declares a credential". run.sh
maps 3 -> 1 and everything else -> 2. (SMA-593 spec §6.)
"""

from __future__ import annotations

import sys

RC_OK = 0
RC_INFRA = 2
RC_ASSERT = 3


class InfraError(Exception):
    """The check could not run. Maps to RC_INFRA."""


def main(argv: list[str]) -> int:
    if argv[1:2] == ["--exit-code-probe"]:
        # Used only by run.sh's negative control to prove the 3 -> 1 mapping is wired.
        return int(argv[2])
    raise InfraError("not implemented yet")


if __name__ == "__main__":
    try:
        raise SystemExit(main(sys.argv))
    except InfraError as exc:
        print(f"workflow-credentials: {exc}", file=sys.stderr)
        raise SystemExit(RC_INFRA) from exc
```

- [ ] **Step 4: Write the wrapper**

`ci/workflow-credentials/run.sh`:

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# repo:workflow-credentials — assert no pull-request-triggered workflow can obtain a
# repository credential. Same-repo pull requests receive repository secrets, so a credential
# in such a workflow is readable by any code the pull request introduces (SMA-407 §7 M2).
#
# Exit codes: 0 pass | 1 the repo is wrong | 2 infrastructure failed.
#
# The checker exits 3, not 1, for an assertion failure. `uv` exits 1 on its own failures —
# MEASURED on a failed resolution both online and with UV_OFFLINE=1 — so without a distinct
# code a PyPI outage would report "a workflow declares a credential". This wrapper owns the
# translation and nothing else may.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$REPO_ROOT/ci/workflow-credentials"

die_infra() { printf 'workflow-credentials: %s\n' "$*" >&2; exit 2; }

# Preflight. `uv` absent yields 127 from the shell, which is neither 0/1/2 nor actionable.
command -v uv >/dev/null 2>&1 \
  || die_infra "uv is not on PATH — run 'proto install', or add ~/.proto/shims to PATH"

# $@ is forwarded to the checker. Returns 0, returns 1 for a real assertion failure, and
# EXITS 2 for anything else.
run_checker() {
  local rc=0
  uv run --project "$HERE" --python '>=3.12' python3 \
    "$HERE/workflow_credentials.py" "$@" || rc=$?
  case "$rc" in
    0) return 0 ;;
    3) return 1 ;;
    *) die_infra "checker exited $rc — uv or the interpreter failed, not an assertion" ;;
  esac
}

MODE=check
while [ $# -gt 0 ]; do
  case "$1" in
    --self-test)        MODE=selftest; shift ;;
    --negative-control) MODE=negctl;   shift ;;
    *) die_infra "unknown flag: $1" ;;
  esac
done

case "$MODE" in
  selftest) run_checker --self-test ;;
  check)    run_checker "$REPO_ROOT" ;;
  negctl)   negative_control ;;
esac
```

Note: `negative_control` is added in Task 5. Until then, run only `--self-test` and the bare mode.

- [ ] **Step 5: Prove the exit-code mapping by hand**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --project ci/workflow-credentials --python '>=3.12' python3 \
  ci/workflow-credentials/workflow_credentials.py --exit-code-probe 3; echo "checker rc=$?"
uv run --project ci/workflow-credentials --python '>=3.12' python3 \
  ci/workflow-credentials/workflow_credentials.py --exit-code-probe 0; echo "checker rc=$?"
```

Expected: `checker rc=3` then `checker rc=0`.

- [ ] **Step 6: Commit**

```bash
chmod +x ci/workflow-credentials/run.sh
git add ci/workflow-credentials/
git commit -m "ci(repo): scaffold the workflow-credentials gate and its uv project (SMA-593)"
```

---

### Task 2: Parsing, the strict loader, and the four rules

**Files:**
- Modify: `ci/workflow-credentials/workflow_credentials.py`

**Interfaces:**
- Produces: `load_documents(path) -> list`, `rule_findings(doc) -> list[tuple[str, str, str]]` returning `(rule_id, yaml_path, message)`, and the constants `SCAN_GLOB`, `EXPR_SPAN`, `STRING_LITERAL`, `SECRETS_CTX`.

- [ ] **Step 1: Write the failing self-test table**

Append to `workflow_credentials.py`:

```python
H = "on:\n  pull_request:\njobs:\n  a:\n"

RULE_CASES: tuple[tuple[str, str, bool], ...] = (
    # (label, yaml, must_be_red) — the 14 MEASURED bypasses of the old regex checker.
    ("01 backslash escape",  H + '    permissions: { note: "a \\" # z", id-token: write }\n', True),
    ("02 single-quoted val", H + "    permissions:\n      id-token: 'write'\n", True),
    ("03 quoted key",        H + '    permissions:\n      "id-token": write\n', True),
    ("04 bracket index",     H + "    env:\n      T: ${{ secrets['PYPI_API_TOKEN'] }}\n", True),
    ("05 context case",      H + "    env:\n      T: ${{ Secrets.PYPI_API_TOKEN }}\n", True),
    ("06 double-quoted val", H + '    permissions:\n      id-token: "write"\n', True),
    ("07 single-quoted key", H + "    permissions:\n      'id-token': write\n", True),
    ("08 quoted secrets",    H + '    "secrets": inherit\n', True),
    ("09 double-bracket",    H + '    env:\n      T: ${{ secrets["PYPI_API_TOKEN"] }}\n', True),
    ("10 uppercase context", H + "    env:\n      T: ${{ SECRETS.PYPI_API_TOKEN }}\n", True),
    ("11 spaced bracket",    H + "    env:\n      T: ${{ secrets[ 'X' ] }}\n", True),
    ("12 write-all workflow", "on:\n  pull_request:\npermissions: write-all\njobs:\n  a:\n    runs-on: x\n", True),
    ("13 write-all job",     H + "    permissions: write-all\n", True),
    ("14 yaml alias",        "on:\n  pull_request:\nx: &w write\njobs:\n  a:\n    permissions:\n      id-token: *w\n", True),
    # The six the OLD checker already caught. Regression pins: the redesign must not trade
    # old coverage for new. The merge-key row documents an input Actions REJECTS (anchors
    # shipped 2025-09-18, merge keys did not), so it is over-coverage, not a closed hole.
    ("A block scalar run",   H + "    steps:\n      - run: |\n          echo ${{ secrets.X }} # literal\n", True),
    ("B no-space expr",      H + "    env:\n      T: ${{secrets.X}}\n", True),
    ("C quoted whole value", H + '    env:\n      T: "${{ secrets.X }}"\n', True),
    ("D flow permissions",   H + "    permissions: {id-token: write}\n", True),
    ("E workflow_call secrets", "on:\n  workflow_call:\n    secrets:\n      PYPI_TOKEN:\n  pull_request:\n", True),
    ("F merge key",          "on:\n  pull_request:\nx: &p\n  id-token: write\njobs:\n  a:\n    permissions:\n      <<: *p\n", True),
    # Expressions the FIRST design missed. Both read real secrets.
    ("G format()",           H + "    env:\n      T: ${{ format('{0}', secrets.X) }}\n", True),
    ("H toJSON(secrets)",    H + "    env:\n      T: ${{ toJSON(secrets) }}\n", True),
    # Honest passes. A first false positive is how a gate gets allowlisted into irrelevance.
    ("P1 contents read",     H + "    permissions:\n      contents: read\n", False),
    ("P2 header comment",    "# never declare `secrets:` or `id-token: write`\n" + H + "    permissions:\n      contents: read\n", False),
    ("P3 hash in a scalar",  H + '    name: "sharp # sign"\n    permissions:\n      contents: read\n', False),
    ("P4 prose says secrets", H + '    name: "scan for secrets in the tree"\n', False),
    ("P5 read-all",          H + "    permissions: read-all\n", False),
    ("P6 id-token none",     H + "    permissions:\n      id-token: none\n", False),
    # R4's MEASURED false-positive class. All three matched a bare \bsecrets\b.
    ("P7 inputs.secrets-file", H + "    env:\n      T: ${{ inputs.secrets-file }}\n", False),
    ("P8 outputs.secrets",   H + "    env:\n      T: ${{ steps.x.outputs.secrets }}\n", False),
    ("P9 hashFiles literal", H + "    env:\n      T: ${{ hashFiles('secrets.txt') }}\n", False),
)
```

- [ ] **Step 2: Run it to verify it fails**

```bash
bash ci/workflow-credentials/run.sh --self-test
```

Expected: rc 2, `not implemented yet`.

- [ ] **Step 3: Implement parsing and the rules**

Replace the stub `main` and add above it:

```python
import glob
import os
import re

import yaml

SCAN_GLOB = ".github/workflows/*.y*ml"

# An Actions expression span, then the literals inside it, then the context name.
# Stripping literals is what stops hashFiles('secrets.txt') matching; it does NOT weaken
# secrets['X'], because the context name sits OUTSIDE the literal. The boundary rejects
# inputs.secrets-file (preceded by '.') and steps.x.outputs.secrets (likewise). All three
# were MEASURED as false positives against a bare \bsecrets\b. (spec §4.3)
EXPR_SPAN = re.compile(r"\$\{\{(.*?)\}\}", re.S)
STRING_LITERAL = re.compile(r"'[^']*'|\"[^\"]*\"")
SECRETS_CTX = re.compile(r"(?<![\w.-])secrets(?![\w-])", re.IGNORECASE)


class AssertionFailure(Exception):
    """The repo is wrong. Maps to RC_ASSERT."""


class _StrictLoader(yaml.SafeLoader):
    """SafeLoader that refuses duplicate mapping keys.

    PyYAML's default is LAST WINS, which is a regression against the regex this replaces:

        permissions: {id-token: write}     <- silently discarded
        permissions: {contents: read}

    MEASURED: safe_load returns only the second mapping, so R2/R3 never see the grant, while
    the old text scan matched it. (spec §4.1)
    """


def _no_duplicate_keys(loader, node, deep=False):
    seen = set()
    for key_node, _ in node.value:
        key = loader.construct_object(key_node, deep=deep)
        if key in seen:
            raise yaml.constructor.ConstructorError(
                "while constructing a mapping", node.start_mark,
                f"duplicate key {key!r}", key_node.start_mark)
        seen.add(key)
    return yaml.SafeLoader.construct_mapping(loader, node, deep=deep)


_StrictLoader.add_constructor(
    yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, _no_duplicate_keys)


def load_documents(path: str) -> list:
    """Every YAML document in one workflow. OSError is infra; bad YAML is the repo's fault."""
    try:
        with open(path, encoding="utf-8") as handle:
            return list(yaml.load_all(handle, Loader=_StrictLoader))
    except OSError as exc:
        raise InfraError(f"cannot read {path}: {exc}") from exc
    except yaml.YAMLError as exc:
        raise AssertionFailure(f"{os.path.basename(path)} is not valid YAML: {exc}") from exc


def _mapping_entries(node, path="$"):
    if isinstance(node, dict):
        for key, value in node.items():
            here = f"{path}.{key}"
            yield here, key, value
            yield from _mapping_entries(value, here)
    elif isinstance(node, list):
        for index, value in enumerate(node):
            yield from _mapping_entries(value, f"{path}[{index}]")


def _scalar_strings(node, path="$"):
    if isinstance(node, dict):
        for key, value in node.items():
            here = f"{path}.{key}"
            if isinstance(key, str):
                yield here, key
            yield from _scalar_strings(value, here)
    elif isinstance(node, list):
        for index, value in enumerate(node):
            yield from _scalar_strings(value, f"{path}[{index}]")
    elif isinstance(node, str):
        yield path, node


def _lower(value):
    return value.strip().lower() if isinstance(value, str) else None


def rule_findings(doc) -> list[tuple[str, str, str]]:
    """(rule_id, yaml_path, message) for every violation in one parsed document."""
    out: list[tuple[str, str, str]] = []
    for where, key, value in _mapping_entries(doc):
        # Keys are compared case-SENSITIVELY: Actions reads schema keys that way, so
        # `SECRETS:` is not a key GitHub honours. Values are lowered, which only ever adds
        # a conservative red.
        if key == "secrets":
            out.append(("R1", where, "declares a `secrets` key"))
        if key == "id-token" and _lower(value) == "write":
            out.append(("R2", where, "grants `id-token: write`"))
        if key == "permissions" and _lower(value) == "write-all":
            out.append(("R3", where, "grants `permissions: write-all`, which includes id-token"))
    for where, text in _scalar_strings(doc):
        for span in EXPR_SPAN.findall(text):
            if SECRETS_CTX.search(STRING_LITERAL.sub("", span)):
                out.append(("R4", where, "reads the `secrets` context"))
                break
    return out


def _self_test() -> int:
    failures = 0
    for label, source, must_be_red in RULE_CASES:
        try:
            findings = [f for doc in yaml.load_all(source, Loader=_StrictLoader)
                        if isinstance(doc, dict) for f in rule_findings(doc)]
        except yaml.YAMLError as exc:
            print(f"  FAIL {label}: parse error {exc}", file=sys.stderr)
            failures += 1
            continue
        is_red = bool(findings)
        if is_red != must_be_red:
            want = "RED" if must_be_red else "pass"
            got = ",".join(sorted({f[0] for f in findings})) or "pass"
            print(f"  FAIL {label}: expected {want}, got {got}", file=sys.stderr)
            failures += 1
    if failures:
        print(f"self-test: {failures} of {len(RULE_CASES)} rows failed", file=sys.stderr)
        return RC_ASSERT
    print(f"== workflow-credentials self-test passed ({len(RULE_CASES)} rows) ==")
    return RC_OK
```

Replace `main` with:

```python
def main(argv: list[str]) -> int:
    if argv[1:2] == ["--self-test"]:
        return _self_test()
    raise InfraError("not implemented yet")
```

and extend the `__main__` guard to catch `AssertionFailure`:

```python
if __name__ == "__main__":
    try:
        raise SystemExit(main(sys.argv))
    except AssertionFailure as exc:
        print(f"workflow-credentials FAILED: {exc}", file=sys.stderr)
        raise SystemExit(RC_ASSERT) from exc
    except InfraError as exc:
        print(f"workflow-credentials: {exc}", file=sys.stderr)
        raise SystemExit(RC_INFRA) from exc
    except Exception as exc:  # noqa: BLE001 — an unexpected crash is INFRA, never an assertion
        print(f"workflow-credentials: unexpected {type(exc).__name__}: {exc}", file=sys.stderr)
        raise SystemExit(RC_INFRA) from exc
```

- [ ] **Step 4: Run the self-test**

```bash
bash ci/workflow-credentials/run.sh --self-test
```

Expected: `== workflow-credentials self-test passed (31 rows) ==`, rc 0.

- [ ] **Step 5: Prove the table can fail**

Temporarily change `SECRETS_CTX` to `re.compile(r"NEVERMATCHES")`, re-run, confirm the R4 rows fail, then revert with `git checkout -- ci/workflow-credentials/workflow_credentials.py` **only if nothing else is uncommitted** — otherwise revert by hand.

- [ ] **Step 6: Commit**

```bash
git add ci/workflow-credentials/workflow_credentials.py
git commit -m "ci(repo): parse workflow yaml and apply the four credential rules (SMA-593)"
```

---

### Task 3: Discovery, the subject pin, and the allowlist

**Files:**
- Modify: `ci/workflow-credentials/workflow_credentials.py`

**Interfaces:**
- Produces: `triggers(doc) -> set[str]`, `discover(root) -> list[str]`, and the constants `EXPECTED_PR_SUBJECTS`, `PR_TRIGGERS`, `PR_CREDENTIAL_ALLOWED`.

- [ ] **Step 1: Write the failing discovery self-test**

Append to `workflow_credentials.py`:

```python
TRIGGER_CASES: tuple[tuple[str, str, bool], ...] = (
    ("mapping form",    "on:\n  pull_request:\njobs: {}\n", True),
    ("list form",       "on: [push, pull_request]\njobs: {}\n", True),
    ("string form",     "on: pull_request\njobs: {}\n", True),
    ("pull_request_target", "on:\n  pull_request_target:\njobs: {}\n", True),
    ("push only",       "on:\n  push:\n    branches:\n      - main\njobs: {}\n", False),
    ("bare on",         "on:\njobs: {}\n", False),
    ("no on key",       "jobs: {}\n", False),
)

# Documents that must not reach the rules at all. Each must raise, and raise the RIGHT class:
# these are the repo being wrong, not infrastructure. The duplicate-key row is the important
# one — PyYAML's default is LAST WINS, so without the strict loader the first `permissions`
# block below is silently discarded and R2 never sees the grant that the OLD regex caught.
PARSE_CASES: tuple[tuple[str, str], ...] = (
    ("duplicate key drops a grant",
     "on:\n  pull_request:\njobs:\n  a:\n    permissions:\n      id-token: write\n"
     "    permissions:\n      contents: read\n"),
    ("top-level sequence", "- on: pull_request\n"),
    ("malformed yaml", "on:\n  pull_request:\n  bad: [unclosed\n"),
)
```

and extend `_self_test` before its `if failures:` block:

```python
    for label, source, expected in TRIGGER_CASES:
        docs = list(yaml.load_all(source, Loader=_StrictLoader))
        got = any(triggers(d) & PR_TRIGGERS for d in docs if isinstance(d, dict))
        if got != expected:
            print(f"  FAIL trigger/{label}: expected {expected}, got {got}", file=sys.stderr)
            failures += 1

    for label, source in PARSE_CASES:
        try:
            docs = list(yaml.load_all(source, Loader=_StrictLoader))
        except yaml.YAMLError:
            continue  # rejected at parse time, which is what we want
        if all(isinstance(d, dict) for d in docs if d is not None):
            print(f"  FAIL parse/{label}: accepted a document that must be rejected",
                  file=sys.stderr)
            failures += 1
```

`discover` turns the surviving non-mapping case into an `AssertionFailure`; the loader turns
the other two into `yaml.YAMLError`, which `load_documents` also re-raises as an
`AssertionFailure`. Both reach the caller as rc 3, never rc 2.

and change the pass message to count both tables:

```python
    total = len(RULE_CASES) + len(TRIGGER_CASES) + len(PARSE_CASES)
    print(f"== workflow-credentials self-test passed ({total} rows) ==")
```

- [ ] **Step 2: Run to verify it fails**

```bash
bash ci/workflow-credentials/run.sh --self-test
```

Expected: rc 2, `unexpected NameError: name 'triggers' is not defined`.

- [ ] **Step 3: Implement discovery and the check**

```python
PR_TRIGGERS = frozenset({"pull_request", "pull_request_target"})

# The subject set, pinned by STRICT EQUALITY. run.sh's sibling gate holds two discovered sets
# the same way (EXPECTED_PUBLISHABLE, EXPECTED_PYPI_PUBLISHABLE) and Check P0 states the
# reason: a stale list silently SHRINKS the gate rather than reporting red. A new
# pull-request-triggered workflow reds here until someone adds it, deliberately. (spec §5.2)
EXPECTED_PR_SUBJECTS = (
    "ci.yml",
    "images.yml",
    "prebuild.yml",
    "security-scan.yml",
    "wheels.yml",
)

# (workflow filename, rule id) -> why a human accepted it. Keyed by BOTH, never by filename
# alone: a file-level key would let the entry permitting one build-arg also permit
# `id-token: write` and a token read in the same file, silently and forever. (spec §5.2)
PR_CREDENTIAL_ALLOWED: dict[tuple[str, str], str] = {}


def triggers(doc) -> set[str]:
    """The trigger names in one document.

    YAML 1.1 parses the `on:` KEY as the boolean True, so doc.get("on") returns None.
    MEASURED on release.yml: top-level keys are ['name', True, 'concurrency', 'permissions',
    'jobs']; `'on' in doc` is False and `True in doc` is True. Read both. (spec §5.1)
    """
    if not isinstance(doc, dict):
        return set()
    on = doc.get("on", doc.get(True))
    if isinstance(on, str):
        return {on}
    if isinstance(on, (list, dict)):
        return {str(x) for x in on}
    return set()


def discover(root: str) -> list[str]:
    """Basenames of every workflow whose triggers make it a subject."""
    paths = sorted(glob.glob(os.path.join(root, SCAN_GLOB)))
    if not paths:
        raise InfraError(
            f"{SCAN_GLOB} matched no file under {root} — the scan root moved and this gate "
            "would assert nothing")
    subjects = []
    for path in paths:
        docs = load_documents(path)
        for doc in docs:
            if doc is not None and not isinstance(doc, dict):
                raise AssertionFailure(
                    f"{os.path.basename(path)}: top-level YAML is not a mapping")
        if any(triggers(d) & PR_TRIGGERS for d in docs if isinstance(d, dict)):
            subjects.append(os.path.basename(path))
    return subjects


def check(root: str) -> int:
    subjects = discover(root)
    # Compared BEFORE any allowlist: the allowlist suppresses rule verdicts, never
    # membership. Counting after it would let "allowlist everything" pass. (spec §5.2)
    if tuple(subjects) != EXPECTED_PR_SUBJECTS:
        raise AssertionFailure(
            f"pull-request-triggered workflows are {subjects}, expected "
            f"{list(EXPECTED_PR_SUBJECTS)} — re-baseline EXPECTED_PR_SUBJECTS deliberately")

    stale = [name for (name, _rule) in PR_CREDENTIAL_ALLOWED if name not in subjects]
    if stale:
        raise AssertionFailure(
            f"PR_CREDENTIAL_ALLOWED names workflows that are not subjects: {sorted(set(stale))}")

    reds: list[str] = []
    for name in subjects:
        for doc in load_documents(os.path.join(root, ".github", "workflows", name)):
            if not isinstance(doc, dict):
                continue
            for rule, where, message = rule_findings(doc):
                reason = PR_CREDENTIAL_ALLOWED.get((name, rule))
                if reason:
                    print(f"  allowed {name} [{rule}] at {where}: {reason}")
                    continue
                reds.append(f"  {name} [{rule}] at {where}: {message}")
    if reds:
        raise AssertionFailure(
            "a pull-request-triggered workflow can obtain a repository credential:\n"
            + "\n".join(reds)
            + "\n  A same-repo pull request receives repository secrets, so this is readable "
              "by any code the PR introduces. Publishing belongs in a workflow with no "
              "pull_request trigger (SMA-407 §7 review M2).")
    print(f"workflow-credentials: {len(subjects)} pull-request-triggered workflow(s) "
          "carry no credential")
    return RC_OK
```

Update `main`:

```python
def main(argv: list[str]) -> int:
    if argv[1:2] == ["--self-test"]:
        return _self_test()
    if len(argv) != 2:
        raise InfraError("usage: workflow_credentials.py <repo-root> | --self-test")
    return check(argv[1])
```

Delete the `--exit-code-probe` branch; Task 5 proves the mapping through a real path instead.

- [ ] **Step 4: Run both modes**

```bash
bash ci/workflow-credentials/run.sh --self-test
bash ci/workflow-credentials/run.sh
```

Expected: self-test passes with 41 rows; the real run prints `5 pull-request-triggered workflow(s) carry no credential`, rc 0.

- [ ] **Step 5: Prove the subject pin bites**

```bash
sed -i '' 's/^    "images.yml",$//' ci/workflow-credentials/workflow_credentials.py
bash ci/workflow-credentials/run.sh; echo "rc=$?"
```

Expected: rc 1, message naming the mismatch. Restore the line by hand afterwards and re-run to confirm rc 0.

- [ ] **Step 6: Commit**

```bash
git add ci/workflow-credentials/workflow_credentials.py
git commit -m "ci(repo): discover pull-request-triggered workflows and pin the subject set (SMA-593)"
```

---

### Task 4: The gate's README

**Files:**
- Create: `ci/workflow-credentials/README.md`

- [ ] **Step 1: Write it**

Cover, in this order: what the gate asserts and why the trigger makes it matter; the four rules with one example each; discovery and the `on:`→`True` trap; `EXPECTED_PR_SUBJECTS` and how to re-baseline; `PR_CREDENTIAL_ALLOWED`'s `(file, rule)` key and that each entry states what a human verified; the exit codes including the checker's 3 and why it is not 1; the dependency route and where the PyYAML pin lives; the non-goals from spec §7 verbatim, **including** R4's residual false-positive surface; and the note that SMA-579's V6 reachability rule is complementary, not redundant.

Open the file with `<!-- SPDX-License-Identifier: Apache-2.0 -->`.

- [ ] **Step 2: Commit**

```bash
git add ci/workflow-credentials/README.md
git commit -m "docs(ci): document the workflow-credentials gate (SMA-593)"
```

---

### Task 5: The negative control

**Files:**
- Modify: `ci/workflow-credentials/run.sh`

**Interfaces:**
- Consumes: `run_checker` from Task 1; `check`/`discover` behaviour from Task 3.
- Produces: `negative_control`, referenced by the `case` block already written in Task 1.

- [ ] **Step 1: Add the function above the `MODE=check` line**

```bash
# The wiring rows — only what needs the real tree. The rule table lives in the checker's
# --self-test, in-process, because ~31 rows through `uv run` would be ~31 subprocesses.
#
# These five discrete lines are pinned by ci_targets.py. Pinning the moon.yml INVOCATION
# alone is not enough: the repo measured two bypasses of exactly that shape on
# ci/release-parity/run.sh — neutering the flag parse so --negative-control falls through to
# the real suite, and gutting the assertion body so the control prints "reported red as
# expected" while calling nothing.
negative_control() {
  local failures=0 tmp rc
  tmp="$(mktemp -d)"

  _expect() { # $1 expected rc, $2 label, then the command
    local want="$1" label="$2"; shift 2
    local got=0
    "$@" >/dev/null 2>&1 || got=$?
    if [ "$got" != "$want" ]; then
      printf '  FAIL %s: expected rc %s, got %s\n' "$label" "$want" "$got" >&2
      failures=$((failures + 1))
    fi
  }

  # A tree with NO workflows at all is infrastructure, not a pass: the scan root moved.
  mkdir -p "$tmp/empty/.github/workflows"
  _expect 2 "an empty workflow dir is INFRA, not a vacuous pass" \
    uv run --project "$HERE" --python '>=3.12' python3 \
      "$HERE/workflow_credentials.py" "$tmp/empty"

  # A tree whose workflows exist but disagree with EXPECTED_PR_SUBJECTS is the repo's fault.
  mkdir -p "$tmp/one/.github/workflows"
  printf 'on:\n  pull_request:\njobs:\n  a:\n    runs-on: x\n' \
    >"$tmp/one/.github/workflows/ci.yml"
  _expect 3 "a shrunken subject set reds against the strict pin" \
    uv run --project "$HERE" --python '>=3.12' python3 \
      "$HERE/workflow_credentials.py" "$tmp/one"

  # THE key row. release.yml fails the credential rules and passes the gate ONLY because
  # discovery excludes it — it has no pull_request trigger. Asserting both halves is what
  # proves the trigger filter does real work rather than decorating.
  # If this reds: re-baseline. It means release.yml no longer reads a secret.
  rc=0
  grep -qE '\$\{\{[[:space:]]*secrets\.' "$REPO_ROOT/.github/workflows/release.yml" || rc=1
  if [ "$rc" != 0 ]; then
    printf '  FAIL release.yml no longer reads a secret — re-baseline this control row\n' >&2
    failures=$((failures + 1))
  fi
  if bash "$0" 2>/dev/null | grep -q 'release.yml'; then
    printf '  FAIL release.yml appeared in the subject set; it has no pull_request trigger\n' >&2
    failures=$((failures + 1))
  fi

  # The 3 -> 1 translation itself. A checker rc 3 must reach the caller as 1, not 3.
  _expect 1 "the wrapper maps a checker assertion (3) onto the repo contract (1)" \
    run_checker "$tmp/one"

  rm -rf "$tmp"
  if [ "$failures" -gt 0 ]; then
    printf 'workflow-credentials negative control: %d row(s) failed\n' "$failures" >&2
    exit 1
  fi
  printf '== workflow-credentials negative control passed ==\n'
}
```

- [ ] **Step 2: Run it**

```bash
bash ci/workflow-credentials/run.sh --negative-control
```

Expected: `== workflow-credentials negative control passed ==`, rc 0.

- [ ] **Step 3: Commit**

```bash
git add ci/workflow-credentials/run.sh
git commit -m "ci(repo): add the workflow-credentials negative control (SMA-593)"
```

---

### Task 6: Wire the gate into Moon, `ci.yml`, CLAUDE.md and the five registries

These land together: CI is red between them. `repo:affected-smoke` asserts every `T` entry resolves to a real task, and the registries must agree with `moon.yml` in the same commit.

**Files:**
- Modify: `moon.yml` (new task; `repo:affected-smoke` inputs; `repo:osv` inputs)
- Modify: `.github/workflows/ci.yml:214`
- Modify: `CLAUDE.md` (marker-delimited command)
- Modify: `ci/affected-graph/ci_targets.py`
- Modify: `ci/actionlint/run.sh:2097-2117`

- [ ] **Step 1: Add the Moon task**

Insert into `moon.yml`, keeping alphabetical placement among the `repo:` tasks:

```yaml
  workflow-credentials:
    description: 'Assert no pull-request-triggered workflow declares a registry credential (SMA-593).'
    # WHY THIS EXISTS — a same-repo pull request receives repository secrets, so a credential
    # in a pull_request-triggered workflow is readable by any code that PR introduces
    # (SMA-407 §7 review M2). This was P-D6 inside repo:publish-metadata, guarding wheels.yml
    # alone with three regexes over comment-stripped text; SMA-593 measured FOURTEEN bypasses,
    # including `permissions: write-all` and a YAML alias that no text scan reaches.
    #
    # WHY ITS OWN GATE — repo:publish-metadata runs `cargo publish --dry-run` per publish
    # group. Widening THAT gate to every workflow would put a cargo build on every ci.yml
    # edit, on a required check. This gate is a sub-second YAML parse instead.
    #
    # The first glob is IDENTICAL to workflow_credentials.py's SCAN_GLOB on purpose:
    # scheduling and scanning must not drift apart. `.y*ml` covers BOTH extensions — Actions
    # accepts .yaml, and globbing .yml alone is a complete bypass by rename.
    #
    # `--self-test` and `--negative-control` run FIRST and in the SAME block: a gate that
    # cannot report red is worse than no gate. `set -euo pipefail` is REQUIRED — Moon does
    # not enable errexit for `script:` blocks, so without it a failing control is masked by
    # the passing real run. These four lines are pinned by SELF_SCHEDULED_GATES.
    script: |
      set -euo pipefail
      bash ci/workflow-credentials/run.sh --self-test
      bash ci/workflow-credentials/run.sh --negative-control
      bash ci/workflow-credentials/run.sh
    toolchain: 'system'
    inputs:
      - '.github/workflows/*.y*ml'
      - 'ci/workflow-credentials/**/*'
```

- [ ] **Step 2: Confirm how Moon resolves those inputs BEFORE writing the registry tuple**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon query tasks --affected 2>/dev/null | head -1 || true
moon query projects | python3 -c "
import json,sys
d=json.load(sys.stdin)
for p in d.get('projects',[]):
    if p.get('id')=='repo':
        t=p['tasks']['workflow-credentials']
        print('inputGlobs:', sorted(t.get('inputGlobs') or {}))
        print('inputFiles:', sorted(t.get('inputFiles') or {}))
"
```

Both entries are wildcards, so both should land in `inputGlobs` and `inputFiles` should be empty. **If the measurement disagrees, write the tuple from the measurement, not from this plan.**

- [ ] **Step 3: Add the four `ci_targets.py` entries**

In `SELF_TASK_EXPECTED_GLOBS`, add — globs first (sorted), then files (sorted):

```python
    # SMA-593. Two globs, no literal files. The first is deliberately identical to
    # workflow_credentials.py's SCAN_GLOB (moon.yml says so); the second is what makes the
    # negative-control pin below reachable — drop it and the pin stays green on exactly the
    # PR that breaks it.
    "workflow-credentials": (
        ".github/workflows/*.y*ml",
        "ci/workflow-credentials/**/*",
    ),
```

In `SELF_SCHEDULED_GATES`, add:

```python
    # SMA-593. FOUR lines: a self-test invocation as well as the control, like
    # version-lockstep. All three modes go through run.sh so each gets the uv preflight —
    # `python3 workflow_credentials.py` directly would fail on `import yaml`.
    "workflow-credentials": (
        "set -euo pipefail",
        "bash ci/workflow-credentials/run.sh --self-test",
        "bash ci/workflow-credentials/run.sh --negative-control",
        "bash ci/workflow-credentials/run.sh",
    ),
```

In `REQUIRED_REPO_TASKS`, add `"workflow-credentials"` in alphabetical position (last).

In `SELF_TASK_EXPECTED_GLOBS["publish-metadata"]`, **delete** the line
`".github/workflows/wheels.yml",` and rewrite the explaining comment at `:218-226` so it no longer says P-D6 reads that file. Leave every other line of that tuple byte-identical — a peer session is editing this file.

- [ ] **Step 4: Pin the negative-control body**

Add a `WORKFLOW_CREDENTIALS_SH_CALL_SITES` tuple next to `RELEASE_PARITY_SH_CALL_SITES`, pinning these discrete lines of `ci/workflow-credentials/run.sh` (whole lines, compared after stripping):

```python
WORKFLOW_CREDENTIALS_SH_CALL_SITES = (
    "--negative-control) MODE=negctl;   shift ;;",
    'negctl)   negative_control ;;',
    'if [ "$failures" -gt 0 ]; then',
    "exit 1",
)
```

Wire it into the same check that consumes `RELEASE_PARITY_SH_CALL_SITES`, following that function exactly.

- [ ] **Step 5: Make the pin reachable, and floor it**

In `moon.yml`, add `'ci/workflow-credentials/**/*'` to `repo:affected-smoke`'s `inputs`. Then add the same glob to `T_AFFECTED_SMOKE_REQUIRED_INPUTS` in `ci/actionlint/run.sh:2097-2117`, immediately after `'ci/release-parity/**/*'` — that table is a containment floor, and the two other script-pinned gates are in it for exactly this reason.

Also add `'ci/workflow-credentials/uv.lock'` to `repo:osv`'s `inputs` so the new lockfile is scanned.

- [ ] **Step 6: Add the target to `T` and to CLAUDE.md, in the same position**

`.github/workflows/ci.yml:214` — append ` :workflow-credentials` to the `T=(…)` array. It must stay a **single-line** bash array.

`CLAUDE.md` — add `:workflow-credentials` at the **same position** inside the `ci-targets` marker block. `check_docs` compares the two as ordered sequences. Do **not** create a second copy of the markers or the command anywhere in the file.

- [ ] **Step 7: Verify the whole graph**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:workflow-credentials --force
moon run repo:affected-smoke --force
moon run repo:actionlint --force
moon run repo:input-liveness --force
```

Expected: all four green. If `repo:input-liveness` reds on `.github/workflows/*.y*ml`, Moon's matcher rejected the pattern — fall back to two globs `*.yml` and `*.yaml` plus an `ALLOW_DEAD_INPUT` entry for the `.yaml` one, and update both registry tuples to match.

- [ ] **Step 8: Commit**

```bash
git add moon.yml .github/workflows/ci.yml CLAUDE.md ci/affected-graph/ci_targets.py ci/actionlint/run.sh
git commit -m "ci(repo): schedule repo:workflow-credentials and pin its registries (SMA-593)"
```

---

### Task 7: Delete P-D6 and correct every citation it leaves behind

**Files:**
- Modify: `ci/publish-metadata/run.sh` (four regions)
- Modify: `ci/publish-metadata/README.md`
- Modify: `moon.yml:497` and the `publish-metadata` inputs
- Modify: `CLAUDE.md:351`
- Modify: `.github/workflows/wheels.yml:12`

- [ ] **Step 1: Delete the four regions of `ci/publish-metadata/run.sh`**

Highest line number first, so earlier deletions do not shift later ones:

1. `:1847-1848` — the real-run call site and its `[ "$status" -eq 0 ] || exit "$status"`.
2. `:1711-1763` — the P-D6 fixture block, ending immediately before the `# Positive control:` comment at `:1764`.
3. `:1040-1120` — the `Check P-D6` comment block, `assert_wheels_has_no_credentials`, `strip_comments` and `PATTERNS`.
4. `:73-78` — the `#   Check P-D6 …` summary entry and its trailing `#` line.

- [ ] **Step 2: Confirm nothing references it**

```bash
grep -rn "P-D6\|assert_wheels_has_no_credentials\|strip_comments" ci/ moon.yml CLAUDE.md .github/
```

Expected: only hits under `ci/workflow-credentials/` and prose in the spec and plan.

- [ ] **Step 3: Correct the stale citations**

- `moon.yml:497` — the `publish-metadata` `description:` ends `while wheels.yml stays credential-free (SMA-578)`. Remove that clause.
- `moon.yml` — remove `'.github/workflows/wheels.yml'` from `publish-metadata`'s `inputs` **and** the comment explaining why it was there.
- `CLAUDE.md:351` — reads "`repo:publish-metadata` asserts this". Change to `repo:workflow-credentials`, and note the guard now covers every pull-request-triggered workflow.
- `.github/workflows/wheels.yml:12` — reads "`repo:publish-metadata` asserts this". Same correction. Leave the ban itself at `:9-10` alone.
- `ci/publish-metadata/README.md` — its check table has **no** P-D6 row to remove; it is already stale for the whole Python arm (no P0/P1/P2 rows). Add the missing rows, or add one line recording the staleness. Do not leave it silently wrong.

- [ ] **Step 4: Verify both gates**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:publish-metadata --force
moon run repo:workflow-credentials --force
moon run repo:affected-smoke --force
```

Expected: all three green. `repo:publish-metadata` must still pass its own negative control with the P-D6 rows gone.

- [ ] **Step 5: Run the full CI graph as CI does**

Copy the command from between the `ci-targets` markers in `CLAUDE.md` and run it verbatim.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "ci(repo): remove P-D6 from publish-metadata and correct its citations (SMA-593)"
```
