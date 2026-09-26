# SMA-692: the console requests a dedicated API audience from Auth0 and Entra ID

- Linear: SMA-692 (milestone "IAM Gaps")
- Related: SMA-691 (§ 8, the origin of this issue), SMA-678 (`oidc.audience`, D8),
  SMA-626 (§ 2.2, the refresh error classes)
- Status: design approved in chat on 2026-09-26. Revised after the adversarial challenge
  (§ 10). The written spec was approved at Gate 1 on 2026-09-26, with D3-a.

## 1. Problem

SMA-691 recommends a dedicated API audience for IAM, distinct from the client id, and
`oidc.audience` set to it. With that setup, an ID token does not pass IAM's audience check.

Two IdPs cannot use this setup with the consoles today:

- **Auth0.** Auth0 issues an access token for an API only when the authorization request
  carries the `audience` parameter, or when the tenant has a "Default Audience". The consoles
  send only `scope` (`ts/packages/paigasus-auth/src/adapters/oidc.ts:223`,
  `buildAuthorizationUrl`).
- **Entra ID.** An access token for an API needs a scope of that API. The consoles read their
  scopes from `PAIGASUS_OIDC_SCOPES` (`ts/packages/paigasus-auth/src/config.ts`, default
  `openid profile email offline_access`). No chart value sets it
  (`charts/paigasus/templates/console-env-configmap.yaml`).

Both zones are affected in the same way. `iam-console` and `gateway-console` both spread
`authEnvShape` (`ts/apps/iam-console/lib/config.ts:50`, `ts/apps/gateway-console/lib/config.ts:66`)
and both read the one console-env ConfigMap (`console-deployment.yaml:86-87`). The gateway console
sends the same access token to the gateway (`ts/apps/gateway-console/lib/chat-route.ts:204`), and
the gateway validates it through IAM. So one audience serves both zones. "The console" in this
spec means both.

## 2. Intent and success criteria

An operator of Auth0 or Entra ID can use the recommended dedicated-audience setup with the
consoles. An operator who sets neither new chart value gets the same authorization request and
the same refresh request as today.

Acceptance criteria (from the issue):

1. An Auth0 operator can make the console request an access token for a dedicated API audience.
2. An Entra ID operator can make the console request a scope of the API application.
3. The runbook states the setup for both.

## 3. Facts this design depends on

Collected from vendor documentation on 2026-09-26. Nothing below was measured against a live
tenant.

- **F1. Auth0 refresh keeps the audience.** "If the audience parameter is omitted, Auth0 returns
  an access token with the original audience" (Auth0, Multi-Resource Refresh Token).
- **F2. Auth0 "Default Audience"** "is equivalent to appending this audience to every
  authorization request made to your tenant for every application" (Auth0, tenant settings).
- **F3. Entra ID scopes.** One authorization request can hold the OIDC scopes plus scopes of one
  resource. At code redemption, "the scopes must all be from a single resource, along with OIDC
  scopes" (Microsoft identity platform, v2 authorization code flow).
- **F4. Entra ID `aud` and `iss`.** "In v2.0 tokens, this value is always the client ID of the
  API." A v1.0 access token has `iss` = `https://sts.windows.net/<tenant>/`, not the v2 issuer
  that the discovery document names. IAM matches `iss` exactly
  (`rs/crates/services/paigasus-iam/src/adapters/oidc/validator.rs:314-316, 326`). So IAM refuses a
  v1.0 access token. The token version follows the API registration's
  `accessTokenAcceptedVersion`; its default (null) gives v1.0 tokens for most account types.
- **F5. Entra ID refresh `scope`.** Optional. "The scopes requested in this leg must be
  equivalent to or a subset of the scopes requested in the original authorization_code request
  leg." The documentation does NOT state which resource a refreshed token targets when `scope`
  is omitted.
- **F6. Unknown parameters.** RFC 6749 § 3.1: "The authorization server MUST ignore unrecognized
  request parameters." Keycloak, Okta, Entra ID and Dex do not document their own handling of an
  `audience` parameter at the authorization endpoint.
- **F7. openid-client 6.8.8** (the installed version): `buildAuthorizationUrl(config,
  parameters)` and `refreshTokenGrant(config, refreshToken, parameters?)` both pass extra
  parameters through verbatim. `new URLSearchParams({ audience: undefined })` sends the string
  `"undefined"`, so the code must omit an absent key, not pass `undefined`.
- **F8. RFC 6749 § 6** limits the refresh `scope` to the scope that the server originally
  GRANTED, not the scope that the client requested. An IdP that granted fewer scopes than the
  configured list can answer the refresh with an error. Examples: a Keycloak user without the
  `offline_access` role, a consent-trimmed scope, an Okta policy that drops a scope. Not measured.
- **F9. The refresh error classes today** (SMA-626 § 2.2, `oidc.ts:139-171`, pinned by
  `tests/adapters/oidc.test.ts:307`). Only `invalid_grant` is definitive (`RefreshRejected`, the
  session is deleted). Every other code, `invalid_scope` included, is transient. A transient
  failure while the access token is live leaves the session in place, and each request in the
  skew window calls the token endpoint again (`src/core/single-flight.ts:208-215`). After the
  access token expires, `resolveSession` throws, `getSession` returns null
  (`src/next/get-session.ts:66-85`), and `requireSession` redirects to `/auth/login`
  (`get-session.ts:120-126`), which deletes the old record (`src/http/routes.ts:209-212`). A
  Server Action gets `Code.Unauthenticated`, which the SDK maps to `relogin`. There is no infinite
  loop. A page that uses only `getSession` shows a signed-out state.

## 4. Decisions

- **D1. Two explicit values, not a derived value and not a generic map.** (Chosen in chat.)
  - A new optional env `PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE`. The console sends it as the
    `audience` authorization parameter only when it is set. The name says "authorization", so
    it does not read as the IAM audience (`oidc.audience`).
  - The existing `PAIGASUS_OIDC_SCOPES`, now settable from the chart.
  - Rejected: derive the `audience` parameter from `oidc.audience` for every IdP. Every IdP
    would then get a parameter that only Auth0 needs, and F6 is not confirmed per vendor.
  - Rejected: a generic map of extra authorization parameters. It lets an operator override
    `state`, `nonce` or PKCE unless the code blocks those keys.
  - Rejected: a boolean such as `oidc.sendAudienceParameter` that sends `paigasus.iamAudience`.
    It removes the D7 comparison, but it brings back the Go-template truth trap
    (`--set-string …=false` is a true string) that SMA-691 D4 avoided.
- **D2. The default authorization request does not change.** With neither value set, the
  authorization URL has the same parameters as today.
- **D3. The refresh `scope`: D3-a, decided at Gate 1.** In chat, the choice was: the refresh grant sends
  `scope` = `PAIGASUS_OIDC_SCOPES` for every IdP. The challenge found facts that the chat choice
  did not have: F8 (the refresh scope must not exceed the GRANTED scope) and F9 (an
  `invalid_scope` on every refresh ends each session at access-token expiry, silently, for every
  user). Nothing in the repo runs a real refresh grant against Keycloak: the e2e tier only checks
  that a refresh token is issued (`tests/e2e/roundtrip.spec.ts:117-120`), the console fake IdP
  ignores `scope`, and the kind job has no refresh step. The two variants:
  - **D3-a (CHOSEN at Gate 1).** Send the refresh `scope` only when the operator set
    `PAIGASUS_OIDC_SCOPES` explicitly. `authEnvShape` makes the key optional with no default, and
    `createAuthRuntime` applies the default for the authorization request only. The chart renders
    the key only when `oidc.scopes` is set (D6), so only an Entra operator (or any operator who
    sets `oidc.scopes`) opts in. The default refresh request stays byte-identical to today, and
    § 2 holds for every request. Cost: the code carries an "explicitly set" distinction.
  - **D3-b (the chat choice, REJECTED at Gate 1).** Send `scope` on every refresh, for every
    IdP. Cost: a behaviour change on Keycloak, Okta and Dex, not measured anywhere; F8 can end
    sessions for users with a trimmed grant. It would have needed a real refresh grant with
    `scope` against Keycloak 26.4 in the auth e2e tier.
  - In both variants the refresh grant never sends `audience`: F1 says Auth0 keeps it.
- **D4. The scopes and the audience are injected once.** `createOidcClient` receives them at
  construction (dependency injection). `BuildAuthorizationUrlParams` loses its `scopes` field.
  `routes.ts:181` is the only production reader of `AuthRuntime.scopes`, so that field goes. The
  `refresh(refreshToken)` signature does not change, so `core/single-flight.ts`
  (`ResolveDeps.refresh`) does not change.
- **D5. A config guard for `openid`.** `authEnvShape` refuses a `PAIGASUS_OIDC_SCOPES` value whose
  whitespace-separated tokens do not include `openid`. Without `openid`, the first login fails at
  once (`idTokenExpected: true`, `oidc.ts:253-258`) with an unclear error. The operator sees only
  `PAIGASUS_OIDC_SCOPES: custom`, because `describeIssues` hides the message of a custom issue on
  a key it does not own (`ts/packages/paigasus-next-config/src/runtime.ts:154-161`). That is
  still better than a login failure; D9 gives the readable reason.
- **D6. Two chart values, rendered only when set.** `oidc.scopes` feeds `PAIGASUS_OIDC_SCOPES`.
  `oidc.authorizationAudience` feeds `PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE`. Both are empty by
  default. The templates read them with `dig "…" "" .Values.oidc`, which returns `""` for an
  absent key. A release made before the values existed has no key under `--reuse-values`, and a
  direct read gives nil; a `toString` on nil gives `"<nil>"`, which is not empty. `dig` removes
  that trap. An empty value renders no key, so the golden files stay byte-identical.
- **D7. The chart fails on an audience mismatch.** (Chosen in chat over a NOTES warning.) The
  render fails when `oidc.authorizationAudience` is set and:
  - `oidc.audience` is empty. IAM then accepts only `oidc.clientId`, and Auth0 refuses a client
    id as an API audience. This closes the gap where `authorizationAudience == clientId` passes
    the equality check; or
  - it is not equal to `oidc.audience`. The console would then ask for a token that IAM refuses on
    every call.
  The comparison is exact string equality. An IdP that takes a space-separated list of audiences
  is out of scope.
- **D8. The new helpers go in `templates/_audience.tpl`,** and `console-env-configmap.yaml`
  includes them. `_helpers.tpl` and `console-deployment.yaml` do not change, so their four
  whole-file copies under `ci/helm-render/fixtures/` do not change. But
  `ci/helm-render/fixtures/leaked-value/templates/console-env-configmap.yaml` is a whole-file copy
  of the live ConfigMap plus one leaking line. It must be re-synced in the same change, and keep
  its mutation line. `charts/CLAUDE.md:22` says "four" copies; it must name all of them.
- **D9. The chart also checks `openid` in `oidc.scopes`.** (Reversed after the challenge.) The
  render fails when `oidc.scopes` is set and its space-separated tokens do not include `openid`.
  Only the chart can give the operator a readable reason, at `helm upgrade` time (D5). The
  `values.yaml` comment also names `offline_access`: without it, the IdP issues no refresh token,
  and every user is signed out at each access-token expiry (`ts/packages/paigasus-auth/README.md:60-74`).
  The chart does not fail on a missing `offline_access`, because an operator can choose short
  sessions.
- **D10. The refresh failure log names the OAuth error code.** Today a transient refresh failure
  logs only `reason: 'transient'` (`single-flight.ts:212`), and `wrapError` keeps only the error
  class name (`oidc.ts:133-136`). After a scope change, an `invalid_scope` looks the same as a
  network error. The adapter adds the `.error` code of a `ResponseBodyError` to the wrapped
  error, and the log line carries it. The code is from the fixed RFC 6749 § 5.2 list and holds no
  secret. The plan must check the logger's field allow-list (`src/ports/logger.ts`) and keep the
  "never the error object, never its message, never a URL" rule.
- **D11. `invalid_scope` stays transient.** SMA-626 § 2.2 made it transient on purpose: a
  deployment-wide misconfiguration must not sign out the whole fleet. F9 shows there is no loop.

## 5. Design

### 5.1 `@paigasus/auth`

`src/config.ts`, `authEnvShape`:

- `PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE`: optional, at least one character, and no surrounding
  whitespace (the rule `httpsUrl` uses, `config.ts:25`).
- `PAIGASUS_OIDC_SCOPES`: the `openid` refinement (D5). A token-exact match: `openidx` fails.
  It becomes optional with no default (D3-a). `createAuthRuntime` holds the default list
  `openid profile email offline_access` for the authorization request.

`src/adapters/oidc.ts`:

- The `createOidcClient` options gain `scopes: string` (always set; the default list when the env
  is absent), `audience?: string`, and `refreshScope?: string` (set only when the env is set,
  D3-a).
- `BuildAuthorizationUrlParams` becomes `{ redirectUri, state }`.
- `buildAuthorizationUrl` passes `scope: scopes`, and spreads `audience` only when it is defined
  (F7).
- `refresh` passes `{ scope: refreshScope }` only when `refreshScope` is defined (D3-a, F7), and
  never `audience`. With no `refreshScope`, the call stays `refreshTokenGrant(config, refreshToken)`.
- D10: the wrapped refresh error carries the OAuth code.

`src/runtime.ts`:

- `CreateAuthRuntimeDeps` gains an OIDC client factory, next to `store`, `resolver` and
  `logger`, with `createOidcClient` as the default. A test can then assert the options that the
  runtime passes. Today `tests/runtime.test.ts:84-87` (`rt.scopes`) is the only check that the
  env reaches the runtime, and D4 removes that field.
- `AuthRuntime.scopes` goes (D4).

`src/http/routes.ts`: the `buildAuthorizationUrl` call drops `scopes`.

`README.md`: the environment variable table gets `PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE`. The
`PAIGASUS_OIDC_SCOPES` row states the `openid` rule and the refresh behaviour per D3.

### 5.2 Helm chart

`values.yaml`, under `oidc`, after `acknowledgeClientIdAudience`:

- `scopes: ""`. NOT required. Empty: the console default. The comment names the Entra ID use,
  that the list must contain `openid` (the render fails without it) and should contain
  `offline_access`, and that both consoles restart when it changes.
- `authorizationAudience: ""`. NOT required. Empty: no `audience` parameter. The comment names
  the Auth0 use and that the value must equal `oidc.audience` (D7).

`templates/_audience.tpl`: two new helpers that fail per D7 and D9. Each failure message names
the values and `docs/ops/RUNBOOK-chart.md` § 6.

`templates/console-env-configmap.yaml`: includes both helpers. It renders `PAIGASUS_OIDC_SCOPES`
and `PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE` only when the value is not empty, each with `| quote`.
Both consoles restart through the existing `checksum/console-env` annotation
(`console-deployment.yaml:27`). IAM does not restart.

`charts/paigasus/README.md:115-144` (the audience section): name the two values.

`ci/helm-render/fixtures/leaked-value/templates/console-env-configmap.yaml` and
`charts/CLAUDE.md:22`: per D8.

### 5.3 Runbook

`docs/ops/RUNBOOK-chart.md`. Every statement below becomes false or incomplete, so each one
changes:

- § 1, the values table: rows for `oidc.scopes` and `oidc.authorizationAudience`.
- § 2, the refused list: the D7 and D9 refusals.
- § 5, the restart table: both values restart both consoles, not IAM.
- § 6, lines 104-106 ("The console sends no `audience` or `resource` parameter"): now true only
  when `oidc.authorizationAudience` is empty.
- § 6, line 136 ("The console requests the scopes `openid profile email offline_access`"): now
  the default, which `oidc.scopes` replaces.
- § 6, the Auth0 line:
  - Create an API. Its identifier is the audience. Enable "Allow Offline Access" on the API, or
    the console gets no refresh token.
  - Set `oidc.audience` and `oidc.authorizationAudience` to that identifier.
  - IAM needs `email` in the access token (`RUNBOOK-chart.md:110-111`). Auth0 does not put it into
    an API access token by default: add it with a post-login Action. Not measured.
  - The tenant "Default Audience" is an alternative. It applies to every application of the
    tenant (F2). With it, `oidc.authorizationAudience` can stay empty.
- § 6, the Entra ID line:
  - Register a SEPARATE app registration for the API. Do not expose the API on the console's own
    registration: the v2 `aud` would then be the console's client id, which is the SMA-691
    anti-pattern.
  - Set its `accessTokenAcceptedVersion` to 2. IAM refuses a v1.0 access token (F4). v1.0 is not
    supported.
  - Expose one scope, grant the console's registration the delegated permission, and give
    consent.
  - Set `oidc.scopes` to `openid profile email offline_access` plus exactly one scope of that
    API, for example `api://paigasus-api/access` (F3).
  - Set `oidc.audience` to the API registration's application (client) id, a GUID (F4).
  - Add `email` as an optional claim of the access token. Not measured.
  - Check the `aud`, `iss` and `email` of a real token before the switch.
- § 6, the migration order: step 1 ("add the API audience … next to the client id",
  `RUNBOOK-chart.md:149-150`) has no Auth0 equivalent, and D7 forces IAM and the consoles to
  change in one upgrade. State the real cases:
  - **Auth0 with a Default Audience D, moving to `oidc.authorizationAudience` = D.** The tokens
    do not change. Nothing breaks.
  - **Auth0 moving from no API (or from D) to a new API A.** A hard cut. A session that logged in
    before the upgrade keeps a token for the old audience, and an Auth0 refresh keeps it (F1).
    IAM refuses those tokens. Every user must log in again, and IAM and the consoles restart at
    different times.
  - **Entra ID moving to a new scope list.** An old session's refresh can fail, because the
    refresh now sends the new `oidc.scopes` (D3-a). The error code is not measured; Entra reports many
    conditions as `invalid_grant` (AADSTS codes), which deletes the session, and others as
    `invalid_scope`, which ends it at access-token expiry (F9). In both cases the user must log
    in again.
  - **Mixed pods.** During the rollout, old and new console pods share one session store. A
    session can be refreshed by a pod with the other scope list for a short window.

## 6. Error handling

- A bad `PAIGASUS_OIDC_SCOPES` (no `openid`) or a bad `PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE`
  (empty, or surrounding whitespace) fails `authEnvShape` at pod start. The pod stays Running and
  NotReady: liveness is a TCP check (`console-deployment.yaml:88-102`,
  `ts/apps/iam-console/app/healthz/route.ts:4-5`). The chart refuses the `openid` case before
  that (D9).
- An IdP that refuses the `audience` parameter returns an OAuth error to the callback. The
  existing callback error path handles it. No new code.
- A refresh that fails with any code other than `invalid_grant` stays transient (D11). F9
  describes what the user sees. D10 makes the code visible in the log.

## 7. Testing

- `tests/config.test.ts`: `PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE` absent, set, empty (refused),
  and with surrounding whitespace (refused); `PAIGASUS_OIDC_SCOPES` without `openid` refused,
  with `openid` in any position accepted, and `openidx` refused.
- `tests/adapters/oidc.test.ts`: the authorization URL has no `audience` when unset (and no
  `"undefined"` string), and has it when set; `scope` equals the injected scopes; the refresh
  request body carries `scope` only when `refreshScope` is injected, and never `audience`; the D10 code reaches the wrapped
  error. The tests read the real request that the fake IdP receives. `tests/fixtures/jwks.ts:148-191`
  reads the token request body but does not expose it, so the plan adds a recorder there.
- `tests/runtime.test.ts`: through the new factory dependency, the runtime passes the env values
  to `createOidcClient`. This replaces the `rt.scopes` assertion.
- The eight test sites that build an `AuthRuntime` literal with `scopes` drop the field:
  `tests/server.test.ts:46`, `tests/support/store-failure.ts:128`,
  `tests/next/get-session.test.ts:84`, `tests/http/callback.test.ts:108`,
  `tests/http/logout.test.ts:151`, `tests/http/login.test.ts:59`,
  `tests/http/route-handler.test.ts:61`, `tests/http/login-returnto-table.test.ts:43`.
- D3-a: with the env absent, the refresh request body has no `scope` (byte-identical to today);
  with the env set, it has `scope` equal to the env value.
- Chart, in `charts/paigasus/tests/env.sh` (not a new script: a new script raises
  `CHART_SCRIPT_FLOOR` in `ci/helm-render/run.sh:33` and its pin in `ci_targets.py`). Rows:
  - both values set: both keys in the ConfigMap;
  - both values unset: both keys absent;
  - `--set oidc.scopes=null` and `--set oidc.authorizationAudience=null` (the reuse-values shape):
    it renders, has no key, and D7 does not fail;
  - a number on both sides of D7 (for example `oidc.audience=123`,
    `oidc.authorizationAudience=123`): it renders;
  - restart scope: a change of either value changes both console pod templates and does not
    change the IAM pod template.
- Chart, in `charts/paigasus/tests/refusals.sh`: D7 with a different value fails; D7 with
  `oidc.audience` empty fails; an equal value renders; D9 without `openid` fails; `openidx` fails.
- The golden files stay byte-identical (D6). A diff in them is a defect.
- Each new assertion must go red when its feature line is deleted, the runtime wiring included.

## 8. Out of scope

- An `audience` or `resource` parameter on the refresh grant.
- RFC 8707 `resource` indicators.
- A space-separated audience list in D7.
- A change to the refresh error classes (D11).
- A live test against an Auth0 or Entra ID tenant.
- A change to `describeIssues` so that it shows the message of a custom issue on
  `PAIGASUS_OIDC_SCOPES`.
- An explicit `oidc.scopes` in the kind values. The kind job has no refresh step, so it adds no
  coverage.

## 9. Residuals

- **R1.** F5 is not measured.
- **R2.** F6 is not confirmed per vendor. D1 keeps the `audience` parameter opt-in, so only an
  operator who sets it depends on it.
- **R3.** The Auth0 `email` Action and the Entra ID optional `email` claim are from vendor
  documentation and not measured. The runbook marks both.
- **R4.** The Entra ID refresh error code after a scope change is not measured (§ 5.3).

## 10. Challenge changelog (2026-09-26)

Verdict: APPROVE WITH CHANGES.

Folded in:

- BLOCKER, D3 contradicts § 2 and changes every refresh: § 2 corrected; D3 made open for Gate 1
  with the two variants and their costs; F8, F9 added.
- MAJOR, § 6 deferred an answered question: F9 and D11 state the behaviour; the "make it
  definitive" branch is deleted.
- MAJOR, the Entra ID text leads to a refused token: v2 required, a separate API registration,
  consent, `email` claim steps; F4 extended with `iss`.
- MAJOR, stale runbook and chart docs: § 5.3 lists every affected section; the Auth0 migration
  cases are stated.
- MAJOR, no test of the runtime wiring: an OIDC client factory in `CreateAuthRuntimeDeps`.
- MAJOR, chart rows for known traps: the null, number, `oidc.audience`-empty and restart rows;
  `env.sh` chosen over a new script.
- MINOR, the `leaked-value` fixture: D8.
- MINOR, the D5 error text and the pod state: D5 and § 6 corrected; D9 reversed (the chart now
  checks `openid`).
- MINOR, the `dig` reason: D6 corrected.
- MINOR, the env name: `PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE`.
- MINOR, `authorizationAudience == clientId` passes D7: D7 also fails when `oidc.audience` is
  empty.
- MINOR, surrounding whitespace: § 5.1.
- MINOR, the refresh error code is not visible: D10.
- MINOR, plan facts (the recorder, the eight literal sites, the `"undefined"` trap): § 7, F7.
- MINOR, both zones: § 1.
- MINOR, `offline_access`: D9 and the `values.yaml` comment.
- MINOR, mixed pods: § 5.3.
- QUESTION, which Entra error code: § 5.3 no longer names one code; R4.
- QUESTION, the Auth0 and Entra `email` steps: marked not measured; R3.
- QUESTION, § 2 vs D3: decided at Gate 1 as D3-a. § 2 holds for every request.

Rejected:

- MINOR, the boolean chart shape: it brings back the truth trap that SMA-691 D4 avoided (D1).
- QUESTION, a space-separated audience list in D7: out of scope (§ 8). No IdP in the runbook
  needs it.
- QUESTION, an explicit `oidc.scopes` in the kind values: the kind job has no refresh step (§ 8).
