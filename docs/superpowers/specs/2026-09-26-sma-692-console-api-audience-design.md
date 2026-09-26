# SMA-692: the console requests a dedicated API audience from Auth0 and Entra ID

- Linear: SMA-692 (milestone "IAM Gaps")
- Related: SMA-691 (§ 8, the origin of this issue), SMA-678 (`oidc.audience`, D8)
- Status: design approved in chat on 2026-09-26. The written spec waits for Gate 1.

## 1. Problem

SMA-691 recommends a dedicated API audience for IAM, distinct from the client id, and
`oidc.audience` set to it. With that setup, an ID token does not pass IAM's audience check.

Two IdPs cannot use this setup with the console today:

- **Auth0.** Auth0 issues an access token for an API only when the authorization request
  carries the `audience` parameter, or when the tenant has a "Default Audience". The console
  sends only `scope` (`ts/packages/paigasus-auth/src/adapters/oidc.ts:223`,
  `buildAuthorizationUrl`).
- **Entra ID.** An access token for an API needs a scope of that API. The console reads its
  scopes from `PAIGASUS_OIDC_SCOPES` (`ts/packages/paigasus-auth/src/config.ts`, default
  `openid profile email offline_access`). No chart value sets it
  (`charts/paigasus/templates/console-env-configmap.yaml`).

## 2. Intent and success criteria

An operator of Auth0 or Entra ID can use the recommended dedicated-audience setup with the
console. An operator who sets neither new value gets the same requests as today.

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
- **F4. Entra ID `aud`.** "In v2.0 tokens, this value is always the client ID of the API. In v1.0
  tokens, it can be the client ID or the resource URI used in the request" (access token claims
  reference). The token version follows the API's `accessTokenAcceptedVersion`.
- **F5. Entra ID refresh `scope`.** Optional. "The scopes requested in this leg must be
  equivalent to or a subset of the scopes requested in the original authorization_code request
  leg." The documentation does NOT state which resource a refreshed token targets when `scope`
  is omitted.
- **F6. Unknown parameters.** RFC 6749 § 3.1: "The authorization server MUST ignore unrecognized
  request parameters." Keycloak, Okta, Entra ID and Dex do not document their own handling of an
  `audience` parameter at the authorization endpoint.
- **F7. openid-client 6.8.8** (the installed version): `buildAuthorizationUrl(config,
  parameters)` and `refreshTokenGrant(config, refreshToken, parameters?)` both pass extra
  parameters through verbatim.

## 4. Decisions

- **D1. Two explicit values, not a derived value and not a generic map.** (Chosen in chat.)
  - A new optional env `PAIGASUS_OIDC_AUDIENCE`. The console sends it as the `audience`
    authorization parameter only when it is set.
  - The existing `PAIGASUS_OIDC_SCOPES`, now settable from the chart.
  - Rejected: derive the `audience` parameter from `oidc.audience`. Every IdP would then get a
    parameter that only Auth0 needs, and F6 is not confirmed per vendor.
  - Rejected: a generic map of extra authorization parameters. It lets an operator override
    `state`, `nonce` or PKCE unless the code blocks those keys.
- **D2. The default request does not change.** With neither value set, the authorization URL has
  the same parameters as today. The refresh request gains `scope` (D3).
- **D3. The refresh grant sends `scope` = `PAIGASUS_OIDC_SCOPES`, for every IdP.** (Chosen in
  chat.) Reason: F5. Without `scope`, an Entra ID refresh can return a token for another resource.
  RFC 6749 § 6 allows a scope equal to the original grant's scope. The refresh grant never sends
  `audience`: F1 says Auth0 keeps it. Accepted cost: after an operator changes the scopes, a
  session that logged in with the old scopes can get `invalid_scope` at refresh. The console
  treats that like any other refresh failure, and the user logs in again (§ 6).
- **D4. The scopes have one source.** `createOidcClient` receives `scopes` and `audience` at
  construction (dependency injection). `BuildAuthorizationUrlParams` loses its `scopes` field,
  and `routes.ts` stops passing it. The `refresh(refreshToken)` signature does not change, so
  `core/single-flight.ts` (`ResolveDeps.refresh`) does not change.
- **D5. A config guard for `openid`.** `authEnvShape` refuses a `PAIGASUS_OIDC_SCOPES` value whose
  space-separated tokens do not include `openid`. Without `openid`, the IdP returns no ID token,
  and the login fails later with an unclear error. An Entra ID operator who replaces the whole
  list can easily make this mistake.
- **D6. Two chart values, rendered only when set.** `oidc.scopes` feeds `PAIGASUS_OIDC_SCOPES`.
  `oidc.authorizationAudience` feeds `PAIGASUS_OIDC_AUDIENCE`. Both are empty by default. The
  templates read them with `dig`, because a release made before the values existed has no key
  under `--reuse-values` (`charts/CLAUDE.md`). An empty value renders no key, so the golden files
  stay byte-identical.
- **D7. The chart fails on an audience mismatch.** When `oidc.authorizationAudience` is set and is
  not equal to the IAM audience (`paigasus.iamAudience`), the render fails. The console would
  then ask for a token that IAM refuses on every call. (Chosen in chat over a NOTES warning.)
- **D8. The new helper goes in `templates/_audience.tpl`,** and `console-env-configmap.yaml`
  includes it. `_helpers.tpl` and `console-deployment.yaml` do not change, so their four
  whole-file negative-control copies under `ci/helm-render/fixtures/` do not change either
  (`charts/CLAUDE.md`, SMA-691 D5).
- **D9. The chart does not check `openid` in `oidc.scopes`.** D5 checks it at pod start. A second
  copy of the rule in the chart is not needed.

## 5. Design

### 5.1 `@paigasus/auth`

`src/config.ts`, `authEnvShape`:

- `PAIGASUS_OIDC_AUDIENCE: z.string().min(1).optional()`. This is the pattern of the other
  optional keys. An empty string fails, as it does for `PAIGASUS_OIDC_REDIRECT_URI`.
- `PAIGASUS_OIDC_SCOPES` keeps its default and gains a refinement: the value split on
  whitespace must contain `openid`. The error message names the key and says why.

`src/adapters/oidc.ts`:

- The `createOidcClient` options gain `scopes: string` and `audience?: string`.
- `BuildAuthorizationUrlParams` becomes `{ redirectUri, state }`.
- `buildAuthorizationUrl` passes `scope: scopes`, and `audience` only when it is defined.
- `refresh` calls `client.refreshTokenGrant(config, refreshToken, { scope: scopes })`.

`src/runtime.ts`: `createAuthRuntime` passes `scopes: cfg.PAIGASUS_OIDC_SCOPES` and
`audience: cfg.PAIGASUS_OIDC_AUDIENCE` to `createOidcClient`. The `scopes` field on
`AuthRuntime` goes if nothing else reads it. The plan checks every reader first.

`src/http/routes.ts`: the `buildAuthorizationUrl` call drops `scopes`.

`README.md`: the environment variable table gets `PAIGASUS_OIDC_AUDIENCE`, and the
`PAIGASUS_OIDC_SCOPES` row states the `openid` rule and that the refresh grant sends it too.

### 5.2 Helm chart

`values.yaml`, under `oidc`, after `acknowledgeClientIdAudience`:

- `scopes: ""`. NOT required. Empty: the console default. The comment names the Entra ID use
  and that the list must contain `openid`.
- `authorizationAudience: ""`. NOT required. Empty: no `audience` parameter. The comment names
  the Auth0 use and that the value must equal the IAM audience (D7).

`templates/_audience.tpl`: a new helper, `paigasus.validateAuthorizationAudience`, that fails
per D7. The failure message names both values and the runbook section.

`templates/console-env-configmap.yaml`: includes the new helper. It renders
`PAIGASUS_OIDC_SCOPES` and `PAIGASUS_OIDC_AUDIENCE` only when the value is not empty, each with
`| quote`.

### 5.3 Runbook

`docs/ops/RUNBOOK-chart.md` § 6, the per-IdP lines:

- **Auth0.** Create an API. Its identifier is the audience. Set `oidc.audience` and
  `oidc.authorizationAudience` to that identifier. Enable "Allow Offline Access" on the API, or
  the console gets no refresh token. The tenant "Default Audience" is an alternative: it applies
  to every application of the tenant (F2). With it, `oidc.authorizationAudience` can stay empty.
- **Entra ID.** Expose an API on an app registration. Set `oidc.scopes` to
  `openid profile email offline_access` plus exactly one scope of that API, for example
  `api://paigasus/access` (F3). Set `oidc.audience` to the API's application (client) id when
  its `accessTokenAcceptedVersion` is 2. With v1 tokens, the `aud` can be the application ID URI
  (F4). Check the `aud` of a real token before the switch.
- **Migration.** The existing migration order in § 6 stays. Add one fact: after the switch, a
  session that logged in before it keeps a token for the old audience. An Auth0 refresh keeps
  the old audience (F1). An Entra ID refresh with the new scope list can fail with
  `invalid_scope`. So every user of such a session must log in again.

## 6. Error handling

- A bad `PAIGASUS_OIDC_SCOPES` (no `openid`) or an empty `PAIGASUS_OIDC_AUDIENCE` fails
  `authEnvShape` at pod start. The pod crash-loops with the Zod message. This is the behaviour
  of every other invalid key.
- An IdP that refuses the `audience` parameter returns an OAuth error to the callback. The
  existing callback error path handles it. No new code.
- A refresh that fails with `invalid_scope` is not `invalid_grant`. The plan must read the
  refresh error classification in `oidc.ts` (lines 139-171) and state which class
  `invalid_scope` falls into, and what the user sees. If it is classed as transient, a session
  can loop on refresh; then the plan adds `invalid_scope` to the definitive class.

## 7. Testing

- `tests/config.test.ts`: `PAIGASUS_OIDC_AUDIENCE` absent, set, and empty (refused);
  `PAIGASUS_OIDC_SCOPES` without `openid` refused, with `openid` in any position accepted, and a
  value like `openidx` refused.
- `tests/adapters/oidc.test.ts`: the authorization URL has no `audience` when unset, and has it
  when set; `scope` equals the injected scopes; the refresh request body carries `scope` and no
  `audience`. The tests read the real request the fake IdP receives, not a mock of
  openid-client.
- Chart: `tests/env.sh` (or a sibling script) renders with both values and asserts both keys in
  the ConfigMap; renders without them and asserts both keys absent. `tests/refusals.sh` asserts
  the D7 failure, and asserts that an equal value renders.
- The golden files stay byte-identical (D6). A diff in them is a defect.
- Each new assertion must go red when its feature line is deleted (see the memory rule
  "red-first is not proof").

## 8. Out of scope

- An `audience` or `resource` parameter on the refresh grant.
- RFC 8707 `resource` indicators.
- A chart check of `openid` in `oidc.scopes` (D9).
- A live test against an Auth0 or Entra ID tenant. The kind job uses Keycloak and sets neither
  new value.

## 9. Residuals

- **R1.** F5 is not measured. D3 removes the dependency on it for the refresh grant.
- **R2.** F6 is not confirmed per vendor. D1 keeps the `audience` parameter opt-in, so only an
  operator who sets it depends on it.
