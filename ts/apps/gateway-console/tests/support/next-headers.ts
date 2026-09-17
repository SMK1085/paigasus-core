// SPDX-License-Identifier: Apache-2.0
//
// Stands in for `next/headers` in the vitest tier (vitest.config.ts aliases it here). The real
// module throws outside a Next request scope. A test sets the request headers and cookies it needs;
// tests/support/setup.ts clears both before every test.
let requestHeaders = new Headers();
let requestCookies = new Map<string, string>();

export function setRequestHeaders(init: Record<string, string>): void {
  requestHeaders = new Headers(init);
}

export function setRequestCookies(values: Record<string, string>): void {
  requestCookies = new Map(Object.entries(values));
}

export function resetNextHeaders(): void {
  requestHeaders = new Headers();
  requestCookies = new Map();
}

export function headers(): Promise<Headers> {
  return Promise.resolve(new Headers(requestHeaders));
}

type CookieValue = { name: string; value: string };

export function cookies(): Promise<{ get(name: string): CookieValue | undefined; has(name: string): boolean; getAll(): CookieValue[] }> {
  const snapshot = new Map(requestCookies);
  return Promise.resolve({
    get: (name) => {
      const value = snapshot.get(name);
      return value === undefined ? undefined : { name, value };
    },
    has: (name) => snapshot.has(name),
    getAll: () => [...snapshot].map(([name, value]) => ({ name, value })),
  });
}
