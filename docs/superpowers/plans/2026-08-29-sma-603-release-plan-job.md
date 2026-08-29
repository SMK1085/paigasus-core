<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-603 — release `plan` job Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** A push to `main` with nothing to release must skip the 12-leg build matrix and the
human approval gate, and must never skip one that has something to release.

**Architecture:** A new `plan` job in `.github/workflows/release.yml` becomes the single holder
of the `PAIGASUS_RELEASE_ENABLED` gate; `wheels`, `prebuild` and `proto-dist` gate transitively
on it. The decision is **not** shell in the workflow — it is a fixture-tested Python checker at
`ci/release-plan/`, deciding purely on tag existence, which is what release-plz itself
short-circuits on. Two new verdicts in `ci/actionlint/release_guard.py` assert the approval
boundary (V8) and the plan job's contract (V9).

**Tech Stack:** GitHub Actions; Python 3.12 (`tomllib`, stdlib only) driven by `uv`; bash;
Moon (`repo:actionlint`, `repo:affected-smoke`).

**Spec:** `docs/superpowers/specs/2026-08-29-sma-603-release-plan-job-design.md` (revision 2).
Read it. The plan implements §3; §2's measurements are the evidence for every decision, and §7
records the approach that was measured and rejected.

## Global Constraints

- Every source file opens with an SPDX header: `# SPDX-License-Identifier: Apache-2.0`
  (`<!-- … -->` for Markdown).
- Prefix every shell command with
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` — the Bash tool's PATH lacks the
  proto-managed CLIs.
- Conventional commits with a workspace scope. **Never start a body line with `word:`** — it
  parses as a footer token and commitlint rejects the commit (`footer-leading-blank`). Write
  "the outputs block", not "outputs: block".
- Do **not** use `git stash`. The stash stack is shared across worktrees.
- The decision must **fail safe**: every inconclusive outcome yields `nothing_to_release=false`,
  which builds. There is no path where uncertainty skips.
- `continue-on-error` may not be added to any job or step on a gated path — `release_guard.py`
  V4 rejects any value but literal `false`.
- Any command in `release.yml` must stay on **one physical line**. `command_segments`
  (`release_guard.py:214`) is per physical line, so a backslash continuation is judged without
  its flags.
- Exit-code contract for a checker in `ci/`: **3** means an assertion failed, and the `run.sh`
  wrapper maps 3 → 1 and everything else → 2. `uv` exits 1 on its own failures, so a shared code
  would let a PyPI outage read as a real violation.

---

## File structure

**Create**

| File | Responsibility |
| --- | --- |
| `ci/release-plan/release_plan.py` | The decision as a pure function, its collection layer, and the fixture table |
| `ci/release-plan/run.sh` | Mode dispatch, exit-code mapping, `$GITHUB_OUTPUT` writing, the negative control |
| `ci/release-plan/pyproject.toml` | A dedicated zero-dependency uv project (`tomllib` is stdlib) |
| `ci/release-plan/uv.lock` | Generated |
| `ci/release-plan/README.md` | What it asserts, and its Non-goals / Limitations |

**Modify**

| File | Change |
| --- | --- |
| `ci/actionlint/run.sh` | Check 11 + `release_plan_self_test`; `SELF_TEST_COUNT` 12 → 13 |
| `ci/affected-graph/ci_targets.py` | `ACTIONLINT_SH_CALL_SITES` gains check 11's call sites |
| `ci/actionlint/release_guard.py` | `_OK_MAIN` restructure; V8a–d; V9a–d; fixture rows |
| `.github/workflows/release.yml` | The `plan` job; gating on three jobs; three comment blocks |
| `docs/ops/RUNBOOK-release-activation.md` | §6 and the step-J trigger-removal row |
| `CLAUDE.md` | Two corrections and one new entry |

---

## Task 1: `ci/release-plan/` — the decision, as a tested script

**Files:**
- Create: `ci/release-plan/release_plan.py`
- Create: `ci/release-plan/run.sh`
- Create: `ci/release-plan/pyproject.toml`
- Create: `ci/release-plan/uv.lock` (generated)
- Create: `ci/release-plan/README.md`

**Interfaces:**
- Consumes: nothing.
- Produces, and later tasks depend on these exact names:
  - `ci/release-plan/run.sh --github-output` — the runtime entry point, invoked by the workflow.
    Always exits 0. Appends `nothing_to_release=true|false` to `$GITHUB_OUTPUT`.
  - `ci/release-plan/run.sh --self-test | --negative-control | --assert` — the CI entry points.
    Exit 0 pass, 1 the repo is wrong, 2 infrastructure failed.
  - `release_plan.py --fixture-count` — prints an integer.
  - `decide(event_name: str, packages: dict[str, str], tags: set[str]) -> tuple[bool, str]`

- [ ] **Step 1: Write `pyproject.toml`**

```toml
# SPDX-License-Identifier: Apache-2.0
# A DEDICATED zero-dependency project, deliberately not the py/ workspace. py/ is a
# [tool.uv.workspace] root whose member paigasus-kernel depends on paigasus-py-bindings by
# path, and that crate builds with maturin — so `uv run --project py` compiles a PyO3 cdylib.
# This checker needs only `tomllib`, which is stdlib from 3.11, so it needs no dependency at
# all. The project exists to pin the interpreter floor, nothing more. (SMA-593 spec §3
# Decision A is the precedent; ci/workflow-credentials/pyproject.toml is the sibling.)
[project]
name = "paigasus-release-plan"
version = "0.0.0"
description = "Decides whether a push to main has anything to release"
requires-python = ">=3.12"
dependencies = []
```

- [ ] **Step 2: Write the failing fixture table and the decision, in `release_plan.py`**

Write the file below in full. The fixture table is the test; there is no separate test file,
matching `ci/workflow-credentials/workflow_credentials.py` and `ci/actionlint/release_guard.py`.

```python
#!/usr/bin/env python3
# SPDX-License-Identifier: Apache-2.0
"""Decide whether a push to `main` has anything to release (SMA-603).

WHY THIS IS NOT A DRY RUN. The obvious design reads
`release-plz release --dry-run --output json` and skips on an empty `releases` array. It is
WRONG, and measurement M6 in the spec is why: with only the `kernel` version group bumped,
release-plz logs that it WOULD publish paigasus-kernel and cut `paigasus-kernel-v0.1.1`, and
still prints `{"releases":[]}` at exit 0. That array records PERFORMED releases, and a dry run
performs none, so it cannot tell "nothing to release" from "a release is pending". Reading it
would have silently, greenly and permanently skipped every kernel-group release.

WHAT THIS READS INSTEAD. Measurements M2 and M6 both show release-plz short-circuiting on TAG
EXISTENCE, before any registry or cargo work: `Already published - Tag <pkg>-v<version> already
exists`. That predicate is a pure function of local state, so it needs no token, no network and
no cargo — and it can be fixture-tested, which the dry-run reading could not be.

FAIL-SAFE DIRECTION. Every inconclusive outcome returns False, which BUILDS. A false build costs
runner time; a false skip silently drops a release. Nothing here may invert that.
"""
from __future__ import annotations

import argparse
import subprocess
import sys
import tomllib
from pathlib import Path

# --- The pinned vocabulary ---------------------------------------------------------------------

# What release-plz TAGS. CLAUDE.md records the measurement from the first live release: it only
# tags what it PUBLISHES — three tags, not six, and the three `publish = false` kernel-family
# binding crates were never mentioned in the release job log at all.
#
# STRICT EQUALITY, asserted by --assert against the DERIVED set. This is the EXPECTED_PR_SUBJECTS
# idiom: a newly publishable crate reds this gate until someone re-baselines deliberately. The
# RUNTIME path does NOT use this set — it derives, so a new crate is honoured immediately even if
# the re-baseline was forgotten. The pin exists to force the re-baseline to be conscious, never to
# drive the decision.
EXPECTED_RELEASABLE = frozenset({
    "paigasus-kernel",
    "paigasus-proto",
    "paigasus-proto-derive",
})

# release-plz's default tag format. --assert refuses to run if `git_tag_name` is configured
# anywhere, because `tag_for` below assumes this shape.
def tag_for(name: str, version: str) -> str:
    return f"{name}-v{version}"


class Inconclusive(Exception):
    """Collection failed. Every raise site must end in nothing_to_release=false."""


# --- The decision, as a pure function ----------------------------------------------------------

def decide(event_name: str, packages: dict[str, str], tags: set[str]) -> tuple[bool, str]:
    """True means "nothing to release; skip the build matrix". Fixture-tested below."""
    if event_name != "push":
        # A workflow_dispatch is a deliberate act meaning "release now", so it ALWAYS builds.
        # That is the lever for the state where tags are cut but a registry is missing
        # (SMA-580's npm half). Spec §3.2 step 1.
        return False, f"event is {event_name!r}, not 'push' — build"
    if not packages:
        return False, "no releasable package resolved — build"
    if not tags:
        # THE SHALLOW-CHECKOUT FLOOR, and it is REDUNDANT FOR SAFETY — say so rather than
        # implying otherwise. With no tags every wanted tag is absent, so `missing` below is
        # non-empty and we would build anyway. It is kept for one reason: it names the
        # misconfiguration in the log, instead of reporting a list of "not yet cut" tags that
        # were in fact never looked for. A reader debugging a surprise build needs that
        # distinction.
        return False, "the repository reports no tags at all — build"
    missing = sorted(tag_for(n, v) for n, v in packages.items() if tag_for(n, v) not in tags)
    if missing:
        return False, f"tags not yet cut: {', '.join(missing)} — build"
    return True, "every releasable package is already tagged — nothing to release"


# --- Collection --------------------------------------------------------------------------------

def load_toml(path: Path) -> dict:
    try:
        with path.open("rb") as fh:
            return tomllib.load(fh)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        raise Inconclusive(f"cannot read {path}: {exc}") from exc


def assert_default_tag_format(cfg: dict) -> None:
    if "git_tag_name" in (cfg.get("workspace") or {}):
        raise Inconclusive("rs/release-plz.toml sets [workspace] git_tag_name; tag_for() assumes "
                           "release-plz's default <package>-v<version>")
    for pkg in cfg.get("package") or []:
        if isinstance(pkg, dict) and "git_tag_name" in pkg:
            raise Inconclusive(f"rs/release-plz.toml sets git_tag_name on "
                               f"{pkg.get('name')!r}; tag_for() assumes the default format")


def crate_manifests(rs_root: Path) -> dict[str, Path]:
    """Map package name -> Cargo.toml. Walks rs/crates/**, so it needs no cargo and no network."""
    found: dict[str, Path] = {}
    for manifest in sorted(rs_root.glob("crates/*/*/Cargo.toml")):
        pkg = load_toml(manifest).get("package") or {}
        name = pkg.get("name")
        if not isinstance(name, str) or not name:
            continue
        if name in found:
            raise Inconclusive(f"two manifests declare package {name!r}: {found[name]}, {manifest}")
        found[name] = manifest
    if not found:
        raise Inconclusive(f"no crate manifests under {rs_root}/crates — the tree moved")
    return found


def releasable_packages(rs_root: Path) -> dict[str, str]:
    """Package -> literal version, for every package release-plz would TAG.

    A package is tagged when Cargo does not say `publish = false` AND rs/release-plz.toml says
    neither `release = false` nor `publish = false`. An ABSENT release-plz entry reads as
    release = true / publish = true, which is release-plz's own default — so an unlisted crate
    counts as releasable and its missing tag makes us BUILD. That is the fail-safe direction.
    """
    cfg = load_toml(rs_root / "release-plz.toml")
    assert_default_tag_format(cfg)
    entries = {p["name"]: p for p in (cfg.get("package") or [])
               if isinstance(p, dict) and isinstance(p.get("name"), str)}

    out: dict[str, str] = {}
    for name, manifest in crate_manifests(rs_root).items():
        pkg = load_toml(manifest).get("package") or {}
        if pkg.get("publish") is False:
            continue
        entry = entries.get(name, {})
        if entry.get("release") is False or entry.get("publish") is False:
            continue
        version = pkg.get("version")
        if not isinstance(version, str):
            # `version.workspace = true` parses as a dict. There is no literal to tag against.
            raise Inconclusive(f"{name} has no literal [package] version in {manifest}")
        out[name] = version
    return out


def repo_tags(repo_root: Path) -> set[str]:
    try:
        proc = subprocess.run(["git", "-C", str(repo_root), "tag", "-l"],
                              capture_output=True, text=True, check=True)
    except (OSError, subprocess.CalledProcessError) as exc:
        raise Inconclusive(f"git tag -l failed: {exc}") from exc
    return {line.strip() for line in proc.stdout.splitlines() if line.strip()}


def run(repo_root: Path, event_name: str) -> tuple[bool, str]:
    try:
        packages = releasable_packages(repo_root / "rs")
        tags = repo_tags(repo_root)
    except Inconclusive as exc:
        return False, f"inconclusive ({exc}) — build"
    return decide(event_name, packages, tags)


# --- The fixture table -------------------------------------------------------------------------

# (label, event_name, packages, tags, expected verdict)
FIXTURES: list[tuple[str, str, dict[str, str], set[str], bool]] = [
    ("every releasable package is tagged -> skip", "push",
     {"a": "1.0.0", "b": "1.0.0"}, {"a-v1.0.0", "b-v1.0.0"}, True),
    ("one tag missing -> build", "push",
     {"a": "1.0.0", "b": "1.0.0"}, {"a-v1.0.0"}, False),
    ("every tag missing -> build", "push",
     {"a": "1.0.1"}, {"a-v1.0.0"}, False),
    # M6's exact shape: the kernel group bumped, the proto group already tagged.
    ("a kernel-only bump -> build (M6)", "push",
     {"paigasus-kernel": "0.1.1", "paigasus-proto": "0.1.0", "paigasus-proto-derive": "0.1.0"},
     {"paigasus-kernel-v0.1.0", "paigasus-proto-v0.1.0", "paigasus-proto-derive-v0.1.0"}, False),
    ("the repo has no tags at all -> build", "push", {"a": "1.0.0"}, set(), False),
    ("no releasable package resolved -> build", "push", {}, {"a-v1.0.0"}, False),
    # A dispatch ALWAYS builds, even in the state that would otherwise skip.
    ("workflow_dispatch with every tag present -> build", "workflow_dispatch",
     {"a": "1.0.0"}, {"a-v1.0.0"}, False),
    ("schedule with every tag present -> build", "schedule",
     {"a": "1.0.0"}, {"a-v1.0.0"}, False),
    # A prefix collision must not read as a hit.
    ("a tag that only PREFIXES the wanted one -> build", "push",
     {"a": "1.0.0"}, {"a-v1.0.0-rc1"}, False),
]


def self_test() -> int:
    rc = 0
    for label, event, packages, tags, want in FIXTURES:
        got, reason = decide(event, packages, tags)
        if got != want:
            print(f"FAIL {label!r}: expected {want}, got {got} ({reason})", file=sys.stderr)
            rc = 3

    # Collection-layer rows, which need the filesystem rather than the pure function.
    for label, fn in (
        ("a missing release-plz.toml is inconclusive", _missing_config_is_inconclusive),
        ("a workspace-inherited version is inconclusive", _workspace_version_is_inconclusive),
        ("a git_tag_name override is inconclusive", _tag_name_override_is_inconclusive),
    ):
        err = fn()
        if err:
            print(f"FAIL {label!r}: {err}", file=sys.stderr)
            rc = 3
    return rc
```

The three collection-layer helpers build a throwaway tree under `tempfile.mkdtemp()`, call
`releasable_packages`, and return `None` when `Inconclusive` was raised or a string describing
what happened instead. Write them directly above `self_test`.

- [ ] **Step 3: Write `main()` and the mode dispatch in `release_plan.py`**

```python
def _assert_repo(repo_root: Path) -> int:
    """--assert. The CI-side assertions; the runtime path uses none of them."""
    problems: list[str] = []
    try:
        packages = releasable_packages(repo_root / "rs")
    except Inconclusive as exc:
        print(f"release-plan: {exc}", file=sys.stderr)
        return 3
    derived = frozenset(packages)
    if derived != EXPECTED_RELEASABLE:
        problems.append(
            f"the derived releasable set {sorted(derived)} does not equal the pinned "
            f"EXPECTED_RELEASABLE {sorted(EXPECTED_RELEASABLE)}. If a crate legitimately became "
            f"publishable, re-baseline the pin deliberately — do not loosen the comparison.")
    if not repo_tags(repo_root):
        problems.append("the repository reports no tags; --assert needs a full checkout")
    for p in problems:
        print(f"release-plan: {p}", file=sys.stderr)
    return 3 if problems else 0


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--self-test", action="store_true")
    ap.add_argument("--assert", dest="do_assert", action="store_true")
    ap.add_argument("--fixture-count", action="store_true")
    ap.add_argument("--event-name", default="")
    ap.add_argument("repo_root", nargs="?", default=".")
    args = ap.parse_args(argv)

    if args.fixture_count:
        print(len(FIXTURES))
        return 0
    if args.self_test:
        return self_test()
    root = Path(args.repo_root)
    if args.do_assert:
        return _assert_repo(root)

    nothing, reason = run(root, args.event_name)
    print(f"release-plan: {reason}")
    print(f"nothing_to_release={'true' if nothing else 'false'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
```

- [ ] **Step 4: Run the self-test to verify it fails on a deliberately inverted decision**

Temporarily invert the final `return True, …` in `decide` to `return False, …`.

Run:
```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --project ci/release-plan --python '>=3.12' python3 ci/release-plan/release_plan.py --self-test
```
Expected: FAIL, exit 3, naming `'every releasable package is tagged -> skip'`. **Restore the
line by editing it back — do not `git checkout` the file**, which would discard the whole task's
work.

- [ ] **Step 5: Run the self-test to verify it passes**

Run the same command. Expected: no output, exit 0.

- [ ] **Step 6: Verify the real repository decides `true` today**

```bash
uv run --project ci/release-plan --python '>=3.12' python3 \
  ci/release-plan/release_plan.py --event-name push .
```
Expected, on `main` at `a73d13c` where all three tags exist:
```
release-plan: every releasable package is already tagged — nothing to release
nothing_to_release=true
```
And with `--event-name workflow_dispatch`, `nothing_to_release=false`.

- [ ] **Step 7: Verify `--assert` passes against the real tree**

```bash
uv run --project ci/release-plan --python '>=3.12' python3 \
  ci/release-plan/release_plan.py --assert .
```
Expected: exit 0, no output. If it reds naming the derived set, read what it derived and fix
`releasable_packages` — do **not** widen `EXPECTED_RELEASABLE` to match a wrong derivation.

- [ ] **Step 8: Write `run.sh`**

Model it on `ci/workflow-credentials/run.sh` (read that file first). Four modes. The
`--github-output` arm inverts the usual contract deliberately:

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Exit codes: 0 pass | 1 the repo is wrong | 2 infrastructure failed — EXCEPT --github-output,
# which always exits 0. See the comment on that arm.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$REPO_ROOT/ci/release-plan"

die_infra() { printf 'release-plan: %s\n' "$*" >&2; exit 2; }

command -v uv >/dev/null 2>&1 \
  || die_infra "uv is not on PATH — run 'proto install', or add ~/.proto/shims to PATH"

run_checker() {
  local rc=0
  uv run --project "$HERE" --python '>=3.12' python3 "$HERE/release_plan.py" "$@" || rc=$?
  case "$rc" in
    0) return 0 ;;
    3) return 1 ;;
    *) die_infra "checker exited $rc — uv or the interpreter failed, not an assertion" ;;
  esac
}

# THE RUNTIME ARM, and the one place in this repo where a checker failure must NOT fail its
# caller. A failed `plan` job SKIPS its dependents rather than building them — GitHub applies an
# implicit success() to a job-level `if:` with no status function — so a broken decision that
# exited non-zero would stop the release entirely. Fail-safe here means: write false, warn
# loudly, exit 0, and let the matrix build. The --self-test/--negative-control/--assert modes
# keep the normal contract, and CI runs those.
github_output() {
  local rc=0 out
  out="$(uv run --project "$HERE" --python '>=3.12' python3 \
    "$HERE/release_plan.py" --event-name "${GITHUB_EVENT_NAME:-}" "$REPO_ROOT" 2>&1)" || rc=$?
  printf '%s\n' "$out"
  if [ "$rc" -ne 0 ] || ! printf '%s\n' "$out" | grep -qE '^nothing_to_release=(true|false)$'; then
    printf '::warning::release-plan could not decide (rc=%s) — building, which is the fail-safe direction\n' "$rc"
    printf 'nothing_to_release=false\n' >> "${GITHUB_OUTPUT:-/dev/stdout}"
    exit 0
  fi
  printf '%s\n' "$out" | grep -E '^nothing_to_release=(true|false)$' >> "${GITHUB_OUTPUT:-/dev/stdout}"
  exit 0
}
```

Then a `negative_control()` and the `MODE` dispatch, following the sibling file's shape exactly.

- [ ] **Step 9: Write `negative_control()`**

It must prove the checker can report each direction, and that the wrapper's 3 → 1 translation
works. Four rows, using the `_expect` helper copied from the sibling:

1. `_expect 1` — `run_checker --assert "$tmp/empty"` on a tree with no `rs/` reaches the caller
   as 1, not 3. This is the translation row.
2. `_expect 0` — `run_checker --self-test`, so the control notices a broken table.
3. A row asserting the runtime arm prints `nothing_to_release=false` for a
   `GITHUB_EVENT_NAME=workflow_dispatch` run against the real repo — the state that would
   otherwise skip. Grep the output.
4. A row asserting the runtime arm prints `nothing_to_release=true` for
   `GITHUB_EVENT_NAME=push` against the real repo. Without both 3 and 4 the control cannot tell
   a working decision from one wired to a constant.

- [ ] **Step 10: Verify all four wrapper modes**

```bash
bash ci/release-plan/run.sh --self-test         ; echo "rc=$?"   # expect rc=0
bash ci/release-plan/run.sh --negative-control  ; echo "rc=$?"   # expect rc=0
bash ci/release-plan/run.sh --assert            ; echo "rc=$?"   # expect rc=0
GITHUB_EVENT_NAME=push bash ci/release-plan/run.sh --github-output ; echo "rc=$?"
# expect rc=0 and a `nothing_to_release=true` line
```

- [ ] **Step 11: Generate the lockfile and write the README**

```bash
uv lock --project ci/release-plan
```
`README.md` states what the gate asserts and, in a **Non-goals** section: it does not verify the
crate is actually on crates.io (only that the tag exists); it does not verify `release-approval`
has required reviewers; and its tag-format assumption is asserted only by
`assert_default_tag_format`.

- [ ] **Step 12: Commit**

```bash
git add ci/release-plan
git commit -m "ci(repo): add the release-plan tag-existence decision (SMA-603)"
```

---

## Task 2: Wire the checker into `repo:actionlint` as check 11

**Files:**
- Modify: `ci/actionlint/run.sh` (`SELF_TEST_COUNT` at line 40; a new `release_plan_self_test`
  beside `release_guard_self_test` at ~4509; a call in `run_self_tests` at ~4540; a new real-run
  block beside check 10's at ~5351)
- Modify: `ci/affected-graph/ci_targets.py` (`ACTIONLINT_SH_CALL_SITES` at line 614)

**Interfaces:**
- Consumes: `ci/release-plan/run.sh` and its four modes from Task 1.
- Produces: `release_plan_self_test`, a bash function name that
  `ACTIONLINT_SH_CALL_SITES` and check 7's definition counter both key on.

- [ ] **Step 1: Add `release_plan_self_test` to `ci/actionlint/run.sh`**

Place it immediately after `release_guard_self_test`. Read that function first and mirror it —
including `SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))` and the arity floor, which is what stops an
emptied fixture table passing.

```bash
# Check 11 (SMA-603) — the release-plan decision. Same bash-wrapper rationale as check 10: check
# 7 counts bash `*_self_test` DEFINITIONS and check 9 mutates lines inside run_self_tests, so a
# Python fixture table is invisible to both. Emptying it would leave this gate passing having
# asserted nothing; the arity floor closes that.
release_plan_sh() {
  bash ci/release-plan/run.sh "$@"
}

release_plan_self_test() {
  local rc=0 n
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))

  n="$(uv run --project ci/release-plan --python '>=3.12' python3 \
    ci/release-plan/release_plan.py --fixture-count)" \
    || infra "check 11: release_plan.py --fixture-count failed"
  case "$n" in ''|*[!0-9]*) infra "check 11: --fixture-count printed '$n', expected an integer" ;; esac
  [ "$n" -ge 9 ] || infra "check 11: release_plan.py reports $n fixtures, expected at least 9"

  release_plan_sh --self-test || { fail "check 11: release_plan.py --self-test reported a broken
      verdict. The release-plan decision is not deciding what it is documented to decide."; rc=1; }

  release_plan_sh --negative-control || { fail "check 11: ci/release-plan/run.sh
      --negative-control failed. The control that proves the checker can report each direction is
      itself broken."; rc=1; }

  return $rc
}
```

- [ ] **Step 2: Wire the call and bump the counter**

Add `release_plan_self_test` to `run_self_tests`, on its own line after `cargo_lock_step_self_test`.
Change `SELF_TEST_COUNT=12` to `SELF_TEST_COUNT=13` at line 40 and extend the trailing comment
that enumerates the tables with `release-plan`.

- [ ] **Step 3: Run the self-test path and verify the counter agrees**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/actionlint/run.sh --self-test ; echo "rc=$?"
```
Expected: rc=0. If it reports `13 of 12 self-tests ran` you changed the call but not
`SELF_TEST_COUNT`; if `12 of 13`, the reverse.

- [ ] **Step 4: Add check 11's real run**

Beside check 10's real-run block, add the `--assert` run. It must **route every exit status**,
for the reason check 10's comment gives — `run.sh` is `set -uo pipefail` with no `-e`, so an
unrouted status leaves the gate asserting nothing (measured at rc 127 from a missing `uv`).

```bash
# ---------------------------------------------------------------------------------------------
# Check 11 — the release-plan decision, over the real repository. Runs here (not in --self-test)
# because it reads the actual rs/ tree and the tag list, like checks 5/6/10.
#
# ROUTE EVERY STATUS, not just 2 — see check 10's comment for the measurement. run.sh maps the
# checker's 3 to 1 and everything else to 2, so 0, 1 and 2 are the only documented statuses here.
# ---------------------------------------------------------------------------------------------
rp_rc=0
release_plan_sh --assert || rp_rc=$?
if [ "$rp_rc" -eq 2 ]; then
  infra "check 11: ci/release-plan/run.sh --assert aborted (exit 2) — uv or the interpreter
      failed, not an assertion."
elif [ "$rp_rc" -eq 1 ]; then
  fail "check 11: ci/release-plan/run.sh --assert reported the repository is wrong — its stderr
      is above. The derived releasable set, a crate version, or the tag-name format changed."
elif [ "$rp_rc" -ne 0 ]; then
  infra "check 11: ci/release-plan/run.sh --assert exited $rp_rc, which is none of its three
      documented statuses (0 clean, 1 repo wrong, 2 infra). This file is 'set -uo pipefail' with
      NO -e, so an unrouted status would finish the gate rc 0 having asserted nothing."
fi
```

- [ ] **Step 5: Pin check 11's call sites in `ci_targets.py`**

Add three entries to `ACTIONLINT_SH_CALL_SITES`, whole-line matched, with a comment explaining
why each is needed — mirroring the existing entries' comments. The three are:
`release_plan_self_test` (the invocation inside `run_self_tests`), and the two production lines
`  release_plan_sh --self-test || { fail "check 11: release_plan.py --self-test reported a broken`
and `release_plan_sh --assert || rp_rc=$?`.

The comment must say what the existing entries' comments say for their own case: the function
name alone is a prefix of its own definition, so a substring test would survive deleting the
call.

- [ ] **Step 6: Verify the gate and the pin together**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:actionlint --force
moon run repo:affected-smoke --force
```
Expected: both pass. If `affected-smoke` fails in under 3 seconds with a `proto-shim` line in its
output, that is the known flake CLAUDE.md documents — capture the output, then re-run.

- [ ] **Step 7: Prove the pin bites**

Delete the `release_plan_self_test` line from `run_self_tests`, run
`moon run repo:affected-smoke --force`, and confirm it reds. **Restore by re-adding the line**,
not by `git checkout`, which would discard the task's other edits.

- [ ] **Step 8: Commit**

```bash
git add ci/actionlint/run.sh ci/affected-graph/ci_targets.py
git commit -m "ci(repo): run the release-plan checker as actionlint check 11 (SMA-603)"
```

---

## Task 3: Restructure the release guard's fixture base

This task adds **no new verdicts**. It only reshapes `_OK_MAIN` so Tasks 4 and 6 have a fixture
base that mirrors the real job graph. Splitting it out keeps the mechanical 34-row rework
reviewable apart from the new logic. **This is the task most likely to go wrong; work row by row.**

**Files:**
- Modify: `ci/actionlint/release_guard.py` (`_OK_MAIN` at line ~416, `FIXTURES` at ~436-620)

**Interfaces:**
- Consumes: nothing.
- Produces: an `_OK_MAIN` carrying jobs `release-pr`, `plan`, `build`, `approve-release`,
  `release` — the exact job ids Tasks 4 and 6 write fixtures against.

- [ ] **Step 1: Replace `_OK_MAIN`**

```python
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
```

- [ ] **Step 2: Run the self-test and collect every broken row**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test
```
Expected: FAIL, on many rows. Redirect to a file and work the list.

- [ ] **Step 3: Re-derive the anchors**

`grep -c '_OK_MAIN.replace' ci/actionlint/release_guard.py` reports **34**. The anchors that
change, by frequency:

| Old anchor | Occurrences | New anchor |
| --- | --- | --- |
| `"    needs: [plan]"` | 12 | `"    needs: [build, approve-release]"` — these rows break the `release` job's gating chain, and that line is now where the chain runs |
| `"run: release-plz release"` | 5 | unchanged; still on the `release` job |
| `"steps: [{run: echo hi}]"` | 3 | unchanged; still on `release-pr` |
| `"if: vars.PAIGASUS_RELEASE_ENABLED == 'true'"` and its indented / trailing-newline variants | 4 | unchanged; still on `plan` |
| `"== 'true'"` | 2 | **ambiguous now** — it also matches `build`'s `!= 'true'`. Retarget each to the full gate expression |
| `"steps: [{run: release-plz release}]"` | 1 | unchanged |

For each of the 12 retargeted rows, re-read the row's `want` string: if it names a job id, it
must now name the job the mutation actually ungates.

- [ ] **Step 4: Verify the self-test passes and the count did not shrink**

```bash
uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test ; echo "rc=$?"
uv run --locked --project py python3 ci/actionlint/release_guard.py --fixture-count
```
Expected: rc=0, and the count is **44** — unchanged. A row deleted rather than fixed is the
failure mode here; the count is the guard against it.

- [ ] **Step 5: Verify the guard still passes on the real workflow**

```bash
uv run --locked --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml
echo "rc=$?"
```
Expected: rc=0, no output. `release.yml` is unchanged in this task, so any violation means the
restructure broke a verdict rather than a fixture.

- [ ] **Step 6: Commit**

```bash
git add ci/actionlint/release_guard.py
git commit -m "test(repo): reshape the release-guard fixture base for the plan job (SMA-603)"
```

---

## Task 4: Guard V8 — the approval boundary, both directions

**Files:**
- Modify: `ci/actionlint/release_guard.py` (constants near line 53; a new verdict function; a
  call in `check_main` ~306; `main()` ~765; new `FIXTURES` rows)

**Interfaces:**
- Consumes: `gated_path_jobs`, `job_publishes`, `needs_of`, `if_text` — all existing.
- Produces: `APPROVAL_JOB = "approve-release"`, `approval_boundary_violations(jobs, name)`,
  `pre_approval_callees(doc)`, `check_called_pre_approval(doc, name)`.

- [ ] **Step 1: Write the failing fixture rows first**

Add to `FIXTURES`, above the existing V7 rows:

```python
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
```

- [ ] **Step 2: Run the self-test to verify the new rows fail**

Run: `uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test`
Expected: FAIL on all six, each reporting `expected a violation containing 'V8…', got: (clean)`
— except the CONTROL row, which should already be clean.

- [ ] **Step 3: Implement `approval_boundary_violations`**

```python
# V8. The approval gate is the ONE human checkpoint in release.yml, and everything downstream of
# it is irreversible. Two directions, and BOTH are needed: V8b says nothing upstream may publish,
# V8c says every publisher must be downstream. Without V8c, deleting ` approve-release` from the
# `release` job's needs: removes the only gate in the file and passes V1, V3, V4, V7 and V8a/b.
APPROVAL_JOB = "approve-release"


def approval_boundary_violations(jobs: dict, name: str) -> list[str]:
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
```

Call it from `check_main`, once, outside the per-job loop, immediately before `return out`.

- [ ] **Step 4: Run the self-test to verify V8a–c pass**

Expected: rc=0. If the CONTROL row now reds, `job_publishes`'s `--dry-run` exemption is not
being reached — do not weaken the control, fix the call.

- [ ] **Step 5: Add V8d — the callee clause — and its fixture**

`check_called` deliberately *permits* a publish step in a `workflow_call`-only workflow, and the
fixture at `release_guard.py:477` asserts that. But `wheels` and `prebuild` are `uses:` jobs
**upstream of the approval gate**, so that permission is a live publish-before-approval path.
Add, and wire into `main()` beside the existing `uses.startswith("./")` loop:

```python
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
```

Its fixture cannot be a `FIXTURES` row (it needs two files), so add it to the
`_critical2_end_to_end` neighbourhood as a third helper returning `str | None`, and register it
in `self_test`'s helper loop. It writes a two-file tree: a main workflow whose pre-approval job
carries `uses: ./called.yml`, and a `called.yml` that is `workflow_call`-only and runs
`cargo publish`. Assert `main()` returns 1 and prints a `V8d` line.

- [ ] **Step 6: Verify the whole guard**

```bash
uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test ; echo "rc=$?"
uv run --locked --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml ; echo "rc=$?"
moon run repo:actionlint --force
```
Expected: rc=0 for all three. The real `release.yml` passes V8 **before** the plan job exists —
`gated_path_jobs("approve-release")` is `{approve-release, wheels, prebuild, proto-dist}`, none
of which publishes, and `release`/`publish-pypi`/`publish-npm` all have `approve-release` on
their path. If it reds, read the message before changing the verdict.

- [ ] **Step 7: Commit**

```bash
git add ci/actionlint/release_guard.py
git commit -m "ci(repo): assert the release approval boundary in both directions (SMA-603)"
```

---

## Task 5: The `plan` job and the gating change in `release.yml`

**Files:**
- Modify: `.github/workflows/release.yml` (jobs at lines 351-364)

**Interfaces:**
- Consumes: `ci/release-plan/run.sh --github-output` from Task 1.
- Produces: a job id `plan` with `outputs.nothing_to_release`, mapped from a step whose `id` is
  `decide`; and the literal consumer condition
  `needs.plan.outputs.nothing_to_release != 'true'` on three jobs. Task 6's V9 pins all of it.

- [ ] **Step 1: Add the `plan` job immediately above `wheels`**

```yaml
  # THE SKIP DECISION (SMA-603). Without this, every push to `main` with the flag on builds the
  # full 12-leg matrix and then waits at `approve-release` for a human, indefinitely, even when
  # there is nothing to release — observed on run 33265567805.
  #
  # WHY NOT `release-plz release --dry-run`. Measured (spec M6): with only the `kernel` version
  # group bumped, release-plz logs that it WOULD publish paigasus-kernel and cut
  # `paigasus-kernel-v0.1.1`, and still prints `{"releases":[]}` at exit 0. That array records
  # PERFORMED releases and a dry run performs none, so it cannot tell "nothing to release" from
  # "a release is pending". Reading it would silently skip every kernel-group release. Do not
  # reintroduce it.
  #
  # This job is the ONLY holder of the literal flag gate now. `wheels`, `prebuild` and
  # `proto-dist` gate transitively through `needs: [plan]` — with the flag off this job skips,
  # and a job whose needs: dependency skipped is itself skipped.
  plan:
    name: decide whether anything is releasable
    if: vars.PAIGASUS_RELEASE_ENABLED == 'true'
    runs-on: ubuntu-latest
    timeout-minutes: 10
    outputs:
      # A STEP output is not a JOB output. Without this mapping every
      # `needs.plan.outputs.nothing_to_release` below is the empty string — which builds, so it
      # fails safe, but the feature would never fire. release_guard.py V9c asserts this key AND
      # that `steps.decide` names a step that exists in this job.
      nothing_to_release: ${{ steps.decide.outputs.nothing_to_release }}
    steps:
      - name: Checkout
        uses: actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1  # v7.0.1
        with:
          # LOAD-BEARING. The tags ARE the signal, and a shallow checkout has none.
          fetch-depth: 0
          persist-credentials: false

      # ONE PHYSICAL LINE. release_guard.py's command_segments is per physical line, so a
      # backslash continuation would be judged without its flags.
      - name: Decide
        id: decide
        env:
          GITHUB_EVENT_NAME: ${{ github.event_name }}
        run: ci/release-plan/run.sh --github-output
```

- [ ] **Step 2: Change the three build jobs' gating**

Replace `if: vars.PAIGASUS_RELEASE_ENABLED == 'true'` on `wheels`, `prebuild` and `proto-dist`
with these two lines each, keeping every other key:

```yaml
    needs: [plan]
    # FAIL-SAFE POLARITY, and it is carried by the operator. The output is named for the SKIP
    # condition and tested with `!=`, so anything but the literal 'true' builds — 'false', the
    # empty string, an unset output. `== 'true'` would invert it; `== 'false'` would fail closed
    # on an unset output. release_guard.py V9b pins both accepted forms.
    if: needs.plan.outputs.nothing_to_release != 'true'
```

- [ ] **Step 3: Verify the guard and actionlint still pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --locked --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml ; echo "rc=$?"
moon run repo:actionlint --force
```
Expected: rc=0 and the gate passes. V1 is satisfied transitively: `wheels`'s `needs:` resolves to
`plan`, which carries the literal `GATE_EXPR`. V8 is satisfied because `plan` runs no publish
marker.

- [ ] **Step 4: Prove the gating change did not break the flag**

Confirm by reading `is_gated` (`release_guard.py:185`) that removing the literal gate from
`plan` reds all four jobs. Temporarily delete `plan`'s `if:` line, run the guard, and confirm it
reports V1 violations for `plan`, `wheels`, `prebuild` and `proto-dist`. **Restore the line by
re-adding it**, not by `git checkout`.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "ci(repo): add the release plan job and gate the build matrix on it (SMA-603)"
```

---

## Task 6: Guard V9 — the plan job's contract

**Files:**
- Modify: `ci/actionlint/release_guard.py` (constants; a new verdict function; a call in
  `check_main`; new `FIXTURES` rows)

**Interfaces:**
- Consumes: the job id `plan`, its `outputs.nothing_to_release`, its step id `decide`, and the
  consumer condition — all from Task 5.
- Produces: `PLAN_JOB`, `PLAN_GATE_EXPR`, `ACCEPTED_PLAN_FORMS`, `plan_contract_violations`.

- [ ] **Step 1: Write the failing fixture rows first**

The V9a row must rename the job rather than delete it — deleting `plan` also breaks the `needs:`
chain and reds V1, so the row would pass for the wrong reason. Renaming keeps every other verdict
satisfied and isolates V9a:

```python
    ("V9a: the plan job renamed out from under V9", "main",
     _OK_MAIN.replace("  plan:\n", "  planning:\n")
             .replace("needs: [plan]", "needs: [planning]")
             .replace("needs.plan.outputs", "needs.planning.outputs"), "V9a"),
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
    ("V9c: the outputs mapping is missing", "main",
     _OK_MAIN.replace("    outputs:\n      nothing_to_release: "
                      "${{ steps.decide.outputs.nothing_to_release }}\n", ""), "V9c"),
    ("V9c: the mapping names a step id that does not exist", "main",
     _OK_MAIN.replace("${{ steps.decide.outputs.nothing_to_release }}",
                      "${{ steps.decdie.outputs.nothing_to_release }}"), "V9c"),
    ("V9d: the decision step no longer invokes the checker", "main",
     _OK_MAIN.replace("run: ci/release-plan/run.sh --github-output",
                      "run: echo nothing_to_release=true >> \"$GITHUB_OUTPUT\""), "V9d"),
```

- [ ] **Step 2: Run the self-test to verify the new rows fail**

Expected: FAIL on each new row, `expected a violation containing 'V9…', got: (clean)`.

- [ ] **Step 3: Implement `plan_contract_violations`**

```python
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


def plan_contract_violations(jobs: dict, name: str) -> list[str]:
    plan = jobs.get(PLAN_JOB)
    if not isinstance(plan, dict):
        return [f"{name}: V9a: no job named '{PLAN_JOB}' exists. V9 keys on that literal name, so "
                f"without this floor a rename would leave it asserting nothing."]
    out: list[str] = []

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
                       f"output.")

    outs = plan.get("outputs")
    expr = outs.get(PLAN_OUTPUT) if isinstance(outs, dict) else None
    if not isinstance(expr, str):
        out.append(f"{name}: V9c: job '{PLAN_JOB}' declares no outputs.{PLAN_OUTPUT}. A STEP "
                   f"output is not a JOB output, so every consumer would read the empty string.")
    else:
        m = _PLAN_STEP_RE.search(expr)
        if not m:
            out.append(f"{name}: V9c: outputs.{PLAN_OUTPUT} is {expr!r}, which names no "
                       f"steps.<id>.outputs.{PLAN_OUTPUT}.")
        else:
            ids = {s.get("id") for s in (plan.get("steps") or []) if isinstance(s, dict)}
            if m.group(1) not in ids:
                out.append(f"{name}: V9c: outputs.{PLAN_OUTPUT} names step id {m.group(1)!r}, "
                           f"which does not exist in '{PLAN_JOB}'. A typo here yields '' "
                           f"forever, silently.")

    runs = "\n".join(str(s.get("run") or "")
                     for s in (plan.get("steps") or []) if isinstance(s, dict))
    if PLAN_SCRIPT not in runs:
        out.append(f"{name}: V9d: job '{PLAN_JOB}' never invokes {PLAN_SCRIPT}. Without this, "
                   f"V9c passes on an inline `echo {PLAN_OUTPUT}=true`.")
    return out
```

Call it from `check_main` beside `approval_boundary_violations`.

- [ ] **Step 4: Verify the self-test and the real workflow**

```bash
uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test ; echo "rc=$?"
uv run --locked --project py python3 ci/actionlint/release_guard.py .github/workflows/release.yml ; echo "rc=$?"
```
Expected: rc=0 for both. The real `release.yml` now has the plan job from Task 5, so V9 has a
real subject.

- [ ] **Step 5: Prove V9b bites the real file**

Change `wheels`'s `if:` in `.github/workflows/release.yml` to `== 'true'`, run the guard, confirm
it reports V9b naming `wheels`. **Restore by editing the operator back.**

- [ ] **Step 6: Commit**

```bash
git add ci/actionlint/release_guard.py
git commit -m "ci(repo): pin the plan job's output wiring and fail-safe polarity (SMA-603)"
```

---

## Task 7: Documentation

**Files:**
- Modify: `.github/workflows/release.yml` (header ~124-135; the `workflow_dispatch` comment ~60;
  the comment at ~474)
- Modify: `docs/ops/RUNBOOK-release-activation.md` (§6 at ~479; the step-J row at ~696)
- Modify: `CLAUDE.md`

- [ ] **Step 1: Rewrite the header's "NO `plan` JOB EXISTS" block**

Its *conclusion* — no dry-run-based plan job — survives; its *reason* is replaced. Record: the
dry-run's `releases` array is empty in dry mode even for a real release (M6), so no dry-run-based
plan job can work, for a more general reason than the derive-crate one; the derive blocker is
permanent but applies to the `proto` group only; and the tag check replaced it. Retitle the block
so it no longer says a plan job does not exist.

- [ ] **Step 2: Fix the two other stale in-file statements**

At ~474, `"There is no `plan` job (see the file-header decision) — … a job that does not exist."`
becomes a statement that the plan job decides *whether to build*, while the version and
per-family flags still come from `release`'s own `released` output. At ~60, replace the "REMOVE
this trigger once the first release has published" instruction with the spec §3.4 decision: the
trigger is **permanent**, because a dispatch is now the "build anyway" lever for the state where
tags are cut but a registry is missing.

Also update the header's "HOW THE GATE REACHES EACH JOB — three ways, not one" note: it is now
**two** ways, literal on `plan` and transitive everywhere else.

- [ ] **Step 3: Update the runbook**

§6 assumes every dispatch reaches the approval gate. That stays true for `workflow_dispatch` and
stops being true for a push; say which is which. The step-J row instructing removal of the
`workflow_dispatch` trigger is withdrawn, with a pointer to the release.yml comment.

- [ ] **Step 4: Update `CLAUDE.md`**

Three edits in the release gotchas:
1. The entry saying the dry-run *requires a git token* — correct it: it makes a live
   authenticated `GET /repos/{owner}/{repo}/commits/{sha}/pulls` and dies 401 on a bad token (M1).
2. The entry saying "The dry-run cannot pass until `paigasus-proto-derive` is published on
   crates.io … This is why the release job graph carries no `plan`-stage dry-run" — true for the
   `proto` version group only, and no longer the operative reason.
3. A new entry for M6: `release-plz release --dry-run --output json` prints `{"releases":[]}` at
   exit 0 **even when it would publish**, so the array cannot be used to detect a pending
   release. Name `ci/release-plan/` as what replaced it, and record that M2/M6 are pinned to
   release-plz 0.3.158 and must be re-measured on a bump.

Do **not** add a second copy of the `<!-- ci-targets:begin -->` marker or its command — a second
copy anywhere in the file, even inside backticks, reds `repo:affected-smoke`.

- [ ] **Step 5: Run the full gate graph, as CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials --base origin/main \
  --include-relations
```
Expected: all pass. Diagnose an unattributed failure via `.moon/cache/ciReport.json`. The three
`repo:release-parity*` gates abort **inconclusive at rc=2** inside an agent session because
`proto` emits NDJSON — `unset AI_AGENT CLAUDECODE CLAUDE_CODE_ENTRYPOINT` before running them, or
an inconclusive abort reads as a pass.

- [ ] **Step 6: Commit**

```bash
git add .github/workflows/release.yml docs/ops/RUNBOOK-release-activation.md CLAUDE.md
git commit -m "docs(repo): record the plan job and correct the dry-run claims (SMA-603)"
```

---

## Acceptance

CI cannot prove the end-to-end behaviour: `release.yml` has no `pull_request` trigger and must
never gain one. The first push to `main` after merge is the acceptance evidence. Its expected
shape:

- `release-pr` runs.
- `plan` runs and logs `every releasable package is already tagged — nothing to release`, then
  `nothing_to_release=true`.
- `wheels`, `prebuild`, `proto-dist`, `approve-release`, `release`, `publish-pypi` and
  `publish-npm` all **skip**. No human is asked to approve anything.

If instead the matrix builds, read `plan`'s log: the reason line names which tag was missing or
what was inconclusive. Building is the fail-safe direction, so that is a correctness question,
not an incident.
