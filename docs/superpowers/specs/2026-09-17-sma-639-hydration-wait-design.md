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

Playwright 1.63's `locator.waitFor()` defaults to **no timeout**, and no config in this
repository sets `use.actionTimeout`. The wait is therefore bounded only by the test budget —
`timeout: isCI ? 120_000 : 60_000` in both `playwright.config.ts` files. When hydration does
not happen, the helper consumes the whole budget and then reports the locator, not a cause.

Measured on SMA-512 pull request 4 (#248). Row R4 failed in CI with:

```
Error: locator.waitFor: Test timeout of 120000ms exceeded.
  - waiting for locator('html[data-hydrated="true"]')
  at waitForHydration (support/login.ts:13)
```

The underlying cause was browser CPU starvation on a loaded runner. R4 runs in about one
second locally and passed 5 of 5 under `CI=1` when run alone. It is not a logic defect and
it does not reproduce.

`signIn` calls the helper, so most rows reach it: **`gateway-console`** has 7 call sites
(5 through `signIn`, 2 direct), **`iam-console`** has 15 (12 through `signIn`, 3 direct).

## 2. Goal and non-goal

**Goal.** Convert an unexplained two-minute timeout into a fast, self-explaining failure.

**Non-goal.** This does not reduce flakiness. It changes what a flake costs and what it
reports. Section 9 states the cost of that trade honestly.

## 3. D1 — Structure

Add `tests/e2e/support/hydration.ts` to each app. Its only Playwright import is a type:

```ts
import type { Page } from '@playwright/test';
```

fully erased under `verbatimModuleSyntax`. The module therefore has **no runtime dependency
on Playwright**, which is what makes section 7's unit test possible.

### The parameter type

The helper does **not** take `Page`, and it does **not** take `Pick<Page, 'locator'>`.

**Measured (tsc, this worktree):** `Pick<Page, 'locator'>` keeps `locator()`'s return type
as the full `Locator` interface, so a plain stub is rejected —
`error TS2322: Type '{ locator: … }' is not assignable to type 'Pick<Page, "locator">'`.
An earlier draft of this spec claimed otherwise; it was wrong, and an implementer following
it would have reached for `as unknown as Locator`, which would have removed tsc from the
stub entirely.

The parameter is a structural type that models both levels:

```ts
type HydrationPage = {
  locator(selector: string): { waitFor(options: { state: 'attached'; timeout: number }): Promise<void> };
};
```

**Measured:** a plain stub and a real `Page` both satisfy it, with no cast and no error —
method-parameter bivariance makes the real `Page` assignable. So every existing call site
keeps type-checking and the unit test needs no escape hatch.

`tsconfig.base.json:11` sets `exactOptionalPropertyTypes`, so the helper must always pass a
real `timeout` and never `timeout: undefined`.

### The re-export

`login.ts` re-exports the **function only**:

```ts
export { waitForHydration } from './hydration';
```

`HYDRATION_TIMEOUT_MS` has no call-site consumer, so the unit test imports it from
`./hydration` directly. No spec file changes.

The existing doc comment at `login.ts:7-11` — why every test waits before a click — **moves
to `hydration.ts`**. It is the reason the helper exists and belongs with it.

### Why it lives under `tests/e2e/support/`

It is e2e support; the unit test importing it is the point, not an accident. **Measured:**
vitest's `exclude: ['tests/e2e/**']` governs *collection*, not the import graph — a
`tests/unit/` test imports a `tests/e2e/support/` module cleanly (1 test, 85 ms, exit 0).

## 4. D2 — The timeout value

```ts
export const HYDRATION_TIMEOUT_MS = 15_000;
```

**One value, not a CI branch.** An earlier draft used `process.env.CI ? 15_000 : 5_000`.
That is wrong, and the reason is the mitigation structure: both configs set
`retries: isCI ? 2 : 0`, so the **local** branch would have been the tighter bound *and* the
one with no retry to absorb it — on the same hardware, running the same full graph CLAUDE.md
tells a developer to run, under the same concurrent load the config comment blames
("Rust compiles plus four `test-e2e` tasks at once", `iam-console/playwright.config.ts:21-23`).

15 s is the number this repo already calibrated for a hydrating page on a loaded runner. Both
configs set `expect: { timeout: isCI ? 15_000 : 5_000 }`, and the comment above it says:

> CI ONLY, same reasoning as `timeout` above: a `waitFor`/`toBeVisible` bounded by the 5 s
> default can fail while the page is still hydrating on a loaded CI runner.

Taking that value unconditionally gives:

| | test budget | this bound | share of budget |
|---|---|---|---|
| CI | 120 s | 15 s | 1/8 |
| local | 60 s | 15 s | 1/4 |

It fails fast in both, is never tighter than the expect timeout the repo already deems
necessary, and it deletes the CI detection along with three problems that came with it: the
untested branch (`process.env.CI` is frozen at module evaluation, so `vi.stubEnv` cannot
reach it and only the current process's branch is ever exercised), the question of whether a
local `moon ci` exports `CI` at all, and a false claim that the helper's test was
"byte-identical" to the config's.

**Not measured:** whether a local `moon ci` sets `CI`. Nothing in `.moon/workspace.yml` or
`.moon/tasks.yml` mentions it, and Moon reads `CI` for `runInCI` rather than setting it — but
this spec does not assert it, because with one value the answer cannot change any outcome.

The value is **not** read from the config at runtime; that would give the helper a runtime
Playwright dependency and undo section 3. The relationship is asserted in a test instead.

## 5. D3 — The message, and what it must not claim

The helper explains a **timeout** and nothing else:

```
React never hydrated: html[data-hydrated="true"] was not attached within 15000 ms.
`Providers` sets that attribute in an effect, so its absence means the client bundle did not
run — a 404 on a chunk, a hydration error thrown before the effect, a browser too CPU-starved
to reach it, or the page rendered outside a `Providers` layout at all — a 404 or an error
boundary. Playwright reported: <original message>
```

The message names **four** causes, not three. `Providers` mounts only in
`app/(console)/layout.tsx` and `app/(public)/layout.tsx` — the root `app/layout.tsx` renders
`html`/`body` only. So a 404 or an error boundary can render without ever going through a
`Providers` layout, and it never sets the attribute either. Omitting this fourth cause would
reintroduce the exact "confident wrong diagnosis" this section bans, on a page that hydrated
fine but simply never mounted `Providers`.

**Every other error is rethrown unchanged.** An earlier draft wrapped all of them. That is a
defect, not a simplification: Playwright rejects a pending `waitFor` with "Target page,
context or browser has been closed" or "Test ended." during teardown, and with a navigation
error on a hard failure. Reporting those as "the client bundle did not run" is a confident
wrong diagnosis, which is worse than the locator dump this issue exists to replace.

The branch is `err instanceof Error && err.name === 'TimeoutError'` — a string comparison, so
it needs no runtime Playwright import. Section 3's constraint is what forces a name check
rather than `instanceof TimeoutError`; that is the compromise, stated rather than hidden.

**To be measured during implementation:** that a `waitFor` which exceeds its *own* `timeout`
option rejects with `name === 'TimeoutError'`. If it does not, the branch is wrong and the
fallback is to also match the message against `/Timeout .* exceeded/`. This must be measured
against a real browser, not assumed.

The error keeps the original as `cause`, and quotes its text so it survives a reporter that
does not print `cause`. The Playwright trace is unaffected: `trace: 'retain-on-failure'` is
per test, not per error.

## 6. Rejected alternative: `expect(locator, message).toBeAttached()`

```ts
await expect(page.locator('html[data-hydrated="true"]'), MESSAGE)
  .toBeAttached({ timeout: HYDRATION_TIMEOUT_MS });
```

Playwright 1.63 supports both `toBeAttached()` and the `expect(value, message)` description
form, and this is already the repository's shape at
`paigasus-app-shell/tests/e2e/support/recorder.ts:55`. It is genuinely simpler: no structural
type, no stub, no error wrapping, and Playwright's own error class, call log and code snippet
all survive.

**It is rejected because it cannot be tested.** The helper would have a runtime Playwright
import, so only a browser run could exercise it — and the failure path never runs when the
e2e tier is green. The whole failure mode this issue describes reached production precisely
because nothing exercised the failure path. The cost of rejecting it is real and stated here:
we lose Playwright's call log, and we take on the `TimeoutError` name check in section 5.

## 7. D4 — Verification

`tests/unit/hydration.test.ts` per app. The unit tier runs without a browser on every pull
request that touches the app, so the failure path is exercised on every run.

The test drives a stub page that records what it was asked:

1. **The call.** The selector is `html[data-hydrated="true"]`, the state is `attached`, and
   the `timeout` option equals `HYDRATION_TIMEOUT_MS`. This is what today's helper gets wrong
   — it passes no `timeout` at all.
2. **The resolve path.** A `waitFor` that resolves makes the helper resolve, throwing nothing.
3. **The message.** A rejected `TimeoutError` makes the helper throw a message that names the
   attribute, the `Providers` effect, and **each of the three causes as a separate assertion**
   — three substring checks, never one whole-string equality against a second copy of the
   literal, which would pass with the message gutted.
4. **The cause.** The thrown error's `cause` is the original error, and its text quotes the
   original message.
5. **The bound, three ways.** All three import the app's real `playwright.config.ts`:
   - `HYDRATION_TIMEOUT_MS === 15_000` — the literal pin. This is also the **only** thing that
     keeps the two apps' values equal; each app's other assertions constrain only its own copy
     against its own config, so without this `iam = 15_000` and `gateway = 30_000` would both
     pass.
   - `HYDRATION_TIMEOUT_MS >= config.expect.timeout` — never tighter than the bound the repo
     already calibrated for a hydrating page.
   - `HYDRATION_TIMEOUT_MS <= config.timeout / 4` — *meaningfully* shorter than the budget, not
     nominally shorter. An earlier draft asserted only `< config.timeout`, an 8× margin, and
     claimed it would red when either number moved. It would not: 15 s → 60 s would have stayed
     green. The `/4` form is exactly tight locally (60 s / 4 = 15 s), so raising the constant at
     all reds it.

   `config.timeout` and `config.expect` are optional in `defineConfig`'s return type and this
   repo is `strict`, so the test **narrows explicitly and fails when either is absent** — never
   skips.

   **Measured:** a vitest unit test imports `../../playwright.config` cleanly (1 test, 596 ms,
   exit 0) in `gateway-console`. `iam-console`'s vitest config aliases `next/cache` in addition,
   so **the implementation must repeat this measurement there** rather than inherit the claim.
6. **The non-timeout path.** A rejection whose `name` is not `TimeoutError` is rethrown
   **unchanged** — same error object, no hydration text. This is the assertion that stops
   section 5's wrong diagnosis from coming back.
7. **No unbounded wait in this app.** A source scan of that app's `tests/e2e/**` asserting no
   `.waitFor(` call omits a `timeout`, following the `tests/unit/e2e-rows.test.ts` precedent.
   This is what stops the defect being reintroduced in a *new* helper in the same app.

Cases 1, 5 and 7 fail on today's code.

## 8. Decisions and residuals

### D5 — Two copies stay two copies

The issue directs it: "Do it in **both** apps' copies; they are the same helper." No shared
e2e-support package exists, and an eight-line helper does not justify creating one — it would
need a `package.json`, a Moon project, and a place in every consuming task's `inputs`.

Case 5's literal pin is what keeps the two values equal, and case 7 keeps each app free of a
new unbounded wait.

**Residual, stated in the repository's own form: nothing gates a third console zone.** CLAUDE.md
records that a third app "repeats the same shape" for the Tailwind guard. A third zone that
copies today's `login.ts` gets the unbounded `waitFor`, ships without a `hydration.test.ts`, and
**nothing reds**. Closing that needs a registry entry of the `TAILWIND_GUARD_INVOCATIONS` kind in
`ci/affected-graph/ci_targets.py`, which would also mean re-baselining that file's 23-key
`EXPECTED_FINDING_KEYS` tuple. That is out of proportion to this change and is deliberately not
done here.

### D6 — `paigasus-app-shell`'s `loadHydrated` stays as it is

`paigasus-app-shell/tests/e2e/support/recorder.ts:55` waits for the same attribute with
`expect(...).toHaveCount(1)` — bounded by the **expect** timeout, 5 s there, since that package's
config sets no `expect` override. It does not have this defect: it is bounded, and it already
fails with an expect-style error naming the locator.

**The contradiction this raises is real and is answered, not dodged.** Section 4 says a 5 s bound
can be too short while hydrating; `app-shell` runs on exactly that bound, with `retries: 0`. The
difference is the page: `app-shell`'s fixture is a minimal Next app with no auth, no IAM client
and no container, and its tier has not produced this flake shape. If it ever does, the same
treatment applies there. Pulling it into this pull request now would widen it into a package with
no retries, for no measured problem.

### `paigasus-auth`'s bounded wait is prior art

`paigasus-auth/tests/e2e/roundtrip.spec.ts:28` already passes `timeout: 5_000` to a `waitFor`.
It is a different package, a different element (a Keycloak heading, not hydration), and it is
already bounded, so it stays as it is. It is cited so a reader knows the repo has two spellings
of this and that only the console helper was unbounded.

## 9. What this costs

Making the wait fail at 15 s can turn a pass into a failure: a runner that would have hydrated at
20 s now fails where it previously passed.

* In CI, `retries: 2` absorbs a single starved attempt, and a retry-dependent pass is reported
  **FLAKY** rather than PASSED, so the condition stays visible. The measured cause was
  concurrent-load CPU starvation, which can persist across retries within one run, so the weight
  here rests on the single measured occurrence never recovering inside 120 s — not on the retry
  mechanism reliably absorbing the condition.
* **Locally there are no retries**, so a local full-graph run can newly red. That is the price of
  the bound, and 15 s (rather than the 5 s of the first draft) is what keeps it small.
* The one occurrence actually measured never recovered inside 120 s, so there is no evidence of a
  real hydration landing between 15 s and 120 s.

**The two-zone tier is the heaviest caller.** `gateway-console/tests/e2e/two-zone-session.spec.ts:71`
and `:74` run in the `two-zone` Playwright project, whose worker fixture starts a Redis container;
Playwright starts a new worker after a failed test, so each CI retry pays a container
teardown-and-restart plus a full login. Line 71 is the first hydration on a cold stack.

Those rows nonetheless keep the same 15 s bound. The container starts in the fixture, **before**
the page load, so it is not inside the hydration wait — a cold two-zone stack does not hydrate more
slowly than a warm one. The retry cost the fixture imposes exists today and is not created by this
change; what changes is that the first attempt gives up after 15 s instead of 120 s. Whether three
15 s-bounded attempts end up cheaper than one 120 s-bounded attempt is unmeasured — a retry restarts
the whole worker fixture (fake IdP, fake IAM, TLS terminator, standalone server, plus a Redis
container in the two-zone project; `tests/e2e/support/harness.ts` budgets 420 s for it in CI), so
the honest claim is only that this change is not obviously more expensive. The bound stands either
way.

If a two-zone row does prove to need a larger bound, the answer is to raise it with the measurement
recorded — not to remove it.

## 10. Files

| File | Change |
|------|--------|
| `ts/apps/iam-console/tests/e2e/support/hydration.ts` | new |
| `ts/apps/iam-console/tests/e2e/support/login.ts` | drop the helper, re-export it, move the comment |
| `ts/apps/iam-console/tests/unit/hydration.test.ts` | new |
| `ts/apps/iam-console/moon.yml` | add `playwright.config.ts` to `test` inputs |
| `ts/apps/gateway-console/tests/e2e/support/hydration.ts` | new |
| `ts/apps/gateway-console/tests/e2e/support/login.ts` | drop the helper, re-export it, move the comment |
| `ts/apps/gateway-console/tests/unit/hydration.test.ts` | new |
| `ts/apps/gateway-console/moon.yml` | add `playwright.config.ts` to `test` inputs |
| `CLAUDE.md` | one bullet: `locator.waitFor()` defaults to no timeout |

Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.

### Why `moon.yml` must change

**Measured.** Each app's `test` task carries `options.merge: replace`
(`iam-console/moon.yml:280`, `gateway-console/moon.yml:249`), so it inherits nothing and lists
every input by hand — and neither list contains `playwright.config.ts`
(`iam-console/moon.yml:235-280`, `gateway-console/moon.yml:210-249`). Without the entry, the one
edit case 5 exists to catch — lowering `timeout:` below `HYDRATION_TIMEOUT_MS` — leaves the
task's cache key unchanged, Moon serves a cached green, and the assertion proves nothing. This is
the staleness class `iam-console/moon.yml:113-119` already records.

### Scheduling, corrected

`tests/**/*` is an input of each app's **`typecheck`, `test` and `test-e2e`** tasks — **not**
`build`. `build` also carries `merge: replace` and its `@group(sources)` is `app/**/*`,
`lib/**/*`, `proxy.ts` only (`gateway-console/moon.yml:33-39`). `build` still runs, as a
scheduled dependency of `test` (`deps: ['~:build']`), not as a selected task.
`gateway-console-ts:test-e2e` needs Docker.

## 11. Acceptance

1. Neither `login.ts` defines `waitForHydration`; both re-export it from `hydration.ts`, and the
   explanatory comment moved with it.
2. `waitForHydration` passes an explicit `timeout` to `waitFor`, equal to `HYDRATION_TIMEOUT_MS`.
3. `HYDRATION_TIMEOUT_MS` is `15_000`, is not below the app's `expect.timeout`, and is at most a
   quarter of the app's Playwright `timeout` — all three asserted against the real config, in both
   apps.
4. A timeout rejection throws a message naming the attribute, the `Providers` effect and the three
   causes, with the original error as `cause`. A non-timeout rejection is rethrown unchanged.
5. No `.waitFor(` under either app's `tests/e2e/**` omits a `timeout`.
6. Both apps' `test`, `typecheck` and `lint` pass, `ts:fmt` is clean, and **both apps'
   `test-e2e` tiers pass — including `gateway-console`'s `two-zone` project**, which is the
   heaviest caller and the one this change can most plausibly disturb.
