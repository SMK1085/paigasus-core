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
# WHY THE BUILD DID NOT CATCH IT — paigasus-console-ts:build declares inputs
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

APP='ts/apps/paigasus-console'
FILE="$APP/next-env.d.ts"

if [ ! -f "$FILE" ]; then
  echo "next-env gate: '$FILE' is missing from the working tree — it is a tracked file." >&2
  exit 2
fi

# Present is not the same as TRACKED, and only tracked is meaningful here. `git diff`
# ignores untracked paths, so after a `git rm --cached` (a plausible "it's generated, stop
# tracking it" move) typegen would recreate the file, the diff would compare nothing, and
# this gate would report a clean pass forever while guarding an untracked file. Verified by
# reproducing exactly that before adding this check. (CodeRabbit review, SMA-519)
if ! git ls-files --error-unmatch -- "$FILE" >/dev/null 2>&1; then
  echo "next-env gate: '$FILE' exists but is NOT tracked by git." >&2
  echo "  This gate compares generated output against the committed copy; with nothing" >&2
  echo "  committed that comparison is vacuous and would pass unconditionally." >&2
  echo "  Re-track the file, or remove this gate deliberately rather than leaving it inert." >&2
  exit 2
fi

# If typegen dies before writing the file, restore it rather than leaving the tree broken.
# A DRIFTING file is deliberately left in place: it is the corrected content, ready to commit.
restore_if_absent() {
  [ -f "$FILE" ] || git checkout -- "$FILE" 2>/dev/null || true
}
trap restore_if_absent EXIT

rm -f "$FILE"

# `next typegen` regenerates route/page/layout types without a full production build
# (~1.5s vs ~5s). It writes into .next/, which is why moon.yml orders this task after
# paigasus-console-ts:build rather than letting the two race on that directory.
if ! pnpm --dir "$APP" exec next typegen >/dev/null 2>&1; then
  echo "next-env gate: 'next typegen' failed in $APP." >&2
  pnpm --dir "$APP" exec next typegen >&2 || true
  exit 2
fi

# Control: typegen must actually have produced the file. Without this the gate would go
# quietly vacuous the day Next changes how this file is emitted.
if [ ! -f "$FILE" ]; then
  echo "next-env gate: 'next typegen' completed but did not emit $FILE." >&2
  echo "  Next no longer generates this file the same way, so this gate is guarding nothing." >&2
  echo "  Update ci/next-env/run.sh (or drop the gate if the file is no longer generated)." >&2
  exit 2
fi

if ! git diff --exit-code -- "$FILE"; then
  echo "" >&2
  echo "next-env gate: the committed $FILE does not match what Next generates." >&2
  echo "  The regenerated file has been left in your working tree — commit it." >&2
  echo "  This usually means Next was upgraded without rebuilding the console app." >&2
  exit 1
fi

echo "next-env gate: $FILE matches 'next typegen' output."
