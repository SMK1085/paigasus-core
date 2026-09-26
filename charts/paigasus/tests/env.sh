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
#   A1 unset        The audience is oidc.clientId. The "oidc.audience is set" comment is absent.
#   A2 reuse-values-no-key
#                   This is `helm upgrade --reuse-values` from a release made before the key
#                   existed. The template reads nil. The audience is still oidc.clientId.
#   A3 set          The value REPLACES oidc.clientId. The exact compare proves a list of one.
#   A4 number       An int64 from --set renders as the string "12345". It is not a rune literal.
#   A5 number-in-file
#                   A number in a values file is a float64. It also renders as the string "12345".
#   A6 restart-scope
#                   A change of the value changes the IAM pod template. It does not change a
#                   console pod template.
# A row counter reds the script when a row call line is deleted.
#
# The script also checks the SMA-691 warning annotation paigasus.io/iam-audience-warning on the
# IAM backend Deployment (check_warning, check_warning_restart). One row per input:
#   W1 default      The annotation is present. The key occurs once in the whole render.
#   W2 reuse-values-no-key
#                   `--set oidc.acknowledgeClientIdAudience=null`. The template reads nil. Present.
#   W3 explicit-equal
#                   oidc.audience is set to the client id. Present.
#   W4 distinct     oidc.audience differs from the client id. Absent.
#   W5 acknowledged The acknowledgement equals the client id. Absent.
#   W6 bool-true    The acknowledgement is the bool true. Present.
#   W7 string-false The acknowledgement is the string "false". Present.
#   W8 stale-ack    The acknowledgement names another client id. Present.
#   W9 pod-template The key is not in spec.template.metadata.annotations.
#   W10 number-in-file
#                   The acknowledgement 12345 from a values file (a float64) equals the client id
#                   12345. Absent.
#   W11 large-number-in-file
#                   The acknowledgement 1000000 from a values file renders as "1e+06". It does not
#                   equal the client id "1000000". Present.
#   W12 case-differs
#                   The compare is exact. PAIGASUS-CONSOLE does not acknowledge paigasus-console.
#   W13 gateway-on  The gateway zone is on. The key still occurs once.
#   W14 no-restart  The acknowledgement changes no pod template, so it restarts no pod.
# A second row counter reds the script when a W row call line is deleted.
#
# The script also checks the SMA-691 NOTES text. No offline helm command prints NOTES, so a copy
# of the chart renders the bytes of NOTES.txt through a probe ConfigMap (check_notes_pin,
# check_notes):
#   N0 pin          The bytes of templates/NOTES.txt equal the three lines of the spec.
#   N1 default      The NOTES body, with the audience "paigasus-console".
#   N2 explicit-equal
#                   oidc.audience is set to the client id. The same body.
#   N3 distinct     oidc.audience differs from the client id. Empty.
#   N4 acknowledged The acknowledgement equals the client id. Empty.
#   N5 stale-ack    The acknowledgement names another client id. The body.
#   N6 other-client The client id is not "paigasus-console". The body, with that audience.
# A third row counter reds the script when an N row call line is deleted.
#
# The script also checks the SMA-692 console OIDC values in the console-env ConfigMap
# (check_console_oidc, check_console_oidc_restart):
#   O1 unset        PAIGASUS_OIDC_SCOPES and PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE are absent,
#                   and so are their comment lines.
#   O2 both-set     Both keys hold the exact values.
#   O3 reuse-values-no-key
#                   `--set oidc.scopes=null --set oidc.authorizationAudience=null`. The render
#                   succeeds, both keys are absent, and the D7 check does not fail.
#   O4 number       oidc.audience and oidc.authorizationAudience are both the number 123. The
#                   render succeeds, and the key holds the string "123".
#   O5 restart-scopes
#                   A change of oidc.scopes changes both console pod templates, not the IAM one.
#   O6 restart-audience
#                   A change of oidc.authorizationAudience does the same.
# A fourth row counter reds the script when an O row call line is deleted.
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

# oidc.audience (SMA-678). Renders go to a file. They do not go through a pipe.
# A Linux runner's pipe holds the whole render. A 512-byte host pipe does not.
# See ci/helm-render/README.md, residual risk 5.
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

# check_audience_restart <label>
# A change of oidc.audience restarts the IAM pod. It does not restart a console pod.
# See docs/ops/RUNBOOK-chart.md § 5. The gateway zone is on. Both consoles are in the render.
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

# A5: a number in a VALUES FILE is a float64. --set gives an int64 instead. See Review Focus 1.
printf 'oidc:\n  audience: 12345\n' >"$TMP/audience-number.yaml"

check_audience "A1 unset"               paigasus-console absent
check_audience "A2 reuse-values-no-key" paigasus-console absent  --set oidc.audience=null
check_audience "A3 set"                 api://default    present --set oidc.audience=api://default
check_audience "A4 number"              12345            present --set oidc.audience=12345
check_audience "A5 number-in-file"      12345            present -f "$TMP/audience-number.yaml"
check_audience_restart "A6 restart-scope"

if [ "$AUDIENCE_ROWS" -lt "$AUDIENCE_ROWS_WANT" ]; then
  echo "FAIL [audience rows]: $AUDIENCE_ROWS audience row(s) ran, want $AUDIENCE_ROWS_WANT"; ec=1
fi

# The IAM audience warning annotation (SMA-691). Renders go to a file, as for check_audience.
WARNING_ROWS=0
WARNING_ROWS_WANT=14

# check_warning <label> <present|absent|pod-template> [helm args...]
#   present       The IAM backend Deployment's metadata.annotations holds the key with the exact
#                 value. The key occurs exactly once in the whole render.
#   absent        The IAM backend Deployment exists. The key occurs nowhere in the render.
#   pod-template  The IAM backend Deployment exists. Its spec.template.metadata.annotations does
#                 not hold the key. A key there restarts the pod on every change.
check_warning() {
  local label="$1" mode="$2"; shift 2
  local out
  WARNING_ROWS=$((WARNING_ROWS + 1))
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" "$@" >"$TMP/warning.yaml" 2>"$TMP/warning.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/warning.err"; ec=1; return 0
  fi
  if ! out="$(MODE="$mode" python3 -c '
import os, sys, yaml
key = "paigasus.io/iam-audience-warning"
value = ("the IAM audience equals oidc.clientId, so an ID token passes IAM" + chr(39)
         + "s audience check. See docs/ops/RUNBOOK-chart.md section 6 (SMA-691).")
with open(sys.argv[1]) as fh:
    raw = fh.read()
docs = [d for d in yaml.safe_load_all(raw) if d]
problems = []
deps = [d for d in docs if d.get("kind") == "Deployment"
        and d["spec"]["template"]["metadata"]["labels"].get("app.kubernetes.io/name") == "iam-backend"]
count = raw.count(key)
mode = os.environ["MODE"]
if len(deps) != 1:
    problems.append(str(len(deps)) + " iam-backend Deployment(s), want 1")
elif mode == "present":
    got = (deps[0]["metadata"].get("annotations") or {}).get(key)
    if got != value:
        problems.append("metadata.annotations[" + key + "] is " + repr(got) + ", want " + repr(value))
    if count != 1:
        problems.append("the key occurs " + str(count) + " time(s) in the render, want 1")
elif mode == "absent":
    if count != 0:
        problems.append("the key occurs " + str(count) + " time(s) in the render, want 0")
elif mode == "pod-template":
    if key in (deps[0]["spec"]["template"]["metadata"].get("annotations") or {}):
        problems.append("spec.template.metadata.annotations holds " + key + ", want it absent")
else:
    problems.append("unknown mode " + repr(mode))
print("|".join(problems) if problems else "OK")' "$TMP/warning.yaml" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

# check_warning_restart <label>
# The acknowledgement changes only Deployment metadata. It does not change a pod template, so it
# restarts no pod. The gateway zone is on, so all three Deployments are in both renders.
check_warning_restart() {
  local label="$1" out
  WARNING_ROWS=$((WARNING_ROWS + 1))
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" --set zones.gateway.enabled=true >"$TMP/warn-restart-1.yaml" 2>"$TMP/warn-restart.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/warn-restart.err"; ec=1; return 0
  fi
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" --set zones.gateway.enabled=true --set oidc.acknowledgeClientIdAudience=paigasus-console >"$TMP/warn-restart-2.yaml" 2>"$TMP/warn-restart.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/warn-restart.err"; ec=1; return 0
  fi
  if ! out="$(python3 -c '
import sys, yaml
key = "paigasus.io/iam-audience-warning"
def deps(path):
    with open(path) as fh:
        docs = [d for d in yaml.safe_load_all(fh) if d]
    return {d["spec"]["template"]["metadata"]["labels"]["app.kubernetes.io/name"]: d for d in docs if d.get("kind") == "Deployment"}
a, b = deps(sys.argv[1]), deps(sys.argv[2])
want = ["gateway-console", "iam-backend", "iam-console"]
problems = []
if sorted(a) != want or sorted(b) != want:
    problems.append("Deployments are " + repr(sorted(a)) + " and " + repr(sorted(b)) + ", want " + repr(want))
else:
    if key not in (a["iam-backend"]["metadata"].get("annotations") or {}):
        problems.append("the first render has no " + key + ", so the row cannot decide anything")
    if key in (b["iam-backend"]["metadata"].get("annotations") or {}):
        problems.append("the acknowledged render still has " + key)
    problems += [n + ": spec.template differs; it must be equal" for n in want if a[n]["spec"]["template"] != b[n]["spec"]["template"]]
print("|".join(problems) if problems else "OK")' "$TMP/warn-restart-1.yaml" "$TMP/warn-restart-2.yaml" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

# W10 and W11: a number in a VALUES FILE is a float64. --set gives an int64 or a string.
printf 'oidc:\n  acknowledgeClientIdAudience: 12345\n' >"$TMP/ack-number.yaml"
printf 'oidc:\n  acknowledgeClientIdAudience: 1000000\n' >"$TMP/ack-large-number.yaml"

check_warning "W1 default"               present
check_warning "W2 reuse-values-no-key"   present      --set oidc.acknowledgeClientIdAudience=null
check_warning "W3 explicit-equal"        present      --set oidc.audience=paigasus-console
check_warning "W4 distinct"              absent       --set oidc.audience=api://paigasus
check_warning "W5 acknowledged"          absent       --set oidc.acknowledgeClientIdAudience=paigasus-console
check_warning "W6 bool-true"             present      --set oidc.acknowledgeClientIdAudience=true
check_warning "W7 string-false"          present      --set-string oidc.acknowledgeClientIdAudience=false
check_warning "W8 stale-ack"             present      --set oidc.acknowledgeClientIdAudience=old-client
check_warning "W9 pod-template"          pod-template
check_warning "W10 number-in-file"       absent       --set oidc.clientId=12345 -f "$TMP/ack-number.yaml"
check_warning "W11 large-number-in-file" present      --set-string oidc.clientId=1000000 -f "$TMP/ack-large-number.yaml"
check_warning "W12 case-differs"         present      --set oidc.acknowledgeClientIdAudience=PAIGASUS-CONSOLE
check_warning "W13 gateway-on"           present      --set zones.gateway.enabled=true
check_warning_restart "W14 no-restart"

if [ "$WARNING_ROWS" -lt "$WARNING_ROWS_WANT" ]; then
  echo "FAIL [warning rows]: $WARNING_ROWS warning row(s) ran, want $WARNING_ROWS_WANT"; ec=1
fi

# The NOTES text (SMA-691). No offline helm command prints NOTES: `helm install --dry-run` needs
# a cluster. So a copy of the chart gets a probe. The probe wraps the bytes of NOTES.txt in a
# named template and renders it into a ConfigMap. N0 pins those bytes.
NOTES_ROWS=0
NOTES_ROWS_WANT=7
NOTES_CHART="$TMP/notes-chart"

# check_notes_pin <label>
check_notes_pin() {
  local label="$1" out
  NOTES_ROWS=$((NOTES_ROWS + 1))
  if ! out="$(python3 -c '
import sys
want = ("{{- /* SPDX-License-Identifier: Apache-2.0 */ -}}\n"
        "{{- include \"paigasus.validate\" . -}}\n"
        "{{- include \"paigasus.iamAudienceNotes\" . -}}\n")
with open(sys.argv[1]) as fh:
    got = fh.read()
print("OK" if got == want else "templates/NOTES.txt is " + repr(got) + ", want " + repr(want))' "$CHART/templates/NOTES.txt" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

# check_notes <label> <body|empty> <audience> [helm args...]
check_notes() {
  local label="$1" want="$2" audience="$3"; shift 3
  local out
  NOTES_ROWS=$((NOTES_ROWS + 1))
  if ! helm template paigasus "$NOTES_CHART" "${BASE[@]+"${BASE[@]}"}" "$@" \
      --show-only templates/zz-notes-probe.yaml >"$TMP/notes.yaml" 2>"$TMP/notes.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/notes.err"; ec=1; return 0
  fi
  if ! out="$(WANT="$want" AUDIENCE="$audience" python3 -c '
import os, sys, yaml
body = "\n".join([
    "WARNING (SMA-691): the IAM audience equals oidc.clientId, so an ID token passes IAM" + chr(39) + "s audience check.",
    "IAM accepts the audience \"" + os.environ["AUDIENCE"] + "\". An OIDC ID token has the client id as its audience.",
    "IAM refuses a Keycloak ID token by its typ claim (SMA-686). Dex does not set that claim.",
    "Other IdPs are not measured.",
    "Recommended: give the API its own audience and set oidc.audience to it.",
    "Follow the order in docs/ops/RUNBOOK-chart.md section 6, or every session breaks.",
    "If your IdP cannot do this (Dex), set oidc.acknowledgeClientIdAudience to the value of",
    "oidc.clientId to remove this warning.",
])
want = body if os.environ["WANT"] == "body" else ""
with open(sys.argv[1]) as fh:
    docs = [d for d in yaml.safe_load_all(fh) if d]
cms = [d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"] == "notes-probe"]
if len(cms) != 1:
    print(str(len(cms)) + " notes-probe ConfigMap(s), want 1")
else:
    got = (cms[0].get("data") or {}).get("notes")
    print("OK" if got == want else "NOTES is " + repr(got) + ", want " + repr(want))' "$TMP/notes.yaml" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

if cp -R "$CHART" "$NOTES_CHART" \
  && { printf '%s\n' '{{- define "paigasus.notesProbe" -}}' && cat "$CHART/templates/NOTES.txt" \
    && printf '%s\n' '{{- end -}}'; } >"$NOTES_CHART/templates/_zz-notes-probe.tpl" \
  && printf '%s\n' 'apiVersion: v1' 'kind: ConfigMap' 'metadata:' '  name: notes-probe' 'data:' \
    '  notes: {{ include "paigasus.notesProbe" . | quote }}' >"$NOTES_CHART/templates/zz-notes-probe.yaml"; then
  :
else
  echo "FAIL [notes probe]: cannot build the probe chart under $NOTES_CHART"; ec=1
fi

check_notes_pin "N0 pin"
check_notes "N1 default"        body  paigasus-console
check_notes "N2 explicit-equal" body  paigasus-console --set oidc.audience=paigasus-console
check_notes "N3 distinct"       empty paigasus-console --set oidc.audience=api://paigasus
check_notes "N4 acknowledged"   empty paigasus-console --set oidc.acknowledgeClientIdAudience=paigasus-console
check_notes "N5 stale-ack"      body  paigasus-console --set oidc.acknowledgeClientIdAudience=old-client
check_notes "N6 other-client"   body  other-client      --set oidc.clientId=other-client

if [ "$NOTES_ROWS" -lt "$NOTES_ROWS_WANT" ]; then
  echo "FAIL [notes rows]: $NOTES_ROWS notes row(s) ran, want $NOTES_ROWS_WANT"; ec=1
fi

# The console OIDC values (SMA-692). Renders go to a file, as for check_audience.
OIDC_ROWS=0
OIDC_ROWS_WANT=6
API_SCOPES="openid profile email offline_access api://paigasus/access"

# check_console_oidc <label> <scopes|-> <authorization audience|-> [helm args...]
# "-" means the key, and its comment line, must be absent from the whole render.
check_console_oidc() {
  local label="$1" scopes="$2" aud="$3"; shift 3
  local out
  OIDC_ROWS=$((OIDC_ROWS + 1))
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" "$@" >"$TMP/oidc.yaml" 2>"$TMP/oidc.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/oidc.err"; ec=1; return 0
  fi
  if ! out="$(SCOPES="$scopes" AUD="$aud" python3 -c '
import os, sys, yaml
want = {"PAIGASUS_OIDC_SCOPES": os.environ["SCOPES"], "PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE": os.environ["AUD"]}
marker = {"PAIGASUS_OIDC_SCOPES": "oidc.scopes is set", "PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE": "oidc.authorizationAudience is set"}
with open(sys.argv[1]) as fh:
    raw = fh.read()
docs = [d for d in yaml.safe_load_all(raw) if d]
cms = [d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"].endswith("-console-env")]
problems = []
if len(cms) != 1:
    problems.append(str(len(cms)) + " console-env ConfigMap(s), want 1")
else:
    data = cms[0].get("data") or {}
    for key in sorted(want):
        if want[key] == "-":
            if key in raw or marker[key] in raw:
                problems.append(key + " or its comment is in the render, want it absent")
        elif data.get(key) != want[key]:
            problems.append(key + " is " + repr(data.get(key)) + ", want " + repr(want[key]))
print("|".join(problems) if problems else "OK")' "$TMP/oidc.yaml" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

# check_console_oidc_restart <label> <scopes|audience>
# The second render changes only the named value. Both console pod templates must differ from
# the first render. The IAM pod template must be equal. The gateway zone is on.
check_console_oidc_restart() {
  local label="$1" mode="$2" out
  local first second
  if [ "$mode" = "scopes" ]; then
    first=(--set zones.gateway.enabled=true)
    second=(--set zones.gateway.enabled=true --set "oidc.scopes=$API_SCOPES")
  else
    first=(--set zones.gateway.enabled=true --set oidc.audience=api://paigasus)
    second=(--set zones.gateway.enabled=true --set oidc.audience=api://paigasus --set oidc.authorizationAudience=api://paigasus)
  fi
  OIDC_ROWS=$((OIDC_ROWS + 1))
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" "${first[@]}" >"$TMP/oidc-restart-1.yaml" 2>"$TMP/oidc-restart.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/oidc-restart.err"; ec=1; return 0
  fi
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" "${second[@]}" >"$TMP/oidc-restart-2.yaml" 2>"$TMP/oidc-restart.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/oidc-restart.err"; ec=1; return 0
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
else:
    problems += [n + ": spec.template is equal; it must differ" for n in ("gateway-console", "iam-console") if a[n] == b[n]]
    if a["iam-backend"] != b["iam-backend"]:
        problems.append("iam-backend: spec.template differs; it must be equal")
print("|".join(problems) if problems else "OK")' "$TMP/oidc-restart-1.yaml" "$TMP/oidc-restart-2.yaml" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

check_console_oidc "O1 unset"              - -
check_console_oidc "O2 both-set"           "$API_SCOPES" api://paigasus \
  --set "oidc.scopes=$API_SCOPES" --set oidc.audience=api://paigasus --set oidc.authorizationAudience=api://paigasus
check_console_oidc "O3 reuse-values-no-key" - - --set oidc.scopes=null --set oidc.authorizationAudience=null
check_console_oidc "O4 number"             - 123 --set oidc.audience=123 --set oidc.authorizationAudience=123
check_console_oidc_restart "O5 restart-scopes"   scopes
check_console_oidc_restart "O6 restart-audience" audience

if [ "$OIDC_ROWS" -lt "$OIDC_ROWS_WANT" ]; then
  echo "FAIL [oidc rows]: $OIDC_ROWS oidc row(s) ran, want $OIDC_ROWS_WANT"; ec=1
fi

if [ "$ec" -eq 0 ]; then echo "== chart env OK =="; fi
exit "$ec"
