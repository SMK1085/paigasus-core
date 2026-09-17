// SPDX-License-Identifier: Apache-2.0
//
// AC 2 (spec § 10.5): a cold login at the gateway zone must never touch the iam-console APP.
import { expect, test } from './support/two-zone-harness';

test('R10: a cold login at the gateway zone reaches the overview with zero connections to the iam-console app (AC 2)', async ({ page, harness }) => {
  // What this row claims, and what it does not — the distinction that made revision 1 of the
  // spec's table wrong. The gateway zone's cold-login path is proxy -> /gateway/auth/login ->
  // the IdP -> /gateway/auth/callback -> IAM over gRPC. The IAM SERVICE sits on that path and
  // must stay up for this test to pass at all. The iam-console APP sits on none of it, and that
  // is exactly the property this row proves. Stopping the iam-console app instead of measuring
  // the forwarder would pass whether or not the design has this property, and would keep
  // passing even after a later change grew a real cross-zone dependency on the app.
  //
  // The vacuity guard for this row — the forwarder's counter must be greater than zero
  // SOMEWHERE in the run, or a forwarder that was never wired into the terminator would also
  // report a zero delta here — lives in R8 (two-zone-session.spec.ts), where traffic through
  // /iam is expected, not in this test, where it is not.
  //
  // Both connections() AND requests() are asserted, and neither may be dropped as redundant.
  // The forwarder proxies through Node's http.globalAgent, which keeps a socket open for reuse;
  // Playwright's own test list runs R12 immediately before R10 in the same worker, and R12
  // drives real /iam/orgs traffic through that same forwarder, so a warm, pooled socket can
  // already sit in the agent's pool when this row takes its "before" snapshot. connections()
  // counts TCP connections, so a real regression that rides that pooled socket would add ZERO
  // new connections and this row would stay green on connections() alone. requests() counts
  // HTTP requests instead, and a request riding a warm, pooled socket still increments it, so it
  // is the assertion that actually proves acceptance criterion 2. connections() stays too: it
  // catches a connection opened and dropped without completing a request, a different failure
  // shape that requests() cannot see.
  // THIS ROW'S OWN POSITIVE CONTROL, and it may not be moved to another test. A zero delta is
  // also what an unwired forwarder reports — one the terminator never routes to counts nothing
  // and this row would pass having proved no isolation at all. R8 drives traffic through /iam,
  // but R8 is a DIFFERENT test: run this row alone (`--grep R10`), or let Playwright restart the
  // worker after a failure, and R8's traffic never happens. A vacuity guard that lives in another
  // test is not a guard on this one.
  //
  // So: reach the IAM zone through the terminator first, and prove the counter moves.
  const wiringBefore = harness.iamConsole.requests();
  const probe = await page.request.get(harness.url('/iam/healthz'));
  expect(probe.ok(), 'the IAM zone did not answer through the terminator, so this row cannot measure isolation').toBe(true);
  expect(harness.iamConsole.requests() - wiringBefore, 'the counting forwarder is not on the /iam route: a zero delta below would prove nothing').toBeGreaterThan(0);

  const connectionsBefore = harness.iamConsole.connections();
  const requestsBefore = harness.iamConsole.requests();

  await page.goto(harness.url('/gateway/overview'));

  expect(new URL(page.url()).pathname).toBe('/gateway/overview');
  await expect(page.getByTestId('zone-overview')).toBeVisible();
  expect(harness.iamConsole.connections() - connectionsBefore).toBe(0);
  expect(harness.iamConsole.requests() - requestsBefore).toBe(0);
});
