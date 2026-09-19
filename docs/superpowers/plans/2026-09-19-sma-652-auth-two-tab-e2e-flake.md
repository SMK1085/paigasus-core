# SMA-652 § 9.2 two-tab e2e flake — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the § 9.2 test in `ts/packages/paigasus-auth/tests/e2e/roundtrip.spec.ts` deterministic by never reusing a tab that holds a Keycloak document, and prove the shared-session property with checks that a silent SSO re-login cannot satisfy.

**Architecture:** Test-only change. The test closes every Keycloak tab as early as it can, then opens a fresh page in the same browser context and asserts a ONE-hop 200 on `/guarded` with an unchanged `__Host-pgs_sid`. On failure it prints diagnostics to stderr (CI keeps only task stdout/stderr) and attaches them.

**Tech Stack:** Playwright 1.63 (`@playwright/test`), TypeScript (strict, ESLint strict type-checked), Keycloak 26.4 in Docker, Moon 2.5.3, pnpm.

**Spec:** `docs/superpowers/specs/2026-09-19-sma-652-auth-two-tab-e2e-flake-design.md` (approved). Read it before any task. § 2 is the measured root cause; § 3 is the design; § 4 is the verification.

## Global Constraints

- Change ONLY `ts/packages/paigasus-auth/tests/e2e/roundtrip.spec.ts` in committed code. No change to `src/`, Keycloak, the realm fixture, the image tag, or `playwright.config.ts`.
- No retry (`toPass`, `retries`) around the final step. No `context.clock`.
- Never print or attach a cookie VALUE, a `code`, or a `state`. Diagnostics use paths, names, domains, statuses only.
- Every file keeps its SPDX header `// SPDX-License-Identifier: Apache-2.0`.
- Every new `waitFor*` call has an explicit `timeout` (Playwright 1.63 has no default).
- Temporary instrumentation is NEVER committed. Mark every temporary block `// SMA652-TEMP` and remove it by deleting the marked block (not by `git checkout --`, which would also revert uncommitted fix work).
- Commits: conventional, scope `ts`, subject ends `(SMA-652)`, body ends with `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`. Add new commits; never `--amend`. Never `--no-verify`.
- Write reports in ASD-STE100 Simplified Technical English.

## Execution environment (every task)

- Worktree: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-652`, branch `feature/sma-652-auth-two-tab-e2e-flake`. A subagent's FIRST action: `EnterWorktree` with `path` set to that worktree, then `git branch --show-current` must print the branch above. Never touch `/Users/smaschek/dev/paigasus/paigasus-core` (the main checkout; another session uses it).
- Prefix shell commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; export PROTO_REPORTER=text;`.
- Package dir: `ts/packages/paigasus-auth`. Docker must be running (`docker info`). Deps are installed.
- Run everything in the FOREGROUND. Do not end a turn while a background job runs. macOS has no `timeout` binary.
- A Playwright invocation starts Keycloak once (slow, about 1 min). Use `--repeat-each` inside one invocation rather than many invocations.
- The `list` reporter prints a test's `console.log`/`console.error` lines to the terminal. Capture output with `2>&1 | tee <scratchpad>/<name>.log` so counts can be taken from the file. Scratchpad: `/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/dd5a5bb9-0ba2-4da1-8515-9429a7c6567d/scratchpad`.

## File Structure

- Modify: `ts/packages/paigasus-auth/tests/e2e/roundtrip.spec.ts` — the § 9.2 test (lines 90–134 today), its comments, two new module-level helpers (`readSessionCookieValue`, `reportFreshTabFailure`), and the type imports they need. The AC 1 test and `attemptKeycloakLogin` / `fillKeycloakLoginForm` stay unchanged.
- Temporary only (scratchpad and a temporary spec file in `tests/e2e/`, deleted after use): the forced-reproduction instrumentation.

---

### Task 1: Red — forced reproduction on the UNCHANGED test

Proves that the forcing harness reproduces the measured failure before any fix exists. Nothing is committed in this task.

**Files:**
- Temporary modify: `ts/packages/paigasus-auth/tests/e2e/roundtrip.spec.ts` (instrumentation block only)
- Create (scratchpad, kept for Task 3): `<scratchpad>/sma652-old-instrumented.spec.ts`

**Interfaces:**
- Produces: `<scratchpad>/sma652-old-instrumented.spec.ts` — a copy of the ORIGINAL `roundtrip.spec.ts` plus the forcing block, used again in Task 3. The env var contract: `SMA652_FORCE=1` enables forcing; `SMA652_TARGET_MS` (default `1000`) is the timer's fire time in ms since the secondary document's start.

- [ ] **Step 1: Insert the forcing block**

In the ORIGINAL § 9.2 test, directly BEFORE the line
`await secondary.goto(`${ZONE_BASE_PATH}/guarded`);`, insert:

```ts
    // SMA652-TEMP begin — forced reproduction (spec § 4 step 1). Never commit.
    if (process.env.SMA652_FORCE === '1' && tab1Completed) {
      const pageHash = /checkAuthSession\(\s*["']([^"']+)["']/.exec(await secondary.content())?.[1];
      const cookieHash = (await context.cookies()).find((c) => c.name === 'KC_AUTH_SESSION_HASH')?.value;
      const norm = (v: string | undefined): string | undefined => (v === undefined ? undefined : decodeURIComponent(v));
      const loser = pageHash !== undefined && cookieHash !== undefined && norm(pageHash) !== norm(cookieHash);
      const sinceDocStart = await secondary.evaluate(() => performance.now());
      const target = Number(process.env.SMA652_TARGET_MS ?? '1000');
      const lead = 5 + Math.random() * 25;
      const waitMs = Math.max(0, target - lead - sinceDocStart);
      let actionAt = Number.POSITIVE_INFINITY;
      secondary.on('request', (r) => {
        if (r.isNavigationRequest() && r.url().includes('/protocol/openid-connect/auth')) {
          const rel = Date.now() - actionAt;
          console.log(`SMA652 reload ${Number.isFinite(rel) ? `${rel >= 0 ? 'AFTER' : 'BEFORE'} action by ${String(Math.abs(rel))}ms` : 'BEFORE action'}`);
        }
      });
      await new Promise((resolve) => setTimeout(resolve, waitMs));
      console.log(`SMA652 classify loser=${String(loser)} pageHashFound=${String(pageHash !== undefined)} waitMs=${waitMs.toFixed(0)} lead=${lead.toFixed(0)}`);
      actionAt = Date.now();
    }
    // SMA652-TEMP end
```

Notes: `secondary.content()` and `performance.now()` only read the page. `rel` is logged in ms only; no URL, `state` or cookie value is printed. If `pageHashFound=false` on every run, inspect one Keycloak page source once (`await secondary.content()` into a scratch file) and fix the regex; do not guess.

- [ ] **Step 2: Typecheck the instrumented file**

Run (package dir): `pnpm exec tsc --noEmit -p tsconfig.json`
Expected: exit 0. (If the package tsconfig does not include `tests/`, instead run `pnpm exec playwright test --list tests/e2e/roundtrip.spec.ts`, which compiles the file; expected: 2 tests listed.)

- [ ] **Step 3: Run the "before" batch**

Run (package dir):
`SMA652_FORCE=1 pnpm exec playwright test tests/e2e/roundtrip.spec.ts -g "9.2" --repeat-each=40 2>&1 | tee <scratchpad>/t1-before-1.log`
Expected: some runs FAIL at the final `toBeVisible` with the secondary on the Keycloak login form.

- [ ] **Step 4: Tally, and tune if needed**

Count: `grep -c "SMA652 classify loser=true" <log>`, `grep -c "reload AFTER action" <log>`, and the failed count from the reporter summary. Requirement: **at least 3 failures** in this session. If fewer: look at the `reload AFTER/BEFORE action by Nms` lines. If most reloads are BEFORE by more than 30 ms, raise `SMA652_TARGET_MS` (for example `1020`, `1040`); if AFTER by more than 30 ms, lower it. Re-run Step 3 with a new log name. Stop after 4 tuning rounds and report if no value reaches 3 failures — do NOT continue to Task 2 in that case.

- [ ] **Step 5: Save the instrumented ORIGINAL file and restore the working tree**

```bash
cp ts/packages/paigasus-auth/tests/e2e/roundtrip.spec.ts <scratchpad>/sma652-old-instrumented.spec.ts
```
Then delete the `// SMA652-TEMP begin` … `// SMA652-TEMP end` block from `roundtrip.spec.ts` with an edit (not `git checkout`). Verify: `git status --porcelain` prints nothing, and `git diff --stat` is empty.

- [ ] **Step 6: Report**

Record: the `SMA652_TARGET_MS` value used, runs, failures, `loser=true` count, reload AFTER/BEFORE counts. No commit.

---

### Task 2: Green — rewrite the § 9.2 test

**Files:**
- Modify: `ts/packages/paigasus-auth/tests/e2e/roundtrip.spec.ts` (import line 13, add helpers after `attemptKeycloakLogin`, replace the whole § 9.2 test at lines 90–134)

**Interfaces:**
- Consumes: `attemptKeycloakLogin(page: Page): Promise<boolean>` (unchanged, same file); constants `SESSION_COOKIE_NAME`, `ZONE_BASE_PATH` from `./constants.js`.
- Produces (used by Task 3's instrumentation): in the new test, the local names `tab1Completed: boolean`, `secondary: Page | null` (null in the tab1-lost mode), `authRequests: string[]`, `fresh: Page`, and the exact statement `if (secondary !== null) await secondary.close();`. The new title contains the substring `one completion signs in the whole context`.

- [ ] **Step 1: Update the imports**

Replace line 13:
```ts
import { expect, test, type Page } from '@playwright/test';
```
with:
```ts
import { expect, test, type BrowserContext, type Page, type Request, type Response, type TestInfo } from '@playwright/test';
```

- [ ] **Step 2: Add the two helpers directly after `attemptKeycloakLogin`**

```ts
/** The session cookie's value, or `undefined`. The value is compared, never printed. */
async function readSessionCookieValue(context: BrowserContext): Promise<string | undefined> {
  return (await context.cookies()).find((c) => c.name === SESSION_COOKIE_NAME)?.value;
}

/** SMA-652: explains a failed fresh-tab check. CI keeps only the task's stdout and stderr (no
 * Playwright attachment or trace survives the runner), so the same text goes to stderr AND to a
 * text/plain attachment. Paths, statuses, cookie NAMES only: a Keycloak URL carries `state`, a
 * callback URL carries `code`, and a cookie value is a credential. */
async function reportFreshTabFailure(
  testInfo: TestInfo,
  context: BrowserContext,
  fresh: Page,
  response: Response | null,
  authRequests: readonly string[],
): Promise<void> {
  const chain: string[] = [];
  let request: Request | null = response?.request() ?? null;
  while (request !== null) {
    const hop = await request.response();
    const url = new URL(request.url());
    chain.unshift(`${String(hop?.status() ?? 0)} ${url.origin}${url.pathname}`);
    request = request.redirectedFrom();
  }
  const finalUrl = new URL(fresh.url());
  const cookies = (await context.cookies()).map((c) => `${c.name} domain=${c.domain} path=${c.path}`);
  const text = [
    'SMA-652 fresh-tab diagnostics',
    `redirect chain (${String(chain.length)} hop(s)): ${chain.length === 0 ? '(no response)' : chain.join(' -> ')}`,
    `final page: ${finalUrl.origin}${finalUrl.pathname}`,
    `cookies (${String(cookies.length)}): ${cookies.join('; ')}`,
    `auth requests after the primary completed (${String(authRequests.length)}): ${authRequests.join(', ') || '(none)'}`,
  ].join('\n');
  console.error(text);
  await testInfo.attach('sma-652-fresh-tab-diagnostics', { body: text, contentType: 'text/plain' });
}
```

- [ ] **Step 3: Replace the whole § 9.2 test**

Replace everything from `test('§ 9.2: two tabs starting a login concurrently both complete', …` to the end of the file with:

```ts
test('§ 9.2: two concurrent logins mint distinct txn cookies, and one completion signs in the whole context', async ({ context }, testInfo) => {
  const tab1 = await context.newPage();
  const tab2 = await context.newPage();

  // Both tabs START a login CONCURRENTLY — this is the property § 9.2 is actually about: each
  // `/auth/login` call mints its OWN __Host-pgs_txn_<id> cookie (routes.ts's per-transaction
  // cookie, never a single fixed name), so two logins racing does not let one overwrite the
  // other's secret. Asserted directly below, against real Keycloak, rather than inferred.
  await Promise.all([tab1.goto(`${ZONE_BASE_PATH}/guarded`), tab2.goto(`${ZONE_BASE_PATH}/guarded`)]);

  // m1 (fix round 1): this is the SINGLE assertion in this test that reds if routes.ts regressed
  // to a fixed transaction-cookie name instead of `txnCookieName(txnId)` — a fixed name would
  // still let one tab's login complete (the second /auth/login would just overwrite the first
  // cookie, and whichever transaction is left standing can still finish), so the completion
  // logic below this point does NOT independently catch that regression. Do not "simplify" this
  // count away believing the rest of the test covers it.
  const txnCookiesAfterBothStarted = (await context.cookies()).filter((c) => c.name.startsWith('__Host-pgs_txn_'));
  expect(txnCookiesAfterBothStarted, 'two concurrent /auth/login calls must mint two DISTINCT txn cookies, not one overwriting the other').toHaveLength(2);

  // MEASURED while building this test: Keycloak 26.4 (dev mode) shares ONE `KC_RESTART` /
  // `AUTH_SESSION_ID` cookie pair across the WHOLE browser context, not one per tab, so two tabs
  // that both just navigated to its `/auth` endpoint race for that single shared slot, and
  // NONDETERMINISTICALLY either tab's login form can point at a session Keycloak no longer
  // recognises ("Your login attempt timed out"). That is Keycloak's session model, not this
  // package's; the txn-cookie count above is what proves § 9.2. What follows completes login on
  // WHICHEVER tab is still valid.
  //
  // SMA-652 (measured on Keycloak 26.4.7; global-setup.ts uses the floating tag `keycloak:26.4`):
  // a Keycloak login page must never stay open, and never be reused, once the other login has
  // completed. The two pages also race for `KC_AUTH_SESSION_HASH`; the losing page's
  // `authChecker.js` (`checkAuthSession`, one-shot, 1000 ms after load) calls `location.reload()`
  // on the mismatch, and `beforeunload` does not cancel that timer — so a later `goto` on that tab
  // can reach /guarded (200) and then be overridden by the reload, leaving the tab on the Keycloak
  // form. An open login page also polls every 2 s and may follow the SSO session into
  // /auth/callback, where `txn_missing` -> /auth/login would delete the shared session. So each
  // Keycloak tab is closed as soon as the test no longer needs it, and the shared-session
  // property is proved on a FRESH page, which no Keycloak document can navigate.
  // Evidence: docs/superpowers/specs/2026-09-19-sma-652-auth-two-tab-e2e-flake-design.md.
  const tab1Completed = await attemptKeycloakLogin(tab1);
  testInfo.annotations.push({ type: 'sma-652-mode', description: tab1Completed ? 'tab1-won' : 'tab1-lost' });
  let primary: Page;
  let secondary: Page | null;
  if (tab1Completed) {
    primary = tab1;
    secondary = tab2;
  } else {
    await tab1.close();
    const tab2Completed = await attemptKeycloakLogin(tab2);
    expect(tab2Completed, 'at least one of the two concurrently-started logins must complete on its first Keycloak submission').toBe(true);
    primary = tab2;
    secondary = null;
  }
  await expect(primary.getByTestId('guarded-heading')).toBeVisible();

  const pageLabels = new Map<Page, string>([[primary, 'primary']]);
  if (secondary !== null) pageLabels.set(secondary, 'secondary');
  const authRequests: string[] = [];
  context.on('request', (request) => {
    if (!request.isNavigationRequest()) return;
    const { pathname } = new URL(request.url());
    if (pathname !== `${ZONE_BASE_PATH}/auth/login` && pathname !== `${ZONE_BASE_PATH}/auth/callback`) return;
    authRequests.push(`${pageLabels.get(request.frame().page()) ?? 'other'} ${pathname}`);
  });

  const sidBefore = await readSessionCookieValue(context);
  expect(sidBefore !== undefined, 'the primary login must have set __Host-pgs_sid').toBe(true);
  if (secondary !== null) await secondary.close();

  const fresh = await context.newPage();
  pageLabels.set(fresh, 'fresh');
  const response = await fresh.goto(`${ZONE_BASE_PATH}/guarded`);
  try {
    // The heading alone cannot tell the shared cookie apart from a SILENT SSO re-login
    // (/guarded -> /auth/login -> Keycloak -> /auth/callback -> a NEW session -> /guarded). The
    // one-hop check and the unchanged sid both fail on that path.
    expect(response !== null, 'the fresh tab navigation must produce a response').toBe(true);
    if (response === null) throw new Error('unreachable');
    expect(response.status()).toBe(200);
    expect(response.request().redirectedFrom() === null, 'the fresh tab must reach /guarded in ONE hop, with no login redirect').toBe(true);
    expect(new URL(response.url()).pathname).toBe(`${ZONE_BASE_PATH}/guarded`);
    await expect(fresh.getByTestId('guarded-heading')).toBeVisible();
    const sidNow = await readSessionCookieValue(context);
    expect(sidNow === sidBefore, 'the fresh tab must reuse the SAME __Host-pgs_sid, not a new session').toBe(true);
  } catch (error) {
    await reportFreshTabFailure(testInfo, context, fresh, response, authRequests);
    throw error;
  }
});
```

The old trailing `await tab1.close(); await tab2.close();` is removed on purpose: the per-test `context` fixture closes every page.

- [ ] **Step 4: Typecheck, lint, format**

Run (package dir): `pnpm exec playwright test --list tests/e2e/roundtrip.spec.ts`
Expected: 2 tests listed, the second with the new title.

Run (repo root): `moon run paigasus-auth-ts:typecheck ts:lint ts:fmt`
Expected: all pass. If `ts:fmt` fails, run `pnpm -C ts exec prettier --write packages/paigasus-auth/tests/e2e/roundtrip.spec.ts` and re-run. If ESLint reports a rule on the new code, fix the code; do not add a disable comment without stating why in the report.

- [ ] **Step 5: Normal runs**

Run (package dir): `pnpm exec playwright test tests/e2e/roundtrip.spec.ts --repeat-each=40 2>&1 | tee <scratchpad>/t2-normal.log`
Expected: 80 passed, 0 failed. (The `list` reporter does not print annotations; the mode split is counted in Task 3 from the instrumented runs.)

Run (repo root): `moon run paigasus-auth-ts:test-e2e`
Expected: pass.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-auth/tests/e2e/roundtrip.spec.ts
git commit -m "fix(ts): prove the § 9.2 shared session on a fresh tab, never a Keycloak one (SMA-652)

The losing Keycloak login page reloads itself 1000 ms after load
(authChecker.js checkAuthSession) and that reload could override the
test's final goto. The test now closes each Keycloak tab early, proves
the property on a fresh page with a one-hop and same-sid check, and
prints path-only diagnostics to stderr on failure.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Verification battery (spec § 4 steps 1–4)

Nothing in this task is committed. Every temporary edit is marked `// SMA652-TEMP` and removed at the end.

**Files:**
- Temporary create: `ts/packages/paigasus-auth/tests/e2e/sma652-old.spec.ts` (copy of `<scratchpad>/sma652-old-instrumented.spec.ts`)
- Temporary modify: `ts/packages/paigasus-auth/tests/e2e/roundtrip.spec.ts`

**Interfaces:**
- Consumes: Task 1's `<scratchpad>/sma652-old-instrumented.spec.ts` and its `SMA652_TARGET_MS` value; Task 2's names `tab1Completed`, `secondary`, `authRequests`, `fresh`, and the statement `if (secondary !== null) await secondary.close();`.

- [ ] **Step 1: Forcing block in the FIXED test**

In `roundtrip.spec.ts`, directly BEFORE `if (secondary !== null) await secondary.close();`, insert the SAME block as Task 1 Step 1, with two changes: wrap it in `if (secondary !== null) { … }` (so `secondary` is a `Page`), and keep everything else identical. The "action" is now the close.

- [ ] **Step 2: Interleaved before/after batches**

Copy the old file in: `cp <scratchpad>/sma652-old-instrumented.spec.ts ts/packages/paigasus-auth/tests/e2e/sma652-old.spec.ts`. Then run, in this order, with Task 1's `SMA652_TARGET_MS`:

1. `SMA652_FORCE=1 SMA652_TARGET_MS=<v> pnpm exec playwright test tests/e2e/sma652-old.spec.ts -g "both complete" --repeat-each=30 2>&1 | tee <scratchpad>/t3-before-a.log`
2. `SMA652_FORCE=1 SMA652_TARGET_MS=<v> pnpm exec playwright test tests/e2e/roundtrip.spec.ts -g "one completion" --repeat-each=30 2>&1 | tee <scratchpad>/t3-after-a.log`
3. repeat 1 as `t3-before-b.log`
4. repeat 2 as `t3-after-b.log`

Expected: the before batches together show at least 3 failures (else the after batches do not count — report and stop). The after batches show 0 failures. For each log, tally runs, failures, `loser=true`, reload AFTER/BEFORE counts. The after-fix guarantee is structural (the document no longer exists); this batch checks the implementation.

- [ ] **Step 3: Negative control**

Delete the forcing block from Step 1 (edit). Directly BEFORE `const response = await fresh.goto(`, insert:
```ts
  await context.clearCookies({ name: SESSION_COOKIE_NAME }); // SMA652-TEMP negative control
```
Run: `pnpm exec playwright test tests/e2e/roundtrip.spec.ts -g "one completion" 2>&1 | tee <scratchpad>/t3-negative.log`
Expected: FAIL on the one-hop check. The stderr diagnostics show a multi-hop chain through `/e2e/auth/login`, Keycloak, `/e2e/auth/callback` ending at `/e2e/guarded` (proof the heading alone would have passed via silent SSO), cookie names with NO values, and `fresh /e2e/auth/login, fresh /e2e/auth/callback` in the auth requests. This run also satisfies spec § 4 step 4 (diagnostics visible in the terminal output of the `list` reporter): confirm the lines are in `t3-negative.log`, and grep the log for `code=`, `state=` and the literal cookie-value shape to confirm none leaked (`grep -E "code=|state=" <log>` must print nothing). Remove the line.

- [ ] **Step 4: Polling-path probe**

Directly BEFORE `if (secondary !== null) await secondary.close();` insert:
```ts
  // SMA652-TEMP begin — polling probe (spec § 4 step 3)
  await new Promise((resolve) => setTimeout(resolve, 2000 + Math.random() * 500));
  console.log(`SMA652 probe authRequests=${JSON.stringify(authRequests)}`);
  // SMA652-TEMP end
```
And in the tab1-lost branch, change `await tab1.close();` temporarily to be skipped (comment it with `// SMA652-TEMP`), and set `secondary = tab1;` instead of `null` (also marked `// SMA652-TEMP`), so both modes keep the other Keycloak tab open past its first 2 s poll.
Run: `pnpm exec playwright test tests/e2e/roundtrip.spec.ts -g "one completion" --repeat-each=20 2>&1 | tee <scratchpad>/t3-probe.log`
Tally: runs, failures, and every `SMA652 probe` line whose array contains `secondary`. Report the count exactly. A non-zero count means the polling path is REAL (spec § 6 product question); a zero count means it was not observed in 20 runs.

- [ ] **Step 5: Clean up and verify the tree**

Delete `tests/e2e/sma652-old.spec.ts`. Remove every `SMA652-TEMP` edit from `roundtrip.spec.ts` by editing. Then:
`grep -rn SMA652 ts/packages/paigasus-auth` → no output.
`git status --porcelain` → no output. `git diff HEAD --stat` → empty.
Run once more: `pnpm exec playwright test tests/e2e/roundtrip.spec.ts --repeat-each=5` → 10 passed.

- [ ] **Step 6: Report**

A table per batch (before-a, after-a, before-b, after-b, negative, probe): runs, failures, loser count, reload AFTER/BEFORE, probe secondary-request count. Quote the negative-control diagnostic lines.

---

### Task 4: Follow-ups (controller)

- [ ] **Step 1:** If Task 3 Step 4's count is non-zero, file a Linear issue (team "Sven Maschek", project "Paigasus Polyglot", labels `Bug`, `area:frontend`, related to SMA-652) describing the measured chain from spec § 6. If zero, file nothing (Sven's decision at Gate 1) and state the count in the PR body.
- [ ] **Step 2:** Search the other e2e suites (`ts/packages/paigasus-auth/tests/e2e/{logout,recovery}.spec.ts`, `ts/apps/*/tests/e2e/**`) for (a) a Keycloak/IdP login page left open while another page in the same context completes a login, and (b) a tab reused after it showed a Keycloak page. Use an `Explore` agent. Report hits; a hit becomes a new Linear issue, not scope here.
- [ ] **Step 3:** Update the auto-memory file `paigasus-auth-two-tab-e2e-flake.md` with the root cause, the fix, and the "never reuse a Keycloak tab" rule.
