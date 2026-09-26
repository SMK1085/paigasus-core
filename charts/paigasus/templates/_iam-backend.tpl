{{/* SPDX-License-Identifier: Apache-2.0 */}}

{{/*
The IAM backend's operator-supplied env (SMA-697): zones.iam.backend.bootstrapAdmins and
zones.iam.backend.extraEnv. Each helper in this file takes the ROOT context. Each value is read
with dig, so `helm upgrade --reuse-values` from a release made before the key existed reads an
empty list.
*/}}

{{/*
paigasus.iamReservedEnv: the env names that the IAM backend template sets itself, joined by
spaces. extraEnv must not repeat one of them. Two entries with one name in one container make
the result depend on the order, and server-side apply refuses a duplicate merge key. Keep this
list equal to the IAM env entries in backend-deployment.yaml.
*/}}
{{- define "paigasus.iamReservedEnv" -}}
IAM_HTTP_ADDR IAM_GRPC_ADDR IAM_MIGRATION__LOCK_WAIT_SECS IAM_DATABASE_URL IAM_AUTHN__ISSUERS IAM_API_KEYS__PEPPER IAM_AUTHN__EXTRA_CA_BUNDLE_PATH IAM_AUTHZ__BOOTSTRAP_ADMINS
{{- end -}}

{{/*
paigasus.iamBootstrapAdmins: the value of IAM_AUTHZ__BOOTSTRAP_ADMINS, or "" for an empty list.
The form is the figment inline form that IAM_AUTHN__ISSUERS uses. Each string is quoted with %q:
without quotes figment reads a subject of digits only as a number, and IAM does not boot.
The test bootstrap_admins_env_in_the_chart_form_parses in
rs/crates/services/paigasus-iam/src/config.rs parses this exact form. paigasus.validate has
already refused an entry that is not two strings.
*/}}
{{- define "paigasus.iamBootstrapAdmins" -}}
{{- $entries := list -}}
{{- range (dig "bootstrapAdmins" list .Values.zones.iam.backend) -}}
{{- $entries = append $entries (printf "{issuer=%q,subject=%q}" .issuer .subject) -}}
{{- end -}}
{{- if $entries -}}
{{- printf "[%s]" (join "," $entries) -}}
{{- end -}}
{{- end -}}

{{/*
paigasus.validateIamBackend: the refusals for the two values. paigasus.validate calls it.

bootstrapAdmins mirrors IamConfig::validate (config.rs, the bootstrap_admins loop): an https
issuer and a subject that is not empty. It adds one rule that IAM does not have: the issuer must
equal oidc.issuer. The chart configures exactly one issuer, and bootstrap_admin.rs matches the
(issuer, subject) pair as exact strings against the issuer of the validated token. An entry with
another issuer is therefore never granted, and IAM gives no signal. Both sides are trimmed, as
Issuer::parse trims.
*/}}
{{- define "paigasus.validateIamBackend" -}}
{{- $admins := dig "bootstrapAdmins" list .Values.zones.iam.backend -}}
{{- if not (kindIs "slice" $admins) -}}
{{- fail "zones.iam.backend.bootstrapAdmins must be a list of {issuer, subject} entries" -}}
{{- end -}}
{{- $oidcIssuer := .Values.oidc.issuer | toString | trim -}}
{{- range $i, $a := $admins -}}
{{- if not (kindIs "map" $a) -}}
{{- fail (printf "zones.iam.backend.bootstrapAdmins[%d] must be a map with the keys issuer and subject" $i) -}}
{{- end -}}
{{- if not $a.issuer -}}
{{- fail (printf "zones.iam.backend.bootstrapAdmins[%d].issuer is empty or missing" $i) -}}
{{- end -}}
{{- if not $a.subject -}}
{{- fail (printf "zones.iam.backend.bootstrapAdmins[%d].subject is empty or missing. IamConfig::validate refuses an empty subject, and IAM does not boot" $i) -}}
{{- end -}}
{{- if not (and (kindIs "string" $a.issuer) (kindIs "string" $a.subject)) -}}
{{- fail (printf "zones.iam.backend.bootstrapAdmins[%d]: issuer and subject must both be strings. Quote a subject of digits only in a values file, or use --set-string. YAML reads it as a number, and a large number loses digits" $i) -}}
{{- end -}}
{{- $issuer := trim $a.issuer -}}
{{- if not $issuer -}}
{{- fail (printf "zones.iam.backend.bootstrapAdmins[%d].issuer is empty or missing" $i) -}}
{{- end -}}
{{- if not (hasPrefix "https://" $issuer) -}}
{{- fail (printf "zones.iam.backend.bootstrapAdmins[%d].issuer is %q: it must be an https URL. IamConfig::validate refuses it, and IAM does not boot" $i $a.issuer) -}}
{{- end -}}
{{- if ne $issuer $oidcIssuer -}}
{{- fail (printf "zones.iam.backend.bootstrapAdmins[%d].issuer is %q: it must equal oidc.issuer (%q). IAM matches the issuer as an exact string, so this entry never gets the platform_admin grant" $i $a.issuer $oidcIssuer) -}}
{{- end -}}
{{- if not (trim $a.subject) -}}
{{- fail (printf "zones.iam.backend.bootstrapAdmins[%d].subject is empty or missing. IamConfig::validate refuses an empty subject, and IAM does not boot" $i) -}}
{{- end -}}
{{- end -}}
{{- $extra := dig "extraEnv" list .Values.zones.iam.backend -}}
{{- if not (kindIs "slice" $extra) -}}
{{- fail "zones.iam.backend.extraEnv must be a list of Kubernetes EnvVar entries" -}}
{{- end -}}
{{- $reserved := splitList " " (include "paigasus.iamReservedEnv" .) -}}
{{- $seen := dict -}}
{{- range $i, $e := $extra -}}
{{- if not (and (kindIs "map" $e) (kindIs "string" $e.name) $e.name) -}}
{{- fail (printf "zones.iam.backend.extraEnv[%d] must be a map with a non-empty string name" $i) -}}
{{- end -}}
{{- range $r := $reserved -}}
{{- if or (eq $e.name $r) (hasPrefix (printf "%s__" $r) $e.name) -}}
{{- fail (printf "zones.iam.backend.extraEnv[%d].name is %q: the chart sets %s itself. Use the chart value for it (see docs/ops/RUNBOOK-chart.md)" $i $e.name $r) -}}
{{- end -}}
{{- end -}}
{{- if hasKey $seen $e.name -}}
{{- fail (printf "zones.iam.backend.extraEnv[%d].name %q is already used by extraEnv[%d]" $i $e.name (index $seen $e.name)) -}}
{{- end -}}
{{- $_ := set $seen $e.name $i -}}
{{- end -}}
{{- end -}}
