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
#
# The script also checks the IAM backend's IAM_AUTHN__ISSUERS value (SMA-678, check_audience).
# One row per property of oidc.audience:
#   A1 unset        the audience is oidc.clientId, and the "oidc.audience is set" comment is absent.
#   A2 reuse-values-no-key
#                   `helm upgrade --reuse-values` from a release made before the key existed: the
#                   template reads nil, and the audience is still oidc.clientId.
#   A3 set          the value REPLACES oidc.clientId. The exact compare proves a list of one.
#   A4 number       an int64 from --set renders as the string "12345", not a rune literal.
#   A5 number-in-file
#                   a number in a values file (a float64) renders as the string "12345" too.
#   A6 restart-scope
#                   a change of the value changes the IAM pod template and no console pod template.
# A row counter reds the script when a row call line is deleted.
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

# oidc.audience (SMA-678). Renders go to a file, not through a pipe: a Linux runner holds the whole
# render in its pipe, a 512-byte host pipe does not (ci/helm-render/README.md, residual risk 5).
TMP="$(mktemp -d "${TMPDIR:-/tmp}/env.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT
AUDIENCE_ROWS=0
AUDIENCE_ROWS_WANT=6

# check_audience <label> <expected audience> <present|absent> [helm args...]
# The third argument is the state of the "oidc.audience is set" YAML comment line.
check_audience() {
  local label="$1" want="$2" comment="$3"; shift 3
  local out
  AUDIENCE_ROWS=$((AUDIENCE_ROWS + 1))
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" "$@" >"$TMP/audience.yaml" 2>"$TMP/audience.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/audience.err"; ec=1; return 0
  fi
  if ! out="$(WANT="$want" COMMENT="$comment" python3 -c '
import os, sys, yaml
want = "[{issuer=\"https://idp.example.test/realms/paigasus\",audiences=[\"" + os.environ["WANT"] + "\"]}]"
line = "            # oidc.audience is set: IAM accepts that audience, not the client id."
with open(sys.argv[1]) as fh:
    raw = fh.read()
docs = [d for d in yaml.safe_load_all(raw) if d]
problems = []
deps = [d for d in docs if d.get("kind") == "Deployment"
        and d["spec"]["template"]["metadata"]["labels"].get("app.kubernetes.io/name") == "iam-backend"]
if len(deps) != 1:
    problems.append(str(len(deps)) + " iam-backend Deployment(s), want 1")
else:
    env = deps[0]["spec"]["template"]["spec"]["containers"][0].get("env") or []
    issuers = [e for e in env if e.get("name") == "IAM_AUTHN__ISSUERS"]
    if len(issuers) != 1:
        problems.append(str(len(issuers)) + " IAM_AUTHN__ISSUERS entries, want 1")
    elif issuers[0].get("value") != want:
        problems.append("IAM_AUTHN__ISSUERS is " + repr(issuers[0].get("value")) + ", want " + repr(want))
count = raw.splitlines().count(line)
if os.environ["COMMENT"] == "present" and count != 1:
    problems.append("the oidc.audience comment line renders " + str(count) + " time(s), want 1")
if os.environ["COMMENT"] == "absent" and count != 0:
    problems.append("the oidc.audience comment line renders " + str(count) + " time(s), want 0")
print("|".join(problems) if problems else "OK")' "$TMP/audience.yaml" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

# check_audience_restart <label>: a change of oidc.audience restarts the IAM pod and no console pod
# (docs/ops/RUNBOOK-chart.md § 5). The gateway zone is on, so both consoles are in the render.
check_audience_restart() {
  local label="$1" out
  AUDIENCE_ROWS=$((AUDIENCE_ROWS + 1))
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" --set zones.gateway.enabled=true >"$TMP/restart-1.yaml" 2>"$TMP/restart.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/restart.err"; ec=1; return 0
  fi
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" --set zones.gateway.enabled=true --set oidc.audience=api://default >"$TMP/restart-2.yaml" 2>"$TMP/restart.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/restart.err"; ec=1; return 0
  fi
  if ! out="$(python3 -c '
import sys, yaml
def templates(path):
    with open(path) as fh:
        docs = [d for d in yaml.safe_load_all(fh) if d]
    return {d["spec"]["template"]["metadata"]["labels"]["app.kubernetes.io/name"]: d["spec"]["template"] for d in docs if d.get("kind") == "Deployment"}
a, b = templates(sys.argv[1]), templates(sys.argv[2])
want = ["gateway-console", "iam-backend", "iam-console"]
problems = []
if sorted(a) != want or sorted(b) != want:
    problems.append("Deployments are " + repr(sorted(a)) + " and " + repr(sorted(b)) + ", want " + repr(want))
elif a["iam-backend"] == b["iam-backend"]:
    problems.append("iam-backend: spec.template is equal; it must differ")
problems += [n + ": spec.template differs; it must be equal" for n in ("gateway-console", "iam-console") if n in a and n in b and a[n] != b[n]]
print("|".join(problems) if problems else "OK")' "$TMP/restart-1.yaml" "$TMP/restart-2.yaml" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

# A5: a number in a VALUES FILE is a float64, not the int64 that --set gives (Review Focus 1).
printf 'oidc:\n  audience: 12345\n' >"$TMP/audience-number.yaml"

check_audience "A1 unset"               paigasus-console absent
check_audience "A2 reuse-values-no-key" paigasus-console absent  --set oidc.audience=null
check_audience "A3 set"                 api://default    present --set oidc.audience=api://default
check_audience "A4 number"              12345            present --set oidc.audience=12345
check_audience "A5 number-in-file"      12345            present -f "$TMP/audience-number.yaml"
check_audience_restart "A6 restart-scope"

if [ "$AUDIENCE_ROWS" -lt "$AUDIENCE_ROWS_WANT" ]; then
  echo "FAIL [audience rows]: $AUDIENCE_ROWS check_audience row(s) ran, want $AUDIENCE_ROWS_WANT"; ec=1
fi

if [ "$ec" -eq 0 ]; then echo "== chart env OK =="; fi
exit "$ec"
