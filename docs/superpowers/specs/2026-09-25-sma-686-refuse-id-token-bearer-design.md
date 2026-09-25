# SMA-686: IAM refuses a Keycloak ID token or logout token as a bearer token

- Linear: SMA-686 (residual R3 of SMA-678, `2026-09-24-sma-678-oidc-audience-design.md` § 5)
- Status: revised after the spec challenge (2026-09-25). The challenge is in § 9.
- Path: architectural (a change to what IAM accepts from the IdP), written for the
  feature-pipeline challenge step
- Evidence: `2026-09-25-sma-686-measurements.md`

## 1. Problem

IAM validates a bearer token in `OidcAuthenticator::authenticate`
(`rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs:171-219`). The validator checks
the length, the algorithm, the `kid`, the issuer, the signature, `aud`, `exp` and `nbf`. It does
not check the token type.

The chart renders the accepted audience as `oidc.audience | default oidc.clientId`
(`charts/paigasus/templates/backend-deployment.yaml:110-114`). An OIDC ID token has `aud` = the
client id. So in the default configuration, IAM accepts an ID token as a bearer token. The
measurements (§ 2) found a second case: IAM also accepts the Keycloak back-channel logout token.

There is one production `Authenticator` (`validator.rs:170`, wired at `adapters/http/mod.rs:147-151`).
The HTTP middleware, the gRPC `AuthEnforce`, both introspect endpoints and the `IsAuthorized`
self-query all reach it through `AuthenticateToken::resolve`
(`src/application/authenticate_token.rs:110`). The gateway does not validate a token. It forwards
the bearer to IAM (`rs/crates/services/paigasus-gateway/src/adapters/http/auth.rs:72-146`). So the
validator is the only place that can refuse the token. The shipped console sends the access token
(`ts/apps/gateway-console/lib/chat-route.ts:204`). The risk is a different client that sends an
ID token, or a party that received an ID token or a logout token and must not call IAM.

## 2. Measured facts

All from `2026-09-25-sma-686-measurements.md`: Keycloak 26.4 (26.4.7,
`sha256:9409c59b…`) and Dex v2.45.1 (`sha256:8499afd6…`), in scratch containers.

Keycloak, realm fixtures A (`keycloak_e2e.rs`) and B (the kind realm):

| Token | Header `typ` | Payload `typ` | `at_hash` | `c_hash` | `nonce` |
|---|---|---|---|---|---|
| Access, every case: password, code flow, refresh grant, RFC 9068 attribute, lightweight access token | `JWT` (`at+jwt` with the RFC 9068 attribute) | `Bearer` | never | never | never |
| ID, every case | `JWT` | `ID` | always | never | only when the request sent one |
| Back-channel logout (RS256, has `exp`, `aud` = client id) | `logout+jwt` | `Logout` | no | no | no |

Dex (password, code flow, refresh grant):

| Token | Header `typ` | Payload `typ` | `at_hash` | `c_hash` | `nonce` |
|---|---|---|---|---|---|
| Access (RS256, kid, `aud` = client id) | none | none | **always** | never | when the request sent one |
| ID | none | none | always | code flow only | when the request sent one |

Conclusions:

- In Keycloak, the payload `typ` claim separates the tokens in every measured case.
- In Dex, the access token carries `at_hash`. An `at_hash` marker would refuse every Dex access
  token. A Dex ID token differs from its access token only by `c_hash`, and only in the code flow.
- In the kind realm with the chart default, the access token and the ID token both have
  `aud: paigasus-console`. The kind job has the gap today.

## 3. Decisions

| # | Decision | Reason |
|---|---|---|
| D1 | The fix adds **no new requirement** on the IdP. It must not refuse an access token from Keycloak or Dex in any configuration measured in § 2. | Sven chose this on 2026-09-25. The evidence covers Keycloak 26.4 and Dex v2.45.1 only. For other IdPs, the claim is not proven; see R1. |
| D2 | IAM refuses a token whose payload `typ` claim is a string equal to `ID` or `Logout`, ASCII case-insensitive. This is the only marker. | Sven chose "Keycloak `typ` only" on 2026-09-25, after the Dex measurement. No measured access token has either value. The rule is specific to Keycloak and to IdPs that copy its `typ` claim. The case-insensitive compare protects against a variant spelling. |
| D3 | `at_hash`, `c_hash` and `nonce` are **not** markers. | Every Dex access token has `at_hash` (§ 2), so that marker breaks D1. `c_hash` is safe on Keycloak and Dex, but it adds little cover. From memory (not measured), authentik puts `c_hash` into the access token in its hybrid flow. From memory (not measured), Keycloak 25+ has a "Nonce backwards compatible" mapper that copies `nonce` into the access token. Dex access tokens carry `nonce` (measured). |
| D4 | The header `typ` is **not** read. | Keycloak sends `JWT` on both tokens by default (§ 2). An `at+jwt` header is not required (D1). |
| D5 | The check runs **after** `jsonwebtoken::decode`, on claims that have a verified signature. | The validator reads nothing from an unverified payload except `iss` (`validator.rs:92-109`). `jsonwebtoken` 11.1.0 deserializes the claims before it validates them (`decoding.rs:286-288`), and its validation reads only `exp`, `nbf`, `sub`, `iss` and `aud`. So an expired ID token reports `Expired`, not the new defect. This order is intentional. |
| D6 | `typ` is read as `Option<serde_json::Value>`. A JSON `null` or a non-string value is not a marker and is not `Malformed`. | A `String` type makes a non-string `typ` a serde failure, and `map_jwt_error` maps that to `Malformed`. That is a new refusal of a token that IAM accepts today, which breaks D1. |
| D7 | A new variant `TokenDefect::NotAnAccessToken` in `paigasus-iam-core`. | The HTTP funnel (`adapters/http/authn.rs:41`) and the gRPC funnel (`adapters/grpc/convert.rs:139-141`) map every `InvalidToken(_)` to the same 401 / `Unauthenticated` with reason `invalid-token`. So the wire response does not change. No code matches exhaustively on `TokenDefect`. `paigasus-iam-core` has `publish = false`, so the new variant is not a semver break. |
| D8 | The validator logs the refusal once, at `info`, with the issuer and the matched value as a static string (`"ID"` or `"Logout"`). It logs no other claim and no token material. | Today nothing logs an `InvalidToken` defect: the two funnels log only `Backend`, and the middleware and the validator do not log. An operator who gets 401s must be able to see why. The level is `info` because the default `log_level` is `info`. Only a correctly signed token from a configured issuer reaches this line, so an unauthenticated caller cannot flood the log with it. |
| D9 | The check applies to every configured issuer and every audience configuration. There is no switch. | No measured access token has a marker (§ 2), so an operator who set `oidc.audience` (SMA-678) sees no change. A switch adds configuration with no known use (YAGNI). |
| D10 | No Notion ADR. | Sven chose this on 2026-09-25. The change hardens one check and changes no interface. The decision is recorded here and in the validator doc comment. |
| D11 | The validator logs an `AudienceMismatch` once, at `info`, with the issuer and the CONFIGURED audiences. It logs no token claim. No other defect is logged. | Sven chose on 2026-09-25 to fold R2 into this PR. The runbook and `ci/kind/README.md` told operators this line exists. `jsonwebtoken` 11.1.0 verifies the signature (`decoding.rs:284`) before `aud` (`decoding.rs:288`), so only a signed token reaches it. `Expired` stays unlogged: every stale client token would write a line. |

## 4. Change

### 4.1 `paigasus-iam-core`

`rs/crates/libs/paigasus-iam-core/src/authn.rs`: add `NotAnAccessToken` to `TokenDefect`, with a
doc comment: the payload `typ` claim marks a Keycloak ID token or logout token (SMA-686).

### 4.2 The validator

`rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs`:

- `WireClaims` gets one field: `typ: Option<serde_json::Value>`.
- A new private function `check_access_token_type(issuer: &Issuer, claims: &WireClaims) ->
  Result<(), AuthnError>` applies D2 and D6. It returns `Err(InvalidToken(NotAnAccessToken))`
  when the marker is present, and it writes the D8 log line.
- `authenticate` calls it right after `decode` (new step 6), before it reads `exp`.
- The module doc comment (lines 3-7) lists the new step. It states that this one rule is specific
  to Keycloak, and that the rest of the pipeline stays provider-agnostic.

### 4.3 The runbook

`docs/ops/RUNBOOK-chart.md` § 6:

- Item 1 (Audience) currently says to set `oidc.audience` "only when you cannot make the IdP put
  the client id into `aud`". Change it to name two cases: that one, and an IdP whose ID token has
  no Keycloak `typ` claim, for which the new paragraph tells how to choose the value.
  (Corrected after the final review: the first wording said "the same `aud` as the access
  token", which is exactly the case where no value exists.)
- A new paragraph after the four items:
  - IAM refuses a bearer token whose `typ` claim is `ID` or `Logout`. Keycloak sets these values on
    its ID token and its back-channel logout token. The IAM log shows the refusal at `info`, with the issuer and the matched value.
  - This adds no requirement on the IdP. No Keycloak or Dex access token measured for SMA-686
    carries these values. A hardcoded-claim mapper that sets `typ` on an access token is not
    supported.
  - An IdP whose ID token has no `typ` claim is not protected by this check (Dex is an example).
    Decode a real ID token and a real access token. If the ID token has no `typ: ID`, choose an
    audience that is in the access token's `aud` and absent from the ID token's `aud`, and set
    `oidc.audience` to it. If the IdP cannot do this, IAM accepts that IdP's ID token.

No chart template or value changes. The comment at `backend-deployment.yaml:105-106` stays
correct.

### 4.4 The CHANGELOG

`rs/crates/services/paigasus-iam/CHANGELOG.md` (maintained by hand): under `## [Unreleased]`, a
`### Security` entry: IAM refuses a bearer token whose `typ` claim is `ID` or `Logout` (SMA-686).

## 5. Tests

### 5.1 Unit tests (`validator.rs`)

Each test signs a real ES256 token with the existing `es256_keypair()` helper and a valid
issuer, audience and `exp`, so only `typ` differs from a token that passes.

Refused with `NotAnAccessToken`:

1. `typ: "ID"`
2. `typ: "id"` (case)
3. `typ: "Logout"`
4. `typ: "LOGOUT"` (case)

Accepted:

5. `typ: "Bearer"`
6. `typ: "DPoP"` (a Keycloak DPoP-bound access token; tests that D2 is a denylist)
7. no `typ` claim, but `at_hash`, `c_hash` and `nonce` present (the Dex access-token shape;
   pins D3)
8. `typ: null` (pins D6)
9. `typ: 1` (pins D6)
10. `typ: " ID"` with a leading space (exact compare after case folding, no trimming)

Order:

11. `typ: "ID"` with an `exp` beyond the leeway reports `Expired` (pins D5)

Log:

12. a refused token writes one `info` line that has the issuer and `"ID"`, and has no `sub`,
    `email` or token material. Capture it with a test subscriber, as the `LogBuffer` pattern in
    `paigasus-gateway/src/adapters/http/auth.rs:798-828` does.

Wire mapping:

13. `adapters/http/authn.rs`: add `NotAnAccessToken` to the defect list of the existing test at
    line 140, so the 401 mapping is pinned for the new variant.

### 5.2 Integration test (`keycloak_e2e.rs`)

- `keycloak_config` audiences change from `["paigasus"]` to `["paigasus", "paigasus-cli"]`. This
  matches the chart default, where the accepted audience is the client id. Without it, the
  audience check refuses the ID token first, and the test proves nothing about the new check.
  Update the comments at `keycloak_e2e.rs:150-152` and `:190-191` to match.
- The test reads `token_body["id_token"]`. It asserts that the ID token payload has `typ: "ID"`,
  so a Keycloak change that removes the marker reds the test.
- It asserts that the access-token payload has `typ: "Bearer"` and `aud` containing `paigasus`.
  This checks D1 for Keycloak in CI directly.
- It calls `state.authn.resolve(&id_token, Provisioning::Enabled)` directly and asserts
  `Err(AuthnError::InvalidToken(TokenDefect::NotAnAccessToken))`. `state.authn` is the
  `AuthenticateToken` use case; `tests/support/mod.rs:635-637` calls it the same way. This proves the cause of the refusal. A 401 alone does not, because
  every defect gives the same 401.
- It sends the ID token as the `Authorization: Bearer` of `POST /v1/organizations` (the middleware
  path) and as the body `token` of `POST /v1/authn/introspect` (the exempt path,
  `adapters/http/authn.rs:70-79`). It asserts 401 with the error code `invalid-token` for both.
  This pins the wire contract.
- The existing access-token assertions (201 on the org create, 200 on both introspects) stay as
  they are.

### 5.3 Proof that the tests bite

In a scratch edit, replace `check_access_token_type(&issuer, &token_data.claims)?;` with
`let _ = check_access_token_type(&issuer, &token_data.claims);`. This compiles under
`warnings = "deny"` (`rs/Cargo.toml:273-274`), because the function and the field stay used.
Then:

- run the unit tests and the integration test with `--no-fail-fast`, and run the integration
  test with `PAIGASUS_REQUIRE_DOCKER=1`, so a run without Docker cannot pass as a skip;
- unit tests 1-4, 12 and the integration test must go red;
- tests 5-11 and 13 must stay green.

Restore the call by an edit, not by `git checkout` (the fix is not yet committed).

### 5.4 Acceptance criteria

| AC | Covered by |
|---|---|
| 1. IAM refuses a valid ID token from the configured issuer | § 5.2 (the real Keycloak ID token, with the defect asserted), § 5.1 tests 1-4 |
| 2. IAM accepts a valid access token from the kind-job Keycloak realm | § 2 (the kind realm access token has no marker, measured for the code flow and the refresh grant); § 5.2 for realm A; and a manual run of the kind job on the PR branch (§ 7) |
| 3. The runbook states the requirement on the IdP, if there is one | § 4.3: there is no new requirement for Keycloak or Dex, and the runbook says what an operator with a different IdP must check |

## 6. Rollout

- No cache keeps a token verdict. The gateway has none (`paigasus-gateway/src/adapters/http/auth.rs:23`).
  The IAM caches are keyed by issuer, key id or principal, not by token.
- The change applies on the first request after the IAM rollout. There is no migration.
- A client that sends an ID token or a logout token gets 401 at once after the rollout.
- Rollback: redeploy the previous IAM image.

## 7. Verification before merge

The kind journeys (`.github/workflows/chart.yml`) do not run on this PR. The `pull_request` path
filter (`chart.yml:27-43`) has no `rs/**`, and the job is not a required check. So the plan must
start the workflow manually on the PR branch (`gh workflow run chart.yml --ref <branch>`) and
record the run URL in the PR body. That run is the AC2 proof for the kind realm.

## 8. Residuals

- **R1: IdPs without a Keycloak `typ` claim are not protected.** This includes Dex (measured) and
  every IdP not measured here (Okta, Entra ID, Auth0, Zitadel, authentik, Cognito). In the default
  configuration, their ID token still passes when its `aud` contains the client id. The runbook
  tells the operator to use `oidc.audience`. For Dex, the access token and the ID token have the
  same `aud`, so that remedy does not work either.
- **R2: folded in (D11).** The runbook and `ci/kind/README.md` said a wrong audience shows in the IAM log, and no code logged it. D11 adds the line.
- **R3: Other Keycloak token types.** The refresh token is HS512, so the algorithm allowlist
  (`validator.rs:26`) already refuses it. No other Keycloak JWT type was measured.

## 9. Spec challenge

Challenger: `feature-factory:spec-challenger` (Opus), 2026-09-25. Verdict: NEEDS REWORK.

| Finding | Severity | Action |
|---|---|---|
| D1 not proven outside Keycloak; `at_hash` probably refuses every Dex access token | BLOCKER | Measured Dex v2.45.1: confirmed. Sven chose "Keycloak `typ` only". D1, D2, D3 rewritten. |
| Nothing logs the refusal; the runbook log claim is false | MAJOR | Folded in: D8 adds one `info` line and test 12. The same false claim for `aud` was R2, later folded in as D11. |
| The integration test cannot show which check caused the 401 | MAJOR | Folded in: § 5.2 asserts the defect through `state.authn`. |
| The kind journeys do not run on this PR, so AC2 is not covered | MAJOR | Folded in: § 7, a manual `chart.yml` run. |
| The § 5.3 mutation does not compile under `warnings = "deny"` | MAJOR | Folded in: the `let _ =` mutation. |
| The runbook text contradicts item 1 and is not precise | MINOR | Folded in: § 4.3. |
| The logout token costs almost nothing to include | MINOR | Measured: it has `exp` and IAM accepts it today. Folded in: `Logout` in D2. |
| No refresh-grant access token in § 2 | MINOR | Measured for Keycloak and Dex: no `typ` marker. Folded in. |
| The evidence is not in the repo | MINOR | Folded in: `2026-09-25-sma-686-measurements.md`. |
| § 5.2 details (stale comments, access-token assertions, wrong citation) | MINOR | Folded in. |
| Unit-test gaps (`c_hash: null`, `DPoP`, wire mapping) | MINOR | Folded in as tests 6, 7 and 13. `c_hash` is no longer read, so test 7 covers it. |
| No rollout note and no CHANGELOG entry | MINOR | Folded in: § 6 and § 4.4. |
| The module doc says "provider-agnostic" | MINOR | Folded in: § 4.2. |
| Does this need a Notion ADR? | QUESTION | Sven: no (D10). |
| At which level to log? | QUESTION | `info` (D8). |
