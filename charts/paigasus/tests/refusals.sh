#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Spec § 7.7. Four values combinations, two valid and two refused. A refused one must fail with
# ITS OWN message, not with an incidental template error from somewhere else — otherwise the
# chart is refusing by accident and a later edit silently makes it install.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHART="$(cd "$HERE/.." && pwd)"
KUBE_VERSION="1.31.0"
ec=0

# The "${X[@]+...}" guards below are load-bearing, not noise. MEASURED: bash 3.2.57
# treats "${A[@]}" on an EMPTY array as an unbound variable under `set -u`, so a
# no-argument run would abort before the first row. bash 5.x does not. Some gates in
# this repo run under 3.2.
BASE=("$@")

# Every other REQUIRED value, held valid by default, so each expect_fail row below can set only
# its own value to "" without being refused by a different check for the wrong reason (the same
# trap the "unknown zone id" row above carries). ingress.host is deliberately NOT here: it stays
# supplied by the caller via BASE, matching this script's existing calling convention.
FIXED=(
  --set ingress.tlsSecretName=console-tls
  --set oidc.issuer=https://idp.example.test/realms/paigasus
  --set oidc.clientId=paigasus-console
  --set oidc.existingSecret=paigasus-console-secret
  --set postgres.existingSecret=paigasus-postgres-secret
  --set zones.iam.backend.apiKeysPepperSecret=paigasus-iam-pepper
)

render() { helm template t "$CHART" --kube-version "$KUBE_VERSION" "${FIXED[@]+"${FIXED[@]}"}" "${BASE[@]+"${BASE[@]}"}" "$@" 2>&1; }

expect_fail() {
  local label="$1" needle="$2"; shift 2
  local out
  out="$(render "$@")" && { echo "FAIL [$label]: rendered, expected a refusal"; ec=1; return; }
  if grep -qF -- "$needle" < <(printf '%s' "$out"); then
    echo "  ok [$label]: refused with its own message"
  else
    echo "FAIL [$label]: refused, but not with \"$needle\""; printf '%s\n' "$out"; ec=1
  fi
}

expect_render() {
  local label="$1"; shift
  local out
  # Capture once. The earlier shape re-ran `render "$@"` bare inside the else branch to print the
  # error, and under `set -e` that non-zero return ABORTED the whole script — so one failing row
  # silently cancelled every row after it. A battery that stops early hides findings, which is the
  # opposite of what it is for.
  if out="$(render "$@")"; then
    echo "  ok [$label]: renders"
  else
    echo "FAIL [$label]: expected a successful render"; printf '%s\n' "$out"; ec=1
  fi
}

expect_fail "no zone enabled" "at least one zone must be enabled" \
  --set zones.iam.enabled=false --set zones.gateway.enabled=false
expect_fail "gateway without iam" "the gateway zone requires the iam zone" \
  --set zones.iam.enabled=false --set zones.gateway.enabled=true
expect_fail "unknown zone id" "is not a known service slug" \
  --set zones.frobnicate.enabled=true --set zones.frobnicate.basePath=/frobnicate
expect_fail "external backend without url" "backend.url is required" \
  --set zones.gateway.enabled=true --set zones.gateway.backend.deploy=false \
  --set zones.gateway.backend.url=""
# ingress.host is supplied by the CALLER via BASE, so without this row nothing ever exercised its
# refusal — the one required value whose check was written first and tested last. A row-level
# --set comes after BASE and wins.
expect_fail "ingress.host empty" "ingress.host is required" \
  --set ingress.host=""
expect_fail "ingress.tlsSecretName empty" "ingress.tlsSecretName is required" \
  --set ingress.tlsSecretName=""
# SMA-695. With ingress.enabled=false the chart renders no Ingress, so it needs no TLS Secret. The
# host stays required: it feeds PAIGASUS_PUBLIC_ORIGIN (spec D1).
expect_render "ingress disabled, tlsSecretName empty" \
  --set ingress.enabled=false --set ingress.tlsSecretName=""
expect_fail "ingress disabled, host empty" "ingress.host is required" \
  --set ingress.enabled=false --set ingress.host=""
# A quoted "false" is a string, and a non-empty string is true in a template `if`. Without this
# refusal the Ingress stays, which is the SMA-695 bug.
expect_fail "ingress.enabled a string" "ingress.enabled must be true or false" \
  --set-string ingress.enabled=false
# The kind check must run before the TLS check. With an empty TLS Secret, a string "false" must
# still get the kind message.
expect_fail "ingress.enabled a string, tlsSecretName empty" "ingress.enabled must be true or false" \
  --set-string ingress.enabled=false --set ingress.tlsSecretName=""
# --set ingress.enabled=null deletes the key. The TLS refusal must then still fire, which catches a
# validator that reads .Values.ingress.enabled without paigasus.ingressEnabled.
expect_fail "ingress.enabled key absent, tlsSecretName empty" "ingress.tlsSecretName is required" \
  --set ingress.enabled=null --set ingress.tlsSecretName=""
# With the Ingress on, the API server refuses such a host. With it off, nothing else does, and
# https://https://… passes the console's https-URL parse.
expect_fail "ingress.host with a scheme" "must be a bare host name" \
  --set ingress.host=https://console.example.test
expect_fail "ingress.host with a port, ingress disabled" "must be a bare host name" \
  --set ingress.enabled=false --set ingress.host=console.example.test:8443
expect_fail "ingress.host with a path" "must be a bare host name" \
  --set ingress.host=console.example.test/iam
expect_fail "oidc.issuer empty" "oidc.issuer is required" \
  --set oidc.issuer=""
expect_fail "oidc.clientId empty" "oidc.clientId is required" \
  --set oidc.clientId=""
expect_fail "oidc.existingSecret empty" "oidc.existingSecret is required" \
  --set oidc.existingSecret=""
expect_fail "postgres.existingSecret empty" "postgres.existingSecret is required" \
  --set postgres.existingSecret=""
expect_fail "apiKeysPepperSecret empty" "apiKeysPepperSecret is required" \
  --set zones.iam.backend.apiKeysPepperSecret=""
expect_fail "iam backend.deploy false" "an external IAM is not supported yet" \
  --set zones.iam.backend.deploy=false
# The needle is a phrase unique to THIS message. An earlier row matched on the values path
# "zones.gateway.backend.deploy", which is also a substring of the generic backend.url refusal —
# so the row passed without the check existing. Only the mutation battery found that.
expect_fail "gateway backend.deploy true" "this chart cannot run the gateway backend" \
  --set zones.gateway.enabled=true --set zones.gateway.backend.deploy=true \
  --set zones.gateway.backend.url=http://gw.example.test:8088
# A basePath that is empty, unrooted, trailing-slashed or DUPLICATED renders an Ingress the API
# server rejects — or worse, two rules claiming one prefix, which makes a console unreachable and
# makes zoneMapFromJson throw in BOTH consoles at first request.
expect_fail "basePath without leading slash" "it must be a path beginning with" \
  --set zones.iam.basePath=iam
expect_fail "basePath with trailing slash" "it must not end in" \
  --set zones.iam.basePath=/iam/
expect_fail "duplicate basePath" "is already used by zone" \
  --set zones.gateway.enabled=true --set zones.gateway.backend.url=http://gw.example.test:8088 \
  --set zones.gateway.basePath=/iam
# oidc.caBundle (SMA-513 PR 3, spec § 5.2). The pods mount ONE key of the ConfigMap through
# `items`, so an empty key would render a volume item with no key, which the API server refuses at
# apply time, long after `helm template` said yes. Refuse it at render time instead.
expect_fail "oidc.caBundle key empty" "oidc.caBundle.key is empty while oidc.caBundle.existingConfigMap is set" \
  --set oidc.caBundle.existingConfigMap=paigasus-idp-ca --set oidc.caBundle.key=""

# SMA-692 D7. The console must not ask for a token that IAM refuses. The chart fails when
# oidc.authorizationAudience is set and oidc.audience is empty (IAM then accepts only the client
# id, and Auth0 refuses a client id as an API audience), or when the two values differ.
expect_fail "authorizationAudience with oidc.audience empty" "while oidc.audience is empty" \
  --set oidc.authorizationAudience=paigasus-console
expect_fail "authorizationAudience differs from oidc.audience" "does not equal oidc.audience" \
  --set oidc.audience=api://paigasus --set oidc.authorizationAudience=api://other
# Final fix M3. @paigasus/auth refuses the same value at pod start on the same rule; the chart
# must refuse it too, at render time, or an install can succeed while the console pod cannot.
expect_fail "authorizationAudience with surrounding whitespace" "has leading or trailing whitespace" \
  --set oidc.audience=api://paigasus --set "oidc.authorizationAudience= api://paigasus"
# SMA-692 D9. The scope list must hold the token openid. openidx does not count.
expect_fail "scopes without openid" "oidc.scopes must contain the scope openid" \
  --set "oidc.scopes=profile email offline_access"
expect_fail "scopes with openidx" "oidc.scopes must contain the scope openid" \
  --set "oidc.scopes=openidx profile email"
expect_render "authorizationAudience equal to oidc.audience" \
  --set oidc.audience=api://paigasus --set oidc.authorizationAudience=api://paigasus
expect_render "scopes with openid not first" \
  --set "oidc.scopes=profile email openid offline_access"
# Final fix I1. A whitespace-only oidc.scopes normalizes to empty, which renders no key — not a
# refusal, the same as an absent value.
expect_render "scopes whitespace-only" \
  --set "oidc.scopes=   "

# zones.iam.backend.bootstrapAdmins and extraEnv (SMA-697). A bad bootstrap admin is either a
# boot failure (IamConfig::validate) or an entry that IAM never matches. Both are refused here.
ADMIN=zones.iam.backend.bootstrapAdmins[0]
OK_ISSUER=https://idp.example.test/realms/paigasus
expect_fail "bootstrapAdmins not a list" "zones.iam.backend.bootstrapAdmins must be a list" \
  --set zones.iam.backend.bootstrapAdmins=admin
expect_fail "bootstrap admin not a map" "bootstrapAdmins[0] must be a map" \
  --set "zones.iam.backend.bootstrapAdmins={admin}"
expect_fail "bootstrap admin subject is a number" "issuer and subject must both be strings" \
  --set "$ADMIN.issuer=$OK_ISSUER" --set "$ADMIN.subject=392488538992280259"
expect_fail "bootstrap admin issuer missing" "bootstrapAdmins[0].issuer is empty or missing" \
  --set "$ADMIN.subject=admin-sub"
expect_fail "bootstrap admin issuer blank" "bootstrapAdmins[0].issuer is empty or missing" \
  --set-string "$ADMIN.issuer= " --set "$ADMIN.subject=admin-sub"
expect_fail "bootstrap admin subject missing" "bootstrapAdmins[0].subject is empty or missing" \
  --set "$ADMIN.issuer=$OK_ISSUER"
expect_fail "bootstrap admin issuer not https" "it must be an https URL" \
  --set "$ADMIN.issuer=http://idp.example.test/realms/paigasus" --set "$ADMIN.subject=admin-sub"
expect_fail "bootstrap admin issuer not oidc.issuer" "it must equal oidc.issuer" \
  --set "$ADMIN.issuer=https://other.example.test" --set "$ADMIN.subject=admin-sub"
expect_fail "bootstrap admin second entry checked" "bootstrapAdmins[1].subject is empty" \
  --set "$ADMIN.issuer=$OK_ISSUER" --set "$ADMIN.subject=admin-sub" \
  --set "zones.iam.backend.bootstrapAdmins[1].issuer=$OK_ISSUER" \
  --set-string "zones.iam.backend.bootstrapAdmins[1].subject= "
expect_fail "extraEnv not a list" "zones.iam.backend.extraEnv must be a list" \
  --set zones.iam.backend.extraEnv=RUST_LOG
expect_fail "extraEnv entry without a name" "extraEnv[0] must be a map with a non-empty string name" \
  --set "zones.iam.backend.extraEnv[0].value=debug"
expect_fail "extraEnv repeats a chart name" "the chart sets IAM_DATABASE_URL itself" \
  --set "zones.iam.backend.extraEnv[0].name=IAM_DATABASE_URL" \
  --set "zones.iam.backend.extraEnv[0].value=postgres://x"
expect_fail "extraEnv nests under a chart name" "the chart sets IAM_AUTHZ__BOOTSTRAP_ADMINS itself" \
  --set "zones.iam.backend.extraEnv[0].name=IAM_AUTHZ__BOOTSTRAP_ADMINS__0__SUBJECT" \
  --set "zones.iam.backend.extraEnv[0].value=admin-sub"
expect_fail "extraEnv duplicate name" "is already used by extraEnv[0]" \
  --set "zones.iam.backend.extraEnv[0].name=RUST_LOG" --set "zones.iam.backend.extraEnv[0].value=info" \
  --set "zones.iam.backend.extraEnv[1].name=RUST_LOG" --set "zones.iam.backend.extraEnv[1].value=debug"
expect_render "bootstrap admin and extraEnv" \
  --set "$ADMIN.issuer=$OK_ISSUER" --set-string "$ADMIN.subject=392488538992280259" \
  --set "zones.iam.backend.extraEnv[0].name=RUST_LOG" --set "zones.iam.backend.extraEnv[0].value=info"

# SMA-694 (spec § 4.4). The HTTPRoute block. ROUTE is a valid route-mode flag set, so a row
# below fails only for the value it names. Each needle carries its key path, so it cannot match
# another refusal.
ROUTE=(--set httpRoute.enabled=true --set 'httpRoute.parentRefs[0].name=gw'
  --set 'httpRoute.parentRefs[0].sectionName=https')
# R1. A quoted "false" is a string, and a non-empty string is true in a template `if`.
expect_fail "httpRoute.enabled a string" "httpRoute.enabled must be true or false" \
  --set-string httpRoute.enabled=false
# R2, R2b, R2c. An absent, null or non-list parentRefs attaches the route to nothing. R2b is the
# `helm upgrade --reuse-values` shape of a release made before the value (AC5). For R2c, helm
# 3.22.0 keeps the map over the default list with a coalesce warning (measured 2026-09-28).
expect_fail "httpRoute without parentRefs" "httpRoute.parentRefs is required" \
  --set httpRoute.enabled=true
expect_fail "httpRoute parentRefs null" "httpRoute.parentRefs is required" \
  --set httpRoute.enabled=true --set httpRoute.parentRefs=null
expect_fail "httpRoute parentRefs a map" "httpRoute.parentRefs is required" \
  --set httpRoute.enabled=true --set httpRoute.parentRefs.name=gw
# R3, R4. Each entry names a parent and selects one listener (D11).
expect_fail "httpRoute parentRef without name" "httpRoute.parentRefs[0] must be a map with a non-empty name" \
  --set httpRoute.enabled=true --set 'httpRoute.parentRefs[0].sectionName=https'
expect_fail "httpRoute parentRef without listener" "httpRoute.parentRefs[0] must set sectionName or port" \
  --set httpRoute.enabled=true --set 'httpRoute.parentRefs[0].name=gw'
# R5. `dig` needs a map.
expect_fail "httpRoute not a map" "httpRoute must be a map" \
  --set httpRoute=true
# R6. The API server refuses an HTTPRoute hostname with an upper-case letter.
expect_fail "httpRoute host not lowercase" "ingress.host must be lowercase when httpRoute.enabled" \
  "${ROUTE[@]}" --set ingress.host=Console.Example.test
# Review Focus 4. The lowercase rule is for route mode only. The Ingress path is unchanged.
expect_render "upper-case host, httpRoute off" \
  --set ingress.host=Console.Example.test
# R7. A port selects the listener too.
expect_render "httpRoute parentRef with port" \
  --set httpRoute.enabled=true --set 'httpRoute.parentRefs[0].name=gw' \
  --set 'httpRoute.parentRefs[0].port=443'
# R8, R9 (D14). The API server needs string annotation values; --set gives a boolean for "true".
expect_fail "httpRoute annotations not a map" "httpRoute.annotations must be a map" \
  "${ROUTE[@]}" --set httpRoute.annotations=x
expect_fail "httpRoute annotation not a string" "httpRoute.annotations.owner must be a string" \
  "${ROUTE[@]}" --set httpRoute.annotations.owner=true
# R10-R16 (D13). The chart refuses what the HTTPRoute v1 CRD refuses, before Argo CD sync time.
GW=(--set zones.gateway.enabled=true --set zones.gateway.backend.url=http://gw.example.test:8088)
expect_fail "httpRoute timeouts not a map" "httpRoute.timeouts must be a map" \
  "${ROUTE[@]}" --set httpRoute.timeouts=10s
expect_fail "httpRoute timeouts unknown key" "httpRoute.timeouts has the unknown key requestTimeout" \
  "${ROUTE[@]}" --set httpRoute.timeouts.requestTimeout=10s
expect_fail "httpRoute timeout bad form" "httpRoute.timeouts.request must be a duration" \
  "${ROUTE[@]}" --set-string httpRoute.timeouts.request=10
expect_fail "httpRoute timeout a number" "httpRoute.timeouts.request must be a duration" \
  "${ROUTE[@]}" --set httpRoute.timeouts.request=10
expect_fail "zone timeout bad form" "zones.gateway.console.httpRouteTimeouts.request must be a duration" \
  "${ROUTE[@]}" "${GW[@]}" --set zones.gateway.console.httpRouteTimeouts.request=ten
expect_fail "httpRoute backendRequest longer than request" "zones.iam: the HTTPRoute backendRequest timeout 1m" \
  "${ROUTE[@]}" --set httpRoute.timeouts.request=10s --set httpRoute.timeouts.backendRequest=1m
expect_render "httpRoute request 0s with backendRequest" \
  "${ROUTE[@]}" --set httpRoute.timeouts.request=0s --set httpRoute.timeouts.backendRequest=1m
expect_render "zone timeout on a disabled zone is not checked" \
  "${ROUTE[@]}" --set zones.gateway.enabled=false --set zones.gateway.console.httpRouteTimeouts.request=ten
# Review Focus 1. "ms" is not "m": 1s is longer than 900ms, and 59s999ms is shorter than 1m.
expect_fail "httpRoute backendRequest 1s over request 900ms" "zones.iam: the HTTPRoute backendRequest timeout 1s" \
  "${ROUTE[@]}" --set httpRoute.timeouts.request=900ms --set httpRoute.timeouts.backendRequest=1s
expect_render "httpRoute backendRequest 59s999ms under request 1m" \
  "${ROUTE[@]}" --set httpRoute.timeouts.request=1m --set httpRoute.timeouts.backendRequest=59s999ms
# Review Focus 2. The gateway zone's 10m default is its merged request, so a chart-wide
# backendRequest of 20m is refused for that zone only.
expect_fail "chart backendRequest over the zone default" "zones.gateway: the HTTPRoute backendRequest timeout 20m" \
  "${ROUTE[@]}" "${GW[@]}" --set httpRoute.timeouts.backendRequest=20m

# SMA-703 (spec D5). oidc.idTokenMarkerClaims copies the IamConfig::validate rules, because a boot
# failure stops the one IAM replica. Each needle carries the key path and the index.
MARKERS=oidc.idTokenMarkerClaims
expect_fail "markers not a list" "oidc.idTokenMarkerClaims must be a list" \
  --set "$MARKERS=at_hash"
expect_fail "markers item a number" "oidc.idTokenMarkerClaims[0] must be a string" \
  --set "$MARKERS={123}"
# Review Focus 1. `--set x={}` does not clear a list: helm 3.22.0 makes it [""].
expect_fail "markers set to {}" "oidc.idTokenMarkerClaims[0] is empty" \
  --set "$MARKERS={}"
# A space after the comma in --set gives the item " azp".
expect_fail "markers item with a space" "oidc.idTokenMarkerClaims[1] is \" azp\"" \
  --set "$MARKERS={at_hash, azp}"
expect_fail "markers item with a tab" "oidc.idTokenMarkerClaims[0] is \"at_hash\\t\"" \
  --set-string "$MARKERS[0]=$(printf 'at_hash\t')"
# A control character: Go's %q writes \x01, which figment cannot read.
expect_fail "markers item with a control character" "oidc.idTokenMarkerClaims[0] is \"a\\x01b\"" \
  --set-string "$MARKERS[0]=$(printf 'a\001b')"
expect_fail "markers item with a quote" "oidc.idTokenMarkerClaims[0] is \"a\\\"b\"" \
  --set-string "$MARKERS[0]=a\"b"
# A name that is not ASCII. The source of this script stays ASCII, so the needle is a prefix.
expect_fail "markers item not ASCII" "oidc.idTokenMarkerClaims[0] is \"azp" \
  --set-string "$MARKERS[0]=$(printf 'azp\303\251')"
# A backslash. --set-string cannot carry one (Helm reads it as an escape), so a values file does.
# The file holds a YAML single-quoted string with one backslash. Go's %q doubles it in the message.
MARKERS_BS="$(mktemp)"
printf 'oidc:\n  idTokenMarkerClaims: ['"'"'a\\b'"'"']\n' >"$MARKERS_BS"
expect_fail "markers item with a backslash" "oidc.idTokenMarkerClaims[0] is \"a\\\\b\"" \
  -f "$MARKERS_BS"
rm -f "$MARKERS_BS"
# One row for each reserved name. The name goes at index 1, after a valid name.
for reserved in iss sub aud exp; do
  expect_fail "markers reserved name $reserved" "oidc.idTokenMarkerClaims[1] is \"$reserved\": every access token carries" \
    --set "$MARKERS={azp,$reserved}"
done
expect_fail "markers duplicate" "oidc.idTokenMarkerClaims[2] \"at_hash\" is already in oidc.idTokenMarkerClaims[0]" \
  --set "$MARKERS={at_hash,azp,at_hash}"
expect_render "markers Zitadel recipe" \
  --set "$MARKERS={at_hash,azp}"

# SMA-731 (spec D5). oidc.accessTokenRequiredClaims goes through the same template as
# oidc.idTokenMarkerClaims (paigasus.validateClaimNameList), with its own path, example and
# reserved-name reason. One more rule: a name must not be in both lists. Each needle carries the
# key path and the index.
REQUIRED=oidc.accessTokenRequiredClaims
expect_fail "required not a list" "oidc.accessTokenRequiredClaims must be a list of claim names, for example [\"jti\"]" \
  --set "$REQUIRED=jti"
expect_fail "required item a number" "oidc.accessTokenRequiredClaims[0] must be a string" \
  --set "$REQUIRED={123}"
# `--set x={}` does not clear a list: helm 3.22.0 makes it [""].
expect_fail "required set to {}" "oidc.accessTokenRequiredClaims[0] is empty. IamConfig::validate refuses an empty name, and IAM does not boot. To remove the value, use [] in a values file or --set-json 'oidc.accessTokenRequiredClaims=[]'" \
  --set "$REQUIRED={}"
expect_fail "required item with a space" "oidc.accessTokenRequiredClaims[1] is \" nbf\"" \
  --set "$REQUIRED={jti, nbf}"
expect_fail "required item with a control character" "oidc.accessTokenRequiredClaims[0] is \"a\\x01b\"" \
  --set-string "$REQUIRED[0]=$(printf 'a\001b')"
expect_fail "required item with a quote" "oidc.accessTokenRequiredClaims[0] is \"a\\\"b\"" \
  --set-string "$REQUIRED[0]=a\"b"
expect_fail "required item not ASCII" "oidc.accessTokenRequiredClaims[0] is \"jti" \
  --set-string "$REQUIRED[0]=$(printf 'jti\303\251')"
REQUIRED_BS="$(mktemp)"
printf 'oidc:\n  accessTokenRequiredClaims: ['"'"'a\\b'"'"']\n' >"$REQUIRED_BS"
expect_fail "required item with a backslash" "oidc.accessTokenRequiredClaims[0] is \"a\\\\b\"" \
  -f "$REQUIRED_BS"
rm -f "$REQUIRED_BS"
for reserved in iss sub aud exp; do
  expect_fail "required reserved name $reserved" "oidc.accessTokenRequiredClaims[1] is \"$reserved\": every token that IAM accepts carries this claim, so the name has no effect" \
    --set "$REQUIRED={jti,$reserved}"
done
expect_fail "required duplicate" "oidc.accessTokenRequiredClaims[2] \"jti\" is already in oidc.accessTokenRequiredClaims[0]" \
  --set "$REQUIRED={jti,nbf,jti}"
expect_fail "required name also a marker" "oidc.accessTokenRequiredClaims[1] \"azp\" is also in oidc.idTokenMarkerClaims[1]" \
  --set "$MARKERS={at_hash,azp}" --set "$REQUIRED={jti,azp}"
expect_render "required Zitadel recipe" \
  --set "$REQUIRED={jti}"
expect_render "required and markers, Zitadel recipe" \
  --set "$MARKERS={at_hash,azp}" --set "$REQUIRED={jti}"
# Review Focus 2. Names compare exactly, as in IAM: JTI and jti are two names.
expect_render "required JTI and marker jti" \
  --set "$MARKERS={jti}" --set "$REQUIRED={JTI}"

# SMA-700 (spec § 4.11). zones.iam.backend.dpop copies the IamConfig::validate rules for the URL
# list, because a refused boot stops the one IAM replica. Each needle carries the key path.
DPOP=zones.iam.backend.dpop
expect_fail "dpop on with no URL" "zones.iam.backend.dpop.enabled is true and zones.iam.backend.dpop.forwardedBaseUrls is empty" \
  --set "$DPOP.enabled=true"
expect_fail "dpop enabled not a bool" "zones.iam.backend.dpop.enabled must be true or false" \
  --set-string "$DPOP.enabled=yes"
expect_fail "dpop URLs not a list" "zones.iam.backend.dpop.forwardedBaseUrls must be a list of URLs" \
  --set "$DPOP.forwardedBaseUrls=https://gw.example.test"
expect_fail "dpop URL http not loopback" "zones.iam.backend.dpop.forwardedBaseUrls[0] is \"http://gw.example.test\": use https" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=http://gw.example.test"
expect_fail "dpop URL ftp" "zones.iam.backend.dpop.forwardedBaseUrls[0] is \"ftp://gw.example.test\": use https" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=ftp://gw.example.test"
expect_fail "dpop URL with no scheme" "zones.iam.backend.dpop.forwardedBaseUrls[0] is \"gw.example.test\": use https" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=gw.example.test"
expect_fail "dpop URL with user info" "zones.iam.backend.dpop.forwardedBaseUrls[0] is \"https://user@gw.example.test\": it must have no query, fragment or user info" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=https://user@gw.example.test"
expect_fail "dpop URL with a fragment" "zones.iam.backend.dpop.forwardedBaseUrls[0] is \"https://gw.example.test/#x\": it must have no query, fragment or user info" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=https://gw.example.test/#x"
# A query holds `=`, which --set reads as a second key. A values file carries it as written.
DPOP_QUERY="$(mktemp)"
printf 'zones:\n  iam:\n    backend:\n      dpop:\n        enabled: true\n        forwardedBaseUrls: ["https://gw.example.test/?a=1"]\n' >"$DPOP_QUERY"
expect_fail "dpop URL with a query" "zones.iam.backend.dpop.forwardedBaseUrls[0] is \"https://gw.example.test/?a=1\": it must have no query, fragment or user info" \
  -f "$DPOP_QUERY"
rm -f "$DPOP_QUERY"
expect_fail "dpop URL with a space" "zones.iam.backend.dpop.forwardedBaseUrls[0] is \"https://gw.example.test/a b\": use printable ASCII only" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=https://gw.example.test/a b"
# Go's url.Parse is lenient where IAM's url::Url::parse is strict. The chart is stricter.
expect_fail "dpop URL port above 65535" "forwardedBaseUrls[0] is \"https://gw.example.test:99999\": its port must be from 1 to 65535" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=https://gw.example.test:99999"
expect_fail "dpop URL loopback octet above 255" "forwardedBaseUrls[0] is \"http://127.0.0.256\"" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=http://127.0.0.256"
expect_fail "dpop URL IPv4 octet above 255" "forwardedBaseUrls[0] is \"https://999.1.1.1\": its IPv4 host has an octet above 255" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=https://999.1.1.1"
expect_fail "dpop URL host with a numeric last label" "forwardedBaseUrls[0] is \"https://gw.1\": its host ends in a number" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=https://gw.1"
expect_fail "dpop URL host with a hex last label" "forwardedBaseUrls[0] is \"https://gw.0x1\": its host ends in a number" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=https://gw.0x1"
expect_fail "dpop URL host with an empty hex last label" "forwardedBaseUrls[0] is \"https://gw.0X\": its host ends in a number" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=https://gw.0X"
expect_fail "dpop URL host with a forbidden code point" "forwardedBaseUrls[0] is \"https://a<b.example\"" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=https://a<b.example"
expect_fail "dpop URL with a dollar sign" "forwardedBaseUrls[0] is \"https://gw.example.test/\$(X)\": use printable ASCII only" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=https://gw.example.test/\$(X)"
# Decision P8: a bad entry is refused also while DPoP is off.
expect_fail "dpop off with a bad URL" "zones.iam.backend.dpop.forwardedBaseUrls[0] is \"http://gw.example.test\": use https" \
  --set "$DPOP.forwardedBaseUrls[0]=http://gw.example.test"
expect_fail "extraEnv sets the DPoP switch" "the chart sets IAM_AUTHN__DPOP__ENABLED itself" \
  --set 'zones.iam.backend.extraEnv[0].name=IAM_AUTHN__DPOP__ENABLED' --set 'zones.iam.backend.extraEnv[0].value=true'
expect_render "dpop https with a prefix" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=https://gw.example.test" --set "$DPOP.forwardedBaseUrls[1]=https://edge.example.test/api/"
expect_render "dpop loopback http" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=http://localhost:8088" --set "$DPOP.forwardedBaseUrls[1]=http://127.0.0.1:8088"
# Review Focus 5.
expect_render "dpop IPv6 loopback http" \
  --set "$DPOP.enabled=true" --set "$DPOP.forwardedBaseUrls[0]=http://[::1]:8088"
expect_render "dpop reuse-values-no-key" --set "$DPOP=null"

expect_render "iam only" --set zones.gateway.enabled=false
expect_render "iam and gateway" --set zones.gateway.enabled=true \
  --set zones.gateway.backend.url=http://gw.example.test:8088

if [ "$ec" -eq 0 ]; then echo "== chart refusals OK =="; fi
exit "$ec"
