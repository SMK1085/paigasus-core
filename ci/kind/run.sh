#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# SMA-513 PR 3 — the kind job. One cluster, the chart installed the way an operator installs it,
# and the Playwright specs against a real ingress
# (docs/superpowers/specs/2026-09-23-sma-513-pr3-kind-chart-job-design.md).
#
#   run.sh up              kind cluster, Traefik, a throwaway CA and two leaves, the CoreDNS hosts
#                          block, Postgres, Redis, Keycloak, the three chart Secrets, the gateway
#                          stub at 0 replicas, the preflight
#   run.sh images          build paigasus-iam, iam-console and gateway-console; load them into kind
#   run.sh install a       helm install with values/a.yaml (both zones)
#   run.sh specs a         Playwright project phase-a (R1, R1-control, R2, R3-control)
#   run.sh stub up         scale the gateway stub to 1 and check /v1/service-info in-cluster (SMA-514)
#   run.sh specs journeys  Playwright project journeys (SMA-514's two scenarios) with its guards:
#                          the skip scan, the step-title check, exactly 2 tests, the JSON report
#   run.sh stub down       scale the gateway stub to 0 (local re-runs only; README order rule)
#   run.sh upgrade b       helm upgrade with a.yaml + b.yaml (gateway zone off), then settle
#   run.sh specs b         Playwright project phase-b (R3), then R3's Deployment check
#   run.sh diagnose        evidence into <state>/diagnose (never a Secret, never the realm ConfigMap)
#   run.sh down            delete the cluster
#
# Exit codes: 0 pass | 1 a spec or an assertion failed (a failed helm install or upgrade counts:
# the chart is the unit under test) | 2 an infrastructure error. Only die_assert and a failed spec
# give 1; the EXIT trap turns any other non-zero exit (an unguarded command under `set -e`) into 2.
#
# Runs under /bin/bash 3.2.57 and bash 5: no mapfile, no associative array, no here-string, no
# here-doc, no pipe into an early-exit reader (ci/actionlint check 13). `images` calls
# ci/images/run.sh, which DOES use here-strings (its assert_pins), so that one mode can hang on a
# host whose new pipe holds 512 bytes (root CLAUDE.md). Every wait has an explicit timeout.
set -euo pipefail

# proto prints NDJSON on stdout in an agent environment (SMA-609). Exported once, here.
export PROTO_REPORTER=text

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HERE="$REPO_ROOT/ci/kind"
STATE="${PAIGASUS_KIND_STATE:-${RUNNER_TEMP:-${TMPDIR:-/tmp}}/paigasus-kind}"
EVIDENCE="$STATE/diagnose"
CLUSTER=paigasus-kind
CONTEXT="kind-$CLUSTER"
RELEASE=paigasus
NS=paigasus
DEPS_NS=paigasus-deps
TRAEFIK_NS=traefik
CONSOLE_HOST=console.paigasus.test
IDP_HOST=idp.paigasus.test
ISSUER="https://$IDP_HOST/realms/paigasus"
KIND_USER=paigasus-kind

# kind v0.31.0 is the newest kind release with a Kubernetes 1.31 node image; the golden files are
# rendered for 1.31.0 (ci/helm-render/helm_render.py:47). chart.yml pins kind to v0.31.0 and
# kubectl to v1.31.14. Refresh: the "Images in this release" list of
#   https://github.com/kubernetes-sigs/kind/releases/tag/<kind version>
KIND_NODE_IMAGE="kindest/node:v1.31.14@sha256:6f86cf509dbb42767b6e79debc3f2c32e4ee01386f0489b3b2be24b0a55aac2b"

# Traefik, decision B10. The .tgz is checked before install. Refresh with:
#   helm pull traefik --repo https://traefik.github.io/charts --version <v>; shasum -a 256 traefik-<v>.tgz
TRAEFIK_CHART_REPO=https://traefik.github.io/charts
TRAEFIK_CHART_VERSION=41.6.0
TRAEFIK_CHART_SHA256=cd7254ea853da73bdb88edc896f079b88d43ffa0bfe699fdbf21081361eac365

# Traefik's own body for a request no router matches (Review Focus 3).
TRAEFIK_404_BODY="404 page not found"

USAGE="usage: ci/kind/run.sh up | images | install a | specs a|b|journeys | stub up|down | upgrade b | diagnose | down"

# helm's cache and config stay in the state directory, never in the operator's home.
export HELM_CACHE_HOME="$STATE/helm/cache" HELM_CONFIG_HOME="$STATE/helm/config" HELM_DATA_HOME="$STATE/helm/data"

ASSERTED=0
die_infra() { printf 'kind: infrastructure error (rc=2): %s\n' "$*" >&2; exit 2; }
die_assert() { printf 'kind: assertion failed (rc=1): %s\n' "$*" >&2; ASSERTED=1; exit 1; }
# A command that fails without a guard exits through `set -e` with ITS status, often 1. That must
# never read as an assertion: rc 1 is reserved for die_assert.
on_exit() {
  local rc=$?
  if [ "$rc" -ne 0 ] && [ "$rc" -ne 2 ] && [ "$ASSERTED" != 1 ]; then
    printf 'kind: infrastructure error (rc=2): an unguarded command exited %s\n' "$rc" >&2
    exit 2
  fi
}
trap on_exit EXIT

need() { command -v "$1" >/dev/null 2>&1 || die_infra "$1 is not on PATH"; }
k() { kubectl --context "$CONTEXT" "$@"; }
h() { "$HELM_BIN" --kube-context "$CONTEXT" "$@"; }
sha256_of() {  # prints only the hex digest of $1
  local line
  if command -v sha256sum >/dev/null 2>&1; then line="$(sha256sum "$1")"; else line="$(shasum -a 256 "$1")"; fi
  printf '%s\n' "${line%% *}"
}
# The number of lines of $2 that match the ERE $1. grep's rc 1 is "none", rc 2 is a broken read.
count_matches() {
  local n rc=0
  n="$(grep -c -E -- "$1" "$2")" || rc=$?
  [ "$rc" -le 1 ] || return 2
  printf '%s\n' "${n:-0}"
}
# The content of a state file; fails (with no output) when the file is missing or empty.
read_state() {
  local v
  v="$(cat "$STATE/$1" 2>/dev/null)" || return 1
  [ -n "$v" ] || return 1
  printf '%s' "$v"
}

# The name the chart gives a resource: charts/paigasus/templates/_helpers.tpl "paigasus.name".
# The base "<release>-<chart>" is cut to the room the suffix leaves (63 - len(suffix) - 1), ONE
# trailing "-" is trimmed (Sprig trimSuffix), then "-<suffix>" is appended.
chart_resource_name() {  # $1 = suffix, e.g. gateway-console
  local suffix="$1" chart base room
  chart="$(sed -n 's/^name:[[:space:]]*\([A-Za-z0-9-]*\)[[:space:]]*$/\1/p' "$REPO_ROOT/charts/paigasus/Chart.yaml")" \
    || return 1
  [ -n "$chart" ] || return 1
  room=$(( 63 - ${#suffix} - 1 ))
  base="$RELEASE-$chart"
  base="${base:0:$room}"
  base="${base%-}"
  printf '%s-%s\n' "$base" "$suffix"
}

# helm through proto, pinned: the chart is rendered with the helm the golden files pin. Model:
# ci/helm-render/run.sh resolve_helm.
resolve_helm() {
  local want got bin
  want="$(sed -n 's/^helm = "\([0-9][0-9.]*\)"$/\1/p' "$REPO_ROOT/.prototools")" || die_infra "cannot read .prototools"
  case "$want" in ''|*[!0-9.]*) die_infra "expected one 'helm = \"X.Y.Z\"' pin in .prototools, got: ${want:-<none>}" ;; esac
  bin="$(cd "$REPO_ROOT" && proto --reporter text bin helm)" || die_infra "'proto --reporter text bin helm' failed; run 'proto install helm'"
  if [ ! -f "$bin" ] || [ ! -x "$bin" ]; then die_infra "helm did not resolve to an executable file. Got: ${bin:-<empty>}"; fi
  got="$("$bin" version --short)" || die_infra "'$bin version --short' failed"
  case "$got" in "v$want"|"v$want+"*) ;; *) die_infra "helm is $got but .prototools pins $want" ;; esac
  HELM_BIN="$bin"
}

# ---------------------------------------------------------------------------------------- up

install_traefik() {
  local dir="$STATE/traefik" got
  rm -rf "$dir" || die_infra "cannot remove $dir"
  mkdir -p "$dir" || die_infra "cannot make $dir"
  "$HELM_BIN" pull traefik --repo "$TRAEFIK_CHART_REPO" --version "$TRAEFIK_CHART_VERSION" --destination "$dir" \
    || die_infra "helm pull traefik $TRAEFIK_CHART_VERSION failed"
  got="$(sha256_of "$dir/traefik-$TRAEFIK_CHART_VERSION.tgz")" || die_infra "cannot hash the Traefik chart"
  [ "$got" = "$TRAEFIK_CHART_SHA256" ] \
    || die_infra "Traefik chart $TRAEFIK_CHART_VERSION has SHA-256 $got, want $TRAEFIK_CHART_SHA256"
  h install traefik "$dir/traefik-$TRAEFIK_CHART_VERSION.tgz" --namespace "$TRAEFIK_NS" --create-namespace \
    -f "$HERE/traefik-values.yaml" --wait --timeout 5m || die_infra "helm install traefik failed"
  TRAEFIK_IP="$(k -n "$TRAEFIK_NS" get service traefik -o jsonpath='{.spec.clusterIP}')" \
    || die_infra "cannot read the traefik Service"
  case "$TRAEFIK_IP" in ''|None) die_infra "the traefik Service has no ClusterIP" ;; esac
  echo "  traefik ClusterIP $TRAEFIK_IP"
}

# The leaf profile that IAM's rustls/webpki needs (spec § 4.2 step 3). Node and Chromium accept a
# v1 leaf, a leaf with no SAN and a CA used as a leaf; webpki refuses all three.
check_leaf() {
  local crt="$1" host="$2" text
  text="$(openssl x509 -in "$crt" -noout -text)" || die_infra "cannot read $crt"
  case "$text" in *"Version: 3 (0x2)"*) ;; *) die_infra "$crt is not X.509 v3" ;; esac
  case "$text" in *"DNS:$host"*) ;; *) die_infra "$crt has no subjectAltName DNS:$host" ;; esac
  case "$text" in *"TLS Web Server Authentication"*) ;; *) die_infra "$crt lacks extendedKeyUsage serverAuth" ;; esac
  case "$text" in *"CA:FALSE"*) ;; *) die_infra "$crt lacks basicConstraints CA:FALSE" ;; esac
  openssl verify -CAfile "$STATE/pki/ca.crt" "$crt" >/dev/null || die_infra "$crt does not verify against the throwaway CA"
}

make_pki() {
  local p="$STATE/pki" host serial
  rm -rf "$p" || die_infra "cannot remove $p"
  mkdir -p "$p" || die_infra "cannot make $p"
  printf '%s\n' '[req]' 'prompt = no' 'distinguished_name = dn' '[dn]' 'CN = paigasus-kind-throwaway-ca' \
    '[v3_ca]' 'basicConstraints = critical,CA:TRUE' 'keyUsage = critical,keyCertSign' 'subjectKeyIdentifier = hash' \
    >"$p/ca.cnf" || die_infra "cannot write $p/ca.cnf"
  openssl req -x509 -new -nodes -newkey rsa:2048 -sha256 -days 2 -config "$p/ca.cnf" -extensions v3_ca \
    -keyout "$p/ca.key" -out "$p/ca.crt" 2>>"$p/openssl.log" || die_infra "openssl could not make the CA (see $p/openssl.log)"
  for host in "$CONSOLE_HOST" "$IDP_HOST"; do
    printf '%s\n' '[v3_leaf]' 'basicConstraints = CA:FALSE' 'extendedKeyUsage = serverAuth' "subjectAltName = DNS:$host" \
      >"$p/$host.cnf" || die_infra "cannot write $p/$host.cnf"
    openssl req -new -nodes -newkey rsa:2048 -subj "/CN=$host" -keyout "$p/$host.key" -out "$p/$host.csr" 2>>"$p/openssl.log" \
      || die_infra "openssl could not make the $host key and CSR"
    serial="$(openssl rand -hex 8)" || die_infra "openssl rand failed for the $host serial"
    [ -n "$serial" ] || die_infra "openssl rand gave an empty serial for $host"
    openssl x509 -req -sha256 -days 2 -in "$p/$host.csr" -CA "$p/ca.crt" -CAkey "$p/ca.key" \
      -set_serial "0x$serial" -extfile "$p/$host.cnf" -extensions v3_leaf -out "$p/$host.crt" 2>>"$p/openssl.log" \
      || die_infra "openssl could not sign the $host leaf"
    check_leaf "$p/$host.crt" "$host"
  done
  echo "  CA and two leaves OK (v3, SAN, serverAuth, CA:FALSE, verified)"
}

# A `hosts` answer has the question name as its owner, so glibc accepts it (spec § 4.2 step 4).
# Both names resolve to the Traefik ClusterIP inside the cluster; every other name falls through.
coredns_hosts() {
  local f="$STATE/Corefile" before after hosts_ere='^[[:space:]]*hosts[[:space:]]*[{]'
  k -n kube-system get configmap coredns -o jsonpath='{.data.Corefile}' >"$f.orig" || die_infra "cannot read the CoreDNS Corefile"
  awk -v ip="$TRAEFIK_IP" -v a="$CONSOLE_HOST" -v b="$IDP_HOST" '
    /^[[:space:]]*kubernetes cluster\.local/ && !done {
      print "    hosts {"
      print "       " ip " " a
      print "       " ip " " b
      print "       fallthrough"
      print "    }"
      done = 1
    }
    { print }' "$f.orig" >"$f" || die_infra "awk could not edit the Corefile"
  before="$(count_matches "$hosts_ere" "$f.orig")" || die_infra "cannot count hosts blocks in $f.orig"
  after="$(count_matches "$hosts_ere" "$f")" || die_infra "cannot count hosts blocks in $f"
  [ "$(( after - before ))" = 1 ] \
    || die_infra "inserted $(( after - before )) CoreDNS hosts block(s), want 1: the 'kubernetes cluster.local' anchor moved (see $f.orig)"
  k -n kube-system create configmap coredns --from-file=Corefile="$f" --dry-run=client -o yaml >"$f.yaml" \
    || die_infra "cannot build the CoreDNS ConfigMap"
  k apply -f "$f.yaml" >/dev/null || die_infra "cannot apply the CoreDNS ConfigMap"
  k -n kube-system rollout restart deployment coredns >/dev/null || die_infra "cannot restart CoreDNS"
  k -n kube-system rollout status deployment coredns --timeout=180s || die_infra "CoreDNS did not become ready in 180 s"
  echo "  CoreDNS: $CONSOLE_HOST and $IDP_HOST -> $TRAEFIK_IP"
}

gen_secret() {  # $1 = state file name, then the `openssl rand` arguments
  local f="$1" v
  shift
  v="$(openssl rand "$@")" || die_infra "openssl rand failed for $f"
  [ -n "$v" ] || die_infra "openssl rand gave an empty value for $f"
  printf '%s' "$v" >"$STATE/$f" || die_infra "cannot write $STATE/$f"
}

make_credentials() {
  gen_secret client-secret -hex 24
  gen_secret user-password -hex 16
  gen_secret postgres-password -hex 24
  gen_secret pepper -base64 32
}

deps() {
  local realm="$STATE/paigasus-realm.json" d cs pw n
  cs="$(read_state client-secret)" || die_infra "$STATE/client-secret is missing or empty"
  pw="$(read_state user-password)" || die_infra "$STATE/user-password is missing or empty"
  # Both values are hex, so neither holds a sed metacharacter.
  sed -e "s/__PAIGASUS_KIND_CLIENT_SECRET__/$cs/" -e "s/__PAIGASUS_KIND_USER_PASSWORD__/$pw/" \
    "$HERE/realm/paigasus-realm.json" >"$realm" || die_infra "cannot fill the realm"
  n="$(count_matches '__PAIGASUS_KIND_' "$realm")" || die_infra "cannot read $realm"
  [ "$n" = 0 ] || die_infra "a realm placeholder survived in $realm"
  k -n "$DEPS_NS" create secret generic postgres-auth --from-file=password="$STATE/postgres-password" >/dev/null \
    || die_infra "cannot create the Secret postgres-auth"
  k -n "$DEPS_NS" create configmap keycloak-realm --from-file=paigasus-realm.json="$realm" >/dev/null \
    || die_infra "cannot create the ConfigMap keycloak-realm"
  k apply -f "$HERE/manifests/postgres.yaml" -f "$HERE/manifests/redis.yaml" -f "$HERE/manifests/keycloak.yaml" \
    || die_infra "cannot apply the dependency manifests"
  for d in postgres redis keycloak; do
    k -n "$DEPS_NS" rollout status "deployment/$d" --timeout=600s || die_infra "$d did not become ready in 600 s"
  done
}

chart_secrets() {
  local pg
  pg="$(read_state postgres-password)" || die_infra "$STATE/postgres-password is missing or empty"
  # User and database as in manifests/postgres.yaml (POSTGRES_USER paigasus, POSTGRES_DB iam).
  printf '%s' "postgres://paigasus:$pg@postgres.$DEPS_NS.svc.cluster.local:5432/iam" >"$STATE/database-url" \
    || die_infra "cannot write $STATE/database-url"
  k -n "$NS" create secret generic paigasus-oidc \
    --from-file=oidc-client-secret="$STATE/client-secret" \
    --from-literal=session-redis-url="redis://redis.$DEPS_NS.svc.cluster.local:6379" >/dev/null \
    || die_infra "cannot create the Secret paigasus-oidc"
  k -n "$NS" create secret generic paigasus-postgres --from-file=database-url="$STATE/database-url" >/dev/null \
    || die_infra "cannot create the Secret paigasus-postgres"
  k -n "$NS" create secret generic paigasus-iam-pepper --from-file=pepper="$STATE/pepper" >/dev/null \
    || die_infra "cannot create the Secret paigasus-iam-pepper"
  k apply -f "$HERE/manifests/gateway-stub.yaml" >/dev/null || die_infra "cannot apply gateway-stub"
}

preflight() {
  local out="$EVIDENCE/idp-preflight.json"
  # Keycloak Ready first: the preflight's own retries are for the ingress route, not the boot.
  k -n "$DEPS_NS" wait pod -l app.kubernetes.io/name=keycloak --for=condition=Ready --timeout=600s \
    || die_infra "Keycloak did not become Ready in 600 s"
  k -n "$NS" delete pod idp-preflight --ignore-not-found --wait=true --timeout=60s >/dev/null \
    || die_infra "cannot delete an old idp-preflight pod"
  k apply -f "$HERE/manifests/idp-preflight.yaml" >/dev/null || die_infra "cannot start the preflight pod"
  if ! k -n "$NS" wait pod/idp-preflight --for=jsonpath='{.status.phase}'=Succeeded --timeout=180s; then
    k -n "$NS" logs pod/idp-preflight >"$out" 2>&1 || true
    k -n "$NS" describe pod/idp-preflight >"$EVIDENCE/idp-preflight.describe.txt" 2>&1 || true
    die_infra "the discovery preflight did not succeed in 180 s; evidence in $out"
  fi
  k -n "$NS" logs pod/idp-preflight >"$out" || die_infra "cannot read the preflight output"
  ISSUER="$ISSUER" python3 -c '
import json, os, sys
with open(sys.argv[1]) as fh:
    lines = [line for line in fh.read().splitlines() if line.startswith("{")]
if not lines:
    print("no JSON object in the preflight output")
    sys.exit(1)
doc = json.loads(lines[-1])
want = os.environ["ISSUER"]
problems = []
if doc.get("issuer") != want:
    problems.append("issuer is " + repr(doc.get("issuer")) + ", want " + repr(want))
if not str(doc.get("jwks_uri", "")).startswith("https://"):
    problems.append("jwks_uri is " + repr(doc.get("jwks_uri")) + ", want an https URL")
if problems:
    print("; ".join(problems))
    sys.exit(1)
print("  preflight OK: issuer " + want + ", jwks_uri " + doc["jwks_uri"])' "$out" \
    || die_infra "the discovery document is wrong; evidence in $out"
}

up() {
  need kind; need kubectl; need openssl; need python3; need docker; need curl
  resolve_helm
  umask 077
  mkdir -p "$STATE" || die_infra "cannot make $STATE"
  chmod 700 "$STATE" || die_infra "cannot chmod $STATE"
  rm -rf "$EVIDENCE" || die_infra "cannot remove $EVIDENCE"
  mkdir -p "$EVIDENCE" || die_infra "cannot make $EVIDENCE"
  echo "== 1. kind cluster $CLUSTER ($KIND_NODE_IMAGE) =="
  kind create cluster --name "$CLUSTER" --image "$KIND_NODE_IMAGE" --config "$HERE/cluster.yaml" --wait 180s \
    || die_infra "kind create cluster failed (does a cluster named $CLUSTER exist already? run.sh down)"
  echo "== 2. Traefik $TRAEFIK_CHART_VERSION =="
  install_traefik
  echo "== 3. throwaway CA, two leaves, two TLS Secrets, the CA ConfigMap =="
  make_pki
  k create namespace "$NS" >/dev/null || die_infra "cannot create namespace $NS"
  k create namespace "$DEPS_NS" >/dev/null || die_infra "cannot create namespace $DEPS_NS"
  # console-tls for the chart's Ingress (paigasus); idp-tls for the Keycloak Ingress (paigasus-deps).
  k -n "$NS" create secret tls console-tls --cert="$STATE/pki/$CONSOLE_HOST.crt" --key="$STATE/pki/$CONSOLE_HOST.key" >/dev/null \
    || die_infra "cannot create the Secret console-tls"
  k -n "$DEPS_NS" create secret tls idp-tls --cert="$STATE/pki/$IDP_HOST.crt" --key="$STATE/pki/$IDP_HOST.key" >/dev/null \
    || die_infra "cannot create the Secret idp-tls"
  k -n "$NS" create configmap paigasus-idp-ca --from-file=ca.crt="$STATE/pki/ca.crt" >/dev/null \
    || die_infra "cannot create the ConfigMap paigasus-idp-ca"
  echo "== 4. CoreDNS hosts block =="
  coredns_hosts
  echo "== 5. Postgres, Redis, Keycloak =="
  make_credentials
  deps
  echo "== 6. the three Secrets the chart refers to =="
  chart_secrets
  echo "== 7. discovery preflight =="
  preflight
  echo "== up: done =="
}

# ------------------------------------------------------------------------------------ images

load_images() {
  local img
  case "${PAIGASUS_KIND_LOAD:-docker-image}" in
    docker-image)
      kind load docker-image "$@" --name "$CLUSTER" || die_infra "kind load docker-image failed (try PAIGASUS_KIND_LOAD=archive)" ;;
    archive)
      docker save -o "$STATE/images.tar" "$@" || die_infra "docker save failed"
      kind load image-archive "$STATE/images.tar" --name "$CLUSTER" || die_infra "kind load image-archive failed"
      rm -f "$STATE/images.tar" || die_infra "cannot remove $STATE/images.tar" ;;
    *) die_infra "PAIGASUS_KIND_LOAD must be docker-image or archive, got: $PAIGASUS_KIND_LOAD" ;;
  esac
  for img in "$@"; do
    docker exec "$CLUSTER-control-plane" crictl inspecti "docker.io/library/$img" >/dev/null \
      || die_infra "$img is not in the node's image store after the load"
    echo "  loaded $img"
  done
}

images() {
  need docker; need kind
  mkdir -p "$STATE" || die_infra "cannot make $STATE"
  bash "$REPO_ROOT/ci/images/run.sh" build iam || die_infra "building paigasus-iam failed"
  bash "$REPO_ROOT/ci/images/run.sh" build-console || die_infra "building the two consoles failed"
  load_images paigasus-iam:dev iam-console:dev gateway-console:dev
}

# --------------------------------------------------------------------------- install/upgrade

install_a() {
  local gw_name gw
  need kubectl; resolve_helm
  h install "$RELEASE" "$REPO_ROOT/charts/paigasus" --namespace "$NS" -f "$HERE/values/a.yaml" --wait --timeout 10m \
    || die_assert "helm install with values/a.yaml did not become ready in 10 minutes"
  # The positive control of R3's absence check in `specs b`: the derived name must name a real
  # Deployment while the gateway zone is on, or that check would pass on a wrong name.
  gw_name="$(chart_resource_name gateway-console)" || die_infra "cannot derive the gateway console Deployment name"
  gw="$(k -n "$NS" get deployment "$gw_name" --ignore-not-found -o name)" || die_infra "cannot read Deployments in $NS"
  [ "$gw" = "deployment.apps/$gw_name" ] \
    || die_assert "phase A has no Deployment $gw_name: the name R3 checks does not match the chart's paigasus.name"
  echo "  phase A: $gw exists"
  echo "== install a: done =="
}

# The pod-template-hash of each iam-console ReplicaSet whose spec.replicas is above 0.
active_hashes() {
  k -n "$NS" get replicasets -l app.kubernetes.io/name=iam-console \
    -o jsonpath='{range .items[?(@.spec.replicas>0)]}{.metadata.labels.pod-template-hash}{" "}{end}'
}

# Prints what remains of the OLD iam-console ReplicaSets: a non-zero spec.replicas or a pod.
# Nothing printed means that every old ReplicaSet is at 0 and no old pod exists.
old_leftovers() {  # $1 = old hashes, space-separated
  local hash reps pods r p out=""
  for hash in $1; do
    reps="$(k -n "$NS" get replicasets -l "app.kubernetes.io/name=iam-console,pod-template-hash=$hash" \
      -o jsonpath='{range .items[*]}{.spec.replicas}{" "}{end}')" || return 1
    for r in $reps; do [ "$r" = 0 ] || out="$out rs/$hash=$r"; done
    pods="$(k -n "$NS" get pods -l "app.kubernetes.io/name=iam-console,pod-template-hash=$hash" -o name)" || return 1
    for p in $pods; do out="$out $p"; done
  done
  printf '%s' "$out"
}

# Spec § 4.5 settle step, part 2: Traefik's own 404 on /gateway/overview.
settle_gateway_404() {
  local deadline code body
  deadline=$(( $(date +%s) + 120 ))
  while :; do
    # A stale body from an earlier curl (or an earlier local re-run with the same state directory)
    # must not read as this iteration's answer (review Task 7 (2)).
    rm -f "$STATE/settle-body"
    code="$(curl -sS --max-time 10 --cacert "$STATE/pki/ca.crt" --resolve "$CONSOLE_HOST:443:127.0.0.1" \
      -o "$STATE/settle-body" -w '%{http_code}' "https://$CONSOLE_HOST/gateway/overview" 2>"$STATE/settle-curl.err" || true)"
    body="$(cat "$STATE/settle-body" 2>/dev/null || true)"
    if [ "$code" = 404 ] && [ "$body" = "$TRAEFIK_404_BODY" ]; then break; fi
    if [ "$(date +%s)" -ge "$deadline" ]; then
      case "$code" in
        ''|000) die_infra "no HTTPS answer from $CONSOLE_HOST 120 s after the upgrade: $(cat "$STATE/settle-curl.err" 2>/dev/null || true)" ;;
        *) die_assert "/gateway/overview still answers $code ('$body') 120 s after the upgrade; want 404 '$TRAEFIK_404_BODY'" ;;
      esac
    fi
    sleep 2
  done
  echo "  settled: /gateway/overview answers Traefik's 404"
}

upgrade_b() {
  local old new left deadline h1 h2 all_old
  need kubectl; need curl; resolve_helm
  old="$(active_hashes)" || die_infra "cannot list iam-console ReplicaSets"
  [ -n "$old" ] || die_infra "no active iam-console ReplicaSet before the upgrade; run 'run.sh install a' first"
  h upgrade "$RELEASE" "$REPO_ROOT/charts/paigasus" --namespace "$NS" -f "$HERE/values/a.yaml" -f "$HERE/values/b.yaml" \
    --wait --timeout 10m || die_assert "helm upgrade with values/b.yaml did not become ready in 10 minutes"
  # Settle step, part 1 (spec § 4.5): every OLD ReplicaSet at spec.replicas=0 and no old pod. helm
  # --wait returns when the NEW ReplicaSet is ready; the old one can still be scaling down.
  deadline=$(( $(date +%s) + 180 ))
  while :; do
    left="$(old_leftovers "$old")" || die_infra "cannot read the old iam-console ReplicaSets and pods"
    [ -n "$left" ] || break
    if [ "$(date +%s)" -ge "$deadline" ]; then
      new="$(active_hashes)" || die_infra "cannot list iam-console ReplicaSets"
      all_old=1
      for h2 in $new; do case " $old " in *" $h2 "*) ;; *) all_old=0 ;; esac; done
      # Review Focus 5: no new ReplicaSet at all means the pod template did not change.
      [ "$all_old" = 0 ] \
        || die_assert "the iam-console pod template did not change on upgrade b (active: $new): the zone map no longer rolls the console"
      die_infra "the old iam-console ReplicaSets or pods still exist 180 s after the upgrade:$left"
    fi
    sleep 2
  done
  echo "  settled: every old iam-console ReplicaSet is at 0 replicas and no old pod exists"
  # Only now are the active hashes the new ones alone.
  new="$(active_hashes)" || die_infra "cannot list iam-console ReplicaSets"
  [ -n "$new" ] || die_assert "no active iam-console ReplicaSet after upgrade b"
  for h1 in $old; do
    for h2 in $new; do
      [ "$h1" != "$h2" ] || die_assert "the iam-console pod template did not change on upgrade b (hash $h1): the zone map no longer rolls the console"
    done
  done
  echo "  iam-console pod-template-hash: old [${old% }] -> new [${new% }]"
  settle_gateway_404
  echo "== upgrade b: done =="
}

# --------------------------------------------------------------------------------------- specs

# SMA-514 spec § 7.2: no test in tests/cluster may skip, fixme, expect a failure or focus. The
# scan is here and not in iam-console-ts:test, because that task's inputs exclude tests/cluster/**
# (ts/apps/iam-console/moon.yml), so Moon would serve a cached PASS. Blanks before the "(" are
# allowed for. NOT seen: a bracket call such as test['skip'](…).
SKIP_ERE='\.(skip|fixme|fail|only)[[:space:]]*\('
CLUSTER_TESTS="$REPO_ROOT/ts/apps/iam-console/tests/cluster"

# The journeys guards that need no cluster, before `--list`. $1 = the evidence directory.
journeys_prerun() {
  local out="$1" rc=0
  grep -rnE --include='*.ts' -- "$SKIP_ERE" "$CLUSTER_TESTS" >"$out/skip-scan.txt" 2>&1 || rc=$?
  case "$rc" in
    0) die_assert "tests/cluster must not skip, fixme, fail or focus a test (SMA-514 AC 2): $(cat "$out/skip-scan.txt")" ;;
    1) echo "  ok [scan]: no .skip/.fixme/.fail/.only in tests/cluster" ;;
    *) die_infra "the skip scan could not read $CLUSTER_TESTS (grep rc $rc): $(cat "$out/skip-scan.txt")" ;;
  esac
  rc=0
  node "$HERE/journeys-report.mjs" sources "$CLUSTER_TESTS/journeys" >"$out/sources.txt" 2>&1 || rc=$?
  cat "$out/sources.txt"
  case "$rc" in
    0) ;;
    3) die_assert "the journeys step titles and EXPECTED_STEPS in ci/kind/journeys-report.mjs disagree" ;;
    *) die_infra "journeys-report.mjs sources exited $rc; see $out/sources.txt" ;;
  esac
  node --test "$HERE/journeys-report.test.mjs" "$HERE/stub-check.test.mjs" >"$out/checker-tests.txt" 2>&1 \
    || die_infra "the kind checkers failed their own unit tests; see $out/checker-tests.txt"
  echo "  ok [checkers]: journeys-report.test.mjs and stub-check.test.mjs pass"
}

specs() {  # $1 = a | b | journeys
  local which="$1" project rc=0 out report pw gw_name gw failed="" list_out list_rc=0 total check_rc=0
  case "$which" in
    a) project=phase-a ;;
    b) project=phase-b ;;
    journeys) project=journeys ;;
    *) die_infra "$USAGE" ;;
  esac
  out="$EVIDENCE/playwright/$project"
  report="$out/report.json"
  pw="$(read_state user-password)" || die_infra "no credentials in $STATE; run 'run.sh up' first"
  need pnpm
  if [ "$which" = b ]; then need kubectl; fi
  if [ "$which" = journeys ]; then need node; fi
  mkdir -p "$out" || die_infra "cannot make $out"
  if [ "$which" = journeys ]; then journeys_prerun "$out"; fi
  # Playwright's rc 1 means "a spec failed" ONLY when the run got as far as running specs. It also
  # exits 1 for a config that will not load, an unknown --project, an empty test list, or a missing
  # browser (spec § 4.7). `--list` alone cannot tell those apart from a real run, so read it before
  # trusting a later rc 1 as an assertion (review Task 7 (1)).
  list_out="$out/list.txt"
  PAIGASUS_KIND_USERNAME="$KIND_USER" PAIGASUS_KIND_PASSWORD="$pw" PAIGASUS_KIND_OUTPUT_DIR="$out" \
    pnpm --dir "$REPO_ROOT/ts/apps/iam-console" exec playwright test \
      --config tests/cluster/playwright.config.ts --project "$project" --list >"$list_out" 2>&1 || list_rc=$?
  [ "$list_rc" = 0 ] \
    || die_infra "playwright --list exited $list_rc for $project (a config that will not load, an unknown --project, or a missing browser); see $list_out"
  total="$(sed -n 's/^Total: \([0-9][0-9]*\) test.*/\1/p' "$list_out")"
  [ -n "$total" ] && [ "$total" -gt 0 ] \
    || die_infra "playwright --list found zero tests for $project; see $list_out"
  if [ "$which" = journeys ] && [ "$total" != 2 ]; then
    die_infra "journeys must hold exactly 2 tests (SMA-514 AC 3); --list found $total; see $list_out"
  fi
  # A report from an earlier run must not read as this run's (the settle_gateway_404 rule).
  rm -f "$report" || die_infra "cannot remove $report"
  PAIGASUS_KIND_USERNAME="$KIND_USER" PAIGASUS_KIND_PASSWORD="$pw" PAIGASUS_KIND_OUTPUT_DIR="$out" \
    pnpm --dir "$REPO_ROOT/ts/apps/iam-console" exec playwright test \
      --config tests/cluster/playwright.config.ts --project "$project" || rc=$?
  case "$rc" in
    0) ;;
    1) failed="Playwright project $project" ;;
    *) die_infra "playwright exited $rc for $project" ;;
  esac
  if [ "$which" = journeys ]; then
    # SMA-514 spec § 7.2, decision D6: a well-formed report that fails a check is rc 1; a missing
    # or unreadable report is rc 2. It runs after a failed spec too, for the evidence.
    node "$HERE/journeys-report.mjs" report "$report" >"$out/report-check.txt" 2>&1 || check_rc=$?
    cat "$out/report-check.txt"
    case "$check_rc" in
      0) ;;
      3) failed="${failed:+$failed; }the journeys report failed its checks (above)" ;;
      *) die_infra "the journeys report is missing or unreadable (checker rc $check_rc); see $out/report-check.txt" ;;
    esac
  fi
  if [ "$which" = b ]; then
    # R3's last clause (spec § 6.4): the gateway console Deployment does not exist. Here and not in
    # the spec file, so the TS tier needs no child_process. By NAME: the chart's Deployment
    # metadata has no labels (charts/paigasus/templates/console-deployment.yaml), and install_a
    # proved the same name exists in phase A.
    gw_name="$(chart_resource_name gateway-console)" || die_infra "cannot derive the gateway console Deployment name"
    gw="$(k -n "$NS" get deployment "$gw_name" --ignore-not-found -o name)" || die_infra "cannot read Deployments in $NS"
    if [ -n "$gw" ]; then
      failed="${failed:+$failed; }[R3] the gateway console Deployment still exists: $gw"
    else
      echo "  ok [R3]: no Deployment $gw_name"
    fi
  fi
  [ -z "$failed" ] || die_assert "$failed"
}

# ---------------------------------------------------------------------------------------- stub

# SMA-514 spec § 4.3-4.4, decision D2. The stub starts AFTER phase A: R3-control needs the gateway
# zone `degraded`. The discovery cache keeps a failed probe for 10 s and a good one for 60 s, so
# after `stub down` wait about 60 s before `specs a` (README).
STUB=gateway-stub

stub_up() {
  local out="$EVIDENCE/stub-check.log"
  need kubectl; need node
  mkdir -p "$EVIDENCE" || die_infra "cannot make $EVIDENCE"
  k -n "$NS" scale "deployment/$STUB" --replicas=1 >/dev/null || die_infra "cannot scale $STUB to 1 replica"
  # 300 s: the stub image is pulled only here, from Docker Hub, and a throttled pull is slow.
  k -n "$NS" rollout status "deployment/$STUB" --timeout=300s || die_infra "$STUB did not become ready in 300 s"
  k -n "$NS" delete pod stub-check --ignore-not-found --wait=true --timeout=60s >/dev/null \
    || die_infra "cannot delete an old stub-check pod"
  k apply -f "$HERE/manifests/stub-check.yaml" >/dev/null || die_infra "cannot start the stub-check pod"
  if ! k -n "$NS" wait pod/stub-check --for=jsonpath='{.status.phase}'=Succeeded --timeout=120s; then
    k -n "$NS" logs pod/stub-check >"$out" 2>&1 || true
    k -n "$NS" describe pod/stub-check >"$EVIDENCE/stub-check.describe.txt" 2>&1 || true
    die_infra "the stub-check pod did not succeed in 120 s; evidence in $out"
  fi
  k -n "$NS" logs pod/stub-check >"$out" || die_infra "cannot read the stub-check output"
  node "$HERE/stub-check.mjs" "$out" \
    || die_infra "the gateway stub does not answer /v1/service-info as the discovery probe needs; evidence in $out"
  echo "== stub up: done =="
}

stub_down() {
  local deadline ips
  need kubectl
  k -n "$NS" scale "deployment/$STUB" --replicas=0 >/dev/null || die_infra "cannot scale $STUB to 0 replicas"
  deadline=$(( $(date +%s) + 120 ))
  while :; do
    ips="$(k -n "$NS" get endpoints "$STUB" -o jsonpath='{.subsets[*].addresses[*].ip}')" \
      || die_infra "cannot read the $STUB endpoints"
    [ -n "$ips" ] || break
    [ "$(date +%s)" -lt "$deadline" ] || die_infra "the $STUB Service still has endpoints 120 s after the scale-down: $ips"
    sleep 2
  done
  echo "  $STUB: 0 replicas, no endpoints. Wait about 60 s before 'specs a' (the discovery cache, README)"
  echo "== stub down: done =="
}

# ------------------------------------------------------------------------------------ diagnose

resolve_helm_quiet() {  # diagnose must not exit 2 because helm is missing
  local bin
  bin="$(cd "$REPO_ROOT" && proto --reporter text bin helm 2>/dev/null)" || return 1
  [ -f "$bin" ] && [ -x "$bin" ] || return 1
  HELM_BIN="$bin"
}

diagnose() {
  local ns p ready d="$EVIDENCE"
  mkdir -p "$d/logs" "$d/describe" || die_infra "cannot make $d"
  # EXCLUDED on purpose (spec § 4.6): Secrets, and the realm ConfigMap keycloak-realm (it holds the
  # client secret and the password). `get all` lists neither, and nothing below reads them. The
  # Playwright traces under playwright/ hold the per-run password as typed; it is a throwaway that
  # dies with the cluster.
  k get all -A -o wide >"$d/get-all.txt" 2>&1 || true
  k get ingress,ingressclass -A -o wide >"$d/ingress.txt" 2>&1 || true
  k get events -A --sort-by=.lastTimestamp >"$d/events.txt" 2>&1 || true
  k -n kube-system get configmap coredns -o yaml >"$d/coredns.yaml" 2>&1 || true
  for ns in "$NS" "$DEPS_NS" "$TRAEFIK_NS"; do
    k -n "$ns" get pods -o jsonpath='{range .items[*]}{.metadata.name}{" "}{.status.conditions[?(@.type=="Ready")].status}{"\n"}{end}' \
      >"$d/pods-$ns.txt" 2>/dev/null || true
    while read -r p ready; do
      [ -n "$p" ] || continue
      k -n "$ns" logs --all-containers "$p" >"$d/logs/$ns-$p.log" 2>&1 || true
      k -n "$ns" logs --all-containers --previous "$p" >"$d/logs/$ns-$p.previous.log" 2>&1 || true
      if [ "$ready" != "True" ]; then k -n "$ns" describe pod "$p" >"$d/describe/$ns-$p.txt" 2>&1 || true; fi
    done <"$d/pods-$ns.txt"
  done
  # SMA-514: the stub's Deployment, pods and endpoints. Its pod logs are in logs/ already.
  k -n "$NS" get deployment "$STUB" -o wide >"$d/gateway-stub.txt" 2>&1 || true
  k -n "$NS" get pods -l "app.kubernetes.io/name=$STUB" -o wide >>"$d/gateway-stub.txt" 2>&1 || true
  k -n "$NS" get endpoints "$STUB" -o wide >>"$d/gateway-stub.txt" 2>&1 || true
  if [ -n "${HELM_BIN:-}" ] || resolve_helm_quiet; then
    # The chart renders no Secret: it only refers to existing ones by name.
    h -n "$NS" get manifest "$RELEASE" >"$d/helm-manifest.yaml" 2>&1 || true
  fi
  echo "evidence in $d"
}

down() {
  need kind
  kind delete cluster --name "$CLUSTER" || die_infra "kind delete cluster failed"
}

cmd="${1:-}"
case "$cmd" in
  up) [ "$#" = 1 ] || die_infra "$USAGE"; up ;;
  images) [ "$#" = 1 ] || die_infra "$USAGE"; images ;;
  install) [ "$#" = 2 ] && [ "$2" = a ] || die_infra "$USAGE"; install_a ;;
  upgrade) [ "$#" = 2 ] && [ "$2" = b ] || die_infra "$USAGE"; upgrade_b ;;
  specs)
    [ "$#" = 2 ] || die_infra "$USAGE"
    case "$2" in a|b|journeys) ;; *) die_infra "$USAGE" ;; esac
    specs "$2" ;;
  stub)
    [ "$#" = 2 ] || die_infra "$USAGE"
    case "$2" in
      up) stub_up ;;
      down) stub_down ;;
      *) die_infra "$USAGE" ;;
    esac ;;
  diagnose) [ "$#" = 1 ] || die_infra "$USAGE"; diagnose ;;
  down) [ "$#" = 1 ] || die_infra "$USAGE"; down ;;
  *) die_infra "$USAGE" ;;
esac
