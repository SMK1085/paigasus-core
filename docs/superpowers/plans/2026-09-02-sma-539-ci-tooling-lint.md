# SMA-539 CI Tooling Lint Gates — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Lint the two CI-tooling surfaces nothing currently checks — inline workflow bash (by enabling actionlint's shellcheck integration) and `ci/**/*.py` (via a new `repo:ruff-ci` gate).

**Architecture:** Half A changes one existing gate: `ci/actionlint/run.sh` resolves a hash-pinned `shellcheck` through `uv run --locked --project py` and passes it to actionlint, failing closed if it cannot. Half B adds `repo:ruff-ci`, a pure-bash gate that derives its corpus with `git ls-files`, resolves `ruff` the same way, and invokes it directly so `rc 1` is unambiguously a lint verdict. Half B pays six registry obligations; Half A pays none.

**Tech Stack:** bash, Moon 2.5.3, uv 0.11.16, ruff 0.16.5, actionlint 1.7.12, shellcheck 0.11.0 (via `shellcheck-py`), Python 3.12.

**Spec:** `docs/superpowers/specs/2026-09-02-sma-539-ci-tooling-lint-design.md` (rev 2)

## Global Constraints

- Every new source file opens with `# SPDX-License-Identifier: Apache-2.0`.
- Branch is `feature/sma-539-repo-lint-the-ci-tooling-inline-workflow-bash-and-cipy`; conventional commits scoped `ci(repo):`, `fix(ci):`, `docs(ci):`.
- **Commit message trap:** a body line beginning `Word:` is parsed by commitlint as a footer and reds `footer-leading-blank`. Never start a body line that way.
- `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` before any `moon`/`uv`/`actionlint` call.
- Exit-code contract for every gate script: `0` pass, `1` the repo is wrong, `2` infrastructure failed. Never conflate them.
- Every `uv run --project py` invocation carries `--locked`. A bare one re-locks `py/uv.lock` as a side effect.
- Ruff version and rule set come from `py/pyproject.toml` + `py/uv.lock` only. No second config, no second lockfile.
- shellcheck specifier is `shellcheck-py>=0.11.0.1,<0.12` — bounded, matching `py/pyproject.toml`'s stated convention. Not an exact pin.
- Interpreter floor for `ci/` scripts run under bare `python3` is **3.12**, matching `target-version = "py312"`.
- `SELF_TASK_EXPECTED_GLOBS` entries are written **glob-sorted then file-sorted**, not in authored order (`ci_targets.py:1408-1423`).
- Mutation evidence must come from invoking `ci/<gate>/run.sh` **directly**. `moon run` reports its own status and cannot show rc 2 vs rc 1.

---

### Task 1: Baseline cleanup — the 27 ruff violations

Makes `ci/**/*.py` pass on an unmutated tree (AC B6). Must land before the gate, or the gate reds on arrival.

**Files:**
- Modify: `ci/actionlint/release_guard.py` (B904 `:33`, C420 `:397`)
- Modify: `ci/affected-graph/cargo_moon_parity.py` (UP031 `:2322`, `:3032`; SIM300 `:2472`)
- Modify: `ci/publish-metadata/categories.py` (UP017 ×3, RUF100 ×3, RUF005 ×3) + the §5.3 preflight
- Modify: `ci/pyo3-stub/check.py` (N818 `:150`, UP014 `:661`, N806 `:1097`)
- Modify: `ci/release-plan/release_plan.py` (N818 `:56`, RUF100 `:220`, `:491`)
- Modify: `ci/workflow-credentials/workflow_credentials.py` (E402 ×5, N818 `:59`, RUF100 `:704`)
- Modify: `ci/pyo3-stub/README.md`, `ci/release-plan/README.md`, `ci/workflow-credentials/README.md` (exception vocabulary)

- [ ] **Step 1: Record the pre-fix baseline**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --project py --locked ruff check --config py/pyproject.toml \
  --output-format concise $(git ls-files 'ci/**/*.py') | tee /tmp/sma539-before.txt | tail -3
```
Expected: `Found 27 errors.`

- [ ] **Step 2: Apply the 12 safe autofixes**

```bash
uv run --project py --locked ruff check --config py/pyproject.toml --fix \
  $(git ls-files 'ci/**/*.py')
```
This fixes C420, SIM300, UP014, the three UP017, and the six RUF100. **It deletes the whole `# noqa` comment, prose included** — Step 3 restores the four that carry prose.

- [ ] **Step 3: Restore the four RUF100 explanations**

Four of the six carried prose. Re-add each as a plain comment with the `noqa:` token dropped. In `ci/workflow-credentials/workflow_credentials.py:704`:

```python
    except Exception as exc:  # an unexpected crash is INFRA, never an assertion
```

Do the same at `ci/publish-metadata/categories.py:320` and `ci/release-plan/release_plan.py:220`, `:491`, preserving each site's original wording. `categories.py:332` and `:347` were bare `# noqa: BLE001` with no prose — leave them deleted.

- [ ] **Step 4: Fix the three N818 exception names**

Rename `Refused` → `RefusedError` (`ci/pyo3-stub/check.py`), `Inconclusive` → `InconclusiveError` (`ci/release-plan/release_plan.py`), `AssertionFailure` → `AssertionFailureError` (`ci/workflow-credentials/workflow_credentials.py`). Update every reference in the same file, the comment references at `ci/release-plan/run.sh:156` and `ci/workflow-credentials/run.sh:65`, and the vocabulary in the three READMEs.

`ci/release-plan/release_plan.py:221` and `:492` print `type(exc).__name__`, so this is operator-visible. It is safe because the only programmatic consumer greps `^nothing_to_release=(true|false)$`, a different token — verify that still holds:

```bash
grep -n 'nothing_to_release' ci/release-plan/run.sh
```

- [ ] **Step 5: Fix N806 and the remaining UP031/RUF005/B904**

`ci/pyo3-stub/check.py:1097` — rename the in-function `_MOD` to `_mod_tmpl` and update its uses. `cargo_moon_parity.py:2322,:3032` — convert `%`-formatting to f-strings. `categories.py:447,456,659` — `[*good, "x"]` instead of `good + ["x"]`. `release_guard.py:33` — add `from exc`.

- [ ] **Step 6: Fix the five E402 imports**

In `ci/workflow-credentials/workflow_credentials.py`, move `glob`, `os`, `re`, `tempfile` and `yaml` up beside `import sys` at `:12`. Leave the `RC_*` constants and `class InfraError` where they are. `_StrictLoader` at `:63` is the only ordering constraint and moving imports **up** satisfies it.

- [ ] **Step 7: Add the interpreter-floor preflight to categories.py**

The `UP017` fix introduced `datetime.UTC`, which is 3.11+, into a file invoked as bare `python3` (`ci/publish-metadata/run.sh:1062,1734,1737`). Add immediately after the imports:

```python
# SMA-539 §5.3 — this file runs under the SYSTEM python3 (ci/publish-metadata/run.sh invokes it
# bare, and the Moon task is toolchain: 'system'), not a uv-pinned interpreter. `datetime.UTC`
# below is 3.11+, and py/pyproject.toml's target-version is py312, so every future UP rule can
# ratchet this floor further. State it rather than failing with an AttributeError mid-run.
if sys.version_info < (3, 12):
    print(
        f"publish-metadata: needs Python >= 3.12, got {sys.version_info.major}."
        f"{sys.version_info.minor}. Run it under the proto-pinned interpreter.",
        file=sys.stderr,
    )
    raise SystemExit(2)
```

Confirm `import sys` is already present and that `2` is this file's infrastructure code, not an assertion code.

- [ ] **Step 8: Verify the corpus is clean**

```bash
uv run --project py --locked ruff check --config py/pyproject.toml $(git ls-files 'ci/**/*.py')
```
Expected: `All checks passed!`

- [ ] **Step 9: Verify nothing regressed**

Every touched script must still self-test. Run each directly:

```bash
bash ci/workflow-credentials/run.sh --self-test && bash ci/workflow-credentials/run.sh --negative-control
bash ci/release-plan/run.sh --self-test
python3 ci/pyo3-stub/check.py --self-test && python3 ci/pyo3-stub/check.py --negative-control
python3 ci/publish-metadata/categories.py --self-test
uv run --locked --project py python3 ci/actionlint/release_guard.py --self-test
```
Expected: all pass. A failure here means a rename missed a reference.

- [ ] **Step 10: Commit**

```bash
git add ci/
git commit -m "fix(ci): clear the 27 ruff violations under ci/ (SMA-539)"
```

---

### Task 2: Half A — enable shellcheck in `repo:actionlint`

**Files:**
- Modify: `py/pyproject.toml` (dev group), `py/uv.lock` (regenerated)
- Modify: `ci/actionlint/run.sh` (`:79` ARGS; resolution after `:4760`; check-3 fixture near `:5268`)
- Modify: `ci/actionlint/README.md`

**Interfaces:**
- Produces: `ARGS` at `:79` gains `-shellcheck="$SHELLCHECK_BIN"`; `SHELLCHECK_BIN` is set after the `--self-test` early exit.

- [ ] **Step 1: Add the dependency**

In `py/pyproject.toml`'s `[dependency-groups] dev`, keeping the list alphabetical:

```toml
    # SMA-539 — shellcheck for actionlint's `run:` blocks. Sourced from PyPI, not a proto plugin,
    # because shellcheck's own release ships 13 archives and NO checksums asset (re-measured
    # 2026-09-02), which SMA-525 D2 refused. shellcheck-py is checksummed on both supported
    # hosts: uv.lock pins a sha256 per wheel, and three of the republisher's digests were
    # verified by hand against koalaman's own release assets. A BUMP RE-OPENS THAT — re-verify
    # rather than assume (spec L1). The sdist path (linux aarch64/riscv64/armv6hf, no wheel)
    # additionally builds through setuptools-download, which uv.lock does NOT pin (spec L3).
    "shellcheck-py>=0.11.0.1,<0.12",
```

- [ ] **Step 2: Regenerate the lock and confirm hashes landed**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv lock --project py
grep -A8 'name = "shellcheck-py"' py/uv.lock | grep -c 'hash = "sha256:'
```
Expected: `5` (one sdist + four wheels).

- [ ] **Step 3: Resolve the binary, after the `--self-test` early exit**

In `ci/actionlint/run.sh`, immediately before the `command -v actionlint` guard at `:5157`, add:

```bash
# SMA-539 — shellcheck for check 1's `run:` blocks. SMA-525 disabled the integration because an
# opportunistic PATH lookup made the gate's strictness a property of the host; this resolves ONE
# hash-pinned binary from py/uv.lock instead, so a dev box and CI agree.
#
# PLACEMENT IS LOAD-BEARING. This sits AFTER the --self-test early exit at :4760, beside the
# actionlint guard, for the same reason that guard does: --self-test must stay runnable on a
# machine with neither binary installed. It also keeps check 9's mutant fan-out — one --self-test
# subprocess per self-test — from paying 15 `uv run` invocations against one py/.venv.
#
# FAIL CLOSED. There is deliberately NO fallback to `-shellcheck=`: a silent downgrade to
# "whatever this host has" is exactly the failure SMA-525 refused, and it would be invisible on
# a green. `[ -x ]` is the ci/release-parity/ecosystems/release-plz.sh idiom — assert the
# resolution rather than discovering it 80 lines later.
SHELLCHECK_BIN="$(uv run --locked --project py python3 -c \
  'import shutil, sys; p = shutil.which("shellcheck"); sys.exit(1) if not p else print(p)')" \
  || infra "could not resolve shellcheck via 'uv run --locked --project py' — run 'uv sync --project py'"
[ -x "$SHELLCHECK_BIN" ] || infra "resolved shellcheck is not executable: $SHELLCHECK_BIN"
ARGS+=("-shellcheck=$SHELLCHECK_BIN")
```

Then change `:79` from `ARGS=(-shellcheck= -pyflakes=)` to `ARGS=(-pyflakes=)`, updating the comment above it to point at the new resolution site and to record that `-pyflakes=` stays off because actionlint applies pyflakes only to steps declaring `shell: python` — `wheels.yml`'s bash heredocs are invisible to it regardless (spec L4).

- [ ] **Step 4: Verify the gate still passes and measure the delta**

```bash
time bash ci/actionlint/run.sh; echo "rc=$?"
```
Expected: rc 0. Record the wall time against the 35.1s baseline in the spec's §1.4.

- [ ] **Step 5: M1 — prove a shell-variable defect reds**

```bash
cp .github/workflows/images.yml /tmp/images.yml.bak
python3 - <<'PY'
import pathlib
p = pathlib.Path(".github/workflows/images.yml")
t = p.read_text().replace("run: |", "run: |\n          rm -rf $UNQUOTED_TARGET", 1)
p.write_text(t)
PY
bash ci/actionlint/run.sh; echo "rc=$? (expect 1, with SC2086)"
cp /tmp/images.yml.bak .github/workflows/images.yml
```
Expected: rc 1 and `SC2086` in the output. **Use a shell variable, not `${{ }}`** — a `${{ }}` fixture passes and proves nothing (spec §1.8).

- [ ] **Step 6: M2 — prove an unresolvable shellcheck aborts rc 2, never green**

```bash
uv run --locked --project py python3 -c \
  'import shutil,sys; p=shutil.which("shellcheck"); sys.exit(1) if not p else print(p)'   # sanity: prints a path
PATH_SAVE="$PATH"
# Simulate by pointing the resolution at a venv without the package:
uv run --locked --project ci/release-plan python3 -c \
  'import shutil,sys; p=shutil.which("shellcheck"); sys.exit(1) if not p else print(p)'; echo "rc=$? (expect 1 — no shellcheck there)"
```
Then temporarily edit the `SHELLCHECK_BIN` line to use `--project ci/release-plan`, run `bash ci/actionlint/run.sh`, confirm **rc 2** and an `INFRASTRUCTURE ERROR` message, and revert. Record the output. This is the fail-closed proof and must not be skipped.

- [ ] **Step 7: Add the check-3 fixture**

After the four existing `selftest_expect_tag` calls (ending near `:5268`), add a fifth. Check 3 is the right home: its fixtures already require actionlint, it runs after the PATH guard, and check 4 is its healthy control — so this needs no `SELF_TEST_COUNT` bump and no mutant multiplication.

```bash
# SMA-539 — the shellcheck integration itself. Without this, a regression that leaves
# SHELLCHECK_BIN unset (or reinstates `-shellcheck=`) leaves every other check green while the
# 648 lines of inline bash go uninspected — the SMA-525 failure re-created inside its own fix.
# A SHELL variable, deliberately: actionlint replaces ${{ }} with inert placeholders before
# shellcheck sees them, so an expression fixture would pass and assert nothing (spec §1.8).
selftest_expect_tag 'unquoted shell variable' 'shellcheck' 'name: selftest
on: [push]
jobs:
  j:
    runs-on: ubuntu-latest
    steps:
      - run: rm -rf $TARGET
'
```

- [ ] **Step 8: Verify the fixture fires, then mutate it**

```bash
bash ci/actionlint/run.sh; echo "rc=$?"        # expect 0
```
Then temporarily change the fixture body to `- run: rm -rf "$TARGET"` (correctly quoted) and re-run: it must **fail**, proving the fixture asserts rather than decorating. Revert.

- [ ] **Step 9: Update `ci/actionlint/README.md`**

Add the integrity story (AC A2): where shellcheck comes from, that its digests were verified against koalaman's assets, that a version bump re-opens that check. Add to Limitations: the `${{ }}` blind spot (with the measured A/B), and that `SC2148`/`SC2164` cannot fire because actionlint supplies the shell and injects `set -e`.

- [ ] **Step 10: Commit**

```bash
git add py/pyproject.toml py/uv.lock ci/actionlint/
git commit -m "ci(repo): lint inline workflow bash with a pinned shellcheck (SMA-539)"
```

---

### Task 3: `ci/ruff/run.sh` — the gate script

**Files:**
- Create: `ci/ruff/run.sh`, `ci/ruff/README.md`

**Interfaces:**
- Produces: `bash ci/ruff/run.sh [--self-test|--negative-control]`. Exit `0` pass, `1` lint violations or a collapsed corpus, `2` infrastructure.
- Produces: the five lines Task 6 pins as `RUFF_SH_CALL_SITES`.

- [ ] **Step 1: Write the script**

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# repo:ruff-ci — lint ci/**/*.py against py/pyproject.toml's Ruff rule set (SMA-539).
#
# .moon/tasks/python.yml scopes `ruff check` to the py project, so ci/ has never been linted;
# ci_targets.py merged through a full review carrying three RUF005 violations (SMA-541).
#
# Exit codes: 0 pass | 1 the repo is wrong | 2 infrastructure failed.
#
# WHY THE BINARY IS RESOLVED, NOT PIPED THROUGH `uv run`. `ruff check` exits 1 on violations and
# `uv` exits 1 on a failed resolution or a stale --locked lock, so one combined command cannot
# tell "ci/ has lint violations" from "PyPI is down". CLAUDE.md records that lesson verbatim for
# repo:workflow-credentials. .moon/tasks/python.yml runs a BARE, re-locking `uv run ruff check .`,
# so py/uv.lock genuinely can be stale in a working tree — without the split, that reds this gate
# and a contributor "fixes" it by re-locking.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
# CWD is pinned: `--config` resolves relative to CWD and ruff resolves src/exclude relative to the
# config's directory, so an unpinned CWD gives different answers from different directories.
cd "$REPO_ROOT"

CONFIG="py/pyproject.toml"
CORPUS_FLOOR=10

die_infra()  { printf 'ruff-ci: %s\n' "$*" >&2; exit 2; }
die_assert() { printf 'ruff-ci: %s\n' "$*" >&2; exit 1; }

# Corpus derivation. Structural equality with what ruff inspects, because the list IS what ruff
# is given — rev 1 of the spec asserted the two matched after the fact, which could drift.
#
# ':(glob)' IS REQUIRED. Without it git matches without FNM_PATHNAME, so `**` is two `*`s and the
# literal `/` still has to be there: 'ci/**/*.py' matches ci/pyo3-stub/check.py but NOT a
# top-level ci/foo.py — which moon's own matcher, and this gate's declared input, WOULD schedule.
# Measured on a temporary ci/_probe.py. The second pathspec is not redundant with the first.
ruff_corpus() {
  local root="${1:-$REPO_ROOT}"
  git -C "$root" ls-files -- ':(glob)ci/**/*.py' 'ci/*.py' | sort
}

resolve_ruff() {
  local p
  p="$(uv run --locked --project py python3 -c \
    'import shutil, sys; p = shutil.which("ruff"); sys.exit(1) if not p else print(p)')" \
    || die_infra "could not resolve ruff via 'uv run --locked --project py' — run 'uv sync --project py'"
  [ -x "$p" ] || die_infra "resolved ruff is not executable: $p"
  printf '%s' "$p"
}

run_check() {
  local root="${1:-$REPO_ROOT}" ruff rc=0
  local -a files
  mapfile -t files < <(ruff_corpus "$root")
  # The floor is what stops a moved directory silently emptying the gate — the SMA-553 class,
  # which repo:input-liveness cannot reach here (task_inputs.py only proves DECLARED inputs live).
  [ "${#files[@]}" -ge "$CORPUS_FLOOR" ] \
    || die_assert "corpus collapsed to ${#files[@]} files (floor $CORPUS_FLOOR) — did ci/ move?"
  ruff="$(resolve_ruff)"
  "$ruff" check --config "$CONFIG" -- "${files[@]}" || rc=$?
  case "$rc" in
    0) printf 'ruff-ci: %d files clean\n' "${#files[@]}" ;;
    1) exit 1 ;;
    *) die_infra "ruff exited $rc — not a lint verdict" ;;
  esac
}

self_test() {
  local failures=0 tmp
  tmp="$(mktemp -d)"
  git -C "$tmp" init -q
  mkdir -p "$tmp/ci/sub" "$tmp/ci/sub/.venv"
  : >"$tmp/ci/top.py"; : >"$tmp/ci/sub/nested.py"
  : >"$tmp/ci/sub/notes.md"; : >"$tmp/ci/sub/.venv/vendored.py"
  git -C "$tmp" add -A -f >/dev/null 2>&1

  _row() { # $1 label, $2 expected-present (0/1), $3 path
    local label="$1" want="$2" path="$3" got=0
    ruff_corpus "$tmp" | grep -qx "$path" || got=1
    if [ "$got" != "$want" ]; then
      printf '  FAIL %s: %s (want present=%s)\n' "$label" "$path" "$want" >&2
      failures=$((failures + 1))
    fi
  }
  # THE regression this table exists for: the pathspec trap above. A top-level ci/foo.py is
  # exactly the file 'ci/**/*.py' without :(glob) silently drops.
  _row 'top-level .py is found'   0 'ci/top.py'
  _row 'nested .py is found'      0 'ci/sub/nested.py'
  _row 'non-.py is not found'     1 'ci/sub/notes.md'
  _row 'a .venv tree is excluded' 1 'ci/sub/.venv/vendored.py'

  # The floor must trip on an empty corpus, or a moved ci/ passes vacuously.
  local empty rc=0
  empty="$(mktemp -d)"; git -C "$empty" init -q
  ( cd "$empty" && REPO_ROOT="$empty" bash "$REPO_ROOT/ci/ruff/run.sh" ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" != 1 ]; then
    printf '  FAIL empty corpus: expected rc 1 from the floor, got %s\n' "$rc" >&2
    failures=$((failures + 1))
  fi
  rm -rf "$tmp" "$empty"

  if [ "$failures" -gt 0 ]; then
    printf 'ruff-ci self-test: %d row(s) failed\n' "$failures" >&2
    exit 1
  fi
  printf '== ruff-ci self-test passed ==\n'
}

negative_control() {
  # Runs against a COPY OF THE REAL TREE INSIDE the worktree, not a bare `mktemp -d`: outside a
  # git repo `git ls-files` returns nothing and ruff's exclusion handling differs, so a tempdir
  # control would exercise a different code path than the real run and prove nothing about it.
  local tmp rc=0
  tmp="$(mktemp -d "$REPO_ROOT/.ruff-negctl-XXXXXX")"
  trap 'rm -rf "$tmp"' RETURN
  git init -q "$tmp"
  mkdir -p "$tmp/ci/probe"
  cp -R "$REPO_ROOT/ci/." "$tmp/ci/" 2>/dev/null || true
  printf 'x = [1]\ny = x + [2]\n' >"$tmp/ci/probe/violation.py"
  git -C "$tmp" add -A -f >/dev/null 2>&1
  ( cd "$tmp" && REPO_ROOT="$tmp" bash "$REPO_ROOT/ci/ruff/run.sh" ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" != 1 ]; then
    printf '  FAIL a planted RUF005 did not red the gate: expected rc 1, got %s\n' "$rc" >&2
    exit 1
  fi
  printf '== ruff-ci negative control passed ==\n'
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
  selftest) self_test ;;
  check)    run_check ;;
  negctl)   negative_control ;;
esac
```

- [ ] **Step 2: Make it executable and shellcheck-clean**

```bash
chmod +x ci/ruff/run.sh
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
SC="$(uv run --locked --project py python3 -c 'import shutil;print(shutil.which("shellcheck"))')"
"$SC" ci/ruff/run.sh
```
Expected: no output. Task 2 put shellcheck in the lock, so this is available; fix anything it reports before continuing.

- [ ] **Step 3: Run all three modes**

```bash
bash ci/ruff/run.sh --self-test;        echo "rc=$?"   # expect 0
bash ci/ruff/run.sh --negative-control; echo "rc=$?"   # expect 0
bash ci/ruff/run.sh;                    echo "rc=$?"   # expect 0, "10 files clean"
```

If `run_check`'s `REPO_ROOT` override does not take effect inside the self-test/control subshells (the script recomputes it from `BASH_SOURCE`), change those subshells to invoke a copy of the script inside the temp tree instead, and re-verify all three modes. Do not paper over it by weakening an assertion.

- [ ] **Step 4: M3 — prove a real violation reds**

```bash
printf 'x = [1]\ny = x + [2]\n' > ci/affected-graph/_probe.py
git add ci/affected-graph/_probe.py
bash ci/ruff/run.sh; echo "rc=$? (expect 1, RUF005)"
git rm -qf --cached ci/affected-graph/_probe.py && rm -f ci/affected-graph/_probe.py
```

- [ ] **Step 5: M4 — prove a uv failure is rc 2, not rc 1**

Temporarily change `resolve_ruff`'s `--project py` to `--project ci/release-plan` (a venv with no ruff), run `bash ci/ruff/run.sh`, and confirm **rc 2** with an infra message rather than rc 1. Revert. This is the exit-code-disambiguation proof and is the reason the two-step design exists.

- [ ] **Step 6: Write `ci/ruff/README.md`**

Cover: what the gate asserts; the exit-code contract and why rc 1 and rc 2 differ; the `:(glob)` pathspec fact with the measurement; why the corpus is derived rather than asserted; why the negative control runs inside the worktree; and the AC B7 decision — `ruff format` is **not** gated, with the measured 2,998-rewritten-lines figure and the "60% comment, hand-wrapped by design" reason. State plainly that this is a decision not to gate formatting, not a claim the corpus is well formatted. Add the L9 isort note (`src` defaults to `py/`, so a future sibling import would be misclassified).

- [ ] **Step 7: Commit**

```bash
git add ci/ruff/
git commit -m "ci(repo): add the ruff gate script for ci/**/*.py (SMA-539)"
```

---

### Task 4: Wire `repo:ruff-ci` into Moon and CI

**Files:**
- Modify: `moon.yml` (new task; `ci/ruff/**/*` on `repo:affected-smoke`)
- Modify: `.github/workflows/ci.yml:234` (`T=(…)`)
- Modify: `CLAUDE.md` (marker-delimited command)

**Interfaces:**
- Consumes: `bash ci/ruff/run.sh` from Task 3.
- Produces: the four `moon.yml` script lines Task 5 pins in `SELF_SCHEDULED_GATES`.

- [ ] **Step 1: Add the task to `moon.yml`**

Place it after `pyo3-stub-drift`, matching that block's comment discipline. `inputs` are written glob-sorted then file-sorted, the order `check_gate_inputs` compares in:

```yaml
  ruff-ci:
    description: 'Lint ci/**/*.py against py/pyproject.toml''s Ruff rule set (SMA-539).'
    # WHY THIS EXISTS — .moon/tasks/python.yml runs `uv run ruff check .` scoped to the py
    # project, and ci/**/*.py is outside it. Nothing else lints it. ci_targets.py merged through
    # a full review carrying three RUF005 violations (SMA-541), found only because a CodeRabbit
    # learning mentioned the convention in passing.
    #
    # WHY --project py AND NOT A DEDICATED uv PROJECT — the ci/workflow-credentials precedent
    # avoids `--project py` because it compiles a PyO3 cdylib. That premise does not hold here:
    # repo:actionlint's `inputs: ['**/*']` strictly supersets this task's, so this gate can never
    # be a cache miss while repo:actionlint is a hit, and the py environment is materialised once
    # per CI run by whichever reaches it first. One lockfile means ONE ruff version, so py:lint
    # and this gate cannot silently diverge — which is half of AC B5.
    #
    # `--self-test` and `--negative-control` run FIRST and in the SAME block: a gate that cannot
    # report red is worse than no gate. `set -euo pipefail` is REQUIRED — Moon does not enable
    # errexit for `script:` blocks and takes the block's status from its LAST command, so without
    # it a failing control is masked by the passing real run. These four lines are pinned by
    # SELF_SCHEDULED_GATES.
    script: |
      set -euo pipefail
      bash ci/ruff/run.sh --self-test
      bash ci/ruff/run.sh --negative-control
      bash ci/ruff/run.sh
    toolchain: 'system'
    inputs:
      # .moon/tasks/python.yml is listed so a change to HOW py:lint invokes ruff re-keys this
      # gate: AC B5 is one rule set AND one tool version, and that file is where they diverge.
      - '.moon/tasks/python.yml'
      - '.prototools'
      - 'ci/**/*.py'
      - 'ci/ruff/**/*'
      - 'py/pyproject.toml'
      - 'py/uv.lock'
```

Note `'ci/**/*.py'` formally reaches under `**/.venv/**`, which `.moon/workspace.yml:41-43` asks tasks not to declare. Benign — the hasher's `ignorePatterns` filters those trees and ruff's default `exclude` skips them — and worth a one-line comment so the next reader need not re-derive it.

- [ ] **Step 2: Add `ci/ruff/**/*` to `repo:affected-smoke`'s inputs**

Alongside the `ci/release-plan/**/*` entry near `moon.yml:212`, with the same reasoning the three predecessors carry: Task 6 pins lines inside `ci/ruff/run.sh`, so a change under `ci/ruff/` must re-key `repo:affected-smoke` or the pin is real but unreachable. `ci/**/*` already covers it, but the narrow globs are kept rather than collapsed — check 8e floors the array at `-ge 20` against 23 entries and the headroom is deliberate (`moon.yml:200-204`).

- [ ] **Step 3: Add `:ruff-ci` to `ci.yml`'s `T=(…)`**

Append to the single-line array at `.github/workflows/ci.yml:234`. It must stay one line (SMA-541).

- [ ] **Step 4: Add `:ruff-ci` to CLAUDE.md's marker-delimited command**

Between `<!-- ci-targets:begin -->` and `<!-- ci-targets:end -->`, in the same position as in `T`. Do not add a second copy of either marker anywhere in the file, even inside backticks — that makes the count 2 and reds the gate.

- [ ] **Step 5: Verify the task runs**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:ruff-ci --force
```
Expected: pass, with the self-test and control lines in the output.

- [ ] **Step 6: Commit**

```bash
git add moon.yml .github/workflows/ci.yml CLAUDE.md
git commit -m "ci(repo): schedule repo:ruff-ci in moon and the CI target array (SMA-539)"
```

---

### Task 5: Registry obligations 3, 4 and 6

**Files:**
- Modify: `ci/affected-graph/ci_targets.py` (`SELF_SCHEDULED_GATES`, `SELF_TASK_EXPECTED_GLOBS`, `REQUIRED_REPO_TASKS`)
- Modify: `ci/actionlint/run.sh` (`T_AFFECTED_SMOKE_REQUIRED_INPUTS` near `:2123`)

- [ ] **Step 1: Prove the gate is currently unregistered**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:affected-smoke --force; echo "rc=$?"
```
Expected: **FAIL**. Task 4 added a `repo:*` task to `T` with no registry entries, so this must red. Record the message — it names the missing obligations and is the M5 evidence.

- [ ] **Step 2: Add the `SELF_SCHEDULED_GATES` entry**

```python
    # SMA-539. Four lines, like the other self-scheduled gates: `set -euo pipefail` is what makes
    # a failing control propagate, since Moon takes a `script:` block's status from its LAST
    # command.
    "ruff-ci": (
        "set -euo pipefail",
        "bash ci/ruff/run.sh --self-test",
        "bash ci/ruff/run.sh --negative-control",
        "bash ci/ruff/run.sh",
    ),
```

- [ ] **Step 3: Add the `SELF_TASK_EXPECTED_GLOBS` entry**

Globs sorted, then literal files sorted — `check_gate_inputs` compares in that order, not authored order (`ci_targets.py:1408-1423`; the `pyo3-stub-drift` entry's comment at `:287-289` says the same).

```python
    # SMA-539. Two globs then four literals, in check_gate_inputs' comparison order. The two py/
    # entries are the rule set and the ruff version pin; .moon/tasks/python.yml is what makes a
    # divergence between py:lint's invocation and this gate's re-key the gate (AC B5).
    "ruff-ci": (
        "ci/**/*.py",
        "ci/ruff/**/*",
        ".moon/tasks/python.yml",
        ".prototools",
        "py/pyproject.toml",
        "py/uv.lock",
    ),
```

If the check reports a mismatch, read the reported order and re-sort to match rather than reordering `moon.yml`.

- [ ] **Step 4: Add the `REQUIRED_REPO_TASKS` entry**

```python
    # SMA-539. Same reasoning as the release-parity* and workflow-credentials entries: this gate
    # carries a --negative-control, and check_forward's `want`/`got` shrink CONSISTENTLY when a
    # task is dropped from `T` and made CI-ineligible in the same edit — so without a floor entry
    # the whole gate, control included, could be switched off with every check green.
    "ruff-ci",
```

- [ ] **Step 5: Add `ci/ruff/**/*` to `T_AFFECTED_SMOKE_REQUIRED_INPUTS`**

In `ci/actionlint/run.sh` near `:2123`, beside `'ci/release-plan/**/*'`, with a comment naming Task 6's pin as the reason. The array's `-ge 20` floor rises with it.

- [ ] **Step 6: Verify both gates pass**

```bash
moon run repo:affected-smoke --force; echo "rc=$?"
bash ci/actionlint/run.sh; echo "rc=$?"
```
Expected: both rc 0.

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/ci_targets.py ci/actionlint/run.sh
git commit -m "ci(repo): register repo:ruff-ci in the gate registries (SMA-539)"
```

---

### Task 6: Obligation 5 — `RUFF_SH_CALL_SITES`

The largest and most delicate task. `check_self_invocation` gains a **seventh required positional parameter**, which touches **45 call sites** in `self_test()` (26 of the single-line `wired_release_plan)` form, ~19 multi-line).

Why it is not optional: `SELF_SCHEDULED_GATES` pins only the four `moon.yml` lines, leaving the control's *body* in `run.sh` unpinned, and CLAUDE.md records the measured outcome for this exact shape — *"deleting every `_expect` and `grep` row left all four byte-identical and the control exited 0 having asserted nothing (MEASURED)."*

**Files:**
- Modify: `ci/affected-graph/ci_targets.py` (`RUFF_SH_CALL_SITES`; `check_self_invocation` signature at `:1275-1277`, body near `:1358`, docstring at `:1309-1311`; the param-required self-test at `:2226-2232`; 45 call sites in `self_test()`)
- Modify: `ci/affected-graph/run.sh` (supply `ruff_sh_text`)

**Interfaces:**
- Consumes: the five pinned lines produced by Task 3's `ci/ruff/run.sh`.
- Produces: `check_self_invocation(run_sh_text, scripts, actionlint_sh_text, release_parity_sh_text, workflow_credentials_sh_text, release_plan_sh_text, ruff_sh_text)`.

- [ ] **Step 1: Define the pin**

Five discrete lines, matched as **stripped whole lines** (the `case` arms and `if` bodies are indented, so a column-0 rule would reject the real executing lines). Copy each verbatim from the file written in Task 3:

```python
# SMA-539. Five discrete lines rather than one span, for the reason CLAUDE.md records against
# ci/release-parity/run.sh: neutering the flag parse alone lets --negative-control fall through
# to the real suite (which then just runs twice and proves nothing), and gutting the assertion
# body alone leaves a control that prints "passed" having called nothing. The corpus-derivation
# and real-invocation lines are pinned too, because a control that survives while the thing it
# controls has been rewritten is worth nothing.
RUFF_SH_CALL_SITES = (
    "--negative-control) MODE=negctl;   shift ;;",
    "negctl)   negative_control ;;",
    "git -C \"$root\" ls-files -- ':(glob)ci/**/*.py' 'ci/*.py' | sort",
    "\"$ruff\" check --config \"$CONFIG\" -- \"${files[@]}\" || rc=$?",
    "printf '  FAIL a planted RUF005 did not red the gate: expected rc 1, got %s\\n' \"$rc\" >&2",
)
```

Verify each is byte-identical to the file:

```bash
python3 - <<'PY'
import pathlib, sys
sys.path.insert(0, "ci/affected-graph")
from ci_targets import RUFF_SH_CALL_SITES
lines = {l.strip() for l in pathlib.Path("ci/ruff/run.sh").read_text().splitlines()}
missing = [s for s in RUFF_SH_CALL_SITES if s not in lines]
print("MISSING:", missing or "none")
PY
```
Expected: `MISSING: none`. Fix the tuple, not the script, if any differ.

- [ ] **Step 2: Extend the signature, docstring and body**

Signature at `:1275-1277` gains `ruff_sh_text` as the seventh positional parameter. Body, beside the release-plan block near `:1358`:

```python
    # SMA-539 — stripped whole lines, like the release-parity, workflow-credentials and
    # release-plan haystacks and unlike the column-0 actionlint one: these sit indented inside
    # `case` arms and function bodies, so column 0 would reject the real, executing lines.
    ruff_lines = {line.strip() for line in ruff_sh_text.splitlines()}
    missing.extend(
        f"ci/ruff/run.sh: {site}"
        for site in RUFF_SH_CALL_SITES
        if site not in ruff_lines
    )
```

Update the docstring at `:1309-1311` to name seven haystacks and list `ruff_sh_text` among the required parameters.

- [ ] **Step 3: Update the 45 call sites**

Do the mechanical majority first, then the rest by hand:

```bash
python3 - <<'PY'
import pathlib
p = pathlib.Path("ci/affected-graph/ci_targets.py")
t = p.read_text()
n = t.count("wired_release_plan)")
t = t.replace("wired_release_plan)", "wired_release_plan, wired_ruff)")
p.write_text(t)
print("single-line call sites updated:", n)
PY
```

Then find the remainder and edit each by hand — several are argument lists split across lines, and a few pass a *deliberately broken* text in one position (e.g. `_wc_broken`, `_rp_commented`) where the new argument must be the healthy `wired_ruff`:

```bash
grep -n 'check_self_invocation(' ci/affected-graph/ci_targets.py | wc -l
python3 -c "import ast,pathlib; ast.parse(pathlib.Path('ci/affected-graph/ci_targets.py').read_text()); print('parses OK')"
```

Define `wired_ruff` in `self_test()` beside `wired_release_plan` (near `:1972`) as the joined `RUFF_SH_CALL_SITES` lines, mirroring how `wired_release_plan` is built.

- [ ] **Step 4: Add the fixture rows**

Mirror the workflow-credentials rows at `:2286-2330` — one row per pinned line deleted, one asserting a ruff site cannot be satisfied by another file's text, and one asserting a **commented-out** copy does not satisfy the pin. Add `"ruff_sh_text"` to the required-parameter name list at `:2226-2227`.

- [ ] **Step 5: Supply the text from `ci/affected-graph/run.sh`**

Find where the other five texts are read and pass `ci/ruff/run.sh` alongside, in the same style.

- [ ] **Step 6: Verify**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
uv run --project py --locked ruff check --config py/pyproject.toml ci/affected-graph/ci_targets.py
moon run repo:affected-smoke --force; echo "rc=$?"
```
Expected: ruff clean (this file is now in the gated corpus), affected-smoke rc 0.

- [ ] **Step 7: Mutate the pin**

Delete the `_expect`-equivalent line from `ci/ruff/run.sh`'s `negative_control` (the pinned `printf '  FAIL a planted RUF005 …'`), re-run `moon run repo:affected-smoke --force`, and confirm it **reds** naming that site. Restore. Without this the pin might be present but unwired.

- [ ] **Step 8: Commit**

```bash
git add ci/affected-graph/
git commit -m "ci(repo): pin ci/ruff/run.sh's control lines from ci_targets.py (SMA-539)"
```

---

### Task 7: Documentation and the full-graph run

**Files:**
- Modify: `CLAUDE.md` (Gotchas)

- [ ] **Step 1: Add the CLAUDE.md Gotchas entry**

One entry covering: the new gate and its six obligations (naming `REQUIRED_REPO_TASKS` as the one the issue's list of five omits); the `ruff format` decision with the 2,998-line measurement; that `ci/**/*.py` is now gated so a new `ci/` Python file must pass ruff; the `:(glob)` pathspec fact; and the `${{ }}` blind spot in `repo:actionlint`'s new shellcheck coverage, with the measured A/B and a pointer to zizmor as the tool that would cover it.

- [ ] **Step 2: Run the full graph as CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  --base origin/main --include-relations
```
Per-project tasks do not run the repo-level gates, so this is the only way to see what CI sees. Note `py/uv.lock` changed in Task 2, which re-keys `contracts:generate` (it lists `/py/uv.lock`) — expect a codegen run and confirm the drift step finds no diff.

- [ ] **Step 3: Confirm all five mutations were recorded**

M1 and M2 from Task 2, M3 and M4 from Task 3, M5 from Task 5 Step 1, plus Task 6 Step 7's pin mutation. Any not actually performed must be performed now. Do not report a mutation as proven from reasoning.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs(ci): record the repo:ruff-ci gate and the shellcheck coverage limit (SMA-539)"
```

---

## Self-Review

**Spec coverage.** §2.2/§3.2 → Task 2. §3.3 fail-closed → Task 2 Step 6 (M2). §3.4 check-3 fixture → Task 2 Steps 7-8. §4.2 exit codes → Task 3 Steps 1, 5 (M4). §4.3 corpus derivation and the `:(glob)` trap → Task 3 Steps 1, 3 and the self-test table. §4.4 modes and `inputs` → Tasks 3, 4. §5.1 the 27 fixes → Task 1. §5.2 the format decision → Task 3 Step 6 and Task 7 Step 1. §5.3 the interpreter floor → Task 1 Step 7. §6 mutations → Tasks 2, 3, 5, 6 and Task 7 Step 3. §7's six obligations → Task 4 (1, 2), Task 5 (3, 4, 6), Task 6 (5). §8's file table is covered across all seven tasks. §9's limitations are documented in Task 2 Step 9, Task 3 Step 6 and Task 7 Step 1.

**Gap accepted deliberately.** §5.1's `RUFF_PER_FILE_IGNORE_REASONS` table is **not** implemented. It would ship empty with no consumer, and an empty table nothing reads is the "declared but never invoked" rot the repo's own pins exist to catch. The spec's intent — that a future exception has a reasoned home rather than forcing a hack — is served by recording the mechanism in `ci/ruff/README.md`. Task 3 Step 6 covers it. Flag this at review; if rejected, the table plus a consumer in `run_check` is a small addition.

**Placeholder scan.** No TBD/TODO. Every code step carries the actual content. Task 6 Step 3 is the one step that cannot be fully literal — 45 heterogeneous call sites — so it gives the sed for the mechanical 26, the command to enumerate the rest, and an AST parse check as the gate.

**Type consistency.** `ruff_corpus`, `resolve_ruff`, `run_check`, `self_test`, `negative_control`, `die_infra`, `die_assert`, `CORPUS_FLOOR`, `CONFIG` are defined in Task 3 Step 1 and used under those names in Tasks 3 and 6. `RUFF_SH_CALL_SITES` and `wired_ruff`/`ruff_sh_text` are used consistently in Task 6. `SHELLCHECK_BIN` is defined and used in Task 2.

**Known risk.** Task 3 Step 3 flags that the `REPO_ROOT` override may not reach the subshells, since the script recomputes it from `BASH_SOURCE`. The step says to fix it by invoking a copy inside the temp tree rather than weakening the assertion — resolve it there rather than discovering it in Task 6.
