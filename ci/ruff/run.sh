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

# proto prints an NDJSON preamble on STDOUT when it detects an agent environment (AI_AGENT /
# CLAUDECODE / CLAUDE_CODE_ENTRYPOINT), which poisons every `$(...)` capture in this file.
# MEASURED on the uv SHIM: default reporter yields `{"type":"message",...}` on stdout, and
# PROTO_REPORTER=text yields none. CLAUDE.md's NDJSON entry had carved shims out as "not proven
# generally"; a captured shim call leaking the preamble is the counterexample that closes it.
# Exported once here rather than prefixed per call site, so a future capture inherits it (SMA-609).
export PROTO_REPORTER=text

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

# PROVENANCE, NOT JUST PRESENCE. `shutil.which` is PATH-based: if the `py` uv project does not
# actually contain ruff (a `[dependency-groups]` rename, a `[tool.uv] default-groups` change, or
# simply `UV_NO_DEV=1` at invocation time — all real, documented uv knobs, none of which trips
# `uv run`'s own exit status), `uv run` still exits 0 and `which` silently returns whatever `ruff`
# is first on the OUTER host PATH. `[ -x ]` then passes on that impostor, and the gate lints with
# a binary this repo never pinned — exactly the "strictness is a property of the host" failure
# SMA-525 refused, now green. MEASURED: with ruff removed from py/.venv/bin and a host impostor
# earlier on PATH, the bare `shutil.which` resolver printed the impostor's path at exit 0.
# `sys.prefix` under `uv run --project py` IS `py/.venv` (measured), so requiring the resolved
# path to live under it is a same-process check that no amount of outer-PATH manipulation can
# spoof — a host binary now routes to `die_infra` (rc 2) instead of a silent pass.
# Containment is COMPONENT-WISE (`is_relative_to` on resolved paths), not a string prefix.
# `p.startswith(sys.prefix)` accepts a SIBLING directory — `py/.venv-host/bin/ruff` starts with
# `py/.venv` and is outside it (MEASURED: startswith True, is_relative_to False). Resolving both
# sides also means a symlinked venv compares by its real location rather than by spelling.
# PROTO_REPORTER=text AND `tail -n1`, deliberately both. proto prints an NDJSON preamble on
# STDOUT when it detects an agent environment (AI_AGENT / CLAUDECODE / CLAUDE_CODE_ENTRYPOINT),
# and MEASURED on merged main: the proto-managed `uv` SHIM leaked that preamble into this very
# capture, so the variable held a JSON blob and the gate died with "not executable: {...}".
# CLAUDE.md's NDJSON entry had carved shims out as "not proven generally" — this is the
# counterexample that closes the carve-out (SMA-609). The env var suppresses it at source; the
# `tail -n1` survives a future reporter change, since the path is always the LAST line. Neither
# weakens the `[ -x ]` below: a preamble-only capture still fails it and still aborts.
resolve_ruff() {
  local p
  p="$(uv run --locked --project py python3 -c \
    'import pathlib, shutil, sys; p = shutil.which("ruff"); sys.exit(1) if not p or not pathlib.Path(p).resolve().is_relative_to(pathlib.Path(sys.prefix).resolve()) else print(p)' | tail -n1)" \
    || die_infra "could not resolve ruff via 'uv run --locked --project py' — run 'uv sync --project py'"
  [ -x "$p" ] || die_infra "resolved ruff is not executable: $p"
  printf '%s' "$p"
}

run_check() {
  local root="${1:-$REPO_ROOT}" ruff rc=0
  local -a files
  # `mapfile ... < <(ruff_corpus "$root")` (the prior form) reads from a process substitution,
  # whose exit status bash discards — a `git ls-files` failure inside ruff_corpus would silently
  # yield zero lines and fall straight into the floor check below, reporting "the repo is wrong"
  # (rc 1) for what is actually an infrastructure failure, violating the exit-code contract at
  # the top of this file. Route through a plain command substitution instead, so the `||` below
  # can see ruff_corpus's real exit status (ruff_corpus's own `| sort` runs under this script's
  # `set -o pipefail`, so a `git` failure already propagates through the pipe).
  local corpus grc=0
  corpus="$(ruff_corpus "$root")" || grc=$?
  [ "$grc" -eq 0 ] || die_infra "git ls-files failed while deriving the ci/**/*.py corpus (rc $grc)"
  # An empty corpus is a legitimate (if floor-tripping) result, not an error — `mapfile <<<
  # "$corpus"` on an empty string would otherwise produce one bogus empty-string element instead
  # of a zero-length array, undercounting the floor message below by one.
  if [ -n "$corpus" ]; then
    mapfile -t files <<< "$corpus"
  fi
  # The floor is what stops a moved directory silently emptying the gate — the SMA-553 class,
  # which repo:input-liveness cannot reach here (task_inputs.py only proves DECLARED inputs live).
  # Deleting a single ci/**/*.py file is a legitimate change that can trip this too, so the
  # message names the re-baseline action rather than only describing the symptom.
  [ "${#files[@]}" -ge "$CORPUS_FLOOR" ] \
    || die_assert "corpus collapsed to ${#files[@]} files (floor $CORPUS_FLOOR) — did ci/ move? If \
this is a legitimate shrink (e.g. a file was deleted on purpose), lower CORPUS_FLOOR in \
ci/ruff/run.sh to match the new corpus size instead of raising it back later."
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
  # uv is resolved EXACTLY ONCE here, from the REAL repo root, via the same resolve_ruff() the
  # real check uses — never from inside the fixture. The prior shape re-entered a COPY of this
  # script inside the fixture, which recomputed REPO_ROOT as the fixture dir and called
  # resolve_ruff() there too, so `uv run --locked --project py` ran a second time with `py`
  # reached through an ABSOLUTE SYMLINK from a foreign root. That mutated the ONE SHARED
  # `py/.venv` every other py Moon task depends on (uv treats the symlink's target, not its
  # location, as the project root, so it still resolved to the real venv and reinstalled into
  # it) — invisible in a lone run of this gate, but under a concurrent `moon ci` it raced and
  # broke `contracts:generate`, `py:typecheck`, `py:test`, `paigasus-kernel-py:test` and
  # `repo:release-parity-py` (measured in CI, PR 206). Resolving ruff once, up front, in the real
  # repo removes every uv call from the fixture, so there is nothing left inside it to race.
  # Since no code under the fixture is ever executed, `py/`, `rs/` and `.prototools` are no
  # longer needed there either.
  local ruff
  ruff="$(resolve_ruff)"

  # negctl_rc, not rc: self_test() above ALSO uses `rc` for its own empty-corpus check, and
  # `if [ "$rc" != 1 ]; then` is therefore not a line unique to this function — a haystack that
  # pinned it would be satisfied by self_test()'s copy even with THIS guard neutered. The distinct
  # name is what makes the guard uniquely pinnable at all, exactly as ci/release-plan/run.sh's
  # `mut_rc` (vs. its own unrelated `rc` uses) does for the same reason.
  local negctl_rc=0
  # Uses a bare `mktemp -d`, OUTSIDE the working tree — not nested under $REPO_ROOT, so it is
  # invisible to every concurrent 'inputs: [**/*]' repo:* task (repo:actionlint,
  # repo:input-liveness) that hash-walks the whole tree during `moon ci`, and a SIGKILL before
  # the EXIT trap below fires leaves cruft in $TMPDIR rather than in the real repo.
  tmp="$(mktemp -d)"
  # EXIT, not RETURN: the failure branch below calls `exit 1` directly, which terminates the
  # process without ever returning from this function — a RETURN trap would silently skip
  # cleanup on exactly the path that most needs it (measured: it left a stray dir on disk).
  trap 'rm -rf "$tmp"' EXIT
  git init -q "$tmp"
  # The root .gitignore is copied in so a plain `git add` would exclude a vendored .venv tree
  # exactly as the real repo does, if one ever showed up here — defensive, since this fixture no
  # longer contains anything but the planted violation below.
  cp "$REPO_ROOT/.gitignore" "$tmp/.gitignore"
  mkdir -p "$tmp/ci/probe"
  printf 'x = [1]\ny = x + [2]\n' >"$tmp/ci/probe/violation.py"
  git -C "$tmp" add -A >/dev/null 2>&1

  # Corpus derivation is pure `git -C` (ruff_corpus), no uv anywhere in this path. The resolved
  # ruff binary is then invoked DIRECTLY against those files, with --config pointing at the REAL
  # py/pyproject.toml by absolute path — not the fixture's (nonexistent) copy — so the rule set
  # under test is the one the real check actually uses. ruff_corpus returns paths relative to
  # $tmp (git -C's own convention), so the invocation below runs WITH CWD=$tmp — without that,
  # ruff resolves "ci/probe/violation.py" against $REPO_ROOT, finds nothing, and reports E902
  # (file not found) at rc 1 — the SAME exit code a real RUF005 finding produces, so a control
  # that skipped this cd would still print "passed" without ever having linted the file
  # (measured: it did, silently, before this comment was written).
  local corpus
  corpus="$(ruff_corpus "$tmp")"
  local -a files=()
  if [ -n "$corpus" ]; then
    mapfile -t files <<< "$corpus"
  fi
  ( cd "$tmp" && "$ruff" check --config "$REPO_ROOT/$CONFIG" -- "${files[@]}" ) >/dev/null 2>&1 || negctl_rc=$?
  if [ "$negctl_rc" != 1 ]; then
    printf '  FAIL a planted RUF005 did not red the gate: expected rc 1, got %s\n' "$negctl_rc" >&2
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
