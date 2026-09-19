#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# The zone map and the service map must carry EXACTLY the enabled zones — no more, no less. The
# "no more" half is its own assertion because the key-set comparison in the gate compares sets
# that all derive from one `range`, so it cannot catch a value written outside that range.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHART="$(cd "$HERE/.." && pwd)"
BASE=(--kube-version 1.31.0 --set ingress.host=console.example.test "$@")
ec=0

keys() {  # keys() <json>
  printf '%s' "$1" | python3 -c 'import json,sys;print(" ".join(sorted(json.load(sys.stdin))))'
}

check() {
  local label="$1" want="$2"; shift 2
  local out zones services
  if ! out="$(helm template t "$CHART" "${BASE[@]}" "$@" 2>&1)"; then
    echo "FAIL [$label]: expected a successful render"; printf '%s\n' "$out"; ec=1; return
  fi
  zones="$(printf '%s' "$out" | python3 -c '
import sys,yaml
for d in yaml.safe_load_all(sys.stdin):
    if d and d.get("kind")=="ConfigMap" and d["metadata"]["name"].endswith("-zonemap"):
        print(d["data"]["PAIGASUS_ZONES"]); break')"
  services="$(printf '%s' "$out" | python3 -c '
import sys,yaml
for d in yaml.safe_load_all(sys.stdin):
    if d and d.get("kind")=="ConfigMap" and d["metadata"]["name"].endswith("-zonemap"):
        print(d["data"]["PAIGASUS_SERVICES"]); break')"
  for pair in "zones:$zones" "services:$services"; do
    local name="${pair%%:*}" json="${pair#*:}" got
    got="$(keys "$json")"
    if [ "$got" != "$want" ]; then
      echo "FAIL [$label/$name]: got \"$got\", want \"$want\""; ec=1
    else
      echo "  ok [$label/$name]: $got"
    fi
  done
}

check "iam only"        "iam"         --set zones.gateway.enabled=false
check "iam and gateway" "gateway iam" --set zones.gateway.enabled=true

if [ "$ec" -eq 0 ]; then echo "== chart maps OK =="; fi
exit "$ec"
