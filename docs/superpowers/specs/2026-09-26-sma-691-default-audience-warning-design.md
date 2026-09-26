# SMA-691: a signal when the IAM audience equals the client id

- Linear: SMA-691 (milestone "IAM Gaps")
- Related: SMA-678 (`oidc.audience`), SMA-686 (IAM refuses a Keycloak ID or logout token)
- Status: design approved in chat on 2026-09-26. Revised after the adversarial challenge
  (§ 9). Waits for the written-spec review.

## 1. Problem

The chart gives IAM one accepted audience:
`oidc.audience | default oidc.clientId | toString`
(`charts/paigasus/templates/backend-deployment.yaml:114`, the `IAM_AUTHN__ISSUERS` entry).
An OIDC ID token has `aud` equal to the client id. So when the audience equals the client id,
an ID token passes IAM's audience check. This is the default.

SMA-686 closed the gap for Keycloak only. IAM refuses a token whose payload `typ` is `ID` or
`Logout`. Dex does not set `typ` (measured in SMA-686). Okta, Entra ID, Auth0, Zitadel and
authentik are not measured. SMA-686 records this as residual R1, and its code review
(finding 5) names the chart default as the root cause.

## 2. Intent and success criteria

The operator must know when the configuration lets an ID token pass IAM's audience check. The
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
  `helm upgrade`, and of any tool that runs a real Helm release (Flux's helm-controller stores
  NOTES in the release). A Deployment annotation reaches a user who renders with
  `helm template` and never sees NOTES, for example Argo CD. (Chosen in chat over "NOTES only"
  and over an opt-in `fail`.)
- **D3. The condition is "audience equals client id", not "audience is unset".** The signal also
  shows when an operator sets `oidc.audience` to the same string as `oidc.clientId`.
- **D4. An acknowledgement value turns the signal off, and it names the client id it
  acknowledges.** A Dex operator cannot make the two values differ, so a signal with no off
  switch shows on every install and upgrade. (The off switch was chosen in chat over "always
  shown".) The value is a string that must equal `oidc.clientId`, not a boolean. Reasons:
  - A change of `oidc.clientId` (for example, a move to a new IdP or a new client) makes the
    acknowledgement stale, and the signal shows again (challenge finding C8).
  - No Go-template truth trap exists: `--set-string …=false` and a bool `true` are both not
    equal to the client id.
- **D5. New helpers go in a new file**, `templates/_audience.tpl`. `_helpers.tpl` does not
  change, so its two whole-file negative-control copies under `ci/helm-render/fixtures/` do not
  change either (`charts/CLAUDE.md`).
- **D6. NOTES output is tested offline through a probe, and end to end in the kind job.**
  Helm executes `NOTES.txt` in `helm template` and `helm lint` (a template error in it fails
  every chart script), but no offline command prints its OUTPUT. Measured on 2026-09-26 with
  Helm v3.22.0: `helm install --dry-run=client` and `--dry-run` both fail with
  `Kubernetes cluster unreachable` when no cluster answers. So the NOTES body lives in a named
  template, `NOTES.txt` only includes it, and a test renders the named template through a probe
  ConfigMap in a copy of the chart (§ 5.2).
- **D7. The signal does not know the IdP.** It also shows for Keycloak, which SMA-686 already
  protects. So the text states only what the chart knows: the audience equals the client id. It
  does not claim that IAM accepts an ID token. A value such as `oidc.idpType` is out of scope.
- **D8. No Notion ADR.** The change adds a warning and one optional value. It changes no
  interface and no behavior. This decision row is the record, as SMA-686 D10 did.

## 4. Design

### 4.1 Values

New key in `charts/paigasus/values.yaml`, in the `oidc` block, after `audience`:

```yaml
  acknowledgeClientIdAudience: ""   # NOT required. Set it to the value of oidc.clientId to
                                    # remove the warning (NOTES and the paigasus.io/iam-audience-
                                    # warning annotation) that shows when the IAM audience equals
                                    # oidc.clientId. It does not change what IAM accepts. A
                                    # change of oidc.clientId shows the warning again.
                                    # See RUNBOOK-chart.md § 6.
```

The acknowledgement is valid only when `toString .Values.oidc.acknowledgeClientIdAudience`
equals `toString .Values.oidc.clientId`. A nil value (`helm upgrade --reuse-values` from a
release made before this key existed) gives `<nil>` from sprig's `toString`, which is not a
client id, so the signal shows.

The value is not required. So the seven chart scripts' value lists, `helm_render.py`
`STUB_VALUES` and `ci/kind/values/a.yaml` do not change (`charts/CLAUDE.md`, the rule for a
new REQUIRED value).

### 4.2 Helpers (`templates/_audience.tpl`, new)

The file opens with `{{/* SPDX-License-Identifier: Apache-2.0 */}}`, as `_helpers.tpl` does.
Each helper takes the ROOT context. Callers inside the `range` of `backend-deployment.yaml`
pass `$root`, never `.` (inside the range, `.` is the zone map).

- `paigasus.iamAudience` returns
  `.Values.oidc.audience | default .Values.oidc.clientId | toString`.
  Line 114 of `backend-deployment.yaml` becomes
  `… ($root.Values.oidc.issuer) (include "paigasus.iamAudience" $root) …` in place of the inline
  expression. The render stays byte-identical (D1): `include` returns the same string. The
  existing SMA-678 comment block stays next to the env entry.
- `paigasus.iamAudienceWarns` returns the string `"true"` when both of these are true, and an
  empty string otherwise:
  1. `include "paigasus.iamAudience" .` equals `toString .Values.oidc.clientId`.
  2. `toString .Values.oidc.acknowledgeClientIdAudience` does not equal
     `toString .Values.oidc.clientId`.

  There is no "the IAM backend is deployed" condition. `paigasus.validate` refuses every render
  in which the IAM backend is absent: `zones.iam.backend.deploy=false`, no enabled zone, and the
  gateway without IAM (`_helpers.tpl:82-90`). A condition that is never false in a successful
  render cannot be tested (challenge finding C11).
- `paigasus.iamAudienceNotes` returns the NOTES body (§ 4.3) when `paigasus.iamAudienceWarns`
  returns `"true"`, and an empty string otherwise.

Both signals call `paigasus.iamAudienceWarns`. No other file repeats the condition.

### 4.3 Signal 1: `templates/NOTES.txt` (new)

The first NOTES file in the chart. Its whole content is exactly:

```text
{{- /* SPDX-License-Identifier: Apache-2.0 */ -}}
{{- include "paigasus.validate" . -}}
{{- include "paigasus.iamAudienceNotes" . -}}
```

It calls `paigasus.validate` first, as every template does (`_helpers.tpl:50-54`). All tags
trim, so an install with no warning renders an EMPTY string, and Helm prints no `NOTES:`
block. The body that `paigasus.iamAudienceNotes` returns is:

```text
WARNING (SMA-691): the IAM audience equals oidc.clientId, so an ID token passes IAM's audience check.
IAM accepts the audience "<audience>". An OIDC ID token has the client id as its audience.
IAM refuses a Keycloak ID token by its typ claim (SMA-686). Dex does not set that claim.
Other IdPs are not measured.
Recommended: give the API its own audience and set oidc.audience to it.
Follow the order in docs/ops/RUNBOOK-chart.md section 6, or every session breaks.
If your IdP cannot do this (Dex), set oidc.acknowledgeClientIdAudience to the value of
oidc.clientId to remove this warning.
```

`<audience>` is the output of `paigasus.iamAudience`, in double quotes. The first line is the
stable marker that the kind job asserts.

### 4.4 Signal 2: the Deployment annotation

On the IAM backend Deployment only, in `metadata.annotations` — NOT in
`spec.template.metadata.annotations` — when
`and (eq $id "iam") (include "paigasus.iamAudienceWarns" $root)` is true:

```yaml
metadata:
  name: ...
  annotations:
    paigasus.io/iam-audience-warning: "the IAM audience equals oidc.clientId, so an ID token passes IAM's audience check. See docs/ops/RUNBOOK-chart.md section 6 (SMA-691)."
```

- The key uses `paigasus.io`, the domain of the wire contract
  (`contracts/proto/paigasus/common/v1/error.proto:16,50`).
- The value is ASCII only ("section 6", not "§ 6").
- A change of Deployment metadata does not change the pod template, so it does not restart a
  pod.
- The whole `annotations:` key is inside the `{{- if }}`. With no warning, the Deployment
  metadata renders as it does today.
- An explanation of the annotation goes ONLY in a Go template comment `{{- /* … */}}`, never in
  a YAML comment. The `if` is true by default, so a YAML comment renders into both golden files
  (`charts/CLAUDE.md`). A YAML comment that names the gateway also fails `repo:helm-render`
  check 2 (`ci/helm-render/helm_render.py:386-407`).

### 4.5 Documents that change

Each of these places must agree with the new recommendation:

- `charts/paigasus/values.yaml:78-90`: the `oidc.audience` comment says that the recommended
  setup is a dedicated API audience. It no longer says "set this value only when your IdP writes
  a different value". It points to the migration order in RUNBOOK § 6. The new key (§ 4.1).
- `docs/ops/RUNBOOK-chart.md`:
  - Values table rows for `oidc.clientId` and `oidc.audience` (lines 25-26): same change as
    `values.yaml`. A new row for `oidc.acknowledgeClientIdAudience`.
  - § 6 item 1 "Audience": the recommended setup, the migration order (§ 4.6), the two
    signals, their exact condition, the acknowledgement and what it does not do.
  - § 6 "complete example" (lines 139-158): mark it as the kind-job setup, which shows the
    warning. Add a second Keycloak example with a dedicated API audience.
  - § 6, per IdP (these lines state what each IdP offers; this issue does not measure them, and
    the runbook says so):
    - Keycloak: an "Audience" protocol mapper on the console client, or a client scope, with
      "Add to access token" on and "Add to ID token" off. The mapper ADDS a value to `aud`. It
      does not replace `aud`.
    - Okta: a custom authorization server whose audience is the API identifier.
    - Auth0: the console cannot send the `audience` parameter today (it sends only `scope`,
      `ts/packages/paigasus-auth/src/adapters/oidc.ts:221-228`; SMA-678 D8). The tenant
      "Default Audience" setting is a possible path, not measured.
    - Entra ID: an access token for an API application ID URI needs a scope of that API. The
      console's scopes are fixed (`ts/packages/paigasus-auth/src/config.ts:46`) and the chart has
      no value for them. So Entra ID cannot use a dedicated audience today. A follow-up issue
      tracks a configurable scope list (§ 8).
  - § 6, Dex: Dex gives the ID token and the access token the same `aud`, so no audience setting
    helps. The operator sets the acknowledgement to remove the warning. IAM still accepts a Dex
    ID token as a bearer token. SMA-686 residual R1 stays open for Dex.
  - § 6: "no warning" does not mean "safe" (R5).
- `charts/paigasus/README.md:115-135`: the `oidc.audience` section and the `env.sh` row list
  (add W1-W9 and N1-N5).
- `charts/paigasus/tests/env.sh:19-32`: the header row list.
- `ci/kind/README.md`: the new NOTES assertion.

### 4.6 The migration order (runbook § 6)

The warning tells every existing default install to set `oidc.audience`. Under the SMA-678
REPLACE contract, IAM then refuses every live access token whose `aud` holds only the client id.
IAM also restarts with a gap (one replica, `maxSurge: 0`). So § 6 gives this order:

1. In the IdP, add the API audience to the ACCESS token, next to the client id. Do not add it
   to the ID token.
2. Wait one access-token lifetime, so that every live access token has the new audience.
3. Set `oidc.audience` to the API audience and upgrade. IAM restarts and then accepts only the
   API audience.
4. Optional: remove the client id from the access token's `aud`.

The wrong order (step 3 before step 1, or an IdP change that replaces `aud`) makes IAM refuse
every console session's token.

## 5. Tests

### 5.1 The annotation (`charts/paigasus/tests/env.sh`, rows W1-W9)

A new row group with its own row counter, in the same style as A1-A6. Each row renders with
`helm template` and the existing `BASE` values (`oidc.clientId=paigasus-console`) INTO A FILE,
as `check_audience` does (`env.sh:110-116`). It does not use the pipe that `check` uses
(`env.sh:69`), because of the 512-byte host pipe (root `CLAUDE.md`). The check reads the IAM
backend Deployment's `metadata.annotations` by exact key and exact value.

| Row | Input | Annotation |
|---|---|---|
| W1 default | no extra value | present; the key occurs exactly once in the whole render |
| W2 reuse-values-no-key | `--set oidc.acknowledgeClientIdAudience=null` | present |
| W3 explicit-equal | `--set oidc.audience=paigasus-console` | present |
| W4 distinct | `--set oidc.audience=api://paigasus` | absent |
| W5 acknowledged | `--set oidc.acknowledgeClientIdAudience=paigasus-console` | absent |
| W6 bool-true | `--set oidc.acknowledgeClientIdAudience=true` | present |
| W7 string-false | `--set-string oidc.acknowledgeClientIdAudience=false` | present |
| W8 stale-ack | `--set oidc.acknowledgeClientIdAudience=old-client` | present |
| W9 pod-template | no extra value | absent from `spec.template.metadata.annotations` |

Each "absent" row also checks that the IAM backend Deployment exists in that render. Otherwise a
render that lost the Deployment passes an "absent" row.

The rows go into `env.sh`, not a new script. So the chart-script floor (`CHART_SCRIPT_FLOOR=7`
and `HELM_RENDER_SH_CALL_SITES`) does not change.

### 5.2 The NOTES text (`env.sh`, rows N1-N5)

- **N0 pin**: the bytes of `templates/NOTES.txt` equal the three lines in § 4.3. So the probe
  below tests what `NOTES.txt` prints.
- The script copies the chart into `$TMP` (`cp -R`, as the helm-render negative control does at
  `ci/helm-render/run.sh:215`). It adds `templates/zz-notes-probe.yaml`: a ConfigMap whose
  `data.notes` is `{{ include "paigasus.iamAudienceNotes" . | quote }}`. It renders with
  `helm template --show-only templates/zz-notes-probe.yaml` into a file and reads `data.notes`.
- Rows: N1 default → the exact body of § 4.3 with `"paigasus-console"`; N2 explicit-equal
  (`oidc.audience=paigasus-console`) → the same body; N3 distinct → empty string;
  N4 acknowledged → empty string; N5 stale-ack → the body.
- A row counter, as for W.

### 5.3 Byte-identity of `IAM_AUTHN__ISSUERS`

Rows A1-A6 already compare its exact value. They must pass unchanged after the helper refactor.

### 5.4 Golden files

`tests/golden/iam-and-gateway.yaml` and `tests/golden/iam-only.yaml` change. Both render with the
default audience, so both gain the annotation. Regenerate them with `render.sh --update` and
review the diff: the ONLY change is two lines per file (`annotations:` and the annotation).

### 5.5 Kind job (`ci/kind/run.sh`)

- At the end of `install_a`, and again at the end of `upgrade_b`: capture
  `helm get notes "$RELEASE" --namespace "$NS"` into a variable, then match the marker line
  `WARNING (SMA-691): the IAM audience equals oidc.clientId` with a `case` pattern. No pipe into
  an early-exit reader (`run.sh:27-30`, actionlint check 13). A `helm get notes` failure is
  `die_infra`. A missing marker is `die_assert`.
- `ci/kind/values/a.yaml` and `b.yaml` do not change. Both stay on the default audience, so the
  kind job is the end-to-end positive control of NOTES on install and on upgrade.
- AC 2 proof: the PR links a green `chart.yml` run. That job is not a required check. It runs on
  this PR because `charts/**` is in its `pull_request` path filter (`.github/workflows/chart.yml:30-31`).

### 5.6 Mutation table (each mutation must red the named row)

Revert each mutation with Edit, never with `git checkout --` (it also reverts the fix under
test).

| Mutation | Must fail |
|---|---|
| Drop condition 1 of `iamAudienceWarns` (audience equals client id) | W4, N3 |
| Drop condition 2 (the acknowledgement) | W5, N4 |
| Compare the acknowledgement with `"true"` in place of the client id | W5, W6 |
| Move the annotation into `spec.template.metadata.annotations` | W1, W9 |
| Put the annotation on every backend Deployment (drop `eq $id "iam"`) | W1 (count) |
| `NOTES.txt` without the include | N0 |
| Change the marker line text | N1 |
| Delete one W or N call line | the row counter |
| Inline expression on line 114 changed (for example drop `toString`) | A4, A5 |
| Delete the kind marker check | nothing; see R2 |

## 6. Rollout

- The upgrade adds one annotation to the IAM backend Deployment's metadata and a NOTES text. No
  pod restarts. `IAM_AUTHN__ISSUERS` does not change.
- `helm rollback` removes the annotation.

## 7. Out of scope

- Any change to what IAM accepts, in the chart or in Rust. RFC 9068 `typ: at+jwt` enforcement
  is a separate matter (SMA-678 R3, SMA-686).
- A `fail` on the default configuration.
- A configurable scope list or an `audience` parameter in the console (§ 8).
- Measurements of Okta, Entra ID, Auth0, Zitadel and authentik tokens.
- A `values.schema.json`. The chart has none today.
- A label for a policy engine, or a separate resource for GitOps tools.

## 8. Follow-up

- A new Linear issue: the console cannot request an API audience from Auth0 (no `audience`
  parameter) or from Entra ID (no configurable scope). Without it, those IdPs cannot use the
  recommended setup.

## 9. Residuals

- **R1.** The signal does not protect. An operator who ignores it, or who sets the
  acknowledgement, still has an IAM that accepts an ID token as a bearer token, except for
  Keycloak (SMA-686).
- **R2.** The kind check of the NOTES marker is not a required check, and deleting it reds
  nothing. The offline N rows cover the text. The kind check sits inside `install_a`, so a NOTES
  regression there fails the install step and `chart.yml` skips the later e2e steps of that run.
  The offline N rows catch a text regression first, so this coupling is accepted.
- **R3.** A user of `helm template` sees the signal only if they read the Deployment's
  annotations. Nothing reds a GitOps sync.
- **R4.** The IdP lines in the runbook are not measured (§ 4.5).
- **R5.** No signal does not prove a safe setup. Any `oidc.audience` that differs from the
  client id turns the signal off. If the IdP also puts that audience into the ID token, the ID
  token still passes and nothing warns.

## 10. Challenge log (2026-09-26)

Folded in: C1 the headline was false for Keycloak (D7, § 4.3); C2 the Auth0 and Entra recipes
could not work with this console (§ 4.5, § 8); C3 no migration order (§ 4.6); C4 NOTES had no
offline test (D6, § 5.2); C5 other documents contradicted the recommendation (§ 4.5); C6 no
mutation table (§ 5.6); C7 a YAML comment would change the goldens (§ 4.4); C8 a stale
acknowledgement (D4); C9 the SPDX header made NOTES non-empty (§ 4.3); C10 NOTES did not call
`paigasus.validate` (§ 4.3); C11 condition 1 could not be false (§ 4.2); C12 the call context
was ambiguous (§ 4.2); C13 W-row gaps (§ 5.1); C14 the kind check pipe rule and the upgrade path
(§ 5.5); C15 Flux runs real releases (D2); C16 the annotation domain (§ 4.4); C17 no rollout
note and no AC 2 proof (§ 5.5, § 6); C18 no-signal-is-not-safe residual (R5).

Rejected: a separate `run.sh notes` mode and workflow step for the kind check. It changes
`.github/workflows/chart.yml` for a check that the offline N rows already cover (R2). A
policy-engine label or separate GitOps resource: out of scope (§ 7).
