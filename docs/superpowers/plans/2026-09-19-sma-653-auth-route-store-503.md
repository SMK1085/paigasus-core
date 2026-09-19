# SMA-653 Auth-Route Store 503 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `@paigasus/auth`'s `/auth/login`, `/auth/callback` and `/auth/logout` return a 503 with a retry control when the session store is unavailable. Also make logout attempt the delete when the read fails.

**Architecture:** A private module `src/http/store-unavailable.ts` builds the 503 page and wraps each store call (`storeStep`). `src/http/routes.ts` uses it at each store call. The error is classified by its `code` field (`isSessionStoreUnavailable`), not by `instanceof`, because Next loads two copies of the package.

**Tech Stack:** TypeScript (ESM, `verbatimModuleSyntax`), Vitest 5, Moon 2.5.3, pnpm 11.

**Spec:** `docs/superpowers/specs/2026-09-19-sma-653-auth-route-store-503-design.md`. Read it before you start. Section and decision numbers (§ 4, D9, …) in this plan refer to it.

## Global Constraints

- Every new source file starts with `// SPDX-License-Identifier: Apache-2.0`.
- Relative imports in `src/` are EXTENSIONLESS (`'../core/errors'`). Test files import with `.js` (`'../../src/core/errors.js'`), as the existing tests do. (CLAUDE.md: Turbopack does not resolve `.js` to `.ts`; ESLint rule `paigasus/no-js-relative-specifier`.)
- No log event and no response may hold a caught error object, an error message, an error name, or a DSN (`src/ports/logger.ts` redaction contract). A `sid` is logged only as `sidTag(sid)` (8 characters).
- `Retry-After` is exactly `5`. The CSP is exactly `default-src 'none'; frame-ancestors 'none'; base-uri 'none'`.
- A 503 response has NO `Set-Cookie` header.
- Commits: conventional, scope `ts`, subject ends with `(SMA-653)`, body ends with `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`. Do not use `--no-verify`. Do not amend. Add new commits only.
- Work in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-653-auth-route-503` on branch `feature/sma-653-auth-route-503`. Check the branch with `git branch --show-current` before the first commit of each task.
- Shell setup for every command: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. The package directory is `ts/packages/paigasus-auth` (below: `PKG`).
- Run a single test file from `PKG` with `pnpm exec vitest run <path>`. Do not start background jobs. Run every command in the foreground.

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `PKG/src/core/errors.ts` | modify | add `isSessionStoreUnavailable` |
| `PKG/src/ports/logger.ts` | modify | add the closed `StoreUnavailableStage` union |
| `PKG/src/ports/session-store.ts` | modify | doc comment: callers classify by `code` |
| `PKG/src/next/get-session.ts` | modify | D8: use `isSessionStoreUnavailable` |
| `PKG/src/core/single-flight.ts` | modify | type its `release_lock` stage |
| `PKG/src/http/store-unavailable.ts` | create | the 503 page, the retry-link builder, `storeStep`, the log helper |
| `PKG/src/http/routes.ts` | modify | the mapping at each store call, D5, D6, D9, D10 |
| `PKG/src/server.ts` | modify | doc comment only |
| `PKG/README.md` | modify | lines 115-126: describe the new behaviour |
| `docs/superpowers/specs/2026-09-09-sma-506-auth-design.md` | modify | one pointer line in § 7.2 |
| `PKG/tests/core/errors.test.ts` | create | `isSessionStoreUnavailable`, including a second module copy |
| `PKG/tests/next/get-session.test.ts` | modify | D8 test |
| `PKG/tests/http/store-unavailable-response.test.ts` | create | the 503 builder: headers, escaping, bytes |
| `PKG/tests/support/store-failure.ts` | create | the shared harness for the route tests |
| `PKG/tests/http/store-unavailable.test.ts` | create | § 4 rows 1-8 through `createAuthRoutes` |
| `PKG/tests/http/route-handler.test.ts` | modify | one row through `createAuthRouteHandler` |

---

### Task 1: Classify a store error by `code`, and close the stage vocabulary

**Files:**
- Modify: `PKG/src/core/errors.ts` (after the `SessionStoreTimeout` class, around line 41)
- Modify: `PKG/src/ports/logger.ts`
- Modify: `PKG/src/ports/session-store.ts:18-25` (doc comment)
- Modify: `PKG/src/next/get-session.ts:34,76-77`
- Modify: `PKG/src/core/single-flight.ts:221`
- Create: `PKG/tests/core/errors.test.ts`
- Modify: `PKG/tests/next/get-session.test.ts` (the `getSession failure attribution (SMA-626 § 2.4)` describe block, around line 216)

**Interfaces:**
- Produces: `isSessionStoreUnavailable(err: unknown): err is SessionStoreUnavailable` in `src/core/errors.ts`.
- Produces: `export type StoreUnavailableStage = 'get_session' | 'release_lock' | 'login_put_transaction' | 'login_delete' | 'callback_take_transaction' | 'callback_delete' | 'callback_set' | 'logout_get' | 'logout_delete';` in `src/ports/logger.ts`.

- [ ] **Step 1: Write the failing tests**

Create `PKG/tests/core/errors.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-653 D2. Next 16 loads a route handler and a page with SEPARATE copies of this package, and
// the runtime (with its store) is shared between them through globalThis (src/runtime.ts). So a
// store error can be an instance of the OTHER copy's class, and `instanceof` is then false. These
// tests build that second copy with vi.resetModules() and a dynamic import.
import { describe, expect, it, vi } from 'vitest';
import { CallbackRejected, SessionStoreTimeout, SessionStoreUnavailable, isSessionStoreUnavailable } from '../../src/core/errors.js';

describe('isSessionStoreUnavailable (SMA-653 D2)', () => {
  it('is true for a SessionStoreUnavailable and for its SessionStoreTimeout subclass', () => {
    expect(isSessionStoreUnavailable(new SessionStoreUnavailable('down'))).toBe(true);
    expect(isSessionStoreUnavailable(new SessionStoreTimeout('get', 4000, 'deadline'))).toBe(true);
    expect(isSessionStoreUnavailable(new SessionStoreTimeout('get', 4000, 'circuit-open'))).toBe(true);
  });

  it('is true for an error from a SECOND copy of core/errors', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    // Precondition: without this, the test passes vacuously if the import returns the same module.
    expect(foreign.SessionStoreUnavailable).not.toBe(SessionStoreUnavailable);
    const err = new foreign.SessionStoreTimeout('get', 4000, 'deadline');
    expect(err instanceof SessionStoreUnavailable).toBe(false);
    expect(isSessionStoreUnavailable(err)).toBe(true);
  });

  it('is false for other errors and for non-errors', () => {
    expect(isSessionStoreUnavailable(new CallbackRejected('txn_missing'))).toBe(false);
    expect(isSessionStoreUnavailable(new Error('session_store_unavailable'))).toBe(false);
    expect(isSessionStoreUnavailable({ code: 'session_store_unavailable' })).toBe(false);
    expect(isSessionStoreUnavailable(undefined)).toBe(false);
  });
});
```

In `PKG/tests/next/get-session.test.ts` (no new imports are needed), inside `describe('getSession failure attribution (SMA-626 § 2.4)', …)`, after the test `logs store.unavailable ONLY for a SessionStoreUnavailable`, add:

```ts
  it('logs store.unavailable for a SessionStoreUnavailable from a second module copy (SMA-653 D8)', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    expect(foreign.SessionStoreUnavailable).not.toBe(SessionStoreUnavailable);
    cookiesMock.mockResolvedValue(cookieJar('some-sid'));
    const { logger, events } = recordingLogger();
    const store: SessionStore = { ...unavailableStore(), get: () => Promise.reject(new foreign.SessionStoreUnavailable('down')) };

    await expect(getSession({ ...baseRuntime(store), logger })).resolves.toBeNull();

    expect(events).toEqual([['store.unavailable', { sid: sidTag('some-sid'), stage: 'get_session' }]]);
  });
```

- [ ] **Step 2: Run the tests and verify that they fail**

Run (from `PKG`): `pnpm exec vitest run tests/core/errors.test.ts tests/next/get-session.test.ts`
Expected: `errors.test.ts` fails to import `isSessionStoreUnavailable` (it is not exported). The new get-session test FAILS with `session.resolve_failed` in place of `store.unavailable`.

- [ ] **Step 3: Implement**

In `PKG/src/core/errors.ts`, after the `SessionStoreTimeout` class, add:

```ts
/**
 * True for a SessionStoreUnavailable, or a subclass such as SessionStoreTimeout, from ANY copy of
 * this module (SMA-653 D2).
 *
 * Why not `instanceof`: Next 16 gives a route handler and a page SEPARATE copies of this package
 * (measured, see src/runtime.ts's comment on RUNTIME_KEY_PREFIX), and the runtime, with its store,
 * is shared between them through globalThis. The store therefore throws the class of whichever
 * copy built the runtime first, and `instanceof` in the other copy is false. The `code` class
 * field is an own property of every instance, and the subclass inherits it, so it survives the
 * duplication.
 */
export function isSessionStoreUnavailable(err: unknown): err is SessionStoreUnavailable {
  return err instanceof Error && (err as { code?: unknown }).code === 'session_store_unavailable';
}
```

In `PKG/src/ports/logger.ts`, after the `AuthEventFields` type, add:

```ts
/**
 * The closed set of `stage` values for `store.unavailable` (SMA-653 § 5). Every emitter uses this
 * type, so an operator can rely on the list. Each value names ONE store call.
 */
export type StoreUnavailableStage =
  | 'get_session'
  | 'release_lock'
  | 'login_put_transaction'
  | 'login_delete'
  | 'callback_take_transaction'
  | 'callback_delete'
  | 'callback_set'
  | 'logout_get'
  | 'logout_delete';
```

In `PKG/src/next/get-session.ts`:
- Change the import on line 34 to `import { isSessionStoreUnavailable } from '../core/errors';`.
- Change `if (err instanceof SessionStoreUnavailable) {` to `if (isSessionStoreUnavailable(err)) {`.
- Change the event fields on the next line to `{ sid: sidTag(sid), stage: 'get_session' satisfies StoreUnavailableStage }`, and add `StoreUnavailableStage` to the type import from `'../ports/logger'` (add a `import type { StoreUnavailableStage } from '../ports/logger';` if that file imports only values from it).
- In the comment block above (lines 67-75), change "Classifying here costs one `instanceof`" to "Classifying here costs one `code` check (`isSessionStoreUnavailable`, SMA-653 D8 — `instanceof` fails across Next's two module copies)".

In `PKG/src/core/single-flight.ts:221`, change `stage: 'release_lock'` to `stage: 'release_lock' satisfies StoreUnavailableStage` and add the type import from `'../ports/logger'`.

In `PKG/src/ports/session-store.ts`, in the doc comment at lines 18-25, change "Callers classify on that class:" to "Callers classify on that class, through `isSessionStoreUnavailable` (its `code`, never `instanceof` — SMA-653 D2):" and add, at the end of that paragraph: "The auth routes (`http/routes.ts`) map it to a 503 (SMA-653)."

- [ ] **Step 4: Run the tests and verify that they pass**

Run (from `PKG`): `pnpm exec vitest run tests/core/errors.test.ts tests/next/get-session.test.ts tests/core/single-flight.test.ts`
Expected: all PASS.

- [ ] **Step 5: Typecheck**

Run (from repo root): `moon run paigasus-auth-ts:typecheck`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-auth/src/core/errors.ts ts/packages/paigasus-auth/src/ports/logger.ts ts/packages/paigasus-auth/src/ports/session-store.ts ts/packages/paigasus-auth/src/next/get-session.ts ts/packages/paigasus-auth/src/core/single-flight.ts ts/packages/paigasus-auth/tests/core/errors.test.ts ts/packages/paigasus-auth/tests/next/get-session.test.ts
git commit -m "fix(ts): classify a session-store failure by code, not instanceof (SMA-653)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 2: The 503 page builder

**Files:**
- Create: `PKG/src/http/store-unavailable.ts`
- Create: `PKG/tests/http/store-unavailable-response.test.ts`

**Interfaces:**
- Consumes (Task 1): `isSessionStoreUnavailable` from `'../core/errors'`, `StoreUnavailableStage` and `sidTag` from `'../ports/logger'`.
- Produces (all exported from `src/http/store-unavailable.ts`):
  - `RETRY_AFTER_SECONDS = 5`
  - `STORE_UNAVAILABLE_CSP: string`
  - `type RetryAffordance = { kind: 'link'; href: string } | { kind: 'post'; action: string }`
  - `escapeHtmlAttribute(value: string): string`
  - `loginRetryHref(basePath: string, returnTo?: string): string`
  - `storeUnavailableResponse(retry: RetryAffordance): Response`
  - `STORE_DOWN` (a `unique symbol`)
  - `storeStep<T>(runtime: StoreFailureContext, stage: StoreUnavailableStage, sid: string | undefined, op: () => Promise<T>): Promise<T | typeof STORE_DOWN>`
  - `interface StoreFailureContext { logger: AuthLogger; zone: string }` (an `AuthRuntime` satisfies it)

- [ ] **Step 1: Write the failing tests**

Create `PKG/tests/http/store-unavailable-response.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-653 § 3: the 503 page and the store-call wrapper, tested directly. The routes that use them
// are tested in store-unavailable.test.ts.
import { describe, expect, it } from 'vitest';
import { SessionStoreTimeout, SessionStoreUnavailable } from '../../src/core/errors.js';
import {
  STORE_DOWN,
  STORE_UNAVAILABLE_CSP,
  escapeHtmlAttribute,
  loginRetryHref,
  storeStep,
  storeUnavailableResponse,
} from '../../src/http/store-unavailable.js';
import type { AuthEventFields, AuthEventName } from '../../src/ports/logger.js';

const HOSTILE = `/iam/a"b<c>d&e'f#g h`;

function recorder(): { ctx: { zone: string; logger: { event(name: AuthEventName, fields: AuthEventFields): void } }; events: Array<[AuthEventName, AuthEventFields]> } {
  const events: Array<[AuthEventName, AuthEventFields]> = [];
  return { ctx: { zone: 'iam', logger: { event: (name, fields) => void events.push([name, { ...fields }]) } }, events };
}

describe('storeUnavailableResponse', () => {
  it('is a 503 with the § 3 headers and no Set-Cookie', async () => {
    const res = storeUnavailableResponse({ kind: 'link', href: '/iam/auth/login' });
    expect(res.status).toBe(503);
    expect(res.headers.get('retry-after')).toBe('5');
    expect(res.headers.get('cache-control')).toBe('no-store');
    expect(res.headers.get('content-type')).toBe('text/html; charset=utf-8');
    expect(res.headers.get('referrer-policy')).toBe('no-referrer');
    expect(res.headers.get('content-security-policy')).toBe(STORE_UNAVAILABLE_CSP);
    expect(STORE_UNAVAILABLE_CSP).toBe("default-src 'none'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'");
    expect(res.headers.getSetCookie()).toEqual([]);
    const body = await res.text();
    expect(body).toContain('<a href="/iam/auth/login">');
    expect(body).toContain('Sign-in is temporarily unavailable');
    expect(body).not.toMatch(/<script|<style|style=/i);
  });

  it('renders a POST form for the logout affordance', async () => {
    const body = await storeUnavailableResponse({ kind: 'post', action: '/iam/auth/logout' }).text();
    expect(body).toContain('<form method="post" action="/iam/auth/logout">');
    expect(body).toContain('Sign-out did not complete');
    expect(body).not.toContain('<a href=');
  });

  it('HTML-escapes the attribute, in a double-quoted attribute, with exact bytes', async () => {
    const body = await storeUnavailableResponse({ kind: 'link', href: HOSTILE }).text();
    expect(body).toContain('href="/iam/a&quot;b&lt;c&gt;d&amp;e&#39;f#g h"');
    expect(body).not.toContain('b<c');
  });
});

describe('escapeHtmlAttribute', () => {
  it('escapes the five characters, & first', () => {
    expect(escapeHtmlAttribute(`&<>"'`)).toBe('&amp;&lt;&gt;&quot;&#39;');
    expect(escapeHtmlAttribute('&lt;')).toBe('&amp;lt;');
  });
});

describe('loginRetryHref', () => {
  it('is the bare login route with no returnTo', () => {
    expect(loginRetryHref('/iam')).toBe('/iam/auth/login');
    expect(loginRetryHref('')).toBe('/auth/login');
  });

  it('URL-encodes returnTo so that & and # survive a round trip', () => {
    const href = loginRetryHref('/iam', HOSTILE);
    expect(href).toBe(`/iam/auth/login?returnTo=${encodeURIComponent(HOSTILE)}`);
    expect(new URL(href, 'https://rp.example.com').searchParams.get('returnTo')).toBe(HOSTILE);
  });
});

describe('storeStep', () => {
  it('returns the value when the call succeeds, and logs nothing', async () => {
    const { ctx, events } = recorder();
    await expect(storeStep(ctx, 'logout_get', 'sid-0123456789', () => Promise.resolve(42))).resolves.toBe(42);
    expect(events).toEqual([]);
  });

  it.each([
    ['SessionStoreUnavailable', () => new SessionStoreUnavailable('down (redis://u:sentinel-pw@redis.invalid)')],
    ['SessionStoreTimeout', () => new SessionStoreTimeout('redis://u:sentinel-pw@redis.invalid', 4000, 'deadline')],
  ])('maps a %s to STORE_DOWN and logs one redacted event', async (_name, makeError) => {
    const { ctx, events } = recorder();
    await expect(storeStep(ctx, 'logout_delete', 'sid-0123456789', () => Promise.reject(makeError()))).resolves.toBe(STORE_DOWN);
    expect(events).toEqual([['store.unavailable', { zone: 'iam', stage: 'logout_delete', sid: 'sid-0123' }]]);
    expect(JSON.stringify(events)).not.toContain('sentinel');
  });

  it('omits sid when the step has none', async () => {
    const { ctx, events } = recorder();
    await storeStep(ctx, 'callback_take_transaction', undefined, () => Promise.reject(new SessionStoreUnavailable('down')));
    expect(events).toEqual([['store.unavailable', { zone: 'iam', stage: 'callback_take_transaction' }]]);
  });

  it('re-throws any other error unchanged', async () => {
    const { ctx, events } = recorder();
    const boom = new Error('boom');
    await expect(storeStep(ctx, 'login_delete', undefined, () => Promise.reject(boom))).rejects.toBe(boom);
    expect(events).toEqual([]);
  });
});
```

- [ ] **Step 2: Run the tests and verify that they fail**

Run (from `PKG`): `pnpm exec vitest run tests/http/store-unavailable-response.test.ts`
Expected: FAIL, the module `src/http/store-unavailable` does not exist.

- [ ] **Step 3: Implement**

Create `PKG/src/http/store-unavailable.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The 503 that an auth route returns when the session store is unavailable (SMA-653, SMA-506
// design § 7.2: "503 with a retry affordance; no partial state").
//
// PRIVATE to this package: routes.ts is the only caller, and server.ts does not export it.
//
// THE PAGE. A fixed HTML document with one retry control: a link for login and callback, and a
// POST form for logout (a link cannot send a POST). It carries NO error text: the error message of
// a SessionStoreUnavailable holds the redacted DSN (adapters/redis-store.ts), and the response is
// not a place for it in any form.
//
// THE HEADERS (spec § 3).
//   - Retry-After: 5. With the shipped 1000 ms store timeout, the SMA-651 circuit cooldown is
//     4000 ms, so a retry after 5 s reaches a closed or re-probing circuit. Advisory only.
//   - Cache-Control: no-store. An error page must never be served from a cache.
//   - Referrer-Policy: no-referrer. A callback 503 is served at /auth/callback?code=…&state=…, and
//     the code may not be spent yet. The retry link must not send that URL as a Referer.
//   - The CSP makes a future escaping defect inert, forbids framing, and `base-uri 'none'` stops an
//     injected <base> from moving the absolute-path links.
//   - NO Set-Cookie. The browser keeps the state it had before the request (spec D4).
//
// ESCAPING. The retry target can hold an attacker-influenced `returnTo` (validated to a same-origin
// path, but still hostile bytes). Two layers: loginRetryHref URL-encodes it as a query value, and
// storeUnavailableResponse HTML-escapes the whole attribute into a DOUBLE-quoted attribute.
import { isSessionStoreUnavailable } from '../core/errors';
import { sidTag, type AuthLogger, type StoreUnavailableStage } from '../ports/logger';

export const RETRY_AFTER_SECONDS = 5;

export const STORE_UNAVAILABLE_CSP = "default-src 'none'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'";

export type RetryAffordance = { kind: 'link'; href: string } | { kind: 'post'; action: string };

/** HTML-escapes a value for a double-quoted attribute. `&` goes first, so no escape is doubled. */
export function escapeHtmlAttribute(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

/** The login route, with `returnTo` URL-encoded as its query value when one is given. */
export function loginRetryHref(basePath: string, returnTo?: string): string {
  const login = `${basePath}/auth/login`;
  return returnTo === undefined ? login : `${login}?returnTo=${encodeURIComponent(returnTo)}`;
}

export function storeUnavailableResponse(retry: RetryAffordance): Response {
  const signOut = retry.kind === 'post';
  const heading = signOut ? 'Sign-out did not complete' : 'Sign-in is temporarily unavailable';
  const sentence = signOut
    ? 'You are still signed in. The session service did not answer. Try again in a few seconds.'
    : 'The session service did not answer. Try again in a few seconds.';
  const control =
    retry.kind === 'post'
      ? `<form method="post" action="${escapeHtmlAttribute(retry.action)}"><button type="submit">Sign out again</button></form>`
      : `<p><a href="${escapeHtmlAttribute(retry.href)}">Try again</a></p>`;
  const body = [
    '<!doctype html>',
    '<html lang="en">',
    `<head><meta charset="utf-8"><title>${heading}</title></head>`,
    '<body>',
    `<h1>${heading}</h1>`,
    `<p>${sentence}</p>`,
    control,
    '</body>',
    '</html>',
    '',
  ].join('\n');

  return new Response(body, {
    status: 503,
    headers: {
      'Retry-After': String(RETRY_AFTER_SECONDS),
      'Cache-Control': 'no-store',
      'Content-Type': 'text/html; charset=utf-8',
      'Referrer-Policy': 'no-referrer',
      'Content-Security-Policy': STORE_UNAVAILABLE_CSP,
    },
  });
}

/** What `storeStep` needs from the runtime. An `AuthRuntime` satisfies it. */
export interface StoreFailureContext {
  logger: AuthLogger;
  zone: string;
}

/** The value `storeStep` returns in place of a result when the store is unavailable. */
export const STORE_DOWN = Symbol('paigasus.auth.store-down');

/**
 * Runs ONE store call. A store-unavailable error (classified by `code`, spec D2) becomes
 * `STORE_DOWN` plus one `store.unavailable` event. Any other error propagates unchanged.
 *
 * The event holds only the zone, the fixed stage literal and a truncated sid. Never the caught
 * error: its message holds the redacted DSN (ports/logger.ts's redaction contract).
 */
export async function storeStep<T>(
  runtime: StoreFailureContext,
  stage: StoreUnavailableStage,
  sid: string | undefined,
  op: () => Promise<T>,
): Promise<T | typeof STORE_DOWN> {
  try {
    return await op();
  } catch (err) {
    if (!isSessionStoreUnavailable(err)) throw err;
    runtime.logger.event('store.unavailable', { zone: runtime.zone, stage, ...(sid !== undefined ? { sid: sidTag(sid) } : {}) });
    return STORE_DOWN;
  }
}
```

If ESLint rejects the `Promise<T | typeof STORE_DOWN>` union with `@typescript-eslint/no-redundant-type-constituents` (it can, when `T` is `unknown`), keep the signature and fix the lint message by the rule's own advice. Do not disable the rule for the file.

- [ ] **Step 4: Run the tests and verify that they pass**

Run (from `PKG`): `pnpm exec vitest run tests/http/store-unavailable-response.test.ts`
Expected: all PASS.

- [ ] **Step 5: Typecheck and lint**

Run (from repo root): `moon run paigasus-auth-ts:typecheck ts:lint`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-auth/src/http/store-unavailable.ts ts/packages/paigasus-auth/tests/http/store-unavailable-response.test.ts
git commit -m "feat(ts): add the auth-route store-unavailable 503 page (SMA-653)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 3: Login route, and the shared route-test harness

**Files:**
- Create: `PKG/tests/support/store-failure.ts`
- Create: `PKG/tests/http/store-unavailable.test.ts`
- Modify: `PKG/src/http/routes.ts` (header comment lines 22-25; `handleLogin` lines 169-191; imports lines 26-34)

**Interfaces:**
- Consumes (Task 2): `storeStep`, `STORE_DOWN`, `storeUnavailableResponse`, `loginRetryHref`, `type RetryAffordance` from `'./store-unavailable'`.
- Produces (test harness, `tests/support/store-failure.ts`): `SENTINEL_DSN`, `ORIGIN`, `BASE_PATH`, `END_SESSION_URL`, `NEW_REFRESH_TOKEN`, `type FailureKind`, `FAILURE_KINDS`, `type StoreMethod`, `storeError(kind)`, `failingStore(...)`, `type FakeOidc`, `fakeOidc()`, `type Harness`, `harness(failOn, makeError)`, `htmlDecode(value)`, `expectStoreUnavailable(res, expected)`, `expectEventsClean(events)`, `storeUnavailableEvents(events)`. Tasks 4, 5 and 6 use these exact names.

- [ ] **Step 1: Write the harness**

Create `PKG/tests/support/store-failure.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-653 route-test harness: a store that fails on chosen methods, a network-free OIDC fake, and
// the assertions that every 503 row shares.
//
// THE SENTINEL. Every thrown store error carries SENTINEL_DSN in its message, the same way the
// real adapter puts the (redacted) DSN there. expectStoreUnavailable and expectEventsClean then
// assert that no byte of it reaches the body, a header, or a logged field. A test that forgets to
// call them is weaker; every row in store-unavailable.test.ts calls both.
import { expect } from 'vitest';
import { claimsPrincipalResolver } from '../../src/adapters/claims-resolver.js';
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import type { AuthorizationRequest, OidcClient, OidcTokens, RefreshedTokens } from '../../src/adapters/oidc.js';
import { SessionStoreTimeout, SessionStoreUnavailable } from '../../src/core/errors.js';
import { STORE_UNAVAILABLE_CSP } from '../../src/http/store-unavailable.js';
import type { AuthEventFields, AuthEventName } from '../../src/ports/logger.js';
import type { SessionStore } from '../../src/ports/session-store.js';
import type { AuthRuntime } from '../../src/runtime.js';

export const SENTINEL_DSN = 'redis://user:sentinel-pw@redis.invalid:6379';
export const ORIGIN = 'https://rp.example.com';
export const BASE_PATH = '/iam';
export const END_SESSION_URL = 'https://issuer.example.com/logout';
export const NEW_REFRESH_TOKEN = 'new-refresh-token';

export type FailureKind = 'unavailable' | 'timeout';
export const FAILURE_KINDS: readonly FailureKind[] = ['unavailable', 'timeout'];

export type StoreMethod = 'get' | 'set' | 'delete' | 'putTransaction' | 'takeTransaction';

export function storeError(kind: FailureKind): Error {
  return kind === 'unavailable'
    ? new SessionStoreUnavailable(`session store unavailable (${SENTINEL_DSN})`)
    : new SessionStoreTimeout(SENTINEL_DSN, 4000, 'deadline');
}

/** Wraps a real store. Records `method:arg` for each guarded call, and rejects on `failOn`. */
export function failingStore(inner: SessionStore, failOn: ReadonlySet<StoreMethod>, makeError: () => Error, calls: string[]): SessionStore {
  const guard = <T>(method: StoreMethod, arg: string, op: () => Promise<T>): Promise<T> => {
    calls.push(`${method}:${arg}`);
    return failOn.has(method) ? Promise.reject(makeError()) : op();
  };
  return {
    get: (sid) => guard('get', sid, () => inner.get(sid)),
    set: (sid, rec, ttlMs, expectedRev) => guard('set', sid, () => inner.set(sid, rec, ttlMs, expectedRev)),
    delete: (sid) => guard('delete', sid, () => inner.delete(sid)),
    tryAcquireLock: (sid, token, ttlMs) => inner.tryAcquireLock(sid, token, ttlMs),
    releaseLock: (sid, token) => inner.releaseLock(sid, token),
    putTransaction: (txnId, tx, ttlMs) => guard('putTransaction', txnId, () => inner.putTransaction(txnId, tx, ttlMs)),
    takeTransaction: (txnId) => guard('takeTransaction', txnId, () => inner.takeTransaction(txnId)),
    close: () => inner.close(),
  };
}

export interface FakeOidc extends OidcClient {
  revokeCalls: string[];
  /** When true, `revoke` rejects with an error whose message holds SENTINEL_DSN. */
  failRevoke: boolean;
}

/** No network. The code exchange always succeeds and returns NEW_REFRESH_TOKEN. */
export function fakeOidc(): FakeOidc {
  const oidc: FakeOidc = {
    revokeCalls: [],
    failRevoke: false,
    buildAuthorizationUrl: (): Promise<AuthorizationRequest> =>
      Promise.resolve({ url: 'https://issuer.example.com/authorize?client_id=test', codeVerifier: 'a-verifier', nonce: 'a-nonce' }),
    authorizationCodeGrant: (): Promise<OidcTokens> =>
      Promise.resolve({
        accessToken: 'new-access-token',
        refreshToken: NEW_REFRESH_TOKEN,
        expiresIn: 300,
        idTokenClaims: { iss: 'https://issuer.example.com', sub: 'a-subject' },
      }),
    refresh: (): Promise<RefreshedTokens> => Promise.reject(new Error('refresh is not used by the auth routes')),
    revoke: (token: string): Promise<void> => {
      oidc.revokeCalls.push(token);
      return oidc.failRevoke ? Promise.reject(new Error(`revoke failed at ${SENTINEL_DSN}`)) : Promise.resolve();
    },
    buildEndSessionUrl: (): Promise<string> => Promise.resolve(END_SESSION_URL),
  };
  return oidc;
}

export interface Harness {
  runtime: AuthRuntime;
  /** The real store behind the failing wrapper: seed and inspect it directly. */
  inner: MemorySessionStore;
  calls: string[];
  events: Array<[AuthEventName, AuthEventFields]>;
  oidc: FakeOidc;
}

export function harness(failOn: readonly StoreMethod[], makeError: () => Error): Harness {
  const inner = new MemorySessionStore();
  const calls: string[] = [];
  const events: Array<[AuthEventName, AuthEventFields]> = [];
  const oidc = fakeOidc();
  const runtime: AuthRuntime = {
    store: failingStore(inner, new Set(failOn), makeError, calls),
    resolver: claimsPrincipalResolver,
    logger: { event: (name, fields) => void events.push([name, { ...fields }]) },
    oidc,
    publicOrigin: ORIGIN,
    redirectUri: `${ORIGIN}${BASE_PATH}/auth/callback`,
    postLogoutRedirectUri: `${ORIGIN}${BASE_PATH}/`,
    cookieDomainless: true,
    skewMs: 30_000,
    lockTtlMs: 10_000,
    lockWaitMs: 3_000,
    ttlMs: 28_800_000,
    absoluteTtlMs: 86_400_000,
    zone: 'iam',
    basePath: BASE_PATH,
    scopes: 'openid profile email',
  };
  return { runtime, inner, calls, events, oidc };
}

const HTML_ENTITIES: Readonly<Record<string, string>> = { '&amp;': '&', '&lt;': '<', '&gt;': '>', '&quot;': '"', '&#39;': "'" };

export function htmlDecode(value: string): string {
  return value.replace(/&(?:amp|lt|gt|quot|#39);/g, (entity) => HTML_ENTITIES[entity] ?? entity);
}

/** Asserts every § 3 property of a 503, and that the retry target is exactly `target`. */
export async function expectStoreUnavailable(res: Response, expected: { kind: 'link' | 'post'; target: string }): Promise<string> {
  expect(res.status).toBe(503);
  expect(res.headers.get('retry-after')).toBe('5');
  expect(res.headers.get('cache-control')).toBe('no-store');
  expect(res.headers.get('content-type')).toBe('text/html; charset=utf-8');
  expect(res.headers.get('referrer-policy')).toBe('no-referrer');
  expect(res.headers.get('content-security-policy')).toBe(STORE_UNAVAILABLE_CSP);
  expect(res.headers.getSetCookie()).toEqual([]);
  const body = await res.text();
  const attribute = expected.kind === 'link' ? 'href' : 'action';
  const match = new RegExp(`${attribute}="([^"]*)"`).exec(body);
  expect(match, `no ${attribute} attribute in the 503 body`).not.toBeNull();
  expect(htmlDecode(match?.[1] ?? '')).toBe(expected.target);
  if (expected.kind === 'post') expect(body).toContain('method="post"');
  for (const text of [body, ...[...res.headers].map(([name, value]) => `${name}: ${value}`)]) {
    expect(text).not.toContain('sentinel-pw');
    expect(text).not.toContain('redis.invalid');
  }
  return body;
}

/** No logged field of any event holds any part of SENTINEL_DSN. */
export function expectEventsClean(events: ReadonlyArray<[AuthEventName, AuthEventFields]>): void {
  const text = JSON.stringify(events);
  expect(text).not.toContain('sentinel-pw');
  expect(text).not.toContain('redis.invalid');
}

/** The fields of each `store.unavailable` event, in order. */
export function storeUnavailableEvents(events: ReadonlyArray<[AuthEventName, AuthEventFields]>): AuthEventFields[] {
  return events.filter(([name]) => name === 'store.unavailable').map(([, fields]) => fields);
}
```

- [ ] **Step 2: Write the failing login tests**

Create `PKG/tests/http/store-unavailable.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-653 § 4: each auth route returns a 503 with a retry control when the session store is
// unavailable. One `describe` per route; each § 4 row runs once with SessionStoreUnavailable and
// once with its SessionStoreTimeout subclass. Every row also asserts redaction (the sentinel DSN
// in the thrown error reaches neither the response nor a log).
import { describe, expect, it, vi } from 'vitest';
import { SessionStoreUnavailable } from '../../src/core/errors.js';
import type { SessionRecord } from '../../src/core/session.js';
import { SESSION_COOKIE } from '../../src/http/cookies.js';
import { createAuthRoutes } from '../../src/http/routes.js';
import { sidTag } from '../../src/ports/logger.js';
import {
  BASE_PATH,
  FAILURE_KINDS,
  ORIGIN,
  SENTINEL_DSN,
  expectEventsClean,
  expectStoreUnavailable,
  harness,
  storeError,
  storeUnavailableEvents,
} from '../support/store-failure.js';

const RETURN_TO = '/iam/orgs';
const OLD_SID = 'old-session-id-0123456789';

function record(overrides: Partial<SessionRecord> = {}): SessionRecord {
  return {
    version: 1,
    rev: 0,
    accessToken: 'old-access-token',
    refreshToken: 'old-refresh-token',
    accessExpiresAt: Date.now() + 60_000,
    absoluteExpiresAt: Date.now() + 60_000,
    idTokenClaims: { iss: 'https://issuer.example.com', sub: 'a-subject' },
    principal: { principalPrn: null, issuer: 'https://issuer.example.com', subject: 'a-subject', memberships: [], roleGrants: [], grantsAvailable: false },
    ...overrides,
  };
}

function loginRequest(returnTo: string, sid?: string): Request {
  const init = sid !== undefined ? { headers: { cookie: `${SESSION_COOKIE}=${sid}` } } : undefined;
  return new Request(`${ORIGIN}${BASE_PATH}/auth/login?returnTo=${encodeURIComponent(returnTo)}`, init);
}

describe.each(FAILURE_KINDS)('GET /auth/login with the store down (%s)', (kind) => {
  it('row 1, no session cookie: putTransaction fails -> 503, link to login with returnTo', async () => {
    const h = harness(['putTransaction'], () => storeError(kind));

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO));

    await expectStoreUnavailable(res, { kind: 'link', target: `/iam/auth/login?returnTo=${encodeURIComponent(RETURN_TO)}` });
    expect(storeUnavailableEvents(h.events)).toEqual([{ zone: 'iam', stage: 'login_put_transaction' }]);
    expect(h.events.map(([name]) => name)).not.toContain('login.started');
    expectEventsClean(h.events);
  });

  it('row 1, with a session cookie: the link goes to returnTo (D9), and the session survives', async () => {
    const h = harness(['putTransaction'], () => storeError(kind));
    await h.inner.set(OLD_SID, record(), 60_000, null);

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO, OLD_SID));

    await expectStoreUnavailable(res, { kind: 'link', target: RETURN_TO });
    expect(h.calls).not.toContain(`delete:${OLD_SID}`);
    expect(await h.inner.get(OLD_SID)).not.toBeNull();
    expectEventsClean(h.events);
  });

  // Row 2 needs a presented sid: with no session cookie, handleLogin makes no delete call at all.
  it('row 2: the delete of the presented session fails -> 503, link to returnTo (D9)', async () => {
    const h = harness(['delete'], () => storeError(kind));
    await h.inner.set(OLD_SID, record(), 60_000, null);

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO, OLD_SID));

    await expectStoreUnavailable(res, { kind: 'link', target: RETURN_TO });
    expect(storeUnavailableEvents(h.events)).toEqual([{ zone: 'iam', stage: 'login_delete', sid: sidTag(OLD_SID) }]);
    expect(h.events.map(([name]) => name)).not.toContain('login.started');
    expectEventsClean(h.events);
  });
});

describe('GET /auth/login — escaping and classification', () => {
  const HOSTILE = `/iam/a"b<c>d&e'f#g h`;

  it('round-trips a hostile returnTo through the login link', async () => {
    const h = harness(['putTransaction'], () => storeError('unavailable'));
    const res = await createAuthRoutes(h.runtime).handle(loginRequest(HOSTILE));
    const body = await expectStoreUnavailable(res, { kind: 'link', target: `/iam/auth/login?returnTo=${encodeURIComponent(HOSTILE)}` });
    // encodeURIComponent leaves ' as is, so the HTML layer must escape it.
    expect(body).toContain('&#39;');
    const href = /href="([^"]*)"/.exec(body)?.[1] ?? '';
    expect(new URL(href.replace(/&#39;/g, "'").replace(/&amp;/g, '&'), ORIGIN).searchParams.get('returnTo')).toBe(HOSTILE);
  });

  it('keeps a hostile returnTo inert in the D9 link', async () => {
    const h = harness(['putTransaction'], () => storeError('unavailable'));
    await h.inner.set(OLD_SID, record(), 60_000, null);
    const res = await createAuthRoutes(h.runtime).handle(loginRequest(HOSTILE, OLD_SID));
    const body = await expectStoreUnavailable(res, { kind: 'link', target: HOSTILE });
    expect(body).toContain('href="/iam/a&quot;b&lt;c&gt;d&amp;e&#39;f#g h"');
  });

  it('maps an error from a SECOND copy of core/errors (D2)', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    expect(foreign.SessionStoreUnavailable).not.toBe(SessionStoreUnavailable);
    const h = harness(['putTransaction'], () => new foreign.SessionStoreUnavailable(`session store unavailable (${SENTINEL_DSN})`));

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO));

    await expectStoreUnavailable(res, { kind: 'link', target: `/iam/auth/login?returnTo=${encodeURIComponent(RETURN_TO)}` });
  });

  it('lets any other store error propagate (D2)', async () => {
    const h = harness(['putTransaction'], () => new Error('boom'));
    await expect(createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO))).rejects.toThrow('boom');
  });
});
```

- [ ] **Step 3: Run the tests and verify that they fail**

Run (from `PKG`): `pnpm exec vitest run tests/http/store-unavailable.test.ts`
Expected: the store-down rows and the second-copy row FAIL with a rejected promise (the error escapes `handle()`). `lets any other store error propagate` PASSES already; that is correct.

- [ ] **Step 4: Implement the login mapping**

In `PKG/src/http/routes.ts`:

Add to the imports:

```ts
import { STORE_DOWN, loginRetryHref, storeStep, storeUnavailableResponse, type RetryAffordance } from './store-unavailable';
```

In `handleLogin`, replace the block from `await runtime.store.putTransaction(txnId, …` (line 180) through the closing `}` of `if (presentedSid !== undefined) { await runtime.store.delete(presentedSid); }` (line 191) with:

```ts
  // SMA-653 D9: read the presented sid BEFORE the first store call, because it decides where the
  // 503's retry link points. When this browser holds a session, the link goes to `returnTo`, NOT to
  // /auth/login: during a wedge `requireSession` sends a signed-in user here, the store call below
  // fails first, and the session record and cookie both survive. A retry link to /auth/login would
  // delete that still-valid session once Redis recovers (SMA-651 § 5). `returnTo` has already
  // passed validateReturnTo and the auth-route guard above, so it cannot loop back into this route.
  const presentedSid = readCookies(req.headers.get('cookie')).get(SESSION_COOKIE);
  const retry: RetryAffordance = { kind: 'link', href: presentedSid !== undefined ? returnTo : loginRetryHref(runtime.basePath, returnTo) };

  const put = await storeStep(runtime, 'login_put_transaction', undefined, () =>
    runtime.store.putTransaction(txnId, { codeVerifier: authorization.codeVerifier, nonce: authorization.nonce, returnTo, secretHash: hashSecret(secret), createdAt: Date.now() }, TXN_TTL_MS),
  );
  if (put === STORE_DOWN) return storeUnavailableResponse(retry);

  // I6 (final fix wave): delete the OLD session record here, not only clear its cookie. A
  // single-tab re-login browser-clears __Host-pgs_sid in THIS same 302, so the browser never
  // sends it again — the callback's own `presentedSid` delete (below) therefore never runs for
  // the common case, and a copied cookie stayed live for the full session TTL after the user
  // signed in again. Reading it from the REQUEST (before it is cleared in the response) is what
  // makes this reachable; clearing the browser cookie alone was never enough.
  //
  // If this delete fails, the stored transaction has no cookie and expires unused (spec § 4 row 2).
  if (presentedSid !== undefined) {
    const deleted = await storeStep(runtime, 'login_delete', presentedSid, () => runtime.store.delete(presentedSid));
    if (deleted === STORE_DOWN) return storeUnavailableResponse(retry);
  }
```

Replace the header comment paragraph at lines 22-25 (`// CallbackRejected is allowed to propagate …`) with:

```ts
// CallbackRejected is allowed to propagate as a rejected promise from `handle()`. Presenting it as
// an HTTP response (an error page, § 10 of the design doc) is a caller concern — this package does
// not decide that here, matching how core/single-flight.ts lets its own AuthError subclasses
// propagate rather than swallowing them into a "safe" return value.
//
// A STORE FAILURE IS THE EXCEPTION, and it is mapped HERE (SMA-653 D1). A SessionStoreUnavailable
// from any store call on these routes becomes a 503 with a retry control (http/store-unavailable.ts;
// SMA-506 design § 7.2). Unlike CallbackRejected, the 503 is a fixed rule of the design, not a
// caller choice, and only this file knows WHICH store call failed — the logout route needs that to
// still attempt its delete when its read fails. Every other error still propagates.
```

- [ ] **Step 5: Run the tests and verify that they pass**

Run (from `PKG`): `pnpm exec vitest run tests/http/store-unavailable.test.ts tests/http/login.test.ts tests/http/login-returnto-table.test.ts`
Expected: all PASS.

- [ ] **Step 6: Typecheck and lint**

Run (from repo root): `moon run paigasus-auth-ts:typecheck ts:lint`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add ts/packages/paigasus-auth/tests/support/store-failure.ts ts/packages/paigasus-auth/tests/http/store-unavailable.test.ts ts/packages/paigasus-auth/src/http/routes.ts
git commit -m "fix(ts): return 503 from /auth/login when the session store is down (SMA-653)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 4: Callback route, with a best-effort revoke (D10)

**Files:**
- Modify: `PKG/src/http/routes.ts` (`handleCallback` lines 237 and 281-304; new helper `bestEffortRevoke`; `handleLogout` step 3, lines 377-385)
- Modify: `PKG/tests/http/store-unavailable.test.ts` (append)

**Interfaces:**
- Consumes (Task 2, 3): `storeStep`, `STORE_DOWN`, `storeUnavailableResponse`, `loginRetryHref`; harness names from `tests/support/store-failure.ts`.
- Produces: `async function bestEffortRevoke(runtime: AuthRuntime, refreshToken: string): Promise<boolean>` in `routes.ts` (module-private). Task 5 uses it.

- [ ] **Step 1: Write the failing callback tests**

Add these imports to `PKG/tests/http/store-unavailable.test.ts`:

```ts
import { hashSecret } from '../../src/core/ids.js';
import { txnCookieName } from '../../src/http/cookies.js';
import { NEW_REFRESH_TOKEN, type Harness } from '../support/store-failure.js';
```

(Merge them into the existing import statements from the same modules.)

Append:

```ts
const STATE = 'state-0123456789';
const TXN_SECRET = 'correct-secret-value-32-bytes-ok';

async function seedTransaction(h: Harness): Promise<void> {
  await h.inner.putTransaction(STATE, { codeVerifier: 'a-verifier', nonce: 'a-nonce', returnTo: RETURN_TO, secretHash: hashSecret(TXN_SECRET), createdAt: Date.now() }, 600_000);
}

function callbackRequest(sid?: string): Request {
  const cookies = [`${txnCookieName(STATE)}=${TXN_SECRET}`, ...(sid !== undefined ? [`${SESSION_COOKIE}=${sid}`] : [])];
  return new Request(`${ORIGIN}${BASE_PATH}/auth/callback?code=a-code&state=${STATE}`, { headers: { cookie: cookies.join('; ') } });
}

describe.each(FAILURE_KINDS)('GET /auth/callback with the store down (%s)', (kind) => {
  it('row 3: takeTransaction fails -> 503, link to login, no exchange, no revoke', async () => {
    const h = harness(['takeTransaction'], () => storeError(kind));
    await seedTransaction(h);

    const res = await createAuthRoutes(h.runtime).handle(callbackRequest());

    await expectStoreUnavailable(res, { kind: 'link', target: '/iam/auth/login' });
    expect(storeUnavailableEvents(h.events)).toEqual([{ zone: 'iam', stage: 'callback_take_transaction' }]);
    expect(h.calls).toEqual([`takeTransaction:${STATE}`]);
    expect(h.oidc.revokeCalls).toEqual([]);
    expectEventsClean(h.events);
  });

  it('row 4: the delete of the presented session fails -> revoke the new token, then 503', async () => {
    const h = harness(['delete'], () => storeError(kind));
    await seedTransaction(h);

    const res = await createAuthRoutes(h.runtime).handle(callbackRequest(OLD_SID));

    await expectStoreUnavailable(res, { kind: 'link', target: `/iam/auth/login?returnTo=${encodeURIComponent(RETURN_TO)}` });
    expect(storeUnavailableEvents(h.events)).toEqual([{ zone: 'iam', stage: 'callback_delete', sid: sidTag(OLD_SID) }]);
    expect(h.oidc.revokeCalls).toEqual([NEW_REFRESH_TOKEN]);
    expect(h.calls.some((call) => call.startsWith('set:'))).toBe(false);
    expect(h.events.map(([name]) => name)).not.toContain('session.created');
    expectEventsClean(h.events);
  });

  it('row 5: the set of the new session fails -> revoke the new token, then 503', async () => {
    const h = harness(['set'], () => storeError(kind));
    await seedTransaction(h);

    const res = await createAuthRoutes(h.runtime).handle(callbackRequest());

    await expectStoreUnavailable(res, { kind: 'link', target: `/iam/auth/login?returnTo=${encodeURIComponent(RETURN_TO)}` });
    const setCall = h.calls.find((call) => call.startsWith('set:'));
    expect(setCall).toBeDefined();
    const newSid = (setCall ?? '').slice('set:'.length);
    expect(storeUnavailableEvents(h.events)).toEqual([{ zone: 'iam', stage: 'callback_set', sid: sidTag(newSid) }]);
    expect(h.oidc.revokeCalls).toEqual([NEW_REFRESH_TOKEN]);
    expect(h.events.map(([name]) => name)).not.toContain('session.created');
    expectEventsClean(h.events);
  });

  it('rows 4 and 5: a failing revoke changes nothing in the response or the log', async () => {
    const h = harness(['set'], () => storeError(kind));
    h.oidc.failRevoke = true;
    await seedTransaction(h);

    const res = await createAuthRoutes(h.runtime).handle(callbackRequest());

    await expectStoreUnavailable(res, { kind: 'link', target: `/iam/auth/login?returnTo=${encodeURIComponent(RETURN_TO)}` });
    expect(h.oidc.revokeCalls).toEqual([NEW_REFRESH_TOKEN]);
    expect(storeUnavailableEvents(h.events).map((fields) => fields.stage)).toEqual(['callback_set']);
    expectEventsClean(h.events);
  });
});
```

- [ ] **Step 2: Run the tests and verify that they fail**

Run (from `PKG`): `pnpm exec vitest run tests/http/store-unavailable.test.ts`
Expected: the four new callback rows FAIL (the error escapes `handle()`) for both kinds. The login rows still PASS.

- [ ] **Step 3: Implement**

In `PKG/src/http/routes.ts`, add this helper directly above `handleLogout`'s comment block (above `// POST /auth/logout — AC 3 …`):

```ts
/**
 * RFC 7009 revocation, BEST EFFORT: `true` when the IdP accepted it, `false` on any failure. Never
 * rethrown and never logged as a raw caught error object — it may embed a URL, matching
 * adapters/oidc.ts's own rule. Used by logout step 3, and by the store-failure paths that would
 * otherwise orphan a live refresh token (SMA-653 D6, D10).
 */
async function bestEffortRevoke(runtime: AuthRuntime, refreshToken: string): Promise<boolean> {
  try {
    await runtime.oidc.revoke(refreshToken);
    return true;
  } catch {
    return false;
  }
}
```

In `handleCallback`, replace `const tx = await runtime.store.takeTransaction(state);` with:

```ts
  const tx = await storeStep(runtime, 'callback_take_transaction', undefined, () => runtime.store.takeTransaction(state));
  // No exchange has happened, so nothing is orphaned. The code is not spent, but the retry starts a
  // new login rather than replaying this URL (spec § 11: the page must not carry the code).
  if (tx === STORE_DOWN) return storeUnavailableResponse({ kind: 'link', href: loginRetryHref(runtime.basePath) });
```

In `handleCallback`, replace the block from `const presentedSid = cookies.get(SESSION_COOKIE);` (line 281) through `if (!stored) { throw new Error(…); }` (line 304) with:

```ts
  // SMA-653 D10. From here on the code exchange has SUCCEEDED, so the IdP holds a session (with
  // `offline_access`, an offline one) and a live refresh token. A store failure below must not
  // orphan it: revoke it, best effort, then answer 503. The retry starts a new login, because the
  // code is spent.
  const failAfterExchange = async (): Promise<Response> => {
    if (tokens.refreshToken !== undefined) await bestEffortRevoke(runtime, tokens.refreshToken);
    return storeUnavailableResponse({ kind: 'link', href: loginRetryHref(runtime.basePath, tx.returnTo) });
  };

  const presentedSid = cookies.get(SESSION_COOKIE);
  if (presentedSid !== undefined) {
    const deleted = await storeStep(runtime, 'callback_delete', presentedSid, () => runtime.store.delete(presentedSid));
    if (deleted === STORE_DOWN) return failAfterExchange();
  }
```

Keep the existing session-fixation comment block (lines 271-280) directly above `const failAfterExchange`… — move it so that it sits directly above `const presentedSid = cookies.get(SESSION_COOKIE);`, unchanged.

Then keep `const sid = newSessionId();` through the `const record: SessionRecord = { … };` literal unchanged, and replace the `set` call and its check with:

```ts
  // expectedRev: null means "insert only if absent" — a `false` here means a record already
  // exists at this freshly-minted, 256-bit random sid (review round 1, M2). Astronomically
  // unlikely, but an unchecked write on the session-creation path is still an unchecked write.
  const stored = await storeStep(runtime, 'callback_set', sid, () => runtime.store.set(sid, record, runtime.ttlMs, null));
  if (stored === STORE_DOWN) return failAfterExchange();
  if (!stored) {
    throw new Error('failed to persist a newly minted session: a record already exists at this sid');
  }
```

In `handleLogout` step 3, replace:

```ts
  let revoked = false;
  if (refreshToken !== undefined) {
    try {
      await runtime.oidc.revoke(refreshToken);
      revoked = true;
    } catch {
      // Best-effort: swallowed. `revoked: false` in the event below is the record of this.
    }
  }
```

with:

```ts
  // Best-effort: a failure is swallowed, and `revoked: false` in the event below is its record.
  const revoked = refreshToken !== undefined ? await bestEffortRevoke(runtime, refreshToken) : false;
```

- [ ] **Step 4: Run the tests and verify that they pass**

Run (from `PKG`): `pnpm exec vitest run tests/http/store-unavailable.test.ts tests/http/callback.test.ts tests/http/logout.test.ts`
Expected: all PASS.

- [ ] **Step 5: Typecheck and lint**

Run (from repo root): `moon run paigasus-auth-ts:typecheck ts:lint`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-auth/src/http/routes.ts ts/packages/paigasus-auth/tests/http/store-unavailable.test.ts
git commit -m "fix(ts): return 503 from /auth/callback when the session store is down (SMA-653)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 5: Logout route — delete after a failed read (D5), 503 when the delete fails (D6)

**Files:**
- Modify: `PKG/src/http/routes.ts` (`handleLogout` step 1, lines 354-360, and the comment block above the function, lines 319-349)
- Modify: `PKG/tests/http/store-unavailable.test.ts` (append)

**Interfaces:**
- Consumes: `storeStep`, `STORE_DOWN`, `storeUnavailableResponse` (Task 2); `bestEffortRevoke` (Task 4); harness names (Task 3).

- [ ] **Step 1: Write the failing logout tests**

Add `END_SESSION_URL` to the import from `'../support/store-failure.js'` in `PKG/tests/http/store-unavailable.test.ts`, then append:

```ts
function logoutRequest(sid: string): Request {
  return new Request(`${ORIGIN}${BASE_PATH}/auth/logout`, { method: 'POST', headers: { cookie: `${SESSION_COOKIE}=${sid}` } });
}

describe.each(FAILURE_KINDS)('POST /auth/logout with the store down (%s)', (kind) => {
  it('row 6: the read fails -> the delete still runs, and logout completes (D5)', async () => {
    const h = harness(['get'], () => storeError(kind));
    await h.inner.set(OLD_SID, record(), 60_000, null);

    const res = await createAuthRoutes(h.runtime).handle(logoutRequest(OLD_SID));

    expect(res.status).toBe(302);
    expect(res.headers.get('location')).toBe(END_SESSION_URL);
    expect(res.headers.getSetCookie().some((cookie) => cookie.startsWith(`${SESSION_COOKIE}=;`))).toBe(true);
    expect(h.calls).toEqual([`get:${OLD_SID}`, `delete:${OLD_SID}`]);
    expect(await h.inner.get(OLD_SID)).toBeNull();
    expect(h.oidc.revokeCalls).toEqual([]);
    expect(h.events).toEqual([
      ['store.unavailable', { zone: 'iam', stage: 'logout_get', sid: sidTag(OLD_SID) }],
      ['logout.completed', { zone: 'iam', sid: sidTag(OLD_SID), revoked: false, endSessionRedirected: true }],
    ]);
    expectEventsClean(h.events);
  });

  it('row 7: the delete fails -> revoke the read token, 503 with a POST form, cookie kept (D6)', async () => {
    const h = harness(['delete'], () => storeError(kind));
    await h.inner.set(OLD_SID, record(), 60_000, null);

    const res = await createAuthRoutes(h.runtime).handle(logoutRequest(OLD_SID));

    await expectStoreUnavailable(res, { kind: 'post', target: '/iam/auth/logout' });
    expect(h.oidc.revokeCalls).toEqual(['old-refresh-token']);
    expect(await h.inner.get(OLD_SID)).not.toBeNull();
    expect(h.events).toEqual([['store.unavailable', { zone: 'iam', stage: 'logout_delete', sid: sidTag(OLD_SID) }]]);
    expectEventsClean(h.events);
  });

  it('row 7: a failing revoke still gives the same 503, and logs nothing more', async () => {
    const h = harness(['delete'], () => storeError(kind));
    h.oidc.failRevoke = true;
    await h.inner.set(OLD_SID, record(), 60_000, null);

    const res = await createAuthRoutes(h.runtime).handle(logoutRequest(OLD_SID));

    await expectStoreUnavailable(res, { kind: 'post', target: '/iam/auth/logout' });
    expect(h.events).toEqual([['store.unavailable', { zone: 'iam', stage: 'logout_delete', sid: sidTag(OLD_SID) }]]);
    expectEventsClean(h.events);
  });

  it('row 8: the read and the delete both fail -> 503, no revoke, two events in order', async () => {
    const h = harness(['get', 'delete'], () => storeError(kind));
    await h.inner.set(OLD_SID, record(), 60_000, null);

    const res = await createAuthRoutes(h.runtime).handle(logoutRequest(OLD_SID));

    await expectStoreUnavailable(res, { kind: 'post', target: '/iam/auth/logout' });
    expect(h.oidc.revokeCalls).toEqual([]);
    expect(h.calls).toEqual([`get:${OLD_SID}`, `delete:${OLD_SID}`]);
    expect(h.events).toEqual([
      ['store.unavailable', { zone: 'iam', stage: 'logout_get', sid: sidTag(OLD_SID) }],
      ['store.unavailable', { zone: 'iam', stage: 'logout_delete', sid: sidTag(OLD_SID) }],
    ]);
    expectEventsClean(h.events);
  });
});
```

`clearCookie` (`src/http/cookies.ts:55`) writes `${name}=; HttpOnly; …; Max-Age=0`, so a cleared session cookie starts with `${SESSION_COOKIE}=;`.

- [ ] **Step 2: Run the tests and verify that they fail**

Run (from `PKG`): `pnpm exec vitest run tests/http/store-unavailable.test.ts`
Expected: rows 6, 7 and 8 FAIL for both kinds (the error escapes `handle()`). Row 6 is the D5 proof: it must fail here, before the fix.

- [ ] **Step 3: Implement**

In `handleLogout`, replace step 1:

```ts
  // STEP 1: delete first, before any network call.
  let refreshToken: string | undefined;
  if (sid !== undefined) {
    const rec = await runtime.store.get(sid);
    refreshToken = rec?.refreshToken;
    await runtime.store.delete(sid);
  }
```

with:

```ts
  // STEP 1: delete first, before any network call.
  //
  // The read only finds a refresh token worth revoking in step 3. Its failure must NEVER cost the
  // delete (SMA-653 D5): a failed read leaves `refreshToken` undefined, and the delete still runs.
  // That rescues a TRANSIENT failure. During a real wedge the read opens the SMA-651 circuit, the
  // circuit then refuses the delete at once, and the delete branch below answers 503 — the usual
  // outcome of a wedge (spec § 4 row 8).
  let refreshToken: string | undefined;
  if (sid !== undefined) {
    const rec = await storeStep(runtime, 'logout_get', sid, () => runtime.store.get(sid));
    refreshToken = rec === STORE_DOWN ? undefined : rec?.refreshToken;

    const deleted = await storeStep(runtime, 'logout_delete', sid, () => runtime.store.delete(sid));
    if (deleted === STORE_DOWN) {
      // SMA-653 D6: the record may still be live, so this is NOT a logout. No cookie is cleared (a
      // cleared cookie would show a false "signed out" while a copied cookie stays live) and there
      // is no IdP redirect. The refresh token that the read found is revoked, best effort, so a
      // copied cookie works only until its access token expires. This keeps the § 9.5 order rule:
      // the delete was attempted first, and it failed. The retry form sends the cookie again.
      if (refreshToken !== undefined) await bestEffortRevoke(runtime, refreshToken);
      return storeUnavailableResponse({ kind: 'post', action: `${runtime.basePath}/auth/logout` });
    }
  }
```

In the comment block above `handleLogout` (starting `// POST /auth/logout — AC 3 …`), after the paragraph that ends "…worth revoking in step 3.", add:

```ts
//
// A STORE FAILURE (SMA-653). A failed read does not stop the delete (D5). A failed delete answers
// 503 with a POST retry form and keeps the session cookie (D6): the user must see that logout did
// not finish, never a false "signed out".
```

- [ ] **Step 4: Run the tests and verify that they pass**

Run (from `PKG`): `pnpm exec vitest run tests/http/store-unavailable.test.ts tests/http/logout.test.ts`
Expected: all PASS. `logout.test.ts` asserts the delete-before-IdP order; it must stay green.

- [ ] **Step 5: Typecheck and lint**

Run (from repo root): `moon run paigasus-auth-ts:typecheck ts:lint`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-auth/src/http/routes.ts ts/packages/paigasus-auth/tests/http/store-unavailable.test.ts
git commit -m "fix(ts): make /auth/logout delete after a failed read, and 503 on a failed delete (SMA-653)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 6: Next boundary test and documentation

**Files:**
- Modify: `PKG/tests/http/route-handler.test.ts` (append one `it` in the existing `describe`)
- Modify: `PKG/src/server.ts:72-97` (doc comment of `createAuthRouteHandler`)
- Modify: `PKG/README.md:115-126`
- Modify: `docs/superpowers/specs/2026-09-09-sma-506-auth-design.md` (§ 7.2, after the table at line 447)

**Interfaces:**
- Consumes: `failingStore`, `storeError`, `expectStoreUnavailable` from `tests/support/store-failure.ts` (Task 3).

- [ ] **Step 1: Write the boundary test**

In `PKG/tests/http/route-handler.test.ts`, add to the imports:

```ts
import { expectStoreUnavailable, failingStore, storeError } from '../support/store-failure.js';
```

Inside `describe('createAuthRouteHandler under basePath /iam (SMA-511 spec § 7.1)', …)`, add:

```ts
  it('passes a store-failure 503 through, with a full-path retry link (SMA-653)', async () => {
    runtime = { ...runtime, store: failingStore(new MemorySessionStore(), new Set(['putTransaction']), () => storeError('timeout'), []) };

    const res = await createAuthRouteHandler(runtime)(new Request(`${BIND}/auth/login?returnTo=%2Fiam%2Forgs`));

    await expectStoreUnavailable(res, { kind: 'link', target: '/iam/auth/login?returnTo=%2Fiam%2Forgs' });
  });
```

- [ ] **Step 2: Run the test**

Run (from `PKG`): `pnpm exec vitest run tests/http/route-handler.test.ts`
Expected: PASS (Task 3 already implemented the behaviour). To prove that this test can fail, temporarily change `if (put === STORE_DOWN) return storeUnavailableResponse(retry);` in `routes.ts` to `if (put === STORE_DOWN) throw new Error('mutant');`, run again, see it FAIL, and revert that one line with the Edit tool (not `git checkout`).

- [ ] **Step 3: Update the documentation**

In `PKG/src/server.ts`, in the `createAuthRouteHandler` doc comment, after the numbered item 2 paragraph (ending "…turns into a 500."), add:

```ts
 *
 * A session-store failure is NOT mapped here. `createAuthRoutes` already answers it with a 503
 * `Response` (SMA-653, http/store-unavailable.ts), which this handler returns unchanged.
```

In `PKG/README.md`, replace the two paragraphs that start `**A failed store call can sign the user out.**` and `**A logout can fail during a wedge.**` (lines 115-126) with:

```markdown
**A failed store call on a page can sign the user out.** `requireSession()` treats a store failure
as "no session" and redirects to `/auth/login`. When `/auth/login` then runs against a store that
answers, it deletes the presented session and clears the cookie. So a Redis stall longer than 4T (a
fork stall during BGSAVE, an fsync stall, a failover) can send a user who loads a page in that
window to the login page, and a new sign-in then replaces the old session. Raise T if your Redis can
stall longer than that. Size T against the worst event-loop lag of the Node process too: a stall
longer than 4T in the process itself fires the deadlines before the replies are read.

**The auth routes answer a store failure with a 503 (SMA-653).** When a store call on
`/auth/login`, `/auth/callback` or `/auth/logout` fails, the route returns a 503 with
`Retry-After: 5`, `Cache-Control: no-store`, a strict CSP and no `Set-Cookie`, and a small HTML page
with a retry control:

- `/auth/login`: a link. If the browser holds a session cookie, the link goes to `returnTo`, so a
  session that survived the stall is not deleted by the retry. Otherwise it goes to `/auth/login`.
- `/auth/callback`: a link to `/auth/login`. If the code exchange already succeeded, the route first
  revokes the new refresh token, best effort.
- `/auth/logout`: if only the read fails, the delete still runs and logout completes. If the delete
  fails, the route revokes the refresh token it read (best effort), keeps the session cookie, and
  shows a form that posts to `/auth/logout` again. The user sees that logout did not finish.

Each failed store call logs `store.unavailable` with a `stage` of `login_put_transaction`,
`login_delete`, `callback_take_transaction`, `callback_delete`, `callback_set`, `logout_get` or
`logout_delete` (pages use `get_session`, the refresh lock uses `release_lock`). Do not put an
ingress custom error page or a mesh retry policy for 503 in front of the auth routes: the first
removes the retry control, and the second replays logins and logouts.
```

In `docs/superpowers/specs/2026-09-09-sma-506-auth-design.md`, directly after the § 7.2 table (after the line `| mid-session, \`set\` fails after a successful refresh | § 8.4 |`), add a blank line and:

```markdown
The route behaviour for every store call on `/auth/login`, `/auth/callback` and `/auth/logout` is
specified in `2026-09-19-sma-653-auth-route-store-503-design.md` (SMA-653).
```

- [ ] **Step 4: Format check**

Run (from repo root): `moon run ts:fmt`
Expected: PASS. If it fails, run the repo's Prettier write command that `ts:fmt`'s failure output names, over the changed files only, then re-run `moon run ts:fmt`.

- [ ] **Step 5: Commit**

```bash
git add ts/packages/paigasus-auth/tests/http/route-handler.test.ts ts/packages/paigasus-auth/src/server.ts ts/packages/paigasus-auth/README.md docs/superpowers/specs/2026-09-09-sma-506-auth-design.md
git commit -m "docs(ts): document the auth-route store-failure 503 (SMA-653)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
```

---

### Task 7: Mutation check and full verification

**Files:** none are changed at the end of this task. Every mutation is reverted.

Restore each mutation with the Edit tool, by reversing the exact edit. Do NOT use `git checkout --` or `git stash`: they can discard other uncommitted work, and the stash stack is shared with other sessions.

- [ ] **Step 1: Run the whole package suite once, clean**

Run (from repo root): `moon run paigasus-auth-ts:test --force`
Expected: PASS.

- [ ] **Step 2: Run each mutation, one at a time**

For each row below: make the edit, run (from `PKG`) `pnpm exec vitest run tests/http tests/core/errors.test.ts tests/next/get-session.test.ts`, record which test failed, then revert the edit. A mutation that leaves every test green is a finding: stop and report it. Do not weaken a test to fix it.

| # | File | Mutation | A test that must fail |
|---|---|---|---|
| M1 | `src/http/routes.ts` | login: replace the `login_put_transaction` `storeStep(…)` wrapper with a direct `await runtime.store.putTransaction(…)` and delete the `STORE_DOWN` line after it | row 1 |
| M2 | `src/http/routes.ts` | login: replace the `login_delete` `storeStep` with a direct `await runtime.store.delete(presentedSid)` | row 2 |
| M3 | `src/http/routes.ts` | callback: replace the `callback_take_transaction` `storeStep` with a direct call | row 3 |
| M4 | `src/http/routes.ts` | callback: replace the `callback_delete` `storeStep` with a direct call | row 4 |
| M5 | `src/http/routes.ts` | callback: replace the `callback_set` `storeStep` with a direct call | row 5 |
| M6 | `src/http/routes.ts` | logout: replace the `logout_get` `storeStep` with a direct `await runtime.store.get(sid)` | row 6 |
| M7 | `src/http/routes.ts` | logout: replace the `logout_delete` `storeStep` with a direct call | rows 7, 8 |
| M8 | `src/http/routes.ts` | logout: restore the old order — make a `get` failure skip the delete (return the 503 when `rec === STORE_DOWN`, before the delete) | row 6 |
| M9 | `src/core/errors.ts` | make `isSessionStoreUnavailable` return `err instanceof SessionStoreUnavailable` | `errors.test.ts` second-copy test; login second-copy row |
| M10 | `src/next/get-session.ts` | use `err instanceof SessionStoreUnavailable` (re-import the class) | the D8 get-session test |
| M11 | `src/http/routes.ts` | D9: make `retry.href` always `loginRetryHref(runtime.basePath, returnTo)` | row 1 with a session cookie; row 2 |
| M12 | `src/http/store-unavailable.ts` | `loginRetryHref`: drop `encodeURIComponent` (use `returnTo` raw) | `loginRetryHref` round trip; login hostile round trip |
| M13 | `src/http/store-unavailable.ts` | `storeUnavailableResponse`: drop `escapeHtmlAttribute` on the href | exact-bytes test; D9 hostile test |
| M14 | `src/http/store-unavailable.ts` | remove `Retry-After` (then, separately, each of `Cache-Control`, `Referrer-Policy`, `Content-Security-Policy`, `Content-Type`) | the builder header test; every 503 row |
| M15 | `src/http/routes.ts` | callback: delete the `bestEffortRevoke` line in `failAfterExchange` | rows 4, 5 |
| M16 | `src/http/routes.ts` | logout: delete the `bestEffortRevoke` line in the delete-failure branch | row 7 |
| M17 | `src/http/store-unavailable.ts` | `storeStep`: log `{ …, error: String(err) }` in the event fields | the `storeStep` redaction test; `expectEventsClean` in every row |

- [ ] **Step 3: Confirm that the tree is clean**

Run: `git status --short`
Expected: no changes (every mutation is reverted).

- [ ] **Step 4: Run the full CI graph for the affected projects**

Run (from repo root): `moon run paigasus-auth-ts:test paigasus-auth-ts:typecheck ts:lint ts:fmt iam-console-ts:typecheck gateway-console-ts:typecheck --force`
Expected: PASS. Report the exact output of any failure. Do not re-run a failure away: capture it first (CLAUDE.md, "Diagnosing an unattributed `moon ci` failure", step 0).

- [ ] **Step 5: Report**

Report the mutation table with the test that failed for each row, and the output summary of Step 4. There is nothing to commit in this task.
