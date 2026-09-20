#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Golden render of the two valid subsets. --kube-version is pinned because helm's default moves
# with the binary, and these files are a byte pin. Re-baseline deliberately with --update; a
# golden change is a reviewable event, never a mechanical edit to clear a red.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHART="$(cd "$HERE/.." && pwd)"
KUBE_VERSION="1.31.0"
UPDATE=0
if [ "${1:-}" = "--update" ]; then UPDATE=1; shift; fi
ec=0
BASE=("$@")

FIXED=(
  --kube-version "$KUBE_VERSION"
  --set ingress.host=console.example.test
  --set ingress.tlsSecretName=console-tls
  --set oidc.issuer=https://idp.example.test/realms/paigasus
  --set oidc.clientId=paigasus-console
  --set oidc.existingSecret=paigasus-console-secret
  --set postgres.existingSecret=paigasus-postgres-secret
  --set zones.iam.backend.apiKeysPepperSecret=paigasus-iam-pepper
  # zones.gateway.backend.url is REQUIRED whenever the gateway zone is enabled (Task 11b): the
  # chart does not deploy the gateway backend, so an "iam-and-gateway" render fails validation
  # without this. A fixture value, not a weakening of the refusal.
  --set zones.gateway.backend.url=http://gw.example.test:8088
)

render_one() {
  local name="$1"; shift
  local want="$HERE/golden/${name}.yaml" got
  # A bare `got="$(cmd)"` assignment aborts the whole script under set -e if cmd fails, cancelling
  # every row after it. Capture inside the if-condition instead, so a failing render is reported
  # and the battery keeps going.
  if ! got="$(helm template paigasus "$CHART" "${FIXED[@]}" "${BASE[@]}" "$@" 2>&1)"; then
    echo "FAIL [$name]: expected a successful render"; printf '%s\n' "$got"; ec=1; return
  fi
  if [ "$UPDATE" -eq 1 ]; then
    mkdir -p "$HERE/golden"; printf '%s\n' "$got" > "$want"
    echo "  updated ${name}.yaml"; return
  fi
  if [ ! -f "$want" ]; then
    echo "FAIL [$name]: no golden file at $want — run render.sh --update"; ec=1; return
  fi
  if ! diff -u "$want" <(printf '%s\n' "$got"); then
    echo "FAIL [$name]: rendered output differs from the golden file"; ec=1
  else
    echo "  ok [$name]"
  fi
}

if helm lint "$CHART" "${FIXED[@]}" "${BASE[@]}" >/dev/null; then
  :
else
  echo "FAIL: helm lint"; helm lint "$CHART" "${FIXED[@]}" "${BASE[@]}" || true; ec=1
fi
render_one "iam-only"        --set zones.gateway.enabled=false
render_one "iam-and-gateway" --set zones.gateway.enabled=true

if [ "$ec" -eq 0 ]; then echo "== chart render OK =="; fi
exit "$ec"
