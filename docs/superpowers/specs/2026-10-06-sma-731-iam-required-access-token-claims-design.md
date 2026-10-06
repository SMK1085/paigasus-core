# SMA-731: IAM refuses a token that does not carry a configured claim

- Linear: SMA-731 (follows SMA-703, PR 361).
- Status: approved by Sven (2026-10-06), after the spec challenge in § 11. Q-a to Q-c take their
  defaults.
- Path: architectural (a change to what IAM accepts from the IdP, and a new chart value).
- Template: `2026-10-02-sma-703-zitadel-id-token-marker-claims-design.md`. This spec copies its
  shape. Where this spec does not say otherwise, the SMA-703 decision applies.

## 1. Problem

SMA-703 added `id_token_marker_claims`. IAM refuses a verified token that carries a configured
claim. The Zitadel recipe is `["at_hash", "azp"]`. This check is a denylist, so it fails open.

If a future Zitadel version removes `at_hash` and `azp` from its ID token, IAM accepts Zitadel ID
tokens as bearer tokens again. No log line, metric or boot line shows that the protection stopped.
The local review of PR 361 raised this (finding 3) and deferred it.

`tests/zitadel_e2e.rs` detects such a change for the pinned image (`v4.15.3`) only. It does not
protect an install that upgrades Zitadel before the pin moves.

## 2. Goals and non-goals

Goals:

- G1. An operator can make IAM refuse a token that does not carry a configured claim, with a
  setting for each issuer. This check fails closed.
- G2. The default (an empty list) keeps the current behaviour for every IdP.
- G3. `docs/ops/RUNBOOK-chart.md` § 6 gives the Zitadel recipe `["jti"]`, and says what an operator
  sees when the claim disappears.
- G4. `tests/zitadel_e2e.rs` shows that IAM refuses each Zitadel ID token when `["jti"]` is
  configured, and that the access tokens still pass.
- G5. `tests/keycloak_e2e.rs` measures `jti` on the Keycloak ID token and access token. The runbook
  gives a Keycloak recipe only if the measurement shows a split.

Non-goals:

- N1. A measurement of Dex, Okta, Auth0 or Entra ID. The runbook says "not measured" for them.
- N2. A new `TokenDefect` variant, a new metric, or a change to the HTTP or gRPC response.
- N3. A check of the claim's value (type, length, format). Only presence counts.
- N4. More than one issuer in the chart (as SMA-703 N4).

## 3. Measured facts

From `2026-10-02-sma-703-zitadel-measurements.md` (Zitadel v4.15.3, M1-M8) and SMA-703 § 3 F8:

- F1. `jti` and `nbf` are on every Zitadel access token and on no Zitadel ID token. This holds for
  the human flow (Login v1, public PKCE app), the refresh grant and the client-credentials grant.
  The measurement file calls the split "INFERRED to be stable; seen in all 20+ tokens"
  (`2026-10-02-sma-703-zitadel-measurements.md:961`). Not measured: Login v2, the JWT-profile
  grant, token exchange.
- F2. The homelab rollout on 2026-10-03 (a confidential app with Login v2) confirmed `jti` on a
  live access token. It did not report on `jti` in the ID token.
- F3. Not measured: whether other IdPs follow this split. Keycloak 26.4 is measured by this issue
  (D8, T20). The result goes into this section as F4 when the plan's first task has run.
- F4. Keycloak 26.4 (measured 2026-10-06 by `tests/keycloak_e2e.rs`): `jti` is on the ID token
  and the access token of the password grant, on the ID token and the access token of the
  refresh grant, and on the DPoP-bound access token. So `jti` does not separate the Keycloak
  tokens. The runbook gives no Keycloak recipe, and the `typ` check (SMA-686) stays the Keycloak
  defence (D6, D8). T20 asserts these five values.

`nbf` is not part of the recipe. One name keeps the recipe small, and `jti` is the claim that
RFC 9068 (JWT access token profile) lists as required. `nbf` is optional in RFC 9068.

## 4. Design

### D1. A required-claim list for each issuer

`IssuerConfig` (`rs/crates/services/paigasus-iam/src/config.rs`) gets one field, next to
`id_token_marker_claims`:

```rust
/// Claim names that every accepted token of this issuer must carry (SMA-731). Empty by default.
/// A verified token that does not carry one of them, or carries it with the value JSON `null`,
/// is refused as `NotAnAccessToken`. For Zitadel, use `["jti"]` (SMA-731 spec § 3, F1).
#[serde(default)]
pub access_token_required_claims: Vec<String>,
```

The name says what the list holds: the claims an access token is required to carry. The chart
value is `oidc.accessTokenRequiredClaims`.

An empty list keeps the current behaviour (G2). The operator decides which names apply, because
the split differs for each IdP.

Rejected alternatives:

- A global rule that requires `jti`. A Keycloak or Dex install whose access token has no `jti`
  would refuse every login.
- A new defect (for example `MissingRequiredClaim`). Sven chose `NotAnAccessToken` (2026-10-06).
  The log line names the claim, so the operator can tell the two checks apart.
- Runbook only. It leaves the fail-open gap of § 1.

### D2. Boot validation and the boot log line

`IamConfig::validate` refuses, with an error that names the issuer and the field:

- an empty name, or a name with leading or trailing whitespace;
- the names `iss`, `sub`, `aud` and `exp`. Every token that IAM accepts carries them, so the name
  has no effect. The message says so;
- a duplicate name in one list;
- a name that is also in `id_token_marker_claims` of the same issuer. IAM would then refuse every
  token of this issuer: a token with the claim is refused as an ID token, and a token without it
  is refused as missing it. The message names both fields.

Names are compared exactly (case-sensitive).

The empty, whitespace, reserved and duplicate rules are the same for both lists. A shared private
function in `config.rs` validates one list, with the field name and the reserved-name message as
parameters. Thus the two lists cannot drift apart. The SMA-703 messages for
`id_token_marker_claims` do not change, so the existing tests keep their assertions.
`RESERVED_MARKER_CLAIMS` (`config.rs:271`) becomes `RESERVED_CLAIM_NAMES`, because it now covers
both lists. Its doc comment and the chart comment that refers to it change with it.

When the list of an issuer is not empty, `OidcAuthenticator::new` writes one `info` line at boot,
with the issuer and its configured names. It is written there and not in `validate`, because
`validate` runs before the logger starts (SMA-703 § 8).

### D3. The check

The check is step 6c of `OidcAuthenticator::authenticate`:

1. The decode (signature, `iss`, `aud`, `exp`, `nbf`).
2. Step 6: the SMA-686 markers (`typ`, `events`, the header `typ`).
3. Step 6b: the SMA-703 configured marker claims.
4. Step 6c (new): the required claims.
5. Step 7: the sender-constraint check (`cnf`).

So:

- An expired, wrongly signed or wrong-audience token keeps its current defect.
- A Keycloak ID token keeps the marker `ID`.
- A Zitadel ID token with both settings reports `claim at_hash` (step 6b), not `missing claim jti`.
- A token that misses a required claim and is bound to a key reports `NotAnAccessToken`, as SMA-703
  does for a marker claim.

A required claim is present when the verified payload has a top-level member with that name and the
value is not JSON `null`. Any other value (a string, a number, `false`, an object, an empty string)
counts as present. This is the SMA-703 rule with the opposite result. The first configured name
that is missing is the one in the log.

**The decode.** The validator uses `StrictPayload` when EITHER list of the issuer is not empty. A
duplicate top-level member then fails as `Malformed` (SMA-703 D3). So `{"jti":"x","jti":null}`
cannot pass, and `{"jti":null,"jti":"x"}` cannot pass. When both lists are empty, the validator
calls `decode::<WireClaims>` as today, so G2 keeps the current code path exactly.

**A value object for the two rules.** `ConfiguredIssuer` does not get a second loose list. A new
private type `ClaimRules { id_token_markers: Vec<String>, required: Vec<String> }` in the oidc
adapter holds both lists. It has two methods:

- `needs_strict_decode(&self) -> bool`: true when either list is not empty.
- `refusal(&self, members: &Map<String, Value>) -> Option<RefusalDetail<'_>>`: step 6b, then step
  6c. It returns `Claim(name)` for the first marker that is present, else `MissingClaim(name)` for
  the first required name that is missing, else `None`.

`authenticate` calls these two methods, so it gets one branch for both steps, not two. The rules
can be unit-tested on a plain map, without JWT signing. The request path does not read
`IssuerConfig` again.

The check is in `OidcAuthenticator::authenticate`, so it applies to both schemes: `Bearer` and,
since SMA-700, `Dpop` (`authenticate_token.rs:203` and `:317`). The API-key path is not a JWT path.
The gateway keeps no cache of introspection results.

### D4. The log line

The refusal uses the existing `log_refusal` with `TokenDefect::NotAnAccessToken`. A new
`RefusalDetail` variant `MissingClaim(&str)` carries the name. The marker text is
`missing claim <name>`, for example `missing claim jti`.

The `MissingClaim` arm has its own message text: "refused a bearer token: it does not carry a claim
that the issuer configuration requires". `NOT_AN_ACCESS_TOKEN_MESSAGE` ("a verified marker shows it
is not an access token") is wrong for the fail-closed case, where the refused tokens ARE access
tokens. With that text an operator can look for a client bug and miss the IdP upgrade. The defect
and the rate-limit key stay the same.

The name is a configured value, so the log never shows token material. The rate limit (one line for
each issuer and defect in 10 seconds) applies, and it is shared with the other `NotAnAccessToken`
refusals of the issuer. The level stays `info`, like every other token refusal. An ID token that a
client presents also gives this line, so `warn` would warn about client errors too (§ 11, Q-c).

No new metric (none exists for refusals today). No change to the HTTP or gRPC response: a 401 with
`invalid-token`, and the defect is not exposed.

Doc comments change with the code:

- `TokenDefect::NotAnAccessToken` (`paigasus-iam-core/src/authn.rs:209-211`) names the SMA-686
  `typ` check only. It gets the SMA-703 marker claims and the SMA-731 required claims.
- The `validator.rs` module doc (lines 3-16, "Four refusals are logged") and the `log_refusal`
  doc get the new refusal.

### D5. The chart

- New optional value `oidc.accessTokenRequiredClaims`, default `[]`. In
  `charts/paigasus/values.yaml` it goes AFTER `oidc.scopes`, not directly after
  `oidc.idTokenMarkerClaims`. The `env.sh` M8 row deletes the lines from `idTokenMarkerClaims:` to
  `scopes:` (`env.sh:792`). A new key in that range would be deleted too, and M8 would then test a
  chart with neither key. Its comment refers to `oidc.idTokenMarkerClaims`. `README.md` documents
  it next to `oidc.idTokenMarkerClaims`.
- `templates/backend-deployment.yaml` adds `,access_token_required_claims=[…]` to the one issuer
  entry of `IAM_AUTHN__ISSUERS` only when the list is not empty. Each item is quoted with `%q`.
  With the default, the rendered value is byte-identical to today. An extra comment line renders
  only when the value is set, like the SMA-703 line.
- The chart reads the value with `dig`, because `--reuse-values` from an older release leaves it
  nil. A nil value is an empty list.
- The chart copies the boot rules of D2, because a boot failure stops IAM (one replica,
  `maxSurge: 0`). It fails the render, with a message that names `oidc.accessTokenRequiredClaims`,
  when:
  - the value is not a list;
  - an item is not a string, or is empty;
  - an item has a character outside printable ASCII, or a space, `"` or `\`;
  - an item is `iss`, `sub`, `aud` or `exp`;
  - the list has a duplicate;
  - an item is also in `oidc.idTokenMarkerClaims`.
- The first five rules are shared with `oidc.idTokenMarkerClaims`. The SMA-703 template
  `paigasus.validateIdTokenMarkerClaims` becomes one parameterized template, called once for each
  list. Its argument is a `dict` with: the root context, the `dig` key (`idTokenMarkerClaims`),
  the display path (`oidc.idTokenMarkerClaims`), the example text for the "not a list" message
  (`["at_hash", "azp"]` or `["jti"]`, see `_iam-backend.tpl:145`), and the reserved-name text. The
  SMA-703 messages do not change, so the existing `refusals.sh` rows keep passing.
- The overlap rule is a separate check. It runs after both lists pass their own checks, and it
  treats a nil value as `[]`.
- All of it stays in `templates/_iam-backend.tpl`, not in `_helpers.tpl` (SMA-703 D5).
- A change of the value changes the IAM pod template, so IAM restarts with a short gap. The
  runbook § 5 restart table gets a row.
- No new required value. So `helm_render.py` `STUB_VALUES`, the chart scripts' required-value
  lists and `ci/kind/values/a.yaml` do not change.

### D6. The runbook

`docs/ops/RUNBOOK-chart.md`:

- The values table gets a row for `oidc.accessTokenRequiredClaims`.
- § 5 restart table gets a row.
- § 6 gets a paragraph after the SMA-703 paragraph: "**IAM refuses a token that does not carry a
  claim that you configure (SMA-731).**" It states:
  - Use a name only when the IdP puts it in every access token and in no ID token. Decode one
    access token for each grant type in use, and one ID token, before you set it.
  - The check fails closed. If the IdP stops putting the claim in its access token, IAM refuses
    every request of that issuer with a 401 `invalid-token`. The IAM log has the line
    "refused a bearer token: it does not carry a claim that the issuer configuration requires"
    with the marker `missing claim <name>`, at `info` level. The log is rate-limited, so one line
    can stand for many refusals. If the IAM log level is above `info`, the line does not show.
    What the console shows a user in this case is not measured; the runbook says so.
  - The boot `info` line shows the setting. A missing boot line means the setting is not on.
  - The chart refusals of D5, and the overlap rule.
  - A change restarts IAM. To remove the value, use `[]` or `--set-json`, not `{}` (SMA-703 § 8).
- The Zitadel bullet adds `oidc.accessTokenRequiredClaims: ["jti"]`, next to the marker claims,
  labelled "measured with Login v1: code flow, refresh grant, client-credentials grant". The
  "decode before you set" list (`RUNBOOK-chart.md:506-507`) adds the JWT-profile grant, which is
  not measured and is a usual Zitadel machine flow. It says why both settings: the marker claims refuse a token that has an ID-token claim, and the required
  claim refuses a token that lacks an access-token claim. If a future Zitadel version drops
  `at_hash` and `azp` from its ID token, the required claim still refuses the ID token. If it
  drops `jti` from the access token, every login fails with `missing claim jti`; then remove the
  value, and check the new token shapes before you set a new name.
- The Keycloak bullet records the measurement of D8:
  - If the Keycloak ID token has no `jti` and the access token has `jti`: give the recipe
    `["jti"]` as a second defence next to the `typ` check, labelled with the Keycloak version.
  - Otherwise: say that `jti` does not separate the Keycloak tokens, so the setting does not help
    for Keycloak. The `typ` check (SMA-686) stays the Keycloak defence.
- The other IdPs stay "not measured".

`iam.toml.example` gets the commented key next to `id_token_marker_claims`, with the Zitadel recipe.
`rs/crates/services/paigasus-iam/CHANGELOG.md` gets entries under `## [Unreleased]` / `### Added`,
in the SMA-703 style.

### D7. No Notion ADR

No Notion ADR, like SMA-703 D8. The change adds one opt-in refusal and one optional config field.

### D8. The Keycloak measurement

`tests/keycloak_e2e.rs` already decodes the ID token and the access token of the password grant
(`keycloak_e2e.rs:152-156`, Keycloak `26.4`), and gets a DPoP-bound access token (`:196-201`). The
config turns DPoP on (`:321-325`). The plan's first task measures `jti` on five tokens:

- the ID token and the access token of the password grant;
- the DPoP-bound access token;
- the ID token and the access token of one refresh grant (a new request in the test, with the
  `refresh_token` of the password grant).

It adds a temporary print of `has_claim(…, "jti")` for each token, runs the test, and records the
result as F4 in § 3. The test then asserts the measured result, so a Keycloak image bump that
changes it fails the test. The temporary print does not stay in the code.

The Keycloak ID token carries `typ: ID` (`keycloak_e2e.rs:154`), so step 6 refuses it before step
6c. Thus a Keycloak ID-token refusal does NOT prove step 6c, and the test does not claim it. If the
split holds on all measured tokens, the test runs IAM with `["jti"]` and checks that the password
access token, the refreshed access token and the DPoP-bound token (on the `Dpop` scheme, with a
valid proof) still pass. The runbook recipe is then labelled with the version and the measured
grants. If the split does not hold, the test asserts the measured presence only.

### D9. Acceptance criteria mapping

| SMA-731 criterion | Where |
|---|---|
| A spec with a decision on the defect and on the name of the setting | D1, D4 |
| The default (empty list) keeps the current behaviour for every IdP | D3 (decode path), T6, T21 |
| `zitadel_e2e.rs`: an ID token without `jti` is refused with `["jti"]`; access tokens pass | T19 |
| `RUNBOOK-chart.md` § 6 gives the Zitadel recipe and what an operator sees | D6 |

## 5. Tests

Unit tests in `validator.rs` (`mod tests`). The fixtures are the SMA-703 Zitadel fixtures (claim
names and value shapes of M1, M4, M5b/M5c, times relative to `Utc::now()`). The issuer is configured
with `access_token_required_claims = ["jti"]` and an empty marker list, unless the test says
otherwise.

- T1. The M1, M4 and M5b ID tokens are refused as `NotAnAccessToken`.
- T2. The M1, M4 and M5b access tokens are accepted.
- T3. A token with `jti: null` is refused. A token with `jti` set to `""`, `0`, `false`, `{}` or
  `[]` is accepted.
- T4. With `["jti", "nbf"]`, a token with `jti` and no `nbf` is refused, and the log names `nbf`.
- T5. Name matching is case-sensitive: with `["jti"]`, a token with `JTI` only is refused.
- T6. With both lists empty, the M1 ID token is accepted (pins G2).
- T7. An expired M1 ID token reports `Expired`. An M1 ID token with a wrong `aud` reports
  `AudienceMismatch`.
- T8. A Keycloak `typ: ID` token without `jti` reports the marker `ID` (step 6 runs first).
- T9. With markers `["at_hash", "azp"]` and required `["jti"]`, the M1 ID token reports
  `claim at_hash` (step 6b runs first), and the M1 access token is accepted.
- T10. A token without `jti` and with a `cnf` member reports `NotAnAccessToken`, not
  `SenderConstrained` (step 6c runs before step 7).
- T11. With required `["jti"]` and an empty marker list, a token with a duplicate `jti` member (one
  `null`, one string, in both orders) is refused as `Malformed`.
- T12. Two issuers, one with `["jti"]` and one with an empty list. A token without `jti` from the
  first is refused. The same token shape from the second is accepted.
- T13. The refusal log line names the issuer and `missing claim jti`. It does not contain a claim
  value, the subject or the email. Repeated refusals log one line.
- T14. The boot line names the issuer and the required names, and is absent for an empty list.
- T14a. On the `Dpop` scheme, a correctly bound token (`cnf.jkt` and a valid proof) without `jti`
  is refused as `NotAnAccessToken`. The same token with `jti` passes.
- T14b. `ClaimRules` unit tests on a plain map: `needs_strict_decode` for the four empty/non-empty
  combinations, and `refusal` for marker-first order, the `null` rule and case-sensitivity.

Config tests in `config.rs`:

- T15. Each name refused by D2 fails `validate`, with the issuer and the field in the message. This
  includes the overlap rule, with both field names in the message.
- T16. The SMA-703 test `validate_refuses_bad_id_token_marker_claims` keeps passing unchanged
  after the shared function (D2).
- T17. A figment env value in the chart form, with `id_token_marker_claims` and
  `access_token_required_claims` both set, parses into both lists. A value without the key parses
  as an empty list.

End-to-end tests:

- T18. `zitadel_e2e.rs`: every access token (human, refreshed, machine) has `jti`, and no ID token
  has it. This pins F1 for `v4.15.3`.
- T19. `zitadel_e2e.rs`: with required `["jti"]` and an EMPTY marker list, IAM refuses the human,
  the refreshed and the machine ID token as `NotAnAccessToken`. The empty marker list proves that
  the required claim refuses the ID tokens by itself. The human and the refreshed access token
  resolve to the human principal, and the machine access token passes the authenticator (then
  `MissingEmail`, as in SMA-703 § 12).
- T19a. `zitadel_e2e.rs`: one more `AppState` with the full recipe (markers `["at_hash", "azp"]`
  and required `["jti"]`). The three access tokens pass the authenticator. This is the setup that
  the runbook tells operators to use.
- T20. `keycloak_e2e.rs`: the measured `jti` presence on the five tokens of D8, and, if the split
  holds, the pass of the three access tokens with `["jti"]`.
- T21. The existing SMA-703 assertions and the empty-list control of `zitadel_e2e.rs` do not
  change.

Mutation proof (as SMA-703 § 12): the plan disables step 6c (the `MissingClaim` branch of
`ClaimRules::refusal`) and checks that T1, T4, T14a and T19 fail. A deleted branch leaves dead
code, and the workspace sets `warnings = "deny"`, so the mutation must disable the branch in a form
that compiles (the plan uses `.filter(|_| false)`). The plan runs it with `--no-fail-fast` and
checks for test failures, not a compile error.

Chart rows:

- T22. `env.sh`: the default render is unchanged. With `["jti"]`, `IAM_AUTHN__ISSUERS` holds the
  quoted list. With both values set, it holds both lists. A render with the key absent
  (`--reuse-values`) succeeds. A change of the value restarts IAM only.
- T23. `refusals.sh`: one row for each D5 rule, each with a message that names
  `oidc.accessTokenRequiredClaims`. The existing `oidc.idTokenMarkerClaims` rows keep passing.

Existing `IssuerConfig { … }` literals (`validator.rs`, `tests/support/mod.rs`,
`tests/keycloak_e2e.rs`, `tests/zitadel_e2e.rs`, `tests/authn_private_ca.rs`) get the new field.

## 6. Error handling and failure modes

- The IdP stops putting a required claim in its access token. IAM refuses every login of that
  issuer, with the log line of D4. This fails closed, and it is the purpose of the setting.
- The operator configures a claim that the IdP's access token does not carry for one grant type
  (for example a machine flow). IAM refuses that grant type only. The runbook tells the operator to
  decode an access token for each grant type first.
- The IdP starts to put the required claim in its ID token. The required claim then gives no
  protection, and nothing warns. For Zitadel the marker claims still apply. Both settings fail
  open at the same time only if Zitadel adds `jti` to its ID token AND drops both `at_hash` and
  `azp` from it.
- A typo in a raw `iam.toml` key gives an empty list, because `IssuerConfig` ignores unknown fields.
  The missing boot line is the only sign (as SMA-703 D2).
- An invalid value in the chart fails the render, not the IAM boot (D5).

## 7. Security notes

- The check reads only the verified payload. It adds no unverified read.
- A duplicate top-level key is `Malformed` whenever either list is set (D3).
- The log carries configured names, never token values.
- The list adds a refusal only. It cannot make IAM accept a token that it refuses today.

## 8. Rollout

No migration. The default is empty. An operator turns the check on with one chart value, which
restarts IAM once. A rollback is the removal of the value, with one more restart.

## 9. Open questions

None that block. Sven decided the defect (`NotAnAccessToken`) and the Keycloak scope (measure) on
2026-10-06. § 11 lists three challenger questions that this spec answers with a default.

## 10. Follow-ups

- Measure what the console shows a user when IAM refuses every access token (§ 11, Q-a). This also
  applies to SMA-703.

## 11. Spec challenge (2026-10-06)

Verdict: APPROVE WITH CHANGES. No BLOCKER. The coordinator checked the evidence of the MAJOR
finding and of the chart findings against the files before folding them in.

| Finding | Severity | Outcome |
|---|---|---|
| The Keycloak branch cannot prove step 6c, and one grant is too narrow for a recipe | MAJOR | Folded in: D8 measures four tokens (password, DPoP, refresh); no claim that the ID-token refusal proves 6c; T20. |
| No unit test on the `Dpop` scheme | MINOR | Folded in: D3, T14a. |
| The "not a list" message has a fixed example | MINOR | Folded in: D5 `dict` argument with the example text; overlap check after both lists, nil as `[]`. |
| `env.sh` M8 would delete the new key too | MINOR | Folded in: D5 puts the key after `oidc.scopes`. |
| The log message is wrong for the fail-closed case | MINOR | Folded in: D4 own text for `MissingClaim`; D6. |
| The `NotAnAccessToken` doc is out of date | MINOR | Folded in: D4 doc-comment list. |
| The `jti` facts are weaker than F1 says | MINOR | Folded in: F1, F2, D6 label and the JWT-profile grant. |
| No mutation proof | MINOR | Folded in: § 5 mutation proof. |
| `authenticate` grows ad-hoc branches | MINOR | Folded in: D3 `ClaimRules`, T14b; `RESERVED_CLAIM_NAMES` (D2). |
| No e2e test of the full recipe | MINOR | Folded in: T19a. |
| Q-a. What does the console show in the fail-closed case? | QUESTION | Not measured. The runbook says so (D6). Follow-up in § 10. |
| Q-b. A report-only mode for the first rollout? | QUESTION | Rejected for now: a rollback is the removal of one value, and the runbook requires a decode of each grant type first. A report-only mode adds a second setting and a state that protects nothing. Sven can override. |
| Q-c. Log the `MissingClaim` refusal at `warn`? | QUESTION | Rejected for now: every other token refusal is `info`, and a client that presents an ID token gives the same line. D6 tells the operator about the log level. Sven can override. |
