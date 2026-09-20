{{/* SPDX-License-Identifier: Apache-2.0 */}}

{{/*
The closed service-slug registry. parseServiceMap validates every PAIGASUS_SERVICES key against
the TypeScript SERVICE_SLUGS and THROWS on an unknown one, in both consoles, at first request.
Keep this list equal to ts/packages/paigasus-discovery/src/core/state.ts.
*/}}
{{- define "paigasus.serviceSlugs" -}}
iam gateway
{{- end -}}

{{- define "paigasus.fullname" -}}
{{- printf "%s-%s" .Release.Name .Chart.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/*
paigasus.name truncates the COMBINED name (release-chart-suffix), not the base. Callers append a
suffix such as "-gateway-console" (16 chars), "-iam-console" (12), "-iam-backend" (12), "-zonemap"
(8) or "-console-env" (12) to paigasus.fullname's already-63-char-truncated base, which can push a
long release name's resource names past Kubernetes' 63-character limit. Call with a two-element
list: (list $ "<suffix>"), e.g. (list $ (printf "%s-backend" $id)).
*/}}
{{/*
Build a resource name that keeps its SUFFIX intact.

Truncating the combined string is not enough. MEASURED at a 52-character release name: the suffix
is cut away entirely, so `iam-backend` and `iam-console` both become `<release>-paigasus-i` — two
Deployments and two Services sharing one name. `helm install` then applies one over the other and
silently destroys a resource, while `helm template` renders it happily and `helm lint` passes.

So the BASE is truncated to whatever room the suffix leaves, and the suffix is appended afterwards.
Distinct suffixes therefore always yield distinct names, whatever the release is called.
*/}}
{{- define "paigasus.name" -}}
{{- $ctx := index . 0 -}}
{{- $suffix := index . 1 -}}
{{- $room := int (sub 63 (add1 (len $suffix))) -}}
{{- $base := printf "%s-%s" $ctx.Release.Name $ctx.Chart.Name | trunc $room | trimSuffix "-" -}}
{{- printf "%s-%s" $base $suffix -}}
{{- end -}}

{{- define "paigasus.enabledZones" -}}
{{- $out := list -}}
{{- range $id, $z := .Values.zones -}}
{{- if $z.enabled -}}{{- $out = append $out $id -}}{{- end -}}
{{- end -}}
{{- join " " (sortAlpha $out) -}}
{{- end -}}

{{/*
Every refusal lives here, and every template calls this first. Helm aborts on the first `fail` it
reaches and evaluation order across files is not a stable contract, so a refusal placed in one
template would fire only when that template happens to render first.
*/}}
{{- define "paigasus.validate" -}}
{{- $enabled := splitList " " (include "paigasus.enabledZones" .) | compact -}}
{{- $slugs := splitList " " (include "paigasus.serviceSlugs" .) -}}
{{- range $id, $z := .Values.zones -}}
{{- if and $z.enabled (not (has $id $slugs)) -}}
{{- fail (printf "zones.%s: \"%s\" is not a known service slug (expected one of %s). parseServiceMap would throw in BOTH consoles at first request." $id $id (join ", " $slugs)) -}}
{{- end -}}
{{- end -}}
{{- if not .Values.ingress.tlsSecretName -}}
{{- fail "ingress.tlsSecretName is required; PAIGASUS_PUBLIC_ORIGIN is validated as https and __Host-pgs_sid requires Secure, so the ingress must terminate TLS" -}}
{{- end -}}
{{- if not .Values.oidc.issuer -}}
{{- fail "oidc.issuer is required" -}}
{{- end -}}
{{- if not .Values.oidc.clientId -}}
{{- fail "oidc.clientId is required" -}}
{{- end -}}
{{- if not .Values.oidc.existingSecret -}}
{{- fail "oidc.existingSecret is required; it must hold keys oidc-client-secret and session-redis-url" -}}
{{- end -}}
{{- if not .Values.postgres.existingSecret -}}
{{- fail "postgres.existingSecret is required; it must hold key \"database-url\", the complete Postgres DSN" -}}
{{- end -}}
{{- if not .Values.zones.iam.backend.apiKeysPepperSecret -}}
{{- fail "zones.iam.backend.apiKeysPepperSecret is required; IamConfig::validate hard-fails boot without it" -}}
{{- end -}}
{{- if and .Values.zones.gateway.enabled .Values.zones.gateway.backend.deploy -}}
{{- fail "zones.gateway.backend.deploy is true: this chart cannot run the gateway backend. GatewayConfig::validate hard-fails on an empty upstream.openai.api_key, and iam.grpc_addr accepts LoopbackInsecure only for a loopback host, so an in-cluster gateway-to-IAM link needs a TLS design this chart does not have. Supply the address through zones.gateway.backend.url instead (decision D9)" -}}
{{- end -}}
{{- if not .Values.zones.iam.backend.deploy -}}
{{- fail "zones.iam.backend.deploy is false: this chart deploys the IAM backend itself (see decision D2), and an external IAM is not supported yet" -}}
{{- end -}}
{{- if eq (len $enabled) 0 -}}
{{- fail "at least one zone must be enabled; a chart with no zone serves nothing" -}}
{{- end -}}
{{- if and (has "gateway" $enabled) (not (has "iam" $enabled)) -}}
{{- fail "the gateway zone requires the iam zone: gateway-console's PAIGASUS_SERVICES schema refuses to construct without an \"iam\" entry, so its pods would crash-loop" -}}
{{- end -}}
{{- range $id, $z := .Values.zones -}}
{{- if and $z.enabled (not $z.backend.deploy) (not $z.backend.url) -}}
{{- fail (printf "zones.%s.backend.url is required when zones.%s.backend.deploy is false" $id $id) -}}
{{- end -}}
{{- end -}}
{{- if not .Values.ingress.host -}}
{{- fail "ingress.host is required; it is the single origin every zone's cookie is scoped to" -}}
{{- end -}}
{{- end -}}

{{- define "paigasus.zoneMapJson" -}}
{{- $m := dict -}}
{{- range $id, $z := .Values.zones -}}
{{- if $z.enabled -}}{{- $_ := set $m $id $z.basePath -}}{{- end -}}
{{- end -}}
{{- toJson $m -}}
{{- end -}}

{{- define "paigasus.serviceMapJson" -}}
{{- $m := dict -}}
{{- $root := . -}}
{{- range $id, $z := .Values.zones -}}
{{- if $z.enabled -}}
{{- if $z.backend.deploy -}}
{{- $svcName := include "paigasus.name" (list $root (printf "%s-backend" $id)) -}}
{{- $_ := set $m $id (printf "http://%s:%d" $svcName (int $z.backend.httpPort)) -}}
{{- else -}}
{{- $_ := set $m $id $z.backend.url -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- toJson $m -}}
{{- end -}}
