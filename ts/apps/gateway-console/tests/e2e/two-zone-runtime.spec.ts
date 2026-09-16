// SPDX-License-Identifier: Apache-2.0
//
// spec § 5.3: `createConsoleRuntime` must be called exactly once per app, at module scope, and
// every accessor but two is memoized with React `cache()`. This is the ONLY control anywhere in
// the repository for that rule. Outside a React server render, `cache()` is a pass-through, so
// no unit or integration test can observe a second call — a violation is invisible until two
// real requests render through it, which is why this row lives in the two-zone e2e tier and
// nowhere else.
//
// Do NOT assert a raw count of Introspect calls for one navigation. After hydration, the Next
// router prefetches every visible link, and each prefetch is another request and therefore
// another legitimate render and another legitimate Introspect call. A raw count is flaky in the
// direction that matters: it FAILS WHEN THE PRODUCT IS CORRECT, and the next person to see it red
// will "fix" it by weakening it to `toBeGreaterThan(0)`, which asserts nothing and would leave
// the once-per-app rule with no control at all.
//
// The invariant this row actually checks is PER REQUEST, not per navigation: `proxy.ts` mints a
// fresh correlation id for every request it forwards, and `@paigasus/console-core` sends it to
// IAM on every call, so the fake's call log carries it. Group the Introspect calls by that id.
// However many requests ran, and however many of them were real navigations versus prefetches,
// no single group may have more than one call — a second `createConsoleRuntime()` call inside
// the SAME request makes a second memoization identity, so its two Introspect calls carry the
// SAME correlation id and land in the SAME group, which is exactly what the last assertion below
// catches and what a raw count would only catch by luck.
//
// Two vacuity guards, both needed: the grouping must have seen at least one id (`byCorrelation.size`
// greater than zero), and no call may have arrived with a null correlation id (a null id cannot be
// grouped, so it would silently fall outside the invariant this row exists to check).
import { ORG_ID } from './support/world';
import { expect, test } from './support/two-zone-harness';

test('R11: no single request makes more than one Introspect call (§ 5.3)', async ({ page, harness }) => {
  // A cold login at the gateway zone's scope route, then a cross-zone hard navigation to the IAM
  // zone's own list page: at least two independent requests, rendered by two different apps,
  // each with its own `createConsoleRuntime()` call — real traffic for the grouping below, not a
  // single request repeated.
  await page.goto(harness.url(`/gateway/orgs/${ORG_ID}`));
  await expect(page.getByTestId('zone-overview')).toBeVisible();

  await page.getByRole('navigation', { name: 'Primary' }).getByRole('link', { name: 'IAM', exact: true }).click();
  await page.waitForURL(`${harness.origin}/iam/orgs`);
  await expect(page.getByRole('region', { name: 'Your organizations' })).toBeVisible();

  const calls = harness.iam.callsTo('authn.introspect');
  const byCorrelation = new Map<string, number>();
  for (const call of calls) {
    if (call.correlationId === null) continue;
    byCorrelation.set(call.correlationId, (byCorrelation.get(call.correlationId) ?? 0) + 1);
  }
  // Vacuity: the grouping must have seen something, and the ids must be real.
  expect(byCorrelation.size).toBeGreaterThan(0);
  expect(calls.filter((call) => call.correlationId === null)).toEqual([]);
  // The invariant: ONE runtime per request means ONE Introspect per request, however many requests ran.
  expect([...byCorrelation.entries()].filter(([, count]) => count > 1)).toEqual([]);
});
