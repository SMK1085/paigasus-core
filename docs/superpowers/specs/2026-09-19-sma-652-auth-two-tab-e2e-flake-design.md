# SMA-652: the § 9.2 two-tab e2e test fails intermittently — design

Linear: SMA-652. Path: bounded (one test file). Written as a spec so the pipeline's
adversarial challenge has a document to read.

## 1. Problem

`paigasus-auth-ts:test-e2e` fails intermittently in CI on
"§ 9.2: two tabs starting a login concurrently both complete"
(`ts/packages/paigasus-auth/tests/e2e/roundtrip.spec.ts`). The failing step is the last one:

```ts
await secondary.goto(`${ZONE_BASE_PATH}/guarded`);
await expect(secondary.getByTestId('guarded-heading')).toBeVisible(); // not found after 5 s
```

Any change to `ts/pnpm-lock.yaml` selects this task, so PRs that do not touch
`@paigasus/auth` go red. Known occurrences: runs 35358572907 (`main`), 35371349003,
35426197675 and 35426686584.

## 2. Root cause (measured)

An investigation on 2026-09-19 forced the failure 6 times and recorded 84 passes for contrast.
The counts and one redacted failing sequence are in Appendix A. All 6 failures occurred in the
mode where tab1 wins and the secondary (tab2) is the loser.

1. Both tabs load the Keycloak login page at the same time. Each response sets
   `KC_AUTH_SESSION_HASH`, and the last response wins. The other page ("the loser") embeds a hash
   that no longer matches the cookie.
2. Keycloak 26.4.7's login template (`theme/keycloak.v2/login/template.ftl:119-127`) calls
   `checkAuthSession(hash)`. `theme/base/login/resources/js/authChecker.js` runs it ONCE,
   1000 ms after load (`AUTH_SESSION_TIMEOUT_MILLISECS`). On a mismatch it calls
   `location.reload()`. The page's `beforeunload` handler clears only the polling interval, not
   this one-shot timeout, so the timer can fire after a navigation away has started.
3. When tab1 wins and becomes primary, tab2 (the loser) is the secondary tab. If tab2's timer
   fires 6–29 ms after `secondary.goto('/guarded')` starts, the order is:
   - the fixture server answers `GET /e2e/guarded` with **200** and the valid `__Host-pgs_sid`;
   - the old Keycloak document's reload requests `/openid-connect/auth` with the ORIGINAL `state`;
   - the reload overrides the goto. Keycloak returns its login form and expires
     `KEYCLOAK_IDENTITY` / `KEYCLOAK_SESSION` (cause not found; recorded as a fact only);
   - the tab stays on the Keycloak login form, and the assertion times out.
4. In all 6 failures the timer fired inside that window. In all 28 delayed runs where it fired
   BEFORE the goto, the test passed. With no delay, the goto starts about 210 ms after the
   secondary page loads, long before the timer. 80 of 80 undelayed local runs passed, also under
   CPU load. A slow CI runner stretches the primary login (form, token exchange, `/guarded`) to
   about 1 s. That last step is inferred; CI timing was not measured.

Refuted: the secondary tab's callback failing and clearing the session (no `/auth/callback`
request from it; 0 `login.callback_rejected` events in 90 runs), an uncommitted session (the
server returned 200 with the same sid), and a leftover txn cookie (both were cleared before the
goto, and `/guarded` reads only `__Host-pgs_sid`).

**For the measured failure, `@paigasus/auth` behaves correctly. The defect is in the test:** it reuses a tab that still
holds a Keycloak document with a pending timer, and a later `goto` cannot cancel that timer.

A second, UNMEASURED path exists through shared server state (spec challenge, MAJOR 1). A
Keycloak login page that stays open also polls every 2 s (`startSessionPolling`). When a
`KEYCLOAK_SESSION` cookie appears, it can move that tab to Keycloak's "logged in in another tab"
URL. With a valid SSO session, Keycloak can then send that tab to `/e2e/auth/callback` with its
own `state`. The primary callback already cleared every txn cookie (`src/http/routes.ts:310-314`),
so that callback fails with `txn_missing`. `src/server.ts:111-114` redirects it to `/auth/login`,
and `handleLogin` deletes the presented session and clears `__Host-pgs_sid`
(`src/http/routes.ts:188-196`). The investigation saw 0 such callbacks in 90 runs. Closing a tab
stops navigation of the tab under test. It does not stop an OPEN tab from changing shared state
first. § 3.1 therefore closes each Keycloak tab as early as the test allows, and § 3.3 records
these requests.

## 3. Design

Change only `ts/packages/paigasus-auth/tests/e2e/roundtrip.spec.ts`, the § 9.2 test. The test
signature becomes `async ({ context }, testInfo) => …`.

### 3.1 Close each Keycloak tab early; prove the property on a fresh tab

1. The concurrent start and the txn-cookie count assertion (`toHaveLength(2)`, the m1 guard)
   stay unchanged. They are the test's primary proof of § 9.2.
2. **Tab1-lost mode:** when `attemptKeycloakLogin(tab1)` returns `false`, close tab1 at once,
   BEFORE `attemptKeycloakLogin(tab2)`. The test does not use tab1 after that point. This removes
   the mode where a Keycloak page stays open longest (more than 5 s).
3. **Tab1-wins mode:** after the primary shows the guarded heading, close the secondary tab at
   once.
4. Before any close in step 3, read the value of `__Host-pgs_sid` from `context.cookies()` and
   keep it in a local variable. Never print it.
5. Open a new page in the SAME context: `const fresh = await context.newPage()`. Then
   `const res = await fresh.goto(`${ZONE_BASE_PATH}/guarded`)`.
6. Assert, in this order:
   - `res` is not `null`, and `res.status()` is 200;
   - `res.request().redirectedFrom()` is `null` — the navigation was ONE hop, with no redirect
     through `/auth/login`, Keycloak or `/auth/callback`;
   - `new URL(res.url()).pathname` is `${ZONE_BASE_PATH}/guarded`;
   - `guarded-heading` is visible;
   - the `__Host-pgs_sid` value in the context is EQUAL to the value from step 4. Compare it as a
     boolean (`expect(sidNow === sidBefore, '…').toBe(true)`), so a failure prints no value.

Why the one-hop and sid checks are needed: the heading and the URL cannot tell "the shared cookie
worked" apart from "a silent SSO re-login". Without a valid sid, the fresh tab goes `/guarded` →
`/auth/login` → Keycloak, which finds the valid `KEYCLOAK_IDENTITY` and redirects at once →
`/auth/callback` → a NEW session → `/guarded` with the heading. The one-hop check and the sid
check both fail on that path, and neither needs the Keycloak origin (its host port is random).

Why no other document can navigate the fresh tab: a closed tab has no document. `newPage()` gives
a page with no opener. Keycloak's origin (https, a mapped port) differs from the fixture origin
(http, port 4319), so a `BroadcastChannel` or `storage` event from a Keycloak page cannot reach
it. The fixture pages contain no script. So the fix does not depend on the content of Keycloak's
scripts.

The property proved: a second tab in the same browser context reaches the authenticated state
through the shared `__Host-pgs_sid`, with no further login. A tab's identity is not part of the
cookie contract, so a fresh tab proves the same property the old step claimed.

### 3.2 Rename the test

The concurrently started second login never completes: a successful callback clears every txn
cookie (`routes.ts:310-314`), and the test now closes its tab. The new title is
"§ 9.2: two concurrent logins mint distinct txn cookies, and one completion signs in the whole
context". The SMA-506 design doc row (`2026-09-09-sma-506-auth-design.md:994`) is a historical
record and is not edited.

### 3.3 Diagnostics that survive CI

CI uploads no Playwright output. Only the task's stdout and stderr reach the `moon-diagnostics`
artifact. So:

- From the moment the primary completes, record every main-frame request to `/auth/login` and
  `/auth/callback` in the context (`context.on('request')`), as tab label + path only.
- Wrap the step 6 assertions in `try { … } catch (e) { report(); throw e; }`. The error is always
  thrown again.
- `report()` writes lines with `console.error` (not cut by the reporter) and also attaches the
  same text as `text/plain`. The lines hold: the fresh tab's redirect chain as path + status (no
  query string — Keycloak URLs carry `state`, callback URLs carry `code`); every cookie in the
  context as name + domain + path, duplicates kept, NO values; the recorded `/auth/login` and
  `/auth/callback` requests.
- Add a `testInfo.annotations` entry `{ type: 'sma-652-mode', description: 'tab1-won' | 'tab1-lost' }`
  on every run, so normal runs report the mode split too.
- Every new `waitFor*` call gets an explicit `timeout` (Playwright 1.63 has no default).

### 3.4 Comments

Rewrite the comment block at lines 107–118 and 127–128. Keep the existing explanation of the
`KC_RESTART` / `AUTH_SESSION_ID` race, because it justifies the "whichever tab is still valid"
logic. Add the timer mechanism (§ 2 steps 1–3) and the polling path, with the Keycloak file names
and the note "measured on 26.4.7; `global-setup.ts` uses the floating tag `keycloak:26.4`". Say
why the test closes tabs instead of reusing them, and why the one-hop and sid checks exist. Keep
it short: this doc holds the evidence.

## 4. Verification

1. **Forced reproduction, anchored on the loser document.** Add TEMPORARY instrumentation (never
   committed) that, in the tab1-wins mode, reads the secondary's
   `performance.getEntriesByType('navigation')[0].domContentLoadedEventEnd` and `performance.now()`,
   and waits until about 5–30 ms before `domContentLoadedEventEnd + 1000 ms`. It classifies each
   run on three points: the secondary is the loser (page hash ≠ `KC_AUTH_SESSION_HASH` cookie);
   a reload request was seen; the reload fired before or after the final action.
   - **Before (unchanged test):** the wait sits before `secondary.goto`. This batch must reproduce
     at least 3 failures in the session, or the "after" batch does not count.
   - **After (fixed test):** the wait sits before `secondary.close()`. Expect 0 failures.
   - Interleave before and after batches on one machine. Report run counts per mode and per
     classification. The after-fix guarantee for the timer is STRUCTURAL (the document no longer
     exists); the batch is a check of the implementation, not the proof.
2. **Negative control for the new checks.** Temporarily run
   `context.clearCookies({ name: SESSION_COOKIE_NAME })` before `fresh.goto`. Expect the heading to
   still show (silent SSO) and the one-hop and sid checks to FAIL. Record the output. Revert.
3. **Polling-path probe.** Temporarily keep the secondary open for 2.0–2.5 s after the primary
   shows the heading (past its first poll). Run at least 20 times and count `/auth/login` and
   `/auth/callback` requests from the secondary. Report the count. If the path is real, the new
   checks fail, and the early close in § 3.1 is what keeps the normal test green. If the count is
   non-zero, file a Linear issue for the product question in § 6.
4. **Diagnostics proof.** Temporarily break a step 6 check. Confirm that the `list` reporter
   output on the terminal (not only the file in `test-results/`) shows the chain, the cookie
   names with no values, and the recorded requests. Revert.
5. **Normal runs.** `pnpm exec playwright test tests/e2e/roundtrip.spec.ts --repeat-each=40`
   passes, and the full `moon run paigasus-auth-ts:test-e2e` passes.
6. `ts:lint`, `ts:fmt` and `paigasus-auth-ts:typecheck` pass.

## 5. Non-goals

- No change to `src/`.
- No change to Keycloak, the realm fixture or its image tag.
- No retry (`toPass`, `retries`) around the final step. A retry would hide a real § 9.2
  regression.
- No `context.clock` control of Keycloak timers. It would change Keycloak's multi-tab behaviour
  under test.
- Why Keycloak expires the SSO cookies on the reload stays unexplained.
- The 5 s budget in `attemptKeycloakLogin` is not changed.

## 6. Follow-ups and open questions

- **Product question (for Sven):** can a real user hit the polling path in § 2? A second tab that
  sits on the Keycloak login form while the first tab signs in could reach `/auth/callback`, get
  `txn_missing`, go to `/auth/login`, and so DELETE the first tab's session, followed by a silent
  SSO re-login. This is not measured. If § 4 step 3 shows it, or Sven wants it examined anyway, it
  becomes its own Linear issue.
- Update the auto-memory entry `paigasus-auth-two-tab-e2e-flake` with the root cause and the fix.
- Search the other e2e suites (`logout.spec.ts`, `recovery.spec.ts`, both console zones) for a
  Keycloak or IdP login page that stays open while ANOTHER page in the same context completes a
  login, and for a tab reused after it showed a Keycloak page. Any hit becomes a new Linear issue.

## Appendix A. Evidence from the investigation (2026-09-19)

| Batch | Delay before the final goto | Runs | Failures |
|---|---|---|---|
| no delay | 0 | 40 | 0 |
| no delay, 24 CPU burners | 0 | 40 | 0 |
| random | 0–2500 ms | 30 | 1 |
| random | 650–950 ms | 30 | 1 |
| random | 780–840 ms | 30 | 4 |

Mode split: tab1-lost 43 of 43 passed; all 6 failures in tab1-wins. Server log for the 90 delayed
runs: 180 `login.started`, 90 `session.created`, 0 `login.callback_rejected`.

Failing sequence (batch 3, run 7), times in ms from test start, query strings reduced to `state`:

| t | Event |
|---|---|
| +59 | both tabs `GET /e2e/guarded` → 302 `/e2e/auth/login` → 302 Keycloak auth |
| +105/+106 | tab2 then tab1 receive `KC_AUTH_SESSION_HASH`; tab1's value wins |
| +259→+300 | tab1 POST → `/e2e/auth/callback` → `/e2e/guarded` 200; both txn cookies cleared |
| +1120 | test: `secondary.goto('/e2e/guarded')` |
| +1122/+1123 | tab2 `GET /e2e/guarded` → 200 (valid sid) |
| +1128 | tab2 `GET …/openid-connect/auth` (the reload from the old document) |
| +1148 | Keycloak 200 login form; `KEYCLOAK_IDENTITY`, `KEYCLOAK_SESSION` expired |
| +6205 | assertion fails; tab2 on the Keycloak login form; `__Host-pgs_sid` unchanged |

