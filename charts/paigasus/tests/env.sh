#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Every environment key a console needs with no default must actually REACH the container.
#
# MEASURED before this test existed: the secrets arrived through `envFrom.secretRef`, which turns
# each Secret KEY into an env var NAME. The documented keys are hyphenated (`oidc-client-secret`,
# `session-redis-url`), a hyphen is illegal in an env var name, and the kubelet SKIPS such keys
# rather than failing — it records an InvalidVariableNames event and starts the container anyway.
# So PAIGASUS_OIDC_CLIENT_SECRET and PAIGASUS_SESSION_REDIS_URL never arrived, and because
# PAIGASUS_SESSION_STORE is forced to `redis`, readiness could never succeed. `helm template`,
# `helm lint` and the golden files all rendered it happily: the defect is in what the NAMES mean
# to the kubelet, not in the YAML's shape.
#
# The ten keys below are the ones @paigasus/auth, @paigasus/discovery and each app's own
# lib/config.ts declare with no default. Anything missing here is a pod that fails its
# configuration parse on the first request.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHART="$(cd "$HERE/.." && pwd)"
BASE=(
  --kube-version 1.31.0
  --set ingress.host=console.example.test
  --set ingress.tlsSecretName=console-tls
  --set oidc.issuer=https://idp.example.test/realms/paigasus
  --set oidc.clientId=paigasus-console
  --set oidc.existingSecret=paigasus-console-secret
  --set postgres.existingSecret=paigasus-postgres-secret
  --set zones.iam.backend.apiKeysPepperSecret=paigasus-iam-pepper
  --set zones.gateway.backend.url=http://gw.example.test:8088
  "$@"
)
ec=0

REQUIRED="PAIGASUS_ZONE PAIGASUS_ZONES PAIGASUS_SERVICES PAIGASUS_IAM_GRPC_URL \
PAIGASUS_PUBLIC_ORIGIN PAIGASUS_OIDC_ISSUER PAIGASUS_OIDC_CLIENT_ID \
PAIGASUS_OIDC_CLIENT_SECRET PAIGASUS_SESSION_STORE PAIGASUS_SESSION_REDIS_URL"

check() {
  local label="$1"; shift
  local out
  if ! out="$(helm template paigasus "$CHART" "${BASE[@]}" "$@" 2>&1)"; then
    echo "FAIL [$label]: render failed"; printf '%s\n' "$out"; ec=1; return
  fi
  local verdict
  verdict="$(printf '%s' "$out" | REQUIRED="$REQUIRED" python3 "$HERE/env_check.py")"
  if [ "${verdict:0:2}" = "OK" ]; then
    echo "  ok [$label]: $verdict"
  else
    echo "FAIL [$label]: $verdict"; ec=1
  fi
}

check "iam only"        --set zones.gateway.enabled=false
check "iam and gateway" --set zones.gateway.enabled=true

if [ "$ec" -eq 0 ]; then echo "== chart env OK =="; fi
exit "$ec"
