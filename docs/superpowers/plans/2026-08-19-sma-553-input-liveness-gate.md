# SMA-553 Input-Liveness Gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Assert that every `repo:*` Moon task's declared `inputs` still match at least one git-tracked file, so a gate that is wired into CI and resolvable cannot silently stop firing.

**Architecture:** A new Python script `ci/affected-graph/task_inputs.py`, scheduled by its own `repo:input-liveness` Moon task with `inputs: ['**/*']` (a narrower input set would serve a cached PASS on exactly the rename that kills a gate). It reads one `moon query tasks` call and matches every declared pattern against `git ls-files -- ":(glob)…"`. Pure functions carry all the logic so `--self-test` can drive them from in-memory fixtures; the subprocess wrappers are thin.

**Tech Stack:** Python 3 (stdlib only — `json`, `re`, `subprocess`, `sys`, `pathlib`), git pathspec `:(glob)` matching, Moon 2.3.2, bash (Moon `script:` blocks).

**Spec:** `docs/superpowers/specs/2026-08-19-sma-553-input-liveness-gate-design.md`. Read §3 (design decisions D1-D13) before starting; every task below cites the decisions it implements.

## Global Constraints

- **Every source file opens with `# SPDX-License-Identifier: Apache-2.0`** (first line, `#` for Python).
- **Python stdlib only.** `repo:affected-smoke` and `repo:input-liveness` are `toolchain: 'system'`, so no third-party import is available. `tomllib` is stdlib and is used by the sibling; nothing here needs it.
- **`ci/affected-graph/*.py` is NOT linted** — `py/pyproject.toml:12` scopes ruff/basedpyright to `packages/*/src/**` and `packages/*/tests/**`. Match the two sibling scripts' style by hand: module docstring-style `#` header, comments explaining *why*, not *what*.
- **Exit codes are 0 / 1 / 2 and nothing else.** 0 pass, 1 assertion failure (an authorial mistake, message names what to edit), 2 infrastructure error (`moon`/`git` failed, output will not parse, shape lacks a needed key). Never use rc 2 for an authorial mistake — SMA-541 D2.
- **Never parse YAML.** Everything about the Moon graph comes from `moon query tasks`. This is the sibling scripts' stated rule and it is what makes them formatting-proof.
- **`INJECTED_GLOB` is the exact string** `.moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}` — Moon-injected onto all 119 tasks graph-wide.
- **The new Moon task is named `input-liveness`**, target `repo:input-liveness`, entry `:input-liveness`. Not a Windows reserved device name; safe.
- **Commits are Conventional Commits with a workspace scope**, subject starts **lowercase**, header ≤100 chars. No `#NNN` issue refs in the body (breaks commitlint's `footer-leading-blank`). Write "SMA-553" in the subject's trailing parens.
- **Run every command from the worktree root** `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-553`, and prefix Moon/proto commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.

---

### Task 1: Pattern classifier (`classify`) and the script skeleton

Implements **D6** (default-deny pattern vocabulary) and the D12 error types. Deliverable: `--self-test` runs and passes, exercising every verdict token.

**Files:**
- Create: `ci/affected-graph/task_inputs.py`

**Interfaces:**
- Consumes: nothing.
- Produces: `classify(pattern) -> str` returning one of `"ok"`, `"negated"`, `"rejected-braces"`, `"rejected-charclass"`, `"rejected-charset"`, `"rejected-dotty"`, `"rejected-globstar"`. Also `GateAssertionError`, `MoonOutputError`, `INFRA_ERRORS`, `INJECTED_GLOB`, and `self_test() -> int` / `main() -> int`, all extended by later tasks.

- [ ] **Step 1: Write the failing self-test**

Create `ci/affected-graph/task_inputs.py` containing only the header, the constants, an empty `classify`, and a `self_test` with the classifier fixtures:

```python
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

# Moon injects this onto EVERY task in the graph — all 119 across all 28 projects, including a task
# declaring literally `inputs: []` (spec E2). So a "resolved input set" is never empty, and the
# empty-inputs check (I3) asserts nothing until this is subtracted. D4.
INJECTED_GLOB = ".moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}"

# The conservative charset a pattern must fall inside before it is handed to git. Doubles as the
# pathspec-injection guard: a pattern starting with ':' would be read by git as pathspec magic, and
# the `--` separator plus quoting is necessary but NOT sufficient.
SAFE_CHARS_RE = re.compile(r"[A-Za-z0-9._/*-]+")


def classify(pattern):
    raise NotImplementedError


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

    if failures:
        print("task-inputs self-test FAILED:", file=sys.stderr)
        for f in failures:
            print(f"  {f}", file=sys.stderr)
        return 1
    print("task-inputs self-test OK")
    return 0


def main():
    raise NotImplementedError


if __name__ == "__main__":
    sys.exit(self_test() if "--self-test" in sys.argv[1:] else main())
```

- [ ] **Step 2: Run it to verify it fails**

Run: `python3 ci/affected-graph/task_inputs.py --self-test`
Expected: FAIL — `NotImplementedError` from `classify`.

- [ ] **Step 3: Implement `classify`**

Replace the `classify` stub. **The order of the checks is the contract** — each rejected form is also caught by a later, more generic rule, so a specific message only exists if it is tested first. This mirrors `ci/actionlint/run.sh:936-941`, which documents the same ordering trap.

```python
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
```

- [ ] **Step 4: Run it to verify it passes**

Run: `python3 ci/affected-graph/task_inputs.py --self-test`
Expected: PASS — prints `task-inputs self-test OK`.

- [ ] **Step 5: Commit**

```bash
git add ci/affected-graph/task_inputs.py
git commit -m "feat(repo): add the input-liveness pattern classifier (SMA-553)"
```

---

### Task 2: Moon ingestion (`_repo_tasks`) with the D4 composition guard

Implements **D4** (authored inputs, guarded by composition) and **D8** (one `moon query tasks`, exact project key). Deliverable: the script can read the real graph, and `--self-test` proves all four rc-2 shape raises fire.

**Files:**
- Modify: `ci/affected-graph/task_inputs.py`

**Interfaces:**
- Consumes: `MoonOutputError`, `INJECTED_GLOB` from Task 1.
- Produces: `_repo_tasks(projects) -> dict[str, tuple[list[str], list[str]]]` mapping task name to `(sorted inputGlobs, sorted inputFiles)`, raising `MoonOutputError` on any shape violation; and `moon_tasks() -> dict` (the subprocess wrapper). Also `authored(globs) -> list[str]`.

- [ ] **Step 1: Write the failing self-test**

Append these fixtures to `self_test()`, immediately before the `if failures:` block:

```python
    # --- _repo_tasks shape rules and the D4 composition guard (rc 2) --------------------------
    def raises_moon(label, projects):
        try:
            _repo_tasks(projects)
        except MoonOutputError:
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
    raises_moon("a non-dict inputGlobs", {"repo": {"promtool": {"inputGlobs": []}}})
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
```

- [ ] **Step 2: Run it to verify it fails**

Run: `python3 ci/affected-graph/task_inputs.py --self-test`
Expected: FAIL with `NameError: name '_repo_tasks' is not defined`.

- [ ] **Step 3: Implement `authored`, `_repo_tasks` and `moon_tasks`**

Insert after `classify`:

```python
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
    shape rule — the same split ci_targets.py uses (`moon_tasks`/`_eligibility`).
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
```

- [ ] **Step 4: Run the self-test and a real-graph smoke check**

Run: `python3 ci/affected-graph/task_inputs.py --self-test`
Expected: PASS.

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 -c "
import sys; sys.path.insert(0, 'ci/affected-graph')
import task_inputs as t
rows = t.moon_tasks()
print(len(rows), 'repo tasks')
print('promtool authored globs:', t.authored(rows['promtool'][0]))
"
```
Expected: `18 repo tasks` and `promtool authored globs: ['ops/observability/prometheus/**/*']`.

- [ ] **Step 5: Commit**

```bash
git add ci/affected-graph/task_inputs.py
git commit -m "feat(repo): read the repo task input sets from moon's graph (SMA-553)"
```

---

### Task 3: The git matcher and the live-fire canaries

Implements **D5** (matcher, tracked predicate, failure polarity, cwd) and **D7** (canaries on every real run). Deliverable: the script can decide liveness against the real tree, and cannot pass with a stuck matcher.

**Files:**
- Modify: `ci/affected-graph/task_inputs.py`

**Interfaces:**
- Consumes: `MoonOutputError` from Task 1.
- Produces: `tracked_files(root) -> set[str]`, `git_matcher(root) -> callable(pattern) -> int`, `check_canaries(matcher) -> list[str]`, and the constants `CANARY_DEAD` / `CANARY_LIVE`.

**Note for the implementer:** `git ls-files` exits **0** even for a malformed pattern — measured: `git ls-files -- ":(glob)[bad"` returns rc 0 and no output. So the non-zero-rc rule below will fire only when git is genuinely broken or absent. `classify()` (Task 1) is the real defense against a malformed pattern; git will not complain, it will just silently return nothing, which reads as `dead` — a false red, which is the safe direction.

- [ ] **Step 1: Write the failing self-test**

Add the constants near `SAFE_CHARS_RE`:

```python
# D7 — live-fire canaries, run on EVERY real invocation, not only under --self-test. This is the
# one failure the fixture table cannot catch: a matcher stuck returning "live" passes I1, I2 and I4
# vacuously while every check still prints PASS. Following ci/actionlint/run.sh:1449-1459 ("the
# self-tests, invoked for real"), which calls its fixture tables unconditionally so they are not
# dead code in CI. Costs one extra `git` call each.
CANARY_DEAD = "zz-no-such-directory-sma553/**/*"
CANARY_LIVE = "ci/affected-graph/*.py"
```

Append to `self_test()` before the `if failures:` block:

```python
    # --- canaries (D7) ------------------------------------------------------------------------
    if check_canaries(lambda p: 0 if p == CANARY_DEAD else 3):
        failures.append("check_canaries: fired on a healthy matcher")
    # The failure this exists for: a matcher that says everything is live. Every other check passes
    # vacuously under it.
    if not check_canaries(lambda p: 3):
        failures.append("check_canaries: missed a matcher stuck returning live")
    if not check_canaries(lambda p: 0):
        failures.append("check_canaries: missed a matcher stuck returning dead")
```

- [ ] **Step 2: Run it to verify it fails**

Run: `python3 ci/affected-graph/task_inputs.py --self-test`
Expected: FAIL with `NameError: name 'check_canaries' is not defined`.

- [ ] **Step 3: Implement the matcher and canaries**

Insert after `moon_tasks`:

```python
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
```

- [ ] **Step 4: Run the self-test and a real-tree check**

Run: `python3 ci/affected-graph/task_inputs.py --self-test`
Expected: PASS.

Run:
```bash
python3 -c "
import sys; sys.path.insert(0, 'ci/affected-graph')
from pathlib import Path
import task_inputs as t
root = Path('.').resolve()
m = t.git_matcher(root)
print('tracked:', len(t.tracked_files(root)))
print('promtool glob:', m('ops/observability/prometheus/**/*'))
print('canaries:', t.check_canaries(m))
"
```
Expected: `tracked: 714`, `promtool glob: 7`, `canaries: []`.

- [ ] **Step 5: Commit**

```bash
git add ci/affected-graph/task_inputs.py
git commit -m "feat(repo): match declared inputs against git's tracked set (SMA-553)"
```

---

### Task 4: The checks (I1-I5) and the allowlist

Implements **I1-I5**, **D11** (`ALLOW_DEAD_INPUT`) and **D13** (the gate asserts its own inputs). Deliverable: `check()` returns violation rows for every failure mode, all fixtured.

**Files:**
- Modify: `ci/affected-graph/task_inputs.py`

**Interfaces:**
- Consumes: `classify`, `authored`, `INJECTED_GLOB` from Tasks 1-2.
- Produces: `check(tasks, tracked, matcher) -> list[tuple[str, str]]` where each row is `(kind, message)` and `kind` is one of `"dead"`, `"not-exact"`, `"no-inputs"`, `"rejected"`, `"floor"`, `"self-inputs"`, `"allowlist"`. Also the constants `REQUIRED_TASKS`, `SELF_TASK`, `SELF_EXPECTED_GLOBS`, `ALLOW_DEAD_INPUT`.

- [ ] **Step 1: Write the failing self-test**

Add the constants after `CANARY_LIVE`:

```python
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
# An entry is a RECORDED DECISION: the reason string is required, and a blank one is itself an
# assertion failure. Mirrors cargo_moon_parity.py's ALLOW_NO_CARGO_BACKING.
ALLOW_DEAD_INPUT = {}
```

Append to `self_test()` before the `if failures:` block:

```python
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

    # Allowlist (D11).
    dead_globs = task_set(promtool=(["ops-moved/**/*", INJECTED_GLOB], [".prototools"]))
    if kinds(dead_globs, allow={("promtool", "ops-moved/**/*"): "reason"}) != []:
        failures.append("check: an allowlisted dead glob still fired")
    dead_file = task_set(promtool=(["ops/**/*", INJECTED_GLOB], ["gone.toml"]))
    if kinds(dead_file, allow={("promtool", "gone.toml"): "reason"}) != []:
        failures.append("check: the allowlist does not cover inputFiles")
    if kinds(dead_globs, allow={("promtool", "ops-moved/**/*"): ""}) != ["allowlist", "dead"]:
        failures.append("check: an allowlist entry with a blank reason must itself be a violation")
    if "allowlist" not in kinds(task_set(), allow={("ghost", "x/**"): "reason"}):
        failures.append("check: an allowlist entry naming no repo task must fire")
    if "allowlist" not in kinds(task_set(), allow={("promtool", "never-declared/**"): "reason"}):
        failures.append("check: an allowlist entry naming an undeclared pattern must fire")
```

- [ ] **Step 2: Run it to verify it fails**

Run: `python3 ci/affected-graph/task_inputs.py --self-test`
Expected: FAIL with `NameError: name 'check' is not defined`.

- [ ] **Step 3: Implement `check`**

Insert after `check_canaries`:

```python
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
            if (name, pattern) in allow:
                continue
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
            if matcher(pattern) == 0:
                rows.append((
                    "dead",
                    f"repo:{name}'s input glob {pattern!r} matches no tracked file, so Moon will "
                    "never schedule that task on a change to what it is meant to guard. Usually a "
                    "moved or renamed directory: update the glob in moon.yml. If it is genuinely "
                    "meant to match nothing, add an ALLOW_DEAD_INPUT entry with a reason."
                ))
        for path in files:
            if (name, path) in allow:
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
        if not reason:
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
        elif pattern not in tasks[name][0] and pattern not in tasks[name][1]:
            rows.append((
                "allowlist",
                f"ALLOW_DEAD_INPUT exempts {pattern!r} on repo:{name}, which declares no such "
                "input in either inputGlobs or inputFiles. Same staleness class as above."
            ))
    return rows
```

- [ ] **Step 4: Run it to verify it passes**

Run: `python3 ci/affected-graph/task_inputs.py --self-test`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add ci/affected-graph/task_inputs.py
git commit -m "feat(repo): assert repo task inputs are live, evaluable and non-empty (SMA-553)"
```

---

### Task 5: `main()`, the Moon task, and the CI/docs wiring

Implements **D2** (standalone task), **D3** (negative control first) and **D12** (exit codes). Deliverable: `moon run repo:input-liveness` passes on the real tree, and the gate is wired into CI.

**Files:**
- Modify: `ci/affected-graph/task_inputs.py`
- Modify: `moon.yml` (new task, inserted alphabetically among the `repo` tasks)
- Modify: `.github/workflows/ci.yml:215`
- Modify: `CLAUDE.md` (marker region + a gotcha line)
- Modify: `ci/affected-graph/README.md`

**Interfaces:**
- Consumes: everything from Tasks 1-4.
- Produces: a working `repo:input-liveness` target.

- [ ] **Step 1: Implement `main`**

Replace the `main` stub:

```python
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
```

- [ ] **Step 2: Run it against the real tree**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/task_inputs.py; echo "rc=$?"
```
Expected: **rc=1**, with exactly one `self-inputs` row — `repo:input-liveness` does not exist yet, so the floor also fires. Both are correct: the task is added in the next step. Confirm the messages name `input-liveness`.

- [ ] **Step 3: Add the Moon task**

In `moon.yml`, insert this block among the `repo` tasks, keeping the file's existing ordering convention (place it after `iam-docker-policy-single-site` and before `install-hooks`):

```yaml
  input-liveness:
    description: 'Assert every repo:* task input still matches a tracked file, so a wired gate cannot silently stop firing (SMA-553).'
    # inputs MUST stay ['**/*'] — asserted by this gate's own D13 floor AND by ci_targets.py, so
    # neither is the sole judge of its own configuration. The verdict depends on the WHOLE tracked
    # tree because a glob dies when files MOVE, and no narrow input list can observe that. This is
    # also why the check is not folded into ci/affected-graph/run.sh: repo:affected-smoke's narrow
    # inputs list no `ops/` path, so renaming ops/ would leave it serving a cached PASS on exactly
    # the change that kills repo:promtool (SMA-553 D2). Same reasoning repo:actionlint records.
    #
    # Negative control FIRST, mirroring repo:affected-smoke and repo:publish-metadata: without it
    # CI runs only the real check, so the self-test that proves these assertions can FIRE never
    # executes and a rotted control ships green (SMA-526). Moon does not enable errexit for
    # `script:` blocks — a failing command followed by a succeeding one still exits 0 — so
    # `set -euo pipefail` is required explicitly.
    script: |
      set -euo pipefail
      python3 ci/affected-graph/task_inputs.py --self-test
      python3 ci/affected-graph/task_inputs.py
    toolchain: 'system'
    inputs:
      - '**/*'
```

- [ ] **Step 4: Verify the gate now passes**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/task_inputs.py; echo "rc=$?"
```
Expected: **rc=0**, `PASS  task-inputs        -> 19 repo tasks: every declared input still matches a tracked file (714 tracked)`.

Run: `time moon run repo:input-liveness --force`
Expected: PASS. **Record the wall time** — it replaces the estimate in the spec's E9 and goes in the README (Task 7).

- [ ] **Step 5: Wire it into CI and the docs**

In `.github/workflows/ci.yml:215`, append `:input-liveness` to the `T=(…)` array. It **must stay a single-line bash array**. Place it after `:iam-docker-policy-single-site`, matching the existing order.

In `CLAUDE.md`, add `:input-liveness` to the marker-delimited command **at the same ordinal position** — `ci_targets.py`'s C3 is an *ordered*, token-for-token mirror, so an off-by-one reds the gate. Do **not** add a second copy of either marker anywhere in the file, not even inside backticks.

Then add this gotcha bullet immediately after the existing "A new `repo:*` gate reds `:affected-smoke`…" bullet:

```markdown
- A `repo:*` task's `inputs` are now asserted **live**: `repo:input-liveness`
  (`ci/affected-graph/task_inputs.py`) fails if a declared glob matches zero tracked files or a
  declared file is untracked, so moving a directory a gate keys on reds CI instead of silently
  switching that gate off. It also asserts its OWN `inputs: ['**/*']` is unchanged — narrowing it
  for cost would make it stop noticing exactly the renames it exists to catch. A genuinely dead
  input needs an `ALLOW_DEAD_INPUT` entry with a reason (SMA-553).
```

- [ ] **Step 6: Add the README bullet**

In `ci/affected-graph/README.md`, after the `ci-targets` bullet (line ~70), add:

```markdown
- **`task-inputs`** (`task_inputs.py`, SMA-553) asserts every `repo:*` task's declared `inputs`
  still match a tracked file — the layer below `ci-targets`, which proves only that a gate is
  *wired*. **I1** no glob matches zero tracked files; **I2** every file input is tracked, by exact
  set membership (a wildcard-free pathspec prefix-matches a directory, so asking git would pass for
  any directory path); **I3** every task declares at least one input of its own, after subtracting
  Moon's injected `.moon/*.{…}` glob, which is present on all 119 tasks and makes a "resolved" input
  set never empty; **I4** every pattern is one the gate will evaluate — braces, character classes
  and pathspec magic are rejected loudly rather than skipped; **I5** the anti-vacuity floors,
  including a **composition** guard requiring the inputs common to every `repo` task to be exactly
  that one injected glob, and a `**/*` assertion on this gate's own task.
  Scheduled by its own `repo:input-liveness` task rather than from `run.sh`: the verdict depends on
  the whole tracked tree, and `repo:affected-smoke`'s narrow inputs would serve a cached PASS on
  exactly the rename that kills a gate. Two live-fire canaries run on every invocation, so a
  matcher stuck reporting "live" cannot pass vacuously. `ALLOW_DEAD_INPUT` ships empty and requires
  a reason. Scope is `repo` only — the other 27 projects carry 98 legitimately-dead convention
  globs inherited from `.moon/tasks/{rust,typescript,python}.yml`.
```

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/task_inputs.py ci/affected-graph/README.md moon.yml .github/workflows/ci.yml CLAUDE.md
git commit -m "feat(repo): schedule the input-liveness gate and wire it into ci (SMA-553)"
```

---

### Task 6: Extend SMA-541's C4 to guard the new task's script

Implements **D10**. Deliverable: deleting either invocation from the new task's script reds `repo:affected-smoke`.

**Files:**
- Modify: `ci/affected-graph/ci_targets.py` (`RUN_SH_CALL_SITES` at :167, `check_self_invocation` at :474, `main` at :832, `self_test` at ~:805)

**Interfaces:**
- Consumes: `moon_tasks`'s raw payload.
- Produces: `_scripts(projects) -> dict[str, str]` and a two-argument `check_self_invocation(run_sh_text, scripts)`.

**Note for the implementer:** `_eligibility` (`:270-330`) returns `{pid: {task: bool}}` and **discards `script`** — line `:320` is `row[name] = (options or {}).get("runInCI") is not False`. Do **not** reshape it: eight self-test fixtures depend on that shape, including an exact-equality `want_polarity` check at `:597-608`. Add a second extractor instead.

- [ ] **Step 1: Write the failing self-test**

Replace the existing `check_self_invocation` fixtures (`ci_targets.py:805-821`) with:

```python
    wired = (
        '  assert_ci_targets || SUITE_RC=1\n'
        '  python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1\n'
    )
    wired_script = (
        "set -euo pipefail\n"
        "python3 ci/affected-graph/task_inputs.py --self-test\n"
        "python3 ci/affected-graph/task_inputs.py\n"
    )
    scripts = {"input-liveness": wired_script}
    if check_self_invocation(wired, scripts):
        failures.append(
            f"check_self_invocation: fired on a wired tree: {check_self_invocation(wired, scripts)}"
        )
    no_call = wired.replace("  assert_ci_targets || SUITE_RC=1\n", "")
    if not check_self_invocation(no_call, scripts):
        failures.append("check_self_invocation: missed a deleted run_suite call")
    no_selftest = wired.replace('  python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1\n', "")
    if not check_self_invocation(no_selftest, scripts):
        failures.append("check_self_invocation: missed a deleted --self-test call")
    silenced = wired.replace("--self-test || NEG_RC=1", "--self-test || true")
    if not check_self_invocation(silenced, scripts):
        failures.append("check_self_invocation: missed a --self-test whose failure is swallowed")
    # SMA-553 D10 — the task-script half. The REAL-RUN line is a strict PREFIX of the --self-test
    # line, so a substring test would report the script below as fully wired while the gate no
    # longer runs at all. Whole-line matching is what distinguishes them.
    if not check_self_invocation(wired, {"input-liveness": wired_script.replace(
        "python3 ci/affected-graph/task_inputs.py\n", ""
    )}):
        failures.append("check_self_invocation: missed a deleted task_inputs real run (prefix hole)")
    if not check_self_invocation(wired, {"input-liveness": wired_script.replace(
        "python3 ci/affected-graph/task_inputs.py --self-test\n", ""
    )}):
        failures.append("check_self_invocation: missed a deleted task_inputs --self-test")
    if not check_self_invocation(wired, {}):
        failures.append("check_self_invocation: missed an absent input-liveness script entirely")
    # The two texts are checked SEPARATELY: a call site in the wrong file must not satisfy the
    # other's requirement, which a concatenated haystack would allow.
    if not check_self_invocation(wired_script, {"input-liveness": wired}):
        failures.append("check_self_invocation: accepted the two texts swapped")

    # _scripts (SMA-553 D10) — a second pure extractor, so _eligibility's shape is untouched.
    got_scripts = _scripts({"repo": {"input-liveness": {"script": "hi"}}, "ts": {"lint": {}}})
    if got_scripts != {"input-liveness": "hi"}:
        failures.append(f"_scripts: returned {got_scripts!r}")
    if _scripts({"repo": {"a": {"command": "true"}}}) != {"a": ""}:
        failures.append("_scripts: a task with no script must map to an empty string, not raise")
```

- [ ] **Step 2: Run it to verify it fails**

Run: `python3 ci/affected-graph/ci_targets.py --self-test`
Expected: FAIL — `check_self_invocation()` takes 1 positional argument but 2 were given.

- [ ] **Step 3: Implement the extension**

Replace `RUN_SH_CALL_SITES` (`:167-176`) with:

```python
# C4 — this gate's own call sites, and (SMA-553) repo:input-liveness's. Placing a gate inside
# repo:affected-smoke rather than making it a repo:* task of its own means C1 does NOT cover its
# execution: it depends on these lines, and deleting one leaves everything green.
#
# Matched as WHOLE LINES, not substrings. Two reasons, both learned the hard way:
#   - the `|| NEG_RC=1` suffix is as load-bearing as the command. Matching the prefix alone left
#     `--self-test || true` looking identical to a wired call site: the self-test still RUNS, its
#     failure is simply swallowed, and the negative control silently stops being able to report red.
#   - `python3 ci/affected-graph/task_inputs.py` is a strict PREFIX of the same line plus
#     ` --self-test`. Under a substring test, deleting the REAL RUN would leave C4 green while the
#     gate no longer ran at all (SMA-553 D10).
#
# The two texts are checked SEPARATELY rather than concatenated, so a call site in the wrong file
# cannot satisfy the other's requirement.
RUN_SH_CALL_SITES = (
    "assert_ci_targets || SUITE_RC=1",
    '"$HERE/ci_targets.py" --self-test || NEG_RC=1',
)

# repo:input-liveness's resolved script must run BOTH its negative control and the real check.
# Its `inputs: ['**/*']` is asserted separately, by SELF_TASK_EXPECTED_GLOBS below.
SELF_SCHEDULED_GATES = {
    "input-liveness": (
        "python3 ci/affected-graph/task_inputs.py --self-test",
        "python3 ci/affected-graph/task_inputs.py",
    ),
}
```

Note the `run.sh` entries are matched with `in`-per-line rather than exact equality, because the two `run.sh` lines carry leading indentation; the task-script lines do not. Replace `check_self_invocation` (`:474-476`) with:

```python
def _scripts(projects):
    """`{task: resolved script}` for the `repo` project. PURE.

    A SEPARATE extractor rather than a wider _eligibility return: that function's
    `{pid: {task: bool}}` shape is pinned by eight self-test fixtures, including an exact-equality
    polarity check, and reshaping it to carry `script` would break all of them for no gain.
    """
    repo = projects.get("repo") or {}
    if not isinstance(repo, dict):
        return {}
    return {name: (task.get("script") or "") for name, task in repo.items()
            if isinstance(task, dict)}


def check_self_invocation(run_sh_text, scripts):
    """Call sites of the affected-graph gates that are missing from where they must appear.

    Whole-line matching (see RUN_SH_CALL_SITES): a required line must be present as a complete,
    stripped line, so neither a swallowed failure suffix nor a prefix-contained sibling passes.
    """
    def lines(text):
        return {line.strip() for line in text.splitlines()}

    missing = [site for site in RUN_SH_CALL_SITES if site not in lines(run_sh_text)]
    for task, required in sorted(SELF_SCHEDULED_GATES.items()):
        present = lines(scripts.get(task, ""))
        missing.extend(
            f"{task} script: {site}" for site in required if site not in present
        )
    return missing
```

Because the `run.sh` sites are indented and the second one is a fragment of a longer line, keep them as substring tests while the task-script sites are whole-line. Implement `lines()` matching only for the script half:

```python
    missing = [site for site in RUN_SH_CALL_SITES if site not in run_sh_text]
```

(that is, leave the `run.sh` half exactly as it was — its two entries already carry their propagation suffix, which is what makes them unambiguous — and apply whole-line matching only to `SELF_SCHEDULED_GATES`, where the prefix hole exists.)

Then in `main` (`:857`), change the call:

```python
    missing_sites = check_self_invocation(run_sh, _scripts(raw_tasks))
```

`main` currently discards the raw payload inside `moon_tasks()`. Add a module-level helper so both extractors see it:

```python
def moon_payload():
    """The raw `tasks` object from one `moon query tasks` call."""
    out = subprocess.run(
        ["moon", "query", "tasks"], capture_output=True, text=True, check=True
    ).stdout
    payload = json.loads(out)
    if not isinstance(payload, dict):
        raise MoonOutputError(
            f"`moon query tasks` returned {type(payload).__name__}, expected a JSON object"
        )
    return payload.get("tasks") or {}
```

and rewrite `moon_tasks()` as `return _eligibility(moon_payload())`. In `main`, replace `tasks = moon_tasks()` with:

```python
        raw_tasks = moon_payload()
        tasks = _eligibility(raw_tasks)
```

- [ ] **Step 4: Add the D13 mirror**

Add near `REQUIRED_REPO_TASKS`:

```python
# SMA-553 D13 — repo:input-liveness's `inputs: ['**/*']` is load-bearing, and asserting it ONLY
# inside that gate would make it the sole judge of its own configuration. This is the second,
# independently-scheduled copy: it runs inside repo:affected-smoke.
SELF_TASK_EXPECTED_GLOBS = {"input-liveness": ("**/*",)}
```

Add the check function next to `check_self_invocation`:

```python
def check_gate_inputs(projects):
    """SMA-553 D13, mirrored. Rows for a self-scheduled gate whose own inputs have drifted."""
    repo = projects.get("repo") or {}
    rows = []
    for task, expected in sorted(SELF_TASK_EXPECTED_GLOBS.items()):
        entry = repo.get(task)
        if not isinstance(entry, dict):
            rows.append(f"repo:{task} is absent from the graph, so its inputs cannot be checked")
            continue
        got = tuple(g for g in sorted(entry.get("inputGlobs") or {})
                    if g != ".moon/*.{yml,yaml,jsonc,json,pkl,hcl,toml}")
        if got != expected or (entry.get("inputFiles") or {}):
            rows.append(
                f"repo:{task}'s authored inputs are {list(got)}, expected {list(expected)} — "
                "narrowing them makes that gate stop noticing the renames it exists to catch "
                "(SMA-553 D13)"
            )
    return rows
```

Wire it into `main` alongside the others (add `bad_gate_inputs = check_gate_inputs(raw_tasks)` and include it in the pass condition and the failure report, with the fix text "restore `inputs: ['**/*']` on the task in moon.yml"), and add two `self_test` rows: a wired `{"repo": {"input-liveness": {"inputGlobs": {"**/*": {}, INJECTED: {}}}}}` must not fire, and one narrowed to `{"ops/**/*": {}}` must.

- [ ] **Step 5: Run both self-tests and the full guard**

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/ci_targets.py --self-test
python3 ci/affected-graph/ci_targets.py; echo "ci_targets rc=$?"
ci/affected-graph/run.sh --negative-control; echo "neg rc=$?"
ci/affected-graph/run.sh; echo "suite rc=$?"
```
Expected: all PASS, rc=0 each. `ci_targets` should report **24 targets**.

- [ ] **Step 6: Commit**

```bash
git add ci/affected-graph/ci_targets.py
git commit -m "feat(repo): guard the input-liveness gate's script and inputs from ci-targets (SMA-553)"
```

---

### Task 7: Mutation verification and the measured cost

Implements the spec's §5 mutation battery. Deliverable: recorded evidence that every assertion fires, plus the real wall-clock figure replacing D2's estimate.

**Files:**
- Modify: `docs/superpowers/specs/2026-08-19-sma-553-input-liveness-gate-design.md` (E9's table, D2's estimate)
- Modify: `ci/affected-graph/README.md` (the cost figure)

**Interfaces:**
- Consumes: the finished gate.
- Produces: no code.

**Restore discipline:** after each mutation, restore with `git checkout -- <file>`. Never leave a mutation committed. Verify `git status --short` is clean before the final commit.

- [ ] **Step 1: Run the nine mutations, recording each verdict**

For each, apply the mutation, run the command, confirm the expected output, then restore.

| # | Mutation | Command | Expected |
|---|---|---|---|
| 1 | `moon.yml`: `repo:promtool` glob → `ops-moved/observability/prometheus/**/*` | `python3 ci/affected-graph/task_inputs.py` | rc 1, a `dead` row naming `repo:promtool` and the glob |
| 2 | `git mv ops/observability ops/obs` | same | rc 1, `dead` rows for **both** `promtool` and `observability-drift` |
| 3 | `moon.yml`: set `repo:machete` to `inputs: []` | same | rc 1, a `no-inputs` row naming `repo:machete` |
| 4 | `moon.yml`: `repo:input-liveness` inputs → `['ops/**/*']` | `python3 ci/affected-graph/task_inputs.py` **and** `python3 ci/affected-graph/ci_targets.py` | rc 1 from **both** — the two independent copies of D13 |
| 5 | `task_inputs.py`: `ALLOW_DEAD_INPUT = {("promtool", "ops/observability/prometheus/**/*"): ""}` | `python3 ci/affected-graph/task_inputs.py` | rc 1, an `allowlist` row about the blank reason |
| 6 | `moon.yml`: delete the real-run line from `input-liveness`'s script, leaving `--self-test` | `python3 ci/affected-graph/ci_targets.py` | rc 1 — the prefix-containment case |
| 7 | `ci.yml`: remove `:input-liveness` from `T`; separately, remove it from CLAUDE.md's marker region | `python3 ci/affected-graph/ci_targets.py` | rc 1 from C1 for the first, C3 for the second |
| 8 | `task_inputs.py`: delete the `check_canaries(matcher)` call from `main()`, and make `git_matcher` return `lambda p: 1` | `python3 ci/affected-graph/task_inputs.py` | rc 0 **— this proves the canary is load-bearing.** Then restore only the `check_canaries` call and re-run: rc 2. Record both. |
| 9 | none — the unmutated tree | `moon run repo:input-liveness --force` | PASS |

- [ ] **Step 2: Measure the real cost**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; time moon run repo:input-liveness --force`

Run it three times, alternating with `time moon run repo:promtool --force` for the per-task-floor reference, and take the median — the same method `.moon/workspace.yml:45` records for SMA-525.

- [ ] **Step 3: Record the figure**

In the spec's E9 table, replace the `repo:actionlint` reference row with a measured `repo:input-liveness` row marked `measured`. In D2, replace "the standalone task is **estimated** at…" with the measured figure and delete the revisit trigger if it came in under the ~35s alternative; if it did **not**, stop and report rather than proceeding — D2's basis would no longer hold.

Add the figure to the README bullet from Task 5, Step 6.

- [ ] **Step 4: Verify the tree is clean and the full graph passes**

Run:
```bash
git status --short
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata \
  :input-liveness --base origin/main --include-relations
```
Expected: `git status --short` shows only the two doc files; `moon ci` exits 0.

If `moon ci` reports an unattributed failure, diagnose with:
`jq '.actions[]|select(.status=="failed")' .moon/cache/ciReport.json`

- [ ] **Step 5: Commit**

```bash
git add docs/superpowers/specs/2026-08-19-sma-553-input-liveness-gate-design.md ci/affected-graph/README.md
git commit -m "docs(repo): record the input-liveness gate's measured cost and mutation results (SMA-553)"
```

---

## Self-Review

**Spec coverage.** D1→T1-4 (the file), D2→T5 (standalone task, `**/*`), D3→T5 (control first, `set -euo pipefail`), D4→T2 (`authored` + intersection guard), D5→T3 (matcher, tracked, cwd, rc polarity), D6→T1 (`classify`), D7→T3 (canaries) + T7 mutation 8 (wiring), D8→T2 (`moon query tasks`, exact key), D9→T4 (no `runInCI` filter — `check` iterates every parsed task), D10→T6, D11→T4 (`ALLOW_DEAD_INPUT` + staleness), D12→T5 (`main`'s three exits), D13→T4 and T6 (both copies). I1-I5→T4. §4's five wiring sites→T5-6. §5's fixture table→T1-4; its nine mutations→T7. §6's limitations need no code. §7 is SMA-556, already filed.

**Placeholder scan.** No TBD/TODO. Every code step carries real code; every verification step carries an exact command and its expected output.

**Type consistency.** `classify` returns `str` everywhere. `_repo_tasks` returns `{name: (globs, files)}` and Tasks 3-5 destructure it that way. `check(tasks, tracked, matcher, allow)` — T4's fixtures pass `allow` positionally as the 4th argument and `main` relies on the `ALLOW_DEAD_INPUT` default; both are consistent. `check_canaries(matcher)` takes one argument in both its fixture and `main`. In `ci_targets.py`, `check_self_invocation` becomes two-argument at every call site (`main` and all fixtures), and `_scripts`/`moon_payload`/`check_gate_inputs` are new names not colliding with anything existing.

**One known wrinkle, flagged deliberately:** Task 6, Step 3 gives the `run.sh` half as a substring test and the script half as whole-line. That asymmetry is intentional — `run.sh`'s two required lines are indented and one is a mid-line fragment, while the prefix hole exists only among the task-script lines. The implementer must keep both behaviours; the fixtures pin them.
