// SPDX-License-Identifier: Apache-2.0
//
// § 10.1's stale-cookie recovery trap (src/next/get-session.ts's own header): the browser keeps
// sending __Host-pgs_sid long after the Redis-backed record it names can disappear for a reason
// the browser never learns about (here: an operator/tooling wipe). Deleting the record STRAIGHT
// OUT OF REDIS — bypassing this package's own /auth/logout entirely — is what proves the recovery
// path handles a record vanishing out from under a live cookie, not just a cookie the app itself
// cleared.
import { createClient } from 'redis';
import { expect, test } from '@playwright/test';
import { KEYCLOAK_PASSWORD, KEYCLOAK_USERNAME, SESSION_COOKIE_NAME, ZONE_BASE_PATH } from './constants.js';
import { readRuntimeEnv } from './runtime-env.js';

test('§ 10.1: a session record deleted straight out of Redis recovers to a login redirect', async ({ page, context }) => {
  await page.goto(`${ZONE_BASE_PATH}/guarded`);
  await page.locator('#username').fill(KEYCLOAK_USERNAME);
  await page.locator('#password').fill(KEYCLOAK_PASSWORD);
  await page.locator('#kc-login').click();
  await expect(page.getByTestId('guarded-heading')).toBeVisible();

  const cookies = await context.cookies();
  const sessionCookie = cookies.find((c) => c.name === SESSION_COOKIE_NAME);
  expect(sessionCookie).toBeDefined();
  if (sessionCookie === undefined) throw new Error('unreachable');

  const { redisUrl } = readRuntimeEnv();
  const redis = createClient({ url: redisUrl });
  await redis.connect();
  try {
    // Mirrors src/adapters/redis-store.ts's own `sessKey` (empty keyPrefix — this fixture's
    // runtime config sets none, matching src/runtime.ts's "all zones share ONE store" comment).
    // Not imported: this suite treats the store as an external system it reaches independently of
    // the app under test, the same way a real operator's `redis-cli DEL` would.
    const deleted = await redis.del(`pgs:sess:${sessionCookie.value}`);
    expect(deleted, 'the session key must have existed before this delete').toBe(1);
  } finally {
    await redis.close();
  }

  // Reload the guarded page with the SAME (now stale) cookie still attached — the browser has no
  // way to know the record is gone.
  await page.goto(`${ZONE_BASE_PATH}/guarded`);

  // Not a blank page pretending to be authenticated, and not the guarded content either: a
  // redirect back to Keycloak's login form.
  await expect(page.getByTestId('guarded-heading')).toHaveCount(0);
  await expect(page.locator('#username')).toBeVisible();
});
