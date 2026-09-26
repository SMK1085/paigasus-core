{{/* SPDX-License-Identifier: Apache-2.0 */}}

{{/*
The IAM access-token audience (SMA-678, SMA-691). Each helper in this file takes the ROOT context.
A caller inside the zone range of backend-deployment.yaml passes $root, never "." (inside that
range, "." is the zone map).
*/}}

{{/*
paigasus.iamAudience: the one audience that IAM accepts, as a string. An empty or absent (nil)
oidc.audience gives oidc.clientId. A set value REPLACES oidc.clientId (SMA-678). toString comes
from SMA-678. MEASURED (SMA-691): include already prints a number as its digits, so toString
changes no render here.
*/}}
{{- define "paigasus.iamAudience" -}}
{{- .Values.oidc.audience | default .Values.oidc.clientId | toString -}}
{{- end -}}

{{/*
paigasus.iamAudienceWarns: "true" when both conditions are true, else "".
  1. The IAM audience (paigasus.iamAudience) equals oidc.clientId. An OIDC ID token has the
     client id as its aud, so an ID token then passes IAM's audience check.
  2. oidc.acknowledgeClientIdAudience does not equal oidc.clientId. The compare is exact, as
     strings. A nil value (helm upgrade --reuse-values from a release made before the key
     existed) gives "<nil>" from toString, so the warning shows.
There is no "the IAM backend is deployed" condition: paigasus.validate refuses every render
without the IAM backend. Both signals (NOTES.txt and the paigasus.io/iam-audience-warning
annotation) call this helper. No other file repeats the condition.
*/}}
{{- define "paigasus.iamAudienceWarns" -}}
{{- $clientId := toString .Values.oidc.clientId -}}
{{- if and (eq (include "paigasus.iamAudience" .) $clientId) (ne (toString .Values.oidc.acknowledgeClientIdAudience) $clientId) -}}
true
{{- end -}}
{{- end -}}

{{/*
paigasus.iamAudienceNotes: the NOTES.txt body when paigasus.iamAudienceWarns is "true", else "".
NOTES.txt only includes this helper, so charts/paigasus/tests/env.sh can test the text offline
(rows N0-N6). The first line is the marker that ci/kind/run.sh asserts. Keep the text equal to
§ 4.3 of docs/superpowers/specs/2026-09-26-sma-691-default-audience-warning-design.md.
*/}}
{{- define "paigasus.iamAudienceNotes" -}}
{{- if include "paigasus.iamAudienceWarns" . -}}
WARNING (SMA-691): the IAM audience equals oidc.clientId, so an ID token passes IAM's audience check.
IAM accepts the audience {{ include "paigasus.iamAudience" . | quote }}. An OIDC ID token has the client id as its audience.
IAM refuses a Keycloak ID token by its typ claim (SMA-686). Dex does not set that claim.
Other IdPs are not measured.
Recommended: give the API its own audience and set oidc.audience to it.
Follow the order in docs/ops/RUNBOOK-chart.md section 6, or every session breaks.
If your IdP cannot do this (Dex), set oidc.acknowledgeClientIdAudience to the value of
oidc.clientId to remove this warning.
{{- end -}}
{{- end -}}
