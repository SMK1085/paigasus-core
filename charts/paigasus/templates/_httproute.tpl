{{/* SPDX-License-Identifier: Apache-2.0 */}}

{{/*
The chart-owned HTTPRoute (SMA-694). Each helper in this file takes the ROOT context, unless its
comment says otherwise. Every caller reads the block as `.Values.httpRoute | default dict`: `dig`
needs a map, and under `helm upgrade --reuse-values` a release made before this block existed has
no httpRoute map. paigasus.validateHttpRoute refuses a httpRoute that is not a map before any other
helper reads it. This file has no negative-control fixture copy; a fixture chart gets it from the
live chart (ci/helm-render/run.sh copies the live chart, then overlays the fixture).
*/}}

{{/*
paigasus.consoleServicePort: the port NUMBER of each console Service. An HTTPRoute backendRef needs
a number and cannot name the port "http" as the Ingress does. console-service.yaml and
httproute.yaml both read it here, so the two cannot drift (spec D6). The container side (the
containerPort and PORT in console-deployment.yaml) is reached by the port name http.
*/}}
{{- define "paigasus.consoleServicePort" -}}
3000
{{- end -}}

{{/*
paigasus.httpRouteEnabled yields "true" or "". Nil counts as not set, which is false here.
paigasus.validateHttpRoute refuses a non-boolean value, so a quoted "false" never reaches an `if`.
Read httpRoute.enabled ONLY through this helper.
*/}}
{{- define "paigasus.httpRouteEnabled" -}}
{{- $h := .Values.httpRoute | default dict -}}
{{- if kindIs "map" $h -}}
{{- if dig "enabled" false $h -}}true{{- end -}}
{{- end -}}
{{- end -}}

{{/*
paigasus.validateHttpRoute: the refusals for the httpRoute block (spec § 4.4). paigasus.validate
calls it last, so the ingress.host refusals (required, bare host) have already run. An ABSENT key
counts as its empty value: under `helm upgrade --reuse-values` a release made before this change
has no parentRefs key, and the API server accepts an HTTPRoute with no parentRefs, which attaches
to nothing.
*/}}
{{- define "paigasus.validateHttpRoute" -}}
{{- $raw := .Values.httpRoute -}}
{{- if not (or (kindIs "invalid" $raw) (kindIs "map" $raw)) -}}
{{- fail (printf "httpRoute must be a map, got %s" (kindOf $raw)) -}}
{{- end -}}
{{- $h := $raw | default dict -}}
{{- $en := dig "enabled" false $h -}}
{{- if not (or (kindIs "bool" $en) (kindIs "invalid" $en)) -}}
{{- fail (printf "httpRoute.enabled must be true or false (a boolean), got %s %q; a quoted \"false\" is a string and would render the HTTPRoutes" (kindOf $en) (toString $en)) -}}
{{- end -}}
{{- if (include "paigasus.httpRouteEnabled" .) -}}
{{- $refs := dig "parentRefs" list $h -}}
{{- if not (and (kindIs "slice" $refs) $refs) -}}
{{- fail "httpRoute.parentRefs is required when httpRoute.enabled is true: a non-empty list that names the listener serving ingress.host" -}}
{{- end -}}
{{- range $i, $r := $refs -}}
{{- if not (and (kindIs "map" $r) (kindIs "string" $r.name) $r.name) -}}
{{- fail (printf "httpRoute.parentRefs[%d] must be a map with a non-empty name" $i) -}}
{{- end -}}
{{- if not (or $r.sectionName $r.port) -}}
{{- fail (printf "httpRoute.parentRefs[%d] must set sectionName or port to select the HTTPS listener; without one the route attaches to every listener, a plain HTTP one included" $i) -}}
{{- end -}}
{{- end -}}
{{- $host := .Values.ingress.host | toString -}}
{{- if ne $host (lower $host) -}}
{{- fail (printf "ingress.host must be lowercase when httpRoute.enabled is true (Gateway API hostnames are lowercase RFC 1123 names), got %q" $host) -}}
{{- end -}}
{{- end -}}
{{- end -}}
