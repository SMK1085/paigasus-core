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

expect_render "iam only" --set zones.gateway.enabled=false
expect_render "iam and gateway" --set zones.gateway.enabled=true \
  --set zones.gateway.backend.url=http://gw.example.test:8088

if [ "$ec" -eq 0 ]; then echo "== chart refusals OK =="; fi
exit "$ec"
