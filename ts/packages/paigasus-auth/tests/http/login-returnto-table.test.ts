// SPDX-License-Identifier: Apache-2.0
//
// The `returnTo` self-redirect guard is DERIVED from the route table (review, defect 3).
//
// THE DEFECT THIS PINS. `handleLogin` used to refuse a `returnTo` by testing a hardcoded
// `${basePath}/auth/` prefix, while `AUTH_ROUTE_SUFFIXES` is the package's single source of truth
// for what `createAuthRoutes` serves and what `authRoutePaths()` makes public. The two agreed only
// because all four suffixes start with `/auth/`. A fifth route outside that prefix would be served,
// be public, and escape the guard — so a crafted link could send the browser back into it after a
// successful login, one loop per click.
//
// The test DRIVES THE TABLE rather than a literal: it mocks `route-table.js` to add a hypothetical
// fifth suffix, `/signin`, and asserts the guard covers it. A guard written against `/auth/` cannot
// pass this file, which is what makes it a control instead of a restatement.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import type { AuthRuntime } from '../../src/runtime.js';

const HYPOTHETICAL_SUFFIX = '/signin';

vi.mock('../../src/http/route-table.js', async (importOriginal) => {
  const actual = await importOriginal<typeof import('../../src/http/route-table.js')>();
  return { ...actual, AUTH_ROUTE_SUFFIXES: [...actual.AUTH_ROUTE_SUFFIXES, HYPOTHETICAL_SUFFIX] };
});

const { createAuthRoutes } = await import('../../src/http/routes.js');

const BASE_PATH = '/iam';

let store: MemorySessionStore;
let runtime: AuthRuntime;
let lastState: string;

beforeEach(() => {
  store = new MemorySessionStore();
  lastState = '';
  // Only what handleLogin reads: no identity provider is contacted, so no OIDC fixture is needed.
  // The fake records the `state` it is handed, which IS the transaction id the store is keyed by.
  runtime = {
    store,
    basePath: BASE_PATH,
    zone: 'iam',
    scopes: 'openid profile',
    redirectUri: 'https://rp.example.com/iam/auth/callback',
    logger: { event: () => undefined },
    oidc: {
      buildAuthorizationUrl: (opts: { state: string }) => {
        lastState = opts.state;
        return Promise.resolve({ url: 'https://idp.example.test/authorize', codeVerifier: 'verifier', nonce: 'nonce' });
      },
    },
  } as unknown as AuthRuntime;
});

/** The returnTo that GET /auth/login stored for this request. */
async function storedReturnTo(raw: string): Promise<string | undefined> {
  const res = await createAuthRoutes(runtime).handle(new Request(`https://rp.example.com${BASE_PATH}/auth/login?returnTo=${encodeURIComponent(raw)}`));
  expect(res.status).toBe(302);
  return (await store.takeTransaction(lastState))?.returnTo;
}

describe('the returnTo self-redirect guard reads the route table', () => {
  it('refuses a returnTo pointing at a suffix the table carries, whatever its prefix', async () => {
    await expect(storedReturnTo(`${BASE_PATH}${HYPOTHETICAL_SUFFIX}`)).resolves.toBe(`${BASE_PATH}/`);
  });

  it('refuses every suffix in the table, including the four real ones', async () => {
    const { AUTH_ROUTE_SUFFIXES } = await import('../../src/http/route-table.js');

    for (const suffix of AUTH_ROUTE_SUFFIXES) {
      await expect(storedReturnTo(`${BASE_PATH}${suffix}`)).resolves.toBe(`${BASE_PATH}/`);
    }
  });

  // The guard must not have become a prefix test that swallows ordinary pages: a path that merely
  // starts like a route is kept. Without this, `startsWith(route)` alone would pass everything above.
  it('keeps a returnTo that only starts like a route in the table', async () => {
    await expect(storedReturnTo(`${BASE_PATH}${HYPOTHETICAL_SUFFIX}-up`)).resolves.toBe(`${BASE_PATH}${HYPOTHETICAL_SUFFIX}-up`);
    await expect(storedReturnTo(`${BASE_PATH}/authors`)).resolves.toBe(`${BASE_PATH}/authors`);
  });

  it('refuses a path BELOW a route in the table, as the old prefix guard did', async () => {
    await expect(storedReturnTo(`${BASE_PATH}${HYPOTHETICAL_SUFFIX}/step-two`)).resolves.toBe(`${BASE_PATH}/`);
  });
});
