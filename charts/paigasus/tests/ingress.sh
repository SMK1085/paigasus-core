#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Spec F2. The ingress path set, PAIGASUS_ZONES's key set and PAIGASUS_SERVICES's key set must
# all agree, for both valid zone subsets — and a disabled zone must leave NO trace anywhere in
# the rendered output. The two checks are separate on purpose: the key-set comparison compares
# sets that all derive from one `range` and cannot see a value written outside that range.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHART="$(cd "$HERE/.." && pwd)"
# Every other REQUIRED value, held valid, so paigasus.validate's per-value refusals (SMA-513
# Task 14) do not block this script's own render assertions for the wrong reason.
BASE=(--kube-version 1.31.0 --set ingress.host=console.example.test \
  --set ingress.tlsSecretName=console-tls \
  --set oidc.issuer=https://idp.example.test/realms/paigasus \
  --set oidc.clientId=paigasus-console \
  --set oidc.existingSecret=paigasus-console-secret \
  --set postgres.existingSecret=paigasus-postgres-secret \
  --set zones.iam.backend.apiKeysPepperSecret=paigasus-iam-pepper \
  --set zones.gateway.backend.url=http://gw.example.test:8088 "$@")
ec=0

coupling() {
  local label="$1" want="$2"; shift 2
  local out got
  # Capture, then test the variable. Re-running a failing helm template bare would abort the
  # whole script under set -e and cancel every row after it.
  if ! out="$(helm template t "$CHART" "${BASE[@]}" "$@" 2>&1)"; then
    echo "FAIL [$label]: expected a successful render"; printf '%s\n' "$out"; ec=1; return
  fi
  got="$(printf '%s' "$out" | python3 -c '
import sys,yaml,json
docs=[d for d in yaml.safe_load_all(sys.stdin) if d]
ing=[d for d in docs if d["kind"]=="Ingress"]
paths=sorted(p["path"] for i in ing for r in i["spec"]["rules"] for p in r["http"]["paths"])
cm=[d for d in docs if d["kind"]=="ConfigMap" and d["metadata"]["name"].endswith("-zonemap")][0]
zones=json.loads(cm["data"]["PAIGASUS_ZONES"]); services=json.loads(cm["data"]["PAIGASUS_SERVICES"])
print("paths=%s zones=%s services=%s" % (",".join(paths), ",".join(sorted(zones)), ",".join(sorted(services))))
print("COUPLED" if sorted(zones)==sorted(services) and paths==sorted(zones.values()) else "DRIFT")')"
  # Process substitution, not a pipe into grep -q: check 13 bans piping into an early-exit reader.
  if grep -qF -- "DRIFT" < <(printf '%s' "$got"); then
    echo "FAIL [$label]: $got"; ec=1
  else
    echo "  ok [$label]: $(printf '%s' "$got" | sed -n 1p)"
  fi
  # A disabled zone must leave NO trace — a separate assertion, because the comparison above
  # compares sets that all derive from one `range` and cannot see a value written outside it.
  local absent
  for absent in $want; do
    if grep -qF -- "$absent" < <(printf '%s' "$out"); then
      echo "FAIL [$label]: disabled zone \"$absent\" appears in the rendered output"; ec=1
    fi
  done
}

coupling "iam only" "gateway" --set zones.gateway.enabled=false
coupling "iam and gateway" "" --set zones.gateway.enabled=true

if [ "$ec" -eq 0 ]; then echo "== chart ingress coupling OK =="; fi
exit "$ec"
