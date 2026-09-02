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

# REPO_ROOT is computed unconditionally from BASH_SOURCE, with no env override — matching every
# other ci/*/run.sh in this repo. An override here would let `REPO_ROOT=<anywhere> bash
# ci/ruff/run.sh` — no flag, the ordinary check path — silently lint a stale tree and report a
# clean pass at rc 0, which is exactly the failure this gate exists to prevent. self_test and
# negative_control below instead point a COPY of this file at their fixture directories, so
# BASH_SOURCE resolves there naturally.
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
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
# Measured, all combinations: 'ci/*.py' ALONE (no magic) already matches every depth — git's `*`
# spans `/` without FNM_PATHNAME turned on — and ':(glob)ci/**/*.py' ALONE also matches every
# depth, since `**/` matches zero directories too. The ONLY broken form is 'ci/**/*.py' with NO
# magic: the literal `/` after `ci` has nothing to match once there is no further `/` before the
# last path component, so it misses a top-level ci/foo.py — which moon's own matcher, and this
# gate's declared input, WOULD schedule. The two pathspecs below are therefore mutually
# redundant; both are kept anyway because the explicit ':(glob)ci/**/*.py' form documents the
# nested-file intent that 'ci/*.py' alone does not make obvious.
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
  # This row does NOT catch a dropped ':(glob)' — either pathspec alone already covers a
  # top-level file (measured, see the comment above ruff_corpus). What it catches is a
  # reduction to the bare 'ci/**/*.py' with no magic at all — the likeliest simplification of
  # the two-pathspec line above — which is the one form that drops a top-level ci/foo.py.
  _row 'top-level .py is found'   0 'ci/top.py'
  _row 'nested .py is found'      0 'ci/sub/nested.py'
  _row 'non-.py is not found'     1 'ci/sub/notes.md'
  _row 'a .venv tree is excluded' 1 'ci/sub/.venv/vendored.py'

  # The floor must trip on an empty corpus, or a moved ci/ passes vacuously. There is no
  # REPO_ROOT override to lean on (see the REPO_ROOT assignment above), so a COPY of this
  # script is placed inside the empty fixture at the same ci/ruff/run.sh relative path — its own
  # BASH_SOURCE-based computation then resolves REPO_ROOT to $empty naturally.
  local empty rc=0
  empty="$(mktemp -d)"; git -C "$empty" init -q
  mkdir -p "$empty/ci/ruff"
  cp "$REPO_ROOT/ci/ruff/run.sh" "$empty/ci/ruff/run.sh"
  ( cd "$empty" && bash "$empty/ci/ruff/run.sh" ) >/dev/null 2>&1 || rc=$?
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
  # Uses a bare `mktemp -d`, OUTSIDE the working tree — not nested under $REPO_ROOT. This
  # fixture is `git init`-ed itself, and `py`/`rs` below are pulled in by ABSOLUTE symlink, so
  # its location changes nothing about what `git ls-files` or ruff sees. Nesting it inside
  # REPO_ROOT instead would only expose it to every concurrent 'inputs: [**/*]' repo:* task
  # (repo:actionlint, repo:input-liveness) that hash-walks the whole tree during `moon ci` —
  # `.moon/workspace.yml`'s hasher.ignorePatterns is a fixed short list, NOT .gitignore-aware, so
  # it would not skip a transient in-tree directory — and a SIGKILL before the EXIT trap below
  # fires would leave cruft in the real repo instead of in $TMPDIR.
  #
  # `py/` and `rs/` are SYMLINKED in rather than copied: resolve_ruff and CONFIG both resolve
  # `py` relative to CWD once the copied script (below) recomputes REPO_ROOT as $tmp, and the
  # control needs the real venv/lock rather than a duplicate one built on every invocation. `rs/`
  # has to come along too — py/packages/paigasus-kernel's `path = "../../../rs/..."` source
  # dependency is resolved textually against the symlink's location, not its target, so without
  # a sibling `rs/` uv fails with "Distribution not found" (measured). The root .gitignore is
  # copied in (not the untracked .venv trees under ci/release-plan and ci/workflow-credentials)
  # so a plain `git add` excludes them exactly as the real repo does — force-adding them would
  # pull vendored third-party code into the corpus this control lints, which is not what it
  # exists to prove.
  local rc=0
  tmp="$(mktemp -d)"
  # EXIT, not RETURN: the failure branch below calls `exit 1` directly, which terminates the
  # process without ever returning from this function — a RETURN trap would silently skip
  # cleanup on exactly the path that most needs it (measured: it left a stray dir on disk).
  trap 'rm -rf "$tmp"' EXIT
  git init -q "$tmp"
  cp "$REPO_ROOT/.gitignore" "$tmp/.gitignore"
  # .prototools is copied in too: proto (behind the uv shim) resolves its pinned tool version by
  # walking UP from CWD looking for this file, and once the fixture lives outside REPO_ROOT
  # there is nothing above it to find — measured as `proto::detect::failed` without this copy.
  cp "$REPO_ROOT/.prototools" "$tmp/.prototools"
  mkdir -p "$tmp/ci/probe"
  cp -R "$REPO_ROOT/ci/." "$tmp/ci/" 2>/dev/null || true
  ln -s "$REPO_ROOT/py" "$tmp/py"
  ln -s "$REPO_ROOT/rs" "$tmp/rs"
  printf 'x = [1]\ny = x + [2]\n' >"$tmp/ci/probe/violation.py"
  git -C "$tmp" add -A >/dev/null 2>&1
  # No REPO_ROOT override: the cp -R above already placed a copy of this script at
  # $tmp/ci/ruff/run.sh, and invoking THAT copy makes its own BASH_SOURCE resolve REPO_ROOT to
  # $tmp naturally.
  ( cd "$tmp" && bash "$tmp/ci/ruff/run.sh" ) >/dev/null 2>&1 || rc=$?
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
