# SPDX-License-Identifier: Apache-2.0
"""Assert no credential-bearing-trigger workflow can obtain a repository credential.

Exit codes are DELIBERATELY not the repo's usual 0/1/2. This process exits 3 for an
assertion failure so that `uv`'s own rc 1 — measured on a failed resolution, online and
under UV_OFFLINE=1 — cannot be mistaken for "a workflow declares a credential". run.sh
maps 3 -> 1 and everything else -> 2. (SMA-593 spec §6.)
"""

from __future__ import annotations

import glob
import os
import re
import sys
import tempfile

import yaml

RC_OK = 0
RC_INFRA = 2
RC_ASSERT = 3


class InfraError(Exception):
    """The check could not run. Maps to RC_INFRA."""


SCAN_GLOB = ".github/workflows/*.y*ml"

# An Actions expression span, then the literals inside it, then the context name.
# Stripping literals is what stops hashFiles('secrets.txt') matching; it does NOT weaken
# secrets['X'], because the context name sits OUTSIDE the literal. The boundary rejects
# inputs.secrets-file (preceded by '.') and steps.x.outputs.secrets (likewise). All three
# were MEASURED as false positives against a bare \bsecrets\b. (spec §4.3)
#
# EXPR_SPAN is LITERAL-AWARE rather than a plain non-greedy `.*?`, because a `}}` can sit
# inside a string literal: `${{ format('{0} }}', secrets.PYPI) }}` terminated the non-greedy
# span at the `}}` INSIDE the literal, so the span never reached `secrets` and the read went
# unseen (MEASURED, SMA-593 F2). The repeat consumes a whole literal atomically or one
# character that does not begin an unquoted `}}`, so it can only stop at a real span end.
#
# The obvious alternative — stripping literals from the whole scalar BEFORE extracting spans —
# was measured and REJECTED: it deletes a shell-quoted expression whole, so `run: echo
# "${{ secrets.X }}"` stops matching. That is the commonest way a workflow actually reads a
# secret, and this repo already carries the shape (.github/workflows/wheels.yml:233 and :262
# write `python - "${{ matrix.expect_tag }}"`). Trading a rare true positive for that false
# negative is a net loss. Row "N shell-quoted expr in run" pins it.
#
# The possessive `*+` is load-bearing for cost, not for meaning: an unterminated `${{` with
# several quotes in the tail backtracks exponentially without it. Python 3.11+ only, and
# run.sh already demands `--python '>=3.12'`.
EXPR_SPAN = re.compile(r"\$\{\{((?:'[^']*'|\"[^\"]*\"|(?!\}\}).)*+)\}\}", re.S)
STRING_LITERAL = re.compile(r"'[^']*'|\"[^\"]*\"")
SECRETS_CTX = re.compile(r"(?<![\w.-])secrets(?![\w-])", re.IGNORECASE)


class AssertionFailureError(Exception):
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
    except UnicodeDecodeError as exc:
        # NOT an OSError and NOT a YAMLError — it is a ValueError, so without this clause it
        # reached the generic handler and reported rc 2, "re-run the job". A workflow that is
        # not UTF-8 is an authorial mistake, and ci_targets.py:28-36 states the repo's rule for
        # those: "a red with a fix, not a broken tool". Same misclassification the zero-match
        # split fixed, in a second place (SMA-593, CodeRabbit pass 2).
        raise AssertionFailureError(
            f"{os.path.basename(path)} is not valid UTF-8: {exc}") from exc
    except yaml.YAMLError as exc:
        raise AssertionFailureError(f"{os.path.basename(path)} is not valid YAML: {exc}") from exc


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
        # R5. Any INDIVIDUAL write scope, not just write-all. A credential-bearing-trigger
        # workflow granting e.g. `contents: write` can push to the repository using the
        # workflow's own `${{ github.token }}` — a real credential, obtained without any
        # `secrets` key or context read, so R1-R4 never see it. Nothing else in this repo
        # catches it: `repo:actionlint` validates scope NAMES against the schema and does
        # not audit breadth, and zizmor is not run here at all (both were claimed as
        # covering this and neither does).
        # `id-token` is excluded because R2 already names it with a sharper message; without
        # the exclusion every id-token grant would be reported twice.
        # MEASURED green on all five subjects at the time of writing: each declares exactly
        # `permissions: {contents: read}`.
        if key == "permissions" and isinstance(value, dict):
            for scope, level in value.items():
                if scope != "id-token" and _lower(level) == "write":
                    out.append(("R5", f"{where}.{scope}", f"grants `{scope}: write`"))
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
    # Arity floors, checked BEFORE anything runs (SMA-593 F6). A table emptied to nothing makes
    # every loop below iterate zero times, so the suite prints "passed" and returns 0 having
    # asserted nothing — the same vacuous shape the negative control exists to catch in run.sh.
    # These are FLOORS, not equalities: adding a row must not need an edit here. Lower one only
    # together with the row you are deliberately retiring.
    for name, table, floor in (
        ("RULE_CASES", RULE_CASES, 42),
        ("TRIGGER_CASES", TRIGGER_CASES, 7),
        ("PARSE_CASES", PARSE_CASES, 3),
    ):
        if len(table) < floor:
            raise InfraError(
                f"{name} holds {len(table)} rows, below its floor of {floor} — a shrunken table "
                "makes this self-test pass vacuously. Lower the floor deliberately, in the same "
                "commit that removes the row.")

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

    # Driven through the PRODUCTION entry point, discover(), not a bare yaml.load_all (SMA-593
    # F5). The old loop called yaml.load_all then isinstance, which is not where any of this
    # handling lives: the "top-level sequence" row passed with discover()'s non-mapping check
    # DELETED, and "malformed yaml" passed with load_documents()' YAMLError -> AssertionFailureError
    # mapping gutted, because a bare `except yaml.YAMLError: continue` made any raising row an
    # automatic pass. There is no such catch now — every row must reach discover() and every row
    # must come back as AssertionFailureError, the class that means "the repo is wrong", never
    # InfraError and never a silent accept.
    for label, source in PARSE_CASES:
        with tempfile.TemporaryDirectory() as root:
            wf_dir = os.path.join(root, ".github", "workflows")
            os.makedirs(wf_dir)
            with open(os.path.join(wf_dir, "x.yml"), "w", encoding="utf-8") as handle:
                handle.write(source)
            try:
                discover(root)
                print(f"  FAIL parse/{label}: discover() accepted a document that must be "
                      "rejected", file=sys.stderr)
                failures += 1
            except AssertionFailureError:
                pass
            except Exception as exc:
                print(f"  FAIL parse/{label}: expected AssertionFailureError, got "
                      f"{type(exc).__name__}: {exc}", file=sys.stderr)
                failures += 1

    fs_failures, fs_rows = _self_test_filesystem()
    failures += fs_failures
    # The filesystem rows are counted, not assumed. Without this, deleting one row leaves the
    # printed total at its old value and the loss is invisible (SMA-593 F6).
    if fs_rows != FILESYSTEM_CASES:
        raise InfraError(
            f"_self_test_filesystem ran {fs_rows} rows, but FILESYSTEM_CASES says "
            f"{FILESYSTEM_CASES} — a row was added or removed without updating the count")

    total = len(RULE_CASES) + len(TRIGGER_CASES) + len(PARSE_CASES) + FILESYSTEM_CASES
    if failures:
        print(f"self-test: {failures} of {total} rows failed", file=sys.stderr)
        return RC_ASSERT
    print(f"== workflow-credentials self-test passed ({total} rows) ==")
    return RC_OK


PR_TRIGGERS = frozenset({"pull_request", "pull_request_target", "issue_comment"})

# The subject set, pinned by STRICT EQUALITY. run.sh's sibling gate holds two discovered sets
# the same way (EXPECTED_PUBLISHABLE, EXPECTED_PYPI_PUBLISHABLE) and Check P0 states the
# reason: a stale list silently SHRINKS the gate rather than reporting red. A new
# credential-bearing-trigger workflow reds here until someone adds it, deliberately. (spec §5.2)
EXPECTED_PR_SUBJECTS = (
    "ci.yml",
    "cla-retrigger.yml",
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

    Both keys are UNIONED, never preferred one over the other (SMA-593 F3). A document can
    legitimately hold BOTH — `"on": push` is the string key, a bare `on: pull_request` is the
    boolean True — and they are two distinct dict keys, so the strict loader sees no duplicate
    to reject. The former `doc.get("on", doc.get(True))` found the string key and never
    consulted the fallback: MEASURED on keys ['on', True, 'jobs'], it returned {'push'} alone,
    so the file dropped out of the subject set and its credential grants went unchecked.
    """
    if not isinstance(doc, dict):
        return set()
    out = set()
    for key in ("on", True):
        if key not in doc:
            continue
        on = doc[key]
        if isinstance(on, str):
            out.add(on)
        elif isinstance(on, (list, dict)):
            out |= {str(x) for x in on}
    return out


def discover(root: str) -> list[str]:
    """Basenames of every workflow whose triggers make it a subject."""
    github_dir = os.path.join(root, ".github")
    # include_hidden: glob's wildcards skip a leading dot, so `*.y*ml` would miss
    # `.github/workflows/.credentials.yml`. The literal `.github` component is unaffected —
    # only WILDCARD matching skips dotfiles — so the omission was invisible until named.
    # A gate that silently declines to read a file is the failure this whole issue is about,
    # so the scan is widened rather than argued about (SMA-593, CodeRabbit PR review).
    paths = sorted(glob.glob(os.path.join(root, SCAN_GLOB), include_hidden=True))
    if not paths:
        # SPLIT, deliberately (SMA-593, controller ruling 10). Two different causes hide in
        # "no files matched", and they triage differently. The wrong repo root is a broken
        # tool, rc 2. Someone deleting or renaming the workflows is an authorial act, and
        # ci_targets.py:28-36 states the repo's rule for exactly that: "someone edited a file
        # into a shape this gate cannot read ... is a red with a fix, not a broken tool", so
        # rc 1. Collapsing both into rc 2 tells a contributor to re-run a job that will never
        # go green.
        #
        # The discriminator is `.github/`, NOT `.github/workflows/` (SMA-593 F9). Splitting on
        # the workflows dir put the authorial case on a branch git cannot reach: git tracks no
        # empty directory, so "directory present, holding no .y*ml" never occurs in a CI
        # checkout. The one authorial act that DOES occur — a PR deleting every workflow —
        # removes the now-empty directory along with them, which landed on rc 2 and told the
        # author to re-run a job that could not go green. That is the exact misclassification
        # the split exists to prevent. `.github/` survives such a PR (CODEOWNERS, issue
        # templates, dependabot.yml all live there), so it is the reliable signal for "this IS
        # the repo root, the workflows are what went missing".
        if not os.path.isdir(github_dir):
            raise InfraError(
                f"{github_dir} does not exist — the checker was given the wrong repo root")
        raise AssertionFailureError(
            f"{SCAN_GLOB} matched no file under {root}, but .github/ exists — the workflows "
            "were removed or renamed, and this gate would assert nothing")
    subjects = []
    for path in paths:
        docs = load_documents(path)
        for doc in docs:
            if doc is not None and not isinstance(doc, dict):
                raise AssertionFailureError(
                    f"{os.path.basename(path)}: top-level YAML is not a mapping")
        if any(triggers(d) & PR_TRIGGERS for d in docs if isinstance(d, dict)):
            subjects.append(os.path.basename(path))
    return subjects


def check(root: str) -> int:
    subjects = discover(root)
    # Compared BEFORE any allowlist: the allowlist suppresses rule verdicts, never
    # membership. Counting after it would let "allowlist everything" pass. (spec §5.2)
    if tuple(subjects) != EXPECTED_PR_SUBJECTS:
        raise AssertionFailureError(
            f"credential-bearing-trigger workflows are {subjects}, expected "
            f"{list(EXPECTED_PR_SUBJECTS)} — re-baseline EXPECTED_PR_SUBJECTS deliberately")

    stale = [name for (name, _rule) in PR_CREDENTIAL_ALLOWED if name not in subjects]
    if stale:
        raise AssertionFailureError(
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
        raise AssertionFailureError(
            "a credential-bearing-trigger workflow can obtain a repository credential:\n"
            + "\n".join(reds)
            + "\n  A same-repo pull request receives repository secrets, so this is readable "
              "by any code the PR introduces — publishing belongs in a workflow with no "
              "pull_request or pull_request_target trigger (SMA-407 §7 review M2). An "
              "issue_comment run gets those same secrets in base-repo context while being "
              "triggerable by any comment on a public repo; the reader there is not code the "
              "PR introduces, but the exposure is the same (SMA-408)."
        )
    # The NAMES, not just a count. ci/workflow-credentials/run.sh --negative-control greps
    # this line to assert release.yml is absent from the subject set; a count alone would
    # make that row match nothing and assert nothing. (Pre-flight ruling 2.)
    print(f"workflow-credentials: subjects: {' '.join(subjects)}")
    print(f"workflow-credentials: {len(subjects)} credential-bearing-trigger workflow(s) "
          "carry no credential")
    return RC_OK


# Six rows exercised against a real filesystem, not a YAML string, so they cannot live in
# RULE_CASES/TRIGGER_CASES/PARSE_CASES. Four cover the zero-match SPLIT (Finding 1, SMA-593
# controller ruling 10, discriminator corrected by F9): discover() must raise InfraError when
# .github/ itself is absent (the checker was handed the wrong root) but AssertionFailureError in
# BOTH authorial shapes — .github/ present with the workflows dir deleted along with its files
# (row 2a, the shape a PR deleting every workflow actually produces, since git tracks no empty
# directory) and .github/workflows/ present but empty (row 2b, reachable only in a working
# tree). A `.yaml` (not `.yml`) workflow must still be discovered, proving SCAN_GLOB's `*.y*ml`
# covers both extensions. Two cover the stale-allowlist guard (Finding 2): PR_CREDENTIAL_ALLOWED
# is this gate's only escape hatch and was otherwise untested — an entry naming a workflow that
# is not a subject must raise AssertionFailureError, and an empty allowlist must not.
FILESYSTEM_CASES = 8


def _self_test_filesystem() -> tuple[int, int]:
    # Returns (failures, rows_run). The row COUNT is returned rather than assumed so the
    # caller can assert it against FILESYSTEM_CASES: deleting a row would otherwise leave the
    # printed total unchanged and the loss invisible (SMA-593 F6). `rows` is incremented once
    # per row, next to the row it counts, so the two cannot drift apart silently.
    failures = 0
    rows = 0

    # 1. .github/ absent entirely -> the checker was given the wrong root: InfraError.
    rows += 1
    with tempfile.TemporaryDirectory() as root:
        try:
            discover(root)
            print("  FAIL discover/missing .github dir: expected InfraError, "
                  "got no exception", file=sys.stderr)
            failures += 1
        except InfraError:
            pass
        except Exception as exc:
            print(f"  FAIL discover/missing .github dir: expected InfraError, "
                  f"got {type(exc).__name__}", file=sys.stderr)
            failures += 1

    # 2a. .github/ present, .github/workflows/ GONE -> workflows deleted: AssertionFailureError.
    # This is the reachable authorial shape. A PR deleting every workflow removes the directory
    # too, because git cannot track it empty — before F9 this landed on InfraError (rc 2) and
    # told the author to re-run a job that could never go green.
    rows += 1
    with tempfile.TemporaryDirectory() as root:
        os.makedirs(os.path.join(root, ".github"))
        try:
            discover(root)
            print("  FAIL discover/no workflows dir under .github: expected AssertionFailureError, "
                  "got no exception", file=sys.stderr)
            failures += 1
        except AssertionFailureError:
            pass
        except Exception as exc:
            print(f"  FAIL discover/no workflows dir under .github: expected AssertionFailureError, "
                  f"got {type(exc).__name__}", file=sys.stderr)
            failures += 1

    # 2b. .github/workflows/ present but empty -> same verdict, AssertionFailureError. Reachable in a
    # working tree rather than a checkout, and kept so the two shapes cannot diverge.
    rows += 1
    with tempfile.TemporaryDirectory() as root:
        os.makedirs(os.path.join(root, ".github", "workflows"))
        try:
            discover(root)
            print("  FAIL discover/empty workflows dir: expected AssertionFailureError, "
                  "got no exception", file=sys.stderr)
            failures += 1
        except AssertionFailureError:
            pass
        except Exception as exc:
            print(f"  FAIL discover/empty workflows dir: expected AssertionFailureError, "
                  f"got {type(exc).__name__}", file=sys.stderr)
            failures += 1

    # 3. a `.yaml` (not `.yml`) pull_request workflow is still discovered.
    rows += 1
    with tempfile.TemporaryDirectory() as root:
        wf_dir = os.path.join(root, ".github", "workflows")
        os.makedirs(wf_dir)
        with open(os.path.join(wf_dir, "x.yaml"), "w", encoding="utf-8") as handle:
            handle.write("on:\n  pull_request:\njobs: {}\n")
        try:
            subjects = discover(root)
            if subjects != ["x.yaml"]:
                print(f"  FAIL discover/.yaml extension: expected ['x.yaml'], got {subjects}",
                      file=sys.stderr)
                failures += 1
        except Exception as exc:
            print(f"  FAIL discover/.yaml extension: unexpected {type(exc).__name__}: {exc}",
                  file=sys.stderr)
            failures += 1

    # 4 & 5. The stale-allowlist guard, exercised through check() with a root whose subjects
    # exactly match EXPECTED_PR_SUBJECTS so the subject-pin comparison clears and the guard is
    # actually reached.
    rows += 2
    with tempfile.TemporaryDirectory() as root:
        wf_dir = os.path.join(root, ".github", "workflows")
        os.makedirs(wf_dir)
        for name in EXPECTED_PR_SUBJECTS:
            with open(os.path.join(wf_dir, name), "w", encoding="utf-8") as handle:
                handle.write(
                    "on:\n  pull_request:\njobs:\n  a:\n    permissions:\n      contents: read\n")

        saved_allowed = dict(PR_CREDENTIAL_ALLOWED)
        try:
            # 4. An allowlist entry naming a workflow that is NOT a subject: AssertionFailureError.
            PR_CREDENTIAL_ALLOWED.clear()
            PR_CREDENTIAL_ALLOWED[("release.yml", "R1")] = "test: not a subject"
            try:
                check(root)
                print("  FAIL check/stale allowlist entry: expected AssertionFailureError, "
                      "got no exception", file=sys.stderr)
                failures += 1
            except AssertionFailureError:
                pass
            except Exception as exc:
                print(f"  FAIL check/stale allowlist entry: expected AssertionFailureError, "
                      f"got {type(exc).__name__}", file=sys.stderr)
                failures += 1

            # 5. An empty allowlist must NOT fire the guard.
            PR_CREDENTIAL_ALLOWED.clear()
            try:
                rc = check(root)
                if rc != RC_OK:
                    print(f"  FAIL check/empty allowlist: expected RC_OK, got {rc}",
                          file=sys.stderr)
                    failures += 1
            except Exception as exc:
                print(f"  FAIL check/empty allowlist: unexpected {type(exc).__name__}: {exc}",
                      file=sys.stderr)
                failures += 1
        finally:
            PR_CREDENTIAL_ALLOWED.clear()
            PR_CREDENTIAL_ALLOWED.update(saved_allowed)

    # 7. A workflow that is not UTF-8 -> authorial, so AssertionFailureError (rc 1), never rc 2.
    rows += 1
    with tempfile.TemporaryDirectory() as root:
        wf = os.path.join(root, ".github", "workflows")
        os.makedirs(wf)
        with open(os.path.join(wf, "x.yml"), "wb") as handle:
            handle.write(b'on:\n  pull_request:\njobs:\n  a:\n    name: "\xff\xfe"\n')
        try:
            discover(root)
            print("  FAIL discover/non-utf8: expected AssertionFailureError, got no exception",
                  file=sys.stderr)
            failures += 1
        except AssertionFailureError:
            pass
        except Exception as exc:
            print(f"  FAIL discover/non-utf8: expected AssertionFailureError, "
                  f"got {type(exc).__name__}", file=sys.stderr)
            failures += 1

    # 8. A DOT-PREFIXED workflow is still discovered. glob's wildcards skip a leading dot, so
    #    `*.y*ml` missed `.credentials.yml` until include_hidden was set. Without this row the
    #    widening could be reverted for tidiness and nothing would notice.
    rows += 1
    with tempfile.TemporaryDirectory() as root:
        wf = os.path.join(root, ".github", "workflows")
        os.makedirs(wf)
        with open(os.path.join(wf, ".credentials.yml"), "w", encoding="utf-8") as handle:
            handle.write("on:\n  pull_request:\njobs:\n  a:\n    runs-on: x\n")
        # Wrapped: without include_hidden, discover() finds nothing and raises rather than
        # returning, which would abort the whole run instead of reporting THIS row.
        try:
            found = discover(root)
        except (AssertionFailureError, InfraError) as exc:
            print(f"  FAIL discover/hidden workflow: expected ['.credentials.yml'], "
                  f"got {type(exc).__name__} — a dot-prefixed workflow was not scanned",
                  file=sys.stderr)
            failures += 1
        else:
            if found != [".credentials.yml"]:
                print(f"  FAIL discover/hidden workflow: expected ['.credentials.yml'], "
                      f"got {found}", file=sys.stderr)
                failures += 1

    return failures, rows


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
    # SMA-593 F2. A `}}` inside a string literal used to end the span early, so `secrets` was
    # never reached. Both rows guard EXPR_SPAN's literal-aware form: M is the bug, N is the
    # regression the rejected "strip literals first" fix would have introduced.
    ("M }} inside a literal", H + "    env:\n      T: ${{ format('{0} }}', secrets.PYPI) }}\n", True),
    ("N shell-quoted expr in run", H + '    steps:\n      - run: echo "${{ secrets.X }}"\n', True),
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
    # R5 — an individual write scope. `contents: write` plus the workflow's own github.token
    # is a real credential that R1-R4 cannot see.
    ("R5a contents write",   H + "    permissions:\n      contents: write\n", True),
    ("R5b packages write",   H + "    permissions:\n      packages: write\n", True),
    ("R5c write at workflow level",
     "on:\n  pull_request:\npermissions:\n  contents: write\njobs:\n  a:\n    runs-on: x\n", True),
    # ...and the shapes it must NOT red on, since a first false positive is how a gate gets
    # allowlisted into irrelevance.
    ("R5d contents read",    H + "    permissions:\n      contents: read\n", False),
    ("R5e read-all",         H + "    permissions: read-all\n", False),
    ("R5f empty permissions", H + "    permissions: {}\n", False),
    ("R5g scope none",       H + "    permissions:\n      contents: none\n", False),
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
    # SMA-593 F3. Two DISTINCT dict keys — the string 'on' and the YAML 1.1 boolean True — so
    # the strict loader has no duplicate to reject. The old `.get("on", .get(True))` returned
    # {'push'} here and the workflow silently left the subject set.
    ("dual on keys",    '"on": push\non:\n  pull_request:\njobs: {}\n', True),
    # SMA-408. `issue_comment` runs in base-repo context with repository secrets, and on a
    # public repo ANY account can trigger it. Same privileged class as pull_request_target.
    ("issue_comment", "on:\n  issue_comment:\n    types: [created]\njobs: {}\n", True),
    # Still false: a trigger that carries no repository secrets stays out of the subject set.
    ("workflow_dispatch only", "on:\n  workflow_dispatch:\njobs: {}\n", False),
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
    except AssertionFailureError as exc:
        print(f"workflow-credentials FAILED: {exc}", file=sys.stderr)
        raise SystemExit(RC_ASSERT) from exc
    except InfraError as exc:
        print(f"workflow-credentials: {exc}", file=sys.stderr)
        raise SystemExit(RC_INFRA) from exc
    except Exception as exc:  # an unexpected crash is INFRA, never an assertion
        print(f"workflow-credentials: unexpected {type(exc).__name__}: {exc}", file=sys.stderr)
        raise SystemExit(RC_INFRA) from exc
