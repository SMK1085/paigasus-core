# SMA-639 `waitForHydration` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `waitForHydration` an explicit 15 s timeout and a failure message that explains what hydration means and what its absence implies, in both console apps, with the behaviour asserted by a unit test that runs without a browser.

**Architecture:** The helper moves out of `tests/e2e/support/login.ts` into its own `tests/e2e/support/hydration.ts`, whose only Playwright import is a **type**. That makes the module free of any runtime Playwright dependency, so a vitest unit test can drive it with a plain stub object. `login.ts` re-exports the function, so no call site changes. The helper bounds the wait at a module constant, and converts a `TimeoutError` — and only a `TimeoutError` — into an explanatory error that keeps the original as `cause`.

**Tech Stack:** TypeScript (strict, `verbatimModuleSyntax`, `exactOptionalPropertyTypes`), Playwright 1.63.0, vitest 5.0.0, Moon 2.5.3, pnpm.

**Spec:** `docs/superpowers/specs/2026-09-17-sma-639-hydration-wait-design.md`

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0` on line 1.
- `HYDRATION_TIMEOUT_MS` is **`15_000`** in both apps. One value, no `process.env.CI` branch.
- The selector is exactly `html[data-hydrated="true"]` and the wait state is exactly `'attached'`.
- The helper takes a **structural** parameter type, never `Page` and never `Pick<Page, 'locator'>`. `Pick<Page, 'locator'>` was **measured** to reject a plain stub (`error TS2322`).
- `tsconfig.base.json` sets `exactOptionalPropertyTypes`: never pass `timeout: undefined`.
- Only a rejection whose `name` is `'TimeoutError'` gets the explanatory message. Every other rejection is rethrown **unchanged** — the same error object.
- Relative imports in these apps are **extensionless** (`from './hydration'`, not `'./hydration.js'`).
- Both apps' copies are textually identical apart from nothing. Write one, then copy it.
- Work in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-639-hydration-timeout` on branch `feature/sma-639-hydration-timeout`. Never `git commit --amend`; always add a new commit.
- Prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.

### Commands

From `ts/apps/<app>/` (where `<app>` is `gateway-console` or `iam-console`):

```bash
../../node_modules/.bin/vitest run tests/unit/hydration.test.ts     # one unit file
../../node_modules/.bin/tsc --noEmit -p tsconfig.json               # typecheck
```

From the repo root, for the gate-level runs in Task 6:

```bash
moon run gateway-console-ts:test iam-console-ts:test
moon run gateway-console-ts:typecheck iam-console-ts:typecheck
moon run ts:lint ts:fmt
```

---

## File Structure

| File | Responsibility |
|---|---|
| `ts/apps/gateway-console/tests/e2e/support/hydration.ts` | **new** — the constant, the structural page type, the helper, the message |
| `ts/apps/gateway-console/tests/e2e/support/login.ts` | loses the helper and its comment; gains a one-line re-export |
| `ts/apps/gateway-console/tests/unit/hydration.test.ts` | **new** — seven cases, no browser |
| `ts/apps/gateway-console/moon.yml` | `test` task gains `playwright.config.ts` as an input |
| `ts/apps/iam-console/…` | the same four files, identical content where shared |
| `CLAUDE.md` | one bullet recording the measured Playwright default |

Task order: Task 1 establishes the helper and its unit test in `gateway-console` alone, so every design decision is proven once before it is duplicated. Task 2 measures the `TimeoutError` name against a real browser — the one assumption the spec could not settle on paper. Task 3 copies the proven shape to `iam-console`. Tasks 4–6 are the repo-level obligations.

---

## Task 1: The helper and its unit test, in `gateway-console`

**Files:**
- Create: `ts/apps/gateway-console/tests/e2e/support/hydration.ts`
- Modify: `ts/apps/gateway-console/tests/e2e/support/login.ts:6-14`
- Create: `ts/apps/gateway-console/tests/unit/hydration.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces:
  - `HYDRATION_TIMEOUT_MS: number` — exported from `tests/e2e/support/hydration.ts`, value `15_000`.
  - `waitForHydration(page: HydrationPage): Promise<void>` — exported from `hydration.ts` and re-exported from `login.ts`.
  - `type HydrationPage = { locator(selector: string): { waitFor(options: { state: 'attached'; timeout: number }): Promise<void> } }` — exported from `hydration.ts`; Task 3 uses the identical declaration.

- [ ] **Step 1: Write the failing test**

Create `ts/apps/gateway-console/tests/unit/hydration.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// waitForHydration (SMA-639). The helper is deliberately free of any RUNTIME @playwright/test
// import, so these cases run with no browser and exercise the failure path on every pull request
// — the path an e2e run never reaches while the tier is green.
import { readFileSync, readdirSync } from 'node:fs';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import config from '../../playwright.config';
import { HYDRATION_TIMEOUT_MS, waitForHydration, type HydrationPage } from '../e2e/support/hydration';

type Recorded = { selector: string; options: { state: 'attached'; timeout: number } };

/** A stub page recording what the helper asked for, and resolving or rejecting on command. */
function stubPage(outcome: { reject?: Error } = {}): { page: HydrationPage; calls: Recorded[] } {
  const calls: Recorded[] = [];
  const page: HydrationPage = {
    locator: (selector: string) => ({
      waitFor: async (options: { state: 'attached'; timeout: number }): Promise<void> => {
        calls.push({ selector, options });
        if (outcome.reject !== undefined) throw outcome.reject;
      },
    }),
  };
  return { page, calls };
}

function timeoutError(message: string): Error {
  const error = new Error(message);
  error.name = 'TimeoutError';
  return error;
}

describe('waitForHydration', () => {
  it('waits for the hydration attribute with an explicit timeout', async () => {
    const { page, calls } = stubPage();
    await waitForHydration(page);
    expect(calls).toHaveLength(1);
    expect(calls[0]?.selector).toBe('html[data-hydrated="true"]');
    expect(calls[0]?.options.state).toBe('attached');
    expect(calls[0]?.options.timeout).toBe(HYDRATION_TIMEOUT_MS);
  });

  it('resolves silently when the attribute attaches', async () => {
    const { page } = stubPage();
    await expect(waitForHydration(page)).resolves.toBeUndefined();
  });

  it('explains a timeout: the attribute, the effect, and each of the three causes', async () => {
    const { page } = stubPage({ reject: timeoutError('locator.waitFor: Timeout 15000ms exceeded.') });
    const error = await waitForHydration(page).catch((caught: unknown) => caught);
    expect(error).toBeInstanceOf(Error);
    const message = (error as Error).message;
    // Each cause is asserted SEPARATELY. One whole-string equality would pass against a second
    // copy of the same literal and prove nothing about the message actually shipped.
    expect(message).toContain('html[data-hydrated="true"]');
    expect(message).toContain('Providers');
    expect(message).toContain('client bundle did not run');
    expect(message).toContain('404');
    expect(message).toContain('hydration error');
    expect(message).toContain('CPU-starved');
  });

  it('keeps the original timeout error as the cause and quotes its message', async () => {
    const original = timeoutError('locator.waitFor: Timeout 15000ms exceeded.');
    const { page } = stubPage({ reject: original });
    const error = await waitForHydration(page).catch((caught: unknown) => caught);
    expect((error as Error).cause).toBe(original);
    expect((error as Error).message).toContain('Timeout 15000ms exceeded.');
  });

  it('rethrows a non-timeout rejection unchanged', async () => {
    // Playwright rejects a pending waitFor this way during teardown. Reporting it as "the client
    // bundle did not run" would be a confident WRONG diagnosis.
    const original = new Error('Target page, context or browser has been closed');
    const { page } = stubPage({ reject: original });
    const error = await waitForHydration(page).catch((caught: unknown) => caught);
    expect(error).toBe(original);
    expect((error as Error).message).not.toContain('Providers');
  });

  it('is bounded well inside the test budget and never below the expect timeout', () => {
    // The literal pin. It is also the ONLY thing keeping this app's value equal to iam-console's:
    // the two relational assertions below constrain each app against its own config only.
    expect(HYDRATION_TIMEOUT_MS).toBe(15_000);
    // defineConfig's return type makes both optional and this project is strict, so an absent
    // value must FAIL here rather than skip the assertion.
    const budget = config.timeout;
    const expectTimeout = config.expect?.timeout;
    expect(typeof budget).toBe('number');
    expect(typeof expectTimeout).toBe('number');
    expect(HYDRATION_TIMEOUT_MS).toBeGreaterThanOrEqual(expectTimeout as number);
    expect(HYDRATION_TIMEOUT_MS).toBeLessThanOrEqual((budget as number) / 4);
  });

  it('leaves no unbounded waitFor anywhere in this app e2e tree', () => {
    // What stops the defect returning in a NEW helper in this app. It cannot see a third app —
    // that residual is stated in the spec, § 8 D5.
    const root = fileURLToPath(new URL('../e2e', import.meta.url));
    const files: string[] = [];
    const walk = (dir: string): void => {
      for (const entry of readdirSync(dir, { withFileTypes: true })) {
        const full = path.join(dir, entry.name);
        if (entry.isDirectory()) walk(full);
        else if (entry.name.endsWith('.ts')) files.push(full);
      }
    };
    walk(root);
    expect(files.length).toBeGreaterThan(0);
    const unbounded = files.flatMap((file) =>
      [...readFileSync(file, 'utf8').matchAll(/\.waitFor\(([^)]*)\)/g)]
        .filter((match) => !(match[1] ?? '').includes('timeout'))
        .map((match) => `${path.relative(root, file)}: .waitFor(${match[1] ?? ''})`),
    );
    expect(unbounded).toEqual([]);
  });
});
```

- [ ] **Step 2: Run the test and confirm it fails for the right reason**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/apps/gateway-console && ../../node_modules/.bin/vitest run tests/unit/hydration.test.ts
```

Expected: FAIL — the module `../e2e/support/hydration` does not exist.

**Do not proceed until the failure message is that one.** A different failure means the test file itself is wrong.

- [ ] **Step 3: Create the helper**

Create `ts/apps/gateway-console/tests/e2e/support/hydration.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
import type { Page } from '@playwright/test';

/**
 * Waits until React hydrated the page: `Providers` sets `html[data-hydrated="true"]` in an effect.
 * A click before that is a plain document request, not a client navigation and not a Server Action
 * call (no `Next-Action` header), so every test waits for it before a click.
 */

/**
 * The bound on that wait (SMA-639). ONE value, deliberately not a `process.env.CI` branch: both
 * playwright.config.ts files set `retries: isCI ? 2 : 0`, so a CI-branched constant would have put
 * the TIGHTER bound on the run with NO retry to absorb it.
 *
 * 15 s is the number this repository already calibrated for a hydrating page on a loaded runner —
 * it is both configs' CI `expect.timeout`, and the comment above that setting says why. It is 1/8
 * of the 120 s CI test budget and 1/4 of the 60 s local one, so the wait fails fast in both.
 *
 * Without it, `locator.waitFor()` has NO timeout at all in Playwright 1.63 (no config here sets
 * `use.actionTimeout`), so the wait consumes the whole test budget and then reports the locator
 * rather than a cause.
 */
export const HYDRATION_TIMEOUT_MS = 15_000;

/**
 * The surface the helper uses, structurally — NOT `Page`, and NOT `Pick<Page, 'locator'>`.
 *
 * MEASURED: `Pick<Page, 'locator'>` keeps `locator()`'s return type as the full `Locator`
 * interface, so a plain stub is rejected with `error TS2322` and a unit test would need
 * `as unknown as Locator`, which removes tsc from the stub entirely. This form is satisfied by a
 * real `Page` (method-parameter bivariance) AND by a plain object, with no cast either way.
 */
export type HydrationPage = {
  locator(selector: string): { waitFor(options: { state: 'attached'; timeout: number }): Promise<void> };
};

// A real Page must keep satisfying the type above; this line fails the typecheck if it stops.
const _pageSatisfiesHydrationPage: (page: Page) => HydrationPage = (page) => page;
void _pageSatisfiesHydrationPage;

const SELECTOR = 'html[data-hydrated="true"]';

export async function waitForHydration(page: HydrationPage): Promise<void> {
  try {
    await page.locator(SELECTOR).waitFor({ state: 'attached', timeout: HYDRATION_TIMEOUT_MS });
  } catch (error: unknown) {
    // ONLY a timeout gets the explanation. Playwright rejects a pending waitFor with "Target page,
    // context or browser has been closed" or "Test ended." during teardown, and reporting those as
    // "the client bundle did not run" would be a confident WRONG diagnosis — worse than the bare
    // locator this helper exists to replace. The check is on `name` rather than
    // `instanceof TimeoutError` because importing that class would give this module a RUNTIME
    // @playwright/test dependency and make its own unit test impossible.
    if (!(error instanceof Error) || error.name !== 'TimeoutError') throw error;
    throw new Error(
      `React never hydrated: ${SELECTOR} was not attached within ${String(HYDRATION_TIMEOUT_MS)} ms. ` +
        '`Providers` sets that attribute in an effect, so its absence means the client bundle did not run — ' +
        'a 404 on a chunk, a hydration error thrown before the effect, or a browser too CPU-starved to reach it. ' +
        `Playwright reported: ${error.message}`,
      { cause: error },
    );
  }
}
```

- [ ] **Step 4: Rewrite `login.ts` to re-export**

In `ts/apps/gateway-console/tests/e2e/support/login.ts`, delete lines 6–14 (the doc comment and the `waitForHydration` function — the comment moved to `hydration.ts` in Step 3) and put this in their place:

```ts
// waitForHydration lives in its own module so it carries no RUNTIME @playwright/test import and a
// vitest unit test can drive it (tests/unit/hydration.test.ts). Re-exported here so every spec
// keeps importing it from './support/login'.
export { waitForHydration } from './hydration';
```

Then add the import `signIn` needs, directly below the existing `import type { Harness } from './harness';` line:

```ts
import { waitForHydration } from './hydration';
```

Leave everything else in the file untouched. `HYDRATION_TIMEOUT_MS` is deliberately **not** re-exported — it has no call-site consumer, and the unit test imports it from `./hydration`.

- [ ] **Step 5: Run the unit test and confirm every case passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/apps/gateway-console && ../../node_modules/.bin/vitest run tests/unit/hydration.test.ts
```

Expected: PASS, 7 tests.

If case 7 ("leaves no unbounded waitFor") fails, it has found a real second unbounded wait in this app — fix that wait, do not weaken the assertion.

- [ ] **Step 6: Prove the two assertions that must bite actually bite**

Verify the test is not vacuous. Make each mutation, run the test, confirm it **fails**, then revert it by deleting the mutation (never `git checkout --`, which would also discard the work in progress).

1. In `hydration.ts`, change `timeout: HYDRATION_TIMEOUT_MS` to `timeout: 999_999`. Expected: cases 1 and 6 fail. Revert.
2. In `hydration.ts`, change `HYDRATION_TIMEOUT_MS` to `60_000`. Expected: case 6 fails on both the literal pin and the `/4` bound. Revert.
3. In `hydration.ts`, delete the `if (!(error instanceof Error) || error.name !== 'TimeoutError') throw error;` line. Expected: case 5 fails. Revert.

Record the three observed failures in the commit message. If any mutation leaves the suite **green**, that assertion is vacuous — fix it before continuing.

- [ ] **Step 7: Typecheck**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/apps/gateway-console && ../../node_modules/.bin/tsc --noEmit -p tsconfig.json
```

Expected: no output, exit 0.

- [ ] **Step 8: Commit**

```bash
git add ts/apps/gateway-console/tests/e2e/support/hydration.ts \
        ts/apps/gateway-console/tests/e2e/support/login.ts \
        ts/apps/gateway-console/tests/unit/hydration.test.ts
git commit -m "test(ts): bound waitForHydration and explain its failure (SMA-639)"
```

---

## Task 2: Measure the `TimeoutError` name against a real browser

The one assumption the spec could not settle on paper: that a `waitFor` exceeding **its own** `timeout` option rejects with `name === 'TimeoutError'`. Task 1's unit test asserts the helper's behaviour *given* that name; this task proves Playwright actually produces it. If it does not, Task 1's branch never fires and the whole message is dead code.

**Files:**
- Create (throwaway): `ts/apps/gateway-console/tests/e2e/zz-name-probe.spec.ts`
- Modify on failure only: `ts/apps/gateway-console/tests/e2e/support/hydration.ts`

**Interfaces:**
- Consumes: `waitForHydration`, `HYDRATION_TIMEOUT_MS` from Task 1.
- Produces: no code. A recorded measurement, and a possible amendment to the branch condition.

- [ ] **Step 1: Write the probe**

Create `ts/apps/gateway-console/tests/e2e/zz-name-probe.spec.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
// THROWAWAY (SMA-639 Task 2). Deleted in Step 4 — never commit this file.
import { test } from '@playwright/test';

test('probe: the name of a waitFor timeout error', async ({ page }) => {
  await page.setContent('<html><body>no hydration marker here</body></html>');
  try {
    await page.locator('html[data-hydrated="true"]').waitFor({ state: 'attached', timeout: 1_000 });
    console.log('PROBE: no rejection — the probe is wrong');
  } catch (error: unknown) {
    const err = error as Error;
    console.log(`PROBE name=${err.name} ctor=${err.constructor.name} message=${err.message.slice(0, 80)}`);
  }
});
```

- [ ] **Step 2: Run it**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/apps/gateway-console && ../../node_modules/.bin/playwright test tests/e2e/zz-name-probe.spec.ts --project=single-zone --reporter=list
```

Expected: one passing test printing a `PROBE name=…` line.

If the run fails in `globalSetup` (it checks the build), run `moon run gateway-console-ts:build` from the repo root first and retry.

- [ ] **Step 3: Act on what it printed**

- If `name=TimeoutError` — the branch in Task 1 is correct. Change nothing.
- If the name is anything else — the branch is wrong and the helper's message would never appear. Widen the condition in `hydration.ts` to also accept a timeout by message, and say in the comment that the name check alone was **measured insufficient**:

```ts
const isTimeout = error instanceof Error && (error.name === 'TimeoutError' || /Timeout .*exceeded/.test(error.message));
if (!isTimeout) throw error;
```

Then add a unit case in `tests/unit/hydration.test.ts` covering a rejection with the observed name, and re-run the unit suite.

- [ ] **Step 4: Delete the probe**

```bash
rm ts/apps/gateway-console/tests/e2e/zz-name-probe.spec.ts
git status --short   # must show no zz-name-probe entry
```

- [ ] **Step 5: Commit only if Step 3 changed something**

If the name was `TimeoutError`, there is nothing to commit — record the measured value in Task 6's report and move on. Otherwise:

```bash
git add ts/apps/gateway-console/tests/e2e/support/hydration.ts ts/apps/gateway-console/tests/unit/hydration.test.ts
git commit -m "fix(ts): match a Playwright timeout by message as well as name (SMA-639)"
```

---

## Task 3: Copy the proven shape to `iam-console`

**Files:**
- Create: `ts/apps/iam-console/tests/e2e/support/hydration.ts`
- Modify: `ts/apps/iam-console/tests/e2e/support/login.ts:6-14`
- Create: `ts/apps/iam-console/tests/unit/hydration.test.ts`

**Interfaces:**
- Consumes: the three files Task 1 produced, as the source to copy.
- Produces: `HYDRATION_TIMEOUT_MS`, `waitForHydration`, `HydrationPage` in `iam-console`, identical to `gateway-console`'s.

- [ ] **Step 1: Copy the helper verbatim**

```bash
cp ts/apps/gateway-console/tests/e2e/support/hydration.ts \
   ts/apps/iam-console/tests/e2e/support/hydration.ts
```

The two files are **byte-identical**. Do not reword the comments for this app.

- [ ] **Step 2: Copy the unit test verbatim**

```bash
cp ts/apps/gateway-console/tests/unit/hydration.test.ts \
   ts/apps/iam-console/tests/unit/hydration.test.ts
```

Also byte-identical: both apps' `playwright.config.ts` carry the same `timeout` and `expect.timeout` values, so every assertion holds unchanged.

- [ ] **Step 3: Rewrite `iam-console`'s `login.ts` the same way**

In `ts/apps/iam-console/tests/e2e/support/login.ts`, delete lines 6–14 — the doc comment (which says "Task 10's `Providers`"; the version in `hydration.ts` drops that stale reference) and the function — and put in their place:

```ts
// waitForHydration lives in its own module so it carries no RUNTIME @playwright/test import and a
// vitest unit test can drive it (tests/unit/hydration.test.ts). Re-exported here so every spec
// keeps importing it from './support/login'.
export { waitForHydration } from './hydration';
```

Then add, directly below the existing `import type { Harness } from './harness';` line:

```ts
import { waitForHydration } from './hydration';
```

- [ ] **Step 4: Run the unit test — this is the second measurement the spec requires**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/apps/iam-console && ../../node_modules/.bin/vitest run tests/unit/hydration.test.ts
```

Expected: PASS, 7 tests.

This run is **not** a formality. The spec (§ 7 case 5) requires the `playwright.config.ts` import to be measured **in this app too**, because `iam-console`'s vitest config aliases `next/cache` in addition to `gateway-console`'s aliases. If the import fails here, report it — do not paper over it by replacing `config.timeout` with a literal.

- [ ] **Step 5: Typecheck**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/apps/iam-console && ../../node_modules/.bin/tsc --noEmit -p tsconfig.json
```

Expected: no output, exit 0.

- [ ] **Step 6: Confirm the two helpers have not drifted**

```bash
diff ts/apps/gateway-console/tests/e2e/support/hydration.ts \
     ts/apps/iam-console/tests/e2e/support/hydration.ts && echo "IDENTICAL"
```

Expected: `IDENTICAL`.

- [ ] **Step 7: Commit**

```bash
git add ts/apps/iam-console/tests/e2e/support/hydration.ts \
        ts/apps/iam-console/tests/e2e/support/login.ts \
        ts/apps/iam-console/tests/unit/hydration.test.ts
git commit -m "test(ts): bound iam-console's waitForHydration the same way (SMA-639)"
```

---

## Task 4: Make `playwright.config.ts` an input of both `test` tasks

Without this the plan's headline assertion is unenforceable. Both `test` tasks carry `options.merge: replace`, so they inherit nothing and list every input by hand — and neither list names `playwright.config.ts`. Lowering `timeout:` below `HYDRATION_TIMEOUT_MS` would therefore leave each task's cache key unchanged, Moon would serve a cached green, and case 6 would prove nothing.

**Files:**
- Modify: `ts/apps/gateway-console/moon.yml` (the `test` task's `inputs`, near line 217)
- Modify: `ts/apps/iam-console/moon.yml` (the `test` task's `inputs`, near line 247)

**Interfaces:**
- Consumes: the unit tests from Tasks 1 and 3.
- Produces: no code.

- [ ] **Step 1: Add the input to `gateway-console`**

In `ts/apps/gateway-console/moon.yml`, inside the **`test`** task's `inputs:` list (the one that already contains `'vitest.config.ts'`, **not** the `build` or `typecheck` list), add directly after the `'vitest.config.ts'` line:

```yaml
      # tests/unit/hydration.test.ts imports this config and asserts HYDRATION_TIMEOUT_MS against
      # its `timeout` and `expect.timeout` (SMA-639). This task carries `merge: replace`, so no
      # inherited input reaches a root-level config file: without this entry, lowering `timeout:`
      # below the constant leaves the cache key unchanged and Moon serves a cached PASS.
      - 'playwright.config.ts'
```

- [ ] **Step 2: Add the same input to `iam-console`**

Make the identical edit in `ts/apps/iam-console/moon.yml`, in **its** `test` task's `inputs:` list, after its `'vitest.config.ts'` line. Use the same comment text.

- [ ] **Step 3: Prove the input actually re-keys the task**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run gateway-console-ts:test          # populate the cache
touch ts/apps/gateway-console/playwright.config.ts
moon run gateway-console-ts:test          # must RE-RUN, not report a cached pass
```

Expected: the second run executes the task rather than reporting it cached.

If the second run reports a cache hit, the entry landed in the wrong task's `inputs` list — find the list containing `'vitest.config.ts'` and move it there.

- [ ] **Step 4: Commit**

```bash
git add ts/apps/gateway-console/moon.yml ts/apps/iam-console/moon.yml
git commit -m "ci(ts): key both consoles' test task on playwright.config.ts (SMA-639)"
```

---

## Task 5: Record the measured Playwright default in CLAUDE.md

**Files:**
- Modify: `CLAUDE.md` (append a bullet to the `## Gotchas` list)

**Interfaces:**
- Consumes: the measurement from Task 2.
- Produces: no code.

- [ ] **Step 1: Add the bullet**

Append to the `## Gotchas` list in `CLAUDE.md`, as a new top-level bullet. Do **not** place it inside the `<!-- ci-targets:begin -->` / `<!-- ci-targets:end -->` or `<!-- moon-diagnosis:begin -->` / `<!-- moon-diagnosis:end -->` marker blocks, and do not add a second copy of any marker — a duplicated marker reds `repo:affected-smoke`.

```markdown
- Playwright's `locator.waitFor()` defaults to **no timeout** (1.63.0), and no `playwright.config.ts`
  in this repo sets `use.actionTimeout`. So an unbounded `waitFor` is bounded only by the TEST
  budget — 120 s in CI, 60 s locally — and when it expires it reports the locator, not a cause.
  That cost SMA-512 pull request 4 two minutes of CI for an unexplained R4 flake. Both consoles'
  hydration waits now go through `tests/e2e/support/hydration.ts`, bounded at
  `HYDRATION_TIMEOUT_MS = 15_000` (one value, deliberately not a `process.env.CI` branch: both
  configs set `retries: isCI ? 2 : 0`, so a branched constant would put the TIGHTER bound on the
  run with NO retry). The helper's only `@playwright/test` import is a TYPE, which is what lets
  `tests/unit/hydration.test.ts` drive it with a stub and exercise the failure path with no
  browser. Two traps measured there: `Pick<Page, 'locator'>` does NOT accept a stub (it keeps the
  full `Locator` return type — `error TS2322`), so the parameter is a structural type; and the
  helper explains a `TimeoutError` ONLY, because Playwright rejects a pending `waitFor` with
  "Target page, context or browser has been closed" during teardown and calling that "the client
  bundle did not run" is a confident wrong diagnosis. **Residual: nothing gates a third console
  zone** — a new app that copies `login.ts` gets an unbounded wait and no `hydration.test.ts`, and
  nothing reds. `@paigasus/app-shell`'s `loadHydrated` is NOT affected: it uses
  `expect(...).toHaveCount(1)`, already bounded by the expect timeout.
```

- [ ] **Step 2: Confirm no marker was duplicated**

```bash
grep -c "ci-targets:begin" CLAUDE.md      # expected: 1
grep -c "moon-diagnosis:begin" CLAUDE.md  # expected: 1
```

Both must print `1`. Any other number reds `repo:affected-smoke`.

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record that locator.waitFor has no default timeout (SMA-639)"
```

---

## Task 6: Full verification

Nothing here is optional, and nothing here may be reported as passing without its output.

**Files:** none modified unless a gate reds.

- [ ] **Step 1: Both apps' unit and typecheck tasks**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run gateway-console-ts:test iam-console-ts:test
moon run gateway-console-ts:typecheck iam-console-ts:typecheck
```

Expected: all four pass.

- [ ] **Step 2: Lint and format**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run ts:lint
moon run ts:fmt
```

Expected: both pass. `ts:fmt` is a separate whole-tree Prettier gate from `ts:lint` — a passing lint does not imply a passing format.

If `ts:fmt` reports a difference, run the repo's formatter and commit the result rather than hand-editing.

- [ ] **Step 3: Both e2e tiers — the acceptance criterion this change can most plausibly break**

Docker must be reachable; `gateway-console-ts:test-e2e` needs it.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
docker info --format '{{.ServerVersion}}'    # must print a version
moon run iam-console-ts:test-e2e
moon run gateway-console-ts:test-e2e
```

Expected: both pass, and `gateway-console`'s run must include the **`two-zone`** project — it is the heaviest caller of the helper (`two-zone-session.spec.ts:71` and `:74`) and the rows most likely to be disturbed.

**This is the step that matters.** Every earlier step tests the helper in isolation; only this one runs it against a real browser at the new bound. If a row that passed before now fails at 15 s, that is a finding to report, not a number to quietly raise.

- [ ] **Step 4: Confirm the diff is exactly what the plan describes**

```bash
git status --short          # must be empty — no stray probe or scratch files
git diff --stat origin/main
```

Expected files, and no others: two `hydration.ts`, two `login.ts`, two `hydration.test.ts`, two `moon.yml`, `CLAUDE.md`, and the two `docs/superpowers/` documents.

- [ ] **Step 5: Report**

Report, with the evidence for each:

1. The value Task 2's probe printed for the error `name`, and whether it changed the branch.
2. The three mutation results from Task 1 Step 6 — each named assertion and the case that failed.
3. That `iam-console`'s `playwright.config.ts` import was measured working (Task 3 Step 4), not assumed.
4. That the Task 4 cache probe re-ran rather than reporting cached.
5. Every command in Steps 1–3 with its pass or fail status. **If anything failed, say so with the output.** Do not report completion on a partial pass.

---

## Self-Review

**Spec coverage.** § 3 structure → Task 1 Steps 3–4, Task 3. § 4 the single 15 s value → Task 1 Step 3, pinned by case 6. § 5 the message and the `TimeoutError`-only branch → Task 1 Step 3, cases 3–5, and Task 2's measurement. § 6 the rejected alternative → a design record, no task. § 7 all seven verification cases → Task 1 Step 1, with Step 6 proving three of them bite. § 8 D5 residual → stated in the case-7 comment and the CLAUDE.md bullet. § 8 D6 → no task, correctly: `app-shell` is out of scope. § 10 file table → Tasks 1, 3, 4, 5. § 11 AC 1–6 → Task 6 Steps 1–3.

**Placeholders.** None. Every code step carries the code; every command carries its expected output; the one genuinely open question (the error `name`) is a task with a measurement and both branches written out, not a TODO.

**Type consistency.** `HYDRATION_TIMEOUT_MS`, `waitForHydration`, `HydrationPage` and the `{ state: 'attached'; timeout: number }` option shape are spelled identically in the helper, the unit test's stub, and Task 3's copy. `login.ts` re-exports `waitForHydration` only — the unit test imports the constant from `./hydration`, matching what the helper exports.
