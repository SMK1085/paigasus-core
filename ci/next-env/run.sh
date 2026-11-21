#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Drift gate for the generated-but-tracked next-env.d.ts (SMA-519).
#
# next-env.d.ts is emitted by Next but committed, because tsconfig.json lists it in
# `include` and `typecheck` runs without a Next build. Nothing asserted the committed copy
# matched what Next actually emits, and it sat stale from the ts/ bootstrap in May until
# SMA-517: a newer Next also emits an `./.next/types/root-params.d.ts` reference.
#
# WHY THE BUILD DID NOT CATCH IT — iam-console-ts:build declares inputs
# ['@group(sources)', 'tsconfig.json', 'package.json', 'next.config.ts']. ts/pnpm-lock.yaml
# is NOT among them, so a Next upgrade never re-keys the task: the build stays cached, the
# file is never regenerated, and the drift is invisible. During SMA-517 a full `moon ci`
# reported the file clean and it took `--force` to surface. Hence this gate keys on the
# lockfile (see moon.yml), which is what actually drives this file's content.
#
# WHY REGENERATE IN A SCRATCH COPY — diffing a file that nothing rewrote is a vacuous assertion.
# If a future Next stops emitting next-env.d.ts, a naive `typegen && git diff` passes forever and
# guards nothing. So typegen must start from an ABSENT file. Next 16.4.6 `writeAppTypeDeclarations`
# skips the write when the content is equal, and it keeps the line ending of an existing file. An
# absent file is a loud failure, not a silent pass.
#
# The gate does NOT remove the real file. It copies the app's files, without next-env.d.ts, into a
# scratch directory and runs typegen there. The old gate removed the tracked file in place until
# the follow-up sweep after PR 516. While the file was absent, concurrent moon tasks saw a tracked
# file missing. That broke repo:affected-smoke on PR 515, and PR 516 added an index fallback to
# ci/affected-graph/python_env_ban.py. The gate then compares the generated file with the
# committed blob and with the working-tree file. On drift it replaces the working-tree file with
# one atomic `mv`. No path removes or truncates the real file.
#
# THE NEGATIVE CONTROL (SMA-637) — `--negative-control` proves that this gate can still go red.
# It copies the real git index to a temporary file and changes the copy for every app: row N1
# appends one line to each staged next-env.d.ts, and row N2 removes each one from the copy. Then
# it runs this whole script in check mode as a CHILD PROCESS, with GIT_INDEX_FILE set to the
# copy. N1 must exit 1 and N2 must exit 2. A temporary index, because the control must not
# change the real index, the work tree or a commit. A child process, because a call of
# check_app alone would not prove the aggregation loop and the final `exit` in real_run.
# Exit codes: 0 every row as expected, 1 a row failed, 2 an infrastructure error.
set -euo pipefail

# Resolve this script's own absolute path BEFORE the cd below. The negative control runs this
# same file again as a child process.
SELF="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"

cd "$(git rev-parse --show-toplevel)"

# One scratch directory per run, made on the first use. The EXIT trap removes it, and it removes
# an install temp file that a failed `mv` left behind. Nothing here touches the real file.
GEN_TMP=""
INSTALL_TMP=""
cleanup_gen() {
  if [ -n "$INSTALL_TMP" ]; then
    rm -f "$INSTALL_TMP"
  fi
  if [ -n "$GEN_TMP" ]; then
    rm -rf "$GEN_TMP"
  fi
}
trap cleanup_gen EXIT

make_gen_tmp() {
  [ -z "$GEN_TMP" ] || return 0
  GEN_TMP="$(mktemp -d "${TMPDIR:-/tmp}/next-env-gen.XXXXXX")" || { GEN_TMP=""; return 1; }
  GEN_TMP="$(cd "$GEN_TMP" && pwd -P)" || return 1
}

check_app() {
  local APP="$1"
  local FILE="$APP/next-env.d.ts"

  if [ ! -f "$FILE" ]; then
    echo "next-env gate: '$FILE' is missing from the working tree — it is a tracked file." >&2
    return 2
  fi

  # Present is not the same as TRACKED, and only tracked is meaningful here. `git diff` ignores
  # untracked paths, so after a `git rm --cached` typegen would recreate the file, the diff would
  # compare nothing, and this gate would report a clean pass forever. (CodeRabbit, SMA-519)
  if ! git ls-files --error-unmatch -- "$FILE" >/dev/null 2>&1; then
    echo "next-env gate: '$FILE' exists but is NOT tracked by git." >&2
    echo "  This gate compares generated output against the committed copy; with nothing" >&2
    echo "  committed that comparison is vacuous and would pass unconditionally." >&2
    return 2
  fi

  if ! make_gen_tmp; then
    echo "next-env gate: could not make a scratch directory for typegen." >&2
    return 2
  fi
  local GEN="$GEN_TMP/$(basename "$APP")"
  local LIST="$GEN_TMP/$(basename "$APP").files"
  local COMMITTED="$GEN_TMP/$(basename "$APP").committed"
  local f rel

  # The scratch copy: every tracked or untracked, not ignored file of the app, except
  # next-env.d.ts. The list goes to a file first, so that a failed `git ls-files` is seen.
  if ! git ls-files -z -co --exclude-standard -- "$APP" > "$LIST"; then
    echo "next-env gate: git ls-files failed for $APP." >&2
    return 2
  fi
  mkdir -p "$GEN" || return 2
  while IFS= read -r -d '' f; do
    [ "$f" != "$FILE" ] || continue
    # Copy regular files only. A symlink or a deleted tracked path is skipped.
    if [ ! -f "$f" ] || [ -L "$f" ]; then
      continue
    fi
    rel="${f#"$APP"/}"
    if ! mkdir -p "$GEN/$(dirname "$rel")" || ! cp "$f" "$GEN/$rel"; then
      echo "next-env gate: could not copy $f into the scratch directory." >&2
      return 2
    fi
  done < "$LIST"
  if [ -e "$GEN/next-env.d.ts" ]; then
    echo "next-env gate: the scratch copy of $APP already holds next-env.d.ts." >&2
    return 2
  fi
  if ! ln -s "$PWD/$APP/node_modules" "$GEN/node_modules"; then
    echo "next-env gate: could not link $APP/node_modules into the scratch directory." >&2
    return 2
  fi

  # `next typegen` regenerates route/page/layout types without a full production build
  # (~1.5s vs ~5s). It writes into the scratch copy's .next/, not into "$APP"/.next/.
  if ! pnpm --dir "$APP" exec next typegen "$GEN" >/dev/null 2>&1; then
    echo "next-env gate: 'next typegen' failed in $APP." >&2
    pnpm --dir "$APP" exec next typegen "$GEN" >&2 || true
    return 2
  fi

  # Control: typegen must actually have produced the file. Without this the gate would go quietly
  # vacuous the day Next changes how this file is emitted.
  if [ ! -f "$GEN/next-env.d.ts" ]; then
    echo "next-env gate: 'next typegen' completed but did not emit $FILE." >&2
    echo "  Next no longer generates this file the same way, so this gate is guarding nothing." >&2
    return 2
  fi

  # The committed copy is the index blob. The negative control plants its rows there.
  if ! git cat-file blob ":$FILE" > "$COMMITTED"; then
    echo "next-env gate: git cat-file blob :$FILE failed." >&2
    return 2
  fi
  local crc=0
  diff -u --label "committed $FILE" --label "generated $FILE" "$COMMITTED" "$GEN/next-env.d.ts" || crc=$?
  if [ "$crc" -gt 1 ]; then
    echo "next-env gate: diff failed (rc $crc) on the committed $FILE." >&2
    return 2
  fi
  if [ "$crc" -eq 0 ] && cmp -s "$FILE" "$GEN/next-env.d.ts"; then
    echo "next-env gate: $FILE matches 'next typegen' output."
    return 0
  fi

  # Drift. If the working-tree file differs from the generated file, show the diff (unless it is
  # the committed diff again) and install the generated file atomically: copy it to a temp file in
  # the same directory, then `mv -f` it over the real file.
  if ! cmp -s "$FILE" "$GEN/next-env.d.ts"; then
    if [ "$crc" -eq 0 ] || ! cmp -s "$FILE" "$COMMITTED"; then
      diff -u --label "working-tree $FILE" --label "generated $FILE" "$FILE" "$GEN/next-env.d.ts" || true
    fi
    INSTALL_TMP="$(mktemp "$APP/.next-env.d.ts.XXXXXX")" || { INSTALL_TMP=""; echo "next-env gate: mktemp failed in $APP." >&2; return 2; }
    # mktemp makes the file with mode 0600. `cp -p` first gives it the real file's mode, and the
    # second `cp` then writes the generated content into it.
    if ! cp -p "$FILE" "$INSTALL_TMP" || ! cp "$GEN/next-env.d.ts" "$INSTALL_TMP" || ! mv -f "$INSTALL_TMP" "$FILE"; then
      echo "next-env gate: could not install the regenerated $FILE." >&2
      return 2
    fi
    INSTALL_TMP=""
  fi

  echo "" >&2
  if [ "$crc" -eq 1 ]; then
    echo "next-env gate: the committed $FILE does not match what Next generates." >&2
    echo "  The regenerated file is left in your working tree. Commit it." >&2
  else
    echo "next-env gate: the working-tree $FILE does not match what Next generates." >&2
    echo "  The committed file matches. Only the working-tree file drifted." >&2
    echo "  The regenerated file is left in your working tree. It equals the committed file." >&2
  fi
  return 1
}

# SMA-512: this gate checked ONE hardcoded app until a second console zone landed, and it would
# have skipped the new one in silence. Discovery plus the subset assertion below is what makes a
# future third zone impossible to miss. It is a SUBSET assertion, not set-equality: every
# `ts/apps/*` directory that has a `package.json` must be in the discovered set.
#
# SMA-637: the --negative-control mode below proves that this gate can still go red.
discover_apps() {
  shopt -s nullglob
  apps=()
  # EXACT filenames, never a trailing wildcard. `next.config.[tjmc][sj]*` matched
  # `next.config.ts.bak` (the `*` swallows any suffix) and MISSED `next.config.mts` (`t` is not in
  # `[sj]`) — both measured. A stray backup file would have added a phantom entry, and an `.mts` app
  # would have been invisible here.
  #
  # An app directory is only a workspace member if it has a `package.json` — the same test the
  # liveness assertion below applies, and the one pnpm's own `apps/*` glob uses. Without this filter
  # a config-bearing directory that is NOT a workspace member reaches check_app, which then runs
  # `pnpm --dir` against a non-member and fails confusingly.
  for cfg in ts/apps/*/next.config.{js,mjs,cjs,ts,mts,cts}; do
    candidate="$(dirname "$cfg")"
    [ -f "$candidate/package.json" ] && apps+=("$candidate")
  done
  shopt -u nullglob

  if [ "${#apps[@]}" -eq 0 ]; then
    echo "next-env gate: no ts/apps/*/next.config.* found — this gate is guarding nothing." >&2
    exit 2
  fi

  # LIVENESS. A Next app directory with no discoverable config would otherwise be skipped without a
  # word, which is the exact defect this rewrite exists to remove. Only a directory with its own
  # package.json counts as a workspace member — the same test pnpm's own `apps/*` glob applies —
  # so a stale leftover directory (an old node_modules/.next from a rename, say) is not mistaken
  # for a missing app.
  dirs=()
  for d in ts/apps/*/; do
    [ -f "${d}package.json" ] && dirs+=("${d%/}")
  done
  missing=()
  # `"${dirs[@]:-}"` iterates ONCE with an empty `d` when `dirs` is empty, which printed a blank
  # row under the heading below. Unreachable today — `apps` needs a package.json AND a config, and
  # the exit above fires when `apps` is empty, so reaching here proves `dirs` is non-empty too —
  # but the `:-` idiom is wrong regardless, and a future reordering of the two blocks would make it
  # live. The length guard is the bash-3.2-safe form under `set -u`; the `apps` check above uses it
  # too.
  if [ "${#dirs[@]}" -gt 0 ]; then
    for d in "${dirs[@]}"; do
      found=0
      for a in "${apps[@]}"; do
        [ "$a" = "$d" ] && found=1
      done
      [ "$found" -eq 1 ] || missing+=("$d")
    done
  fi
  if [ "${#missing[@]}" -gt 0 ]; then
    echo "next-env gate: these ts/apps/* directories have no discoverable next.config.*:" >&2
    printf '  %s\n' "${missing[@]}" >&2
    echo "  Each would be skipped by this gate in silence. Add a config, or remove the directory." >&2
    exit 2
  fi
}

# Preserve the HIGHEST severity seen across apps: rc 2 (infrastructure failure — a missing or
# untracked file, or a broken typegen) outranks rc 1 (content drift), because an infrastructure
# failure must never be reported as if it were mere drift needing a commit.
real_run() {
  rc=0
  for APP in "${apps[@]}"; do
    ec=0
    check_app "$APP" || ec=$?
    if [ "$ec" -gt "$rc" ]; then
      rc="$ec"
    fi
  done
  exit "$rc"
}

# ---- The negative control (SMA-637) ------------------------------------------------------------
NEGCTL_TMP=""

# One EXIT handler in negctl mode: it removes the temporary files, then does what the check-mode
# handler does. It REPLACES `trap cleanup_gen EXIT`, so it must call cleanup_gen itself.
negctl_cleanup() {
  if [ -n "$NEGCTL_TMP" ]; then
    rm -rf "$NEGCTL_TMP"
  fi
  cleanup_gen
}

negctl_infra() {
  echo "next-env negative control: INFRA: $*" >&2
  exit 2
}

# The REAL index entries (mode, blob, stage, path) of every app's next-env.d.ts.
real_index_entries() {
  local a
  for a in "${apps[@]}"; do
    git ls-files -s -- "$a/next-env.d.ts" || negctl_infra "git ls-files -s failed for $a/next-env.d.ts"
  done
}

# The child has its own EXIT trap, which removes its scratch directory and any install temp file.
# The child runs with TMPDIR set to "$NEGCTL_TMP/child-tmp", so its scratch directory is there.
# Prove that the trap removed both.
assert_no_leftover() {
  local a left
  for left in "$NEGCTL_TMP"/child-tmp/next-env-gen.*; do
    if [ -e "$left" ]; then
      negctl_infra "[$1] the child left the scratch directory $left behind"
    fi
  done
  for a in "${apps[@]}"; do
    for left in "$a"/.next-env.d.ts.*; do
      if [ -e "$left" ]; then
        negctl_infra "[$1] the child left the install temp file $left behind"
      fi
    done
  done
}

negative_control() {
  local real_index idx_drift idx_untracked out_drift out_untracked
  local a f blob staged rc_drift rc_untracked n_marker fails fails_row

  # Refuse a caller's index (Sven, 2026-10-01): `git rev-parse --git-path index` would return
  # it, and the control would plant its rows in a copy of an index that is not the real one.
  if [ "${GIT_INDEX_FILE+set}" = set ]; then
    negctl_infra "GIT_INDEX_FILE is set ('${GIT_INDEX_FILE}'). The control copies the real index only. Unset GIT_INDEX_FILE and run again."
  fi

  NEGCTL_TMP="$(mktemp -d "${TMPDIR:-/tmp}/next-env-negctl.XXXXXX")" || negctl_infra "mktemp -d failed"
  trap negctl_cleanup EXIT
  # GIT_INDEX_FILE must be an absolute path: the child runs git from the top level, and a
  # relative TMPDIR would otherwise give a relative index path.
  NEGCTL_TMP="$(cd "$NEGCTL_TMP" && pwd -P)" || negctl_infra "cannot resolve the temporary directory"
  case "$NEGCTL_TMP" in
    /*) ;;
    *) negctl_infra "the temporary directory '$NEGCTL_TMP' is not an absolute path" ;;
  esac

  real_index="$(git rev-parse --git-path index)" || negctl_infra "git rev-parse --git-path index failed"
  [ -f "$real_index" ] || negctl_infra "the real index '$real_index' does not exist"
  real_index_entries > "$NEGCTL_TMP/index-before"
  mkdir "$NEGCTL_TMP/child-tmp" || negctl_infra "cannot make $NEGCTL_TMP/child-tmp"
  fails=0

  # ---- N1 drift: the staged blob of every app gets one extra last line. ----
  idx_drift="$NEGCTL_TMP/index-drift"
  out_drift="$NEGCTL_TMP/out-drift"
  cp "$real_index" "$idx_drift" || negctl_infra "[N1] cannot copy the index to $idx_drift"
  for a in "${apps[@]}"; do
    f="$a/next-env.d.ts"
    git cat-file blob ":$f" > "$NEGCTL_TMP/planted" || negctl_infra "[N1] git cat-file blob :$f failed"
    printf '%s\n' '// SMA-637 negative control' >> "$NEGCTL_TMP/planted"
    blob="$(git hash-object -w --no-filters -- "$NEGCTL_TMP/planted")" || negctl_infra "[N1] git hash-object -w failed for $f"
    git cat-file -e "$blob" || negctl_infra "[N1] the planted blob $blob for $f is not in the object store"
    staged="$(git rev-parse ":$f")" || negctl_infra "[N1] git rev-parse :$f failed"
    [ "$blob" != "$staged" ] || negctl_infra "[N1] the planted blob for $f equals the index blob, so N1 would prove nothing"
    GIT_INDEX_FILE="$idx_drift" git update-index --cacheinfo "100644,$blob,$f" || negctl_infra "[N1] git update-index --cacheinfo failed for $f"
  done
  rc_drift=0
  GIT_INDEX_FILE="$idx_drift" TMPDIR="$NEGCTL_TMP/child-tmp" "$BASH" "$SELF" >"$out_drift" 2>&1 || rc_drift=$?
  assert_no_leftover N1
  # An infrastructure failure must never be reported as drift (see real_run).
  if [ "$rc_drift" -eq 2 ]; then
    echo "INFRA [N1] child exited 2 (infrastructure), expected 1" >&2
    cat "$out_drift" >&2
    exit 2
  fi
  fails_row="$fails"
  if [ "$rc_drift" -ne 1 ]; then
    echo "FAIL [N1] expected rc 1, got $rc_drift" >&2
    fails=$((fails + 1))
  fi
  # The marker is the same for every app, so count it: one removed line per app.
  n_marker="$(grep -cF -- '-// SMA-637 negative control' "$out_drift" || true)"
  if [ "$n_marker" != "${#apps[@]}" ]; then
    echo "FAIL [N1] expected ${#apps[@]} diff lines '-// SMA-637 negative control', got $n_marker" >&2
    fails=$((fails + 1))
  fi
  for a in "${apps[@]}"; do
    if ! grep -qF -- "the committed $a/next-env.d.ts does not match what Next generates" "$out_drift"; then
      echo "FAIL [N1 $a] missing 'the committed $a/next-env.d.ts does not match what Next generates'" >&2
      fails=$((fails + 1))
    fi
  done
  if [ "$fails" -eq "$fails_row" ]; then
    echo "next-env negative control: PASS [N1] planted drift gave rc 1 and named every app"
  else
    cat "$out_drift" >&2
  fi

  # ---- N2 untracked: every app's file is removed from the index copy. ----
  idx_untracked="$NEGCTL_TMP/index-untracked"
  out_untracked="$NEGCTL_TMP/out-untracked"
  cp "$real_index" "$idx_untracked" || negctl_infra "[N2] cannot copy the index to $idx_untracked"
  for a in "${apps[@]}"; do
    f="$a/next-env.d.ts"
    GIT_INDEX_FILE="$idx_untracked" git update-index --force-remove -- "$f" || negctl_infra "[N2] git update-index --force-remove failed for $f"
    if GIT_INDEX_FILE="$idx_untracked" git ls-files --error-unmatch -- "$f" >/dev/null 2>&1; then
      negctl_infra "[N2] $f is still listed in the index copy"
    fi
  done
  rc_untracked=0
  GIT_INDEX_FILE="$idx_untracked" TMPDIR="$NEGCTL_TMP/child-tmp" "$BASH" "$SELF" >"$out_untracked" 2>&1 || rc_untracked=$?
  assert_no_leftover N2
  fails_row="$fails"
  if [ "$rc_untracked" -ne 2 ]; then
    echo "FAIL [N2] expected rc 2, got $rc_untracked" >&2
    fails=$((fails + 1))
  fi
  for a in "${apps[@]}"; do
    if ! grep -qF -- "'$a/next-env.d.ts' exists but is NOT tracked by git" "$out_untracked"; then
      echo "FAIL [N2 $a] missing ''$a/next-env.d.ts' exists but is NOT tracked by git'" >&2
      fails=$((fails + 1))
    fi
  done
  if [ "$fails" -eq "$fails_row" ]; then
    echo "next-env negative control: PASS [N2] an untracked file gave rc 2 and named every app"
  else
    cat "$out_untracked" >&2
  fi

  # Post-condition: the real index entries did not change.
  real_index_entries > "$NEGCTL_TMP/index-after"
  cmp -s "$NEGCTL_TMP/index-before" "$NEGCTL_TMP/index-after" || negctl_infra "the real index entries for next-env.d.ts changed during the control"

  if [ "$fails" -ne 0 ]; then
    echo "next-env-drift negative control: $fails check(s) failed" >&2
    exit 1
  fi
  echo "== next-env-drift negative control passed =="
  exit 0
}

MODE=check
while [ $# -gt 0 ]; do
  case "$1" in
    --negative-control) MODE=negctl; shift ;;
    *) echo "next-env gate: unknown flag: $1" >&2; exit 2 ;;
  esac
done

discover_apps
case "$MODE" in
  check)  real_run ;;
  negctl) negative_control ;;
  *) echo "next-env gate: unknown mode '$MODE'" >&2; exit 2 ;;
esac
