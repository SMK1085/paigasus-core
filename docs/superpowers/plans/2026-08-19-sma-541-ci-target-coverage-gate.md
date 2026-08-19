# CI Target Coverage Gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Assert that every CI-eligible `repo:*` Moon task is wired into `.github/workflows/ci.yml`'s `T=(…)` array, that every `T` entry still resolves to a CI-eligible task, and that CLAUDE.md's documented full-graph command mirrors `T`.

**Architecture:** A new `ci/affected-graph/ci_targets.py` — a Python sibling of `cargo_moon_parity.py`, following its exact conventions (rc 0/1/2, `--self-test`, never parse `moon.yml`, read Moon's own resolved output). `ci/affected-graph/run.sh` calls it from `run_suite` and calls its `--self-test` from the `--negative-control` branch, so `repo:affected-smoke` executes both halves in CI with no new Moon task.

**Tech Stack:** Python 3 stdlib only (`re`, `json`, `subprocess`, `sys`, `pathlib`, `itertools`). Bash for the `run.sh` wiring. Moon 2.3.2. No new dependencies — `repo:affected-smoke` is `toolchain: 'system'`.

**Spec:** `docs/superpowers/specs/2026-08-19-sma-541-ci-target-coverage-gate-design.md`

## Global Constraints

- Every source file opens with `# SPDX-License-Identifier: Apache-2.0` (`//` for Rust/TS).
- Exit-code contract (spec D2): **0** pass, **1** assertion failure, **2** infrastructure error. rc 2 is reserved for genuine tool failure — `moon` failing, non-JSON output, a missing output key. Every *authorial* mistake (no `T=(…)` line, two of them, a missing CLAUDE.md marker, an empty `repo` task set) is **rc 1**. `run.sh` turns rc 2 into `exit 2` of the whole guard, which would destroy every other assertion's diagnostics.
- Never parse `moon.yml`. Read Moon's own resolved output via `moon query`.
- Only ever **one** `moon query tasks` subprocess call, filtered by project id **in Python** — moon's `--project` filter is regex-based and unanchored (spec D8).
- `options.runInCI` absent, or `options` absent, means **CI-eligible** (default toward inclusion). If *no* task in the whole output carries an `options` key, that is rc 2.
- Branch: `feature/sma-541-ci-target-coverage-gate` (already created, spec already committed).
- Conventional commits with a workspace scope. Subject must **start lowercase** and be **≤100 chars**. No `#NNN` issue refs in the body (commitlint `footer-leading-blank`) — write "SMA-541", never "#141".
- Bash tool PATH lacks the proto CLIs: prefix every command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Python is run as `python3`, matching `run.sh` and `cargo_moon_parity.py`.

## File Structure

| File | Responsibility |
|---|---|
| `ci/affected-graph/ci_targets.py` | **Create.** All parsing and all four checks, plus `--self-test`. One file, mirroring `cargo_moon_parity.py`'s single-module shape. |
| `ci/affected-graph/run.sh` | **Modify.** Add `assert_ci_targets()`; call it last in `run_suite`; add the `--self-test` line to the `--negative-control` branch. |
| `CLAUDE.md` | **Modify.** Wrap the full-graph command in `<!-- ci-targets:begin/end -->` markers; reword the illustrative gate list to lead with "e.g."; add a gotcha bullet. |
| `moon.yml` | **Modify.** Add `CLAUDE.md` and `.prototools` to `repo:affected-smoke`'s `inputs`. |
| `ci/affected-graph/README.md` | **Modify.** Document C1-C4, the `T_EXEMPT` contract and the marker contract. |

**Reference facts, measured — do not re-derive:**
- `moon query tasks` emits JSON natively. `--json` is **not** a valid flag (`error: unexpected argument '--json' found`).
- Its shape is `{"tasks": {"<project-id>": {"<task-name>": {..., "options": {"runInCI": bool, ...}}}}}`.
- `repo` has 18 tasks; `install-hooks` is the only `runInCI: false`; no task anywhere is `internal: true`.
- `T` currently holds 23 entries and is **correct today** — the gate must go green on an unmutated tree.
- `prettier --check .` runs with cwd `ts/`, so repo-root `CLAUDE.md` is **not** prettier-formatted.

---

### Task 1: `ci_targets.py` scaffold, the `T` parser, and the self-test harness

**Files:**
- Create: `ci/affected-graph/ci_targets.py`

**Interfaces:**
- Consumes: nothing.
- Produces: `class GateAssertionError(RuntimeError)`; `class MoonOutputError(RuntimeError)`; `INFRA_ERRORS` tuple; `parse_t(text: str) -> list[str]` returning **bare** task names (no leading colon), raising `GateAssertionError`; `self_test() -> int`; `main() -> int`; the `sys.exit(...)` dispatch.

- [ ] **Step 1: Write the failing test**

Create `ci/affected-graph/ci_targets.py` containing **only** the self-test fixtures for the parser, plus the harness. The implementation comes in Step 3.

```python
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

    if failures:
        print("ci-targets self-test FAILED:", file=sys.stderr)
        for f in failures:
            print(f"  {f}", file=sys.stderr)
        return 1
    print("ci-targets self-test OK")
    return 0


def main():
    raise NotImplementedError


if __name__ == "__main__":
    sys.exit(self_test() if "--self-test" in sys.argv[1:] else main())
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/ci_targets.py --self-test ; echo "rc=$?"
```

Expected: **rc=1**, with a `NameError: name 'parse_t' is not defined` traceback — the fixtures reference a function that does not exist yet.

- [ ] **Step 3: Write the minimal implementation**

Insert `parse_t` immediately after the two regex constants, before `self_test`:

```python
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
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
python3 ci/affected-graph/ci_targets.py --self-test ; echo "rc=$?"
```

Expected: `ci-targets self-test OK`, **rc=0**.

- [ ] **Step 5: Verify it parses the REAL ci.yml — 23 targets**

```bash
python3 -c "
import sys; sys.path.insert(0, 'ci/affected-graph')
from ci_targets import parse_t
t = parse_t(open('.github/workflows/ci.yml').read())
print(len(t), t)
"
```

Expected: `23` and a list beginning `['build', 'test', 'lint', 'fmt', 'deny', ...]` ending `'publish-metadata'`.

- [ ] **Step 6: Commit**

```bash
git add ci/affected-graph/ci_targets.py
git commit -m "feat(repo): parse ci.yml's moon ci target array (SMA-541)"
```

---

### Task 2: CLAUDE.md markers and the docs parser

**Files:**
- Modify: `CLAUDE.md:62-68` (the full-graph bullet)
- Modify: `ci/affected-graph/ci_targets.py`

**Interfaces:**
- Consumes: `GateAssertionError` (Task 1).
- Produces: `MARKER_BEGIN`, `MARKER_END` constants; `parse_doc_targets(text: str) -> tuple[list[str], str]` returning `(bare task names, whitespace-normalised region text)`.

**Why markers and not prose-matching:** the first spec draft selected the command by three coincident substrings. Converting the 5-line command to a fenced ```` ```bash ```` block — an ordinary doc cleanup — would zero-match it, and merging the two neighbouring gotchas that already contain `` `moon ci :build` `` (`CLAUDE.md:13`) and `` `moon ci --include-relations` `` (`:89`) could two-match it. Blast radius: the repo's only required status check.

- [ ] **Step 1: Restructure the CLAUDE.md bullet**

Replace `CLAUDE.md:62-68` — currently:

```markdown
- Per-project Moon tasks (`<proj>:build/test/lint/fmt`) do NOT run the repo-level gates
  (`:deny`, `:osv`, `:machete`, `:affected-smoke`, codegen-drift, CODEOWNERS). Before pushing
  new crates/deps/proto, run the full graph like CI does: `moon ci :build :test :lint :fmt
  :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift
  :next-env-drift :wasm-getrandom-free :redis-connect-single-site :iam-docker-policy-single-site
  :promtool :observability-drift :nats-permissions :release-parity :release-parity-py
  :release-parity-ts :publish-metadata --base origin/main --include-relations`.
```

with — note the illustrative list now leads with `e.g.` and sits **outside** the markers, so its `:deny`/`:osv`/`:machete`/`:affected-smoke` tokens are not extracted:

```markdown
- Per-project Moon tasks (`<proj>:build/test/lint/fmt`) do NOT run the repo-level gates
  (e.g. `:deny`, `:osv`, `:machete`, `:affected-smoke`, codegen-drift, CODEOWNERS). Before pushing
  new crates/deps/proto, run the full graph like CI does. The command between the markers below is
  gated against `ci.yml`'s `T=(…)` array by `repo:affected-smoke` — keep the two identical, and do
  not remove the markers (SMA-541):
  <!-- ci-targets:begin -->
  `moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free
  :redis-connect-single-site :iam-docker-policy-single-site :promtool :observability-drift
  :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata
  --base origin/main --include-relations`
  <!-- ci-targets:end -->
```

- [ ] **Step 2: Write the failing test**

Add to `self_test()`, immediately before the `if failures:` block:

```python
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
```

- [ ] **Step 3: Run the test to verify it fails**

```bash
python3 ci/affected-graph/ci_targets.py --self-test ; echo "rc=$?"
```

Expected: **rc=1**, `NameError: name 'MARKER_BEGIN' is not defined`.

- [ ] **Step 4: Write the minimal implementation**

Add the constants next to the regexes:

```python
# The docs command is delimited EXPLICITLY, not recognised by prose shape. Prose-shape matching was
# fragile in both directions against ordinary doc edits: converting the command to a fenced code
# block zero-matches it, and CLAUDE.md already carries two neighbouring `moon ci …` spans that a
# reword could turn into a second match (D7). Markers also make the contract visible to whoever
# edits the file, and keep the illustrative gate list in the same bullet safely outside.
MARKER_BEGIN = "<!-- ci-targets:begin -->"
MARKER_END = "<!-- ci-targets:end -->"
```

and `parse_doc_targets` after `parse_t`:

```python
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
```

- [ ] **Step 5: Run the test to verify it passes**

```bash
python3 ci/affected-graph/ci_targets.py --self-test ; echo "rc=$?"
```

Expected: `ci-targets self-test OK`, **rc=0**.

- [ ] **Step 6: Verify the parsers agree on the REAL files**

```bash
python3 -c "
import sys; sys.path.insert(0, 'ci/affected-graph')
from ci_targets import parse_t, parse_doc_targets
t = parse_t(open('.github/workflows/ci.yml').read())
d, region = parse_doc_targets(open('CLAUDE.md').read())
print('T  :', len(t))
print('doc:', len(d))
print('equal:', d == t)
print('flags:', '--base origin/main' in region, '--include-relations' in region)
"
```

Expected: `T: 23`, `doc: 23`, `equal: True`, `flags: True True`. **If `equal` is False, the CLAUDE.md edit in Step 1 dropped or reordered a target — fix it before continuing; do not adjust the parser to compensate.**

- [ ] **Step 7: Confirm the HTML comments render invisibly**

The markers sit inside a markdown list item. Confirm GitHub hides them rather than printing them literally:

```bash
gh api -X POST /markdown -f mode=gfm -f text="$(sed -n '62,76p' CLAUDE.md)" | grep -c "ci-targets:begin" || echo "OK: marker not rendered"
```

Expected: `OK: marker not rendered` (grep finds 0 and exits 1). If the marker **does** appear in the rendered HTML, move both markers to column 0 (unindented, flush left) and re-run — an HTML comment indented 4+ spaces inside a list can be parsed as an indented code block.

- [ ] **Step 8: Commit**

```bash
git add ci/affected-graph/ci_targets.py CLAUDE.md
git commit -m "feat(repo): delimit the documented full-graph command with markers (SMA-541)"
```

---

### Task 3: The Moon task model, the floor, and C1 (forward, strict equality)

**Files:**
- Modify: `ci/affected-graph/ci_targets.py`

**Interfaces:**
- Consumes: `MoonOutputError`, `GateAssertionError` (Task 1).
- Produces: `T_EXEMPT: dict[str, str]` (ships `{}`); `REQUIRED_REPO_TASKS: tuple[str, ...]`; `moon_tasks() -> dict[str, dict[str, bool]]` mapping project id → task name → **CI-eligible**; `check_floor(tasks, floor=...) -> list[str]`; `check_forward(tasks, t_targets, exempt=None) -> tuple[list[str], list[str], list[str]]` returning `(missing, unexpected, bad_exempt)`.

**Why strict equality and not a subset test:** with a subset test, adding `options: { runInCI: false }` to `repo:promtool` while leaving `:promtool` in `T` passes C1 (excluded), passes C2 (still resolves), and leaves the docs unchanged — three green checks while `moon ci` runs nothing. That re-opens the exact failure this gate exists to close, in one line.

- [ ] **Step 1: Write the failing test**

Add to `self_test()`, before the `if failures:` block:

```python
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
    # install-hooks is runInCI:false and absent from T -> must NOT trip the gate (issue AC #3).
    forward("runInCI-false-absent", tasks_fixture, aligned_t, {}, [], [])
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

    if check_floor(tasks_fixture) != []:
        failures.append("check_floor: fired on a fixture containing every floor member")
    thin = {"repo": {"deny": True}}
    if check_floor(thin) != ["affected-smoke", "promtool", "publish-metadata"]:
        failures.append(f"check_floor: did not name every absent floor member: {check_floor(thin)}")
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
python3 ci/affected-graph/ci_targets.py --self-test ; echo "rc=$?"
```

Expected: **rc=1**, `NameError: name 'check_forward' is not defined`.

- [ ] **Step 3: Write the minimal implementation**

Add the two tables after `MARKER_END`:

```python
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
```

and the three functions after `parse_doc_targets`:

```python
def moon_tasks():
    """Moon's own resolved task graph: project id -> task name -> CI-eligible.

    ONE subprocess call, filtered by project id in Python rather than with `--project repo`:
    moon's query filters are regex-based and unanchored, so a future project named e.g.
    `paigasus-repo-ts` would silently join the "repo task set" and false-red C1 (D8).

    Eligibility polarity is deliberately `is not False`: an absent `runInCI`, or an absent
    `options` object, means ELIGIBLE. Defaulting toward inclusion means a moon output change
    cannot silently exempt a gate — it can only over-require, which is a loud red.
    """
    out = subprocess.run(
        ["moon", "query", "tasks"], capture_output=True, text=True, check=True
    ).stdout
    projects = json.loads(out).get("tasks") or {}
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
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
python3 ci/affected-graph/ci_targets.py --self-test ; echo "rc=$?"
```

Expected: `ci-targets self-test OK`, **rc=0**.

- [ ] **Step 5: Verify against the REAL graph — C1 must be clean today**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 -c "
import sys; sys.path.insert(0, 'ci/affected-graph')
from ci_targets import moon_tasks, parse_t, check_forward, check_floor
tasks = moon_tasks()
t = parse_t(open('.github/workflows/ci.yml').read())
print('projects:', len(tasks), 'repo tasks:', len(tasks['repo']))
print('floor:', check_floor(tasks))
print('missing/unexpected/bad_exempt:', check_forward(tasks, t))
"
```

Expected: `repo tasks: 18`, `floor: []`, `missing/unexpected/bad_exempt: ([], [], [])`.

- [ ] **Step 6: Commit**

```bash
git add ci/affected-graph/ci_targets.py
git commit -m "feat(repo): assert every ci-eligible repo task is in the target array (SMA-541)"
```

---

### Task 4: C2 (reverse) and C3 (docs mirror)

**Files:**
- Modify: `ci/affected-graph/ci_targets.py`

**Interfaces:**
- Consumes: `moon_tasks()`'s shape and `parse_t`/`parse_doc_targets` output (Tasks 1-3).
- Produces: `REQUIRED_DOC_FLAGS: tuple[str, ...]`; `check_reverse(tasks, t_targets) -> list[str]`; `check_docs(t_targets, doc_targets, region) -> list[str]`.

- [ ] **Step 1: Write the failing test**

Add to `self_test()`, before the `if failures:` block:

```python
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
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
python3 ci/affected-graph/ci_targets.py --self-test ; echo "rc=$?"
```

Expected: **rc=1**, `NameError: name 'check_reverse' is not defined`.

- [ ] **Step 3: Write the minimal implementation**

Add the constant next to `REQUIRED_REPO_TASKS`:

```python
# C3 checks the flag tail too. The first spec draft omitted it on the stated grounds that
# assert_include_relations "already owns the flag question" — it does not: that function greps
# ci.yml only (run.sh:126) and never opens CLAUDE.md. Without this, the documented command could
# lose --include-relations and silently under-build, which is the very behaviour that makes
# checking the docs worth doing (D6).
REQUIRED_DOC_FLAGS = ("--base origin/main", "--include-relations")
```

and the two functions after `check_forward`:

```python
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
```

- [ ] **Step 4: Run the test to verify it passes**

```bash
python3 ci/affected-graph/ci_targets.py --self-test ; echo "rc=$?"
```

Expected: `ci-targets self-test OK`, **rc=0**.

- [ ] **Step 5: Verify against the REAL tree — both must be clean today**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 -c "
import sys; sys.path.insert(0, 'ci/affected-graph')
from ci_targets import moon_tasks, parse_t, parse_doc_targets, check_reverse, check_docs
tasks = moon_tasks()
t = parse_t(open('.github/workflows/ci.yml').read())
d, region = parse_doc_targets(open('CLAUDE.md').read())
print('dead entries:', check_reverse(tasks, t))
print('doc problems:', check_docs(t, d, region))
"
```

Expected: `dead entries: []` and `doc problems: []`.

- [ ] **Step 6: Commit**

```bash
git add ci/affected-graph/ci_targets.py
git commit -m "feat(repo): assert target-array entries resolve and the docs mirror them (SMA-541)"
```

---

### Task 5: C4 (self-invocation) and `main()`

**Files:**
- Modify: `ci/affected-graph/ci_targets.py`

**Interfaces:**
- Consumes: every check from Tasks 1-4.
- Produces: `RUN_SH_CALL_SITES: tuple[str, ...]`; `check_self_invocation(run_sh_text) -> list[str]`; a working `main() -> int` replacing the `NotImplementedError` stub.

**Note on the call-site strings:** they are matched as literal substrings including their bash suffixes (`|| SUITE_RC=1`, `|| NEG_RC=1`). Matching the bare name `assert_ci_targets` would also match the function *definition*, so deleting the call would still pass. Task 6 writes `run.sh` to contain exactly these strings — **if you change one, change both.**

- [ ] **Step 1: Write the failing test**

Add to `self_test()`, before the `if failures:` block:

```python
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
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
python3 ci/affected-graph/ci_targets.py --self-test ; echo "rc=$?"
```

Expected: **rc=1**, `NameError: name 'check_self_invocation' is not defined`.

- [ ] **Step 3: Write the minimal implementation**

Add the constant next to `REQUIRED_DOC_FLAGS`:

```python
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
```

then `check_self_invocation` after `check_docs`, and replace the `main()` stub entirely:

```python
def check_self_invocation(run_sh_text):
    """Call sites of this gate that are missing from run.sh."""
    return [site for site in RUN_SH_CALL_SITES if site not in run_sh_text]


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
```

- [ ] **Step 4: Run the self-test to verify it passes**

```bash
python3 ci/affected-graph/ci_targets.py --self-test ; echo "rc=$?"
```

Expected: `ci-targets self-test OK`, **rc=0**.

- [ ] **Step 5: Run the gate for real — C4 must FAIL, everything else PASS**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/ci_targets.py ; echo "rc=$?"
```

Expected: **rc=1**, reporting only the `missing_sites` section with both call sites listed — `run.sh` is not wired until Task 6. This is the correct intermediate state and is itself evidence C4 bites. If any *other* section appears, stop and fix it before continuing.

- [ ] **Step 6: Commit**

```bash
git add ci/affected-graph/ci_targets.py
git commit -m "feat(repo): wire the ci-targets checks into a main entry point (SMA-541)"
```

---

### Task 6: `run.sh` wiring

**Files:**
- Modify: `ci/affected-graph/run.sh` (add `assert_ci_targets` near `assert_cargo_moon_parity` at `:166-175`; call it at the end of `run_suite`; add the `--self-test` line in the `--negative-control` branch next to the existing `cargo_moon_parity.py --self-test`)

**Interfaces:**
- Consumes: `ci_targets.py`'s rc 0/1/2 contract and the exact strings in `RUN_SH_CALL_SITES`.
- Produces: `repo:affected-smoke` running both halves of the gate.

- [ ] **Step 1: Add the assertion helper**

Insert immediately after the existing `assert_cargo_moon_parity()` function (which ends with its `esac`/`}` around `run.sh:175`):

```bash
# SMA-541 — CI target-array coverage. rc 2 (infra) aborts, mirroring run_case.
assert_ci_targets() {
  local ec=0
  python3 "$HERE/ci_targets.py" || ec=$?
  case "$ec" in
    0) return 0 ;;
    1) return 1 ;;
    *) echo "== affected-graph guard ABORTED: ci-targets infrastructure error (rc=$ec) ==" >&2; exit 2 ;;
  esac
}
```

- [ ] **Step 2: Call it last in `run_suite`**

In `run_suite`, the final three lines are currently:

```bash
  assert_cargo_moon_parity || SUITE_RC=1
  # assert_include_relations returns only 0/1 (no infra code), so collapsing is correct here.
  assert_include_relations || SUITE_RC=1
  return "$SUITE_RC"
```

Replace with:

```bash
  assert_cargo_moon_parity || SUITE_RC=1
  # assert_include_relations returns only 0/1 (no infra code), so collapsing is correct here.
  assert_include_relations || SUITE_RC=1
  # LAST deliberately: assert_ci_targets is the only assertion that can still exit 2 (a broken
  # `moon query`), and an rc-2 abort kills the script — so anything ordered after it would lose
  # its diagnostics on exactly the runs where they are most useful (SMA-541 D2).
  assert_ci_targets || SUITE_RC=1
  return "$SUITE_RC"
```

- [ ] **Step 3: Add the self-test to the negative-control branch**

Find, in the `--negative-control` block:

```bash
  python3 "$HERE/cargo_moon_parity.py" --self-test || NEG_RC=1
```

and add directly beneath it:

```bash
  # 4) the ci-target coverage gate must fire on synthetic violations of each of its four checks —
  #    including its two hand-rolled parsers, which are the part it cannot self-detect a fault in.
  python3 "$HERE/ci_targets.py" --self-test || NEG_RC=1
```

- [ ] **Step 4: Run the real gate — everything must now pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/ci_targets.py ; echo "rc=$?"
```

Expected: **rc=0** and a single `PASS  ci-targets  -> 23 targets: ...` line. C4 is satisfied now that both call sites exist.

- [ ] **Step 5: Run the whole guard, both halves**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ci/affected-graph/run.sh --negative-control ; echo "control rc=$?"
ci/affected-graph/run.sh ; echo "suite rc=$?"
```

Expected: control prints `ci-targets self-test OK` plus `negative-control OK`, **rc=0**; the suite prints every existing `PASS` line plus `PASS  ci-targets`, then `== affected-graph cascade intact ==`, **rc=0**.

- [ ] **Step 6: Commit**

```bash
git add ci/affected-graph/run.sh
git commit -m "feat(repo): run the ci-targets gate from the affected-graph guard (SMA-541)"
```

---

### Task 7: Moon inputs and documentation

**Files:**
- Modify: `moon.yml:130-145` (`repo:affected-smoke` `inputs`)
- Modify: `CLAUDE.md` (gotcha bullet, after the full-graph bullet edited in Task 2)
- Modify: `ci/affected-graph/README.md`

**Interfaces:**
- Consumes: the gate from Tasks 1-6.
- Produces: no code interface — this task makes the gate *reachable* (cache-key completeness) and documented.

**Why this is not optional:** without `CLAUDE.md` in the inputs, a docs-only edit leaves `repo:affected-smoke` serving a **cached PASS** and C3 is real but unreachable. Without `.prototools`, a moon bump touching only that file replays a cached PASS from a *different moon version*, while this guard's expected sets are explicitly a snapshot at the pinned version.

- [ ] **Step 1: Add the two inputs**

In `moon.yml`, `repo:affected-smoke`'s `inputs` list currently ends:

```yaml
      - 'ts/packages/*/package.json'
      - 'ts/apps/*/package.json'
```

Append:

```yaml
      # SMA-541 — this task now asserts that CLAUDE.md's documented full-graph command mirrors
      # ci.yml's `T=(...)` array, so a docs-only edit MUST re-key it; without this the assertion is
      # real but unreachable behind a cached PASS. (repo:actionlint's `**/*` already covers
      # CLAUDE.md, but that is a different task and cannot green this one.)
      - 'CLAUDE.md'
      # The guard shells out to the proto-pinned `moon` for its query output, and this file's own
      # README states the expected sets are "a snapshot ... at the pinned moon version" — which is
      # only true if a version bump re-runs it. Every other repo gate that shells out to a
      # proto-pinned binary lists this (repo:osv, repo:promtool, the three release-parity*).
      - '.prototools'
```

- [ ] **Step 2: Prove the new input re-keys the task**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:affected-smoke >/dev/null 2>&1 && echo "warm run OK"
printf '\n' >> CLAUDE.md
moon query tasks --affected --downstream deep <<< "CLAUDE.md" \
  | python3 -c "import sys,json; print(sorted((json.load(sys.stdin).get('tasks') or {}).get('repo', {})))"
git checkout CLAUDE.md
```

Expected: the printed list **includes `affected-smoke`** (alongside `actionlint`). If `affected-smoke` is absent, the input was not added correctly — fix before continuing.

- [ ] **Step 3: Add the CLAUDE.md gotcha bullet**

Directly after the full-graph bullet (the one now carrying the markers), insert:

```markdown
- A new `repo:*` gate reds `:affected-smoke` until it is in **both** `ci.yml`'s `T=(…)` array and
  the marker-delimited command above — `ci/affected-graph/ci_targets.py` asserts the two agree, and
  that every `T` entry still resolves to a CI-eligible task. That last half matters because
  `moon ci` exits **0** on a target that resolves to nothing (even with real targets around it), so
  a typo is otherwise a silent no-op on every PR. A gate that must stay out of `T` needs a
  `T_EXEMPT` entry with a reason — `runInCI: false` is NOT a general escape, since Moon then drops
  the task from `moon run` under `CI=true` too (see the comments in `ts/moon.yml`). `T` must also
  stay a single-line bash array (SMA-541).
```

- [ ] **Step 4: Add the README bullet**

In `ci/affected-graph/README.md`, in the list of "checks that the per-case project sets structurally cannot make" (after the `A5` bullet), add:

```markdown
- **`ci-targets`** (`ci_targets.py`, SMA-541) asserts `ci.yml`'s hand-written `moon ci` target array
  is complete and live: **C1** every CI-eligible `repo:*` task appears in `T=(…)` and — strict
  equality, not a subset — nothing in `T` names a `repo` task that is switched off; **C2** every `T`
  entry resolves to a CI-eligible task somewhere in the graph; **C3** CLAUDE.md's marker-delimited
  command mirrors `T` token-for-token in order and keeps its `--base origin/main
  --include-relations` tail; **C4** both of this gate's own call sites are still present in
  `run.sh`. `moon ci` exits **0** on a target that resolves to nothing — measured, including the
  mixed case — so without C2 a renamed or mistyped entry is a silent no-op on every PR.

  Maintenance: adding a `repo:*` task means adding `:<name>` to `T` **and** to the command between
  `<!-- ci-targets:begin -->` / `<!-- ci-targets:end -->` in CLAUDE.md. A task that must stay out of
  `T` goes in `T_EXEMPT` with a required non-empty reason — `runInCI: false` is not a general
  escape, because Moon then also drops the task from `moon run` under `CI=true` (`ts/moon.yml`).
  `REQUIRED_REPO_TASKS` is the floor that stops the comparison degrading to two empty sets.
  Not covered: whether a `repo:*` task's `inputs` still match anything — see the follow-up in the
  design doc's L3.
```

- [ ] **Step 5: Re-run the gate and the guard**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/ci_targets.py ; echo "gate rc=$?"
ci/affected-graph/run.sh --negative-control && ci/affected-graph/run.sh ; echo "guard rc=$?"
```

Expected: both **rc=0**. (The CLAUDE.md edits in Steps 3-4 are outside the markers, so C3 is unaffected — if it now fails, the new bullet was accidentally placed *inside* the markers.)

- [ ] **Step 6: Commit**

```bash
git add moon.yml CLAUDE.md ci/affected-graph/README.md
git commit -m "docs(repo): document the ci-targets gate and key it on its inputs (SMA-541)"
```

---

### Task 8: Mutation verification and the full CI graph

**Files:** none modified permanently — every mutation here is reverted.

**Interfaces:**
- Consumes: the finished gate.
- Produces: recorded evidence that each check fires against the **real** tree, not only against fixtures.

**Why:** the fixture table proves the check *functions* work on synthetic data. This proves the wiring — real `moon query` output, real files, real parsers — reports red. `git stash`/`git checkout --` after each mutation; verify `git status` is clean at the end.

- [ ] **Step 1: C1 missing — a new gate nobody added to `T`**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cp moon.yml /tmp/moon.yml.orig
cat >> moon.yml <<'YAML'

  sma541-throwaway:
    description: 'Temporary task proving the ci-targets gate names a repo task missing from T.'
    command: 'true'
    toolchain: 'system'
    inputs: []
YAML
python3 ci/affected-graph/ci_targets.py ; echo "rc=$?"
cp /tmp/moon.yml.orig moon.yml
```

Expected: **rc=1**, a `missing` section naming `sma541-throwaway`.

- [ ] **Step 2: C1 unexpected — the blocker case, a gate switched off but left in `T`**

```bash
python3 - <<'PY'
import pathlib, re
p = pathlib.Path("moon.yml"); t = p.read_text()
p.write_text(t.replace(
    "  promtool:\n    description:",
    "  promtool:\n    options:\n      runInCI: false\n    description:", 1))
PY
python3 ci/affected-graph/ci_targets.py ; echo "rc=$?"
git checkout -- moon.yml
```

Expected: **rc=1**, an `unexpected` section naming `promtool`. **This is the case a subset test would have passed** — if it reports rc=0, `check_forward` regressed to a subset test.

- [ ] **Step 3: C2 dead entry — a typo'd target**

```bash
sed -i.bak 's/:affected-smoke :parity-corpus-drift/:afected-smoke :parity-corpus-drift/' \
  .github/workflows/ci.yml
python3 ci/affected-graph/ci_targets.py ; echo "rc=$?"
mv .github/workflows/ci.yml.bak .github/workflows/ci.yml
```

Expected: **rc=1**, reporting **both** a `missing` row for `affected-smoke` (C1) and a `dead` row for `afected-smoke` (C2), plus a C3 divergence. Confirms the two checks are complementary rather than redundant.

- [ ] **Step 4: C3 docs drift — a target dropped from CLAUDE.md**

```bash
sed -i.bak 's/:promtool :observability-drift/:observability-drift/' CLAUDE.md
python3 ci/affected-graph/ci_targets.py ; echo "rc=$?"
mv CLAUDE.md.bak CLAUDE.md
```

Expected: **rc=1**, a `doc_problems` section naming the first divergence position and printing both lists. C1/C2 stay silent — the docs are the only thing wrong.

- [ ] **Step 5: Parser guards — a marker removed, and a second `T` assignment**

```bash
sed -i.bak 's|<!-- ci-targets:end -->||' CLAUDE.md
python3 ci/affected-graph/ci_targets.py ; echo "marker rc=$?"
mv CLAUDE.md.bak CLAUDE.md

sed -i.bak 's|^\( *\)T=(:build|\1T+=(:extra)\n\1T=(:build|' .github/workflows/ci.yml
python3 ci/affected-graph/ci_targets.py ; echo "append rc=$?"
mv .github/workflows/ci.yml.bak .github/workflows/ci.yml
```

Expected: both **rc=1** (assertion failures, **not** rc=2 — an authorial mistake must not abort the whole guard), each naming the marker / the assignment count.

- [ ] **Step 6: Confirm a clean tree is green, and measure the cost**

```bash
git status --short          # expect: no modifications
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
time python3 ci/affected-graph/ci_targets.py
```

Expected: `git status` clean; **rc=0**; record the wall-clock time. Add the measured number to the README bullet from Task 7 Step 4 (the design rejects a standalone Moon task partly on cost grounds, so the added cost belongs on record).

- [ ] **Step 7: Run the full graph like CI does**

Per-project Moon tasks do not run the repo-level gates, so run the whole array:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata \
  --base origin/main --include-relations
echo "rc=$?"
```

Expected: **rc=0**. If a task fails for an unattributed reason, diagnose with
`jq '.actions[]|select(.status=="failed")' .moon/cache/ciReport.json`. Docker-backed IAM suites skip when the daemon is unreachable; that is expected locally and not a regression from this change.

- [ ] **Step 8: Commit any README cost figure**

```bash
git add ci/affected-graph/README.md
git commit -m "docs(repo): record the measured ci-targets gate cost (SMA-541)"
```

(Skip this commit if Step 6 produced no README change.)

---

## Self-Review

**Spec coverage.** Every design decision maps to a task: D1 → Tasks 1/6; D2 → Task 5 (`main()` exception split) + Task 6 (`assert_ci_targets` rc handling); D3 → Task 3; D4 → Task 4; D5 → Task 3 (`T_EXEMPT`); D6 → Task 4 (`REQUIRED_DOC_FLAGS`); D7 → Task 2; D8 → Task 3 (`moon_tasks`); D9 → Task 7; D10 → Task 1 (`parse_t` token classification); D11 → Tasks 1/2 (parsers as pure functions with fixtures); D12 → Task 1 (both regexes); D13 → Task 5 (C4). Checks C1-C4 → Tasks 3, 4, 4, 5. §5's mutation battery → Task 8. §5's cost measurement → Task 8 Step 6. §6 limitations need no code.

**Placeholder scan.** No TBD/TODO. Every code step carries the actual code. Every verification step carries the exact command and its expected output.

**Type consistency.** `parse_t` and `parse_doc_targets` both return **bare** names (no leading colon) throughout, and every consumer (`check_forward`, `check_reverse`, `check_docs`) treats them that way; only rendered messages re-add the `:`. `moon_tasks()`'s `dict[str, dict[str, bool]]` shape is what `tasks_fixture` in Task 3 mirrors and what `check_floor`/`check_forward`/`check_reverse` all consume. `check_forward` returns a 3-tuple in Tasks 3 and 5 alike. `RUN_SH_CALL_SITES` (Task 5) matches the exact strings Task 6 writes into `run.sh` — flagged in both tasks.

**Known intentional intermediate state.** After Task 5, `python3 ci_targets.py` exits **1** on the real tree because `run.sh` is not yet wired (C4). Task 5 Step 5 states this explicitly and Task 6 Step 4 resolves it.
