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
    # MEASURED (pyyaml 6.0.3, this project's pinned version): duplicate detection must run
    # on the POST-merge key set. `<<: *anchor` resolves only inside flatten_mapping(), which
    # splices the anchored mapping's entries into node.value and removes the merge-tagged
    # entry; without calling it first, construct_object() below chokes on the raw
    # merge-tagged key node — no constructor is ever registered for
    # tag:yaml.org,2002:merge, on ANY loader (SafeLoader/FullLoader/Loader/UnsafeLoader all
    # lack it), since PyYAML recognizes that tag only inside flatten_mapping, never as an
    # ordinary constructible key. Without this line, the "F merge key" self-test row — a
    # legitimate `<<:` merge — dies with ConstructorError instead of resolving, even though
    # plain `yaml.safe_load` handles the same input fine.
    loader.flatten_mapping(node)
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
    # GitHub evaluates an `if:` value as an expression even WITHOUT the ${{ }} wrapper, so
    # `if: secrets.TOKEN != ''` references the secrets context with no span for the loop
    # above to extract. EXPR_SPAN.sub("") first removes any WRAPPED part, which the loop
    # above already reported — otherwise a wrapped `if:` would be counted twice.
    for where, key, value in _mapping_entries(doc):
        if key == "if" and isinstance(value, str):
            bare = STRING_LITERAL.sub("", EXPR_SPAN.sub("", value))
            if SECRETS_CTX.search(bare):
                out.append(("R4", where, "reads the `secrets` context in a bare `if:` expression"))
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

    if failures:
        print(f"self-test: {failures} of {len(RULE_CASES)} rows failed", file=sys.stderr)
        return RC_ASSERT
    total = len(RULE_CASES) + len(TRIGGER_CASES) + len(PARSE_CASES)
    print(f"== workflow-credentials self-test passed ({total} rows) ==")
    return RC_OK


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
            for rule, where, message in rule_findings(doc):
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
              "pull_request trigger (SMA-407 §7 review M2)."
        )
    # The NAMES, not just a count. ci/workflow-credentials/run.sh --negative-control greps
    # this line to assert release.yml is absent from the subject set; a count alone would
    # make that row match nothing and assert nothing. (Pre-flight ruling 2.)
    print(f"workflow-credentials: subjects: {' '.join(subjects)}")
    print(f"workflow-credentials: {len(subjects)} pull-request-triggered workflow(s) "
          "carry no credential")
    return RC_OK


def main(argv: list[str]) -> int:
    if argv[1:2] == ["--self-test"]:
        return _self_test()
    if len(argv) != 2:
        raise InfraError("usage: workflow_credentials.py <repo-root> | --self-test")
    return check(argv[1])


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
    ("I bare if secrets",    H + "    steps:\n      - if: secrets.TOKEN != ''\n        run: echo hi\n", True),
    ("J bare if uppercase",  H + "    steps:\n      - if: SECRETS.TOKEN != ''\n        run: echo hi\n", True),
    ("K wrapped if",         H + "    steps:\n      - if: ${{ secrets.TOKEN != '' }}\n        run: echo hi\n", True),
    ("L if without secrets", H + "    steps:\n      - if: github.event_name == 'push'\n        run: echo hi\n", False),
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
