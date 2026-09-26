# SMA-690: IAM refuses a sender-constrained access token

- Linear: SMA-690 (residual R4 of SMA-686, `2026-09-25-sma-686-refuse-id-token-bearer-design.md`
  § 8; SMA-686 code review finding 4)
- Status: approved at GATE 1 (2026-09-26). The spec challenge is in § 9.
- Path: architectural (a change to what IAM accepts from a client)
- Evidence: `2026-09-26-sma-690-measurements.md`

## 1. Problem

IAM validates a bearer token in `OidcAuthenticator::authenticate`
(`rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs:300-363`). It does not read the
`cnf` claim, and it does not check a DPoP proof. A DPoP-bound access token (RFC 9449) carries
`cnf.jkt`. An mTLS-bound access token (RFC 8705) carries `cnf.x5t#S256`. IAM accepts both as a
plain `Authorization: Bearer` token, so the sender constraint is lost.

The purpose of a sender-constrained token is that a thief without the client's private key
cannot use it. Today a stolen DPoP-bound token (from a log or a proxy) works against IAM and the
gateway.

SMA-686 made the payload `typ: DPoP` an accepted, tested case (`accepts_non_marker_typ_values`,
`validator.rs:784-799`), because its decision D1 added no new refusal of an access token that IAM
accepted. This spec reverses that one case.

The request paths (read in the code on `main` at `2346451d`, and checked again by the spec
challenge):

- IAM HTTP: `require_bearer` (`adapters/http/auth_middleware.rs:51-55`) and the exempt
  `POST /v1/authn/introspect` (`adapters/http/authn.rs:88`).
- IAM gRPC: `AuthEnforce` (`adapters/grpc/authn.rs:217-221`) and the exempt
  `AuthnService.Introspect` (`adapters/grpc/authn.rs:60`).
- All four call `AuthenticateToken::resolve`, which calls the one production `Authenticator`
  (`application/authenticate_token.rs:110`). The bearer parser `bearer_from_headers`
  (`adapters/auth.rs:33-44`) accepts only the scheme `Bearer`. No code reads a `DPoP` header.
- The API-key path needs the configured key prefix and never falls back to OIDC.
- The gateway does not validate a JWT and has no cache. It forwards the raw bearer to IAM over gRPC
  (`paigasus-gateway/src/adapters/http/auth.rs:72-146`, `:250-315`).
- No IAM cache keeps a token verdict. The JWKS cache and the authz decision cache hold keys and
  decisions.

So the validator is the one place that can refuse the token.

## 2. Measured facts

From `2026-09-26-sma-690-measurements.md`: Keycloak 26.4.7
(`sha256:9409c59bdfb65dbffa20b11e6f18b8abb9281d480c7ca402f51ed3d5977e6007`), in scratch
containers, over HTTP container to container, with the realm fixture of `keycloak_e2e.rs` (client
`paigasus-cli`, no DPoP attribute).

| Case | Result |
|---|---|
| M1: password grant, no DPoP proof | `token_type: Bearer`; payload `typ: Bearer`; header `typ: JWT`; no `cnf` |
| M2: the same grant with one DPoP proof header, no client change | `token_type: DPoP`; payload `typ: DPoP`; header `typ: JWT`; `cnf: {"jkt": …}`. The `jkt` equals the RFC 7638 thumbprint of the proof key. |
| M3: client attribute `dpop.bound.access.tokens: "true"` (set through the Admin REST API) | No proof: 400 `invalid_request`. With a proof: the M2 shape. |
| M4: refresh grant on the M2 refresh token | No proof: 400 `invalid_grant`. With a proof from the same key: the M2 shape, the same `jkt`. |
| M5: Keycloak userinfo with the M2 token as `Bearer`, no proof | 401 `invalid_token` |
| M6: the ID token issued with the M2 token | No `cnf` |

Conclusions:

- Keycloak binds a token when the client sends a DPoP proof to the token endpoint. The realm needs
  no DPoP setting. So every Keycloak realm has the gap.
- A bound session stays bound: a refresh without a proof fails (M4).
- Keycloak itself enforces the binding on its own endpoint (M5). IAM does not.
- A Keycloak DPoP-bound access token always has both `typ: DPoP` and `cnf.jkt` (M2, M3, M4).
- No access token that IAM accepts today in the SMA-686 measurements has `cnf` or `typ: DPoP`.
  The claim-name lists are at `2026-09-25-sma-686-measurements.md:16`, `:18`, `:20`, `:100`,
  `:149`, `:187` and `:251`. This covers the Keycloak Bearer token (realm A and the kind realm B,
  password, code flow and refresh grant) and every Dex v2.45.1 access token. The SMA-686 case B5
  (the Keycloak lightweight access token, no `aud`) is refused today by SMA-686 D13, and this spec
  does not change that.
- mTLS-bound tokens were not measured.

No shipped client uses DPoP. The binding happens at the token endpoint (M2). The console's OIDC
adapter calls `authorizationCodeGrant` (`ts/packages/paigasus-auth/src/adapters/oidc.ts:249`) and
`refreshTokenGrant` (`:286`) with no DPoP option. A search for `dpop` in the `ts/` and `py/`
sources finds nothing. The TS SDK and the Python SDK do not call the token endpoint. The realm
fixtures (`ci/kind/realm/paigasus-realm.json`, `tests/fixtures/keycloak-realm.json`,
`ts/packages/paigasus-auth/tests/e2e/keycloak-realm.json`) set no DPoP attribute.

## 3. Decisions

| # | Decision | Reason |
|---|---|---|
| D1 | IAM **refuses** a sender-constrained token. It does not check a DPoP proof in this issue. | Sven chose "Refuse cnf" on 2026-09-26. A proof check needs the `DPoP` scheme, proof validation, a `jti` replay store, and a gateway-to-IAM contract change that carries the proof, the method and the URL. It is a separate issue (D9). |
| D2 | Marker 1: the payload has a `cnf` member whose value is not JSON `null`. Any shape counts: `jkt`, `x5t#S256`, an unknown member, an empty object, or a non-object value. | RFC 7800 § 3.1 defines `cnf` as a confirmation of the key that the presenter must prove. IAM can prove none of them, so the check is fail-closed. A `null` value confirms no key, so it is not a marker; this matches SMA-686 D6. |
| D3 | Marker 2: the payload `typ` is a string equal to `DPoP`, ASCII case-insensitive. D2 is checked first, so a token with both markers reports `cnf`. | Keycloak sets `typ: DPoP` on every DPoP-bound token (§ 2). It is a second guard for a token whose `cnf` a mapper removed. No access token that IAM accepts today has this value (§ 2). Sven approved both markers on 2026-09-26. |
| D4 | mTLS-bound tokens are refused by D2. IAM does not add an mTLS binding check, now or in D9. | IAM does not terminate the client's TLS connection (the ingress does), so it cannot see the client certificate. A binding check needs a trusted certificate header from the ingress. That is out of scope (R2). |
| D5 | The check runs after `jsonwebtoken::decode`, on claims with a verified signature, and after the SMA-686 token-type check. | The validator reads nothing from an unverified payload except `iss`. An expired bound token reports `Expired`. An ID token with a `cnf` member reports `NotAnAccessToken`. The order is fixed so that a test can pin it. |
| D6 | `cnf` is read as `Option<serde_json::Value>`. A wrong shape never becomes `Malformed`. | Same reason as SMA-686 D6: the refusal must name the true cause. serde maps a JSON `null` to `None`, so D2 needs no extra code for `null`. |
| D7 | A new variant `TokenDefect::SenderConstrained` in `paigasus-iam-core`. The wire response does not change: 401, code `invalid-token`, `WWW-Authenticate: Bearer error="invalid_token"`; gRPC `Unauthenticated` with the same reason. | Both funnels map every `InvalidToken(_)` to one response (`adapters/http/authn.rs:41`, `adapters/grpc/convert.rs:139-141`). So `error.proto` does not change. `paigasus-iam-core` has `publish = false`, so the variant is not a semver break. IAM does not send a `DPoP` challenge (RFC 9449 § 7.1), because it does not support DPoP. |
| D8 | The refusal has its own log line at `info`, with its own static message: "refused a bearer token: it is bound to a key, and IAM cannot check the binding". The line names the issuer and a static marker, `cnf` or `typ DPoP`. The SMA-686 line stays byte-identical. Both go through `log_refusal` and its rate limit (SMA-686 D14, D15). | An operator who gets 401s must see why, and must see the cause without reading the `marker` field. A separate message keeps the two existing SMA-686 log tests (`validator.rs:854`, `:986`) valid. Only a correctly signed token from a configured issuer reaches the line. |
| D9 | A follow-up Linear issue tracks the DPoP proof check (RFC 9449 § 4.3 and § 7) on the IAM HTTP and gRPC paths and through the gateway. On the `Bearer` scheme, that issue keeps both markers. On the `DPoP` scheme, it requires `cnf.jkt` and a proof whose key thumbprint matches it. | D1. D3's guard is still needed on the `Bearer` scheme after DPoP support. |
| D10 | No switch. The check applies to every configured issuer. | No shipped client uses DPoP (§ 2). A switch that accepts a bound token as a bearer token restores the gap. |
| D11 | No Notion ADR. Sven approved the spec with this decision at GATE 1 on 2026-09-26. | The change hardens one check and changes no interface, like SMA-686 D10. But it also records that IAM does not support DPoP, which is a protocol-support decision. SMA-686 D10 was Sven's choice for SMA-686 only. |

## 4. Change

### 4.1 `paigasus-iam-core`

`rs/crates/libs/paigasus-iam-core/src/authn.rs`: add `SenderConstrained` to `TokenDefect`, with a
doc comment: the token is bound to a key (a `cnf` claim or a Keycloak `typ: DPoP`), and IAM
cannot check the binding (SMA-690).

### 4.2 The validator

`rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs`:

- `WireClaims` gets one field: `cnf: Option<serde_json::Value>`. Its doc comment names `cnf` next
  to `typ` and `events` as an untyped value.
- A new constant `SENDER_CONSTRAINED_TYPES: [&str; 1] = ["DPoP"]`.
- A new private function `sender_constraint_marker(claims: &WireClaims) -> Option<&'static str>`.
  It returns `Some("cnf")` for D2, else `Some("typ DPoP")` for D3, else `None`.
- A new `RefusalDetail` variant, `Binding(&'static str)`, with its own `tracing::info!` call in
  `log_refusal` and the D8 message. The fields are `issuer`, `marker` and `suppressed`, as in the
  `Marker` arm. The `Marker` arm and its message do not change.
- `authenticate` calls `sender_constraint_marker` right after the SMA-686 check (step 6), as
  step 7. On `Some(marker)`, it calls
  `self.log_refusal(&issuer, TokenDefect::SenderConstrained, RefusalDetail::Binding(marker))` and
  returns `Err(invalid(TokenDefect::SenderConstrained))`.
- Doc text that becomes false and must change:
  - the module doc (lines 3-10): list step 7, and change "Two refusals are logged" (line 9) to
    three;
  - the `NON_ACCESS_TOKEN_TYPES` comment (line 36), "(or `DPoP`)": say that a Keycloak
    DPoP-bound token carries `DPoP` and that step 7 refuses it;
  - the `RefusalLog` doc (line 53): "`issuers × 2`" becomes "`issuers × 3`";
  - the `RefusalDetail::Marker` doc (line 94) stays; add a doc for `Binding`;
  - the `log_refusal` doc (lines 151-152), "only `NotAnAccessToken` and `AudienceMismatch`": add
    `SenderConstrained`;
  - the comment in `accepts_non_marker_typ_values` (line 785), "a denylist of two values": the
    `DPoP` case moves out of this test (§ 5.1 test 10).

### 4.3 The runbook

`docs/ops/RUNBOOK-chart.md` § 6, a new paragraph after the SMA-686 paragraph (after line 134), in
the SMA-686 style:

- **IAM refuses a sender-constrained token (SMA-690).** IAM refuses an access token that has a
  `cnf` claim with any value except `null`, or a `typ` claim of `DPoP` in any letter case. A
  DPoP-bound token (RFC 9449) and an mTLS-bound token (RFC 8705) have these claims. IAM cannot
  check the binding, so it does not accept such a token as a bearer token.
- Keycloak binds an access token when the client sends a `DPoP` header to the token endpoint. The
  client needs no DPoP setting for this. A client that calls Paigasus must not send a `DPoP` header
  to the token endpoint.
- Do not set the Keycloak client attribute `dpop.bound.access.tokens` on a client that calls
  Paigasus. A Keycloak client policy can also require DPoP; do not use one for such a client
  (not measured).
- A bound login stays bound when the client refreshes the token. A client that got a bound token
  must log in again without a `DPoP` header.
- The console does not send a `DPoP` header. The SDKs do not get tokens; they send the token that
  the caller gives them. If you use the SDKs, do not turn on DPoP in your own OIDC library.
- The IAM log shows the refusal at `info`: "it is bound to a key, and IAM cannot check the
  binding", with the issuer and the marker `cnf` or `typ DPoP`. The rate limit of the SMA-686
  paragraph applies.

No chart template or value changes.

### 4.4 The CHANGELOG

`rs/crates/services/paigasus-iam/CHANGELOG.md`, under `## [Unreleased]` / `### Security`: IAM
refuses a sender-constrained access token (a `cnf` claim, or a Keycloak `typ` of `DPoP`). Before,
IAM accepted a DPoP-bound or mTLS-bound token as a plain bearer token, so the sender constraint
did not protect it (SMA-690).

### 4.5 The follow-up issue

Before the PR opens, create a Linear issue (project "Paigasus Polyglot", milestone "IAM Gaps",
priority Low, label `area:iam`, related to SMA-690): "IAM checks a DPoP proof (RFC 9449)". Its
description states D9. Record its key in the PR body and in § 8 R1.

### 4.6 The measurement file

`2026-09-26-sma-690-measurements.md` had two self-contradictions (the location of the probe
scripts, and "`cnf` is `null`" in the M6 row). They are corrected in the spec commit.

## 5. Tests

### 5.1 Unit tests (`validator.rs`)

Each test signs a real ES256 token with the existing helpers (`es256_keypair()`, `claims_with`,
`authenticate_json`). Only the named claims differ from a token that passes.

Refused with `SenderConstrained`:

1. `cnf: {"jkt": "…"}` (the Keycloak DPoP shape, M2)
2. `cnf: {"x5t#S256": "…"}` (the RFC 8705 shape)
3. `cnf: {"jwk": {…}}` (another RFC 7800 member)
4. `cnf: {}`
5. `cnf: "x"` (a non-object; pins D6: not `Malformed`)
6. `typ: "DPoP"`, no `cnf` (D3)
7. `typ: "dpop"` (case)
8. `typ: "DPoP"` and `cnf: {"jkt": "…"}` (the full Keycloak shape)

Accepted:

9. `cnf: null` (D2)
10. The SMA-686 cases stay in `accepts_non_marker_typ_values` except `DPoP`: `Bearer`, the Dex
    shape, `typ: null`, `typ: 1`, `typ: " ID"`. Add `typ: " DPoP"` (a leading space: exact
    compare after case folding).

Order:

11. `typ: "ID"` with `cnf: {"jkt": "…"}` reports `NotAnAccessToken` (D5).
12. `cnf: {"jkt": "…"}` with an `exp` beyond the leeway reports `Expired` (D5).

Marker:

13. A direct test of `sender_constraint_marker`: `cnf` alone gives `Some("cnf")`; `cnf` and
    `typ: "DPoP"` give `Some("cnf")` (D3 order); `typ: "dpop"` alone gives `Some("typ DPoP")`;
    `cnf: null` gives `None`.

Log (filter on the D8 message text, not on the SMA-686 text):

14. A token with `cnf: {"jkt": "<value>"}` writes one `info` line with the issuer and the marker
    `cnf`, and with no `<value>`, no `sub`, no `email` and no token material.
15. A token with `typ: "dpop"` only writes one line with the marker `typ DPoP`. The token's own
    spelling `dpop` does not appear in the line.
16. A second refusal for the same issuer within 10 s writes no line (the rate limit covers the new
    defect).

The two existing SMA-686 log tests (`validator.rs:854`, `:986`) do not change and must stay green.

### 5.2 Wire mapping

17. `adapters/http/authn.rs`: add `TokenDefect::SenderConstrained` to the defect list of the
    existing test (lines 141-145), so the 401 mapping is pinned for the new variant.
18. `adapters/grpc/convert.rs`: add one row for it next to the `Malformed` row of the mapping
    table (line 842), so the `Unauthenticated` / `invalid-token` mapping is pinned too.

The e2e test (§ 5.3) does not call a gRPC endpoint. The gRPC proof is the `resolve` assertion (the
same use case that the gRPC paths call, § 1) plus test 18.

### 5.3 Integration test (`keycloak_e2e.rs`)

The test already runs Keycloak 26.4 and requests a password-grant token
(`keycloak_e2e.rs:120-137`). Its issuer is `https://127.0.0.1:{mapped}/realms/…`
(`keycloak_e2e.rs:93`).

- Assert that the existing access token has no `cnf` member. This checks AC 2 for Keycloak in CI.
- Request a second password-grant token with a `DPoP` header. The test makes a P-256 key with
  `p256` (a dev-dependency already) and signs the proof with `jsonwebtoken`: header
  `typ: "dpop+jwt"`, `alg: ES256`, `jwk` = the public key; payload `jti`, `htm: "POST"`,
  `htu` = `token_url` exactly as sent, `iat` = now.
  - Make `jti` with `rand` (a normal dependency). `Uuid::new_v4()` does not compile in these
    tests (`tests/support/mod.rs:615-618`).
  - `support::es256_keypair` is private (`tests/support/mod.rs:221`). A new helper in the test
    file needs no change in `support`. A new `pub` helper in `support` needs `#[allow(dead_code)]`,
    as at `support/mod.rs:147`, `:164` and `:175`, or every other test binary fails under
    `warnings = "deny"`.
- Assert on the new token: the response `token_type` is `DPoP`; the payload `typ` is `DPoP`;
  `cnf.jkt` equals the RFC 7638 SHA-256 thumbprint of the proof key. Build the thumbprint input as
  the literal canonical string `{"crv":"P-256","kty":"EC","x":"…","y":"…"}` and hash it with
  `sha2` (a dependency of the crate already). So a Keycloak change to the token shape reds this
  test.
- Call `state.authn.resolve(&dpop_token, Provisioning::Enabled)` and assert
  `Err(AuthnError::InvalidToken(TokenDefect::SenderConstrained))`. A 401 alone does not prove the
  cause.
- Send the bound token as the `Authorization: Bearer` of `POST /v1/organizations` and as the body
  `token` of `POST /v1/authn/introspect`. Assert 401 with the code `invalid-token` on both.
- The existing access-token assertions stay as they are.

Risks that the plan must handle on the first run:

- M2 ran over HTTP, container to container. The e2e runs over HTTPS on the host-mapped port.
  Keycloak compares the proof `htu` with the URL of the request that it received. The discovery
  `token_endpoint` is the same URL as `token_url`, so it is not a fallback. If Keycloak refuses the
  proof, record the error body and stop; do not guess a different `htu`.
- The proof `iat` uses the host clock. From memory (not measured), Keycloak accepts `iat` only in
  a window of a few seconds. On Docker Desktop for macOS, the VM clock can drift after the host
  sleeps, so the test can fail locally with a proof error. CI on Linux shares one kernel clock. If
  the local run fails with an `iat` error, record it; do not widen a Keycloak setting in the realm
  fixture to hide it.

### 5.4 Proof that the tests bite

Mutation A (the check). In a scratch edit, change the step-7 call to
`if let Some(marker) = sender_constraint_marker(&token_data.claims).filter(|_| false) {`. This
compiles under `warnings = "deny"`, because the function stays used. Run the unit tests and the
integration test with `--no-fail-fast`, and run the integration test with
`PAIGASUS_REQUIRE_DOCKER=1`:

- tests 1-8 and 14-16 must go red;
- the integration test must go red at the `SenderConstrained` defect assertion. Record the line
  that panicked. The two 401 assertions after it do not run, so this run does not prove them;
- tests 9-13, 17 and 18 must stay green. Tests 11 and 12 stay green because an earlier check
  refuses the token; they pin the order, not the new check. Test 13 calls the function directly.

Mutation B (the log). Restore mutation A. Then replace the step-7 `self.log_refusal(..)` call with
`let _ = RefusalDetail::Binding(marker);`. This compiles, because the `Binding` variant stays
constructed and `log_refusal` stays used by step 6. (Corrected while writing the plan:
`let _ = marker;` leaves `Binding` never constructed, which is a dead-code error.) Tests 14-16 must go red. Tests 1-13 must stay green.

Restore each mutation with an edit, not with `git checkout` (the fix is not yet committed). After
the last restore, run the whole suite once more.

### 5.5 Acceptance criteria

| AC | Covered by |
|---|---|
| 1. IAM does not accept a sender-constrained token without a check of its binding. A test proves this. | D1-D3; § 5.1 tests 1-8; § 5.3 with a real Keycloak DPoP-bound token and the defect asserted; § 5.4 mutation A |
| 2. IAM still accepts every access token that SMA-686 measured and that IAM accepts today (Keycloak 26.4 `Bearer`, Dex v2.45.1) | § 2 (no such token has `cnf` or `typ: DPoP`; case B5 is refused today by SMA-686 D13); § 5.1 test 10; § 5.3 (the Keycloak Bearer token has no `cnf` and still gets 201 and 200) |
| 3. The runbook (`docs/ops/RUNBOOK-chart.md` § 6) states the behavior | § 4.3 |

## 6. Rollout and effect on operators

- The change applies on the first request after the IAM rollout. No cache keeps a token verdict
  (§ 1). There is no migration.
- A client that sends a DPoP-bound or mTLS-bound token gets 401 at once. This is a new
  requirement on such a client: it must get a plain Bearer token for Paigasus, and a bound login
  must start again without a `DPoP` header (M4). No shipped client is affected (§ 2).
- A Keycloak client with the attribute `dpop.bound.access.tokens: "true"` cannot get a token that
  IAM accepts (M3). The runbook says this.
- Rollback: redeploy the previous IAM image.

## 7. Verification before merge

- `moon ci` with the full target list from the root `CLAUDE.md`.
- No manual kind run (`chart.yml`) is needed:
  - the kind Keycloak image has the measured digest (`ci/kind/manifests/keycloak.yaml:30`);
  - the kind realm B code-flow token and its refresh token have `typ: Bearer` and no `cnf`
    (`2026-09-25-sma-686-measurements.md:18`, `:251-252`);
  - the console passes no DPoP option (`oidc.ts:249`, `:286`).
- `chart.yml` runs on `push` to `main` for `rs/**` (`chart.yml:8-12`), so it runs after the merge.
  It is not a required check, so a red run there blocks nothing. Check that run after the merge.

## 8. Residuals

- **R1: no DPoP support.** A client that wants DPoP cannot use Paigasus. The follow-up issue SMA-700
  (§ 4.5) tracks the proof check.
- **R2: no mTLS binding.** IAM does not see the client certificate (D4). A future check needs the
  ingress to forward a verified certificate hash. No issue is planned.
- **R3: an IdP that binds a token without `cnf` or `typ: DPoP`.** Such a token passes. RFC 9449
  § 6 and RFC 8705 § 3 require `cnf` for a JWT access token, so a compliant IdP always sets it.
  Not measured outside Keycloak.
- **R4: an opaque bound token.** IAM accepts only a JWT, so an opaque token is refused already.
- **R5: Keycloak client policies.** A realm client policy can require DPoP. Not measured. The
  runbook tells operators not to use one for a client that calls Paigasus.

## 9. Spec challenge

Challenger: `feature-factory:spec-challenger` (Opus), 2026-09-26. Verdict: APPROVE WITH CHANGES.
No BLOCKER.

| Finding | Severity | Action |
|---|---|---|
| The log-message change is ambiguous and breaks two SMA-686 log tests | MAJOR | Folded in: D8 and § 4.2 add a separate `Binding` variant and message; the SMA-686 line stays byte-identical; § 4.2 lists the four doc lines that become false. |
| § 5.4 claims red results that one run cannot show | MAJOR | Folded in: the e2e goes red at the defect assertion only; a second mutation (B) proves the log tests. |
| The e2e proof differs from M2 (HTTPS, host clock, `jti`, private helper, thumbprint input) | MINOR | Folded in: § 5.3 risks and details. |
| No test pins the `typ DPoP` marker or the D2-before-D3 order | MINOR | Folded in: tests 13 and 15. |
| AC 2 claims a token that IAM refuses today (case B5) | MINOR | Folded in: § 2 and § 5.5 name "IAM accepts today" and cite the claim lists. |
| Wrong evidence for "no shipped client uses DPoP" | MINOR | Folded in: § 2 cites `oidc.ts:249`, `:286`. |
| Four gaps in the runbook text | MINOR | Folded in: § 4.3 names the attribute key, the refresh effect, the SDK case and the SMA-686 wording. |
| § 7 gives no evidence for "no kind run" | MINOR | Folded in: § 7 lists the four facts. |
| D9 contradicts D3 | MINOR | Folded in: D9 keeps both markers on the `Bearer` scheme. |
| The evidence file contradicts itself | MINOR | Fixed in the evidence file (§ 4.6). |
| Did Sven choose "no ADR" for SMA-690? | QUESTION | Sven approved the spec with D11 (no ADR) at GATE 1. |
| Should the runbook name Keycloak client policies? | QUESTION | Folded in: § 4.3 and R5, marked "not measured". |
| Should the e2e call a gRPC endpoint? | QUESTION | No: § 5.2 states that `resolve` plus test 18 covers the gRPC paths. The e2e app is HTTP only; a gRPC server in the test adds cost and proves the same use case call. |
