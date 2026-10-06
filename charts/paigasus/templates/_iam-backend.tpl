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
IAM_HTTP_ADDR IAM_GRPC_ADDR IAM_MIGRATION__LOCK_WAIT_SECS IAM_DATABASE_URL IAM_AUTHN__ISSUERS IAM_API_KEYS__PEPPER IAM_AUTHN__EXTRA_CA_BUNDLE_PATH IAM_AUTHZ__BOOTSTRAP_ADMINS IAM_AUTHN__DPOP__ENABLED IAM_AUTHN__DPOP__FORWARDED_BASE_URLS
{{- end -}}

{{/*
paigasus.iamBootstrapAdmins: the value of IAM_AUTHZ__BOOTSTRAP_ADMINS, or "" for an empty list.
The form is the figment inline form that IAM_AUTHN__ISSUERS uses. Each string is quoted with %q:
without quotes figment reads a subject of digits only as a number, and IAM does not boot.
The test bootstrap_admins_env_in_the_chart_form_parses in
rs/crates/services/paigasus-iam/src/config.rs parses this exact form. paigasus.validate has
already refused an entry that is not two strings. The issuer is rendered trimmed, the same value
that the validation compares with oidc.issuer. The subject is rendered as written: IAM does not
trim a subject, and the match is exact.
*/}}
{{- define "paigasus.iamBootstrapAdmins" -}}
{{- $entries := list -}}
{{- range (dig "bootstrapAdmins" list .Values.zones.iam.backend) -}}
{{- $entries = append $entries (printf "{issuer=%q,subject=%q}" (trim .issuer) .subject) -}}
{{- end -}}
{{- if $entries -}}
{{- printf "[%s]" (join "," $entries) -}}
{{- end -}}
{{- end -}}

{{/*
paigasus.validateIamBackend: the refusals for the two values, and for oidc.idTokenMarkerClaims
(SMA-703, paigasus.validateIdTokenMarkerClaims below) and zones.iam.backend.dpop (SMA-700,
paigasus.validateIamDpop below). paigasus.validate calls it.

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
{{- include "paigasus.validateIdTokenMarkerClaims" . -}}
{{- include "paigasus.validateIamDpop" . -}}
{{- end -}}

{{/*
paigasus.iamIdTokenMarkerClaims: the suffix ,id_token_marker_claims=[...] for the one issuer entry
of IAM_AUTHN__ISSUERS (SMA-703), or "" when oidc.idTokenMarkerClaims is empty, absent or nil. An
absent key comes from `helm upgrade --reuse-values` on a release made before the key; dig then
gives the default. Each name is quoted with %q, like the audience. figment reads the inline form
(the test issuers_env_in_the_chart_form_parses_id_token_marker_claims in
rs/crates/services/paigasus-iam/src/config.rs). paigasus.validateIdTokenMarkerClaims has already
refused every name that %q would escape.
*/}}
{{- define "paigasus.iamIdTokenMarkerClaims" -}}
{{- $names := dig "idTokenMarkerClaims" list .Values.oidc -}}
{{- if and (kindIs "slice" $names) $names -}}
{{- $quoted := list -}}
{{- range $names -}}
{{- $quoted = append $quoted (printf "%q" .) -}}
{{- end -}}
{{- printf ",id_token_marker_claims=[%s]" (join "," $quoted) -}}
{{- end -}}
{{- end -}}

{{/*
paigasus.validateIdTokenMarkerClaims: the refusals for oidc.idTokenMarkerClaims (SMA-703 D5). A
bad name stops IAM at boot (IamConfig::validate), and the IAM Deployment has one replica with
maxSurge 0, so the old pod stops before the new pod fails. So the chart copies the boot rules and
fails the render instead. The character rule is stricter than IAM: printable ASCII only, with no
space, no " and no \. Go's %q writes other characters as escapes (\t, \x01) that figment does not
read. The rule also refuses leading and trailing whitespace. A nil value counts as an empty list.
The nil case happens when a release from before SMA-703 has no key and the user passes
--set oidc.idTokenMarkerClaims=null. Then Helm keeps a nil value in the user values.
*/}}
{{- define "paigasus.validateIdTokenMarkerClaims" -}}
{{- $names := dig "idTokenMarkerClaims" list .Values.oidc -}}
{{- if kindIs "invalid" $names -}}
{{- $names = list -}}
{{- end -}}
{{- if not (kindIs "slice" $names) -}}
{{- fail "oidc.idTokenMarkerClaims must be a list of claim names, for example [\"at_hash\", \"azp\"]" -}}
{{- end -}}
{{- $seen := dict -}}
{{- range $i, $n := $names -}}
{{- if not (kindIs "string" $n) -}}
{{- fail (printf "oidc.idTokenMarkerClaims[%d] must be a string. Quote the name in a values file, or use --set-string" $i) -}}
{{- end -}}
{{- if not $n -}}
{{- fail (printf "oidc.idTokenMarkerClaims[%d] is empty. IamConfig::validate refuses an empty name, and IAM does not boot. To remove the value, use [] in a values file or --set-json 'oidc.idTokenMarkerClaims=[]', not --set oidc.idTokenMarkerClaims={}" $i) -}}
{{- end -}}
{{- if not (regexMatch `^[!#-\[\]-~]+$` $n) -}}
{{- fail (printf "oidc.idTokenMarkerClaims[%d] is %q: use printable ASCII only, with no space, no \" and no \\. IAM cannot read another character from IAM_AUTHN__ISSUERS" $i $n) -}}
{{- end -}}
{{- /* Keep this list equal to RESERVED_MARKER_CLAIMS in rs/crates/services/paigasus-iam/src/config.rs. */ -}}
{{- if has $n (list "iss" "sub" "aud" "exp") -}}
{{- fail (printf "oidc.idTokenMarkerClaims[%d] is %q: every access token carries this claim, so IAM would refuse every token. IamConfig::validate refuses it, and IAM does not boot" $i $n) -}}
{{- end -}}
{{- if hasKey $seen $n -}}
{{- fail (printf "oidc.idTokenMarkerClaims[%d] %q is already in oidc.idTokenMarkerClaims[%d]. IamConfig::validate refuses a duplicate, and IAM does not boot" $i $n (index $seen $n)) -}}
{{- end -}}
{{- $_ := set $seen $n $i -}}
{{- end -}}
{{- end -}}

{{/*
paigasus.iamDpopEnabled: "true" when zones.iam.backend.dpop.enabled is true, else "" (SMA-700).
The key can be absent or nil under `helm upgrade --reuse-values` from a release made before it;
then DPoP is off. paigasus.validateIamDpop has already refused a value that is not a map or a bool.
*/}}
{{- define "paigasus.iamDpopEnabled" -}}
{{- $dpop := dig "dpop" dict .Values.zones.iam.backend -}}
{{- if and (kindIs "map" $dpop) (eq (toString (dig "enabled" false $dpop)) "true") -}}
true
{{- end -}}
{{- end -}}

{{/*
paigasus.iamDpopForwardedBaseUrls: the value of IAM_AUTHN__DPOP__FORWARDED_BASE_URLS (SMA-700), in
the figment inline form that IAM_AUTHN__ISSUERS uses: each URL quoted with %q, joined by a comma,
in brackets. The test dpop_env_in_the_chart_form_parses in rs/crates/services/paigasus-iam/src/config.rs
parses this exact form. Called only when paigasus.iamDpopEnabled is "true", so the map exists.
*/}}
{{- define "paigasus.iamDpopForwardedBaseUrls" -}}
{{- $dpop := dig "dpop" dict .Values.zones.iam.backend -}}
{{- $quoted := list -}}
{{- range (dig "forwardedBaseUrls" list $dpop) -}}
{{- $quoted = append $quoted (printf "%q" .) -}}
{{- end -}}
{{- printf "[%s]" (join "," $quoted) -}}
{{- end -}}

{{/*
paigasus.validateIamDpop: the refusals for zones.iam.backend.dpop (SMA-700 § 4.11). A refused boot
stops the one IAM replica (maxSurge 0), so the chart copies IamConfig::validate's rules for the
list: enabled with an empty list; an entry that is not https or loopback http; an entry with a
query, a fragment or user info. Entries are checked also when enabled is false, as IAM checks them.
The character rule is stricter than IAM, as for idTokenMarkerClaims: printable ASCII only, with no
space, no " and no \, because %q writes other characters as escapes that figment does not read.
Loopback http is localhost, a dotted 127.a.b.c, or ::1; IAM also accepts other 127/8 spellings,
so the chart is stricter, never looser. A nil value counts as absent.
*/}}
{{- define "paigasus.validateIamDpop" -}}
{{- $dpop := dig "dpop" dict .Values.zones.iam.backend -}}
{{- if kindIs "invalid" $dpop -}}
{{- $dpop = dict -}}
{{- end -}}
{{- if not (kindIs "map" $dpop) -}}
{{- fail "zones.iam.backend.dpop must be a map with the keys enabled and forwardedBaseUrls" -}}
{{- end -}}
{{- $enabled := dig "enabled" false $dpop -}}
{{- if kindIs "invalid" $enabled -}}
{{- $enabled = false -}}
{{- end -}}
{{- if not (kindIs "bool" $enabled) -}}
{{- fail "zones.iam.backend.dpop.enabled must be true or false" -}}
{{- end -}}
{{- $urls := dig "forwardedBaseUrls" list $dpop -}}
{{- if kindIs "invalid" $urls -}}
{{- $urls = list -}}
{{- end -}}
{{- if not (kindIs "slice" $urls) -}}
{{- fail "zones.iam.backend.dpop.forwardedBaseUrls must be a list of URLs" -}}
{{- end -}}
{{- if and $enabled (not $urls) -}}
{{- fail "zones.iam.backend.dpop.enabled is true and zones.iam.backend.dpop.forwardedBaseUrls is empty. IamConfig::validate refuses it, and IAM does not boot. List the public URLs at which clients reach the gateway" -}}
{{- end -}}
{{- range $i, $u := $urls -}}
{{- if not (kindIs "string" $u) -}}
{{- fail (printf "zones.iam.backend.dpop.forwardedBaseUrls[%d] must be a string" $i) -}}
{{- end -}}
{{- if not (regexMatch `^[!#%-\[\]-~]+$` $u) -}}
{{- fail (printf "zones.iam.backend.dpop.forwardedBaseUrls[%d] is %q: use printable ASCII only, with no space, no \", no $ and no \\. IAM cannot read another character from IAM_AUTHN__DPOP__FORWARDED_BASE_URLS" $i $u) -}}
{{- end -}}
{{- $p := urlParse $u -}}
{{- if or $p.query $p.fragment $p.userinfo (contains "?" $u) (contains "#" $u) -}}
{{- fail (printf "zones.iam.backend.dpop.forwardedBaseUrls[%d] is %q: it must have no query, fragment or user info. IamConfig::validate refuses it, and IAM does not boot" $i $u) -}}
{{- end -}}
{{- $host := lower $p.hostname -}}
{{- if not (and $host (or (eq $p.scheme "https") (eq $p.scheme "http"))) -}}
{{- fail (printf "zones.iam.backend.dpop.forwardedBaseUrls[%d] is %q: use https, or http on localhost, 127.x.x.x or [::1]. IamConfig::validate refuses it, and IAM does not boot" $i $u) -}}
{{- end -}}
{{- /* Go's url.Parse is lenient where IAM's url::Url::parse is strict. The rules below are stricter
       than IAM: a plain host name, a dotted IPv4 or exactly [::1]; a port from 1 to 65535. */ -}}
{{- if not (regexMatch `^([A-Za-z0-9_-]+(\.[A-Za-z0-9_-]+)*|\[::1\])(:[0-9]{1,5})?$` $p.host) -}}
{{- fail (printf "zones.iam.backend.dpop.forwardedBaseUrls[%d] is %q: its host or port is not valid. Use a host name of letters, digits, hyphen, underscore and dots, a dotted IPv4 address, or [::1], with an optional decimal port. IamConfig::validate refuses a URL that does not parse, and IAM does not boot" $i $u) -}}
{{- end -}}
{{- $port := trimPrefix ":" (regexFind `:[0-9]+$` $p.host) -}}
{{- if and $port (or (lt (atoi $port) 1) (gt (atoi $port) 65535)) -}}
{{- fail (printf "zones.iam.backend.dpop.forwardedBaseUrls[%d] is %q: its port must be from 1 to 65535. IamConfig::validate refuses it, and IAM does not boot" $i $u) -}}
{{- end -}}
{{- $v4 := false -}}
{{- if ne $host "::1" -}}
{{- if contains "xn--" $host -}}
{{- fail (printf "zones.iam.backend.dpop.forwardedBaseUrls[%d] is %q: its host has an IDNA label (xn--). Use a host that IAM can parse for certain" $i $u) -}}
{{- end -}}
{{- if regexMatch `^([0-9]+|0[xX][0-9A-Fa-f]*)$` (last (splitList "." $host)) -}}
{{- if not (regexMatch `^(0|[1-9][0-9]{0,2})(\.(0|[1-9][0-9]{0,2})){3}$` $host) -}}
{{- fail (printf "zones.iam.backend.dpop.forwardedBaseUrls[%d] is %q: its host ends in a number but is not a dotted IPv4 address, and IAM reads such a host as an IPv4 address and refuses it. IAM does not boot" $i $u) -}}
{{- end -}}
{{- range (splitList "." $host) -}}
{{- if gt (atoi .) 255 -}}
{{- fail (printf "zones.iam.backend.dpop.forwardedBaseUrls[%d] is %q: its IPv4 host has an octet above 255. IamConfig::validate refuses it, and IAM does not boot" $i $u) -}}
{{- end -}}
{{- end -}}
{{- $v4 = true -}}
{{- end -}}
{{- end -}}
{{- $loopback := or (eq $host "localhost") (eq $host "::1") (and $v4 (hasPrefix "127." $host)) -}}
{{- if not (or (eq $p.scheme "https") (and (eq $p.scheme "http") $loopback)) -}}
{{- fail (printf "zones.iam.backend.dpop.forwardedBaseUrls[%d] is %q: use https, or http on localhost, 127.x.x.x or [::1]. IamConfig::validate refuses it, and IAM does not boot" $i $u) -}}
{{- end -}}
{{- end -}}
{{- end -}}
