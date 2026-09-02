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

# REPO_ROOT honours a pre-set override (used by self_test/negative_control below to point the
# whole script at a throwaway tree) and otherwise computes itself from BASH_SOURCE as usual. A
# plain recomputation here would silently ignore the override those two callers rely on and let
# their subshells exercise the REAL repo while appearing to test a fixture.
REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
# CWD is pinned: `--config` resolves relative to CWD and ruff resolves src/exclude relative to the
# config's directory, so an unpinned CWD gives different answers from different directories.
cd "$REPO_ROOT"

CONFIG="py/pyproject.toml"
CORPUS_FLOOR=10
# Global, not function-local: negative_control's EXIT trap fires after the function that sets
# this has already returned (including via the `exit 1` on its own failure path), and a `local`
# binding is gone by then — referencing it under `set -u` would itself be an unbound-variable
# error (measured).
tmp=""

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
  # .venv is left UNTRACKED (gitignored), not force-added: ruff_corpus excludes it by tracking,
  # the same mechanism the real repo's own untracked ci/*/.venv trees rely on — not by any
  # pattern of its own. `-f` here would defeat the very row it is meant to prove.
  printf '.venv/\n' >"$tmp/.gitignore"
  git -C "$tmp" add -A >/dev/null 2>&1

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

  # The floor must trip on an empty corpus, or a moved ci/ passes vacuously. REPO_ROOT is
  # overridden and the script honours it (see the REPO_ROOT assignment above), so this actually
  # runs run_check against the empty tree rather than the real repo.
  local empty rc=0 script="$REPO_ROOT/ci/ruff/run.sh"
  empty="$(mktemp -d)"; git -C "$empty" init -q
  ( cd "$empty" && REPO_ROOT="$empty" bash "$script" ) >/dev/null 2>&1 || rc=$?
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
  #
  # `py/` and `rs/` are SYMLINKED in rather than copied: resolve_ruff and CONFIG both resolve
  # `py` relative to CWD once REPO_ROOT is overridden below, and the control needs the real
  # venv/lock rather than a duplicate one built on every invocation. `rs/` has to come along too
  # — py/packages/paigasus-kernel's `path = "../../../rs/..."` source dependency is resolved
  # textually against the symlink's location, not its target, so without a sibling `rs/` uv
  # fails with "Distribution not found" (measured). The root .gitignore is copied in (not the
  # untracked .venv trees under ci/release-plan and ci/workflow-credentials) so a plain `git add`
  # excludes them exactly as the real repo does — force-adding them would pull vendored
  # third-party code into the corpus this control lints, which is not what it exists to prove.
  local rc=0 script="$REPO_ROOT/ci/ruff/run.sh"
  tmp="$(mktemp -d "$REPO_ROOT/.ruff-negctl-XXXXXX")"
  # EXIT, not RETURN: the failure branch below calls `exit 1` directly, which terminates the
  # process without ever returning from this function — a RETURN trap would silently skip
  # cleanup on exactly the path that most needs it (measured: it left a stray dir on disk).
  trap 'rm -rf "$tmp"' EXIT
  git init -q "$tmp"
  cp "$REPO_ROOT/.gitignore" "$tmp/.gitignore"
  mkdir -p "$tmp/ci/probe"
  cp -R "$REPO_ROOT/ci/." "$tmp/ci/" 2>/dev/null || true
  ln -s "$REPO_ROOT/py" "$tmp/py"
  ln -s "$REPO_ROOT/rs" "$tmp/rs"
  printf 'x = [1]\ny = x + [2]\n' >"$tmp/ci/probe/violation.py"
  git -C "$tmp" add -A >/dev/null 2>&1
  ( cd "$tmp" && REPO_ROOT="$tmp" bash "$script" ) >/dev/null 2>&1 || rc=$?
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
