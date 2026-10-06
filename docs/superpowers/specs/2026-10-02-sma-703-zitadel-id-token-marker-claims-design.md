# SMA-703: IAM refuses a Zitadel ID token by a configured claim name

- Linear: SMA-703 (follows SMA-686 and SMA-691)
- Status: approved by Sven (2026-10-02), after the spec challenge in § 11.
- Path: architectural (a change to what IAM accepts from the IdP, and a new chart value).
- Measurement: `2026-10-02-sma-703-zitadel-measurements.md` (Zitadel v4.15.3, real tokens).

## 1. Problem

IAM must refuse an ID token that a client presents as a bearer token. SMA-686 added a token-type
check. It matches a Keycloak payload `typ` (`ID`, `Logout`) and the standard back-channel logout
markers. SMA-691 added a chart warning when the IAM audience equals `oidc.clientId`. Its remedy is
an API audience that only the access token carries.

Zitadel defeats both defences. A live install (v4.15.3, 2026-09-26) showed the same `aud` on both
tokens. The measurement for this spec (§ 3) shows more: no Zitadel scope puts an audience on the
access token only, and no Zitadel token carries a `typ` that the SMA-686 check matches. So today
IAM accepts a Zitadel ID token as a bearer token.

## 2. Goals and non-goals

Goals:

- G1. An operator can make IAM refuse a Zitadel ID token, with a setting for each issuer.
- G2. The default behaviour does not change for any IdP. Keycloak, Dex, Okta, Auth0 and Entra ID
  installs need no change.
- G3. `docs/ops/RUNBOOK-chart.md` § 6 states Zitadel's measured status next to Keycloak and Dex.
- G4. A test uses the measured Zitadel token shapes.

Non-goals:

- N1. A full fix for Dex. A Dex access token carries `at_hash` and `nonce` (SMA-686 § 2). `c_hash`
  marks only a Dex ID token from the code flow. A refreshed Dex ID token has no separating claim.
  So SMA-686 residual R1 stays open for Dex (D6).
- N2. A Docker Zitadel end-to-end test in CI. Follow-up issue (§ 10).
- N3. A change to the SMA-691 warning logic, or to the NOTES text. The NOTES line "Other IdPs are
  not measured" (`_audience.tpl:48`) becomes partly stale. This is accepted: the NOTES stay short,
  and the runbook carries the per-IdP detail.
- N4. More than one issuer in the chart. The chart keeps exactly one issuer.
- N5. Putting `email` into a Zitadel access token. The runbook names the need (F12), but this issue
  does not measure a Zitadel Action.

## 3. Measured facts (Zitadel v4.15.3, 2026-10-02)

Source: the measurement file. Every fact is MEASURED on real tokens, unless it is marked INFERRED.
The setup: one project P, one web app A (public client with PKCE, access token type JWT), one
human user, the built-in login v1, and one machine user with client credentials and access token
type JWT. Not measured: Login v2, a confidential app, the JWT-profile grant, token exchange, the
device flow, other Zitadel versions.

- F1. The header of every token is `{"alg":"RS256","kid":…,"typ":"JWT"}` (M1-M6; the first M5
  block is abbreviated, so F1 rests on M5b and M5c for the machine flow). No token has a payload
  `typ`. So the SMA-686 check matches no Zitadel token.
- F2. Human flow: the access token and the ID token of one response have the same `aud` array
  (M1-M4, M6). It holds A's client id, the id of P, and one more id `393381921700315139`. INFERRED:
  that id is A's application id. The live install of 2026-09-26 showed two values only.
- F3. Machine flow: the ID token `aud` is a superset of the access token `aud`. With the scope
  `urn:zitadel:iam:org:project:id:{P}:aud`, the access token has `aud = [P]` and the ID token has
  `aud = [P, <machine client id>]` (M5b, M5c). Without the scope, both have the machine client id
  only.
- F4. The scope `urn:zitadel:iam:org:project:id:{id}:aud` puts the project id on BOTH tokens, for
  P and for a second project P2 (M2, M3, M5b, M5c). No scope put an audience on the access token
  only. The literal scope `urn:zitadel:iam:org:project:id:zitadel:aud` adds the Zitadel system
  project id to both tokens (M2b).
- F5. Every token carries `client_id`, every ID token included (M8). So `client_id` does not
  separate the tokens. This disproves an earlier draft (a per-issuer `require_client_id`), which
  would refuse nothing.
- F6. Claims on every ID token and on no access token: `at_hash`, `azp`, `amr`, `auth_time`. This
  holds for the human flow (M1-M3, M6), the refresh grant (M4, M6c) and the machine flow (M5b,
  M5c).
- F7. `nonce` is on an ID token only when the client sent a `nonce` (the human flow). `sid` is on
  human-flow ID tokens only.
- F8. Claims on access tokens only: `jti`, `nbf`.
- F9. The refresh grant returns a new access token and a new ID token (M4). The new ID token keeps
  `nonce`, `auth_time` and `sid`, with a new `at_hash`.
- F10. The client-credentials grant with scope `openid` also returns an ID token (M5).
- F11. "User Info inside ID Token" and "Add user roles to the access token" change only the profile
  claims of the ID token (M6). The role claim itself was not measured (the user has no role).
- F12. No measured access token carries `email`, also with "User Info inside ID Token" on (M6). The
  ID token does carry it with that setting. IAM requires `email` in the access token (runbook § 6
  item 2). The live install added it with a Complement Token action (SMA-703 issue text).
- F13. INFERRED: an app created without `accessTokenType` gets the opaque type. IAM can validate
  only a JWT access token.

## 4. Design

### D1. A claim-name list for each issuer

`IssuerConfig` (`rs/crates/services/paigasus-iam/src/config.rs`) gets one field:

```rust
/// Claim names that mark a verified token as NOT an access token for this issuer (SMA-703).
/// Empty by default. A token that carries one of them, with any value except JSON `null`, is
/// refused as `NotAnAccessToken`. For Zitadel, use `["at_hash", "azp"]` (spec § 3, F6).
#[serde(default)]
pub id_token_marker_claims: Vec<String>,
```

An empty list keeps the current behaviour (G2). Only the operator decides which names apply,
because a name that marks an ID token for one IdP is on the access token of another IdP: Dex puts
`at_hash` and `nonce` on its access token, and Keycloak puts `azp` on its access token.

The Zitadel recipe uses `at_hash` and `azp`. Both are on every measured Zitadel ID token. `amr` and
`auth_time` are too (F6), but another IdP puts `auth_time` into its access token, so a future
Zitadel version could too. Two names keep that risk small. § 9 Q1 asks whether to add them.

Rejected alternatives:

- A global rule on `at_hash`, `nonce` or `azp`. It refuses every Dex or Keycloak access token.
- A per-issuer `require_client_id`. F5 shows a Zitadel ID token carries `client_id`.
- A named IdP profile (for example `zitadel`). Each new IdP would need a code change.
- Runbook only. It leaves Zitadel open, but a measured separating claim exists (F6).

### D2. Boot validation and the boot log line

`IamConfig::validate` refuses, with an error that names the issuer:

- an empty name, or a name with leading or trailing whitespace;
- the names `iss`, `sub`, `aud` and `exp`, because every token IAM accepts carries them, so the
  issuer would refuse every token;
- a duplicate name in one list.

Names are compared exactly (case-sensitive), because JSON member names are case-sensitive.

When the list of an issuer is not empty, IAM writes one `info` line at boot with the issuer and its
configured names. So an operator can see that the setting is on before a refusal occurs. This also
helps with a typo in a raw `iam.toml` key: `IssuerConfig` ignores unknown fields, so the missing
line is the only sign.

### D3. The check

The check is part of step 6 of `OidcAuthenticator::authenticate`, after the SMA-686 markers. So:

- An expired, wrongly signed, or wrong-audience token keeps its current defect, because the
  signature and claims validation runs first.
- A Keycloak ID token keeps the marker `ID`.
- A token that is both an ID token and bound to a key reports `NotAnAccessToken`, as in SMA-690
  D5.

A configured claim is a marker when the verified payload has a top-level member with that name and
the value is not JSON `null`. Any other value (a string, a number, an object, an empty string) is
a marker. This is the rule SMA-690 uses for `cnf`.

**The decode.** `WireClaims` cannot hold arbitrary names, so the validator needs the payload as a
map. A plain `serde_json::Map` keeps the LAST value of a repeated key with no error
(`serde_json` `map.rs`, `values.insert`), and a derived struct does not check a duplicate of a
field it ignores. So both obvious methods let `{"at_hash":"x","at_hash":null}` pass. The design:

- A private type `StrictPayload(serde_json::Map<String, Value>)` with a hand-written
  `Deserialize`. Its map visitor returns an error on a repeated top-level key.
- When the issuer's list is NOT empty, the validator calls `decode::<StrictPayload>` with the same
  `Validation`. The signature is checked once. A duplicate top-level key fails serde, and
  `map_jwt_error` maps that to `Malformed`. `WireClaims` is read from the map INSIDE
  `StrictPayload::deserialize`, not after `decode` returns: `jsonwebtoken` 11.1.0 deserializes the
  caller's type before it validates `exp` and `aud` (`src/decoding.rs:287-288`), so this keeps the
  current defect order (a wrong-shaped claim stays `Malformed` ahead of `Expired`). The configured
  names are looked up in the map.
- When the list IS empty, the validator calls `decode::<WireClaims>` as today. So G2 keeps the
  current code path exactly.

The map holds only bytes that `jsonwebtoken` verified. This adds no unverified read. A duplicate
`cnf` stays `Malformed` on both paths, so `duplicate_cnf_claim_is_never_authenticated` keeps its
result.

`ConfiguredIssuer` gets the list, so the request path does not read `IssuerConfig` again. The
composition root passes the whole `IssuerConfig` already, so no wiring changes.

### D4. The log line

The refusal uses the existing `log_refusal` with `TokenDefect::NotAnAccessToken`. The marker text
is `claim <name>`, for example `claim at_hash`. The name is a configured value, so the log never
shows token material (SMA-686 D8). `RefusalDetail::Marker` holds a `&'static str` today. The plan
changes it to hold a configured string too, or adds a variant. The text of the log message does not
change. The rate limit (one line for each issuer and defect in 10 seconds) applies.

No new metric. No change to the HTTP or gRPC response: a 401 with `invalid_token`, and the defect is
not exposed.

### D5. The chart

- New optional value `oidc.idTokenMarkerClaims`, default `[]`, documented in
  `charts/paigasus/values.yaml` next to `oidc.audience`, and in `charts/paigasus/README.md` next to
  the other `oidc.*` sections.
- `templates/backend-deployment.yaml` adds `,id_token_marker_claims=[…]` to the one issuer entry of
  `IAM_AUTHN__ISSUERS` only when the list is not empty. Each item is quoted with `%q`, like
  `audiences`. With the default, the rendered value is byte-identical to today. No new YAML comment
  goes outside an `{{- if }}` block, so no existing golden changes.
- The chart reads the value with `dig`, because `--reuse-values` from an older release leaves it
  nil.
- The chart copies IAM's boot rules (D2), because a boot failure stops IAM: the IAM Deployment has
  one replica with `maxSurge: 0` and `maxUnavailable: 1`, so the old pod stops before the new pod
  fails. SMA-692 made the chart copy the pod's whitespace rule for the same reason. The chart fails
  the render, with a message that names `oidc.idTokenMarkerClaims`, when:
  - the value is not a list;
  - an item is not a string, or is empty;
  - an item has a character outside printable ASCII, or a space, `"` or `\`. This also covers
    leading and trailing whitespace. Go's `%q` writes escapes (`\a`, `\v`, `\xNN`) that figment
    does not read, so this rule keeps the env value parseable;
  - an item is `iss`, `sub`, `aud` or `exp`;
  - the list has a duplicate.
- The checks go into `paigasus.validateIamBackend` (`templates/_iam-backend.tpl`), which already
  checks `bootstrapAdmins` with `dig` and `kindIs "slice"`. Not into `_helpers.tpl`, whose
  whole-file copies in `ci/helm-render/fixtures/` would then need a re-sync.
- A change of the value changes the IAM pod template, so IAM restarts with a short gap (one
  replica). The runbook says so.
- The SMA-691 warning logic does not change (N3). A Zitadel operator decides the audience as in D6.
- The chart does not need a new required value, so `helm_render.py` `STUB_VALUES`, the seven chart
  scripts' required-value lists and `ci/kind/values/a.yaml` do not change. The plan adds rows to the
  existing `env.sh` and `refusals.sh`, and no new `tests/*.sh` script.

### D6. The runbook

`docs/ops/RUNBOOK-chart.md` § 6:

- The SMA-686 paragraph ("The check does not protect an IdP whose ID token has no `typ` claim")
  gets the opt-in: `oidc.idTokenMarkerClaims` refuses a token that carries a named claim, for an
  IdP whose ID token has a claim that its access token never has.
- The acknowledgement paragraph ("IAM still accepts an ID token as a bearer token, except a
  Keycloak ID token") adds: "or a token with a claim named in `oidc.idTokenMarkerClaims`".
- A new "Zitadel" bullet in the per-IdP list, after Dex, labelled "Measured, Zitadel v4.15.3 with
  Login v1, 2026-10-02 (SMA-703)", because the list header says "Not measured". It states, short:
  - Both tokens have the same `aud` in the human flow. In the machine flow the ID token `aud`
    holds the access token `aud`. Both tokens have `client_id`. No `urn:zitadel:…:aud` scope puts
    an audience on the access token only (F2-F5).
  - Set the access token type to JWT on the app, and on each machine user (F13).
  - IAM needs `email` in the access token. Zitadel does not put it there (F12). Add it with a
    Zitadel Action (not measured here). Never send the ID token instead.
  - Set `oidc.idTokenMarkerClaims: ["at_hash", "azp"]`.
  - The audience. Option 1, for an install with machine clients: set `oidc.audience` to the project
    id, and let each machine client request the project scope. Without the scope a machine token has
    only its own client id and IAM refuses it as `AudienceMismatch`. Option 2, for a console-only
    install: keep the client id and set `oidc.acknowledgeClientIdAudience`.
  - Before the switch: decode one access token for EACH grant type in use and check that it has
    neither claim. Decode one ID token and check that it has both claims.
  - After the switch: send an ID token to IAM. Expect a 401 and the IAM log line with
    `claim at_hash`.
  - A change of the value restarts IAM.
  - After each Zitadel upgrade, decode the tokens again. If a new version puts `azp` or `at_hash`
    into the access token, IAM refuses every token, and the log line names the claim. If a new
    version drops both from the ID token, the protection stops with no sign.
- The Dex bullet adds: `oidc.idTokenMarkerClaims: ["c_hash"]` refuses a Dex ID token from the code
  flow only. A refreshed Dex ID token has no separating claim, so R1 stays open (SMA-686 § 2).

### D7. Acceptance criteria mapping

| SMA-703 criterion | Where |
|---|---|
| A decision for how IAM tells the tokens apart | D1, D3 |
| A test with a realistic Zitadel token shape | § 5, T1-T4 |
| Runbook § 6 names Zitadel's status | D6 |
| The `urn:zitadel:…:aud` scope verified on a real instance | § 3 F4, the measurement file |

### D8. No Notion ADR

Default: no Notion ADR, like SMA-686 D10. The change adds one opt-in refusal and one optional
config field, and the decision is recorded here and in the field's doc comment. § 9 Q2 asks Sven to
confirm.

## 5. Tests

Unit tests in `validator.rs` (`mod tests`). The Zitadel fixtures copy the claim NAMES and value
shapes of M1, M4 and M5b/M5c. They do not copy the times: `exp`, `iat`, `nbf` and `auth_time` are
set relative to `Utc::now()`, because the measured `exp` values end on 2026-10-03. `iss` is the
test issuer. `aud` stays an array (so `WireAudience::Multiple` runs) that contains the test
audience. The issuer is configured with `["at_hash", "azp"]` unless the test says otherwise.

- T1. The M1 human-flow ID token is refused as `NotAnAccessToken`.
- T2. The M5b machine ID token (no `nonce`) is refused as `NotAnAccessToken`.
- T3. The M1 and M5b access tokens are accepted.
- T4. The M4 refreshed ID token is refused, and the refreshed access token is accepted.
- T5. With the default empty list, the M1 ID token is accepted. This pins G2 and documents the open
  state without the setting.
- T6. With the default empty list, the Dex shape (`at_hash`, `c_hash`, `nonce`) is accepted.
- T7. A configured claim with value `null` is not a marker. The values `""`, `0`, `{}` and `[]` are
  markers.
- T8. With `["at_hash"]` only, a token with `azp` and without `at_hash` is accepted. Only the
  configured names count.
- T9. An expired M1 ID token reports `Expired`, and an M1 ID token with a wrong `aud` reports
  `AudienceMismatch`.
- T10. A Keycloak `typ: ID` token with `at_hash` reports the marker `ID`. The SMA-686 markers run
  first.
- T11. The refusal log line names the issuer and `claim at_hash`. It does not contain the claim's
  value, the subject or the email. Repeated refusals log one line.
- T12. With the list set, a token with a duplicate `at_hash` member (one `null`, one string, in both
  orders) is refused as `Malformed`. A duplicate `cnf` is `Malformed` with the list set and with
  the list empty.
- T13. Name matching is case-sensitive: with `["at_hash"]`, a token with `AT_HASH` only is accepted.
- T14. Two issuers, one with `["at_hash"]` and one with an empty list. An ID token from the first is
  refused. The same token shape from the second is accepted.
- T15. The boot log line (D2) names the issuer and the configured names, and is absent for an
  empty list.

Config tests in `config.rs`:

- T16. Each name refused by D2 fails `validate` with the issuer in the message.
- T17. A figment env value
  `[{issuer="…",audiences=["…"],id_token_marker_claims=["at_hash","azp"]}]` parses into the list.
  A value without the key parses as an empty list.

Chart rows:

- T18. `env.sh`: with `oidc.idTokenMarkerClaims: ["at_hash","azp"]`, `IAM_AUTHN__ISSUERS` holds the
  quoted list. With the default, the value is unchanged. A change of the value changes the pod
  template (a restart row, like the existing A6).
- T19. `refusals.sh`: one row for each D5 rule (not a list, a non-string item, an empty item, a
  space or a control character, a reserved name, a duplicate). Each fails the render with a
  message that names `oidc.idTokenMarkerClaims`.
- T20. A render with the key absent from `oidc` (the `--reuse-values` case) succeeds.

Existing `IssuerConfig { … }` literals (`validator.rs`, `tests/support/mod.rs`,
`tests/keycloak_e2e.rs`, `tests/authn_private_ca.rs`) get the new field. The Keycloak e2e test does
not change its behaviour.

## 6. Error handling and failure modes

- A wrong name (for example `nonce` on an IdP whose access token carries `nonce`) refuses every
  login for that issuer. The failure is loud: every request gets 401, and the IAM log names the
  marker. The runbook tells the operator to decode a real access token for each grant type first.
- A name that is absent from the IdP's ID token gives no protection, and nothing warns. The runbook
  recipe uses two names, both measured on every Zitadel ID token, and tells the operator to check an
  ID token.
- A future Zitadel version can add `azp` or `at_hash` to its access token. IAM then refuses every
  login, with the log line above. This fails closed.
- A future Zitadel version can drop both claims from its ID token. The protection then stops, and
  nothing warns. A Zitadel end-to-end test (N2) would detect this for the pinned version.
- An invalid value in the chart fails the render, not the IAM boot (D5).

## 7. Security notes

- The check reads only the verified payload. It adds no unverified read.
- A duplicate top-level key is `Malformed` on the new path (D3). On the old path the behaviour does
  not change.
- The log carries configured names, never token values.
- The list adds a refusal only. It cannot make IAM accept a token that it refuses today.

## 8. Rollout

No migration. The default is empty. An operator turns the check on with one chart value. The change
restarts IAM once. A rollback is the removal of that value, with one more restart. Use
`--set oidc.idTokenMarkerClaims=null`, or `[]` in a values file. `--set oidc.idTokenMarkerClaims={}`
does NOT clear the list: Helm 3.22.0 makes it `[""]`, and the chart refuses that (measured during
planning).

The boot line of D2 is written in `OidcAuthenticator::new`, not in `IamConfig::validate`, because
`validate` runs before the logger starts (`main.rs:63-64`).

## 9. Open questions for Sven

Sven approved the spec on 2026-10-02 with no answer to Q1-Q3. So Q1 and Q2 take their defaults
(two names; no Notion ADR). Q3 stays open and does not block this issue.

- Q1. The Zitadel recipe: `["at_hash", "azp"]` (default), or all four measured names with `amr`
  and `auth_time` too? Four names also cover an ID token from an unmeasured flow that has no
  `at_hash`. They add two names that a future Zitadel version could put into the access token.
- Q2. No Notion ADR, like SMA-686 D10 (default)?
- Q3. M4 suggests that a refreshed Zitadel access token keeps the original `iat` and `exp`. If true,
  a console session cannot extend its access token. Out of scope here. Should I open a Linear issue
  to measure it?
  - Answer (SMA-732, 2026-10-06): MEASURED, Zitadel v4.15.3. A refresh gives an access token with
    `exp` = refresh time + lifetime, before and after the first `exp`. M4 probably showed equal
    `iat` and `exp` because its refresh came about 10 ms after the login (an inference from the
    `jti` values, K4). This run measured an immediate refresh (Tq) 7 ms and 6 ms after the login,
    and it showed the same equal `iat` and `exp`. Limits: Login v1, a confidential
    web app, JWT access tokens, a lifetime of 10 s. The 12 h default, Login v2 and opaque tokens
    use the same code path, but that is code reading, not a measurement. See
    `2026-10-05-sma-732-zitadel-refresh-exp-measurements.md`.

## 10. Follow-ups

- DONE (§ 12): A Docker Zitadel end-to-end test, like `keycloak_e2e.rs`, that pins F6 for the
  pinned version.
- The `email` claim in a Zitadel access token: measure a Zitadel Action that adds it. § 12 runs
  the `addEmailClaim` Action of the reference install in the test, on the human flow and on the
  refresh grant.

## 11. Spec challenge (2026-10-02)

Verdict: APPROVE WITH CHANGES. The coordinator checked the BLOCKER and the evidence findings
against the code and the measurement file before folding them in.

| Finding | Severity | Outcome |
|---|---|---|
| Both named decode methods let a duplicate member pass | BLOCKER | Folded in: D3 `StrictPayload`, T12 pins `Malformed`. |
| F2/F3 contradict the evidence (machine flow) | MAJOR | Folded in: F2, F3 restated. The extra `aud` id marked INFERRED. |
| The Zitadel recipe is not a working setup | MAJOR | Folded in: F12, N5, D6 (email, audience options, machine token type, per-grant checks). |
| The fixtures expire on 2026-10-03 | MAJOR | Folded in: § 5 preamble. Machine fixtures from M5b/M5c. |
| The chart must copy the boot rules | MAJOR | Folded in: D5, T19. |
| The Dex sentence is not accurate | MINOR | Folded in: N1, D6 (`c_hash` partial). |
| Runbook and NOTES text become stale | MINOR | Runbook folded in (D6). NOTES drift accepted (N3). |
| The measurement file cannot be reproduced | MINOR | Folded in: its Reproduction section now says the scripts are not committed. |
| The first M5 block is edited by hand | MINOR | Folded in: marked abbreviated; F1 rests on M5b/M5c. |
| The location of the chart validation | MINOR | Folded in: `_iam-backend.tpl` (D5). |
| `%q` and figment escapes do not agree | MINOR | Folded in: the character rule in D5. |
| No test for two issuers | MINOR | Folded in: T14. |
| No boot-time signal | MINOR | Folded in: D2, T15. |
| Restart and README | MINOR | Folded in: D5, D6, § 8, T18. |
| Zitadel upgrades | MINOR | Folded in: D6. The alert name was not verified, so the runbook names the log line only. |
| F6 needs its condition | MINOR | Folded in: F7. |
| The ADR decision is missing | MINOR | Folded in: D8, Q2. |
| Two names or four? | QUESTION | Q1. |
| What is `393381921700315139`? | QUESTION | Marked INFERRED (F2). Whether other apps of P add their client ids is not measured; the runbook makes no claim about it. |
| Login v2 label | QUESTION | Folded in: the runbook label names Login v1. |
| Refreshed access token keeps `iat`/`exp` | QUESTION | Q3. Answered by SMA-732: a refresh extends `exp` (measured, see the Q3 answer). |
| T12: which defect? | QUESTION | `Malformed`. |

## 12. Addendum (2026-10-03): Zitadel end-to-end test

`rs/crates/services/paigasus-iam/tests/zitadel_e2e.rs` runs a real Zitadel `v4.15.3` in Docker,
with its own Postgres (`16-alpine`) on a Docker network, and IAM's own migrated Postgres. It
completes the first follow-up in § 10, and it relaxes N2: the test runs in CI with the other
Docker-backed suites. Sven approved the design ("full console path").

The setup is the paigasus console's client type. This fills the gap "a confidential app is not
measured" of § 3:

- A confidential web app (`OIDC_AUTH_METHOD_TYPE_BASIC`), code flow with PKCE, refresh token,
  access token type JWT, "User Info inside ID Token" on.
- A human user who logs in through Login v1. The test uses plain HTTP requests, with no browser, and sends the forms itself.
- A machine user with a client secret, the client-credentials grant, and access token type JWT.
- The `addEmailClaim` v1 Action (Complement Token, pre access token creation).

The test asserts:

- Every ID token (human, refreshed, machine) has `at_hash` and `azp`. No access token has either.
  This checks F6 for the pinned version.
- The Action puts `email` into the human access token, also after the refresh grant. This fills
  the gap of F12 and N5 for the reference install.
- IAM with `["at_hash", "azp"]` refuses each ID token as `NotAnAccessToken`, and a protected route
  returns 401 `invalid-token`.
- The human access token provisions a user by JIT. The refreshed access token resolves to the same
  principal. The machine access token passes the authenticator. It has no `email`, so JIT
  provisioning fails with `MissingEmail`.
- A control: with an empty list, IAM does not refuse the human, the refreshed or the machine ID token as `NotAnAccessToken`. The human and the refreshed ID token resolve to the human principal. The machine ID token gives `IdentityNotProvisioned`, and `ProvisioningFailed(MissingEmail)` with provisioning on.

Measured during the build of the test (2026-10-03, v4.15.3):

- A confidential app gives `aud = [client id, P]` on both human tokens. The public app of § 3 had a
  third value (F2).
- Zitadel takes the host of `iss` from `ZITADEL_EXTERNALDOMAIN`. It takes the port of `iss` from
  the request `Host` header, not from `ZITADEL_EXTERNALPORT`. A request with another host gets
  404.
- The discovery endpoint answers some seconds before the management API. The API first returns
  503.

A mutation shows that the test can fail: with `["nonce"]` as the IAM list, the machine ID token
(which has no `nonce`) is not refused, and the test fails.
