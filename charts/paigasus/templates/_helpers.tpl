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
{{- if not .Values.postgres.host -}}
{{- fail "postgres.host is required by the IAM backend" -}}
{{- end -}}
{{- if not .Values.postgres.existingSecret -}}
{{- fail "postgres.existingSecret is required; it must hold key \"password\"" -}}
{{- end -}}
{{- if not .Values.zones.iam.backend.apiKeysPepperSecret -}}
{{- fail "zones.iam.backend.apiKeysPepperSecret is required; IamConfig::validate hard-fails boot without it" -}}
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
{{- $full := include "paigasus.fullname" . -}}
{{- range $id, $z := .Values.zones -}}
{{- if $z.enabled -}}
{{- if $z.backend.deploy -}}
{{- $_ := set $m $id (printf "http://%s-%s-backend:%d" $full $id (int $z.backend.httpPort)) -}}
{{- else -}}
{{- $_ := set $m $id $z.backend.url -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- toJson $m -}}
{{- end -}}
