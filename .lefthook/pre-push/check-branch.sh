#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Reject pushing a branch whose name doesn't match the Paigasus convention.
# Validates the CURRENT branch (HEAD); pass a branch name as $1 to override (e.g. tests).
# Use ${1-...} (not ${1:-...}) so an explicit empty arg exercises the detached-HEAD path,
# while lefthook's no-arg invocation still resolves HEAD.
set -euo pipefail

branch="${1-$(git symbolic-ref --short HEAD 2>/dev/null || true)}"

# Detached HEAD / no branch: nothing to validate.
[ -z "$branch" ] && exit 0

# Allow-list: default branch and bot branches.
[ "$branch" = "main" ] && exit 0
case "$branch" in
  dependabot/*) exit 0 ;;
esac

# Enforce: feature/<lowercase-slug>.
if printf '%s' "$branch" | grep -Eq '^feature/[a-z0-9._-]+$'; then
  exit 0
fi

cat >&2 <<EOF
✖ Branch name "$branch" is not allowed.
  Branches must match:  ^feature/[a-z0-9._-]+\$
  Rename this branch:   git branch -m feature/<slug>
  (main and dependabot/* are exempt; CI enforces the same rule server-side.)
EOF
exit 1
