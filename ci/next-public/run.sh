#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# repo:next-public-free — ban NEXT_PUBLIC_ across ts/, and require every app to build its Next
# config through @paigasus/next-config's factory (SMA-502).
#
# WHY. Next inlines every NEXT_PUBLIC_* value at `next build` time, so one OCI image cannot serve
# two self-hosters with different IdP issuers or API URLs. That single fact is the whole reason
# @paigasus/next-config exists. A grep is enough to enforce it mechanically, which beats relying
# on review forever.
#
# Exit codes: 0 pass | 1 the repo is wrong | 2 infrastructure failed.
#
# CHECK 2 EXISTS BECAUSE CHECK 1 IS NOT SUFFICIENT. `env:` in a next.config is a build-time
# inlining channel exactly equivalent to NEXT_PUBLIC_. createNextConfig() refuses an `extend.env`
# key — but a hand-written next.config.ts that never calls the factory contains no NEXT_PUBLIC_
# literal, passes check 1, and bakes a deployment value into the image anyway.
set -euo pipefail

# proto prints an NDJSON preamble on STDOUT inside an agent session, which poisons a `$(...)`
# capture. Exported once so any future capture in this file inherits it (SMA-609).
export PROTO_REPORTER=text

# Computed from BASH_SOURCE with NO env override, matching every other ci/*/run.sh. An override
# would let `REPO_ROOT=<anywhere> bash ci/next-public/run.sh` — the ordinary check path, no flag —
# scan an empty tree and report a clean pass. self_test and negative_control instead point a COPY
# of this file at their fixture directories, so BASH_SOURCE resolves there naturally.
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$REPO_ROOT"

BANNED='NEXT_PUBLIC_'
# The actual match requires an identifier character after the prefix (grep -E). The hazard is a
# REAL env var — NEXT_PUBLIC_API_URL — never the bare prefix on its own, which is not a valid env
# var name and cannot be read by anything. Matching the bare prefix banned it from being NAMED,
# which broke the one place it is most useful to name: an error message explaining the ban (fix
# round 1, SMA-502). This narrows nothing about evasion — a computed name
# (process.env['NEXT_PUBLIC_' + x]) already defeated a literal text scan either way; see README L2.
#
# The class is [A-Za-z0-9_], WITH the underscore (fix round 2). [A-Za-z0-9] alone MISSES
# NEXT_PUBLIC__DEBUG — a double-underscore name is a perfectly valid env var, and the class without
# `_` does not match the character after the prefix in that case
# (`echo 'NEXT_PUBLIC__FOO' | grep -qE 'NEXT_PUBLIC_[A-Za-z0-9]'` finds nothing — MEASURED). The
# original bare-literal check caught that name; the fix-round-1 narrowing silently traded one false
# positive (naming the bare prefix in prose) for a false NEGATIVE (missing a real double-underscore
# variable), which is the wrong direction for a gate whose only job is to catch this prefix. Adding
# `_` back to the class closes that miss while still not matching the bare prefix on its own —
# `NEXT_PUBLIC_` followed by `.`, a space, or a backtick (the three ways this file's own restored
# prose follows it) is still not in the class, so the fix-round-1 goal (prose may name the bare
# prefix) still holds.
BANNED_RE="${BANNED}[A-Za-z0-9_]"
# The floor is what stops a moved or renamed ts/ silently emptying the gate — the SMA-553 class,
# which repo:input-liveness cannot reach here because it proves a DECLARED glob is live, never
# that the scan still sees anything.
#
# MEASURED (fix round 2): the real corpus is 90 tracked ts/ files after the lockfile and Markdown
# exclusions — grown from 72 as Tasks 2-6 added files; a stale count is exactly the kind of comment
# this branch keeps correcting, so read this as a snapshot, not a fixed ratio. CORPUS_FLOOR is a
# COLLAPSE detector, not a proportion of the current corpus: it answers "does ts/ still exist",
# not "is ts/ still N-sized". 48 was set once, from 72's two-thirds, and stays fixed at 48 as the
# corpus grows — raising it to track growth would turn a collapse detector into a size assertion
# that reds on every legitimate deletion, which is not this floor's job.
CORPUS_FLOOR=48

# The SAME collapse-detector idea, applied to check 2's own list. Check 2 iterated app configs and
# printed its success line when the list was EMPTY, so renaming ts/apps — or adding an app that
# uses next.config.mjs, which check 2 did not match — silently dropped that app while CORPUS_FLOOR
# stayed satisfied, because the corpus counts ts/ FILES and not app configs. One app today.
#
# Deliberate re-baseline, exactly like CORPUS_FLOOR: raise this when a second console zone app
# lands, and lower it only for a real removal.
APP_CONFIG_FLOOR=1

# GLOBAL, not function-local: negative_control's EXIT trap fires after the function that sets this
# has already returned (including via the `exit 1` on its own failure path), and a `local` binding
# is gone by then — referencing it under `set -u` would itself be an unbound-variable error. This
# is MEASURED in this repo; see ci/ruff/run.sh:40-44.
tmp=""

die_infra()  { printf 'next-public-free: %s\n' "$*" >&2; exit 2; }
die_assert() { printf 'next-public-free: %s\n' "$*" >&2; exit 1; }

# Corpus: TRACKED files under ts/, minus the lockfile and Markdown.
#
# Markdown is excluded so a document can name the string it bans; a document compiles into
# nothing. The lockfile is excluded because a dependency NAME can contain the prefix and a
# lockfile is not compiled either. Untracked paths are out of scope entirely, so node_modules
# needs no rule and a third-party dependency using the prefix is unaffected.
next_public_corpus() {
  local root="${1:-$REPO_ROOT}"
  git -C "$root" ls-files -- 'ts/' | grep -v -e '^ts/pnpm-lock\.yaml$' -e '\.md$' | sort
}

# An allowlist entry is a RECORDED DECISION: path<TAB>reason. Ships empty.
allowed_path() {
  case "$1" in
    *) return 1 ;;
  esac
}

check_prefix() {
  local root="${1:-$REPO_ROOT}" corpus rc=0
  local -a files=()
  corpus="$(next_public_corpus "$root")" || rc=$?
  [ "$rc" -eq 0 ] || die_infra "git ls-files failed while deriving the ts/ corpus (rc $rc)"
  if [ -n "$corpus" ]; then
    # PROCESS SUBSTITUTION, NOT A HERE-STRING. `mapfile -t files <<< "$corpus"` DEADLOCKS on
    # bash 5.3.15 (macOS/homebrew): bash writes a here-string into a pipe from `do_redirections`
    # BEFORE the builtin that would drain it starts, so anything past the pipe's capacity blocks
    # on write() forever. MEASURED on this host: 10 lines (330 bytes) passes, 20 lines (670 bytes)
    # hangs; /bin/bash 3.2 never hangs, and neither does process substitution at 200 lines, which
    # forks a writer. It is INTERMITTENT — the same command completed minutes earlier in the same
    # session — because macOS can hand back a 512-byte pipe under memory pressure instead of the
    # usual 16K. The gate's real corpus is 90 paths, well past that, so this hung `moon run
    # repo:next-public-free` with no output and no failure. Do not "simplify" either of these
    # two reads back to `<<<`.
    mapfile -t files < <(printf '%s\n' "$corpus")
  fi
  [ "${#files[@]}" -ge "$CORPUS_FLOOR" ] \
    || die_assert "corpus collapsed to ${#files[@]} files (floor $CORPUS_FLOOR) — did ts/ move? If \
this is a legitimate shrink, lower CORPUS_FLOOR in ci/next-public/run.sh to match the new corpus \
size instead of raising it back later."
  local hits=0 f
  for f in "${files[@]}"; do
    allowed_path "$f" && continue
    if grep -qE -- "$BANNED_RE" "$root/$f" 2>/dev/null; then
      printf '  %s uses %s\n' "$f" "$BANNED" >&2
      hits=$((hits + 1))
    fi
  done
  if [ "$hits" -gt 0 ]; then
    die_assert "$hits file(s) use $BANNED. It is inlined at build time, so one image cannot serve two deployments — read it at request time via @paigasus/next-config/runtime instead."
  fi
  printf 'next-public-free: %d tracked ts/ files free of %s\n' "${#files[@]}" "$BANNED"
}

# ALL FOUR Next config extensions, not just .ts. `next.config.{js,mjs,cjs}` are equally valid Next
# config names, and an app using one was invisible to this check while the corpus floor — which
# counts ts/ files, not app configs — stayed happily satisfied.
APP_CONFIG_GLOB='ts/apps/*/next.config.[tjmc][sj]*'

app_configs() {
  local root="${1:-$REPO_ROOT}"
  git -C "$root" ls-files -- "$APP_CONFIG_GLOB" \
    | grep -E '/next\.config\.(ts|js|mjs|cjs)$' || true
}

check_factory() {
  local root="${1:-$REPO_ROOT}" missing=0 cfg listing
  local -a configs=()
  listing="$(app_configs "$root")"
  if [ -n "$listing" ]; then
    # Process substitution, for the bash 5.3.15 here-string deadlock documented in check_prefix.
    mapfile -t configs < <(printf '%s\n' "$listing")
  fi
  # The floor runs BEFORE the loop, because an empty list makes the loop a no-op and the success
  # line below then reports a check that examined nothing.
  [ "${#configs[@]}" -ge "$APP_CONFIG_FLOOR" ] \
    || die_assert "found ${#configs[@]} app Next config(s) (floor $APP_CONFIG_FLOOR) — did ts/apps \
move, or did an app land under a config name this gate does not match? If an app was legitimately \
removed, lower APP_CONFIG_FLOOR in ci/next-public/run.sh to match instead of raising it back later."
  for cfg in "${configs[@]}"; do
    if ! grep -q 'createNextConfig' "$root/$cfg" 2>/dev/null; then
      printf '  %s does not call createNextConfig()\n' "$cfg" >&2
      missing=$((missing + 1))
    fi
  done
  if [ "$missing" -gt 0 ]; then
    die_assert "$missing app config(s) bypass @paigasus/next-config. A hand-written \`env:\` block inlines exactly as $BANNED does, so the prefix scan alone cannot see it."
  fi
  printf 'next-public-free: all %d app Next config(s) use the factory\n' "${#configs[@]}"
}

# Build a git fixture tree. Each fixture IS a git repository, because the scan reads git ls-files:
# a bare mktemp -d would exercise a different code path and prove nothing about the tracked-file
# filter or the exclusion list. Fixtures live OUTSIDE the working tree — repo:actionlint and
# repo:input-liveness carry inputs: ['**/*'] and hash-walk the whole tree concurrently.
make_fixture() {
  local dir="$1" n=0
  git -C "$dir" init -q
  mkdir -p "$dir/ts/apps/probe" "$dir/ts/packages/probe/src"
  # 60 filler files (+ the config below) clears CORPUS_FLOOR (48) with margin: self_test calls
  # check_prefix directly against this fixture (clean/violating/markdown/lockfile rows), so an
  # undersized fixture would trip the floor instead of exercising the row it is meant to test.
  while [ "$n" -lt 60 ]; do
    printf 'export const filler%s = %s;\n' "$n" "$n" >"$dir/ts/packages/probe/src/filler$n.ts"
    n=$((n + 1))
  done
  printf 'import { createNextConfig } from "@paigasus/next-config";\nexport default createNextConfig({ zone: "probe", basePath: "/probe", outputFileTracingRoot: "/x" });\n' \
    >"$dir/ts/apps/probe/next.config.ts"
  git -C "$dir" add -A >/dev/null 2>&1
}

self_test() {
  local failures=0 clean violating markdown lockfile shrunk nofactory bareprefix realvar underscorevar rc
  local noapps mjsconfig mjsok

  clean="$(mktemp -d)"; make_fixture "$clean"
  rc=0; ( cd "$clean" && check_prefix "$clean" && check_factory "$clean" ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" != 0 ]; then
    printf '  FAIL clean fixture: expected rc 0, got %s\n' "$rc" >&2; failures=$((failures + 1))
  fi

  violating="$(mktemp -d)"; make_fixture "$violating"
  printf 'export const url = process.env.NEXT_PUBLIC_API_URL;\n' >"$violating/ts/packages/probe/src/bad.ts"
  git -C "$violating" add -A >/dev/null 2>&1
  rc=0; ( cd "$violating" && check_prefix "$violating" ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" != 1 ]; then
    printf '  FAIL violating fixture: expected rc 1, got %s\n' "$rc" >&2; failures=$((failures + 1))
  fi

  markdown="$(mktemp -d)"; make_fixture "$markdown"
  printf 'Never use NEXT_PUBLIC_ in this workspace.\n' >"$markdown/ts/packages/probe/NOTES.md"
  git -C "$markdown" add -A >/dev/null 2>&1
  rc=0; ( cd "$markdown" && check_prefix "$markdown" ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" != 0 ]; then
    printf '  FAIL markdown-only fixture: expected rc 0, got %s\n' "$rc" >&2; failures=$((failures + 1))
  fi

  lockfile="$(mktemp -d)"; make_fixture "$lockfile"
  printf 'packages:\n  next-public-shim: 1.0.0 # NEXT_PUBLIC_ in a dep name\n' >"$lockfile/ts/pnpm-lock.yaml"
  git -C "$lockfile" add -A >/dev/null 2>&1
  rc=0; ( cd "$lockfile" && check_prefix "$lockfile" ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" != 0 ]; then
    printf '  FAIL lockfile-only fixture: expected rc 0, got %s\n' "$rc" >&2; failures=$((failures + 1))
  fi

  shrunk="$(mktemp -d)"; git -C "$shrunk" init -q
  mkdir -p "$shrunk/ts"; printf 'x\n' >"$shrunk/ts/only.ts"; git -C "$shrunk" add -A >/dev/null 2>&1
  rc=0; ( cd "$shrunk" && check_prefix "$shrunk" ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" != 1 ]; then
    printf '  FAIL collapsed corpus: expected rc 1 from the floor, got %s\n' "$rc" >&2; failures=$((failures + 1))
  fi

  nofactory="$(mktemp -d)"; make_fixture "$nofactory"
  printf 'export default { env: { ISSUER: process.env.ISSUER } };\n' >"$nofactory/ts/apps/probe/next.config.ts"
  git -C "$nofactory" add -A >/dev/null 2>&1
  rc=0; ( cd "$nofactory" && check_factory "$nofactory" ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" != 1 ]; then
    printf '  FAIL hand-written app config: expected rc 1, got %s\n' "$rc" >&2; failures=$((failures + 1))
  fi

  # These two rows pin the fix-round-1 refinement (SMA-502): the scan must match a REAL env var
  # name, never the bare prefix on its own. Without the first row, someone "simplifying" BANNED_RE
  # back to the bare BANNED literal reintroduces the false positive that broke naming the prefix
  # in an error message — with every other row still green, since none of them names the bare
  # prefix in a non-Markdown file.
  bareprefix="$(mktemp -d)"; make_fixture "$bareprefix"
  printf '// Refuses env because it inlines exactly as NEXT_PUBLIC_ does.\nexport const ok = 1;\n' \
    >"$bareprefix/ts/packages/probe/src/comment.ts"
  git -C "$bareprefix" add -A >/dev/null 2>&1
  rc=0; ( cd "$bareprefix" && check_prefix "$bareprefix" ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" != 0 ]; then
    printf '  FAIL bare-prefix-in-comment fixture: expected rc 0, got %s\n' "$rc" >&2; failures=$((failures + 1))
  fi

  realvar="$(mktemp -d)"; make_fixture "$realvar"
  printf 'export const url = process.env.NEXT_PUBLIC_API_URL;\n' >"$realvar/ts/packages/probe/src/real.ts"
  git -C "$realvar" add -A >/dev/null 2>&1
  rc=0; ( cd "$realvar" && check_prefix "$realvar" ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" != 1 ]; then
    printf '  FAIL real-env-var fixture: expected rc 1, got %s\n' "$rc" >&2; failures=$((failures + 1))
  fi

  # Pins the fix-round-2 correction: [A-Za-z0-9] alone MISSES a double-underscore name, which is a
  # perfectly valid env var (NEXT_PUBLIC__DEBUG) the original bare-literal check caught. Without
  # this row, narrowing the class back to [A-Za-z0-9] reintroduces that false negative with every
  # other row still green — bareprefix/realvar do not exercise an underscore right after the prefix.
  underscorevar="$(mktemp -d)"; make_fixture "$underscorevar"
  printf 'export const debug = process.env.NEXT_PUBLIC__DEBUG;\n' >"$underscorevar/ts/packages/probe/src/underscore.ts"
  git -C "$underscorevar" add -A >/dev/null 2>&1
  rc=0; ( cd "$underscorevar" && check_prefix "$underscorevar" ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" != 1 ]; then
    printf '  FAIL double-underscore env var fixture: expected rc 1, got %s\n' "$rc" >&2; failures=$((failures + 1))
  fi

  # Pins APP_CONFIG_FLOOR. Without this row, check 2 goes SILENT for free: renaming ts/apps leaves
  # `git ls-files` empty, the loop never runs, and the success line prints having examined nothing
  # — with every other row still green, since the corpus floor counts ts/ FILES, not app configs.
  noapps="$(mktemp -d)"; make_fixture "$noapps"
  git -C "$noapps" rm -q --cached 'ts/apps/probe/next.config.ts' >/dev/null 2>&1
  rm -f "$noapps/ts/apps/probe/next.config.ts"
  rc=0; ( cd "$noapps" && check_factory "$noapps" ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" != 1 ]; then
    printf '  FAIL zero app configs: expected rc 1 from APP_CONFIG_FLOOR, got %s\n' "$rc" >&2; failures=$((failures + 1))
  fi

  # Pins the extension widening. `next.config.mjs` is as valid a Next config name as `.ts`, so an
  # app using one must still be required to call the factory. With the old `next.config.ts`-only
  # glob this fixture passed at rc 0 while bypassing @paigasus/next-config entirely.
  mjsconfig="$(mktemp -d)"; make_fixture "$mjsconfig"
  git -C "$mjsconfig" rm -q --cached 'ts/apps/probe/next.config.ts' >/dev/null 2>&1
  rm -f "$mjsconfig/ts/apps/probe/next.config.ts"
  printf 'export default { env: { ISSUER: process.env.ISSUER } };\n' >"$mjsconfig/ts/apps/probe/next.config.mjs"
  git -C "$mjsconfig" add -A >/dev/null 2>&1
  rc=0; ( cd "$mjsconfig" && check_factory "$mjsconfig" ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" != 1 ]; then
    printf '  FAIL hand-written next.config.mjs: expected rc 1, got %s\n' "$rc" >&2; failures=$((failures + 1))
  fi

  # The positive half of the same widening, and it is the row that actually DETECTS a narrowing.
  # MEASURED: reverting APP_CONFIG_GLOB to `next.config.ts` leaves the row above green — with no
  # config matched, APP_CONFIG_FLOOR reds it at rc 1 for a different reason, which is the right
  # verdict but not the reason the row asserts. This row fails on that mutation (expected rc 0,
  # got 1), so the two rows are kept as a pair rather than either one alone.
  mjsok="$(mktemp -d)"; make_fixture "$mjsok"
  git -C "$mjsok" rm -q --cached 'ts/apps/probe/next.config.ts' >/dev/null 2>&1
  rm -f "$mjsok/ts/apps/probe/next.config.ts"
  printf 'import { createNextConfig } from "@paigasus/next-config";\nexport default createNextConfig({ zone: "probe", basePath: "/probe", outputFileTracingRoot: "/x" });\n' \
    >"$mjsok/ts/apps/probe/next.config.mjs"
  git -C "$mjsok" add -A >/dev/null 2>&1
  rc=0; ( cd "$mjsok" && check_factory "$mjsok" ) >/dev/null 2>&1 || rc=$?
  if [ "$rc" != 0 ]; then
    printf '  FAIL factory-using next.config.mjs: expected rc 0, got %s\n' "$rc" >&2; failures=$((failures + 1))
  fi

  rm -rf "$clean" "$violating" "$markdown" "$lockfile" "$shrunk" "$nofactory" "$bareprefix" "$realvar" "$underscorevar" "$noapps" "$mjsconfig" "$mjsok"
  if [ "$failures" -gt 0 ]; then
    printf 'next-public-free self-test: %d row(s) failed\n' "$failures" >&2
    exit 1
  fi
  printf '== next-public-free self-test passed ==\n'
}

negative_control() {
  local negctl_rc=0
  # `tmp` is the FILE-SCOPE global declared above — deliberately not `local`. See its comment.
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  make_fixture "$tmp"
  printf 'export const url = process.env.NEXT_PUBLIC_API_URL;\n' >"$tmp/ts/packages/probe/src/planted.ts"
  git -C "$tmp" add -A >/dev/null 2>&1
  ( cd "$tmp" && check_prefix "$tmp" ) >/dev/null 2>&1 || negctl_rc=$?
  if [ "$negctl_rc" != 1 ]; then
    printf '  FAIL a planted NEXT_PUBLIC_ did not red the gate: expected rc 1, got %s\n' "$negctl_rc" >&2
    exit 1
  fi
  printf '== next-public-free negative control passed ==\n'
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
  check)    check_prefix; check_factory ;;
  negctl)   negative_control ;;
esac
