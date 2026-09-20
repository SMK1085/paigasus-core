#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Spec § 7.7. Four values combinations, two valid and two refused. A refused one must fail with
# ITS OWN message, not with an incidental template error from somewhere else — otherwise the
# chart is refusing by accident and a later edit silently makes it install.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHART="$(cd "$HERE/.." && pwd)"
KUBE_VERSION="1.31.0"
ec=0
BASE=("$@")

render() { helm template t "$CHART" --kube-version "$KUBE_VERSION" "${BASE[@]}" "$@" 2>&1; }

expect_fail() {
  local label="$1" needle="$2"; shift 2
  local out
  out="$(render "$@")" && { echo "FAIL [$label]: rendered, expected a refusal"; ec=1; return; }
  if grep -qF -- "$needle" < <(printf '%s' "$out"); then
    echo "  ok [$label]: refused with its own message"
  else
    echo "FAIL [$label]: refused, but not with \"$needle\""; printf '%s\n' "$out"; ec=1
  fi
}

expect_render() {
  local label="$1"; shift
  local out
  # Capture once. The earlier shape re-ran `render "$@"` bare inside the else branch to print the
  # error, and under `set -e` that non-zero return ABORTED the whole script — so one failing row
  # silently cancelled every row after it. A battery that stops early hides findings, which is the
  # opposite of what it is for.
  if out="$(render "$@")"; then
    echo "  ok [$label]: renders"
  else
    echo "FAIL [$label]: expected a successful render"; printf '%s\n' "$out"; ec=1
  fi
}

expect_fail "no zone enabled" "at least one zone must be enabled" \
  --set zones.iam.enabled=false --set zones.gateway.enabled=false
expect_fail "gateway without iam" "the gateway zone requires the iam zone" \
  --set zones.iam.enabled=false --set zones.gateway.enabled=true
expect_fail "unknown zone id" "is not a known service slug" \
  --set zones.frobnicate.enabled=true --set zones.frobnicate.basePath=/frobnicate
expect_fail "external backend without url" "backend.url is required" \
  --set zones.gateway.enabled=true --set zones.gateway.backend.deploy=false \
  --set zones.gateway.backend.url=""
expect_render "iam only" --set zones.gateway.enabled=false
expect_render "iam and gateway" --set zones.gateway.enabled=true \
  --set zones.gateway.backend.url=http://gw.example.test:8088

if [ "$ec" -eq 0 ]; then echo "== chart refusals OK =="; fi
exit "$ec"
