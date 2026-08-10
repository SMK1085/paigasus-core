#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Asserts the committed test fixture grants EXACTLY the subjects in `subjects.env` (SMA-493 D10).
#
# Why this exists: the permission lists live in two encodings — `provision.sh` (what deploys) and
# `accounts.conf.tmpl` (what the integration test proves). Without this gate the artifact that is
# PROVEN is not the artifact that is DEPLOYED, and the acceptance criterion would be satisfied by
# the wrong file.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "$here/subjects.env"

tmpl="$here/test/accounts.conf.tmpl"
fail=0

expect_present() {
  local subject="$1"
  if ! grep -qF -- "\"$subject\"" "$tmpl"; then
    echo "MISSING from accounts.conf.tmpl: $subject" >&2
    fail=1
  fi
}

for s in "${PUBLISHER_PUB[@]}" "${PUBLISHER_SUB[@]}" \
         "${CONSUMER_PUB[@]}" "${CONSUMER_SUB[@]}" \
         "${PROVISIONER_PUB[@]}" "${PROVISIONER_SUB[@]}"; do
  expect_present "$s"
done

# The other direction: every quoted subject inside an allow list must be accounted for, so a
# fixture cannot quietly grant something `subjects.env` never authorised.
declared=$(printf '%s\n' "${PUBLISHER_PUB[@]}" "${PUBLISHER_SUB[@]}" \
                          "${CONSUMER_PUB[@]}" "${CONSUMER_SUB[@]}" \
                          "${PROVISIONER_PUB[@]}" "${PROVISIONER_SUB[@]}" | sort -u)
granted=$(grep -oE 'allow: \[[^]]*\]' "$tmpl" | grep -oE '"[^"]+"' | tr -d '"' | sort -u)

while IFS= read -r s; do
  [ -z "$s" ] && continue
  if ! printf '%s\n' "$declared" | grep -qxF -- "$s"; then
    echo "UNDECLARED grant in accounts.conf.tmpl (not in subjects.env): $s" >&2
    fail=1
  fi
done <<< "$granted"

if [ "$fail" -ne 0 ]; then
  echo "ops/nats: accounts.conf.tmpl and subjects.env disagree" >&2
  exit 1
fi
echo "ops/nats: accounts.conf.tmpl grants exactly the subjects declared in subjects.env"
