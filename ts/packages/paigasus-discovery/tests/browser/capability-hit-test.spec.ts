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
//    what actually proves the CSS alone matters here: with `pointer-events: none` on the child, a
//    click aimed at it resolves to the WRAPPER (a plain, href-less <span>), so the anchor's
//    native click-to-navigate behaviour — which needs no JavaScript at all — never triggers.
//    Remove the style and the click resolves to the anchor ITSELF, which navigates with zero
//    script running. See disabled.tsx's own header for the hit-testing mechanics.
import { expect, test } from '@playwright/test';
import { CHILD_HREF } from './app.js';
import { startFixtureServer, type FixtureServer } from './fixture-server.js';

let fixture: FixtureServer;

test.beforeAll(async () => {
  fixture = await startFixtureServer();
});

test.afterAll(async () => {
  await fixture.close();
});

test('after hydration, a click on the disabled child never activates the ancestor', async ({ page }) => {
  await page.goto(fixture.url);
  // force: true — the same reason tests/capability.test.tsx passes `pointerEventsCheck: 0` to
  // user-event: the point of this test is a click LANDING despite `pointer-events: none`
  // somewhere in the subtree, so a preflight actionability check for exactly that property would
  // refuse to perform the click this test exists to prove is safe.
  await page.getByTestId('child-link').click({ force: true });
  const clicked = await page.evaluate(() => (window as Window & { __ancestorClicked?: boolean }).__ancestorClicked);
  expect(clicked).toBeUndefined();
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
