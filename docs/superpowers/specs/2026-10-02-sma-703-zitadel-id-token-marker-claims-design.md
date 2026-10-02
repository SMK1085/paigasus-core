# SMA-703: IAM refuses a Zitadel ID token by a configured claim name

- Linear: SMA-703 (follows SMA-686 and SMA-691)
- Status: draft (2026-10-02), before the spec challenge.
- Path: architectural (a change to what IAM accepts from the IdP, and a new chart value).
- Measurement: `2026-10-02-sma-703-zitadel-measurements.md` (Zitadel v4.15.3, real tokens).

## 1. Problem

IAM must refuse an ID token that a client presents as a bearer token. SMA-686 added a token-type
check. It matches a Keycloak payload `typ` (`ID`, `Logout`) and the standard back-channel logout
markers. SMA-691 added a chart warning when the IAM audience equals `oidc.clientId`. Its remedy is
an API audience that only the access token carries.

Zitadel defeats both defences. A live install (v4.15.3, 2026-09-26) showed the same `aud` on both
tokens. The measurement for this spec (§ 3) shows more: no Zitadel setting separates the two
tokens by `aud`, and no Zitadel token carries a `typ` that the SMA-686 check matches. So today IAM
accepts a Zitadel ID token as a bearer token.

## 2. Goals and non-goals

Goals:

- G1. An operator can make IAM refuse a Zitadel ID token, with a setting for each issuer.
- G2. The default behaviour does not change for any IdP. Keycloak, Dex, Okta, Auth0 and Entra ID
  installs need no change.
- G3. `docs/ops/RUNBOOK-chart.md` § 6 states Zitadel's measured status next to Keycloak and Dex.
- G4. A test uses the measured Zitadel token shapes.

Non-goals:

- N1. A fix for Dex. A Dex access token carries `at_hash` and `nonce` (SMA-686 measurement), so a
  claim-name list cannot separate Dex tokens. SMA-686 residual R1 stays open for Dex.
- N2. A Docker Zitadel end-to-end test in CI. Follow-up issue (§ 10).
- N3. A change to the SMA-691 warning logic or text.
- N4. More than one issuer in the chart. The chart keeps exactly one issuer.

## 3. Measured facts (Zitadel v4.15.3, 2026-10-02)

Source: the measurement file. Every fact here is MEASURED on real tokens, unless it is marked
INFERRED. The setup: one project P, one web app A (PKCE, access token type JWT), one human user,
the built-in login v1, and one machine user with client credentials.

- F1. The header of every token is `{"alg":"RS256","kid":…,"typ":"JWT"}`. No token has a payload
  `typ`. So the SMA-686 check matches no Zitadel token.
- F2. The access token and the ID token of one response have the same `aud` array. Without an
  audience scope it holds the client ids of P's apps and the id of P.
- F3. The scope `urn:zitadel:iam:org:project:id:{id}:aud` adds the project id to the `aud` of BOTH
  tokens (M2, M3). This was also true for a second project P2. No scope put an audience on the
  access token only. The literal scope `urn:zitadel:iam:org:project:id:zitadel:aud` adds the
  Zitadel system project id to both tokens.
- F4. Every token carries `client_id`, and the ID token too (M8). So `client_id` does not separate
  the tokens. This disproves the design of an earlier draft (a per-issuer `require_client_id`),
  which would refuse nothing.
- F5. Claims on every ID token and on no access token: `at_hash`, `azp`, `amr`, `auth_time`. This
  holds for the human flow (M1, M2, M3, M6), the refresh grant (M4) and the machine flow (M5).
- F6. Claims on human-flow ID tokens only: `nonce`, `sid`. The machine ID token has no `nonce`.
- F7. Claims on access tokens only: `jti`, `nbf`.
- F8. The refresh grant returns a new access token and a new ID token (M4). The new ID token keeps
  `nonce`, `auth_time` and `sid`, with a new `at_hash`.
- F9. The client-credentials grant with scope `openid` also returns an ID token (M5).
- F10. "User Info inside ID Token" and "Add user roles to the access token" change only the profile
  claims of the ID token (M6). The role claim itself was not measured (the user has no role).
- F11. INFERRED: an app created without `accessTokenType` gets the opaque type. IAM can validate
  only a JWT access token, so the app must use the JWT type.

Not measured: Login v2, a confidential app, other Zitadel versions.

## 4. Design

### D1. A claim-name list for each issuer

`IssuerConfig` (`rs/crates/services/paigasus-iam/src/config.rs`) gets one field:

```rust
/// Claim names that mark a verified token as NOT an access token for this issuer (SMA-703).
/// Empty by default. A token that carries one of them, with any value except JSON `null`, is
/// refused as `NotAnAccessToken`. For Zitadel, use `["at_hash", "azp"]` (spec § 3, F5).
#[serde(default)]
pub id_token_marker_claims: Vec<String>,
```

An empty list keeps the current behaviour (G2). Only the operator decides which names apply,
because a name that marks an ID token for one IdP is on the access token of another IdP: Dex puts
`at_hash` and `nonce` on its access token, and Keycloak puts `azp` on its access token.

Rejected alternatives:

- A global rule on `at_hash`, `nonce` or `azp`. It refuses every Dex or Keycloak access token.
- A per-issuer `require_client_id`. F4 shows a Zitadel ID token carries `client_id`.
- A named IdP profile (for example `zitadel`). Each new IdP would need a code change.
- Runbook only. It leaves Zitadel open, but a measured separating claim exists (F5).

### D2. Boot validation

`IamConfig::validate` refuses, with an error that names the issuer:

- an empty name, or a name with leading or trailing whitespace;
- the names `iss`, `sub`, `aud` and `exp`, because every token IAM accepts carries them, so the
  issuer would refuse every token;
- a duplicate name in one list.

Names are compared exactly (case-sensitive), because JSON member names are case-sensitive.

### D3. The check

The check is part of step 6 of `OidcAuthenticator::authenticate`, after the SMA-686 markers. So:

- An expired, wrongly signed, or wrong-audience token keeps its current defect, because `decode`
  runs first.
- A Keycloak ID token keeps the marker `ID`.
- A token that is both an ID token and bound to a key reports `NotAnAccessToken`, as in SMA-690
  D5.

A configured claim is a marker when the verified payload has a member with that name and the value
is not JSON `null`. Any other value (a string, a number, an object, an empty string) is a marker.
This is the rule SMA-690 uses for `cnf`.

`WireClaims` cannot hold arbitrary names. The validator therefore also decodes the verified payload
into a `serde_json::Map<String, Value>`, only when the issuer's list is not empty. This decode reads
the same bytes that `jsonwebtoken` already verified. It is not an unverified read. The plan
chooses how to get those bytes (for example a second `decode::<serde_json::Map<…>>` with the same
`Validation`, or one decode into the map followed by `serde_json::from_value` into `WireClaims`).
Whatever the method, a duplicate member name must never let a token pass (see the SMA-690
`duplicate_cnf_claim_is_never_authenticated` test). A test pins this for a configured name.

`ConfiguredIssuer` gets the list, so the request path does not read `IssuerConfig` again.

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
  `charts/paigasus/values.yaml` next to `oidc.audience`.
- `templates/backend-deployment.yaml` adds `,id_token_marker_claims=[…]` to the one issuer entry of
  `IAM_AUTHN__ISSUERS` only when the list is not empty. Each item is quoted with `%q`, like
  `audiences`. With the default, the rendered value is byte-identical to today, so no existing
  golden changes.
- The chart reads the value with `dig`, because `--reuse-values` from an older release leaves it nil.
- The chart fails the render when the value is not a list, or when an item is not a non-empty
  string. IAM's boot validation (D2) remains the full check. The chart does not copy the
  reserved-name rule.
- The SMA-691 warning does not change (N3). Its text stays true, because the new check is not an
  audience check. A Zitadel operator sets `oidc.acknowledgeClientIdAudience` as well.
- The chart does not need a new required value, so `helm_render.py` `STUB_VALUES`, the seven chart
  scripts' required-value lists and `ci/kind/values/a.yaml` do not change. If the plan adds a new
  `tests/*.sh` script, the `CHART_SCRIPT_FLOOR` rule in `charts/CLAUDE.md` applies. The plan should
  add rows to the existing `env.sh` and `refusals.sh` instead.

### D6. The runbook

`docs/ops/RUNBOOK-chart.md` § 6:

- The SMA-686 paragraph ("The check does not protect an IdP whose ID token has no `typ` claim")
  gets the opt-in: `oidc.idTokenMarkerClaims` refuses a token that carries a named claim, for an
  IdP whose ID token has a claim that its access token never has.
- A new "Zitadel" bullet in the per-IdP list, after Dex, marked "Measured, Zitadel v4.15.3,
  2026-10-02 (SMA-703)", because the list header says "Not measured". It states F2, F3 and F4 in
  short form, and the setup:
  - Set the app's access token type to JWT (F11).
  - Set `oidc.idTokenMarkerClaims: ["at_hash", "azp"]`.
  - Set `oidc.acknowledgeClientIdAudience` to the client id.
  - Before the switch, decode a real access token and check that it has neither claim.
- The Dex bullet gets one sentence: the list cannot help Dex, because a Dex access token carries
  `at_hash` and `nonce`.

### D7. Acceptance criteria mapping

| SMA-703 criterion | Where |
|---|---|
| A decision for how IAM tells the tokens apart | D1, D3 |
| A test with a realistic Zitadel token shape | § 5, tests T1-T4 |
| Runbook § 6 names Zitadel's status | D6 |
| The `urn:zitadel:…:aud` scope verified on a real instance | § 3 F3, the measurement file |

## 5. Tests

Unit tests in `validator.rs` (`mod tests`). The Zitadel claim sets are copied from the measurement
file, with `iss` and `aud` changed to the test issuer and audience. The issuer is configured with
`["at_hash", "azp"]` unless the test says otherwise.

- T1. The M1 human-flow ID token is refused as `NotAnAccessToken`.
- T2. The M5 machine ID token (no `nonce`) is refused as `NotAnAccessToken`.
- T3. The M1 and M5 access tokens are accepted.
- T4. The M4 refreshed ID token is refused, and the refreshed access token is accepted.
- T5. With the default empty list, the M1 ID token is accepted. This pins G2 and documents the
  open state without the setting.
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
- T12. A token with a duplicate `at_hash` member (one `null`, one string, in both orders) is never
  authenticated.
- T13. Name matching is case-sensitive: with `["at_hash"]`, a token with `AT_HASH` only is accepted.

Config tests in `config.rs`:

- T14. Each name refused by D2 fails `validate` with the issuer in the message.
- T15. A figment env value
  `[{issuer="…",audiences=["…"],id_token_marker_claims=["at_hash","azp"]}]` parses into the list.
  A value without the key parses as an empty list.

Chart rows:

- T16. `env.sh`: with `oidc.idTokenMarkerClaims: ["at_hash","azp"]`, `IAM_AUTHN__ISSUERS` holds the
  quoted list. With the default, the value is unchanged.
- T17. `refusals.sh`: a non-list value, an empty-string item, and a non-string item each fail the
  render with a message that names `oidc.idTokenMarkerClaims`.
- T18. A render with `--reuse-values`-style nil (the key absent from `oidc`) succeeds.

Existing `IssuerConfig { … }` literals (`config.rs`, `validator.rs`, `tests/support/mod.rs`,
`tests/keycloak_e2e.rs`, `tests/authn_private_ca.rs`) get the new field. The Keycloak e2e test does
not change its behaviour.

## 6. Error handling and failure modes

- A wrong name (for example `nonce` on an IdP whose access token carries `nonce`) refuses every
  login for that issuer. The failure is loud: every request gets 401, and the IAM log names the
  marker. The runbook tells the operator to decode a real access token first.
- A name that is absent from the IdP's ID token gives no protection, and nothing warns. The runbook
  recipe uses two names (`at_hash`, `azp`), both measured on every Zitadel ID token.
- A future Zitadel version can add `azp` or `at_hash` to its access token. IAM then refuses every
  login, with the log line above. This fails closed.
- A future Zitadel version can drop both claims from its ID token. The protection then stops, and
  nothing warns. A Zitadel end-to-end test (N2) would detect this.

## 7. Security notes

- The check reads only the verified payload. It adds no unverified read.
- The log carries configured names, never token values.
- The list adds a refusal only. It cannot make IAM accept a token that it refuses today.

## 8. Rollout

No migration. The default is empty. An operator turns the check on with one chart value. A rollback
is the removal of that value.

## 9. Open questions

- Q1. Should the SMA-691 warning stay quiet when `oidc.idTokenMarkerClaims` is set? Default: no
  (N3). The acknowledgement already exists for this.
- Q2. Should the plan commit the measurement scripts? Default: no. Only the results file is
  committed. The scripts were run stage by stage and not end to end.

## 10. Follow-ups

- A Docker Zitadel end-to-end test, like `keycloak_e2e.rs`, that pins F5 for the pinned version.
