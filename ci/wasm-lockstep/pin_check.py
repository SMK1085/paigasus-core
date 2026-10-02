# SPDX-License-Identifier: Apache-2.0
"""The pin check of .github/workflows/wasm-lockstep.yml (SMA-693 spec 5.4).

A PyYAML parse, never a text scan: SMA-593 measured fourteen bypasses of a text scan, a YAML
alias among them. The rules (P0-P21) are the trust model of spec 5.1 and 5.2 in checkable form.
ci/wasm-lockstep/README.md lists each rule and what it does not prove.

  pin_check.py <workflow.yml>                       the rules on one workflow
  pin_check.py --self-test                          the in-process fixture table (T3)
  pin_check.py --negative-control <workflow.yml>    mutations of the real workflow must red

Exit codes: 0 pass | 3 a rule failed | 2 infrastructure. Not 1: `uv` exits 1 on a failed
resolution. ci/wasm-lockstep/run.sh maps 3 -> 1 and every other non-zero code -> 2.

The five PyYAML coercions of ci/CLAUDE.md are handled: a bare `on:` key is the boolean True;
`if: false` and `continue-on-error: false` are booleans; a quoted "false" is a string; `needs:`
can be a scalar. Aliases resolve in the parse, so an alias that injects a `secrets` read is seen.
A merge key (`<<:`) is refused: GitHub Actions does not merge it, so a merged parse would check a
workflow that GitHub does not run.
"""

from __future__ import annotations

import copy
import re
import shlex
import sys

import yaml

RC_OK = 0
RC_INFRA = 2
RC_ASSERT = 3


class InfraError(Exception):
    """The check could not run. Maps to RC_INFRA."""


class AssertionFailureError(Exception):
    """The workflow is wrong. Maps to RC_ASSERT."""


# The expression grammar of ci/workflow-credentials/workflow_credentials.py, copied (the two gates
# have separate uv projects on purpose). See that file for the measured reasons of each part.
EXPR_SPAN = re.compile(r"\$\{\{((?:'[^']*'|\"[^\"]*\"|(?!\}\}).)*+)\}\}", re.S)
STRING_LITERAL = re.compile(r"'[^']*'|\"[^\"]*\"")
SECRETS_CTX = re.compile(r"(?<![\w.-])secrets(?![\w-])", re.IGNORECASE)
NEEDS_CTX = re.compile(r"(?<![\w.-])needs(?![\w-])", re.IGNORECASE)
STATUS_FN = re.compile(r"(?<![\w.-])(always|failure|cancelled)\s*\(", re.IGNORECASE)
SHA_PIN = re.compile(r"[0-9a-f]{40}")
ASSIGNMENT = re.compile(r"[A-Za-z_][A-Za-z0-9_]*=.*", re.S)
IMAGE_ASSIGN = re.compile(r"\bLOCKSTEP_IMAGE(?:[:+])?=")
RUNNER_FILES = re.compile(r"GITHUB_(?:ENV|PATH)\b")
IF_PATH = re.compile(r"\$\.jobs\.[^.\[]+(?:\.steps\[\d+\])?\.if")
EXEC_ENV_KEYS = frozenset({"BASH_ENV", "ENV"})
CHECKER_SUBS = frozenset({"artifact", "status"})
GH_BODY_FLAGS = frozenset({"-f", "-F", "--field", "--raw-field", "--input"})

EXPECTED_JOBS = frozenset({"build", "propose"})
EXPECTED_TRIGGERS = frozenset({"schedule", "workflow_dispatch"})
PERMISSIONS = {"contents": "read"}
ALLOWED_ACTIONS = {
    "build": frozenset({"actions/checkout", "actions/upload-artifact"}),
    "propose": frozenset({"actions/checkout", "actions/download-artifact", "actions/create-github-app-token"}),
}
PROPOSE_ORDER = ("checkout", "download", "verify", "apply", "token", "commit", "base", "push", "pr", "close")
BUILD_OUTPUTS = {"changed": "${{ steps.lock.outputs.changed }}"}
TOKEN_WITH = {
    "client-id": "${{ secrets.PAIGASUS_BOT_APP_ID }}",
    "private-key": "${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}",
    "permission-contents": "write",
    "permission-pull-requests": "write",
}
REFSPEC = "HEAD:refs/heads/deps/wasm-bindgen-lockstep"
LEASE_PREFIX = "--force-with-lease=refs/heads/deps/wasm-bindgen-lockstep:"
PUSH_REMOTE = "https://x-access-token:${PUSH_TOKEN}@github.com/${GITHUB_REPOSITORY}.git"
CHECKER = "ci/wasm-lockstep/lockstep_check.py"
CONTAINER_SCRIPT = "ci/wasm-lockstep/container.sh"
IMAGE_TOKEN = "$LOCKSTEP_IMAGE"
IMAGE_PIN = re.compile(r"docker\.io/library/rust:[0-9][0-9.]*-bookworm@sha256:[0-9a-f]{64}")
DOCKER_FLAGS = frozenset({"--rm"})
DOCKER_VALUES = {"--cap-drop": "ALL", "--security-opt": "no-new-privileges", "--user": "65534:65534", "--volume": "$RUNNER_TEMP/work:/work", "--workdir": "/work"}

# Command words. A word is the first word of a simple command, after a separator, a keyword
# that starts a command, or `sudo`. `case` is NOT allowed: its patterns read as command words.
COMMON_WORDS = frozenset({"if", "then", "else", "elif", "fi", "for", "do", "done", "while", "until",
                          "!", "{", "}", "exit", "true", "set", "read", "[", "test", "echo", "printf", "cat", "sed"})
JOB_WORDS = {
    "build": COMMON_WORDS | {"df", "sudo", "rm", "git", "tar", "mkdir", "cp", "chmod", "python3", "docker"},
    "propose": COMMON_WORDS | {"python3", "git", "gh", "cp", "grep"},
}
SUDO_WORDS = frozenset({"rm", "docker", "chown"})
# The one chown form that is allowed: it gives the work copy to the container's user (nobody).
SUDO_CHOWN = ["chown", "-R", "65534:65534", "$RUNNER_TEMP/work"]
STARTS_COMMAND = frozenset({"if", "then", "else", "elif", "do", "while", "until", "!", "{"})
SEPARATOR_CHARS = frozenset(";&|()")
# set -e ignores the status of a command negated with `!`, so `! test -L f` never stops a step.
# A `!` is allowed only where its status is tested: right after if, elif, while or until.
NEGATION_OK = frozenset({"if", "elif", "while", "until"})
SUBST = "__SUBST__"


class _StrictLoader(yaml.SafeLoader):
    """SafeLoader that refuses duplicate keys (PyYAML's default is last-wins) and merge keys."""


def _construct_mapping(loader, node, deep=False):
    for key_node, _ in node.value:
        if key_node.tag == "tag:yaml.org,2002:merge":
            raise yaml.constructor.ConstructorError(
                "while constructing a mapping", node.start_mark,
                "a merge key (<<:), which GitHub Actions does not merge", key_node.start_mark)
    seen = set()
    for key_node, _ in node.value:
        key = loader.construct_object(key_node, deep=deep)
        if key in seen:
            raise yaml.constructor.ConstructorError(
                "while constructing a mapping", node.start_mark, f"duplicate key {key!r}", key_node.start_mark)
        seen.add(key)
    return yaml.SafeLoader.construct_mapping(loader, node, deep=deep)


_StrictLoader.add_constructor(yaml.resolver.BaseResolver.DEFAULT_MAPPING_TAG, _construct_mapping)


def parse_text(text: str, label: str) -> dict:
    try:
        docs = list(yaml.load_all(text, Loader=_StrictLoader))
    except yaml.YAMLError as exc:
        raise AssertionFailureError(f"P0 {label} is not valid YAML for this check: {exc}") from exc
    if len(docs) != 1 or not isinstance(docs[0], dict):
        raise AssertionFailureError(f"P0 {label} must hold exactly one YAML mapping")
    return docs[0]


def load(path: str) -> dict:
    try:
        with open(path, encoding="utf-8") as handle:
            text = handle.read()
    except (OSError, UnicodeDecodeError) as exc:
        raise InfraError(f"cannot read {path}: {exc}") from exc
    return parse_text(text, path)


def _strings(node, path="$"):
    """(path, last key, string) for every string key and value."""
    if isinstance(node, dict):
        for key, value in node.items():
            here = f"{path}.{key}"
            if isinstance(key, str):
                yield here, None, key
            if isinstance(value, str):
                yield here, key, value
            else:
                yield from _strings(value, here)
    elif isinstance(node, list):
        for index, value in enumerate(node):
            if isinstance(value, str):
                yield f"{path}[{index}]", None, value
            else:
                yield from _strings(value, f"{path}[{index}]")


def _reads(node, context: re.Pattern, path: str = "$", skip_if: bool = False) -> list[str]:
    """Paths whose string reads `context` in an expression span, or in a bare `if:` value."""
    out = []
    for where, key, text in _strings(node, path):
        is_if = key == "if" and IF_PATH.fullmatch(where) is not None
        if skip_if and is_if:
            continue
        spans = [STRING_LITERAL.sub("", s) for s in EXPR_SPAN.findall(text)]
        if is_if:
            spans.append(STRING_LITERAL.sub("", EXPR_SPAN.sub("", text)))
        if any(context.search(span) for span in spans):
            out.append(where)
    return out


def _needs_list(value) -> list:
    # PyYAML coercion: `needs: build` is a scalar, and iterating a string yields its characters.
    if isinstance(value, str):
        return [value]
    return list(value) if isinstance(value, list) else [value]


def triggers(doc: dict) -> set[str]:
    """`on:` parses as the boolean True. Read both keys and UNION them (SMA-593 F3)."""
    out: set[str] = set()
    for key in ("on", True):
        if key not in doc:
            continue
        value = doc[key]
        if isinstance(value, str):
            out.add(value)
        elif isinstance(value, (list, dict)):
            out |= {str(x) for x in value}
        else:
            out.add(repr(value))
    return out


# ---- the shell reader -----------------------------------------------------------------------

def _substitutions(text: str) -> tuple[str, list[str], list[str]]:
    """Replace each `$(...)` with a placeholder and return (outer, inner texts, problems). A
    backtick or a here-document is a problem: this reader does not follow them, so it refuses."""
    out, inner, problems = [], [], []
    i = 0
    while i < len(text):
        if text.startswith("$((", i):
            problems.append("an arithmetic expansion $((...)); write the value another way")
            i += 3
            continue
        if text.startswith("$(", i):
            depth, j = 1, i + 2
            while j < len(text) and depth:
                depth += {"(": 1, ")": -1}.get(text[j], 0)
                j += 1
            if depth:
                problems.append("an unbalanced $(")
                return "".join(out), inner, problems
            inner.append(text[i + 2:j - 1])
            out.append(SUBST)
            i = j
            continue
        if text.startswith("<(", i) or text.startswith(">(", i):
            problems.append("a process substitution, which hides a command from this reader")
        if text[i] == "`":
            problems.append("a backtick command substitution")
        if text.startswith("<<", i):
            problems.append("a here-document")
        out.append(text[i])
        i += 1
    return "".join(out), inner, problems


def commands(script: str) -> tuple[list[list[str]], list[str]]:
    """Every simple command of a `run:` script as [word, args...], and the problems found."""
    outer, inner, problems = _substitutions(script.replace("\\\n", " "))
    found: list[list[str]] = []
    for text in inner:
        sub, sub_problems = commands(text)
        found += sub
        problems += sub_problems
    for line in outer.splitlines():
        lexer = shlex.shlex(line, posix=True, punctuation_chars=True)
        lexer.whitespace_split = True
        try:
            tokens = list(lexer)
        except ValueError as exc:
            problems.append(f"a line the shell reader cannot split ({exc}): {line.strip()!r}")
            continue
        expect, skip, keyword = True, False, None
        for token in tokens:
            if skip:
                skip = False
                continue
            if token and set(token) <= SEPARATOR_CHARS | {"<", ">"}:
                if "<" in token or ">" in token:
                    skip = True
                else:
                    expect, keyword = True, None
                continue
            if expect:
                if ASSIGNMENT.fullmatch(token):
                    continue
                if token == "!" and keyword not in NEGATION_OK:
                    problems.append(f"a bare `!` negation, which set -e ignores; write `if ...; then exit 1; fi`: {line.strip()!r}")
                found.append([token])
                expect = token in STARTS_COMMAND
                keyword = token if expect else None
                continue
            found[-1].append(token)
    return found, problems


# ---- the rules ---------------------------------------------------------------------------------

def _docker_run(args: list[str]) -> list[str]:
    out, seen, i = [], set(), 0
    while i < len(args) and args[i] != IMAGE_TOKEN:
        name, eq, value = args[i].partition("=")
        if name in DOCKER_FLAGS and not eq:
            seen.add(name)
            i += 1
            continue
        if name in DOCKER_VALUES:
            if not eq:
                value = args[i + 1] if i + 1 < len(args) else ""
                i += 1
            if value != DOCKER_VALUES[name]:
                out.append(f"docker run {name} {value!r}, expected {DOCKER_VALUES[name]!r}")
            seen.add(name)
            i += 1
            continue
        out.append(f"docker run option {args[i]!r} is not on the allowlist")
        i += 1
    if i >= len(args):
        out.append('docker run has no "$LOCKSTEP_IMAGE" image argument')
        return out
    missing = (DOCKER_FLAGS | set(DOCKER_VALUES)) - seen
    if missing:
        out.append(f"docker run lacks {sorted(missing)}")
    rest = args[i + 1:]
    if len(rest) != 3 or rest[:2] != ["bash", CONTAINER_SCRIPT] or rest[2] not in ("update", "build"):
        out.append(f"docker run must run `bash {CONTAINER_SCRIPT} update|build`, not {rest!r}")
    return out


def _gh_api(args: list[str]) -> list[str]:
    out = []
    if args[:1] == ["graphql"]:
        out.append("gh api graphql is a POST")
    for i, arg in enumerate(args):
        name, eq, value = arg.partition("=")
        method = None
        if name in ("--method", "-X") and not eq:
            method = args[i + 1] if i + 1 < len(args) else ""
        elif name == "--method" or (arg.startswith("-X") and len(arg) > 2):
            method = value if name == "--method" else arg[2:]
        if method is not None and method.upper() != "GET":
            out.append(f"gh api --method {method!r}: propose may read with GET only")
        if name in GH_BODY_FLAGS or (arg.startswith(("-f", "-F")) and not arg.startswith("--")):
            out.append(f"gh api {arg!r} makes the call a POST")
    return out


def _gh_pr_list(args: list[str]) -> list[str]:
    """`gh pr list --head <branch>` filters by branch NAME only, so a fork PR from a branch of the
    same name matches. This repo is public: the App token must never edit or close such a PR. The
    list must request isCrossRepository and select on it (P21)."""
    out = []
    for flag in ("--json", "--jq"):
        values = [args[i + 1] for i, a in enumerate(args[:-1]) if a == flag]
        if not any("isCrossRepository" in v for v in values):
            out.append(f"gh pr list must pass {flag} naming isCrossRepository, so a fork pull request is not selected")
    jq = [args[i + 1] for i, a in enumerate(args[:-1]) if a == "--jq"]
    if jq and not any(re.search(r"select\(\s*\.isCrossRepository\s*\|\s*not\s*\)", v) for v in jq):
        out.append("gh pr list --jq must select(.isCrossRepository | not)")
    return out


def _tokens(line: str) -> list[str] | None:
    lexer = shlex.shlex(line, posix=True, punctuation_chars=True)
    lexer.whitespace_split = True
    try:
        return list(lexer)
    except ValueError:
        return None


def _checker_tail(script: str, where: str) -> list[str]:
    """A `lockstep_check.py artifact|status` command must be the LAST command of its step, whole,
    with no `||`, `&&`, `;`, `|` or `&` joined to it and no open if/for/while around it. Else a
    refusal (exit 3) can be ignored: `... || true`, `... || rc=$?`, `if false; then ...; fi`."""
    found, _problems = commands(script)
    hits = [c for c in found if c[:2] == ["python3", CHECKER] and c[2:3] and c[2] in CHECKER_SUBS]
    if not hits:
        return []
    msg = [f"P18 {where}: the checker command must be the last whole command of its step, with nothing joined to it"]
    outer = _substitutions(script.replace("\\\n", " "))[0]
    lines = [ln for ln in outer.splitlines() if ln.strip() and not ln.strip().startswith("#")]
    tokens = _tokens(lines[-1]) if lines else None
    if len(hits) != 1 or tokens is None or tokens[:3] != hits[0][:3]:
        return msg
    if any(tok and set(tok) <= SEPARATOR_CHARS for tok in tokens[3:]):
        return msg
    depth = 0
    for cmd in found[:-1]:
        depth += (cmd[0] in ("if", "for", "while", "until", "{")) - (cmd[0] in ("fi", "done", "}"))
    return msg if depth != 0 or found[-1] != hits[0] else []


def command_violations(job: str, script: str, where: str) -> list[str]:
    out = []
    found, problems = commands(script)
    out += [f"P5 {where}: {p}" for p in problems]
    if IMAGE_ASSIGN.search(script):
        out.append(f"P4 {where}: a run script assigns LOCKSTEP_IMAGE, which would replace the pinned image")
    if RUNNER_FILES.search(script):
        out.append(f"P19 {where}: a run script names GITHUB_ENV or GITHUB_PATH, which change the later steps of the job")
    allowed = JOB_WORDS[job]
    for cmd in found:
        word = cmd[0]
        if word != "docker" and IMAGE_TOKEN[1:] in cmd[1:]:
            out.append(f"P4 {where}: {word!r} names LOCKSTEP_IMAGE as an argument, so it can set the variable")
        if word == "git" and any(a == "-c" or a.startswith("-c") or a.startswith("--config-env") for a in cmd[1:]):
            out.append(f"P20 {where}: git must not take -c or --config-env, which set a config value for one call")
        if word == "gh" and cmd[1:3] == ["pr", "list"]:
            out += [f"P21 {where}: {p}" for p in _gh_pr_list(cmd[3:])]
        if word == "gh" and cmd[1:2] == ["api"]:
            out += [f"P20 {where}: {p}" for p in _gh_api(cmd[2:])]
        if word not in allowed:
            out.append(f"P5 {where}: the command word {word!r} is not on the {job} allowlist")
        elif word == "python3" and cmd[1:2] != [CHECKER]:
            out.append(f"P5 {where}: python3 may run {CHECKER} only, not {cmd[1:2]!r}")
        elif word == "sudo" and (cmd[1:2] == [] or cmd[1] not in SUDO_WORDS or (cmd[1] == "docker" and cmd[2:3] != ["image"])
                                 or (cmd[1] == "chown" and cmd[1:] != SUDO_CHOWN)):
            out.append(f"P5 {where}: sudo may run rm, `docker image` and `chown -R 65534:65534 $RUNNER_TEMP/work` only, not {cmd[1:3]!r}")
        elif word == "docker":
            if cmd[1:2] != ["run"]:
                out.append(f"P4 {where}: docker may run `docker run` only, not {cmd[1:2]!r}")
            else:
                out += [f"P4 {where}: {p}" for p in _docker_run(cmd[2:])]
    return out


def _env_violations(env, where: str, image_ok: bool = False) -> list[str]:
    if env is None:
        return []
    if not isinstance(env, dict):
        return [f"P19 {where}: env must be a mapping, not {type(env).__name__}"]
    out = []
    for key in env:
        if key == "LOCKSTEP_IMAGE" and not image_ok:
            out.append(f"P4 {where}: env sets LOCKSTEP_IMAGE below the workflow level")
        if key in EXEC_ENV_KEYS:
            out.append(f"P19 {where}: env sets {key}, which makes bash run a file before a step")
    return out


def _steps(job: dict, name: str) -> list[dict]:
    steps = job.get("steps")
    if not isinstance(steps, list) or not all(isinstance(s, dict) for s in steps):
        raise AssertionFailureError(f"P0 job {name} has no list of step mappings")
    return steps


def _uses_violation(uses: str, job: str, where: str) -> str | None:
    action, at, ref = uses.partition("@")
    if action not in ALLOWED_ACTIONS[job]:
        return f"P6 {where}: {action!r} is not on the {job} action allowlist"
    if not at or not SHA_PIN.fullmatch(ref):
        return f"P6 {where}: {uses!r} is not pinned to a full commit SHA"
    return None


def step_violations(job: str, steps: list[dict]) -> list[str]:
    out = []
    for index, step in enumerate(steps):
        where = f"jobs.{job}.steps[{index}] ({step.get('id', '?')})"
        if "uses" in step:
            problem = _uses_violation(str(step["uses"]), job, where)
            if problem:
                out.append(problem)
            if str(step["uses"]).startswith("actions/checkout@"):
                with_block = step.get("with") if isinstance(step.get("with"), dict) else {}
                if with_block.get("persist-credentials") not in (False, "false"):
                    out.append(f"P17 {where}: a checkout must set persist-credentials: false")
        if "run" in step:
            out += command_violations(job, str(step["run"]), where)
            if job == "propose":
                out += _checker_tail(str(step["run"]), where)
        if "working-directory" in step:
            out.append(f"P19 {where}: working-directory moves a step into the work copy or a runner path")
        out += _env_violations(step.get("env"), where)
        if step.get("shell", "bash") != "bash":
            out.append(f"P13 {where}: shell must be bash, not {step.get('shell')!r}")
        if step.get("continue-on-error", False) not in (False, "false"):
            out.append(f"P14 {where}: continue-on-error must be absent or false")
        if STATUS_FN.search(str(step.get("if", ""))):
            out.append(f"P15 {where}: an if: with always(), failure() or cancelled() runs after a refusal")
    return out


def violations(doc: dict) -> list[str]:
    out: list[str] = []
    if triggers(doc) != EXPECTED_TRIGGERS:
        out.append(f"P2 the triggers are {sorted(triggers(doc))}, expected {sorted(EXPECTED_TRIGGERS)}")
    if doc.get("permissions") != PERMISSIONS:
        out.append(f"P10 the top-level permissions are {doc.get('permissions')!r}, expected {PERMISSIONS}")
    if "defaults" in doc:
        out.append("P13 a workflow-level defaults: block can change the shell of every run step")
    top = {k: v for k, v in doc.items() if k != "jobs"}
    out += [f"P3 {w} reads the secrets context outside the jobs" for w in _reads(top, SECRETS_CTX)]
    out += _env_violations(doc.get("env"), "the workflow", image_ok=True)
    env = doc.get("env") if isinstance(doc.get("env"), dict) else {}
    if not IMAGE_PIN.fullmatch(str(env.get("LOCKSTEP_IMAGE", ""))):
        out.append("P4 env.LOCKSTEP_IMAGE must be docker.io/library/rust:<version>-bookworm@sha256:<64 hex>")
    jobs = doc.get("jobs")
    if not isinstance(jobs, dict):
        return [*out, "P1 the workflow has no jobs mapping"]
    if set(jobs) != EXPECTED_JOBS:
        out.append(f"P1 the jobs are {sorted(map(str, jobs))}, expected {sorted(EXPECTED_JOBS)}")
    out += [f"P9 {w} reads the needs context outside an if:" for w in _reads(doc, NEEDS_CTX, skip_if=True)]
    for name in sorted(EXPECTED_JOBS & set(jobs)):
        job = jobs[name]
        if not isinstance(job, dict):
            out.append(f"P1 job {name} is not a mapping")
            continue
        if job.get("permissions") != PERMISSIONS:
            out.append(f"P10 job {name} permissions are {job.get('permissions')!r}, expected {PERMISSIONS}")
        for key in ("container", "services", "uses", "secrets", "defaults"):
            if key in job:
                out.append(f"P12 job {name} declares {key}:")
        out += _env_violations(job.get("env"), f"jobs.{name}")
        if job.get("continue-on-error", False) not in (False, "false"):
            out.append(f"P14 job {name}: continue-on-error must be absent or false")
        out += step_violations(name, _steps(job, name))
    build, propose = jobs.get("build"), jobs.get("propose")
    if isinstance(build, dict):
        if "environment" in build:
            out.append("P3 build declares an environment")
        out += [f"P3 {w} reads the secrets context in build" for w in _reads(build, SECRETS_CTX, "$.jobs.build")]
        if build.get("outputs") != BUILD_OUTPUTS:
            out.append(f"P9 build outputs are {build.get('outputs')!r}, expected {BUILD_OUTPUTS}")
    if isinstance(propose, dict):
        out += _propose_violations(propose)
    out += _push_violations(jobs)
    return out


def _propose_violations(propose: dict) -> list[str]:
    out = []
    if propose.get("environment") != "release-pr":
        out.append(f"P11 propose environment is {propose.get('environment')!r}, expected 'release-pr'")
    if _needs_list(propose.get("needs")) != ["build"]:
        out.append(f"P11 propose needs is {propose.get('needs')!r}, expected build")
    if str(propose.get("if", "")).strip() != "needs.build.result == 'success'":
        out.append("P11 propose if: must be exactly needs.build.result == 'success'")
    steps = _steps(propose, "propose")
    ids = tuple(str(s.get("id")) for s in steps)
    if ids != PROPOSE_ORDER:
        out.append(f"P7 the propose step order is {list(ids)}, expected {list(PROPOSE_ORDER)}")
    by_id = {str(s.get("id")): s for s in steps}
    verify = by_id.get("verify", {})
    verify_cmds = commands(str(verify.get("run", "")))[0]
    if not any(c[:3] == ["python3", CHECKER, "artifact"] for c in verify_cmds):
        out.append(f"P7 the verify step does not run `python3 {CHECKER} artifact`")
    token = by_id.get("token", {})
    if not str(token.get("uses", "")).startswith("actions/create-github-app-token@") or token.get("with") != TOKEN_WITH:
        out.append(f"P16 the token step must use actions/create-github-app-token with exactly {sorted(TOKEN_WITH)}")
    for index, step in enumerate(steps):
        if step.get("id") == "token":
            continue
        reads = _reads(step, SECRETS_CTX, f"$.jobs.propose.steps[{index}]")
        out += [f"P3 {w} reads the secrets context outside the token step" for w in reads]
    rest = {k: v for k, v in propose.items() if k != "steps"}
    out += [f"P3 {w} reads the secrets context in propose" for w in _reads(rest, SECRETS_CTX, "$.jobs.propose")]
    return out


def _push_violations(jobs: dict) -> list[str]:
    pushes = []
    for name, job in jobs.items():
        if not isinstance(job, dict) or not isinstance(job.get("steps"), list):
            continue
        for step in job["steps"]:
            if not isinstance(step, dict):
                continue
            for cmd in commands(str(step.get("run", "")))[0]:
                if cmd[0] == "git" and "push" in cmd[1:]:
                    pushes.append((name, step.get("id"), cmd))
    if len(pushes) != 1 or pushes[0][:2] != ("propose", "push"):
        return [f"P8 expected exactly one git push, in propose step push; found {[(p[0], p[1]) for p in pushes]}"]
    cmd = pushes[0][2]
    args = cmd[cmd.index("push") + 1:]
    out = []
    leases = [a for a in args if a.startswith(LEASE_PREFIX)]
    other_options = [a for a in args if a.startswith("-") and not a.startswith(LEASE_PREFIX)]
    positional = [a for a in args if not a.startswith("-")]
    if len(leases) != 1:
        out.append(f"P8 the push needs exactly one {LEASE_PREFIX}<sha> option")
    if other_options:
        out.append(f"P8 the push carries other options {other_options}")
    if positional != [PUSH_REMOTE, REFSPEC]:
        out.append(f"P8 the push target is {positional}, expected {[PUSH_REMOTE, REFSPEC]}")
    return out


# ---- the self-test (T3) -----------------------------------------------------------------------

FIXTURE = """\
name: wasm-lockstep
on:
  schedule:
    - cron: '17 6 * * 2'
  workflow_dispatch:
permissions:
  contents: read
env:
  LOCKSTEP_IMAGE: docker.io/library/rust:1.95.0-bookworm@sha256:""" + "a" * 64 + """
jobs:
  build:
    runs-on: ubuntu-latest
    permissions:
      contents: read
    outputs:
      changed: ${{ steps.lock.outputs.changed }}
    steps:
      - id: checkout
        uses: actions/checkout@""" + "1" * 40 + """
        with:
          persist-credentials: false
      - id: update
        run: |
          docker run --rm --cap-drop=ALL --security-opt=no-new-privileges --user 65534:65534 --volume "$RUNNER_TEMP/work:/work" --workdir /work "$LOCKSTEP_IMAGE" bash ci/wasm-lockstep/container.sh update
      - id: lock
        run: |
          rc=0
          python3 ci/wasm-lockstep/lockstep_check.py lock --old a --new b > "$RUNNER_TEMP/v.txt" || rc=$?
          if [ "$rc" -eq 0 ]; then
            echo "changed=true" >> "$GITHUB_OUTPUT"
          fi
      - id: upload
        if: steps.lock.outputs.changed == 'true'
        uses: actions/upload-artifact@""" + "2" * 40 + """
  propose:
    needs: build
    if: needs.build.result == 'success'
    runs-on: ubuntu-latest
    environment: release-pr
    permissions:
      contents: read
    steps:
      - id: checkout
        if: needs.build.outputs.changed == 'true'
        uses: actions/checkout@""" + "1" * 40 + """
        with:
          persist-credentials: false
      - id: download
        if: needs.build.outputs.changed == 'true'
        uses: actions/download-artifact@""" + "3" * 40 + """
      - id: verify
        if: needs.build.outputs.changed == 'true'
        run: python3 ci/wasm-lockstep/lockstep_check.py artifact --dir d --old rs/Cargo.lock
      - id: apply
        if: needs.build.outputs.changed == 'true'
        run: cp a b
      - id: token
        uses: actions/create-github-app-token@""" + "4" * 40 + """
        with:
          client-id: ${{ secrets.PAIGASUS_BOT_APP_ID }}
          private-key: ${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}
          permission-contents: write
          permission-pull-requests: write
      - id: commit
        if: needs.build.outputs.changed == 'true'
        run: git commit -F "$RUNNER_TEMP/pr-title.txt"
      - id: base
        if: needs.build.outputs.changed == 'true'
        env:
          GH_TOKEN: ${{ github.token }}
        run: |
          now="$(gh api "repos/${GITHUB_REPOSITORY}/git/ref/heads/main" --jq .object.sha)"
          echo "moved=false" >> "$GITHUB_OUTPUT"
      - id: push
        if: needs.build.outputs.changed == 'true' && steps.base.outputs.moved == 'false'
        env:
          PUSH_TOKEN: ${{ steps.token.outputs.token }}
        run: |
          git push --force-with-lease="refs/heads/deps/wasm-bindgen-lockstep:${lease}" "https://x-access-token:${PUSH_TOKEN}@github.com/${GITHUB_REPOSITORY}.git" HEAD:refs/heads/deps/wasm-bindgen-lockstep
      - id: pr
        if: needs.build.outputs.changed == 'true' && steps.base.outputs.moved == 'false'
        run: |
          gh pr list --head deps/wasm-bindgen-lockstep --state open --json number,isCrossRepository --jq '.[] | select(.isCrossRepository | not) | .number'
          gh pr create --title "$(cat "$RUNNER_TEMP/pr-title.txt")" --body "run cargo update"
      - id: close
        if: needs.build.outputs.changed == 'false'
        run: |
          gh pr list --head deps/wasm-bindgen-lockstep --state open --json number,isCrossRepository --jq '.[] | select(.isCrossRepository | not) | .number'
"""

PUSH_LINE = 'git push --force-with-lease="refs/heads/deps/wasm-bindgen-lockstep:${lease}" "https://x-access-token:${PUSH_TOKEN}@github.com/${GITHUB_REPOSITORY}.git" HEAD:refs/heads/deps/wasm-bindgen-lockstep'
DOCKER_LINE = 'docker run --rm --cap-drop=ALL --security-opt=no-new-privileges --user 65534:65534 --volume "$RUNNER_TEMP/work:/work" --workdir /work "$LOCKSTEP_IMAGE"'
TOKEN_STEP = "      - id: token\n        uses: actions/create-github-app-token@" + "4" * 40 + """
        with:
          client-id: ${{ secrets.PAIGASUS_BOT_APP_ID }}
          private-key: ${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}
          permission-contents: write
          permission-pull-requests: write
"""
VERIFY_RUN = "        run: python3 ci/wasm-lockstep/lockstep_check.py artifact --dir d --old rs/Cargo.lock\n"
VERIFY_STEP = """      - id: verify
        if: needs.build.outputs.changed == 'true'
        run: python3 ci/wasm-lockstep/lockstep_check.py artifact --dir d --old rs/Cargo.lock
"""
PR_LIST = "gh pr list --head deps/wasm-bindgen-lockstep --state open --json number,isCrossRepository --jq '.[] | select(.isCrossRepository | not) | .number'"
BUILD_STEP_ANCHOR = "      - id: update\n"
PROPOSE_RUN = "        run: cp a b\n"
BUILD_RUN = "          rc=0\n"

# (label, (old, new) replacements on FIXTURE, rule id that must appear or "PASS")
SELF_TEST_ROWS: tuple[tuple[str, tuple[tuple[str, str], ...], str], ...] = (
    ("the fixture passes, its PR body text says cargo update", (), "PASS"),
    ("needs as a list", (("    needs: build\n", "    needs: [build]\n"),), "PASS"),
    ("a boolean if: false on a step", (("      - id: close\n        if: needs.build.outputs.changed == 'false'\n", "      - id: close\n        if: false\n"),), "PASS"),
    ("secrets.X read in build", ((BUILD_STEP_ANCHOR, BUILD_STEP_ANCHOR + "        env:\n          X: ${{ secrets.X }}\n"),), "P3"),
    ("secrets['X'] read in build", ((BUILD_STEP_ANCHOR, BUILD_STEP_ANCHOR + "        env:\n          X: ${{ secrets['X'] }}\n"),), "P3"),
    ("toJSON(secrets) read in build", ((BUILD_STEP_ANCHOR, BUILD_STEP_ANCHOR + "        env:\n          X: ${{ toJSON(secrets) }}\n"),), "P3"),
    ("Secrets.X in another case in build", ((BUILD_STEP_ANCHOR, BUILD_STEP_ANCHOR + "        env:\n          X: ${{ Secrets.X }}\n"),), "P3"),
    ("a bare if: secrets.X in build", ((BUILD_STEP_ANCHOR, BUILD_STEP_ANCHOR + "        if: secrets.X != ''\n"),), "P3"),
    ("a workflow-level env reads a secret", (("env:\n", "env:\n  K: ${{ secrets.X }}\n"),), "P3"),
    ("a YAML alias injects a secrets read into build", (
        ("permissions:\n  contents: read\nenv:\n", "permissions:\n  contents: read\nx-leak: &leak ${{ secrets.X }}\nenv:\n"),
        (BUILD_STEP_ANCHOR, BUILD_STEP_ANCHOR + "        env:\n          X: *leak\n")), "P3"),
    ("a secrets read in propose outside the token step", ((PROPOSE_RUN, PROPOSE_RUN + "        env:\n          K: ${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}\n"),), "P3"),
    ("an environment on build", (("  build:\n    runs-on: ubuntu-latest\n", "  build:\n    runs-on: ubuntu-latest\n    environment: release-pr\n"),), "P3"),
    ("a third job with environment release-pr", (("jobs:\n", "jobs:\n  third:\n    runs-on: x\n    environment: release-pr\n    permissions:\n      contents: read\n    steps:\n      - run: echo\n"),), "P1"),
    ("a job-level uses with secrets: inherit in build", (("  build:\n    runs-on: ubuntu-latest\n", "  build:\n    uses: ./.github/workflows/x.yml\n    secrets: inherit\n    runs-on: ubuntu-latest\n"),), "P12"),
    ("a container: key on build", (("  build:\n    runs-on: ubuntu-latest\n", "  build:\n    container: ubuntu:24.04\n    runs-on: ubuntu-latest\n"),), "P12"),
    ("docker run -e", ((DOCKER_LINE, DOCKER_LINE.replace("--rm", "--rm -e TOKEN")),), "P4"),
    ("docker run --env=X", ((DOCKER_LINE, DOCKER_LINE.replace("--rm", "--rm --env=X")),), "P4"),
    ("docker run --env-file", ((DOCKER_LINE, DOCKER_LINE.replace("--rm", "--rm --env-file f")),), "P4"),
    ("docker run --privileged", ((DOCKER_LINE, DOCKER_LINE.replace("--rm", "--rm --privileged")),), "P4"),
    ("docker run with the docker socket", ((DOCKER_LINE, DOCKER_LINE.replace("--rm", "--rm -v /var/run/docker.sock:/var/run/docker.sock")),), "P4"),
    ("docker run --network=host", ((DOCKER_LINE, DOCKER_LINE.replace("--rm", "--rm --network=host")),), "P4"),
    ("docker run --pid=host", ((DOCKER_LINE, DOCKER_LINE.replace("--rm", "--rm --pid=host")),), "P4"),
    ("docker run --cap-add", ((DOCKER_LINE, DOCKER_LINE.replace("--rm", "--rm --cap-add SYS_PTRACE")),), "P4"),
    ("docker run without --user", ((DOCKER_LINE, DOCKER_LINE.replace(" --user 65534:65534", "")),), "P4"),
    ("docker run as root", ((DOCKER_LINE, DOCKER_LINE.replace("65534:65534", "0:0")),), "P4"),
    ("an image tag without a digest", (("@sha256:" + "a" * 64, ""),), "P4"),
    ("cargo on the build host", ((BUILD_RUN, BUILD_RUN + "          ( cd rs && cargo build )\n"),), "P5"),
    ("a bare ! test -L on the build host", ((BUILD_RUN, BUILD_RUN + '          ! test -L "$RUNNER_TEMP/work/rs"\n'),), "P5"),
    ("a work-copy script on the build host", ((BUILD_RUN, BUILD_RUN + '          bash "$RUNNER_TEMP/work/x.sh"\n'),), "P5"),
    ("the token step before the checker step", ((VERIFY_STEP + "      - id: apply\n        if: needs.build.outputs.changed == 'true'\n        run: cp a b\n" + TOKEN_STEP,
                                                  TOKEN_STEP + VERIFY_STEP + "      - id: apply\n        if: needs.build.outputs.changed == 'true'\n        run: cp a b\n"),), "P7"),
    ("a cargo command in propose", ((PROPOSE_RUN, "        run: cargo update -p wasm-bindgen\n"),), "P5"),
    ("cargo inside $(...) in propose", ((PROPOSE_RUN, '        run: echo "$(cargo metadata)"\n'),), "P5"),
    ("a backtick in propose", ((PROPOSE_RUN, "        run: echo `id`\n"),), "P5"),
    ("bash -c in propose", ((PROPOSE_RUN, "        run: bash -c 'cargo build'\n"),), "P5"),
    ("python3 -c in propose", ((PROPOSE_RUN, "        run: python3 -c 'print(1)'\n"),), "P5"),
    ("env as a command wrapper in propose", ((PROPOSE_RUN, "        run: env cargo build\n"),), "P5"),
    ("cargo after a pipe in propose", ((PROPOSE_RUN, "        run: cat a | cargo build\n"),), "P5"),
    ("a push refspec to refs/heads/main", ((PUSH_LINE, PUSH_LINE.replace("HEAD:refs/heads/deps/wasm-bindgen-lockstep", "HEAD:refs/heads/main")),), "P8"),
    ("a push with --force", ((PUSH_LINE, PUSH_LINE.replace('--force-with-lease="refs/heads/deps/wasm-bindgen-lockstep:${lease}"', "--force")),), "P8"),
    ("a second git push", ((PROPOSE_RUN, "        run: git push origin HEAD:refs/heads/x\n"),), "P8"),
    ("needs.build.outputs.x in a run in propose", ((PROPOSE_RUN, '        run: echo "${{ needs.build.outputs.changed }}"\n'),), "P9"),
    ("needs['build'] in an env in propose", ((PROPOSE_RUN, PROPOSE_RUN + "        env:\n          C: ${{ needs['build'].outputs.changed }}\n"),), "P9"),
    ("NEEDS in another case in a with: in propose", (("          persist-credentials: false\n      - id: download", "          persist-credentials: false\n          ref: ${{ NEEDS.build.outputs.changed }}\n      - id: download"),), "P9"),
    ("a second build output", (("      changed: ${{ steps.lock.outputs.changed }}\n", "      changed: ${{ steps.lock.outputs.changed }}\n      version: x\n"),), "P9"),
    ("the bare on: key replaced by a null value", (("on:\n  schedule:\n    - cron: '17 6 * * 2'\n  workflow_dispatch:\n", "on:\n"),), "P2"),
    ("a quoted on key adds push next to the bare one", (("permissions:\n  contents: read\nenv:", "'on': push\npermissions:\n  contents: read\nenv:"),), "P2"),
    ("a pull_request trigger", (("  workflow_dispatch:\n", "  workflow_dispatch:\n  pull_request:\n"),), "P2"),
    ("a job widens permissions", (("    environment: release-pr\n    permissions:\n      contents: read\n", "    environment: release-pr\n    permissions:\n      contents: write\n"),), "P10"),
    ("top-level write-all", (("permissions:\n  contents: read\nenv:", "permissions: write-all\nenv:"),), "P10"),
    ("propose without the release-pr environment", (("    environment: release-pr\n", ""),), "P11"),
    ("continue-on-error: true on verify", ((VERIFY_STEP, VERIFY_STEP + "        continue-on-error: true\n"),), "P14"),
    ('continue-on-error: "true" (a string) on verify', ((VERIFY_STEP, VERIFY_STEP + '        continue-on-error: "true"\n'),), "P14"),
    ("if: always() on the push step", (("      - id: push\n        if: needs.build.outputs.changed == 'true' && steps.base.outputs.moved == 'false'\n", "      - id: push\n        if: always()\n"),), "P15"),
    ("shell: python on a propose step", ((PROPOSE_RUN, PROPOSE_RUN + "        shell: python\n"),), "P13"),
    ("persist-credentials: true on a checkout", (("        with:\n          persist-credentials: false\n      - id: download", "        with:\n          persist-credentials: true\n      - id: download"),), "P17"),
    ("the token asks for workflows: write", (("          permission-pull-requests: write\n", "          permission-pull-requests: write\n          permission-workflows: write\n"),), "P16"),
    ("an unlisted action in propose", (("actions/download-artifact@" + "3" * 40, "evil/action@" + "3" * 40),), "P6"),
    ("an action pinned to a tag", (("actions/download-artifact@" + "3" * 40, "actions/download-artifact@v8"),), "P6"),
    ("a merge key", (("    outputs:\n", "    <<: {timeout-minutes: 5}\n    outputs:\n"),), "P0"),
    ("a duplicate key", (("    environment: release-pr\n", "    environment: release-pr\n    environment: other\n"),), "P0"),
    # ---- fix round 1: bypasses that passed the first rule set ----
    ("verify joined with || true", ((VERIFY_RUN, VERIFY_RUN[:-1] + " || true\n"),), "P18"),
    ("verify joined with || rc=$?", ((VERIFY_RUN, VERIFY_RUN[:-1] + " || rc=$?\n"),), "P18"),
    ("verify followed by ; true", ((VERIFY_RUN, VERIFY_RUN[:-1] + " ; true\n"),), "P18"),
    ("verify piped into cat", ((VERIFY_RUN, VERIFY_RUN[:-1] + " | cat\n"),), "P18"),
    ("verify behind an && guard", ((VERIFY_RUN, "        run: test -f x && " + VERIFY_RUN.split("run: ", 1)[1]),), "P18"),
    ("verify inside a skipped if", ((VERIFY_RUN, "        run: |\n          if false; then\n            " + VERIFY_RUN.split("run: ", 1)[1].rstrip("\n") + "\n          fi\n"),), "P18"),
    ("verify followed by exit 0", ((VERIFY_RUN, "        run: |\n          " + VERIFY_RUN.split("run: ", 1)[1].rstrip("\n") + "\n          exit 0\n"),), "P18"),
    ("status joined with || true", ((PROPOSE_RUN, "        run: python3 ci/wasm-lockstep/lockstep_check.py status --file s || true\n"),), "P18"),
    ("status as a whole last command", ((PROPOSE_RUN, "        run: |\n          set -euo pipefail\n          git status > s\n          python3 ci/wasm-lockstep/lockstep_check.py status --file s\n"),), "PASS"),
    ("LOCKSTEP_IMAGE assigned in front of docker run", ((DOCKER_LINE, "LOCKSTEP_IMAGE=docker.io/evil/x:latest " + DOCKER_LINE),), "P4"),
    ("LOCKSTEP_IMAGE in a step env of build", ((BUILD_STEP_ANCHOR, BUILD_STEP_ANCHOR + "        env:\n          LOCKSTEP_IMAGE: docker.io/evil/x:latest\n"),), "P4"),
    ("LOCKSTEP_IMAGE in a job env of build", (("  build:\n    runs-on: ubuntu-latest\n", "  build:\n    runs-on: ubuntu-latest\n    env:\n      LOCKSTEP_IMAGE: docker.io/evil/x:latest\n"),), "P4"),
    ("LOCKSTEP_IMAGE set with read", ((BUILD_RUN, BUILD_RUN + "          read -r LOCKSTEP_IMAGE < x\n"),), "P4"),
    ("a process substitution in propose: cat <(...)", ((PROPOSE_RUN, "        run: cat <(cargo build)\n"),), "P5"),
    ("a process substitution in propose: > >(...)", ((PROPOSE_RUN, "        run: echo x > >(cargo build)\n"),), "P5"),
    ("working-directory on a build step", ((BUILD_STEP_ANCHOR, BUILD_STEP_ANCHOR + "        working-directory: /tmp\n"),), "P19"),
    ("working-directory on a propose step", ((PROPOSE_RUN, PROPOSE_RUN + "        working-directory: /tmp\n"),), "P19"),
    ("defaults.run.working-directory on a job", (("  build:\n    runs-on: ubuntu-latest\n", "  build:\n    runs-on: ubuntu-latest\n    defaults:\n      run:\n        working-directory: /tmp\n"),), "P12"),
    ("defaults.run.working-directory on the workflow", (("permissions:\n  contents: read\nenv:", "permissions:\n  contents: read\ndefaults:\n  run:\n    working-directory: /tmp\nenv:"),), "P13"),
    ("BASH_ENV in a step env", ((BUILD_STEP_ANCHOR, BUILD_STEP_ANCHOR + "        env:\n          BASH_ENV: /x\n"),), "P19"),
    ("ENV in a job env", (("  propose:\n", "  propose:\n    env:\n      ENV: /x\n"),), "P19"),
    ("BASH_ENV in the workflow env", (("env:\n", "env:\n  BASH_ENV: /x\n"),), "P19"),
    ("a write to GITHUB_ENV", ((BUILD_RUN, BUILD_RUN + '          echo "BASH_ENV=x" >> "$GITHUB_ENV"\n'),), "P19"),
    ("a write to GITHUB_PATH in brace form", ((BUILD_RUN, BUILD_RUN + "          echo /x >>${GITHUB_PATH}\n"),), "P19"),
    ("a write to GITHUB_ENV, unquoted", ((BUILD_RUN, BUILD_RUN + "          echo x > $GITHUB_ENV\n"),), "P19"),
    ("gh api -X POST in propose", ((PROPOSE_RUN, "        run: gh api repos/x/y -X POST\n"),), "P20"),
    ("gh api --method=DELETE in propose", ((PROPOSE_RUN, "        run: gh api repos/x/y --method=DELETE\n"),), "P20"),
    ("gh api -XPUT in propose", ((PROPOSE_RUN, "        run: gh api repos/x/y -XPUT\n"),), "P20"),
    ("gh api -f in propose (an implicit POST)", ((PROPOSE_RUN, "        run: gh api repos/x/y -f a=b\n"),), "P20"),
    ("gh api graphql in propose", ((PROPOSE_RUN, "        run: gh api graphql\n"),), "P20"),
    ("gh api --method GET in propose", ((PROPOSE_RUN, "        run: gh api repos/x/y --method GET\n"),), "PASS"),
    ("git -c before the subcommand in propose", ((PROPOSE_RUN, "        run: git -c core.fsmonitor=x status\n"),), "P20"),
    ("git -ckey=value in propose", ((PROPOSE_RUN, "        run: git -ccore.pager=x log\n"),), "P20"),
    ("git -c in build", ((BUILD_RUN, BUILD_RUN + "          git -c core.x=y archive HEAD\n"),), "P20"),
    ("sudo chown of the work copy to nobody in build", ((BUILD_RUN, BUILD_RUN + '          sudo chown -R 65534:65534 "$RUNNER_TEMP/work"\n'),), "PASS"),
    ("sudo chown of another path in build", ((BUILD_RUN, BUILD_RUN + '          sudo chown -R 65534:65534 /etc\n'),), "P5"),
    ("sudo chown to root in build", ((BUILD_RUN, BUILD_RUN + '          sudo chown -R 0:0 "$RUNNER_TEMP/work"\n'),), "P5"),
    ("sudo bash in build", ((BUILD_RUN, BUILD_RUN + '          sudo bash x.sh\n'),), "P5"),
    ("sudo chown in propose", ((PROPOSE_RUN, '        run: sudo chown -R 65534:65534 "$RUNNER_TEMP/work"\n'),), "P5"),
    ("gh pr list --head without the cross-repo filter", ((PR_LIST, "gh pr list --head deps/wasm-bindgen-lockstep --state open --json number --jq '.[].number'"),), "P21"),
    ("gh pr list with --json but a jq that does not select", ((PR_LIST, PR_LIST.replace("select(.isCrossRepository | not) | ", "")),), "P21"),
    ("gh pr list that selects the fork pull requests", ((PR_LIST, PR_LIST.replace("| not)", ")")),), "P21"),
    ("docker run without --cap-drop=ALL", ((DOCKER_LINE, DOCKER_LINE.replace(" --cap-drop=ALL", "")),), "P4"),
    ("docker run without --security-opt=no-new-privileges", ((DOCKER_LINE, DOCKER_LINE.replace(" --security-opt=no-new-privileges", "")),), "P4"),
    ("docker run with --cap-drop=NET_RAW only", ((DOCKER_LINE, DOCKER_LINE.replace("--cap-drop=ALL", "--cap-drop=NET_RAW")),), "P4"),
    ("needs in an env key named if in propose", ((PROPOSE_RUN, PROPOSE_RUN + "        env:\n          if: ${{ needs.build.outputs.changed }}\n"),), "P9"),
    ("continue-on-error: true on the build job", (("  build:\n    runs-on: ubuntu-latest\n", "  build:\n    runs-on: ubuntu-latest\n    continue-on-error: true\n"),), "P14"),
    ('continue-on-error: "true" on the propose job', (("    environment: release-pr\n", "    environment: release-pr\n    continue-on-error: \"true\"\n"),), "P14"),
)


def _rules_of(text: str) -> list[str]:
    try:
        return violations(parse_text(text, "fixture"))
    except AssertionFailureError as exc:
        return [str(exc)]


def self_test() -> int:
    failures = 0
    for label, replacements, want in SELF_TEST_ROWS:
        text = FIXTURE
        drift = [old for old, _new in replacements if old not in text]
        for old, new in replacements:
            text = text.replace(old, new, 1)
        if drift:
            print(f"  FAIL  {label}: the fixture no longer holds {drift[0]!r}, so the row tests nothing", file=sys.stderr)
            failures += 1
            continue
        got = _rules_of(text)
        ok = not got if want == "PASS" else any(v.startswith(want + " ") for v in got)
        if ok:
            print(f"  ok    {label}: {want}")
        else:
            print(f"  FAIL  {label}: want {want}, got {got}", file=sys.stderr)
            failures += 1
    print(f"pin_check self-test: {len(SELF_TEST_ROWS)} rows, {failures} failed")
    return RC_ASSERT if failures else RC_OK


def negative_control(path: str) -> int:
    """Mutations of the REAL workflow. The real one must pass, and each mutation must red."""
    real = load(path)
    failures = 0
    base = violations(real)
    if base:
        print(f"  FAIL  the real workflow does not pass, so no mutation can prove anything: {base}", file=sys.stderr)
        return RC_ASSERT

    def expect(label: str, mutated: dict, want: str) -> None:
        nonlocal failures
        got = violations(mutated)
        if any(v.startswith(want + " ") for v in got):
            print(f"  ok    {label}: {want}")
        else:
            print(f"  FAIL  {label}: want {want}, got {got}", file=sys.stderr)
            failures += 1

    leak = copy.deepcopy(real)
    leak["jobs"]["build"]["steps"][0].setdefault("env", {})["LEAK"] = "${{ secrets.PAIGASUS_BOT_PRIVATE_KEY }}"
    expect("a secrets read added to build", leak, "P3")
    order = copy.deepcopy(real)
    steps = order["jobs"]["propose"]["steps"]
    ids = [s.get("id") for s in steps]
    v, t = ids.index("verify"), ids.index("token")
    steps[v], steps[t] = steps[t], steps[v]
    expect("the token step moved before the verify step", order, "P7")
    target = copy.deepcopy(real)
    for step in target["jobs"]["propose"]["steps"]:
        if step.get("id") == "push":
            step["run"] = step["run"].replace(REFSPEC, "HEAD:refs/heads/main")
    expect("the push refspec moved to main", target, "P8")
    output = copy.deepcopy(real)
    for step in output["jobs"]["propose"]["steps"]:
        if step.get("id") == "apply":
            step.setdefault("env", {})["C"] = "${{ needs.build.outputs.changed }}"
    expect("needs.build.outputs read in an env of propose", output, "P9")
    print(f"pin_check negative control: 4 mutations, {failures} failed")
    return RC_ASSERT if failures else RC_OK


def main(argv: list[str]) -> int:
    if argv == ["--self-test"]:
        return self_test()
    if len(argv) == 2 and argv[0] == "--negative-control":
        return negative_control(argv[1])
    if len(argv) != 1 or argv[0].startswith("-"):
        raise InfraError("usage: pin_check.py <workflow.yml> | --self-test | --negative-control <workflow.yml>")
    found = violations(load(argv[0]))
    for line in found:
        print(f"pin_check: {line}", file=sys.stderr)
    if found:
        return RC_ASSERT
    print(f"pin_check: {argv[0]} satisfies P0-P21")
    return RC_OK


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except AssertionFailureError as exc:
        print(f"pin_check: {exc}", file=sys.stderr)
        sys.exit(RC_ASSERT)
    except InfraError as exc:
        print(f"pin_check: infrastructure error: {exc}", file=sys.stderr)
        sys.exit(RC_INFRA)
