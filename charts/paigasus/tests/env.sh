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

# The "${X[@]+...}" guards below are load-bearing, not noise. MEASURED: bash 3.2.57
# treats "${A[@]}" on an EMPTY array as an unbound variable under `set -u`, so a
# no-argument run would abort before the first row. bash 5.x does not. Some gates in
# this repo run under 3.2.

REQUIRED="PAIGASUS_ZONE PAIGASUS_ZONES PAIGASUS_SERVICES PAIGASUS_IAM_GRPC_URL \
PAIGASUS_PUBLIC_ORIGIN PAIGASUS_OIDC_ISSUER PAIGASUS_OIDC_CLIENT_ID \
PAIGASUS_OIDC_CLIENT_SECRET PAIGASUS_SESSION_STORE PAIGASUS_SESSION_REDIS_URL"

check() {
  local label="$1"; shift
  local out
  if ! out="$(helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" "$@" 2>&1)"; then
    echo "FAIL [$label]: render failed"; printf '%s\n' "$out"; ec=1; return
  fi
  local verdict
  # Inlined, like every sibling script here. A separate .py under charts/ would be linted by
  # nothing: repo:ruff-ci's corpus is ci/**/*.py and py:lint's is py/, so charts/**/*.py falls
  # between them. Use only DOUBLE quotes inside this block — a single quote would close it.
  verdict="$(printf '%s' "$out" | REQUIRED="$REQUIRED" python3 -c '
import os, sys, yaml
required = set(os.environ["REQUIRED"].split())
docs = [d for d in yaml.safe_load_all(sys.stdin) if d]
cms = {d["metadata"]["name"]: set(d.get("data", {})) for d in docs if d["kind"] == "ConfigMap"}
problems, consoles = [], 0
for d in docs:
    if d["kind"] != "Deployment" or "console" not in d["metadata"]["name"]:
        continue
    consoles += 1
    name = d["metadata"]["name"]
    c = d["spec"]["template"]["spec"]["containers"][0]
    # A key is delivered only if its NAME is a legal env var name. The kubelet drops the rest
    # silently, so a key that merely appears in the YAML is not evidence that it arrives.
    seen = set(e["name"] for e in c.get("env", []))
    for src in c.get("envFrom", []):
        if "configMapRef" in src:
            seen |= cms.get(src["configMapRef"]["name"], set())
        elif "secretRef" in src:
            problems.append(name + " uses envFrom.secretRef: Secret KEYS become env var NAMES, "
                            "and a hyphenated key is skipped without error. Use secretKeyRef.")
    missing = sorted(required - seen)
    if missing:
        problems.append(name + " never receives: " + ", ".join(missing))
    illegal = sorted(n for n in seen if not n.replace("_", "").isalnum() or n[:1].isdigit())
    if illegal:
        problems.append(name + " has illegal env var name(s): " + ", ".join(illegal))
if consoles == 0:
    problems.append("no console Deployment rendered")
print("|".join(problems) if problems
      else "OK {0} console(s), all {1} keys delivered".format(consoles, len(required)))')"
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
