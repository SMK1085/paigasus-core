#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# Every rendered resource name must be a legal Kubernetes name AND unique within its kind, for a
# release name of any length.
#
# MEASURED before this test existed: at a 52-character release name the suffix was truncated away
# entirely, so `iam-backend` and `iam-console` both rendered as `<release>-paigasus-i` — two
# Deployments and two Services sharing one name. `helm install` applies one over the other and
# silently destroys a resource. Neither `helm template` nor `helm lint` says a word about it, which
# is why this needs its own assertion rather than trusting the renderer.
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

check() {
  local label="$1" release="$2"; shift 2
  local out
  if ! out="$(helm template "$release" "$CHART" "${BASE[@]}" "$@" 2>&1)"; then
    echo "FAIL [$label]: render failed"; printf '%s\n' "$out"; ec=1; return
  fi
  local verdict
  verdict="$(printf '%s' "$out" | python3 -c '
import sys, yaml, collections, re
docs = [d for d in yaml.safe_load_all(sys.stdin) if d]
names = [(d["kind"], d["metadata"]["name"]) for d in docs]
problems = []
# DNS-1123 label, which is what a Kubernetes object name must be: lowercase alphanumerics and
# hyphens only, no leading or trailing hyphen, at most 63 characters. Length alone is not enough —
# a truncation that lands on a hyphen, or a suffix source that ever admits an underscore or an
# uppercase letter, produces a name the API server rejects at apply time and nothing here would
# have said so.
DNS1123 = re.compile(r"^[a-z0-9]([-a-z0-9]*[a-z0-9])?$")
for kind, name in names:
    if len(name) > 63:
        problems.append(f"{kind}/{name} is {len(name)} chars, over the 63 limit")
    elif not DNS1123.match(name):
        problems.append(f"{kind}/{name} is not a valid DNS-1123 label")
for (kind, name), n in collections.Counter(names).items():
    if n > 1:
        problems.append(f"{n} {kind} objects share the name {name}")
longest = max((len(n) for _, n in names), default=0)
print("|".join(problems) if problems else f"OK {len(names)} objects, longest {longest}")')"
  if [ "${verdict:0:2}" = "OK" ]; then
    echo "  ok [$label]: $verdict"
  else
    echo "FAIL [$label]: $verdict"; ec=1
  fi
}

# 53 is Helm's own ceiling: a longer release name is refused outright with
# "the length must not be longer than 53" (measured 2026-09-20 on helm 3.22.0), so it is the
# worst case this chart can actually be installed under.
check "short release"   paigasus                                              --set zones.gateway.enabled=true
check "40-char release" my-very-long-release-name-for-console-xy              --set zones.gateway.enabled=true
check "52-char release" aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  --set zones.gateway.enabled=true
check "53-char release" aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa --set zones.gateway.enabled=true

if [ "$ec" -eq 0 ]; then echo "== chart names OK =="; fi
exit "$ec"
