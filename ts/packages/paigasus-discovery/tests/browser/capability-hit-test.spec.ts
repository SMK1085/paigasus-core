// SPDX-License-Identifier: Apache-2.0
//
// SMA-509. jsdom (../capability.test.tsx) cannot model hit-testing, which is exactly the property
// this design depends on: `pointer-events: none` moved from the WRAPPER to the CHILD so that a
// click resolves to the wrapper (whose capture handler blocks it) even when this component sits
// inside a clickable ancestor. This is a REAL Chromium, not jsdom.
//
// Two cases, and they need two DIFFERENT observables:
//
//  - AFTER hydration, the wrapper's onClickCapture is what blocks the ancestor's React onClick —
//    `window.__ancestorClicked` (app.tsx) proves that directly.
//  - BEFORE hydration (`javaScriptEnabled: false`), no React handler exists at all, on the
//    wrapper OR the ancestor — `window.__ancestorClicked` would stay unset regardless of the CSS,
//    which would not be a real test of anything. The CHILD's OWN `href` (app.tsx's CHILD_HREF) is
//    what actually proves the CSS alone matters here: with `pointer-events: none` reaching the
//    child, a click aimed at it resolves to the WRAPPER (a plain, href-less <span>), so the
//    anchor's native click-to-navigate behaviour — which needs no JavaScript at all — never
//    triggers. Remove the protection and the click resolves to the anchor ITSELF, which navigates
//    with zero script running. See disabled.tsx's own header for the hit-testing mechanics.
//
// SMA-509 finding C: the AFTER-hydration test used to assert only that `__ancestorClicked` stayed
// falsy. If entry.tsx failed to load, or `hydrateRoot` never ran, the server-rendered ancestor has
// no React handler at all, so that flag is trivially falsy and the test would pass while proving
// NOTHING. It now (1) clicks an always-enabled hydration control first and requires it to fire,
// and (2) fails on any page or console error, since Playwright does not do so by default.
//
// SMA-509 finding A added the two BEFORE-hydration variant cases below: the pre-hydration CSS
// guarantee must hold not only for a plain single-element child, but for a Fragment child and for
// a custom component that does not forward `style` to its own DOM node — see app.tsx's header and
// disabled.tsx's file header for the full reasoning.
import { expect, test } from '@playwright/test';
import { CHILD_HREF } from './app.js';
import { startFixtureServer, type FixtureServer } from './fixture-server.js';

let fixture: FixtureServer;

test.beforeAll(async () => {
  fixture = await startFixtureServer('default');
});

test.afterAll(async () => {
  await fixture.close();
});

test('after hydration, a click on the disabled child never activates the ancestor', async ({ page }) => {
  const consoleErrors: string[] = [];
  const pageErrors: string[] = [];
  page.on('console', (message) => {
    if (message.type() === 'error') consoleErrors.push(message.text());
  });
  page.on('pageerror', (error) => {
    pageErrors.push(error.message);
  });

  await page.goto(fixture.url);

  // Positive hydration control (SMA-509 finding C). If entry.tsx failed to load, or
  // `hydrateRoot` never ran, `window.__ancestorClicked` below would stay falsy trivially and the
  // real assertion would pass without proving anything about pointer events at all. This click
  // MUST set the flag, or hydration is broken and the test must fail loudly, here, first.
  await page.getByTestId('hydration-control').click();
  const controlClicked = await page.evaluate(() => (window as Window & { __controlClicked?: boolean }).__controlClicked);
  expect(controlClicked).toBe(true);

  // force: true — the same reason tests/capability.test.tsx passes `pointerEventsCheck: 0` to
  // user-event: the point of this test is a click LANDING despite `pointer-events: none`
  // somewhere in the subtree, so a preflight actionability check for exactly that property would
  // refuse to perform the click this test exists to prove is safe.
  await page.getByTestId('child-link').click({ force: true });
  const clicked = await page.evaluate(() => (window as Window & { __ancestorClicked?: boolean }).__ancestorClicked);
  expect(clicked).toBeUndefined();

  expect(pageErrors, pageErrors.join('\n')).toHaveLength(0);
  expect(consoleErrors, consoleErrors.join('\n')).toHaveLength(0);
});

test('before hydration (JavaScript disabled), the CSS alone blocks the click', async ({ browser }) => {
  const context = await browser.newContext({ javaScriptEnabled: false });
  try {
    const page = await context.newPage();
    await page.goto(fixture.url);
    await page.getByTestId('child-link').click({ force: true });
    // click() does not itself wait for a resulting same-document navigation to complete — give a
    // would-be (broken) one a moment to actually land before reading the URL.
    await page.waitForTimeout(300);
    expect(new URL(page.url()).pathname).not.toBe(CHILD_HREF);
  } finally {
    await context.close();
  }
});

test('before hydration, a Fragment child is still protected by the CSS rule', async ({ browser }) => {
  const fragmentFixture = await startFixtureServer('fragment');
  try {
    const context = await browser.newContext({ javaScriptEnabled: false });
    try {
      const page = await context.newPage();
      await page.goto(fragmentFixture.url);
      await page.getByTestId('child-link').click({ force: true });
      await page.waitForTimeout(300);
      expect(new URL(page.url()).pathname).not.toBe(CHILD_HREF);
    } finally {
      await context.close();
    }
  } finally {
    await fragmentFixture.close();
  }
});

test('before hydration, a non-style-forwarding custom component child is still protected', async ({ browser }) => {
  const customComponentFixture = await startFixtureServer('custom-component');
  try {
    const context = await browser.newContext({ javaScriptEnabled: false });
    try {
      const page = await context.newPage();
      await page.goto(customComponentFixture.url);
      await page.getByTestId('child-link').click({ force: true });
      await page.waitForTimeout(300);
      expect(new URL(page.url()).pathname).not.toBe(CHILD_HREF);
    } finally {
      await context.close();
    }
  } finally {
    await customComponentFixture.close();
  }
});
