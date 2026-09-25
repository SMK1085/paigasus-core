#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# repo:release-plan — decide whether a push to `main` has anything to release, so the release
# workflow's `plan` job can skip its ~15-minute build matrix when it does not. The decision is
# TAG EXISTENCE, not a `release-plz release --dry-run` read: see release_plan.py's module
# docstring for why the dry-run reading is silently, permanently wrong (measurement M6).
#
# Exit codes: 0 pass | 1 the repo is wrong | 2 infrastructure failed — EXCEPT --github-output,
# which exits 0 on every run that can name the chain keys, and 2 only when neither
# `release_plan.py --keys` nor a bash read of ci/images/chains.toml names one. See the comment on
# that arm.
#
# The checker exits 3, not 1, for an assertion failure. `uv` exits 1 on its own failures, so
# without a distinct code a PyPI outage would read as "the repo is wrong". This wrapper owns
# the 3 -> 1 translation and nothing else may.
set -euo pipefail

# proto prints an NDJSON preamble on STDOUT when it detects an agent environment (AI_AGENT /
# CLAUDECODE / CLAUDE_CODE_ENTRYPOINT), which poisons every `$(...)` capture in this file.
# MEASURED on the uv SHIM: default reporter yields `{"type":"message",...}` on stdout, and
# PROTO_REPORTER=text yields none. CLAUDE.md's NDJSON entry had carved shims out as "not proven
# generally"; a captured shim call leaking the preamble is the counterexample that closes it.
# Exported once here rather than prefixed per call site, so a future capture inherits it (SMA-609).
export PROTO_REPORTER=text

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$REPO_ROOT/ci/release-plan"

die_infra() { printf 'release-plan: %s\n' "$*" >&2; exit 2; }

# The `uv` preflight. It is a FUNCTION called from the mode dispatch at the bottom of this file,
# NOT a top-level statement, and that placement is load-bearing (SMA-603 fix wave, C1).
#
# It used to run above the dispatch, so it applied to `--github-output` too. `uv` is a
# proto-managed shim and is absent from a bare `ubuntu-latest` runner, so the RUNTIME arm exited
# 2 and wrote nothing at all. A failed `plan` job SKIPS its dependents — see the comment on
# `github_output` below — so a missing toolchain dropped the entire publish path silently. The
# three CI modes keep the loud exit-2 contract: they are assertions and must fail visibly.
require_uv() {
  command -v uv >/dev/null 2>&1 \
    || die_infra "uv is not on PATH — run 'proto install', or add ~/.proto/shims to PATH"
}

# `--locked` on all six `uv run` calls below (SMA-603 fix round 2,
# ruled in; widened to six as rows 3/4/7/8 of the negative control gained their own direct
# calls): mirrors ci/actionlint/run.sh's release_guard_py() wrapper for check 10, and the
# rationale applies MORE strongly here — these six calls run across EVERY invocation of the four
# CI modes (--self-test, --negative-control, --assert, --github-output), where
# ci/actionlint/run.sh's own uv call is only the --fixture-count bypass path. It is inert today
# because this project is zero-dependency (see pyproject.toml's own comment) and becomes live the
# moment it gains one — without it, any of these six calls could silently re-lock py/uv.lock's
# sibling here as a side effect of deciding whether to release. Verified against the current
# lock: all six exit clean.

# $@ is forwarded to the checker. Returns 0, returns 1 for a real assertion failure, and
# EXITS 2 for anything else.
run_checker() {
  local rc=0
  uv run --locked --project "$HERE" --python '>=3.12' python3 "$HERE/release_plan.py" "$@" || rc=$?
  case "$rc" in
    0) return 0 ;;
    3) return 1 ;;
    *) die_infra "checker exited $rc — uv or the interpreter failed, not an assertion" ;;
  esac
}

# SMA-688. $1 a key list, one key on each line. Returns 0 when the list holds at least one key and
# every line is a key of the shape [a-z][a-z0-9-]*. Both key sources go through this one test:
# the `release_plan.py --keys` answer and the sed read of ci/images/chains.toml. A key is later
# split by `for key in $keys`, so the shape also keeps spaces and glob characters out.
keys_are_valid() {
  [ -n "$1" ] && ! grep -qvE '^[a-z][a-z0-9-]*$' < <(printf '%s\n' "$1")
}

# SMA-688. The fail-safe branch. $1 the rc for the annotation, $2 the key list. It writes
# nothing_to_release=false, and skip_<key>=false and an EMPTY version_<key>= for every key, and
# exits 0.
#
# SMA-658. The image chains read their own outputs, and an unset output makes the chain RUN
# (spec § 4.1). The version is written EMPTY, deliberately, rather than left unwritten: an output
# GitHub never saw and one written as an empty string behave differently in a `release.yml`
# expression, and the chains must not depend on that distinction. An empty version means
# "unknown" here, never a real version: the publish job's label compare fails on it before the
# first registry write.
write_failsafe() {
  local key
  printf '::warning::release-plan could not decide (rc=%s) — building, which is the fail-safe direction\n' "$1"
  printf 'nothing_to_release=false\n' >> "${GITHUB_OUTPUT:-/dev/stdout}"
  for key in $2; do
    printf 'skip_%s=false\nversion_%s=\n' "$key" "$key" >> "${GITHUB_OUTPUT:-/dev/stdout}"
  done
  exit 0
}

# THE RUNTIME ARM, and the one place in this repo where a checker failure must NOT fail its
# caller. A failed `plan` job SKIPS its dependents rather than building them — GitHub applies an
# implicit success() to a job-level `if:` with no status function — so a broken decision that
# exited non-zero would stop the release entirely. Fail-safe here means: write false, warn
# loudly, exit 0, and let the matrix build. The --self-test/--negative-control/--assert modes
# keep the normal contract, and CI runs those.
#
# SMA-688, the chain keys (spec § 4.3). The keys come from `release_plan.py --keys`, which reads
# ci/images/chains.toml with tomllib. When that call fails, prints no key, or prints a line
# outside [a-z][a-z0-9-]* (for example, `uv` is missing on the runner), the arm reads the keys
# from the same file with sed, which needs no toolchain. It then takes the fail-safe branch for
# every key, and does NOT run the decision: a toolchain that cannot name the keys cannot be
# trusted to decide. That keeps the SMA-603 C1 contract. Negative-control rows 6, 10 and 11
# assert it.
#
# ONE EXIT 2. When the sed read also finds no key (chains.toml is missing, unreadable or empty),
# the arm cannot name the chain outputs it must write, and a chain output that nobody writes runs
# that chain with an empty version. So it writes nothing_to_release=false and EXITS 2: the plan
# job fails, and every chain and the kernel release skip until a fix. Rows 12 and 13 assert it.
#
# THIS ARM CALLS NO PREFLIGHT, deliberately. An ABSENT `uv` makes the command substitutions
# below exit 127, which `|| keys_rc=$?` and `|| rc=$?` catch like any other failure. `set -e`
# does not fire on them: each assignment is the left operand of an `||` list.
github_output() {
  local rc=0 out keys keys_rc=0 key failsafe=0
  keys="$(uv run --locked --project "$HERE" --python '>=3.12' python3 "$HERE/release_plan.py" --keys "$REPO_ROOT")" || keys_rc=$?
  if [ "$keys_rc" -ne 0 ] || ! keys_are_valid "$keys"; then
    # The sed pattern admits only a bare `[chain.<key>]` header line, with the key in the same
    # shape that keys_are_valid tests. The test below still runs, so both sources pass one rule.
    # TWIN: CHAIN_HEADER_SED_RE in release_plan.py holds the same pattern, and --assert fails
    # when this read and tomllib find different keys. Change both in the same commit.
    keys="$(sed -n 's/^\[chain\.\([a-z][a-z0-9-]*\)\]$/\1/p' "$REPO_ROOT/ci/images/chains.toml" 2>/dev/null)" || keys=
    if ! keys_are_valid "$keys"; then
      printf '::error::release-plan could not name the chain keys (--keys rc=%s, and ci/images/chains.toml names no key). It writes nothing_to_release=false and fails the plan job: a chain output that nobody writes would run that chain with an empty version.\n' "$keys_rc"
      # SMA-688. This line writes the same text as write_failsafe's own
      # `nothing_to_release=false` line, but in a different form. Each pinned line in
      # RELEASE_PLAN_SH_CALL_SITES must be unique text in this file (see that pin's own
      # comment). Two byte-identical lines could not each be pinned on their own.
      printf 'nothing_to_release=%s\n' false >> "${GITHUB_OUTPUT:-/dev/stdout}"
      exit 2
    fi
    printf '::warning::release-plan --keys gave no usable key list (rc=%s). The keys come from a sed read of ci/images/chains.toml.\n' "$keys_rc"
    write_failsafe "$keys_rc" "$keys"
  fi
  out="$(uv run --locked --project "$HERE" --python '>=3.12' python3 \
    "$HERE/release_plan.py" --event-name "${GITHUB_EVENT_NAME:-}" "$REPO_ROOT" 2>&1)" || rc=$?
  printf '%s\n' "$out"
  # SMA-658 fix round 2, widened by SMA-688 to every key. Under `set -euo pipefail`, each
  # `grep -E ... | tail -n 1` pipeline below exits 1 (aborting this function before its
  # `exit 0`) when its pattern has ZERO matches. So a checker that prints a verdict but omits
  # one chain key — a partial write — must route into the fail-safe branch, which writes every
  # key explicitly. Do not replace a missing-key check with `|| true` on the pipelines: that
  # would let the key go unwritten silently.
  #
  # `for key in $keys` splits on purpose: keys_are_valid admits only [a-z][a-z0-9-]* lines, so
  # a key holds no space and no glob character.
  if [ "$rc" -ne 0 ] || ! grep -qE '^nothing_to_release=(true|false)$' < <(printf '%s\n' "$out"); then
    failsafe=1
  fi
  for key in $keys; do
    if ! grep -qE "^skip_${key}=(true|false)\$" < <(printf '%s\n' "$out") \
      || ! grep -qE "^version_${key}=" < <(printf '%s\n' "$out"); then
      failsafe=1
    fi
  done
  if [ "$failsafe" -ne 0 ]; then
    write_failsafe "$rc" "$keys"
  fi
  # `tail -n 1` guards against a second, forged verdict line ahead of the genuine one — e.g. a
  # releasable package name containing a newline could make the reason line above emit a
  # literal `nothing_to_release=true`, and both would otherwise match the grep and both would
  # get appended. The genuine verdict from `main()` is always printed LAST, so taking only the
  # final match is safe.
  printf '%s\n' "$out" | grep -E '^nothing_to_release=(true|false)$' | tail -n 1 \
    >> "${GITHUB_OUTPUT:-/dev/stdout}"
  # One grep for each key and output, each taken LAST for the same forged-line reason. The `=`
  # after the key is load-bearing: `skip_iam` is a string prefix of `skip_iam-console`.
  for key in $keys; do
    printf '%s\n' "$out" | grep -E "^skip_${key}=(true|false)\$" | tail -n 1 \
      >> "${GITHUB_OUTPUT:-/dev/stdout}"
    printf '%s\n' "$out" | grep -E "^version_${key}=" | tail -n 1 \
      >> "${GITHUB_OUTPUT:-/dev/stdout}"
  done
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
    # The member list is what crate_manifests() globs — it derives the package set from
    # `[workspace] members` rather than a hardcoded `crates/*/*` glob (SMA-603 fix wave, 2e).
    printf '[workspace]\nmembers = ["crates/*/*"]\n' > "$dir/rs/Cargo.toml"
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
  # manifest, so releasable_packages() raises InconclusiveError, --assert exits 3, and run_checker
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
  out="$(uv run --locked --project "$HERE" --python '>=3.12' python3 "$HERE/release_plan.py" \
    --event-name push "$tmp/synthetic-true" 2>&1)" || true
  if ! grep -q '^nothing_to_release=true$' < <(printf '%s\n' "$out"); then
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
  out="$(uv run --locked --project "$HERE" --python '>=3.12' python3 "$HERE/release_plan.py" \
    --event-name push "$tmp/synthetic-false" 2>&1)" || true
  if ! grep -q '^nothing_to_release=false$' < <(printf '%s\n' "$out"); then
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
  local gh_out_tmp rc line_count key
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
  # SMA-688. Every chain output exactly once. `^<key>=` with the `=` is load-bearing: `skip_iam`
  # is a string prefix of `skip_iam-console`, so a pattern without it counts two keys as one.
  for key in skip_iam version_iam skip_gateway version_gateway \
    skip_iam-console version_iam-console skip_gateway-console version_gateway-console; do
    line_count="$(grep -c "^${key}=" "$gh_out_tmp" || true)"
    if [ "$line_count" != "1" ]; then
      printf '  FAIL row 5: GITHUB_OUTPUT held %s line(s) for %s, expected exactly 1\n' \
        "$line_count" "$key" >&2
      failures=$((failures + 1))
    fi
  done
  rm -f "$gh_out_tmp"

  # SMA-688. $1 a row label, $2 an output file. Asserts the fail-safe write of spec § 4.3: exactly
  # nine lines, nothing_to_release=false, and skip_<key>=false and an EMPTY version_<key>= for each
  # of the four registry keys. The exact line count also catches a line for a key that is not in
  # the registry (for example skip_IAM_X) and a second verdict line.
  _expect_failsafe_outputs() {
    local label="$1" file="$2" key n
    n="$(grep -c '' "$file" || true)"
    if [ "$n" != "9" ]; then
      printf '  FAIL %s: GITHUB_OUTPUT held %s line(s), expected the 9 fail-safe lines\n' \
        "$label" "$n" >&2
      printf '  --- %s contents ---\n' "$file" >&2
      cat "$file" >&2
      failures=$((failures + 1))
    fi
    if ! grep -qx 'nothing_to_release=false' "$file"; then
      printf '  FAIL %s: nothing_to_release=false was not written\n' "$label" >&2
      failures=$((failures + 1))
    fi
    for key in iam gateway iam-console gateway-console; do
      if ! grep -qx "skip_${key}=false" "$file" || ! grep -qx "version_${key}=" "$file"; then
        printf '  FAIL %s: the fail-safe pair skip_%s=false and version_%s= was not written\n' \
          "$label" "$key" "$key" >&2
        failures=$((failures + 1))
      fi
    done
  }

  # Row 6 — THE C1 REGRESSION ROW, widened by SMA-688. `--github-output` must still exit 0 and
  # write the fail-safe outputs when `uv` cannot be found at all. The `uv` preflight lived above
  # the mode dispatch until the SMA-603 fix wave, so the runtime arm exited 2 and wrote NOTHING on
  # a runner with no proto toolchain — the `plan` job then failed, and because every consumer
  # carries a status-function-free `if:` (implicit success()), the whole publish path skipped.
  # Reordering this file re-arms that trap in one edit, which is why the row exists.
  #
  # SMA-688: without `uv` the wrapper cannot run `release_plan.py --keys`. It reads the keys from
  # ci/images/chains.toml with sed, and writes all nine outputs fail-safe (spec § 4.3). So the
  # restricted PATH now also holds `sed`, and the row asserts all nine outputs, not the verdict only.
  #
  # A hermetic PATH, not a guessed one. `PATH=/usr/bin:/bin` would be a silent no-op on any host
  # that installs uv system-wide; a directory holding symlinks to exactly the externals this arm
  # needs cannot. The precondition is asserted rather than assumed.
  local nouv_dir nouv_out rc6=0 t tpath
  nouv_dir="$tmp/nouv-path"
  mkdir -p "$nouv_dir"
  for t in bash dirname grep sed tail; do
    tpath="$(command -v "$t" || true)"
    if [ -z "$tpath" ]; then
      printf '  FAIL row 6 cannot build its restricted PATH: %s is not on PATH\n' "$t" >&2
      failures=$((failures + 1))
    else
      ln -s "$tpath" "$nouv_dir/$t"
    fi
  done
  if ( PATH="$nouv_dir"; export PATH; command -v uv >/dev/null 2>&1 ); then
    printf '  FAIL row 6 proves nothing: uv is still reachable under the restricted PATH\n' >&2
    failures=$((failures + 1))
  fi
  nouv_out="$(mktemp)"
  PATH="$nouv_dir" GITHUB_EVENT_NAME=push GITHUB_OUTPUT="$nouv_out" \
    "$nouv_dir/bash" "$0" --github-output >/dev/null 2>&1 || rc6=$?
  if [ "$rc6" -ne 0 ]; then
    printf '  FAIL --github-output exited %s with uv unreachable, expected 0 (the fail-safe arm)\n' \
      "$rc6" >&2
    failures=$((failures + 1))
  fi
  if ! grep -qx 'nothing_to_release=false' "$nouv_out"; then
    printf '  FAIL --github-output with uv unreachable did not write nothing_to_release=false\n' >&2
    printf '  --- %s contents ---\n' "$nouv_out" >&2
    cat "$nouv_out" >&2
    failures=$((failures + 1))
  fi
  _expect_failsafe_outputs "row 6 (uv unreachable)" "$nouv_out"
  rm -f "$nouv_out"

  # Row 7 — the FIXTURES verdict loop in release_plan.py:self_test(). The loop is deletable in
  # silence: with it gone, `--self-test` still returns 0 and every other control here still
  # passes, so the fixture table would guard nothing. The arity floor in ci/actionlint/run.sh
  # check 11 does not close this — it counts ROWS, not whether any row is EVALUATED.
  #
  # The row mutates a COPY of the checker: it inverts the first fixture's expected verdict
  # (`True` -> `False` on the "every releasable package is tagged -> skip" row) and asserts the
  # mutated copy's --self-test reports the broken verdict (exit 3). Deleting the loop, or
  # dropping its `rc = 3`, makes the mutant pass and this row red. The mutation is asserted to
  # have actually changed the file, so a renamed fixture cannot make this row vacuous.
  local mut_dir mut_rc=0
  mut_dir="$tmp/fixture-mutant"
  mkdir -p "$mut_dir"
  sed 's/{"a-v1\.0\.0", "b-v1\.0\.0"}, True)/{"a-v1.0.0", "b-v1.0.0"}, False)/' \
    "$HERE/release_plan.py" > "$mut_dir/release_plan.py"
  if cmp -s "$HERE/release_plan.py" "$mut_dir/release_plan.py"; then
    printf '  FAIL row 7 is vacuous: the fixture mutation matched nothing in release_plan.py\n' >&2
    failures=$((failures + 1))
  fi
  uv run --locked --project "$HERE" --python '>=3.12' python3 "$mut_dir/release_plan.py" \
    --self-test >/dev/null 2>&1 || mut_rc=$?
  if [ "$mut_rc" != "3" ]; then
    printf '  FAIL a fixture with an INVERTED expected verdict exited %s, expected 3 — the\n' \
      "$mut_rc" >&2
    printf '       FIXTURES loop in self_test() no longer evaluates its rows\n' >&2
    failures=$((failures + 1))
  fi

  # Row 8 — the COLLECTION-LAYER loop and the shape validation, the same class row 7 closes for
  # the FIXTURES loop. Delete the collection loop, delete the new fixtures, or neuter
  # config_sections, and --self-test still returns 0 with every other row here passing.
  #
  # The mutation is a CONDITION neutering, not a line deletion. Every `raise InconclusiveError(...)`
  # in release_plan.py spans two physical lines, so a sed that deletes the raise leaves an empty
  # `if` body -> IndentationError -> the mutant exits 1, and this row would red with a diagnostic
  # pointing at the wrong thing while `cmp -s` passed.
  #
  # It asserts on STDERR as well as rc. rc 3 alone is satisfiable by the arity floor in
  # self_test(): delete two rows from COLLECTION_ROWS and the floor fires, self_test() returns 3
  # FOR THE FLOOR, and an rc-only assertion goes green while the neutered check went undetected —
  # the two controls covering for each other's absence. Rows 3/4 grep for a specific verdict line
  # for the same reason.
  local mut8_dir mut8_rc=0 mut8_out
  mut8_dir="$tmp/shape-mutant"
  mkdir -p "$mut8_dir"
  sed 's/if not isinstance(workspace, dict):/if False and not isinstance(workspace, dict):/' \
    "$HERE/release_plan.py" > "$mut8_dir/release_plan.py"
  if cmp -s "$HERE/release_plan.py" "$mut8_dir/release_plan.py"; then
    printf '  FAIL row 8 is vacuous: the shape mutation matched nothing in release_plan.py\n' >&2
    failures=$((failures + 1))
  fi
  mut8_out="$(uv run --locked --project "$HERE" --python '>=3.12' python3 \
    "$mut8_dir/release_plan.py" --self-test 2>&1)" || mut8_rc=$?
  if [ "$mut8_rc" != "3" ]; then
    printf '  FAIL a neutered [workspace] shape check exited %s, expected 3 — the collection\n' \
      "$mut8_rc" >&2
    printf '       loop in self_test() no longer evaluates its rows\n' >&2
    failures=$((failures + 1))
  fi
  if ! grep -q "a non-table \[workspace\] is inconclusive" < <(printf '%s\n' "$mut8_out"); then
    printf '  FAIL the mutant exited 3 without reporting the non-table [workspace] row — the\n' >&2
    printf '       exit code came from somewhere else (the arity floor, most likely)\n' >&2
    printf '  --- mutant output ---\n%s\n' "$mut8_out" >&2
    failures=$((failures + 1))
  fi

  # Row 9 — SMA-658 fix round 2. The exact untested case the review named: the checker's stdout
  # carries a `nothing_to_release=...` verdict but OMITS one service key entirely — a PARTIAL,
  # malformed write, distinct from row 6's all-broken decision (there rc is non-zero and nothing
  # parses at all). Before fix round 2's presence check folded all five keys together, this
  # shape made `github_output()` ABORT under `set -euo pipefail`: the `skip_gateway` grep found
  # zero matches, exited 1, and the function never reached its `exit 0` — breaking the documented
  # "--github-output always exits 0" contract on a shape neither row 5 nor row 6 reaches.
  #
  # `github_output()` hardcodes `$HERE`, so it cannot be pointed at a synthetic tree the way rows
  # 3/4 are (see the comment above this function) — UNLESS the invoked run.sh's own `$HERE`
  # resolves into the tree. So the whole `$HERE` tree (pyproject.toml, uv.lock, release_plan.py,
  # run.sh) is copied under a stub REPO_ROOT, only the COPY's release_plan.py is replaced with the
  # fixture, and the COPY of run.sh is invoked — never "$0". `BASH_SOURCE[0]` inside that copy
  # resolves `REPO_ROOT` to the stub root, so `HERE` there is the stub directory and the real,
  # tracked `release_plan.py` is never touched: no swap, no restore, and nothing left corrupted if
  # this test is killed mid-run (a signal an EXIT trap cannot catch).
  local stub_root stub_out rc9=0
  stub_root="$tmp/github-output-stub/ci/release-plan"
  mkdir -p "$stub_root"
  cp -R "$HERE/." "$stub_root/"
  cat > "$stub_root/release_plan.py" <<'PYEOF'
import sys
if "--keys" in sys.argv:
    print("iam\ngateway\niam-console\ngateway-console")
    raise SystemExit(0)
print("release-plan: fixture -- a malformed decision missing three chain keys")
print("nothing_to_release=true")
print("skip_iam=true")
print("version_iam=1.0.0")
PYEOF
  stub_out="$(mktemp)"
  GITHUB_OUTPUT="$stub_out" GITHUB_EVENT_NAME=push bash "$stub_root/run.sh" --github-output \
    >/dev/null 2>&1 || rc9=$?
  if [ "$rc9" -ne 0 ]; then
    printf '  FAIL row 9: the wrapper exited %s against a checker output missing one chain\n' \
      "$rc9" >&2
    printf '       key, expected 0 (the fail-safe direction)\n' >&2
    failures=$((failures + 1))
  fi
  # SMA-688: all nine outputs. The stub names four keys and prints one of them, so every other
  # key must come from the fail-safe branch.
  for key in nothing_to_release skip_iam version_iam skip_gateway version_gateway \
    skip_iam-console version_iam-console skip_gateway-console version_gateway-console; do
    if ! grep -q "^${key}=" "$stub_out"; then
      printf '  FAIL row 9: %s was not written when the checker output was missing a chain key\n' \
        "$key" >&2
      failures=$((failures + 1))
    fi
  done
  if ! grep -qx 'skip_iam-console=false' "$stub_out"; then
    printf '  FAIL row 9: the fail-safe branch did not write skip_iam-console=false\n' >&2
    failures=$((failures + 1))
  fi
  rm -f "$stub_out"

  # Rows 10 to 13 — SMA-688, the keys branch (spec § 4.3). Each stub replaces release_plan.py in a
  # COPY of this directory under a stub repository root (the row-9 technique), and gives a --keys
  # answer the wrapper must refuse. The stub's decision run prints a COMPLETE decision for all four
  # keys with every skip_ set to true. So a wrapper that runs the decision after a refused --keys
  # answer and trusts it writes true, and the fail-safe assertions red.
  #   Row 10: --keys prints no key.              chains.toml is present -> the bash read, exit 0.
  #   Row 11: --keys prints the invalid IAM_X.   chains.toml is present -> the bash read, exit 0.
  #   Row 12: --keys fails (exit 3).             chains.toml is missing -> exit 2.
  #   Row 13: --keys fails (exit 3).             chains.toml is empty   -> exit 2.
  # In rows 12 and 13 no source names a key, so the wrapper cannot name the chain outputs. It
  # must write nothing_to_release=false only: exactly one line, so no skip_ line.
  _keys_branch_row() { # $1 row, $2 a Python literal: the --keys stdout, $3 the --keys exit code, $4 copy | missing | empty
    local label="$1" keys_body="$2" keys_exit="$3" registry="$4" stub out rc=0 want=2 n
    stub="$tmp/keys-stub-${label}"
    mkdir -p "$stub/ci/release-plan" "$stub/ci/images"
    cp -R "$HERE/." "$stub/ci/release-plan/"
    case "$registry" in
      copy)    cp "$REPO_ROOT/ci/images/chains.toml" "$stub/ci/images/chains.toml"; want=0 ;;
      empty)   : > "$stub/ci/images/chains.toml" ;;
      missing) : ;;
    esac
    printf 'import sys\nif "--keys" in sys.argv:\n    sys.stdout.write(%s)\n    raise SystemExit(%s)\nprint("nothing_to_release=true")\nfor k in ("iam", "gateway", "iam-console", "gateway-console"):\n    print(f"skip_{k}=true")\n    print(f"version_{k}=1.0.0")\n' \
      "$keys_body" "$keys_exit" > "$stub/ci/release-plan/release_plan.py"
    out="$(mktemp)"
    GITHUB_OUTPUT="$out" GITHUB_EVENT_NAME=push bash "$stub/ci/release-plan/run.sh" --github-output \
      >/dev/null 2>&1 || rc=$?
    if [ "$rc" -ne "$want" ]; then
      printf '  FAIL row %s: the wrapper exited %s after a --keys answer it must not trust, expected %s\n' \
        "$label" "$rc" "$want" >&2
      failures=$((failures + 1))
    fi
    if [ "$want" -eq 0 ]; then
      _expect_failsafe_outputs "row $label" "$out"
    else
      n="$(grep -c '' "$out" || true)"
      if [ "$n" != "1" ] || ! grep -qx 'nothing_to_release=false' "$out"; then
        printf '  FAIL row %s: expected nothing_to_release=false as the only line, got %s line(s)\n' \
          "$label" "$n" >&2
        printf '  --- %s contents ---\n' "$out" >&2
        cat "$out" >&2
        failures=$((failures + 1))
      fi
    fi
    rm -f "$out"
  }
  _keys_branch_row 10 "''" 0 copy
  _keys_branch_row 11 "'iam\\nIAM_X\\n'" 0 copy
  _keys_branch_row 12 "''" 3 missing
  _keys_branch_row 13 "''" 3 empty

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
  selftest) require_uv; run_checker --self-test ;;
  negctl)   require_uv; negative_control ;;
  assert)   require_uv; run_checker --assert "$REPO_ROOT" ;;
  *)        die_infra "one mode is required: --github-output | --self-test | --negative-control | --assert" ;;
esac
