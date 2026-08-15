#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Asserts the committed test fixture grants EXACTLY the subjects in `subjects.env` (SMA-493 D10),
# PER IDENTITY — not merely that the same subjects appear somewhere in the file. A flattened-set
# comparison (union everything, compare sets) would pass even if, say, `iam.>` were granted to
# `gateway-consumer` instead of `iam-publisher`: the two identities' allow-lists would still union
# to the same overall set, so nothing would look wrong. That is exactly the mistake this gate
# exists to catch — the whole point of SMA-493 is that `gateway-consumer` must NOT be able to read
# `iam.>` — so each user's own stanza is checked against that user's own `subjects.env` arrays,
# not against the pooled total.
#
# Why this exists: the permission lists live in two encodings — `provision.sh` (what deploys) and
# `accounts.conf.tmpl` (what the integration test proves). Without this gate the artifact that is
# PROVEN is not the artifact that is DEPLOYED, and the acceptance criterion would be satisfied by
# the wrong file.
set -euo pipefail

# `local -n` (nameref) below needs bash >= 4.3; macOS ships 3.2 as `/bin/bash`. Guard first so a
# contributor running this locally gets an actionable message instead of a raw syntax error.
(( BASH_VERSINFO[0] > 4 || (BASH_VERSINFO[0] == 4 && BASH_VERSINFO[1] >= 3) )) || {
  echo "this script needs bash >= 4.3 (macOS ships 3.2 — try: brew install bash)" >&2; exit 1; }

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=/dev/null
source "$here/subjects.env"

tmpl="$here/test/accounts.conf.tmpl"
fail=0

# Bounds one identity's stanza to the lines from its own nkey placeholder up to (but not
# including) the NEXT identity's nkey placeholder — or end of file for the last identity — then
# checks that stanza's own `publish:`/`subscribe:` lines against that identity's own PUB/SUB
# arrays, in both directions. `next_placeholder` empty means "to end of file".
check_identity() {
  local identity="$1" nkey_placeholder="$2"; shift 2
  local -n pub_arr="$1"; shift
  local -n sub_arr="$1"; shift
  local next_placeholder="${1:-}"

  # `|| true` on each of these: under `set -e`/`pipefail`, a `grep` that matches nothing makes
  # the pipeline (and so the plain assignment) fail, which would exit the WHOLE script right here
  # — before the `[ -z ... ]` checks below ever run — turning a named "MISSING placeholder"
  # diagnostic into a silent, unexplained abort. Tolerating no-match keeps the diagnostics live.
  local start
  start=$(grep -nF -- "$nkey_placeholder" "$tmpl" | head -1 | cut -d: -f1 || true)
  if [ -z "$start" ]; then
    echo "MISSING from accounts.conf.tmpl: no nkey placeholder $nkey_placeholder found for $identity" >&2
    fail=1
    return
  fi

  local end
  if [ -n "$next_placeholder" ]; then
    local next_start
    next_start=$(grep -nF -- "$next_placeholder" "$tmpl" | head -1 | cut -d: -f1 || true)
    if [ -z "$next_start" ]; then
      echo "MISSING from accounts.conf.tmpl: no nkey placeholder $next_placeholder found (needed to bound $identity's stanza)" >&2
      fail=1
      return
    fi
    end=$((next_start - 1))
  else
    end=$(wc -l < "$tmpl")
  fi

  local block pub_line sub_line
  block=$(sed -n "${start},${end}p" "$tmpl")
  # Each identity's stanza has exactly one `publish:`/`subscribe:` line in this template's
  # formatting (one allow-list per direction, all on one line) — bounding the search to this
  # identity's own line range is what makes the per-identity assertion possible at all.
  pub_line=$(grep 'publish:' <<< "$block" || true)
  sub_line=$(grep 'subscribe:' <<< "$block" || true)

  local s
  for s in "${pub_arr[@]}"; do
    if ! grep -qF -- "\"$s\"" <<< "$pub_line"; then
      echo "MISSING from accounts.conf.tmpl: $s (expected in $identity's publish.allow)" >&2
      fail=1
    fi
  done
  for s in "${sub_arr[@]}"; do
    if ! grep -qF -- "\"$s\"" <<< "$sub_line"; then
      echo "MISSING from accounts.conf.tmpl: $s (expected in $identity's subscribe.allow)" >&2
      fail=1
    fi
  done

  # The other direction: every quoted subject in THIS identity's own allow lists must be
  # accounted for by THIS identity's own subjects.env arrays, so one identity cannot quietly grant
  # something subjects.env only authorised for a different identity.
  local pub_declared sub_declared pub_granted sub_granted
  pub_declared=$(printf '%s\n' "${pub_arr[@]}" | sort -u)
  sub_declared=$(printf '%s\n' "${sub_arr[@]}" | sort -u)
  pub_granted=$(grep -oE 'allow: \[[^]]*\]' <<< "$pub_line" | grep -oE '"[^"]+"' | tr -d '"' | sort -u || true)
  sub_granted=$(grep -oE 'allow: \[[^]]*\]' <<< "$sub_line" | grep -oE '"[^"]+"' | tr -d '"' | sort -u || true)

  local g
  while IFS= read -r g; do
    [ -z "$g" ] && continue
    if ! printf '%s\n' "$pub_declared" | grep -qxF -- "$g"; then
      echo "UNDECLARED grant in accounts.conf.tmpl (not in subjects.env): $g (found in $identity's publish.allow)" >&2
      fail=1
    fi
  done <<< "$pub_granted"
  while IFS= read -r g; do
    [ -z "$g" ] && continue
    if ! printf '%s\n' "$sub_declared" | grep -qxF -- "$g"; then
      echo "UNDECLARED grant in accounts.conf.tmpl (not in subjects.env): $g (found in $identity's subscribe.allow)" >&2
      fail=1
    fi
  done <<< "$sub_granted"
}

check_identity iam-publisher    '{{PUBLISHER_NKEY}}'   PUBLISHER_PUB   PUBLISHER_SUB   '{{CONSUMER_NKEY}}'
check_identity gateway-consumer '{{CONSUMER_NKEY}}'    CONSUMER_PUB    CONSUMER_SUB    '{{PROVISIONER_NKEY}}'
check_identity iam-provisioner  '{{PROVISIONER_NKEY}}' PROVISIONER_PUB PROVISIONER_SUB ''

if [ "$fail" -ne 0 ]; then
  echo "ops/nats: accounts.conf.tmpl and subjects.env disagree" >&2
  exit 1
fi
echo "ops/nats: accounts.conf.tmpl grants exactly the subjects declared in subjects.env"
