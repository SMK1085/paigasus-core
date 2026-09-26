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
