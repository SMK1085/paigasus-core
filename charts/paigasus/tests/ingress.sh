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

# The "${X[@]+...}" guards below are load-bearing, not noise. MEASURED: bash 3.2.57
# treats "${A[@]}" on an EMPTY array as an unbound variable under `set -u`, so a
# no-argument run would abort before the first row. bash 5.x does not. Some gates in
# this repo run under 3.2.

coupling() {
  local label="$1" want="$2"; shift 2
  local out got
  # Capture, then test the variable. Re-running a failing helm template bare would abort the
  # whole script under set -e and cancel every row after it.
  if ! out="$(helm template t "$CHART" "${BASE[@]+"${BASE[@]}"}" "$@" 2>&1)"; then
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

# SMA-695. ingress.enabled=false renders no Ingress. Every enabled zone keeps its console Service
# (port name http, port 3000), because the operator's own route targets it. PAIGASUS_ZONES and
# PAIGASUS_SERVICES must still hold exactly the enabled zones. A separate function from
# `coupling`, which compares Ingress paths with the zone map and so needs an Ingress.
no_ingress() {
  local label="$1" want="$2"; shift 2
  local out got
  if ! out="$(helm template t "$CHART" "${BASE[@]+"${BASE[@]}"}" --set ingress.enabled=false "$@" 2>&1)"; then
    echo "FAIL [$label]: expected a successful render"; printf '%s\n' "$out"; ec=1; return
  fi
  got="$(printf '%s' "$out" | python3 -c '
import sys,yaml,json
want=sorted(sys.argv[1].split(","))
docs=[d for d in yaml.safe_load_all(sys.stdin) if d]
problems=[]
if any(d["kind"]=="Ingress" for d in docs):
    problems.append("an Ingress is rendered")
cm=[d for d in docs if d["kind"]=="ConfigMap" and d["metadata"]["name"].endswith("-zonemap")][0]
zones=sorted(json.loads(cm["data"]["PAIGASUS_ZONES"]))
services=sorted(json.loads(cm["data"]["PAIGASUS_SERVICES"]))
if zones!=want or services!=want:
    problems.append("zones=%s services=%s, want %s" % (zones,services,want))
for z in want:
    svc=[d for d in docs if d["kind"]=="Service" and d["metadata"]["name"].endswith("-%s-console" % z)]
    ports=[p for s in svc for p in s["spec"]["ports"] if p.get("name")=="http" and p.get("port")==3000]
    if len(svc)!=1 or len(ports)!=1:
        problems.append("zone %s: want one console Service with port http/3000, got %d Service(s)" % (z,len(svc)))
print("; ".join(problems) if problems else "NO-INGRESS-OK")' "$want")"
  if [ "$got" = "NO-INGRESS-OK" ]; then
    echo "  ok [$label]: no Ingress; console Services for $want"
  else
    echo "FAIL [$label]: $got"; ec=1
  fi
}

# SMA-695. A release made before ingress.enabled existed has no such key under
# `helm upgrade --reuse-values`. `--set ingress.enabled=null` deletes the key (measured on helm
# 3.22.0), which gives that shape. The Ingress must stay: a plain .Values.ingress.enabled reads nil
# there and would delete the live Ingress.
ingress_count() {
  local label="$1" want="$2"; shift 2
  local out n
  if ! out="$(helm template t "$CHART" "${BASE[@]+"${BASE[@]}"}" "$@" 2>&1)"; then
    echo "FAIL [$label]: expected a successful render"; printf '%s\n' "$out"; ec=1; return
  fi
  n="$(printf '%s' "$out" | python3 -c '
import sys,yaml
print(sum(1 for d in yaml.safe_load_all(sys.stdin) if d and d["kind"]=="Ingress"))')"
  if [ "$n" = "$want" ]; then
    echo "  ok [$label]: $n Ingress"
  else
    echo "FAIL [$label]: $n Ingress, want $want"; ec=1
  fi
}

no_ingress "ingress disabled, iam only" "iam" --set zones.gateway.enabled=false
no_ingress "ingress disabled, iam and gateway" "iam,gateway" --set zones.gateway.enabled=true
ingress_count "ingress.enabled key absent" 1 --set ingress.enabled=null

# SMA-694. httpRoute.enabled=true renders one HTTPRoute per enabled zone, from the same zones range
# as the Ingress. ROUTE holds a valid parentRef, so the parentRefs refusal does not block a row.
ROUTE=(--set httpRoute.enabled=true --set 'httpRoute.parentRefs[0].name=gw'
  --set 'httpRoute.parentRefs[0].sectionName=https')

# H1-H3 (spec § 5.1). $want is the comma list of enabled zones. $absent is a word that must not
# appear in the render once the HTTPRoute apiVersion lines are removed (H2): every HTTPRoute
# renders `apiVersion: gateway.networking.k8s.io/v1`, so the raw render always holds "gateway".
# Do NOT weaken the needle to make H2 pass. Remove only the apiVersion line.
route_coupling() {
  local label="$1" want="$2" absent="$3"; shift 3
  local out got stripped
  if ! out="$(helm template t "$CHART" "${BASE[@]+"${BASE[@]}"}" "${ROUTE[@]}" "$@" 2>&1)"; then
    echo "FAIL [$label]: expected a successful render"; printf '%s\n' "$out"; ec=1; return
  fi
  got="$(printf '%s' "$out" | python3 -c '
import sys,yaml,json
want=sorted(sys.argv[1].split(","))
docs=[d for d in yaml.safe_load_all(sys.stdin) if d]
problems=[]
routes=[d for d in docs if d["kind"]=="HTTPRoute"]
cm=[d for d in docs if d["kind"]=="ConfigMap" and d["metadata"]["name"].endswith("-zonemap")][0]
zones=json.loads(cm["data"]["PAIGASUS_ZONES"])
if sorted(zones)!=want:
    problems.append("PAIGASUS_ZONES keys %s, want %s" % (sorted(zones),want))
if len(routes)!=len(want):
    problems.append("%d HTTPRoute(s), want %d" % (len(routes),len(want)))
paths=[]
for r in routes:
    n=r["metadata"]["name"]; spec=r["spec"]
    if r["apiVersion"]!="gateway.networking.k8s.io/v1":
        problems.append("%s: apiVersion %s" % (n,r["apiVersion"]))
    if spec.get("hostnames")!=["console.example.test"]:
        problems.append("%s: hostnames %s" % (n,spec.get("hostnames")))
    rules=spec.get("rules") or []
    if len(rules)!=1:
        problems.append("%s: %d rules, want 1" % (n,len(rules))); continue
    rule=rules[0]
    if "filters" in rule:
        problems.append("%s: the rule has filters" % n)
    m=rule.get("matches") or []
    if len(m)!=1 or m[0].get("path",{}).get("type")!="PathPrefix":
        problems.append("%s: want one PathPrefix match, got %s" % (n,m)); continue
    paths.append(m[0]["path"]["value"])
    refs=rule.get("backendRefs") or []
    if len(refs)!=1:
        problems.append("%s: %d backendRefs, want 1" % (n,len(refs))); continue
    svc=[d for d in docs if d["kind"]=="Service" and d["metadata"]["name"]==refs[0].get("name")]
    if len(svc)!=1:
        problems.append("%s: backendRef %s names no rendered Service" % (n,refs[0].get("name"))); continue
    if refs[0].get("port") not in [p.get("port") for p in svc[0]["spec"]["ports"]]:
        problems.append("%s: backendRef port %s is not a port of Service %s" % (n,refs[0].get("port"),svc[0]["metadata"]["name"]))
    sel=svc[0]["spec"]["selector"]
    deps=[d for d in docs if d["kind"]=="Deployment" and all(d["spec"]["template"]["metadata"]["labels"].get(k)==v for k,v in sel.items())]
    zone_env=[e.get("value") for d in deps for c in d["spec"]["template"]["spec"]["containers"] for e in c.get("env",[]) if e.get("name")=="PAIGASUS_ZONE"]
    if len(deps)!=1 or zones.get(zone_env[0] if zone_env else None)!=m[0]["path"]["value"]:
        problems.append("%s: path %s reaches the Deployment(s) of zone(s) %s" % (n,m[0]["path"]["value"],zone_env))
if sorted(paths)!=sorted(zones.values()):
    problems.append("route paths %s, want %s" % (sorted(paths),sorted(zones.values())))
print("; ".join(problems) if problems else "ROUTE-OK paths=%s" % ",".join(sorted(paths)))' "$want")"
  if [ "${got:0:8}" = "ROUTE-OK" ]; then
    echo "  ok [$label]: $got"
  else
    echo "FAIL [$label]: $got"; ec=1
  fi
  if [ -n "$absent" ]; then
    stripped="$(printf '%s\n' "$out" | grep -v '^apiVersion: gateway\.networking\.k8s\.io/' || true)"
    if grep -qiF -- "$absent" < <(printf '%s' "$stripped"); then
      echo "FAIL [$label]: disabled zone \"$absent\" appears in the rendered output"; ec=1
    fi
  fi
}

# H4-H8 (spec § 5.1). Prints, per HTTPRoute, "<zone>:<timeouts JSON or ->:<annotations JSON or ->",
# sorted, joined by one space, or "none". The zone comes from the selector of the Service behind
# the backendRef, so a swapped name cannot hide.
route_shape() {
  local label="$1" want="$2"; shift 2
  local out got
  if ! out="$(helm template t "$CHART" "${BASE[@]+"${BASE[@]}"}" "$@" 2>&1)"; then
    echo "FAIL [$label]: expected a successful render"; printf '%s\n' "$out"; ec=1; return
  fi
  got="$(printf '%s' "$out" | python3 -c '
import sys,yaml,json
docs=[d for d in yaml.safe_load_all(sys.stdin) if d]
rows=[]
for r in (d for d in docs if d["kind"]=="HTTPRoute"):
    rule=r["spec"]["rules"][0]
    svc=[d for d in docs if d["kind"]=="Service" and d["metadata"]["name"]==rule["backendRefs"][0]["name"]][0]
    zone=svc["spec"]["selector"]["app.kubernetes.io/name"].rsplit("-console",1)[0]
    t=json.dumps(rule["timeouts"],sort_keys=True,separators=(",",":")) if "timeouts" in rule else "-"
    a=json.dumps(r["metadata"]["annotations"],sort_keys=True,separators=(",",":")) if "annotations" in r["metadata"] else "-"
    rows.append("%s:%s:%s" % (zone,t,a))
print(" ".join(sorted(rows)) if rows else "none")')"
  if [ "$got" = "$want" ]; then
    echo "  ok [$label]: $got"
  else
    echo "FAIL [$label]: got \"$got\", want \"$want\""; ec=1
  fi
}

# H1, H2 (AC1-AC3).
route_coupling "route, iam only" "iam" "gateway" --set zones.gateway.enabled=false
route_coupling "route, iam and gateway" "iam,gateway" "" --set zones.gateway.enabled=true
# H3 (D3, AC7). Route mode with the Ingress off needs no TLS Secret.
route_coupling "route, ingress disabled, no TLS Secret" "iam,gateway" "" --set zones.gateway.enabled=true \
  --set ingress.enabled=false --set ingress.tlsSecretName=""
ingress_count "route, ingress disabled: no Ingress" 0 "${ROUTE[@]}" --set zones.gateway.enabled=true \
  --set ingress.enabled=false --set ingress.tlsSecretName=""
# H4 (AC5). A release made before this change has no httpRoute map.
route_shape "route, httpRoute key absent" "none" "${ROUTE[@]}" --set zones.gateway.enabled=true \
  --set httpRoute=null

if [ "$ec" -eq 0 ]; then echo "== chart ingress coupling OK =="; fi
exit "$ec"
