#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# repo:release-plan — decide whether a push to `main` has anything to release, so the release
# workflow's `plan` job can skip its ~15-minute build matrix when it does not. The decision is
# TAG EXISTENCE, not a `release-plz release --dry-run` read: see release_plan.py's module
# docstring for why the dry-run reading is silently, permanently wrong (measurement M6).
#
# Exit codes: 0 pass | 1 the repo is wrong | 2 infrastructure failed — EXCEPT --github-output,
# which always exits 0. See the comment on that arm.
#
# The checker exits 3, not 1, for an assertion failure. `uv` exits 1 on its own failures, so
# without a distinct code a PyPI outage would read as "the repo is wrong". This wrapper owns
# the 3 -> 1 translation and nothing else may.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$REPO_ROOT/ci/release-plan"

die_infra() { printf 'release-plan: %s\n' "$*" >&2; exit 2; }

# Preflight. `uv` absent yields 127 from the shell, which is neither 0/1/2 nor actionable.
command -v uv >/dev/null 2>&1 \
  || die_infra "uv is not on PATH — run 'proto install', or add ~/.proto/shims to PATH"

# $@ is forwarded to the checker. Returns 0, returns 1 for a real assertion failure, and
# EXITS 2 for anything else.
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
  # `tail -n 1` guards against a second, forged verdict line ahead of the genuine one — e.g. a
  # releasable package name containing a newline could make the reason line above emit a
  # literal `nothing_to_release=true`, and both would otherwise match the grep and both would
  # get appended. The genuine verdict from `main()` is always printed LAST, so taking only the
  # final match is safe. This is deliberately unverified-assumption-free: the reviewer could not
  # confirm whether Actions takes the first or the last of two same-named `>>` output keys, and
  # a fail-safe control must not lean on an assumption nobody checked.
  printf '%s\n' "$out" | grep -E '^nothing_to_release=(true|false)$' | tail -n 1 \
    >> "${GITHUB_OUTPUT:-/dev/stdout}"
  exit 0
}

# The wiring rows — only what needs the real tree, plus rows that build their OWN synthetic git
# trees rather than reading the real repository. Rows 3 and 4 used to invoke `--github-output`
# against the LIVE repository and assert the printed verdict flipped with GITHUB_EVENT_NAME.
# That depended on TRANSIENT repository state: it held only while every releasable package's
# manifest version already had a matching tag. On the release PR itself, release-plz has just
# bumped rs/crates/*/Cargo.toml while the new tags do not exist yet — FIXTURES' own "a
# kernel-only bump -> build (M6)" row is exactly this shape, and `decide()` correctly returns
# False for it. So the old row 4 would fail, `--negative-control` would exit 1, and this gate
# would red on precisely the PR it exists to serve — and the same window reopens on `main` in
# the gap between a release merging and its tags landing. Fixed: rows 3 and 4 now build
# throwaway git trees under $tmp and invoke the CHECKER directly — not `github_output`, which
# hardcodes $REPO_ROOT and so cannot be pointed at a synthetic tree — against each, asserting
# BOTH directions. That still proves the decision responds to its input rather than being wired
# to a constant, without depending on what state the real repository happens to be in.
#
# Removing `--github-output` from rows 3/4 left `github_output()` ITSELF — its non-zero /
# malformed-output catch, its `::warning::` annotation, its real `$GITHUB_OUTPUT` append — with
# nothing automated exercising it. Row 5 closes that gap: it runs the real `--github-output`
# mode against the real repository with `$GITHUB_OUTPUT` pointed at a scratch file, and asserts
# only that the wrapper exits 0 and writes EXACTLY ONE matching verdict line — never WHICH
# direction. Asserting a direction there would reintroduce the exact repository-state dependency
# this comment just described removing from rows 3/4; see row 5's own comment for why that must
# stay true even as the fixture table or the real repository's tags change over time.
negative_control() {
  local failures=0 tmp out

  _expect() { # $1 expected rc, $2 label, then the command
    local want="$1" label="$2"; shift 2
    local got=0
    "$@" >/dev/null 2>&1 || got=$?
    if [ "$got" != "$want" ]; then
      printf '  FAIL %s: expected rc %s, got %s\n' "$label" "$want" "$got" >&2
      failures=$((failures + 1))
    fi
  }

  # Builds a throwaway git repo at $1 with one releasable package, "a" at version 1.0.0. The
  # caller adds whatever tag(s) the row needs before reading the checker's verdict.
  # `commit.gpgsign false` and `tag.gpgsign false` are scoped to THIS synthetic repo's LOCAL
  # config only — they never touch the real repository's SSH-signed commits — and exist because
  # the environment's global `commit.gpgsign = true` / `tag.gpgsign = true` would otherwise make
  # a plain `git commit` or lightweight `git tag` here hang, or fail outright (MEASURED: an
  # unguarded `git tag` here errors "fatal: no tag message?", since the global config silently
  # upgrades it to an annotated, signed tag that needs one) on a signer this fixture has no
  # business invoking.
  _build_synthetic_tree() {
    local dir="$1"
    mkdir -p "$dir/rs/crates/libs/a"
    printf '[[package]]\nname = "a"\nrelease = true\n' > "$dir/rs/release-plz.toml"
    printf '[package]\nname = "a"\nversion = "1.0.0"\npublish = true\n' \
      > "$dir/rs/crates/libs/a/Cargo.toml"
    git -C "$dir" init -q
    git -C "$dir" config commit.gpgsign false
    git -C "$dir" config tag.gpgsign false
    git -C "$dir" config user.email "release-plan-negative-control@example.invalid"
    git -C "$dir" config user.name "release-plan negative control"
    git -C "$dir" add -A
    git -C "$dir" commit -q -m "synthetic fixture"
  }

  tmp="$(mktemp -d)"

  # Row 1 — the 3 -> 1 translation itself. A tree with no `rs/` at all cannot resolve any crate
  # manifest, so releasable_packages() raises Inconclusive, --assert exits 3, and run_checker
  # must map that onto the repo contract's 1, not pass the 3 through and not silently mask it
  # as an infra failure.
  mkdir -p "$tmp/empty"
  _expect 1 "the wrapper maps a checker assertion (3) onto the repo contract (1)" \
    run_checker --assert "$tmp/empty"

  # Row 2 — the self-test itself must still be capable of catching a broken fixture table. This
  # is a smoke check that --self-test wiring reaches the real FIXTURES list, not a fixture retest
  # (the table already re-runs on every CI invocation of --self-test).
  _expect 0 "the self-test passes against the real fixture table" \
    run_checker --self-test

  # Row 3 — a synthetic tree where the wanted tag ALREADY exists -> the checker must print
  # nothing_to_release=true. Invoked as a direct `release_plan.py` call (not run_checker, whose
  # 3 -> 1 mapping does not apply to the bare runtime path, and not github_output, which cannot
  # target this tree at all).
  _build_synthetic_tree "$tmp/synthetic-true"
  git -C "$tmp/synthetic-true" tag "a-v1.0.0"
  out="$(uv run --project "$HERE" --python '>=3.12' python3 "$HERE/release_plan.py" \
    --event-name push "$tmp/synthetic-true" 2>&1)" || true
  if ! printf '%s\n' "$out" | grep -q '^nothing_to_release=true$'; then
    printf '  FAIL a synthetic tree with every tag already cut did not print nothing_to_release=true\n' >&2
    printf '  --- output ---\n%s\n' "$out" >&2
    failures=$((failures + 1))
  fi

  # Row 4 — a synthetic tree where a DIFFERENT tag exists but not the wanted one (so this
  # exercises the "tags not yet cut" branch, not the separate "no tags at all" branch FIXTURES
  # already covers) -> the checker must print nothing_to_release=false. Without both this row
  # and row 3, the control cannot tell a working decision from one wired to a constant in
  # either direction.
  _build_synthetic_tree "$tmp/synthetic-false"
  git -C "$tmp/synthetic-false" tag "a-v0.9.0"
  out="$(uv run --project "$HERE" --python '>=3.12' python3 "$HERE/release_plan.py" \
    --event-name push "$tmp/synthetic-false" 2>&1)" || true
  if ! printf '%s\n' "$out" | grep -q '^nothing_to_release=false$'; then
    printf '  FAIL a synthetic tree with a missing tag did not print nothing_to_release=false\n' >&2
    printf '  --- output ---\n%s\n' "$out" >&2
    failures=$((failures + 1))
  fi

  # Row 5 — the direction-agnostic wrapper row. Rows 3 and 4 above prove the DECISION responds
  # to its input; neither one exercises the run.sh `github_output()` WRAPPER itself — its
  # non-zero/malformed-output catch, its `::warning::` annotation, its real `$GITHUB_OUTPUT`
  # append — because both call release_plan.py directly. This row closes that gap by running the
  # real --github-output mode against the REAL repository, with $GITHUB_OUTPUT pointed at a
  # scratch file so nothing leaks into an actual Actions output file. It deliberately asserts
  # NOTHING about WHICH verdict comes back — only that the wrapper exits 0 and writes EXACTLY
  # ONE line matching the verdict pattern. Asserting a direction here would depend on the real
  # repository's tag state again, exactly what C1 removed from this control — DO NOT
  # "strengthen" this row with a true/false assertion; that reintroduces the same failure mode
  # that used to red this gate on the release PR. The line-COUNT assertion is a second,
  # independent guard against M2's forged-second-line failure mode reappearing — it is the
  # reason this row asserts more than the regex alone.
  local gh_out_tmp rc line_count
  gh_out_tmp="$(mktemp)"
  rc=0
  GITHUB_OUTPUT="$gh_out_tmp" bash "$0" --github-output >/dev/null 2>&1 || rc=$?
  if [ "$rc" -ne 0 ]; then
    printf '  FAIL the --github-output wrapper exited %s against the real repo, expected 0\n' \
      "$rc" >&2
    failures=$((failures + 1))
  fi
  line_count="$(grep -cE '^nothing_to_release=(true|false)$' "$gh_out_tmp" || true)"
  if [ "$line_count" != "1" ]; then
    printf '  FAIL GITHUB_OUTPUT held %s matching verdict line(s), expected exactly 1\n' \
      "$line_count" >&2
    printf '  --- %s contents ---\n' "$gh_out_tmp" >&2
    cat "$gh_out_tmp" >&2
    failures=$((failures + 1))
  fi
  rm -f "$gh_out_tmp"

  rm -rf "$tmp"
  if [ "$failures" -gt 0 ]; then
    printf 'release-plan negative control: %d row(s) failed\n' "$failures" >&2
    exit 1
  fi
  printf '== release-plan negative control passed ==\n'
}

MODE=
while [ $# -gt 0 ]; do
  case "$1" in
    --github-output)     MODE=output; shift ;;
    --self-test)         MODE=selftest; shift ;;
    --negative-control)  MODE=negctl; shift ;;
    --assert)            MODE=assert; shift ;;
    *) die_infra "unknown flag: $1" ;;
  esac
done

case "$MODE" in
  output)   github_output ;;
  selftest) run_checker --self-test ;;
  negctl)   negative_control ;;
  assert)   run_checker --assert "$REPO_ROOT" ;;
  *)        die_infra "one mode is required: --github-output | --self-test | --negative-control | --assert" ;;
esac
