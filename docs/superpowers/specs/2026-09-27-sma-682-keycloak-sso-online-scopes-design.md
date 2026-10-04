# SMA-682 — keep the Keycloak SSO session after a console login

- Issue: [SMA-682](https://linear.app/smaschek/issue/SMA-682)
- Workspaces: `ci/kind/` (realm, values, report checker), `ts/apps/iam-console/tests/cluster/`
  (journey J1), `ts/packages/paigasus-auth/` (README and test comments only), `docs/ops/`,
  `charts/paigasus/` (comments and README only)
- Related: SMA-514 (J1, decisions D9, D10 and D11), SMA-681 (`id_token_hint`, merged in
  `a71e4063`), SMA-506 (the default scope list, its spec § 6.5), SMA-692 (`PAIGASUS_OIDC_SCOPES`
  and `oidc.scopes`), SMA-652 (never reuse a Keycloak tab)
- Status: approved by Sven on 2026-10-04. Written by the feature-factory Stage 1 agent on
  2026-09-27 and challenged once (verdict NEEDS REWORK, folded, see § 12). Q1 and Q2 are decided
  (§ 11). Q3 had no answer, so D4 stays.

## 1. Problem

After a user logs in to a console, Keycloak keeps no SSO session. A new authorization request in
the same browser shows the login form again. Keycloak clears `KEYCLOAK_IDENTITY` and
`KEYCLOAK_SESSION`.

Facts from the issue and from the SMA-681 spec § 3.1 (Keycloak 26.4.7, the unchanged kind realm,
the package default scope):

- `@paigasus/auth` requests `openid profile email offline_access` when `PAIGASUS_OIDC_SCOPES` is
  absent (`ts/packages/paigasus-auth/src/runtime.ts:112`, `DEFAULT_OIDC_SCOPES`).
- The kind realm grants `offline_access` as a DEFAULT client scope
  (`ci/kind/realm/paigasus-realm.json:54`). So Keycloak grants it also when the console does not
  ask for it.
- The token response holds a refresh token with `typ: Offline`. The next `/auth` request in the same
  cookie jar shows the login form. Keycloak logs `AuthenticationManager: No user session`, with no
  logout event.
- With `offline_access` moved to the OPTIONAL client scopes and the scope `openid profile email`,
  the token response holds an online refresh token (`typ: Refresh`), and the next `/auth` request
  returns 302 with a code (SMA-681 spec § 3, row M-a). So the online session survives when no
  offline token is issued.

Consequences today:

1. Other applications on the same realm get no SSO from a console login.
2. J1 cannot check that logout ends the IdP session, because no IdP session exists. SMA-514 D11
   removed the SSO control and the IdP-session check for this reason.
3. When the console session expires, the user must type the password again. Keycloak cannot log
   the user in silently. **Correction after the challenge:** the D2 setup does NOT fix this
   consequence on the Keycloak default timeouts. The console session (8 h idle, 24 h absolute) is
   longer than the Keycloak default SSO session (30 min idle, 10 h max). So with D2 the SSO session
   always ends first, and the user still types the password at each console expiry. See D9.

A second fact is wrong in the repository. The package README section "Why `offline_access` is a
default scope" says that Keycloak "never issues a refresh token for the code flow" without
`offline_access`. SMA-506 spec § 6.5 says the opposite ("Keycloak issues a refresh token for the
code flow without it"), and SMA-681 row M-a measured it. The IdPs that need `offline_access` for a
refresh token are Entra ID, Auth0 and Okta (RUNBOOK-chart § 6).

The mechanism inside Keycloak that removes the online session is not known. Nobody read the
Keycloak 26.x code path from code to token for `offline_access`. This spec does not depend on the
mechanism. It depends only on the measured fact that an online-only login keeps the session.
Measurement M10 (§ 6.1) checks one hypothesis about it.

## 2. Decisions

| Id | Decision | Why |
|---|---|---|
| D1 | The package default `DEFAULT_OIDC_SCOPES` does NOT change. It keeps `offline_access`. | The default serves every IdP. Entra ID, Auth0 and Okta issue no refresh token without `offline_access`. Without a refresh token, `resolveSession` deletes the session at the first access-token expiry (README "Why `offline_access`…"). A change of the default makes mass logouts on those IdPs. |
| D2 | For Keycloak, the documented configuration requests no offline token: set `oidc.scopes` (chart) or `PAIGASUS_OIDC_SCOPES` (package) to `openid profile email`, and put `offline_access` in the client's OPTIONAL client scopes, not the default ones. | Both parts are necessary. A default client scope is granted also when the console does not request it. An optional scope that the console requests is granted too. Only both together give an online-only login. The runbook states the D9 trade-off next to it, so an operator can choose approach B instead. |
| D3 | The kind job uses the D2 scope configuration: `ci/kind/values/a.yaml` sets `oidc.scopes: "openid profile email"`, and the kind realm moves `offline_access` from `defaultClientScopes` to `optionalClientScopes`. The kind realm also registers one probe redirect URI for J1 (D6). | The kind realm is the Keycloak example of the SCOPE setup in RUNBOOK-chart § 6, and J1 can then prove SSO. It is NOT a complete example of the timeout setup: it keeps the Keycloak default SSO timeouts (D9). The runbook sentence "a complete example of this setup" changes to say this. |
| D4 | The kind user keeps the `offline_access` realm role. | The role has no effect when no offline token is requested. Removing it can make Keycloak assign `default-roles-paigasus` instead, which also holds `offline_access` (not measured). The smallest change is to keep it. See Q3. |
| D5 | The package e2e realm (`ts/packages/paigasus-auth/tests/e2e/keycloak-realm.json`) does NOT change. | That suite tests the package DEFAULT scope list, with `offline_access` (`roundtrip.spec.ts:114-120` asserts that a refresh token is issued). Only its comments that name SMA-682 get one clause: the fixture keeps the default on purpose. |
| D6 | J1 checks the IdP session with a `prompt=none` authorization request, sent as a BROWSER navigation (`page.goto`) in a NEW context that holds only the two Keycloak cookies captured after login. The `redirect_uri` is a dedicated probe URI, `https://console.paigasus.test/kind-sso-probe`, which no ingress rule routes. The result is the `Location` of the Keycloak 302, read from the redirect chain. | (a) Playwright's Node-side request API cannot resolve the kind hosts. They resolve only through Chromium's `--host-resolver-rules` (`tests/cluster/playwright.config.ts:42-46`, and "Spec defect 4" in `phase-a/sso.spec.ts:34-35`). So `context.request` is not possible. (b) A replay of the captured cookies in a new context checks the SERVER-side SSO session. The end-session response expires `KEYCLOAK_IDENTITY` in context A, so a probe in context A could show `login_required` also with a live server session. This is the same rule that steps 3 and 6 apply to the console sid. SMA-514 D10 (the Keycloak cookie replay) is therefore restored, not obsolete. (c) The probe URI keeps the console out of the probe. With `redirect_uri=/iam/auth/callback`, the foreign `state` gives `txn_missing`, `server.ts:125-128` returns 302 to `/iam/auth/login`, and `handleLogin` then logs in again silently through SSO. That adds a console session and can hide a failure. The ingress routes only the zone base paths (`charts/paigasus/templates/ingress.yaml:39`), so the probe URI reaches no console. |
| D7 | J1 gets two new steps with new titles, not new assertions inside old steps. `ci/kind/journeys-report.mjs` `EXPECTED_STEPS`, its fixtures and its test change with them. | A step title says what the step proves. The report checker then proves that each check ran. |
| D8 | J1 step 5 asserts that Keycloak shows no "Do you want to log out?" page: the end-session document response is a 3xx. The assertion is placed right after the end-session request is observed, BEFORE the `id_token_hint` assertions and before the wait for `/iam/`. After `/iam/` loads, the step also asserts that no recorded document on `IDP_HOST` has status 200. What this proves depends on measurement M6 (§ 6.1): see § 3.3.1. | The Linear comment of 2026-09-25 asks for it. The placement lets the § 6.2 mutation fail at the D8 message, not at the hint assertion or at a 30 s timeout. M6 returned `login_required` (§ 13): D8 is "no confirmation page on the console's real logout sequence". It is not a proof of SMA-681 AC 1 with a live session. |
| D9 | The kind realm keeps the Keycloak default SSO timeouts. The runbook does NOT tell operators to raise the realm timeouts as a default step. It states the trade-off and the choices (§ 3.5). What the runbook recommends was Q2, decided on 2026-10-04 (§ 11). | With an online refresh token, a refresh fails when the Keycloak SSO session has ended. So the console session can last no longer than the Keycloak SSO session. `SSO Session Idle` and `SSO Session Max` are realm-wide: they apply to every client of the realm. Raising them makes a stolen `KEYCLOAK_IDENTITY` cookie and an unattended SSO session live longer for all applications. The chart does not expose `PAIGASUS_SESSION_TTL_SECONDS` or `PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS` (no match under `charts/`), so chart operators have the fixed package defaults, 8 h and 24 h (`config.ts:102-103`). The Keycloak `Client Session Idle` and `Client Session Max` settings (realm or client level) can cap the session lower still. J1 runs for less than 5 min, so the kind values do not matter for the test. |
| D10 | No chart render check and no startup check detects `offline_access` on a Keycloak issuer. | The chart and the package do not know the IdP product. A guess from the issuer URL (`/realms/`) is not reliable. The documentation and J1 are the controls. See residual R1, which names an IdP-neutral log signal as a deferred option. |

### Approaches that were rejected

- **A. Remove `offline_access` from the package default.** This fixes Keycloak and breaks Entra ID,
  Auth0 and Okta (D1).
- **B. Keep the offline token and accept no SSO, with documentation only.** The issue permits it.
  J1 then can never prove that logout ends the IdP session, and other applications of the realm get
  no SSO. **After the challenge, B is a real option for PRODUCTION operators, not only a rejected
  one.** On Keycloak default timeouts, D2 shortens console sessions (D9) and does not fix
  consequence 3. Q2 decides what the runbook recommends. The kind job uses D2 in every case,
  because J1 needs the SSO session.
- **C. Choose the scopes from the IdP product in the chart.** The chart has no IdP-product value.
  A new value is a new interface for one IdP, and D2 is one line of values.

## 3. Design

### 3.1 The kind realm — `ci/kind/realm/paigasus-realm.json`

```json
"redirectUris": [
  "https://console.paigasus.test/iam/auth/callback",
  "https://console.paigasus.test/gateway/auth/callback",
  "https://console.paigasus.test/kind-sso-probe"
],
...
"defaultClientScopes": ["basic", "web-origins", "acr", "profile", "roles", "email"],
"optionalClientScopes": ["address", "phone", "microprofile-jwt", "offline_access"]
```

No other change. JSON has no comments, and Keycloak refuses an unknown field (SMA-506
measurements, M6 item 1). The reason for each change lives in the J1 header comment, in `a.yaml`
and in the runbook. The probe URI is a registered redirect URI on a throwaway test realm. It names
the console host only, so it adds no open redirect to a foreign host.

### 3.2 The kind values — `ci/kind/values/a.yaml`

Add `scopes: "openid profile email"` under `oidc:`, with a comment: the kind realm is the Keycloak
example of the scope setup of RUNBOOK-chart § 6; no `offline_access`, so Keycloak keeps the SSO
session (SMA-682).

Effects, from the SMA-692 design:

- The chart renders `PAIGASUS_OIDC_SCOPES` into `console-env` for both consoles.
- Each refresh request then sends `scope=openid profile email`. That is the requested scope, so it
  is not wider than the grant (RFC 6749 § 6). Measurement M3 must show that Keycloak 26.7 accepts
  it. If M3 fails, stop and report.
- `repo:helm-render` row 7 renders `a.yaml`. The value holds `openid`, so the render passes.

### 3.3 Journey J1 — `ts/apps/iam-console/tests/cluster/journeys/auth-roundtrip.spec.ts`

Constants and a helper at file scope:

```ts
const PROBE_REDIRECT = `${ORIGIN}/kind-sso-probe`;
const KEYCLOAK_COOKIES = ['KEYCLOAK_IDENTITY', 'KEYCLOAK_SESSION'] as const;

/**
 * A prompt=none authorization request, sent as a browser navigation in a NEW context that holds
 * only `idpCookies`. Returns the Location of the Keycloak 302. The server-side SSO session decides
 * the answer, not the cookie state of any other context.
 */
async function silentAuthorize(browser: Browser, authorizationEndpoint: URL, idpCookies: readonly Cookie[]): Promise<URL>
```

The `@playwright/test` import on line 30 gets `Cookie`. `redirectChain` lives in
`tests/cluster/support/login.ts:45`.

- `authorizationEndpoint` comes from step 1: the origin and path of the last hop of the cold-visit
  chain, which step 1 already asserts is the IdP authorization endpoint. Step 1 returns it. So no
  discovery request and no second source of truth are necessary.
- The helper makes a new context (`ignoreHTTPSErrors: true`), adds `idpCookies` with
  `context.addCookies`, opens a page and calls `page.goto` on the authorization endpoint with
  `client_id=paigasus-console`, `response_type=code`, `scope=openid`,
  `redirect_uri=${PROBE_REDIRECT}`, a random `state`, and `prompt=none`.
- It reads `redirectChain(response)`. It asserts that `chain[0]` is on `IDP_HOST` with a status in
  300-399, and that `chain[1]` has the origin and path of `PROBE_REDIRECT` and the same `state`. It
  returns `chain[1].url`. The caller asserts `code` or `error`. It does not assert the status of
  the probe URI itself (expected 404 from the ingress controller; recorded as an annotation).
- The context is not closed and not reused, like the other replay contexts of J1. With
  `prompt=none`, Keycloak never shows its form (OIDC Core § 3.1.2.1), so no Keycloak tab exists to
  reuse (SMA-652).

The step list becomes seven steps. The new titles are fixed here:

1. `cold visit: /iam/orgs goes through /iam/auth/login to the IdP form` (unchanged title; now
   returns the authorization endpoint)
2. `login: the callback returns to /iam/orgs with a session` (unchanged)
3. `control: the sid replays in both zones` (unchanged)
4. **NEW** `control: the IdP session answers prompt=none with a code` — read the cookies of
   context A for `https://${IDP_HOST}/realms/paigasus/`, keep the two `KEYCLOAK_COOKIES`, and
   assert that both are present. Keep them for step 7. Then `silentAuthorize` with them. Assert
   `code` is present and `error` is absent. This is issue AC 1: a new authorization request with
   the browser's SSO cookies completes with no login form. On `main` this step fails at the cookie
   check, because Keycloak clears both cookies (§ 1).
5. `logout: the shell form ends at the IdP and returns to /iam/ with no session cookie` (title
   unchanged). Add D8 in this order, right after `const endSessionRequest = await endSession;` and
   the `client_id` check: `(await endSessionRequest.response())?.status()` is in 300-399, with the
   message `D8: Keycloak answered end-session with a page, not a redirect (a confirmation page?)`.
   The `id_token_hint` assertions follow it. After `await back` and the `/iam/` load, assert that
   no recorded document on `IDP_HOST` has status 200. Update the annotation text: remove
   "(SMA-682: no confirmation page on kind stack)", and write "(no confirmation page, SMA-682 D8)".
6. `the old sid is refused by both zones` (unchanged)
7. **NEW** `the IdP session is dead: prompt=none returns login_required` — `silentAuthorize` with
   the cookies captured in step 4 (the values from before logout), in a new context. Assert
   `error` is `login_required` and `code` is absent. Step 4 is the control: the same cookies and
   the same request shape returned a code before logout. Because the cookies are replayed, the
   result cannot come from the cookie clear in context A. It can come only from the server-side
   end of the SSO session.

The header comment changes:

- Line 7: "The five step titles" becomes "The seven step titles".
- The D11 paragraph and the D9 paragraph are replaced with the new facts: SSO exists on the kind
  stack since SMA-682 (D2, D3); steps 4 and 7 replay the two Keycloak cookies in a new context
  (SMA-514 D10 restored, for the reason in D6 (b)); the probe URI and why (D6 (c)); what step 5's
  D8 assertion proves (§ 3.3.1).

The code of step 4 is never exchanged. Keycloak lets it expire (the realm default code lifespan,
60 s). It belongs to the same client and the same user session, so it adds no user session.

#### 3.3.1 What D8 and step 7 prove depends on M6

`handleLogout` awaits `bestEffortRevoke` (step 3, `ts/packages/paigasus-auth/src/http/routes.ts:541`)
BEFORE it builds the end-session redirect (step 4). With an ONLINE refresh token, Keycloak
revocation acts on the client session. If the revoke also ends the user session, no SSO session
exists at end-session. Then Keycloak shows no confirmation page, with or without the hint, and
end-session has no session to end. SMA-681 rows M-b and M-c used curl with no revoke, so they do
not answer this.

Measurements M6 and M7 (§ 6.1) decide which text is true:

- **M6 returns a code (the revoke leaves the SSO session alive).** D8 is the end-to-end proof of
  SMA-681 AC 1 with a live SSO session, and step 7 proves that end-session ends the IdP session.
  § 5 AC 2 and AC 3 stay as written.
- **M6 returns `login_required` (the revoke ends the SSO session).** Then:
  - D8 is restated as "no confirmation page on the console's real logout sequence". The spec, the
    J1 header comment and the step 5 comment do not claim an SMA-681 proof.
  - Step 7 proves that "the console logout sequence (revoke, then end-session) ends the IdP
    session", not that end-session does it. Issue AC 2 ("logout ends the IdP session") is still
    met.
  - The § 6.2 D8 mutation cannot bite. Record that.
  - Add residual R7: the end-session path of logout is not proven end to end on the kind stack.

### 3.4 The report checker — `ci/kind/journeys-report.mjs` and its test

- `EXPECTED_STEPS['auth-roundtrip.spec.ts']` gets the seven titles of § 3.3 in order.
- Fixtures in `ci/kind/fixtures/journeys-report/` (measured J1 step counts today):
  - `pass.json` (5 J1 steps) and `skipped.json` (5): get the two new steps, in order, so they hold
    all seven. (`skipped.json` holds 0 J2 steps; that does not change.)
  - `early-return.json` (2): does NOT change. It must keep only 2 J1 steps: it is the "early
    return" case.
  - `fail-expected.json` (1): does not change.
- `ci/kind/journeys-report.test.mjs:86-87`: the test title "(now after step 2 of 5)" becomes
  "(now after step 2 of 7)", and the expected message `'3 step(s) never ran'` becomes
  `'5 step(s) never ran'`.
- `ci/kind/journeys-report.test.mjs:91` (`'7 step(s) never ran'`) deletes the steps of
  `suites[1]`, which is J2 (7 steps). It does not change.
- The pinned annotation text of the checker test (`SMA-514 scenario 1: logout hops across both
  zones`) is fixture data, not the J1 text, so it does not change.

### 3.5 Documentation

- `ts/packages/paigasus-auth/README.md`
  - Section "Why `offline_access` is a default scope" (line 61): replace "some identity providers (Keycloak,
    for example)" with "Entra ID, Auth0 and Okta". Add a paragraph "Keycloak: offline or online
    tokens". It states D2, the SSO fact of § 1, and the § 3.5.1 trade-off in short form, with a
    link to RUNBOOK-chart § 6.
  - The `PAIGASUS_OIDC_SCOPES` table row (line 37): "Keep `offline_access`, except on Keycloak when you want
    SSO (see …)".
- `docs/ops/RUNBOOK-chart.md`
  - Line 37 (the values table), line 157 (§ 6 item 1) and lines 253-258 (the scope paragraphs):
    the same exception.
  - Lines 260-264 ("**Set `oidc.scopes` only when your IdP needs it.**"): add Keycloak with D2 as
    an IdP that needs it, and say that the D2 list is the requested list, so the RFC 6749 § 6
    refusal does not apply to it (M3).
  - Keycloak example 1 (heading at line 440, text at lines 457-460): "Put `basic`, `profile` and `email` in the client's default
    client scopes, and `offline_access` in the optional client scopes. Set
    `oidc.scopes=openid profile email`." Remove "and the `offline_access` role" from "Give each user an email address and the
    `offline_access` role" as a requirement; say that the role is not used. Change "is a complete example of this setup" to
    "is an example of this scope setup. It keeps the Keycloak default SSO timeouts." Add the
    § 3.5.1 paragraph.
- `charts/paigasus/values.yaml:184-195` (the `oidc.scopes` comment, lines 188 and 191): the text "It should contain
  offline_access" and "Set it only when your IdP needs it" get one clause: "on Keycloak, leave out
  offline_access to keep SSO (RUNBOOK-chart § 6)".
- `charts/paigasus/README.md:278-281` (the `oidc.scopes` bullet): the same clause.
- `ts/packages/paigasus-auth/tests/e2e/logout.spec.ts:31` and `constants.ts:35`: the D5 clause.

#### 3.5.1 The runbook paragraph "Keycloak: online tokens and session length"

It states these facts, then the Q2 recommendation (§ 11): approach B (item 8) when no other
application of the realm needs SSO; D2 on the default timeouts, or D2 with raised realm timeouts
(item 4), when one does.

1. With D2, each console refresh needs a live Keycloak SSO session. So the console session ends
   when the SSO session ends.
2. The Keycloak defaults are `SSO Session Idle` 30 min and `SSO Session Max` 10 h. The console
   session is 8 h idle and 24 h absolute. The chart does not expose the two console values, so
   chart operators cannot change them.
3. So on the Keycloak defaults, D2 SHORTENS console sessions: an idle user is signed out after
   30 min, and every user after 10 h. D2 then gives SSO to other applications of the realm, and
   nothing more. It does not remove the password prompt at console expiry, because the SSO
   session has already ended by then.
4. `SSO Session Idle` and `SSO Session Max` are realm-wide. Raising them to the console values
   extends the SSO session of every application in the realm, and extends the life of a stolen
   `KEYCLOAK_IDENTITY` cookie and of an unattended SSO session.
5. `Client Session Idle` and `Client Session Max` (realm or client level) can cap the console
   session lower than the SSO session. Leave them unset, or set them to at least the SSO values.
6. Online sessions survive a Keycloak restart only with persistent user sessions (the default since
   Keycloak 26; operators can turn it off). Without them, a restart signs out every console user at
   the next refresh.
7. A logout in another application of the realm, or an admin "sign out" of the user, ends the
   console session at the next refresh. With offline tokens, it does not.
8. The alternative is approach B: keep the default scopes and `offline_access` as a default client
   scope. Console sessions then last 8 h / 24 h, independent of the SSO timeouts, and no SSO
   session is kept.

### 3.6 Rollout and migration

SMA-681 recorded on 2026-09-25 that no deployment exists. This spec assumes that this is still
true, so there is **no migration** (Q1). If a deployment exists, the order does not matter for
safety: `oidc.scopes` alone changes nothing (the default scope still grants `offline_access`), and
the realm change alone changes nothing (the console still requests `offline_access`). The change
takes effect only when both parts are done. An existing session keeps its offline refresh token
until the next login; M9 measures that its refresh still works with the new refresh `scope`.
M9 (§ 13): the refresh returned 200.

The production realm (issue "Decide" item 3): the repository holds no production realm. The only
realm guidance for operators is RUNBOOK-chart § 6, which this spec changes. See Q1.

## 4. What does not change

- `src/runtime.ts`, `src/config.ts`, `src/adapters/oidc.ts`, `src/http/routes.ts`: no code change
  in `@paigasus/auth`. The logout order (revoke, then end-session) does not change (§ 3.3.1).
- The chart templates. `oidc.scopes` exists since SMA-692.
- `rs/crates/services/paigasus-iam/tests/fixtures/keycloak-realm.json`: a password-grant client,
  not the code flow.
- The J2 journey (`zone-round-trip.spec.ts`).

## 5. Acceptance criteria

1. After a console login on the kind stack, a `prompt=none` authorization request with the
   captured Keycloak cookies, in a new browser context, returns a code with no login form (J1
   step 4). This is issue AC 1.
2. After "Sign out", the same request with the same captured cookies returns
   `error=login_required` (J1 step 7). Together with step 4, this is issue AC 2. M6 showed that
   the revoke ends the SSO session (§ 13), so this AC is: the console logout sequence
   ends the IdP session (§ 3.3.1).
3. After "Sign out", Keycloak shows no confirmation page. The end-session response is a 3xx, and
   the browser goes directly back to `/iam/` (J1 step 5, D8). This is the Linear comment of
   2026-09-25. M6 returned `login_required` (§ 13), so this does not also prove SMA-681 AC 1 with a
   live session.
4. `EXPECTED_STEPS` lists the seven J1 titles, and `node --test ci/kind/journeys-report.test.mjs`
   passes with the § 3.4 changes.
5. The `chart` workflow (`kind` job) passes on the PR, with the seven J1 steps in its report.
6. `repo:helm-render` passes (row 7 renders the new `a.yaml`).
7. The package README, RUNBOOK-chart, `values.yaml` and the chart README no longer say that
   Keycloak needs `offline_access` for a refresh token. The README and RUNBOOK-chart describe D2
   and the § 3.5.1 facts.
8. The package default scope list is unchanged, and the `@paigasus/auth` e2e suite passes with no
   change to its realm.

## 6. Test strategy

### 6.1 Measurements before code (Keycloak 26.7, the digest from `ci/kind/manifests/keycloak.yaml`)

SMA-681 measured on 26.4.7. The kind job now pins 26.7. Repeat the SMA-681 method: `docker run`
of the pinned image, `start-dev --import-realm`, a scratch copy of the NEW realm (§ 3.1) with the
secrets filled and `http://localhost:9999/*` added, curl with one cookie jar per user agent, scope
`openid profile email`. No repository file changes. Record the results in a `## 13.
Measurements` section of this spec.

| Row | Request | Expected | Decides |
|---|---|---|---|
| M1 | The code exchange | a refresh token with `typ: Refresh`, not `Offline` | D2 |
| M2 | A `prompt=none` `/auth` request right after M1, the same jar | 302 to the redirect URI with `code` | AC 1 |
| M3 | `grant_type=refresh_token` with `scope=openid profile email` | 200 | § 3.2; stop if not 200 |
| M4 | Logout with the hint and NO revoke, then M2 again | 302 with `error=login_required` | step 7 |
| M5 | Control: the UNCHANGED realm and the old default scope, then M2 | 302 with `error=login_required` (the defect, reproduced on 26.7) | stop rule below |
| M6 | New login; revoke the online refresh token (RFC 7009, as `bestEffortRevoke` does); then M2 in the same jar | not known | § 3.3.1 |
| M7 | After M6: end-session with NO hint | 200 page (session alive) or 302 (session ended) | § 3.3.1 cross-check |
| M8 | Two jars, two logins of the same user; revoke jar 1's refresh token; M2 in jar 2 | code expected | R6 (cross-device) |
| M9 | Unchanged realm: log in with `offline_access`, keep the offline refresh token; move `offline_access` to optional with the admin API; refresh that token with `scope=openid profile email` | record the status | § 3.6 |
| M10 | Log in first to a second scratch client (online scopes only), then to the console client with `offline_access` in the same jar; then M2 for the second client | record | R2, approach B |
| M11 | Probe URI: on the kind stack, `page.goto` of a `prompt=none` URL with `redirect_uri=/kind-sso-probe` | chain[0] 302 from the IdP, chain[1] the probe URI (status recorded, 404 expected) | D6 |

Stop rules:

- If M5 does not reproduce the defect on 26.7, stop and report: the defect can be gone in 26.7,
  and the design then needs a new decision.
- If M3 is not 200, stop and report: the D2 refresh `scope` needs a new decision.
- If M8 returns `login_required`, a console logout on one device ends the SSO session on another
  device. Record it in R6 and in the runbook paragraph (§ 3.5.1); it is not a stop.

### 6.2 Proof that the new steps bite

- Step 7 bites: run J1 with step 7's assertion copied to the end of step 4 (before logout). It must
  fail with a `code` in the `Location`. Remove the copy after.
- Step 4 bites: run J1 with the OLD realm and no `oidc.scopes` (the state of `main`). Step 4 must
  fail, at the cookie check (expected) or at the `code` check. Record which one. This is the
  red-first run of the defect.
- The cookie replay needs no separate bite run. Step 4 is its control: a new context that holds
  ONLY the two captured cookies gets a code. So the replay carries the SSO session, and step 7's
  `login_required` with the same values can come only from the server (D6 (b)).
- D8 bites, only if M6 returns a code (§ 3.3.1): a local mutation removes `id_token_hint` in
  `handleLogout`, and also disables the hint assertions of step 5
  (`auth-roundtrip.spec.ts:136-142` today). Step 5 must fail with the D8 message, not with a
  timeout and not with the hint message. Run it once if the kind stack is available, and record
  the result. If M6 returns `login_required`, record that D8 cannot bite (§ 3.3.1).
  Recorded: M6 returned `login_required` (§ 13), so the D8 mutation cannot bite (§ 3.3.1). It is
  not run.

### 6.3 Unit and gate runs

- `node --test ci/kind/journeys-report.test.mjs`.
- `repo:helm-render` (row 7).
- `iam-console-ts:typecheck` and `ts:fmt` for the journey file (a separate Prettier gate).
- The `chart` workflow on the PR. It is not a required check, so read its result by hand.
- The full `moon ci` target list of the root `CLAUDE.md` before the push.

## 7. Files expected to change

| File | Change |
|---|---|
| `ci/kind/realm/paigasus-realm.json` | `offline_access` from default to optional client scopes; the probe redirect URI |
| `ci/kind/values/a.yaml` | `oidc.scopes: "openid profile email"` and a comment |
| `ts/apps/iam-console/tests/cluster/journeys/auth-roundtrip.spec.ts` | `silentAuthorize`, steps 4 and 7, D8 assertions, step 1 return value, header comment |
| `ci/kind/journeys-report.mjs` | `EXPECTED_STEPS` for J1 |
| `ci/kind/fixtures/journeys-report/pass.json`, `skipped.json` | the two new J1 steps |
| `ci/kind/journeys-report.test.mjs` | the early-return test title and its expected count (lines 86-87) |
| `ts/packages/paigasus-auth/README.md` | the corrected section and table row |
| `ts/packages/paigasus-auth/tests/e2e/logout.spec.ts`, `constants.ts` | D5 comment clause |
| `docs/ops/RUNBOOK-chart.md` | the Keycloak exception (lines 37, 157, 253-264), example 1 (lines 440-460), § 3.5.1 paragraph |
| `ci/kind/manifests/keycloak.yaml` | header comment (lines 8-12): it still names 26.4; say that the kind job pins 26.7 and the package e2e pins 26.4 |
| `charts/paigasus/values.yaml`, `charts/paigasus/README.md` | one clause each |

`ci/kind/README.md` does not describe the realm scopes (only line 69 names the realm ConfigMap), so
it does not change.

## 8. Residuals

- R1: Nothing gates a Keycloak deployment that keeps `offline_access`. That deployment works, but
  it has no SSO (§ 1). The runbook is the only control (D10). A deferred, IdP-neutral option: the
  `scope` field of the token response (RFC 6749 § 5.1) shows whether `offline_access` was granted.
  A boolean on the `session.created` log event would make R1 visible in the logs with no guess of
  the IdP product. It is not in this spec because it is a package code change for a
  documentation-level issue; it is a candidate follow-up issue.
- R2: The Keycloak mechanism that removes the online session is not known (§ 1). A later Keycloak
  version can change it. M5 re-measures it on 26.7, and M10 checks one hypothesis. The J1 step 4
  control catches a change in one direction only. M10 (§ 13): the SSO session of the second client
  in the same jar survived a console login with `offline_access`. The console login in M10 probably
  reused the live SSO session (not shown in the log), so M10 is not the M5-type test of a fresh
  offline login.
- R3: With D2, the console session length is capped by the Keycloak SSO session and by any client
  session caps (D9, § 3.5.1). On the defaults, an idle user is signed out after 30 min. The runbook
  says so.
- R4: The package e2e suite still runs with no SSO session (D5). Its logout test cannot prove the
  D8 fact; J1 does, within the limits of § 3.3.1.
- R5: With D2, a Keycloak restart without persistent user sessions, a logout in another application
  of the realm, or an admin sign-out ends the console session at the next refresh (§ 3.5.1 items 6
  and 7).
- R6: Keycloak revocation can act on every session of the user for the console client. If M8 shows
  it, a console logout on one device ends the SSO session on another device. Measured (§ 13, M8): no.
  Jar 2 kept its SSO session.
- R7 (M6 returned `login_required`, § 13): the end-session path of logout is not proven end to end
  on the kind stack (§ 3.3.1).

## 9. Out of scope

- A change of the package default scope list (D1).
- An IdP-product value in the chart (approach C).
- SSO timeout values in the kind realm (D3, D9).
- A chart value for `PAIGASUS_SESSION_TTL_SECONDS` or `PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS`.
- The `session.created` scope signal (R1).
- The Entra ID, Auth0 and Okta behaviour of SSO with `offline_access`. Not measured.

## 10. Notes for the implementer

- Do not use `context.request` or any `APIRequestContext` against `console.paigasus.test` or
  `idp.paigasus.test` (`playwright.config.ts:42-46`).
- Do not use `redirect_uri=/iam/auth/callback` for the probe (D6 (c)).

## 11. Open questions

- Q1 (**decided 2026-10-04: no deployment exists**, so there is no migration and M9 stays
  informative): Does a deployment, or a production (non-kind) Keycloak realm, exist outside this
  repository? SMA-681 said none existed on 2026-09-25, and § 3.6 assumes "no migration". The issue
  asks to check "the production realm configuration". The repository holds only the kind realm,
  the package e2e realm and the IAM test realm. If a real realm exists, it needs the D2 change by
  hand, and M9 becomes a required measurement, not an informative one.
- Q2 (**decided 2026-10-04: the proposal below.** The runbook states (i), (ii) and (iii) with
  their facts. It recommends (iii) for operators who need no SSO for other applications, and (i) or
  (ii) for those who do): what does the runbook recommend to Keycloak operators?
  (i) D2 on the default timeouts: SSO for other applications, but console sessions end after
  30 min idle and 10 h max. (ii) D2 and raise `SSO Session Idle` and `SSO Session Max` realm-wide
  to 8 h and 24 h: console sessions as today, SSO for all, and a longer SSO session for every
  application of the realm (a security trade-off). (iii) Approach B: keep `offline_access`, no SSO,
  console sessions as today. This spec's default proposal: state all three with their facts, and
  recommend (iii) for operators who do not need SSO for other applications, (i) or (ii) for those
  who do. The kind job uses D2 in every case.
- Q3: D4 keeps the `offline_access` role on the kind user. Two stricter variants would make an
  accidental `offline_access` request fail loudly instead of silently losing SSO: (a) remove the
  role ("Offline tokens not allowed"; needs one more measurement of the default-role assignment);
  (b) remove `offline_access` from the client's scopes completely, so Keycloak may refuse the
  request with `invalid_scope` (not verified). Both close R1 for the kind job but turn a silent SSO
  loss into a login outage. Is that trade wanted?

## 12. Challenge changelog

Verdict of the spec-challenger (Opus), round 1: **NEEDS REWORK**. Every finding was checked
against the repository before it was folded.

Folded:

- BLOCKER D6 (`context.request` cannot resolve the kind hosts): confirmed at
  `playwright.config.ts:42-46` and `phase-a/sso.spec.ts:34-35`. D6 now uses a browser navigation
  in a new context with the replayed Keycloak cookies and a dedicated probe redirect URI (a
  combination of the critique's options (a) and (b)). The `txn_missing` path was confirmed at
  `server.ts:125-128`. The ingress routes only the zone base paths (`ingress.yaml:39`), so the
  probe URI reaches no console. M11 measures it.
- MAJOR revoke before end-session: confirmed at `routes.ts:541`. Added § 3.3.1, M6, M7, and the
  conditional restatement of D8, AC 2, AC 3 and residual R7.
- MAJOR the D8 mutation cannot reach the D8 assertion: D8 now sits right after the end-session
  request is observed, before the hint assertions and before `await back`, with a named message.
  The mutation also disables the hint assertions and must fail with the D8 message.
- MAJOR step 7 cannot tell a server-side end from a cookie clear: steps 4 and 7 replay the
  captured Keycloak cookies in a new context. § 6.2 records that step 4 is the replay control.
- MAJOR D9 is a realm-wide security change, and consequence 1 is not fixed on defaults: confirmed
  that the chart exposes no session TTL (`grep` under `charts/`) and that the package defaults are
  8 h and 24 h (`config.ts:102-103`). § 1 consequence 3 is corrected, D9 no longer recommends a
  realm-wide raise, § 3.5.1 states the trade-off, approach B is an operator option, and Q2 is now a
  decision before implementation.
- MINOR checker test and fixtures: confirmed the J1 step counts per fixture and the test at
  `journeys-report.test.mjs:86-87`. § 3.4 now lists each fixture, keeps `early-return.json`, and
  changes the test to `5 step(s) never ran`. The header line 7 change is listed.
- MINOR D3 and D9 conflict: D3 now calls the kind realm an example of the SCOPE setup only, and the
  runbook sentence changes with it.
- MINOR doc list: added `RUNBOOK-chart.md:201-206` and `values.yaml:130-140`.
- MINOR operational changes: § 3.5.1 items 6 and 7, residual R5.
- MINOR wording: § 3.2 no longer states the M3 result as a fact; `runtime.ts:112` corrected.
- MINOR R1 signal: named in R1 as a deferred option, and in § 9.
- QUESTION deployment exists: § 3.6 states "no migration" as an assumption, gives the order, and
  adds M9. Q1 is extended.
- QUESTION Keycloak code path: § 1 states that nobody read it; M10 added; R2 updated.
- QUESTION cross-device revocation: M8 and residual R6.
- QUESTION Q3 variant: added to Q3 as variant (b).

Freshness check, 2026-10-04 (against `main` at `020a6a33`): no decision or fact of §§ 1-3 is
contradicted. Line references moved in RUNBOOK-chart, `values.yaml`, the chart README and
`ingress.yaml`, and were updated. Added: the `Cookie` import, and the `keycloak.yaml` header comment
(it still names 26.4).

Rejected: none. The challenger's option (a) alone (probe URI in context A) was not used, because
it keeps the cookie-clear ambiguity of the step 7 finding; the design uses (a) and (b) together.

## 13. Measurements

Measured on 2026-10-04 by Task 1 of the plan, with
`quay.io/keycloak/keycloak:26.7@sha256:82a77884f3af238beab1e7afd63b5f530e1b5c0590bd7aa60b40a40463e29b2c`
(version line of the run: `Keycloak 26.7.4`), `start-dev --import-realm`, run locally with `docker run`.
The method is the method of the SMA-681 spec § 3: two scratch realms, curl, one cookie jar per
user agent, scope `openid profile email`, the realm default token lifespan. No repository file
changed. The realm `paigasus` is the § 3.1 realm, with the secrets filled, `http://localhost:9999/*`
added to the redirect URIs and `post.logout.redirect.uris`, and a second confidential client
`sma682-second` (the same scopes, no audience mapper) for M10. The realm `paigasus-old` is the
unchanged `ci/kind/realm/paigasus-realm.json` with the same three additions, and no second client.
The revoke is RFC 7009 with no `token_type_hint`, as `bestEffortRevoke` sends it.

| Row | Request | Result | Decides |
|---|---|---|---|
| M1 | The code exchange | status 200, typ `Refresh`, granted scope `openid email profile` | D2 |
| M1 cookies | The jar after the login | `AUTH_SESSION_ID`, `KC_AUTH_SESSION_HASH`, `KEYCLOAK_IDENTITY`, `KEYCLOAK_SESSION` | step 4 cookie names |
| M2 | `prompt=none` right after M1, the same jar | status 302, code=yes, no error | AC 1 |
| M3 | Refresh with `scope=openid profile email` | status 200, typ `Refresh`, id_token yes | § 3.2 |
| M4 | Logout with the hint and no revoke, then M2 again | end-session status 302 (to the post-logout URI, no code); then status 302, code=no, error=`login_required` | step 7 |
| M5 | Unchanged realm, old default scope, then M2 | typ `Offline`; cookies `AUTH_SESSION_ID`, `KC_AUTH_SESSION_HASH`, `KEYCLOAK_IDENTITY`, `KEYCLOAK_SESSION`; status 302, code=no, error=`login_required` | the defect on 26.7 |
| M6 | New login, revoke, then M2 in the same jar | revoke status 200 (token typ `Refresh`); status 302, code=no, error=`login_required` | § 3.3.1 |
| M7 | After M6, end-session with no hint | status 302 (to the post-logout URI); the `grep` count of 0 is not meaningful on a 302 body | § 3.3.1 cross-check |
| M8 | Two jars, revoke jar 1, M2 in jar 2 (and jar 1) | revoke status 200; jar 2: status 302, code=yes, no error; jar 1: status 302, code=no, error=`login_required` | R6 |
| M9 | Unchanged realm, offline token, scope moved by the admin API, refresh | admin statuses 204 (remove default) and 204 (add optional); refresh status 200, no error, typ `Offline` | § 3.6 (informative, Q1) |
| M10 | Second client first, console with `offline_access` second, then M2 for the second client | second client typ `Refresh` (200); console typ `Offline` (200); then status 302, code=yes, no error | R2, approach B |
| M11 | Probe URI on the kind stack | recorded from the PR's `chart` run, see below | D6 |

Branch of § 3.3.1: B: M6 returned `login_required`. M7 is not a cross-check in branch B. It had no
live session, and Keycloak answered 302. That is consistent with the revoke having ended the SSO
session (M6). It does not test whether Keycloak 26.7 shows a confirmation page on a live session
with no hint. SMA-681 rows M-b and M-c measured that on 26.4.7 only.

M11 is covered by the `chart` workflow (`kind` job) on the pull request, not locally. J1 writes one
`prompt=none probe` annotation per probe, and `journeys-report.mjs` prints each annotation in the
job log of the step "Specs, journeys" on a green run. Plan Task 5 copies the two lines here.
