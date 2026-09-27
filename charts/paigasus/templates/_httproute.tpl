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
paigasus.httpRouteTimeouts takes (list $root $z) and yields the merged timeouts of one zone's rule
as YAML, or "" when no key is set (spec D13). A non-empty key in
zones.<id>.console.httpRouteTimeouts wins over the same key in httpRoute.timeouts. The merge is
per key, and an empty string is "not set". Both maps are read with dig: a release made before this
change has neither key.
*/}}
{{- define "paigasus.httpRouteTimeouts" -}}
{{- $root := index . 0 -}}
{{- $z := index . 1 -}}
{{- $h := $root.Values.httpRoute | default dict -}}
{{- $chart := dig "timeouts" dict $h | default dict -}}
{{- $zone := dig "console" "httpRouteTimeouts" dict $z | default dict -}}
{{- $out := dict -}}
{{- range $k := list "request" "backendRequest" -}}
{{- $v := index $zone $k | default (index $chart $k) -}}
{{- if $v -}}{{- $_ := set $out $k ($v | toString) -}}{{- end -}}
{{- end -}}
{{- if $out -}}{{- toYaml $out -}}{{- end -}}
{{- end -}}

{{/*
paigasus.durationMs takes a Gateway API Duration string (GEP-2257) that already matches
^([0-9]{1,5}(h|m|s|ms)){1,4}$, and yields its value in milliseconds. "ms" comes first in the
alternation, so "10ms" is not read as ten minutes.
*/}}
{{- define "paigasus.durationMs" -}}
{{- $total := 0 -}}
{{- range $part := regexFindAll "[0-9]+(ms|h|m|s)" . -1 -}}
{{- $digits := regexFind "^[0-9]+" $part -}}
{{- $n := int $digits -}}
{{- $unit := trimPrefix $digits $part -}}
{{- if eq $unit "h" -}}{{- $total = add $total (mul $n 3600000) -}}{{- end -}}
{{- if eq $unit "m" -}}{{- $total = add $total (mul $n 60000) -}}{{- end -}}
{{- if eq $unit "s" -}}{{- $total = add $total (mul $n 1000) -}}{{- end -}}
{{- if eq $unit "ms" -}}{{- $total = add $total $n -}}{{- end -}}
{{- end -}}
{{- $total -}}
{{- end -}}

{{/*
paigasus.validateHttpRouteTimeouts takes (list <path> <value>) and refuses a timeouts map that the
HTTPRoute v1 CRD would refuse at apply time (spec § 4.4, checks 5 to 7). An absent value (nil)
passes: it means "not set". So does an empty string for a key.
*/}}
{{- define "paigasus.validateHttpRouteTimeouts" -}}
{{- $path := index . 0 -}}
{{- $m := index . 1 -}}
{{- if not (kindIs "invalid" $m) -}}
{{- if not (kindIs "map" $m) -}}
{{- fail (printf "%s must be a map with the optional keys request and backendRequest, got %s" $path (kindOf $m)) -}}
{{- end -}}
{{- range $k, $v := $m -}}
{{- if not (has $k (list "request" "backendRequest")) -}}
{{- fail (printf "%s has the unknown key %s; the allowed keys are request and backendRequest" $path $k) -}}
{{- end -}}
{{- if not (or (kindIs "invalid" $v) (and (kindIs "string" $v) (eq $v ""))) -}}
{{- if not (and (kindIs "string" $v) (regexMatch "^([0-9]{1,5}(h|m|s|ms)){1,4}$" (toString $v))) -}}
{{- fail (printf "%s.%s must be a duration such as 30s, 10m or 1h, got %s %q" $path $k (kindOf $v) (toString $v)) -}}
{{- end -}}
{{- end -}}
{{- end -}}
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
{{- $ann := dig "annotations" dict $h -}}
{{- if not (or (kindIs "invalid" $ann) (kindIs "map" $ann)) -}}
{{- fail (printf "httpRoute.annotations must be a map, got %s" (kindOf $ann)) -}}
{{- end -}}
{{- range $k, $v := ($ann | default dict) -}}
{{- if not (kindIs "string" $v) -}}
{{- fail (printf "httpRoute.annotations.%s must be a string, got %s; quote it in a values file or use --set-string" $k (kindOf $v)) -}}
{{- end -}}
{{- end -}}
{{- include "paigasus.validateHttpRouteTimeouts" (list "httpRoute.timeouts" (dig "timeouts" nil $h)) -}}
{{- $root := . -}}
{{- range $id, $z := .Values.zones -}}
{{- if $z.enabled -}}
{{- include "paigasus.validateHttpRouteTimeouts" (list (printf "zones.%s.console.httpRouteTimeouts" $id) (dig "console" "httpRouteTimeouts" nil $z)) -}}
{{- $t := include "paigasus.httpRouteTimeouts" (list $root $z) | fromYaml -}}
{{- if and $t.request $t.backendRequest -}}
{{- $r := include "paigasus.durationMs" $t.request | int64 -}}
{{- $b := include "paigasus.durationMs" $t.backendRequest | int64 -}}
{{- if and (ne $r 0) (gt $b $r) -}}
{{- fail (printf "zones.%s: the HTTPRoute backendRequest timeout %s is longer than the request timeout %s; the HTTPRoute CRD refuses it (a request of 0s means no timeout and allows any backendRequest)" $id $t.backendRequest $t.request) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
