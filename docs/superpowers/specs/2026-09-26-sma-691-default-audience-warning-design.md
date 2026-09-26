# SMA-691: a signal when the IAM audience equals the client id

- Linear: SMA-691 (milestone "IAM Gaps")
- Related: SMA-678 (`oidc.audience`), SMA-686 (IAM refuses a Keycloak ID or logout token)
- Status: design, approved in chat on 2026-09-26. Waits for the written-spec review.

## 1. Problem

The chart gives IAM one accepted audience:
`oidc.audience | default oidc.clientId | toString`
(`charts/paigasus/templates/backend-deployment.yaml`, the `IAM_AUTHN__ISSUERS` entry).
An OIDC ID token has `aud` equal to the client id. So when the audience equals the client id,
an ID token passes IAM's audience check. This is the default.

SMA-686 closed the gap for Keycloak only. IAM refuses a token whose payload `typ` is `ID` or
`Logout`. Dex does not set `typ` (measured in SMA-686). Okta, Entra ID, Auth0, Zitadel and
authentik are not measured. SMA-686 records this as residual R1, and its code review
(finding 5) names the chart default as the root cause.

## 2. Intent and success criteria

The operator must know when the configuration lets an ID token pass the audience check. The
chart does not change what IAM accepts.

Acceptance criteria (from the issue):

1. An operator who installs with the default audience gets a clear signal that an ID token can
   pass the audience check.
2. The kind job still installs and passes.
3. `docs/ops/RUNBOOK-chart.md` § 6 states the recommended audience setup.

## 3. Decisions

- **D1. No behavior change.** The render of `IAM_AUTHN__ISSUERS` stays byte-identical for every
  input. The SMA-678 contract stays: a set `oidc.audience` REPLACES the client id. There is no
  `fail`: a `fail` breaks every default install and `ci/kind/values/a.yaml`.
- **D2. Two signals.** A `NOTES.txt` warning reaches a user of `helm install` and
  `helm upgrade`. A Deployment annotation reaches a user of `helm template`, for example
  Argo CD or Flux, who never sees NOTES. (Chosen in chat over "NOTES only" and over an opt-in
  `fail`.)
- **D3. The condition is "audience equals client id", not "audience is unset".** The signal also
  shows when an operator sets `oidc.audience` to the same string as `oidc.clientId`.
- **D4. An acknowledgement value turns the signal off.** A Dex operator cannot make the two
  values differ. Without a way to turn it off, the warning shows on every install and upgrade
  for ever. (Chosen in chat over "always shown".)
- **D5. New helpers go in a new file**, `templates/_audience.tpl`. `_helpers.tpl` does not
  change, so its two whole-file negative-control copies under `ci/helm-render/fixtures/` do not
  change either (`charts/CLAUDE.md`).
- **D6. The kind job tests NOTES.** No offline test can render NOTES. Measured on 2026-09-26
  with Helm v3.22.0: `helm install --dry-run=client` and `--dry-run` both fail with
  `Kubernetes cluster unreachable` when no cluster answers, and `helm template` never renders
  `NOTES.txt`. The kind job has a real cluster and installs with the default audience.

## 4. Design

### 4.1 Values

New key in `charts/paigasus/values.yaml`, in the `oidc` block, after `audience`:

```yaml
  acknowledgeClientIdAudience: false   # NOT required. true removes the warning (NOTES and the
                                       # paigasus.dev/iam-audience-warning annotation) that shows
                                       # when the IAM audience equals oidc.clientId. It does not
                                       # change what IAM accepts. See RUNBOOK-chart.md § 6.
```

Only the string form of the value counts: the signal is off when
`toString .Values.oidc.acknowledgeClientIdAudience` is exactly `"true"`. Reason: with
`--set-string oidc.acknowledgeClientIdAudience=false` the value is the string `"false"`, and a
non-empty string is true in a Go template. A plain truth test would then turn the signal off.
A nil value (`helm upgrade --reuse-values` from a release made before this key existed) is not
`"true"`, so the signal shows.

The value is not required. So the seven chart scripts' value lists, `helm_render.py`
`STUB_VALUES` and `ci/kind/values/a.yaml` do not change (`charts/CLAUDE.md`, the rule for a
new REQUIRED value).

### 4.2 Helpers (`templates/_audience.tpl`, new)

- `paigasus.iamAudience` returns
  `.Values.oidc.audience | default .Values.oidc.clientId | toString`.
  The `IAM_AUTHN__ISSUERS` line uses this helper in place of the inline expression. The render
  stays byte-identical (D1). The existing SMA-678 comment block stays next to the env entry.
- `paigasus.iamAudienceWarns` returns the string `"true"` when ALL of these are true, and an
  empty string otherwise:
  1. the IAM zone is enabled and its backend is deployed. These are the same conditions under
     which `backend-deployment.yaml` renders the IAM backend. Read them with `dig`
     (`charts/CLAUDE.md`).
  2. `include "paigasus.iamAudience" .` equals `toString .Values.oidc.clientId`.
  3. `toString .Values.oidc.acknowledgeClientIdAudience` is not `"true"`.

Both signals call `paigasus.iamAudienceWarns`. No other file repeats the condition.

### 4.3 Signal 1: `templates/NOTES.txt` (new)

The first NOTES file in the chart. It opens with the SPDX header as a template comment
(`{{/* SPDX-License-Identifier: Apache-2.0 */}}`). When the helper returns `"true"`, it prints:

```text
WARNING (SMA-691): IAM accepts an ID token as a bearer token.
The audience that IAM accepts (<audience>) equals oidc.clientId. An OIDC ID token has this
audience too, so it passes IAM's audience check. IAM refuses a Keycloak ID token by its typ
claim (SMA-686). Other IdPs, for example Dex, do not set that claim.
Fix: give the API its own audience and set oidc.audience to it.
If your IdP cannot do this (Dex), set oidc.acknowledgeClientIdAudience=true to remove this
warning. See docs/ops/RUNBOOK-chart.md § 6.
```

`<audience>` is the output of `paigasus.iamAudience`. When the helper returns an empty string,
NOTES prints nothing. The first line is the stable marker that the kind job asserts.

### 4.4 Signal 2: the Deployment annotation

On the IAM backend Deployment only (`eq $id "iam"`), in `metadata.annotations` — NOT in
`spec.template.metadata.annotations` — when the helper returns `"true"`:

```yaml
metadata:
  name: ...
  annotations:
    paigasus.dev/iam-audience-warning: "IAM accepts an ID token as a bearer token: the audience equals oidc.clientId. See RUNBOOK-chart.md § 6 (SMA-691)."
```

The key uses the `paigasus.dev` domain, which `rs/crates/services/paigasus-iam/src/config.rs`
already uses. A change of Deployment metadata does not change the pod template, so it does not
restart a pod. The whole `annotations:` key is inside the `{{- if }}`: with no warning, the
Deployment metadata renders as it does today. Any YAML comment that explains the annotation
goes inside the same `{{- if }}` (`charts/CLAUDE.md`: a template comment renders into the
manifest).

### 4.5 Tests

**`charts/paigasus/tests/env.sh`**: a new row group W1-W7 with its own row counter, in the same
style as the A1-A6 audience rows. Each row renders with `helm template` and the existing `BASE`
values (`oidc.clientId=paigasus-console`) and checks for the annotation on the IAM backend
Deployment's `metadata.annotations`, by exact key and exact value:

| Row | Input | Annotation |
|---|---|---|
| W1 default | no extra value | present |
| W2 reuse-values-no-key | `--set oidc.acknowledgeClientIdAudience=null` | present |
| W3 explicit-equal | `--set oidc.audience=paigasus-console` | present |
| W4 distinct | `--set oidc.audience=api://paigasus` | absent |
| W5 acknowledged | `--set oidc.acknowledgeClientIdAudience=true` | absent |
| W6 string-false | `--set-string oidc.acknowledgeClientIdAudience=false` | present |
| W7 pod-template | no extra value | absent from `spec.template.metadata.annotations` |

Each "absent" row also needs the positive control of its own rendering: the IAM backend
Deployment must exist in that render. Otherwise a render that lost the Deployment passes an
"absent" row.

The rows go into `env.sh`, not a new script. So the chart-script floor (`CHART_SCRIPT_FLOOR=7`
and `HELM_RENDER_SH_CALL_SITES`) does not change.

**Byte-identity of `IAM_AUTHN__ISSUERS`**: rows A1-A6 already compare its exact value. They
must pass unchanged after the helper refactor.

**Golden files**: `tests/golden/iam-and-gateway.yaml` and `tests/golden/iam-only.yaml` change.
Both render with the default audience, so both gain the annotation. Regenerate them with
`render.sh --update` and review the diff: the ONLY change must be the new annotation lines.

**Kind job** (`ci/kind/run.sh`, `install_a`): after the install, run
`helm get notes "$RELEASE" --namespace "$NS"` and assert that the output contains the marker line
`WARNING (SMA-691): IAM accepts an ID token as a bearer token.` A `helm get notes` failure is
`die_infra`. A missing marker is `die_assert`. `ci/kind/values/a.yaml` does not change: it stays
on the default audience, so this is the positive control of the NOTES path. The kind README
lists the new assertion.

### 4.6 Runbook (`docs/ops/RUNBOOK-chart.md`)

- The values table gets a row for `oidc.acknowledgeClientIdAudience`.
- § 6 item 1 "Audience" states the recommended setup: a dedicated API audience, distinct from the
  client id, and `oidc.audience` set to it. One line per IdP on how to get it:
  - Keycloak: an "Audience" protocol mapper on the console client (or a client scope) that adds
    the API audience to the access token only.
  - Okta: a custom authorization server whose audience is the API identifier.
  - Auth0: an API whose identifier is the audience. The console requests it with the
    `audience` parameter.
  - Entra ID: the API app registration's application ID URI.
  These lines state what each IdP offers. This issue does not measure them. The runbook says so.
- § 6 states the two signals (NOTES and the annotation), the exact condition that shows them,
  and the acknowledgement value.
- § 6 states what a Dex operator gets: Dex gives the ID token and the access token the same
  `aud`, so no audience setting helps. The warning shows until the operator sets
  `oidc.acknowledgeClientIdAudience=true`. IAM still accepts a Dex ID token as a bearer token.
  SMA-686 residual R1 stays open for Dex.

## 5. Out of scope

- Any change to what IAM accepts, in the chart or in Rust. RFC 9068 `typ: at+jwt` enforcement
  is a separate matter (SMA-678 R3, SMA-686).
- A `fail` on the default configuration.
- Measurements of Okta, Entra ID, Auth0, Zitadel and authentik tokens.
- A `values.schema.json`. The chart has none today.

## 6. Residuals

- **R1.** The signal does not protect. An operator who ignores it, or who sets the
  acknowledgement, still has an IAM that accepts an ID token as a bearer token, except for
  Keycloak (SMA-686).
- **R2.** No offline test renders NOTES (D6). The NOTES text is tested only in the kind job,
  which runs the default path. The acknowledged and distinct-audience paths of NOTES are not
  tested; they share the helper with the annotation, which W1-W7 test.
- **R3.** A user of `helm template` sees the signal only if they read the Deployment's
  annotations. Nothing reds a GitOps sync.
- **R4.** The IdP lines in the runbook are not measured (§ 4.6).
