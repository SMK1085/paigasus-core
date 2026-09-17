# SMA-639 — `waitForHydration`: its own timeout and a message that explains itself

Linear: <https://linear.app/smaschek/issue/SMA-639>
Related: SMA-512 (the pull request whose CI run measured the defect)

## 1. The defect

Both consoles carry the same e2e helper
(`ts/apps/iam-console/tests/e2e/support/login.ts:12-14` and
`ts/apps/gateway-console/tests/e2e/support/login.ts:12-14`):

```ts
export async function waitForHydration(page: Page): Promise<void> {
  await page.locator('html[data-hydrated="true"]').waitFor({ state: 'attached' });
}
```

Playwright 1.63's `locator.waitFor()` defaults to **no timeout**. The wait is therefore
bounded only by the test budget — `timeout: isCI ? 120_000 : 60_000` in both
`playwright.config.ts` files. When hydration does not happen, the helper consumes the whole
budget and then reports the locator, not a cause.

Measured on SMA-512 pull request 4 (#248). Row R4 failed in CI with:

```
Error: locator.waitFor: Test timeout of 120000ms exceeded.
  - waiting for locator('html[data-hydrated="true"]')
  at waitForHydration (support/login.ts:13)
```

The underlying cause was browser CPU starvation on a loaded runner. R4 runs in about one
second locally and passed 5 of 5 under `CI=1` when run alone. It is not a logic defect and
it does not reproduce.

`signIn` calls the helper, so most rows reach it: seven call sites in `gateway-console`,
and `iam-console` carries the same helper. Every future occurrence costs the full test
budget and says nothing.

## 2. Goal and non-goal

**Goal.** Convert an unexplained two-minute timeout into a fast, self-explaining failure.

**Non-goal.** This does not reduce flakiness. It changes what a flake costs and what it
reports. Section 8 states the cost of that trade honestly.

## 3. D1 — Structure

Add `tests/e2e/support/hydration.ts` to each app. Its only Playwright import is

```ts
import type { Page } from '@playwright/test';
```

a type-only import, fully erased under `verbatimModuleSyntax`. The module therefore has
**no runtime dependency on Playwright**, which is what makes section 6's unit test possible:
a vitest test can call the helper with a stub object.

`login.ts` re-exports the helper:

```ts
export { HYDRATION_TIMEOUT_MS, waitForHydration } from './hydration';
```

No call site changes. The existing `import { signIn, waitForHydration } from './support/login'`
lines in the spec files keep working, and `signIn` keeps calling it.

The `Page` parameter is narrowed to the surface the helper uses, so the stub in the unit
test satisfies it without constructing a real `Page`:

```ts
type HydrationPage = Pick<Page, 'locator'>;
```

A real `Page` satisfies `Pick<Page, 'locator'>`, so every existing call site still
type-checks.

## 4. D2 — The timeout value

```ts
export const HYDRATION_TIMEOUT_MS = process.env.CI ? 15_000 : 5_000;
```

This **deliberately mirrors each app's `expect.timeout`**, which both `playwright.config.ts`
files already set to `isCI ? 15_000 : 5_000`. That is not an arbitrary number: the config's
own comment states why it was raised, and it describes this exact condition —

> CI ONLY, same reasoning as `timeout` above: a `waitFor`/`toBeVisible` bounded by the 5 s
> default can fail while the page is still hydrating on a loaded CI runner.

So the repo has already calibrated a hydration-aware bound for a loaded CI runner, and this
helper adopts it rather than inventing a second number.

Two consequences, both intended:

* In CI the wait is **1/8 of the test budget** (15 s against 120 s), so it fails fast.
* The truthiness test is `process.env.CI`, byte-identical to the two config files' own
  `!!process.env.CI`. The helper and the config can therefore never disagree about whether
  a run is a CI run.

The value is **not** read from the config at runtime — importing `playwright.config.ts` into
test support would give the helper a runtime Playwright dependency and undo section 3. The
relationship is asserted in a test instead (section 6, case 5).

## 5. D3 — The message

The helper catches, then rethrows with the explanation, preserving the original error twice
over: as `cause`, and quoted in the text so it survives a reporter that does not print
`cause`.

```
React never hydrated: html[data-hydrated="true"] was not attached within <N> ms.
`Providers` sets that attribute in an effect, so its absence means the client bundle did not
run — a 404 on a chunk, a hydration error thrown before the effect, or a browser too
CPU-starved to reach it. Playwright reported: <original message>
```

Three points of design:

* It names **what the attribute means** (`Providers` sets it in an effect), which is the
  fact a reader needs and the original error does not carry.
* It names **the three things its absence implies**, which is the triage the reader would
  otherwise have to reconstruct.
* It wraps **every** error from `waitFor`, not only a timeout. Any failure of this wait is
  worth the context, and no caller branches on Playwright's `TimeoutError` class — all
  call sites simply `await`.

The Playwright trace is unaffected: `trace: 'retain-on-failure'` is per test, not per error.

## 6. D4 — Verification

`tests/unit/hydration.test.ts` per app. The unit tier runs on every pull request that
touches the app (`tests/**/*` is an input of each app's `test` task), and it runs without a
browser, so the failure path is exercised on every run rather than only when the e2e tier
is red.

The test drives a stub page that records what it was asked:

1. **The call.** The selector is `html[data-hydrated="true"]`, the state is `attached`, and
   the `timeout` option equals `HYDRATION_TIMEOUT_MS`. This is what the current helper gets
   wrong — it passes no `timeout` at all.
2. **The resolve path.** A `waitFor` that resolves makes the helper resolve, throwing nothing.
3. **The message.** A `waitFor` that rejects makes the helper throw a message naming the
   attribute, stating that the client bundle did not run, and listing the three causes.
4. **The cause.** The thrown error's `cause` is the original error, and its text quotes the
   original message.
5. **The relationship.** `HYDRATION_TIMEOUT_MS < config.timeout`, importing the app's real
   `playwright.config.ts`. This pins what the issue actually asks for — "shorter than the
   test budget" — rather than pinning a literal, so raising either number alone reds the
   test.

   **Measured, not assumed:** a vitest unit test in `gateway-console` imports
   `../../playwright.config` cleanly (1 test, 596 ms, exit 0) on vitest 5.0.0. If a future
   change makes that import fail, the fallback is a literal comparison against `60_000`,
   the smaller of the two budgets — weaker, and a deliberate downgrade to be noted, not a
   silent substitution.

Cases 1 and 5 are the two that can fail on the current code. Cases 2–4 pin the new
behaviour against a later edit.

## 7. Decisions taken and their reasons

### D5 — Two copies stay two copies

The issue directs this: "Do it in **both** apps' copies; they are the same helper." No
shared e2e-support package exists, and an eight-line helper does not justify creating one —
it would need its own `package.json`, its own Moon project, and a place in every consuming
task's `inputs`.

The duplication is bounded by the per-app test: each app's `hydration.test.ts` asserts that
app's own copy, so a drift in one app reds that app's own tier. There is deliberately **no
single-site gate** (the repo's `repo:*-single-site` pattern) — those exist where a second
copy of a *policy* silently diverges from the one that is enforced. Here both copies are
independently asserted, so a divergence cannot hide.

### D6 — `paigasus-app-shell`'s `loadHydrated` is out of scope

`ts/packages/paigasus-app-shell/tests/e2e/support/recorder.ts:55` waits for the same
attribute, but with `await expect(page.locator('html[data-hydrated="true"]')).toHaveCount(1)`.
That is bounded by the **expect** timeout, not the test budget — 5 s there, since that
package's `playwright.config.ts` sets no `expect` override — and it already fails with an
expect-style error naming the locator and the expected count.

It does not have this defect. Pulling it in would widen the pull request into a package
whose e2e tier runs with `retries: 0`, for no measured problem.

## 8. What this costs

Making the wait fail at 15 s can turn a pass into a failure: a starved CI runner that would
have hydrated at 20 s now fails where it previously passed. That is the deliberate trade,
and three things bound it.

* CI runs `retries: 2`, so a single starved attempt does not red the run.
* A test that needs a retry is reported **FLAKY**, not PASSED, so the condition stays
  visible rather than being hidden by the retry.
* The one occurrence actually measured never recovered inside 120 s, so there is no
  evidence of a real hydration that lands between 15 s and 120 s.

If such an occurrence appears, the answer is to raise `HYDRATION_TIMEOUT_MS` with the
measurement recorded — not to remove the bound.

## 9. Files

| File | Change |
|------|--------|
| `ts/apps/iam-console/tests/e2e/support/hydration.ts` | new |
| `ts/apps/iam-console/tests/e2e/support/login.ts` | drop the helper, re-export it |
| `ts/apps/iam-console/tests/unit/hydration.test.ts` | new |
| `ts/apps/gateway-console/tests/e2e/support/hydration.ts` | new |
| `ts/apps/gateway-console/tests/e2e/support/login.ts` | drop the helper, re-export it |
| `ts/apps/gateway-console/tests/unit/hydration.test.ts` | new |

Every file opens with `// SPDX-License-Identifier: Apache-2.0`.

**Scheduling.** `tests/**/*` is an input of each app's `build`, `typecheck`, `test` and
`test-e2e` tasks, so this change selects all four in both apps.
`gateway-console-ts:test-e2e` needs Docker, so a local full run needs a reachable daemon.

## 10. Acceptance

1. Neither `login.ts` defines `waitForHydration`; both re-export it from `hydration.ts`.
2. `waitForHydration` passes an explicit `timeout` to `waitFor`, equal to
   `HYDRATION_TIMEOUT_MS`.
3. `HYDRATION_TIMEOUT_MS` is smaller than the app's Playwright `timeout`, asserted against
   the real config.
4. A rejected wait throws a message naming the attribute, the `Providers` effect, and the
   three causes, with the original error as `cause`.
5. Both apps' `test`, `typecheck` and `lint` tasks pass, and `ts:fmt` is clean.
