#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# oidc.caBundle (SMA-513 PR 3, spec § 5). The value lets every pod trust an IdP whose chain ends at
# a private CA. One row per property:
#   set, set-custom-key, set-subpath-key
#                   each of the three Deployments mounts ONE key of the ConfigMap read-only through
#                   `items` (so a missing key stops the pod at volume setup, before Node can start
#                   with only a warning), gets its own env key in the POD env, and no version
#                   annotation while version is unset.
#   unset, null-cabundle
#                   nothing of it renders. render.sh's golden files pin the bytes; this row names
#                   the property. `null-cabundle` is `helm upgrade --reuse-values` from a release
#                   made before the value existed (Review Focus 1).
#   version-strings, version-numbers
#                   a version change restarts all three pods (every spec.template differs), and a
#                   NUMERIC version (an int64 from --set) still renders (Review Focus 1).
# Every row runs after a failure. python3 must have PyYAML, as for env.sh; repo:helm-render puts
# its venv first on PATH. Use only DOUBLE quotes inside the python3 blocks.
set -euo pipefail
export PROTO_REPORTER=text
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHART="$(cd "$HERE/.." && pwd)"
# The eighth copy of the required values (ci/helm-render/README.md, residual risk 4). The gateway
# zone is ON, so each row sees all three Deployments.
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
  --set zones.gateway.enabled=true
  "$@"
)
ec=0

# The "${X[@]+...}" guards below are load-bearing, not noise. MEASURED: bash 3.2.57
# treats "${A[@]}" on an EMPTY array as an unbound variable under `set -u`, so a
# no-argument run would abort before the first row. bash 5.x does not. Some gates in
# this repo run under 3.2.

# Renders go to files, not through a pipe: a Linux runner holds the whole render in its pipe, a
# 512-byte host pipe does not (ci/helm-render/README.md, residual risk 5).
TMP="$(mktemp -d "${TMPDIR:-/tmp}/ca-bundle.XXXXXX")"
trap 'rm -rf "$TMP"' EXIT

# render <name> <helm args...>: writes $TMP/<name>.yaml. On failure it reports, sets ec=1 and
# returns 1, and the caller skips the rest of its row.
render() {
  local name="$1"; shift
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" "$@" >"$TMP/$name.yaml" 2>"$TMP/$name.err"; then
    echo "FAIL [$name]: render failed"; cat "$TMP/$name.err"; ec=1; return 1
  fi
}

# verdict <label> <set|unset> <key> <render file>
verdict() {
  local label="$1" mode="$2" key="$3" file="$4" out
  if ! out="$(MODE="$mode" KEY="$key" python3 -c '
import os, sys, yaml
mode, key = os.environ["MODE"], os.environ["KEY"]
mount = "/etc/paigasus/idp-ca"
with open(sys.argv[1]) as fh:
    raw = fh.read()
docs = [d for d in yaml.safe_load_all(raw) if d]
deps = {}
for d in docs:
    if d.get("kind") == "Deployment":
        deps[d["spec"]["template"]["metadata"]["labels"]["app.kubernetes.io/name"]] = d
env_of = {"iam-console": "NODE_EXTRA_CA_CERTS", "gateway-console": "NODE_EXTRA_CA_CERTS", "iam-backend": "IAM_AUTHN__EXTRA_CA_BUNDLE_PATH"}
problems = []
if sorted(deps) != sorted(env_of):
    problems.append("Deployments are [" + ", ".join(sorted(deps)) + "], want [" + ", ".join(sorted(env_of)) + "]")
for name in sorted(env_of):
    d = deps.get(name)
    if d is None:
        continue
    tpl = d["spec"]["template"]
    pod = tpl["spec"]
    c = pod["containers"][0]
    vols = [v for v in pod.get("volumes") or [] if v.get("name") == "idp-ca"]
    mounts = [m for m in c.get("volumeMounts") or [] if m.get("name") == "idp-ca"]
    envs = [e for e in c.get("env") or [] if e.get("name") in ("NODE_EXTRA_CA_CERTS", "IAM_AUTHN__EXTRA_CA_BUNDLE_PATH")]
    if mode == "set":
        want_vol = [{"name": "idp-ca", "configMap": {"name": "paigasus-idp-ca", "items": [{"key": key, "path": key}]}}]
        want_mount = [{"name": "idp-ca", "mountPath": mount, "readOnly": True}]
        want_env = [{"name": env_of[name], "value": mount + "/" + key}]
        if vols != want_vol:
            problems.append(name + ": volume is " + repr(vols) + ", want " + repr(want_vol))
        if mounts != want_mount:
            problems.append(name + ": volumeMount is " + repr(mounts) + ", want " + repr(want_mount))
        if envs != want_env:
            problems.append(name + ": CA env is " + repr(envs) + ", want " + repr(want_env))
    elif vols or mounts or envs:
        problems.append(name + ": renders " + repr(vols + mounts + envs) + " with oidc.caBundle unset")
    if "checksum/idp-ca" in (tpl["metadata"].get("annotations") or {}):
        problems.append(name + ": checksum/idp-ca renders, but oidc.caBundle.version is unset")
for d in docs:
    if d.get("kind") == "ConfigMap" and "NODE_EXTRA_CA_CERTS" in (d.get("data") or {}):
        problems.append(d["metadata"]["name"] + " carries NODE_EXTRA_CA_CERTS; it belongs in the pod env")
if mode == "unset":
    for needle in ("idp-ca", "NODE_EXTRA_CA_CERTS", "EXTRA_CA_BUNDLE"):
        if needle in raw:
            problems.append("the render contains " + needle)
print("|".join(problems) if problems else "OK")' "$file" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

# differ <file 1> <file 2>: all three pod templates must differ (the check-3 style).
differ() {
  python3 -c '
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
problems += [n + ": spec.template is equal; it must differ" for n in want if n in a and n in b and a[n] == b[n]]
for n in want:
    if n in b and "checksum/idp-ca" not in (b[n]["metadata"].get("annotations") or {}):
        problems.append(n + ": no checksum/idp-ca annotation while oidc.caBundle.version is set")
print("|".join(problems) if problems else "OK")' "$1" "$2"
}

row_set() {  # row_set <label> <key> <helm args...>
  local label="$1" key="$2"; shift 2
  render "$label" "$@" || return 0
  verdict "$label" set "$key" "$TMP/$label.yaml"
}

row_unset() {  # row_unset <label> [helm args...]
  local label="$1"; shift
  render "$label" "$@" || return 0
  verdict "$label" unset ca.crt "$TMP/$label.yaml"
}

row_version() {  # row_version <label> <version 1> <version 2>
  local label="$1" v1="$2" v2="$3" out
  render "$label-1" --set oidc.caBundle.existingConfigMap=paigasus-idp-ca --set "oidc.caBundle.version=$v1" || return 0
  render "$label-2" --set oidc.caBundle.existingConfigMap=paigasus-idp-ca --set "oidc.caBundle.version=$v2" || return 0
  if ! out="$(differ "$TMP/$label-1.yaml" "$TMP/$label-2.yaml" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

row_set "set" ca.crt --set oidc.caBundle.existingConfigMap=paigasus-idp-ca
row_set "set-custom-key" bundle.pem --set oidc.caBundle.existingConfigMap=paigasus-idp-ca --set oidc.caBundle.key=bundle.pem
row_set "set-subpath-key" certs/idp.pem --set oidc.caBundle.existingConfigMap=paigasus-idp-ca --set oidc.caBundle.key=certs/idp.pem
row_unset "unset"
row_unset "null-cabundle" --set oidc.caBundle=null
row_version "version-strings" v1 v2
row_version "version-numbers" 1 2

if [ "$ec" -eq 0 ]; then echo "== chart ca-bundle OK =="; fi
exit "$ec"
