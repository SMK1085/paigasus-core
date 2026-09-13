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
# WHY DELETE-THEN-REGENERATE — diffing a file that nothing rewrote is a vacuous assertion:
# if a future Next stops emitting next-env.d.ts, a naive `typegen && git diff` would pass
# forever while guarding nothing. Removing it first makes the gate self-proving — an absent
# file is a loud failure, not a silent pass.
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# If typegen dies before writing a file, restore it rather than leaving the tree broken. A
# DRIFTING file is deliberately left in place: it is the corrected content, ready to commit.
RESTORE_FILES=()
restore_if_absent() {
  local f
  for f in "${RESTORE_FILES[@]:-}"; do
    [ -f "$f" ] || git checkout -- "$f" 2>/dev/null || true
  done
}
trap restore_if_absent EXIT

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

  rm -f "$FILE"
  RESTORE_FILES+=("$FILE")

  # `next typegen` regenerates route/page/layout types without a full production build
  # (~1.5s vs ~5s). It writes into "$APP"/.next/, which is why moon.yml orders this task after
  # iam-console-ts:build rather than letting the two race on that directory.
  if ! pnpm --dir "$APP" exec next typegen >/dev/null 2>&1; then
    echo "next-env gate: 'next typegen' failed in $APP." >&2
    pnpm --dir "$APP" exec next typegen >&2 || true
    return 2
  fi

  # Control: typegen must actually have produced the file. Without this the gate would go quietly
  # vacuous the day Next changes how this file is emitted.
  if [ ! -f "$FILE" ]; then
    echo "next-env gate: 'next typegen' completed but did not emit $FILE." >&2
    echo "  Next no longer generates this file the same way, so this gate is guarding nothing." >&2
    return 2
  fi

  if ! git diff --exit-code -- "$FILE"; then
    echo "" >&2
    echo "next-env gate: the committed $FILE does not match what Next generates." >&2
    echo "  The regenerated file has been left in your working tree — commit it." >&2
    return 1
  fi

  echo "next-env gate: $FILE matches 'next typegen' output."
  return 0
}

# SMA-512: this gate checked ONE hardcoded app until a second console zone landed, and it would
# have skipped the new one in silence. Discovery plus the set-equality assertion below is what
# makes a future third zone impossible to miss.
#
# NOTE: this gate has no --self-test and no --negative-control, deliberately. Adding them costs a
# SELF_SCHEDULED_GATES entry plus a SELF_TASK_EXPECTED_GLOBS or SELF_TASK_GLOBS_EXEMPT entry in
# ci/affected-graph/ci_targets.py. The loop and the set-equality assertion are the control.
shopt -s nullglob
apps=()
for cfg in ts/apps/*/next.config.[tjmc][sj]*; do
  apps+=("$(dirname "$cfg")")
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
for d in "${dirs[@]}"; do
  found=0
  for a in "${apps[@]}"; do
    [ "$a" = "$d" ] && found=1
  done
  [ "$found" -eq 1 ] || missing+=("$d")
done
if [ "${#missing[@]}" -gt 0 ]; then
  echo "next-env gate: these ts/apps/* directories have no discoverable next.config.*:" >&2
  printf '  %s\n' "${missing[@]}" >&2
  echo "  Each would be skipped by this gate in silence. Add a config, or remove the directory." >&2
  exit 2
fi

# Preserve the HIGHEST severity seen across apps: rc 2 (infrastructure failure — a missing or
# untracked file, or a broken typegen) outranks rc 1 (content drift), because an infrastructure
# failure must never be reported as if it were mere drift needing a commit.
rc=0
for APP in "${apps[@]}"; do
  ec=0
  check_app "$APP" || ec=$?
  if [ "$ec" -gt "$rc" ]; then
    rc="$ec"
  fi
done
exit "$rc"
