# SMA-656 Discovery 503 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `@paigasus/auth`'s `/auth/login` and `/auth/callback` answer an OIDC discovery failure with a 503 that has a retry link and names the identity provider. The route logs one `oidc.discovery_failed` event with a closed `reason`. No library error object and no URL gets into a response or a log.

**Architecture:** The adapter's `getConfig()` throws a new `OidcDiscoveryFailed` (in `src/core/errors.ts`) with a `reason` that `classifyDiscoveryError` sets from the library error. The routes classify the error by its `code` (`isOidcDiscoveryFailed`), because Next loads two copies of the package that share one runtime. One private route helper logs the event and builds the 503 with the existing store-503 builder, which gets an optional `service` field on its `link` variant.

**Tech Stack:** TypeScript 5 (ESM, `verbatimModuleSyntax`, `exactOptionalPropertyTypes`), Vitest 5.0.1, openid-client 6.8.8 with oauth4webapi 3.8.8, Node 24, Moon 2.5.3, pnpm 11.

**Spec:** `docs/superpowers/specs/2026-09-26-sma-656-auth-login-oidc-503-design.md`. Read all of it before you start. Decision, acceptance and test numbers (D1–D10, A1–A4, T0–T16, § 4.6) in this plan refer to it.

## Global Constraints

- Worktree: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-656`. Branch: `feature/sma-656-auth-login-discovery-503`. Before the first commit of each task, run `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-656 branch --show-current` and confirm the branch name.
- `PKG` below means `ts/packages/paigasus-auth` in the worktree.
- Shell setup for every command: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Every new source file starts with `// SPDX-License-Identifier: Apache-2.0`.
- Relative imports in `PKG/src/` are EXTENSIONLESS (`'../core/errors'`). Test files import with `.js` (`'../../src/core/errors.js'`), as the existing tests do.
- Commits: conventional, scope `ts` (`feat(ts): …`, `fix(ts): …`, `test(ts): …`, `docs(ts): …`), subject ends with `(SMA-656)`. The message ends with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>` after a blank line. Use `git commit -m "<subject>" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"`.
- Never use `--no-verify`. Never use `git commit --amend`. Never use `git reset`. Never use a bare `git stash`. Never use `git checkout -- <file>` to undo a mutation: restore by an edit.
- Do not put a `#NNN` line or a `token: value` line in a commit body (commitlint `footer-leading-blank`). The only trailer is the `Co-Authored-By` line.
- Do not install software. Do not start background jobs. Run every command in the foreground.
- The page sentence for the identity provider is exactly: `The identity provider is not available. Try again in a few seconds.`
- The event name is exactly `oidc.discovery_failed`. Its fields are exactly `{ zone, stage, reason }`. `stage` is `'login' | 'callback'`.
- The reason list, in this order: `timeout`, `network`, `dns`, `tls`, `http_server_error`, `http_client_error`, `invalid_metadata`, `issuer_mismatch`, `other`.
- No event and no response may hold a caught error object, an error message, an error name or a URL (`src/ports/logger.ts` redaction contract; A2).
- Run ONE test file (from `PKG`): `pnpm exec vitest run <path>`. The package has NO `test` script in `package.json`, so `pnpm --filter @paigasus/auth test` is a SILENT NO-OP: it exits 0 and runs no test. Do not use it. Read the `Test Files … passed` line: the Moon task runs vitest with `--passWithNoTests`, so a wrong path filter passes with zero files.
- Run the package suite (from the worktree root): `moon run paigasus-auth-ts:test`. It runs `pnpm exec vitest run --passWithNoTests` in `PKG`.
- Typecheck (a separate Moon target; a green vitest does NOT mean `tsc` passes): `moon run paigasus-auth-ts:typecheck` (from the worktree root). It runs `pnpm exec tsc -p tsconfig.json --noEmit` in `PKG`.
- Lint: `moon run ts:lint` (from the worktree root; `pnpm exec eslint .` over `ts/`). A faster local check: `pnpm -C ts exec eslint packages/paigasus-auth`.
- Prettier (a separate gate, `ts:fmt`): after you touch a file under `ts/`, run `pnpm -C ts exec prettier --write <paths relative to ts/>` for the files you changed, then `moon run ts:fmt`. `printWidth` is 200.
- After every task that touches a `.ts` file: run typecheck, lint and Prettier before the commit.

## Review Focus

These five input classes and failure modes are the most likely to escape the spec's own tests. Each one has a test in the task named.

1. **Hostile `returnTo` bytes in the IdP 503 link.** The IdP page reuses the builder, but no test sends a hostile `returnTo` through a discovery 503. Expected: the link query is URL-encoded, the attribute is HTML-escaped, and the round trip gives the original bytes, on login (with and without a session cookie) and on the callback. Tests: Task 3 (builder), Task 5 (login rows), Task 6 (callback row).
2. **A reload of the callback 503.** The page is served at `/auth/callback?code=…&state=…`, so a reload sends the same callback again. Expected: `CallbackRejected` with reason `state_unknown`, one `login.callback_rejected` event, and no second `oidc.discovery_failed` event (D10). Test: Task 6.
3. **A retry after a failed discovery.** The 503's `Retry-After` and the retry link assume that the next call runs discovery again (D5). Expected: two calls on one client make two discovery requests. Test: Task 4 (and mutation M15 in Task 9).
4. **The status boundaries and a cause that is not a `Response`.** D8 rows 2 and 3 read `cause.status` only as a range. Expected: 499 → `http_client_error`; 500, 503 and 599 → `http_server_error`; no cause, or a plain object with `status: 503`, → `http_client_error`; `status` on any other code is not read. Test: Task 4 (T9b).
5. **A fetch that never connects compared with a refused connection.** `http://127.0.0.1:1` is on the Fetch "bad port" list: undici refuses it before any connect, with `cause` `Error: bad port` and no `code` (measured on Node 24.16.0 while this plan was written). So the spec's "unreachable issuer" row measures the D8 row 11 "none" branch, not a refused connection. Expected: both give `network`. Test: Task 4 adds a closed-port row (a real `ECONNREFUSED`) next to the `127.0.0.1:1` row.

## File Structure

| File | Change | Task |
|---|---|---|
| `PKG/tests/http/store-unavailable-response.test.ts` | modify: T0 goldens, T8 | 1, 3 |
| `PKG/src/core/errors.ts` | modify: the reason list, `OidcDiscoveryFailed`, `isOidcDiscoveryFailed`, `oidcDiscoveryReason`; the D7 site count | 2, 4 |
| `PKG/tests/core/errors.test.ts` | modify: T10 | 2 |
| `PKG/src/ports/logger.ts` | modify: the event name, `OidcDiscoveryStage` | 3 |
| `PKG/src/http/store-unavailable.ts` | modify: `RetryLink`, the `service` field, the header | 3 |
| `PKG/src/adapters/oidc.ts` | modify: `libraryErrorName`, `classifyDiscoveryError`, the `getConfig` throw, the header | 4 |
| `PKG/tests/fixtures/discovery-failures.ts` | create: the failing discovery server | 4 |
| `PKG/tests/adapters/oidc.test.ts` | modify: T9, T9b | 4 |
| `PKG/tests/support/store-failure.ts` | modify: `FakeOidc` fields, shared callback helpers, parameterized sentinels, header | 5 |
| `PKG/tests/http/store-unavailable.test.ts` | modify: use the shared callback helpers | 5 |
| `PKG/tests/http/discovery-failed.test.ts` | create: T1–T7, T12–T15 | 5, 6 |
| `PKG/src/http/routes.ts` | modify: the helper, `handleLogin`, `handleCallback`, the header | 5, 6 |
| `PKG/tests/http/route-handler.test.ts` | modify: T11, T16 | 7 |
| `PKG/src/server.ts` | modify: doc comments only | 7 |
| `PKG/README.md` | modify: the discovery 503 | 8 |
| `docs/superpowers/specs/2026-09-09-sma-506-auth-design.md` | modify: § 7.1 pointer, § 12 event | 8 |

---

### Task 1: Golden tests for the store 503 page (T0)

T0 runs on the UNCHANGED code and is committed before any production change. It passes now and must pass with no edit after Task 3. That is the proof of "byte-identical" in D4.

**Files:**
- Modify: `PKG/tests/http/store-unavailable-response.test.ts` (constants after line 10; a new `describe` after line 55)

**Interfaces:**
- Consumes: `storeUnavailableResponse(retry: RetryAffordance): Response` from `src/http/store-unavailable.ts` (unchanged).
- Produces: the module constants `LINK_GOLDEN` and `POST_GOLDEN` in the test file. Task 3 uses `LINK_GOLDEN`.

- [ ] **Step 1: Write the golden tests**

In `PKG/tests/http/store-unavailable-response.test.ts`, after line 10 (`const HOSTILE = …;`), add:

```ts

// SMA-656 T0. The full page bodies, written on the code BEFORE SMA-656 changed the builder. They
// must pass with no edit after that change: that is the proof that every SMA-653 call site's page
// stays byte-identical (SMA-656 D4).
const LINK_GOLDEN = [
  '<!doctype html>',
  '<html lang="en">',
  '<head><meta charset="utf-8"><title>Sign-in is temporarily unavailable</title></head>',
  '<body>',
  '<h1>Sign-in is temporarily unavailable</h1>',
  '<p>The session service did not answer. Try again in a few seconds.</p>',
  '<p><a href="/iam/auth/login">Try again</a></p>',
  '</body>',
  '</html>',
  '',
].join('\n');

const POST_GOLDEN = [
  '<!doctype html>',
  '<html lang="en">',
  '<head><meta charset="utf-8"><title>Sign-out did not complete</title></head>',
  '<body>',
  '<h1>Sign-out did not complete</h1>',
  '<p>The session service did not answer, so your session may still be active. Try again in a few seconds.</p>',
  '<form method="post" action="/iam/auth/logout"><button type="submit">Sign out again</button></form>',
  '</body>',
  '</html>',
  '',
].join('\n');
```

After the closing `});` of `describe('storeUnavailableResponse', …)` (line 55), add:

```ts

describe('storeUnavailableResponse — golden bodies (SMA-656 T0)', () => {
  it('the link page is byte-identical', async () => {
    expect(await storeUnavailableResponse({ kind: 'link', href: '/iam/auth/login' }).text()).toBe(LINK_GOLDEN);
  });

  it('the post page is byte-identical', async () => {
    expect(await storeUnavailableResponse({ kind: 'post', action: '/iam/auth/logout' }).text()).toBe(POST_GOLDEN);
  });
});
```

- [ ] **Step 2: Run the tests and see them pass on the unchanged code**

Run (from `PKG`): `pnpm exec vitest run tests/http/store-unavailable-response.test.ts`
Expected: PASS, `Tests  14 passed (14)`. A golden test is red-first only against a change, so here it must PASS. If it fails, the golden text is wrong: correct the golden from the builder output, not the builder. Mutation M10 in Task 9 proves that T0 can fail.

- [ ] **Step 3: Format, lint, commit**

Run: `pnpm -C ts exec prettier --write packages/paigasus-auth/tests/http/store-unavailable-response.test.ts`, then `pnpm -C ts exec eslint packages/paigasus-auth/tests/http/store-unavailable-response.test.ts`.
Expected: no lint error.

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-656 branch --show-current
git add ts/packages/paigasus-auth/tests/http/store-unavailable-response.test.ts
git commit -m "test(ts): pin the golden store 503 page bodies (SMA-656)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The `OidcDiscoveryFailed` error and its classifiers (T10)

**Files:**
- Modify: `PKG/src/core/errors.ts` (insert after `isRefreshRejected`, which ends at line 85)
- Modify: `PKG/tests/core/errors.test.ts` (import line 8; append after line 71)

**Interfaces:**
- Produces, in `src/core/errors.ts`:
  - `export const OIDC_DISCOVERY_FAILURE_REASONS: readonly ['timeout', 'network', 'dns', 'tls', 'http_server_error', 'http_client_error', 'invalid_metadata', 'issuer_mismatch', 'other']`
  - `export type OidcDiscoveryFailureReason = (typeof OIDC_DISCOVERY_FAILURE_REASONS)[number]`
  - `export class OidcDiscoveryFailed extends AuthError { readonly code = 'oidc_discovery_failed'; readonly reason: OidcDiscoveryFailureReason; constructor(message: string, reason: OidcDiscoveryFailureReason) }`
  - `export function isOidcDiscoveryFailed(err: unknown): boolean`
  - `export function oidcDiscoveryReason(err: unknown): OidcDiscoveryFailureReason`
- Consumes: the private `hasAuthErrorCode(err: unknown, code: string): boolean` (errors.ts:68).

- [ ] **Step 1: Write the failing tests**

In `PKG/tests/core/errors.test.ts`, replace line 8:

```ts
import { CallbackRejected, RefreshRejected, SessionStoreTimeout, SessionStoreUnavailable, isRefreshRejected, isSessionStoreUnavailable } from '../../src/core/errors.js';
```

with:

```ts
import {
  AuthError,
  CallbackRejected,
  OIDC_DISCOVERY_FAILURE_REASONS,
  OidcDiscoveryFailed,
  RefreshRejected,
  SessionStoreTimeout,
  SessionStoreUnavailable,
  isOidcDiscoveryFailed,
  isRefreshRejected,
  isSessionStoreUnavailable,
  oidcDiscoveryReason,
} from '../../src/core/errors.js';
```

Append at the end of the file (after line 71):

```ts

// SMA-656 D1. The runtime and its `oidc` client are shared through globalThis, so the adapter can
// build an OidcDiscoveryFailed in one module copy and http/routes.ts can classify it in the other.
describe('isOidcDiscoveryFailed (SMA-656 D1, T10)', () => {
  it('is true for an OidcDiscoveryFailed', () => {
    expect(isOidcDiscoveryFailed(new OidcDiscoveryFailed('oidc discovery failed: TypeError', 'network'))).toBe(true);
  });

  it('is true for an error from a SECOND copy of core/errors', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    // Precondition: without this, the test passes vacuously if the import returns the same module.
    expect(foreign.OidcDiscoveryFailed).not.toBe(OidcDiscoveryFailed);
    const err = new foreign.OidcDiscoveryFailed('oidc discovery failed: TypeError', 'network');
    expect(err instanceof OidcDiscoveryFailed).toBe(false);
    expect(isOidcDiscoveryFailed(err)).toBe(true);
  });

  it('is true for any Error carrying the code, not only an OidcDiscoveryFailed', () => {
    expect(isOidcDiscoveryFailed(Object.assign(new Error('x'), { code: 'oidc_discovery_failed' }))).toBe(true);
  });

  it('is false for other errors and for non-errors', () => {
    // A plain object, not an Error.
    expect(isOidcDiscoveryFailed({ code: 'oidc_discovery_failed' })).toBe(false);
    // The literal in the MESSAGE, not in `code`.
    expect(isOidcDiscoveryFailed(new Error('oidc_discovery_failed'))).toBe(false);
    // What the adapter threw before SMA-656.
    expect(isOidcDiscoveryFailed(new Error('oidc discovery failed: TypeError'))).toBe(false);
    expect(isOidcDiscoveryFailed(new SessionStoreUnavailable('down'))).toBe(false);
    expect(isOidcDiscoveryFailed(new RefreshRejected('invalid_grant'))).toBe(false);
    expect(isOidcDiscoveryFailed(undefined)).toBe(false);
  });

  it('is neither a store failure nor a refresh rejection', () => {
    const err = new OidcDiscoveryFailed('oidc discovery failed: TypeError', 'timeout');
    expect(isSessionStoreUnavailable(err)).toBe(false);
    expect(isRefreshRejected(err)).toBe(false);
  });
});

describe('OidcDiscoveryFailed (SMA-656 D3)', () => {
  it('has a fixed name and code, keeps the message, and has no cause', () => {
    const err = new OidcDiscoveryFailed('oidc discovery failed: ClientError', 'issuer_mismatch');
    expect(err).toBeInstanceOf(AuthError);
    expect(err.name).toBe('OidcDiscoveryFailed');
    expect(err.code).toBe('oidc_discovery_failed');
    expect(err.message).toBe('oidc discovery failed: ClientError');
    expect(err.reason).toBe('issuer_mismatch');
    expect(err.cause).toBeUndefined();
  });
});

describe('oidcDiscoveryReason (SMA-656 D8, T10)', () => {
  it('the closed list is exactly the D8 list, in order', () => {
    expect(OIDC_DISCOVERY_FAILURE_REASONS).toEqual(['timeout', 'network', 'dns', 'tls', 'http_server_error', 'http_client_error', 'invalid_metadata', 'issuer_mismatch', 'other']);
  });

  it.each(OIDC_DISCOVERY_FAILURE_REASONS)('returns %s unchanged', (reason) => {
    expect(oidcDiscoveryReason(new OidcDiscoveryFailed('oidc discovery failed: TypeError', reason))).toBe(reason);
  });

  it.each([
    ['a missing reason', Object.assign(new Error('x'), { code: 'oidc_discovery_failed' })],
    ['a URL', Object.assign(new Error('x'), { code: 'oidc_discovery_failed', reason: 'https://idp.invalid/x' })],
    ['a near miss', Object.assign(new Error('x'), { code: 'oidc_discovery_failed', reason: 'TIMEOUT' })],
    ['a number', Object.assign(new Error('x'), { code: 'oidc_discovery_failed', reason: 42 })],
    ['undefined', undefined],
    ['null', null],
    ['a bare string', 'timeout'],
  ] as const)('returns other for %s', (_label, err) => {
    expect(oidcDiscoveryReason(err)).toBe('other');
  });
});
```

- [ ] **Step 2: Run the tests and see them fail**

Run (from `PKG`): `pnpm exec vitest run tests/core/errors.test.ts`
Expected: FAIL. `OidcDiscoveryFailed`, `isOidcDiscoveryFailed`, `oidcDiscoveryReason` and `OIDC_DISCOVERY_FAILURE_REASONS` are not exported. `it.each(OIDC_DISCOVERY_FAILURE_REASONS)` then gets `undefined`, so vitest can fail the whole file at collection; otherwise the new rows fail with `TypeError: … is not a constructor` or `… is not a function`. Either form is the expected red.

- [ ] **Step 3: Implement**

In `PKG/src/core/errors.ts`, after the closing `}` of `isRefreshRejected` (line 85) and before `/** Configuration is internally inconsistent. …`, insert:

```ts

/** Why OIDC discovery failed (SMA-656 D8). The one list; the type derives from it. */
export const OIDC_DISCOVERY_FAILURE_REASONS = ['timeout', 'network', 'dns', 'tls', 'http_server_error', 'http_client_error', 'invalid_metadata', 'issuer_mismatch', 'other'] as const;
export type OidcDiscoveryFailureReason = (typeof OIDC_DISCOVERY_FAILURE_REASONS)[number];

/**
 * OIDC discovery did not complete (SMA-656). The message holds only the library error's name, and
 * there is no `cause` (D3): the `cause` of an issuer-mismatch ClientError holds the expected issuer
 * and the metadata, the `cause` of a 404 ClientError is a Response with a `url`, and Node prints the
 * `cause` chain when it logs an error. `reason` comes from adapters/oidc.ts's
 * `classifyDiscoveryError` (D8).
 *
 * INVARIANT (D10): thrown only by the adapter's `getConfig()`, which every method awaits before any
 * other request — so when a caller sees this, no token request was sent, no authorization code was
 * spent and no token exists. http/routes.ts's callback 503 depends on this: it does not revoke.
 *
 * `name` is set explicitly for the reason SessionStoreTimeout records: a production bundle can
 * mangle `constructor.name`. NOT exported from src/server.ts, for the SMA-657 D4 reason: a caller
 * of createOidcClient or AuthRuntime.oidc CAN receive one, but no consumer needs to CLASSIFY one.
 */
export class OidcDiscoveryFailed extends AuthError {
  readonly code = 'oidc_discovery_failed';
  readonly reason: OidcDiscoveryFailureReason;

  constructor(message: string, reason: OidcDiscoveryFailureReason) {
    super(message);
    this.name = 'OidcDiscoveryFailed';
    this.reason = reason;
  }
}

/** True for an OidcDiscoveryFailed from ANY copy of this module (SMA-656 D1). The crossing path is
 * real: the runtime and its `oidc` client are shared through globalThis, so adapters/oidc.ts builds
 * this class in whichever copy created the runtime, and http/routes.ts classifies it in whichever
 * copy serves the request. See `hasAuthErrorCode`. */
export function isOidcDiscoveryFailed(err: unknown): boolean {
  return hasAuthErrorCode(err, 'oidc_discovery_failed');
}

/**
 * The `reason` of a caught discovery error, read defensively (SMA-656 D8). A value outside
 * OIDC_DISCOVERY_FAILURE_REASONS — a missing field, a foreign string, a number — gives 'other', so
 * no value that the list does not name can reach a log line.
 */
export function oidcDiscoveryReason(err: unknown): OidcDiscoveryFailureReason {
  const reason: unknown = typeof err === 'object' && err !== null ? (err as { reason?: unknown }).reason : undefined;
  return OIDC_DISCOVERY_FAILURE_REASONS.find((known) => known === reason) ?? 'other';
}
```

(The spec § 4.1 says "after `SessionStoreTimeout` and its classifier". `hasAuthErrorCode` and `isRefreshRejected` sit between them, so the block goes after `isRefreshRejected`, next to the other classifiers.)

- [ ] **Step 4: Run the tests and see them pass**

Run (from `PKG`): `pnpm exec vitest run tests/core/errors.test.ts`
Expected: PASS, every row.

- [ ] **Step 5: Typecheck, lint, format, commit**

Run (from the worktree root): `moon run paigasus-auth-ts:typecheck`. Then `pnpm -C ts exec eslint packages/paigasus-auth/src/core/errors.ts packages/paigasus-auth/tests/core/errors.test.ts`. Then `pnpm -C ts exec prettier --write packages/paigasus-auth/src/core/errors.ts packages/paigasus-auth/tests/core/errors.test.ts`.
Expected: no error.

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-656 branch --show-current
git add ts/packages/paigasus-auth/src/core/errors.ts ts/packages/paigasus-auth/tests/core/errors.test.ts
git commit -m "feat(ts): add the OidcDiscoveryFailed error and its classifiers (SMA-656)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The event name, the stage type and the identity-provider page (T8)

**Files:**
- Modify: `PKG/src/ports/logger.ts` (lines 26–27; after line 36)
- Modify: `PKG/src/http/store-unavailable.ts` (header lines 3–4, 8–11, 14–15; line 36; lines 49–52)
- Modify: `PKG/tests/http/store-unavailable-response.test.ts` (a constant after `POST_GOLDEN`; a new `describe` after the T0 `describe`)

**Interfaces:**
- Produces, in `src/ports/logger.ts`: `'oidc.discovery_failed'` in `AuthEventName`; `export type OidcDiscoveryStage = 'login' | 'callback';`
- Produces, in `src/http/store-unavailable.ts`:
  - `export type RetryLink = { kind: 'link'; href: string; service?: 'session_store' | 'identity_provider' };`
  - `export type RetryAffordance = RetryLink | { kind: 'post'; action: string };`
  - `storeUnavailableResponse(retry: RetryAffordance): Response` (same signature; the `link` variant with `service: 'identity_provider'` gives the D4 sentence).
- Consumes: `LINK_GOLDEN` from Task 1.

- [ ] **Step 1: Write the failing tests**

In `PKG/tests/http/store-unavailable-response.test.ts`, after the `POST_GOLDEN` constant, add:

```ts

// SMA-656 D4, T8. The IdP page: the same heading, the same link, one different sentence.
const IDP_LINK_GOLDEN = [
  '<!doctype html>',
  '<html lang="en">',
  '<head><meta charset="utf-8"><title>Sign-in is temporarily unavailable</title></head>',
  '<body>',
  '<h1>Sign-in is temporarily unavailable</h1>',
  '<p>The identity provider is not available. Try again in a few seconds.</p>',
  '<p><a href="/iam/auth/login">Try again</a></p>',
  '</body>',
  '</html>',
  '',
].join('\n');
```

After the closing `});` of `describe('storeUnavailableResponse — golden bodies (SMA-656 T0)', …)`, add:

```ts

describe('storeUnavailableResponse — the identity provider (SMA-656 D4, T8)', () => {
  it('names the identity provider, with the same headers and no Set-Cookie', async () => {
    const res = storeUnavailableResponse({ kind: 'link', href: '/iam/auth/login', service: 'identity_provider' });
    expect(res.status).toBe(503);
    expect(res.headers.get('retry-after')).toBe('5');
    expect(res.headers.get('cache-control')).toBe('no-store');
    expect(res.headers.get('content-type')).toBe('text/html; charset=utf-8');
    expect(res.headers.get('referrer-policy')).toBe('no-referrer');
    expect(res.headers.get('content-security-policy')).toBe(STORE_UNAVAILABLE_CSP);
    expect(res.headers.getSetCookie()).toEqual([]);
    const body = await res.text();
    expect(body).toBe(IDP_LINK_GOLDEN);
    expect(body).not.toContain('The session service did not answer');
  });

  it('an explicit session_store service gives the T0 page', async () => {
    expect(await storeUnavailableResponse({ kind: 'link', href: '/iam/auth/login', service: 'session_store' }).text()).toBe(LINK_GOLDEN);
  });

  // Review Focus 1: the IdP variant escapes the attribute exactly as the store variant does.
  it('HTML-escapes a hostile href in the IdP page', async () => {
    const body = await storeUnavailableResponse({ kind: 'link', href: HOSTILE, service: 'identity_provider' }).text();
    expect(body).toContain('href="/iam/a&quot;b&lt;c&gt;d&amp;e&#39;f#g h"');
    expect(body).not.toContain('b<c');
  });
});
```

- [ ] **Step 2: Run the tests and see them fail**

Run (from `PKG`): `pnpm exec vitest run tests/http/store-unavailable-response.test.ts`
Expected: FAIL in `names the identity provider, …`: the body holds `The session service did not answer. Try again in a few seconds.`, not the IdP sentence (vitest does not typecheck, so the unknown `service` field runs and the builder ignores it). The other two new rows pass already: the escaping and the default sentence exist today. T0 passes.

- [ ] **Step 3: Implement the logger types**

In `PKG/src/ports/logger.ts`, replace lines 26–27:

```ts
  | 'store.unavailable'
  | 'store.operation_timeout';
```

with:

```ts
  | 'store.unavailable'
  | 'store.operation_timeout'
  | 'oidc.discovery_failed';
```

After line 36 (the `StoreUnavailableStage` union), add:

```ts

/**
 * The closed set of `stage` values for `oidc.discovery_failed` (SMA-656 D7): the route that needed
 * the discovered configuration. `AuthEventFields` is a free record, so the one emitter,
 * http/routes.ts's `discoveryFailedResponse`, takes this type as a typed parameter. A later refresh
 * or logout stage is a new member here, not a new event name.
 */
export type OidcDiscoveryStage = 'login' | 'callback';
```

- [ ] **Step 4: Implement the builder change**

In `PKG/src/http/store-unavailable.ts`, replace lines 3–4:

```ts
// The 503 that an auth route returns when the session store is unavailable (SMA-653, SMA-506
// design § 7.2: "503 with a retry affordance; no partial state").
```

with:

```ts
// The 503 that an auth route returns when the session store is unavailable (SMA-653, SMA-506
// design § 7.2: "503 with a retry affordance; no partial state"), and, since SMA-656, when OIDC
// discovery fails on /auth/login or /auth/callback (SMA-506 design § 7.1: "login fails, 503 page").
// The names `storeUnavailableResponse` and `STORE_UNAVAILABLE_CSP` are historical: they predate the
// identity-provider case, and a rename is out of scope (SMA-656 § 8).
```

Replace lines 8–11:

```ts
// THE PAGE. A fixed HTML document with one retry control: a link for login and callback, and a
// POST form for logout (a link cannot send a POST). It carries NO error text: the error message of
// a SessionStoreUnavailable holds the redacted DSN (adapters/redis-store.ts), and the response is
// not a place for it in any form.
```

with:

```ts
// THE PAGE. A fixed HTML document with one retry control: a link for login and callback, and a
// POST form for logout (a link cannot send a POST). It carries NO error text: the error message of
// a SessionStoreUnavailable holds the redacted DSN (adapters/redis-store.ts), an openid-client error
// can hold a URL, and the response is not a place for either in any form. The link variant's
// optional `service` picks the sentence: the session service (the default, so every SMA-653 call
// site stays byte-identical) or the identity provider (SMA-656 D4). The IdP sentence does not say
// WHY discovery failed: a 404 or an issuer mismatch is not "did not answer".
```

Replace lines 14–15:

```ts
//   - Retry-After: 5. With the shipped 1000 ms store timeout, the SMA-651 circuit cooldown is
//     4000 ms, so a retry after 5 s reaches a closed or re-probing circuit. Advisory only.
```

with:

```ts
//   - Retry-After: 5. With the shipped 1000 ms store timeout, the SMA-651 circuit cooldown is
//     4000 ms, so a retry after 5 s reaches a closed or re-probing circuit. Advisory only. For the
//     identity-provider case it is advisory too: a retry runs discovery again, because the adapter
//     clears its cached discovery after a failure, and that retry can wait up to
//     PAIGASUS_OIDC_HTTP_TIMEOUT_MS for its answer (SMA-656 D5).
```

Replace line 36:

```ts
export type RetryAffordance = { kind: 'link'; href: string } | { kind: 'post'; action: string };
```

with:

```ts
/** The link variant. `service` picks the sentence; a missing field means 'session_store' (SMA-656 D4). */
export type RetryLink = { kind: 'link'; href: string; service?: 'session_store' | 'identity_provider' };

/** The post variant (logout) has no `service`, so the type cannot express a state the builder ignores. */
export type RetryAffordance = RetryLink | { kind: 'post'; action: string };

const SESSION_STORE_SENTENCE = 'The session service did not answer. Try again in a few seconds.';
const IDENTITY_PROVIDER_SENTENCE = 'The identity provider is not available. Try again in a few seconds.';
const SIGN_OUT_SENTENCE = 'The session service did not answer, so your session may still be active. Try again in a few seconds.';

function sentenceFor(retry: RetryAffordance): string {
  if (retry.kind === 'post') return SIGN_OUT_SENTENCE;
  const service = retry.service ?? 'session_store';
  return service === 'identity_provider' ? IDENTITY_PROVIDER_SENTENCE : SESSION_STORE_SENTENCE;
}
```

Replace lines 50–52 (inside `storeUnavailableResponse`):

```ts
  const signOut = retry.kind === 'post';
  const heading = signOut ? 'Sign-out did not complete' : 'Sign-in is temporarily unavailable';
  const sentence = signOut ? 'The session service did not answer, so your session may still be active. Try again in a few seconds.' : 'The session service did not answer. Try again in a few seconds.';
```

with:

```ts
  const signOut = retry.kind === 'post';
  const heading = signOut ? 'Sign-out did not complete' : 'Sign-in is temporarily unavailable';
  const sentence = sentenceFor(retry);
```

`routes.ts` still imports `type RetryAffordance` and compiles: the type keeps its name. Task 5 changes that import to `RetryLink`.

- [ ] **Step 5: Run the tests and see them pass**

Run (from `PKG`): `pnpm exec vitest run tests/http/store-unavailable-response.test.ts tests/http/store-unavailable.test.ts`
Expected: PASS. T0 passes with NO edit to the goldens.

- [ ] **Step 6: Typecheck, lint, format, commit**

Run (from the worktree root): `moon run paigasus-auth-ts:typecheck`. Then `pnpm -C ts exec eslint packages/paigasus-auth/src/ports/logger.ts packages/paigasus-auth/src/http/store-unavailable.ts packages/paigasus-auth/tests/http/store-unavailable-response.test.ts`. Then `pnpm -C ts exec prettier --write packages/paigasus-auth/src/ports/logger.ts packages/paigasus-auth/src/http/store-unavailable.ts packages/paigasus-auth/tests/http/store-unavailable-response.test.ts`.
Expected: no error.

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-656 branch --show-current
git add ts/packages/paigasus-auth/src/ports/logger.ts ts/packages/paigasus-auth/src/http/store-unavailable.ts ts/packages/paigasus-auth/tests/http/store-unavailable-response.test.ts
git commit -m "feat(ts): add the identity-provider 503 page and the oidc.discovery_failed event (SMA-656)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The adapter throws `OidcDiscoveryFailed` with a closed reason (T9, T9b)

**Spec D8 is reasoned, not measured.** T9 measures every row that a local fixture can produce. If a T9 row measures a DIFFERENT `reason` than this plan expects, STOP and report the row, the measured library error (class, `name`, `code`, `cause`) and the measured reason. Do NOT edit the test to match, and do not edit the D8 table yourself: the spec owner corrects the table. (While this plan was written, a direct `client.discovery(…)` call per mode gave: 404 and 503 → `ClientError` `OAUTH_RESPONSE_IS_NOT_CONFORM` with a `Response` cause of that status; `not-json` → `OAUTH_RESPONSE_IS_NOT_JSON`; `wrong-issuer` → `OAUTH_JSON_ATTRIBUTE_COMPARISON_FAILED`; `issuer-not-a-url` → `TypeError` with own `code` `ERR_INVALID_URL`; `hang` → `OAUTH_TIMEOUT`; a closed port → `TypeError` with `cause.code` `ECONNREFUSED`; `127.0.0.1:1` → `TypeError` with `cause` `Error: bad port` and no `code`. All match D8.)

**Files:**
- Create: `PKG/tests/fixtures/discovery-failures.ts`
- Modify: `PKG/src/adapters/oidc.ts` (header lines 18–20; import line 35; `wrapError` lines 128–136; insert after `classifyRefreshError`, which ends at line 172; the `getConfig` catch at lines 204–208)
- Modify: `PKG/src/core/errors.ts` (the rule comment, lines 57–59 before Task 2; find it by its text)
- Modify: `PKG/tests/adapters/oidc.test.ts` (imports lines 12–16; append after line 326)

**Interfaces:**
- Produces, in `src/adapters/oidc.ts`: `export function classifyDiscoveryError(err: unknown): OidcDiscoveryFailureReason` (for tests only; NOT re-exported from `src/server.ts`); the private `libraryErrorName(err: unknown): string`. Every `OidcClient` method now rejects with `OidcDiscoveryFailed` when discovery fails.
- Produces, in `tests/fixtures/discovery-failures.ts`: `type DiscoveryFailureMode = 'status-404' | 'status-503' | 'not-json' | 'wrong-issuer' | 'issuer-not-a-url' | 'hang'`; `interface DiscoveryFailureFixture { readonly issuer: string; readonly requests: number; close(): Promise<void> }`; `startDiscoveryFailureFixture(mode: DiscoveryFailureMode): Promise<DiscoveryFailureFixture>`; `closedPortIssuer(): Promise<string>`.
- Consumes: `OidcDiscoveryFailed`, `OidcDiscoveryFailureReason`, `isOidcDiscoveryFailed` from Task 2.

- [ ] **Step 1: Read the callers again (spec § 4.2)**

Read each site and confirm that no change is needed. Every public adapter method calls `getConfig()`, so each now receives `OidcDiscoveryFailed`:
- `PKG/src/http/routes.ts` `handleLogout`, the `try { … buildEndSessionUrl … } catch { … }` near line 506: it catches every error and falls back to `runtime.postLogoutRedirectUri`. No change.
- `PKG/src/http/routes.ts` `bestEffortRevoke` (line 361) and `PKG/src/core/single-flight.ts` `revokeAll` (line 60): they discard every error with no log line. No change.
- `PKG/src/core/single-flight.ts` near line 208: the refresh path classifies only `isRefreshRejected`, so this error stays a transient failure. No change.
- `PKG/src/next/get-session.ts` near line 66: it catches and logs `session.resolve_failed`. No change.
- `PKG/tests/e2e/fixture-server.ts` near line 169: its diagnostic line now prints `OidcDiscoveryFailed`, not `Error`. No change.
If a site differs from this description, STOP and report it.

- [ ] **Step 2: Create the fixture module**

Create `PKG/tests/fixtures/discovery-failures.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-656 T9: a local HTTP server whose discovery document fails in one chosen way, so
// tests/adapters/oidc.test.ts can measure the `reason` that the REAL adapter gives. startOidcFixture
// (jwks.ts) has no hook for these cases, and a mode there would widen every caller of that fixture.
//
// OPEN HANDLES. The `hang` mode accepts a request and never answers. close() therefore destroys
// every socket the server holds BEFORE it calls server.close(): server.close() alone waits for the
// open connections, and it would wait for ever on a hung one. Every caller closes the fixture in an
// afterEach, so a failed assertion does not leave a server or a socket open.
import { createServer, type Server } from 'node:http';
import type { AddressInfo, Socket } from 'node:net';

export type DiscoveryFailureMode = 'status-404' | 'status-503' | 'not-json' | 'wrong-issuer' | 'issuer-not-a-url' | 'hang';

export interface DiscoveryFailureFixture {
  /** The issuer to give createOidcClient. Plain http on 127.0.0.1, so it needs allowInsecureRequests. */
  readonly issuer: string;
  /** How many HTTP requests the server received. A row asserts on it, so it cannot pass without reaching the server. */
  readonly requests: number;
  close(): Promise<void>;
}

export async function startDiscoveryFailureFixture(mode: DiscoveryFailureMode): Promise<DiscoveryFailureFixture> {
  const sockets = new Set<Socket>();
  let issuer = '';
  let requests = 0;

  const server: Server = createServer((_req, res) => {
    requests += 1;
    const metadata = (issuerValue: string): void => {
      res.writeHead(200, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ issuer: issuerValue, authorization_endpoint: `${issuer}/authorize`, token_endpoint: `${issuer}/token`, jwks_uri: `${issuer}/jwks` }));
    };
    switch (mode) {
      case 'status-404':
        // A wrong realm: the path does not exist.
        res.writeHead(404, { 'content-type': 'text/plain' });
        res.end('not found');
        return;
      case 'status-503':
        // An ingress in front of an IdP that is down.
        res.writeHead(503, { 'content-type': 'text/plain' });
        res.end('service unavailable');
        return;
      case 'not-json':
        // An HTML page with a 200 in place of the document.
        res.writeHead(200, { 'content-type': 'text/html' });
        res.end('<!doctype html><title>sign in</title>');
        return;
      case 'wrong-issuer':
        // A valid URL that is not the configured issuer.
        metadata(`${issuer}/other-realm`);
        return;
      case 'issuer-not-a-url':
        metadata('not a url');
        return;
      case 'hang':
        // Never answer. close() destroys the socket.
        return;
    }
  });
  server.on('connection', (socket) => {
    sockets.add(socket);
    socket.on('close', () => sockets.delete(socket));
  });

  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as AddressInfo;
  issuer = `http://127.0.0.1:${String(port)}`;

  return {
    issuer,
    get requests(): number {
      return requests;
    },
    close(): Promise<void> {
      for (const socket of sockets) socket.destroy();
      return new Promise<void>((resolve, reject) => {
        server.close((err) => (err ? reject(err) : resolve()));
      });
    },
  };
}

/**
 * An issuer on a port that nothing listens on: the port was bound, then released, so a connect gets
 * ECONNREFUSED. Not `127.0.0.1:1`: port 1 is on the Fetch "bad port" list, so undici refuses it
 * BEFORE any connect, with a cause that has no `code` (Review Focus 5).
 */
export async function closedPortIssuer(): Promise<string> {
  const server = createServer();
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as AddressInfo;
  await new Promise<void>((resolve, reject) => {
    server.close((err) => (err ? reject(err) : resolve()));
  });
  return `http://127.0.0.1:${String(port)}`;
}
```

- [ ] **Step 3: Write the failing tests**

In `PKG/tests/adapters/oidc.test.ts`, replace lines 12–16:

```ts
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import * as client from 'openid-client';
import { createOidcClient, type OidcClient } from '../../src/adapters/oidc.js';
import { startOidcFixture, type OidcFixture } from '../fixtures/jwks.js';
import { RefreshRejected } from '../../src/core/errors.js';
```

with:

```ts
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import * as client from 'openid-client';
import { classifyDiscoveryError, createOidcClient, type OidcClient } from '../../src/adapters/oidc.js';
import { startOidcFixture, type OidcFixture } from '../fixtures/jwks.js';
import { closedPortIssuer, startDiscoveryFailureFixture, type DiscoveryFailureFixture } from '../fixtures/discovery-failures.js';
import { OidcDiscoveryFailed, RefreshRejected, isOidcDiscoveryFailed, type OidcDiscoveryFailureReason } from '../../src/core/errors.js';
```

Append at the end of the file (after line 326):

```ts

// ---------------------------------------------------------------------------------------------
// SMA-656 T9. Each row makes discovery fail in one way, through the REAL adapter, and measures the
// `reason`. The D8 table was reasoned from the library code; these rows are its measurement. If a
// row measures another reason, the D8 table is corrected, not this test.
// ---------------------------------------------------------------------------------------------
describe('createOidcClient — a discovery failure is an OidcDiscoveryFailed with a closed reason (SMA-656 T9)', () => {
  let failing: DiscoveryFailureFixture | undefined;

  afterEach(async () => {
    await failing?.close();
    failing = undefined;
  });

  function clientFor(issuer: string, httpTimeoutMs = 2000): OidcClient {
    return createOidcClient({ issuer, clientId: 'test-client', clientSecret: 'test-secret', httpTimeoutMs, clockToleranceSeconds: 30, allowInsecureRequests: true });
  }

  function buildUrl(oidc: OidcClient): Promise<unknown> {
    return oidc.buildAuthorizationUrl({ redirectUri: REDIRECT_URI, scopes: 'openid', state: STATE });
  }

  function expectDiscoveryFailed(err: unknown, reason: OidcDiscoveryFailureReason): void {
    expect(isOidcDiscoveryFailed(err)).toBe(true);
    expect(err).toBeInstanceOf(OidcDiscoveryFailed);
    const failed = err as OidcDiscoveryFailed;
    expect(failed.name).toBe('OidcDiscoveryFailed');
    // D3: the name of the library error, nothing else.
    expect(failed.message).toMatch(/^oidc discovery failed: \w+$/);
    expect(failed.message).not.toContain('127.0.0.1');
    expect(failed.cause).toBeUndefined();
    expect(failed.reason).toBe(reason);
  }

  // Review Focus 5: port 1 is on the Fetch "bad port" list, so no connect happens at all.
  it('an issuer on a port that fetch blocks (http://127.0.0.1:1) -> network', async () => {
    expectDiscoveryFailed(await buildUrl(clientFor('http://127.0.0.1:1')).catch((e: unknown) => e), 'network');
  });

  it('a refused connection (a closed port) -> network', async () => {
    expectDiscoveryFailed(await buildUrl(clientFor(await closedPortIssuer())).catch((e: unknown) => e), 'network');
  });

  it.each([
    ['status-404', 'http_client_error'],
    ['status-503', 'http_server_error'],
    ['not-json', 'invalid_metadata'],
    ['wrong-issuer', 'issuer_mismatch'],
    ['issuer-not-a-url', 'invalid_metadata'],
  ] as const)('%s -> %s', async (mode, reason) => {
    failing = await startDiscoveryFailureFixture(mode);
    const err: unknown = await buildUrl(clientFor(failing.issuer)).catch((e: unknown) => e);
    expect(failing.requests).toBeGreaterThan(0);
    expectDiscoveryFailed(err, reason);
  });

  it('hang, with httpTimeoutMs 200 -> timeout', async () => {
    failing = await startDiscoveryFailureFixture('hang');
    const err: unknown = await buildUrl(clientFor(failing.issuer, 200)).catch((e: unknown) => e);
    expect(failing.requests).toBe(1);
    expectDiscoveryFailed(err, 'timeout');
  });

  // D10: the callback calls authorizationCodeGrant, which runs getConfig() first too.
  it('authorizationCodeGrant throws the same class', async () => {
    const err: unknown = await clientFor('http://127.0.0.1:1')
      .authorizationCodeGrant({ currentUrl: callbackUrl(), codeVerifier: 'verifier-value', expectedState: STATE, expectedNonce: NONCE })
      .catch((e: unknown) => e);
    expectDiscoveryFailed(err, 'network');
  });

  // Review Focus 3, D5: a failed discovery is not cached, so a retry runs discovery again.
  it('the next call after a failed discovery runs discovery again', async () => {
    failing = await startDiscoveryFailureFixture('status-503');
    const oidc = clientFor(failing.issuer);
    expectDiscoveryFailed(await buildUrl(oidc).catch((e: unknown) => e), 'http_server_error');
    expectDiscoveryFailed(await buildUrl(oidc).catch((e: unknown) => e), 'http_server_error');
    expect(failing.requests).toBe(2);
  });
});

// SMA-656 T9b. The D8 rows that a fixture cannot produce cheaply, with constructed errors. The
// shapes copy openid-client 6.8.8's errorHandler (build/index.js:117-165): a ClientError carries
// `code`, and for OAUTH_RESPONSE_IS_NOT_CONFORM its `cause` is the Response; for OAUTH_PARSE_ERROR
// its `cause` is oauth4webapi's error, whose own `cause` is what failed while the body was read.
describe('classifyDiscoveryError — constructed errors (SMA-656 T9b)', () => {
  function clientError(code: string | undefined, cause?: unknown): client.ClientError {
    const err = new client.ClientError('a constructed ClientError', cause === undefined ? undefined : { cause });
    if (code !== undefined) err.code = code;
    return err;
  }

  function parseError(nested: unknown): client.ClientError {
    return clientError('OAUTH_PARSE_ERROR', new Error('failed to parse "response" body as JSON', { cause: nested }));
  }

  function fetchFailed(causeCode?: string): TypeError {
    const cause = causeCode === undefined ? new Error('connect failed') : Object.assign(new Error('connect failed'), { code: causeCode });
    return new TypeError('fetch failed', { cause });
  }

  const status = (value: number): Response => new Response(null, { status: value });

  const ROWS: ReadonlyArray<readonly [string, unknown, OidcDiscoveryFailureReason]> = [
    ['row 1: OAUTH_TIMEOUT', clientError('OAUTH_TIMEOUT', new DOMException('timed out', 'TimeoutError')), 'timeout'],
    ['row 1: OAUTH_ABORT (defensive)', clientError('OAUTH_ABORT', new DOMException('aborted', 'AbortError')), 'timeout'],
    ['row 2: NOT_CONFORM, status 500', clientError('OAUTH_RESPONSE_IS_NOT_CONFORM', status(500)), 'http_server_error'],
    ['row 2: NOT_CONFORM, status 503', clientError('OAUTH_RESPONSE_IS_NOT_CONFORM', status(503)), 'http_server_error'],
    ['row 2: NOT_CONFORM, status 599', clientError('OAUTH_RESPONSE_IS_NOT_CONFORM', status(599)), 'http_server_error'],
    ['row 3: NOT_CONFORM, status 499', clientError('OAUTH_RESPONSE_IS_NOT_CONFORM', status(499)), 'http_client_error'],
    ['row 3: NOT_CONFORM, status 404', clientError('OAUTH_RESPONSE_IS_NOT_CONFORM', status(404)), 'http_client_error'],
    ['row 3: NOT_CONFORM, no cause', clientError('OAUTH_RESPONSE_IS_NOT_CONFORM'), 'http_client_error'],
    ['row 3: NOT_CONFORM, a cause that is not a Response', clientError('OAUTH_RESPONSE_IS_NOT_CONFORM', { status: 503 }), 'http_client_error'],
    ['row 4: PARSE_ERROR, the body timed out', parseError(new DOMException('timed out', 'TimeoutError')), 'timeout'],
    ['row 4: PARSE_ERROR, the body was aborted', parseError(new DOMException('aborted', 'AbortError')), 'timeout'],
    ['row 5: PARSE_ERROR, the connection reset during the body', parseError(new TypeError('terminated')), 'network'],
    ['row 6: PARSE_ERROR, bad JSON', parseError(new SyntaxError('Unexpected token')), 'invalid_metadata'],
    ['row 6: PARSE_ERROR, no nested cause', clientError('OAUTH_PARSE_ERROR'), 'invalid_metadata'],
    ['row 6: RESPONSE_IS_NOT_JSON', clientError('OAUTH_RESPONSE_IS_NOT_JSON', status(200)), 'invalid_metadata'],
    ['row 6: RESPONSE_IS_NOT_JSON, a 503 cause is not read', clientError('OAUTH_RESPONSE_IS_NOT_JSON', status(503)), 'invalid_metadata'],
    ['row 6: INVALID_RESPONSE', clientError('OAUTH_INVALID_RESPONSE'), 'invalid_metadata'],
    ['row 6: INVALID_SERVER_METADATA (defensive)', clientError('OAUTH_INVALID_SERVER_METADATA'), 'invalid_metadata'],
    ['row 6: MISSING_SERVER_METADATA (defensive)', clientError('OAUTH_MISSING_SERVER_METADATA'), 'invalid_metadata'],
    ['row 7: JSON_ATTRIBUTE_COMPARISON_FAILED', clientError('OAUTH_JSON_ATTRIBUTE_COMPARISON_FAILED', { expected: 'https://idp.invalid/', attribute: 'issuer' }), 'issuer_mismatch'],
    ['row 8: TypeError with own code ERR_INVALID_URL', Object.assign(new TypeError('Invalid URL'), { code: 'ERR_INVALID_URL' }), 'invalid_metadata'],
    ['row 9: fetch failed, ENOTFOUND', fetchFailed('ENOTFOUND'), 'dns'],
    ['row 11: fetch failed, ECONNREFUSED', fetchFailed('ECONNREFUSED'), 'network'],
    ['row 11: fetch failed, ECONNRESET', fetchFailed('ECONNRESET'), 'network'],
    ['row 11: fetch failed, EAI_AGAIN', fetchFailed('EAI_AGAIN'), 'network'],
    ['row 11: fetch failed, a cause with no code', fetchFailed(), 'network'],
    ['row 11: fetch failed, a cause code that is a URL', fetchFailed('https://idp.invalid/x'), 'network'],
    ['row 11: a TypeError with no cause', new TypeError('fetch failed'), 'network'],
    ['row 12: an unknown ClientError code', clientError('OAUTH_SOMETHING_NEW'), 'other'],
    ['row 12: a ClientError with no code', clientError(undefined), 'other'],
    ['row 12: HTTP_REQUEST_FORBIDDEN', clientError('OAUTH_HTTP_REQUEST_FORBIDDEN'), 'other'],
    ['row 12: a TypeError with another own code', Object.assign(new TypeError('bad argument'), { code: 'ERR_INVALID_ARG_TYPE' }), 'other'],
    ['row 12: a plain Error', new Error('boom'), 'other'],
    ['row 12: a string', 'https://idp.invalid/x', 'other'],
    ['row 12: undefined', undefined, 'other'],
  ];

  it.each(ROWS)('%s', (_label, err, reason) => {
    expect(classifyDiscoveryError(err)).toBe(reason);
  });

  it.each(['CERT_HAS_EXPIRED', 'DEPTH_ZERO_SELF_SIGNED_CERT', 'SELF_SIGNED_CERT_IN_CHAIN', 'UNABLE_TO_VERIFY_LEAF_SIGNATURE', 'UNABLE_TO_GET_ISSUER_CERT_LOCALLY', 'ERR_TLS_CERT_ALTNAME_INVALID'])(
    'row 10: fetch failed, %s -> tls',
    (code) => {
      expect(classifyDiscoveryError(fetchFailed(code))).toBe('tls');
    },
  );
});
```

- [ ] **Step 4: Run the tests and see them fail**

Run (from `PKG`): `pnpm exec vitest run tests/adapters/oidc.test.ts`
Expected: FAIL.
- Every T9 row fails at `expect(isOidcDiscoveryFailed(err)).toBe(true)`: today the adapter throws a plain `Error('oidc discovery failed: …')`.
- Every T9b row fails with `TypeError: classifyDiscoveryError is not a function` (the export does not exist).
- The existing rows pass, including `wraps a discovery failure without leaking the underlying error object`.

- [ ] **Step 5: Implement the adapter change**

In `PKG/src/adapters/oidc.ts`, replace lines 18–20:

```ts
// NEVER LOG A CAUGHT LIBRARY ERROR OBJECT. `openid-client` errors may embed a URL (the discovery
// document location, a token endpoint). Every method here catches and rethrows through
// `wrapError`, which keeps only the error's `name`.
```

with:

```ts
// NEVER LOG A CAUGHT LIBRARY ERROR OBJECT. `openid-client` errors may embed a URL (the discovery
// document location, a token endpoint). Every method here catches and rethrows through
// `wrapError`, which keeps only the error's `name`. A DISCOVERY failure is different (SMA-656):
// `getConfig` throws `OidcDiscoveryFailed` (core/errors.ts) with the same name-only message, no
// `cause`, and a `reason` from a closed list that `classifyDiscoveryError` below sets.
//
// THE NO-REVOKE INVARIANT (SMA-656 D10). `OidcDiscoveryFailed` comes only from `getConfig()`, and
// every method below awaits `getConfig()` before it sends any other request. So a caller that sees
// one knows that no token request was sent: no authorization code was spent and no token exists.
// http/routes.ts's callback 503 does not revoke because of this. A change that throws it AFTER a
// token request (for example a re-discovery on a JWKS `kid` miss inside the grant) breaks that
// route, which must then revoke.
```

Replace line 35:

```ts
import { RefreshRejected } from '../core/errors';
```

with:

```ts
import { OidcDiscoveryFailed, RefreshRejected, type OidcDiscoveryFailureReason } from '../core/errors';
```

Replace lines 128–136:

```ts
/**
 * Never rethrow a caught openid-client/oauth4webapi error object — several of its error classes
 * (`ResponseBodyError`, `OperationProcessingError`, ...) can carry the request URL. Keep only the
 * error's `name`, which identifies the failure class without any request or response content.
 */
function wrapError(stage: string, cause: unknown): Error {
  const name = cause instanceof Error ? cause.name : 'unknown_error';
  return new Error(`oidc ${stage} failed: ${name}`);
}
```

with:

```ts
/**
 * The library error's `name`, and nothing else. `wrapError` and the discovery throw (SMA-656 D3)
 * both use it, so the two messages cannot drift apart.
 */
function libraryErrorName(err: unknown): string {
  return err instanceof Error ? err.name : 'unknown_error';
}

/**
 * Never rethrow a caught openid-client/oauth4webapi error object — several of its error classes
 * (`ResponseBodyError`, `OperationProcessingError`, ...) can carry the request URL. Keep only the
 * error's `name`, which identifies the failure class without any request or response content.
 */
function wrapError(stage: string, cause: unknown): Error {
  return new Error(`oidc ${stage} failed: ${libraryErrorName(cause)}`);
}
```

After the closing `}` of `classifyRefreshError` (line 172 before this edit), insert:

```ts

/** SMA-656 D8 row 6: the ClientError codes that mean "the document is not usable metadata". The
 * last two are defensive: discovery does not validate endpoints. */
const INVALID_METADATA_CODES: ReadonlySet<string> = new Set(['OAUTH_RESPONSE_IS_NOT_JSON', 'OAUTH_PARSE_ERROR', 'OAUTH_INVALID_RESPONSE', 'OAUTH_INVALID_SERVER_METADATA', 'OAUTH_MISSING_SERVER_METADATA']);

/** SMA-656 D8 row 10: the Node TLS codes that a failed `fetch` carries on its `cause`. */
const TLS_CAUSE_CODES: ReadonlySet<string> = new Set([
  'CERT_HAS_EXPIRED',
  'DEPTH_ZERO_SELF_SIGNED_CERT',
  'SELF_SIGNED_CERT_IN_CHAIN',
  'UNABLE_TO_VERIFY_LEAF_SIGNATURE',
  'UNABLE_TO_GET_ISSUER_CERT_LOCALLY',
  'ERR_TLS_CERT_ALTNAME_INVALID',
]);

/** The own `code` property of an object, or undefined. Reads nothing else. */
function codeOf(value: unknown): unknown {
  return typeof value === 'object' && value !== null ? (value as { code?: unknown }).code : undefined;
}

/** The own `name` property of an object, or undefined. Reads nothing else. */
function nameOf(value: unknown): unknown {
  return typeof value === 'object' && value !== null ? (value as { name?: unknown }).name : undefined;
}

/**
 * Why discovery failed, as one value of a closed list (SMA-656 D8). The rows are tested in the D8
 * order, and the first match wins. Derived from openid-client 6.8.8's `errorHandler` and
 * `performDiscovery` (build/index.js:117-165, 260-299) and measured by tests/adapters/oidc.test.ts
 * (T9); the rows that a fixture cannot produce cheaply are tested with constructed errors (T9b).
 *
 * Reads only the class, `code`, `cause.code`, `cause.status` and the `name` of the nested cause of
 * an OAUTH_PARSE_ERROR. Each string is compared against a fixed literal or a closed set, and
 * `status` is read only as a number range, so the return value is always one of the fixed literals:
 * no library value, message or URL can come out of this function (A2).
 *
 * `instanceof client.ClientError` is SAFE here, for the reason classifyRefreshError records above:
 * this check and the `client.discovery` call in getConfig read the one module-level `client`
 * binding. This is the fourth non-builtin `instanceof` site that core/errors.ts's rule counts, and
 * it is on the `instanceof` side of that line. Its OUTPUT crosses module copies as
 * OidcDiscoveryFailed, which is why routes classify THAT by `code`. `TypeError` and `Response` are
 * builtins, which both copies share.
 */
export function classifyDiscoveryError(err: unknown): OidcDiscoveryFailureReason {
  if (err instanceof client.ClientError) {
    const code = err.code;
    // Row 1. OAUTH_ABORT is defensive: the adapter passes no abort signal.
    if (code === 'OAUTH_TIMEOUT' || code === 'OAUTH_ABORT') return 'timeout';
    // Rows 2 and 3. oauth4webapi puts the Response itself on `cause` for this code.
    if (code === 'OAUTH_RESPONSE_IS_NOT_CONFORM') {
      const response = err.cause;
      return response instanceof Response && response.status >= 500 && response.status <= 599 ? 'http_server_error' : 'http_client_error';
    }
    // Rows 4 and 5: the body failed after the headers arrived. The other PARSE_ERROR cases fall
    // through to row 6.
    if (code === 'OAUTH_PARSE_ERROR') {
      const nested = err.cause instanceof Error ? err.cause.cause : undefined;
      const nestedName = nameOf(nested);
      if (nestedName === 'TimeoutError' || nestedName === 'AbortError') return 'timeout';
      if (nested instanceof TypeError) return 'network';
    }
    // Row 6.
    if (code !== undefined && INVALID_METADATA_CODES.has(code)) return 'invalid_metadata';
    // Row 7.
    if (code === 'OAUTH_JSON_ATTRIBUTE_COMPARISON_FAILED') return 'issuer_mismatch';
    return 'other';
  }
  if (err instanceof TypeError) {
    const ownCode = codeOf(err);
    // Row 8: `new URL(as.issuer)` in performDiscovery runs outside errorHandler.
    if (ownCode === 'ERR_INVALID_URL') return 'invalid_metadata';
    // Rows 9-11: a failed `fetch` is a TypeError with no own code.
    if (ownCode === undefined) {
      const causeCode = codeOf(err.cause);
      if (causeCode === 'ENOTFOUND') return 'dns';
      if (typeof causeCode === 'string' && TLS_CAUSE_CODES.has(causeCode)) return 'tls';
      return 'network';
    }
  }
  // Row 12.
  return 'other';
}
```

In `getConfig`, replace lines 204–208:

```ts
      .catch((err: unknown) => {
        // Let the NEXT call retry discovery instead of replaying this rejection forever.
        configPromise = undefined;
        throw wrapError('discovery', err);
      });
```

with:

```ts
      .catch((err: unknown) => {
        // Let the NEXT call retry discovery instead of replaying this rejection forever.
        configPromise = undefined;
        // SMA-656 D1, D3, D8: the same name-only message that wrapError('discovery', …) made, a
        // closed `reason`, and NO `cause`.
        throw new OidcDiscoveryFailed(`oidc discovery failed: ${libraryErrorName(err)}`, classifyDiscoveryError(err));
      });
```

In `PKG/src/core/errors.ts`, replace the three lines:

```ts
 * — the boundary is the CLOSURE, not whether state is shared. The three remaining sites in this
 * package that test a NON-BUILTIN class each carry a comment saying which side of that line they
 * fall on.
```

with:

```ts
 * — the boundary is the CLOSURE, not whether state is shared. The four remaining sites in this
 * package that test a NON-BUILTIN class each carry a comment saying which side of that line they
 * fall on: server.ts's CallbackRejected catch, adapters/operation-deadline.ts's SessionStoreTimeout
 * check, and adapters/oidc.ts's classifyRefreshError and classifyDiscoveryError (SMA-656). All four
 * are on the `instanceof` side.
```

- [ ] **Step 6: Run the tests and see them pass**

Run (from `PKG`): `pnpm exec vitest run tests/adapters/oidc.test.ts tests/core/errors.test.ts`
Expected: PASS, every row. **If a T9 row fails on `expect(failed.reason).toBe(…)` only, STOP.** Report the mode, the expected and the measured reason. Do not edit the test, the fixture or the D8 order to make it pass (spec D8, T9).

Then run (from `PKG`): `pnpm exec vitest run tests/http tests/next tests/core`
Expected: PASS (no route test depends on the old plain `Error`).

- [ ] **Step 7: Typecheck, lint, format, commit**

Run (from the worktree root): `moon run paigasus-auth-ts:typecheck`. Then `pnpm -C ts exec eslint packages/paigasus-auth/src/adapters/oidc.ts packages/paigasus-auth/src/core/errors.ts packages/paigasus-auth/tests/adapters/oidc.test.ts packages/paigasus-auth/tests/fixtures/discovery-failures.ts`. Then `pnpm -C ts exec prettier --write packages/paigasus-auth/src/adapters/oidc.ts packages/paigasus-auth/src/core/errors.ts packages/paigasus-auth/tests/adapters/oidc.test.ts packages/paigasus-auth/tests/fixtures/discovery-failures.ts`.
Expected: no error. If ESLint rejects a construct, fix it by the rule's own advice. Do not disable a rule.

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-656 branch --show-current
git add ts/packages/paigasus-auth/src/adapters/oidc.ts ts/packages/paigasus-auth/src/core/errors.ts ts/packages/paigasus-auth/tests/adapters/oidc.test.ts ts/packages/paigasus-auth/tests/fixtures/discovery-failures.ts
git commit -m "feat(ts): throw OidcDiscoveryFailed with a closed reason from OIDC discovery (SMA-656)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: The harness and the login route (T1–T7)

**Files:**
- Modify: `PKG/tests/support/store-failure.ts` (the whole file; the full new content is below)
- Modify: `PKG/tests/http/store-unavailable.test.ts` (imports lines 8, 11, 14–28; delete lines 133–143; the four `seedTransaction(h)` calls)
- Create: `PKG/tests/http/discovery-failed.test.ts`
- Modify: `PKG/src/http/routes.ts` (header lines 27–31; imports lines 32, 35, 37, 41; a helper before `handleLogin` at line 123; `handleLogin` lines 176–194)

**Interfaces:**
- Produces, in `tests/support/store-failure.ts`: `FakeOidc.authorizationError?: Error`; `FakeOidc.codeGrantError?: Error`; `STORE_SENTINELS: readonly string[]`; `SENTINEL_IDP_URL: string`; `IDP_SENTINELS: readonly string[]`; `STATE: string`; `TXN_SECRET: string`; `seedTransaction(h: Harness, returnTo: string): Promise<void>`; `callbackRequest(sid?: string): Request`; `expectStoreUnavailable(res: Response, expected: { kind: 'link' | 'post'; target: string }, forbidden?: readonly string[]): Promise<string>`; `expectEventsClean(events: ReadonlyArray<[AuthEventName, AuthEventFields]>, forbidden?: readonly string[]): void`.
- Produces, in `src/http/routes.ts`: the private `discoveryFailedResponse(runtime: AuthRuntime, stage: OidcDiscoveryStage, err: unknown, href: string): Response`.
- Consumes: `isOidcDiscoveryFailed`, `oidcDiscoveryReason` (Task 2); `OidcDiscoveryStage`, `RetryLink` (Task 3).

- [ ] **Step 1: Change the harness**

Replace the whole content of `PKG/tests/support/store-failure.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-653 route-test harness: a store that fails on chosen methods, a network-free OIDC fake, and
// the assertions that every 503 row shares.
//
// SMA-656 uses it for the OIDC discovery rows too (tests/http/discovery-failed.test.ts). The fake
// can reject `buildAuthorizationUrl` and `authorizationCodeGrant` with a chosen error. The
// transaction seed and the callback request live here, so both route test files use one copy. The
// two redaction assertions take the forbidden strings as a parameter.
//
// THE SENTINELS. Every thrown store error carries SENTINEL_DSN in its message, the same way the
// real adapter puts the (redacted) DSN there. A discovery test error carries SENTINEL_IDP_URL, a URL
// that the OIDC library could put in its own error text. expectStoreUnavailable and
// expectEventsClean then assert that no forbidden string reaches the body, a header, or a logged
// field. The default is STORE_SENTINELS, so the SMA-653 rows did not change. A test that forgets to
// call them is weaker; every row in store-unavailable.test.ts and discovery-failed.test.ts that
// produces a response calls both.
import { expect } from 'vitest';
import { claimsPrincipalResolver } from '../../src/adapters/claims-resolver.js';
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import type { AuthorizationRequest, BuildEndSessionUrlParams, OidcClient, OidcTokens, RefreshedTokens } from '../../src/adapters/oidc.js';
import { SessionStoreTimeout, SessionStoreUnavailable } from '../../src/core/errors.js';
import { hashSecret } from '../../src/core/ids.js';
import { SESSION_COOKIE, txnCookieName } from '../../src/http/cookies.js';
import { STORE_UNAVAILABLE_CSP } from '../../src/http/store-unavailable.js';
import type { AuthEventFields, AuthEventName } from '../../src/ports/logger.js';
import type { SessionStore } from '../../src/ports/session-store.js';
import type { AuthRuntime } from '../../src/runtime.js';

export const SENTINEL_DSN = 'redis://user:sentinel-pw@redis.invalid:6379';
/** The parts of SENTINEL_DSN that no response and no event may hold (the SMA-653 rows). */
export const STORE_SENTINELS: readonly string[] = ['sentinel-pw', 'redis.invalid'];
/** SMA-656: a URL the OIDC library could put in its error text. Each discovery test error's message holds it. */
export const SENTINEL_IDP_URL = 'https://idp.invalid/.well-known/openid-configuration?sentinel-656';
/** The parts of SENTINEL_IDP_URL that no response and no event may hold (the SMA-656 rows). */
export const IDP_SENTINELS: readonly string[] = ['idp.invalid', 'sentinel-656'];
export const ORIGIN = 'https://rp.example.com';
export const BASE_PATH = '/iam';
export const END_SESSION_URL = 'https://issuer.example.com/logout';
export const NEW_REFRESH_TOKEN = 'new-refresh-token';
/** The harness runtime's OIDC client id. */
export const CLIENT_ID = 'paigasus-console';
/** The callback rows' transaction id (the `state`) and its browser-bound secret. */
export const STATE = 'state-0123456789';
export const TXN_SECRET = 'correct-secret-value-32-bytes-ok';
const b64 = (value: unknown): string => Buffer.from(JSON.stringify(value)).toString('base64url');
/**
 * The raw ID token that `fakeOidc().authorizationCodeGrant` returns. JWT-shaped, not signed. Its
 * `aud` is CLIENT_ID, so logout would send it as the hint (http/routes.ts `hintAudienceMatches`).
 */
export const FAKE_ID_TOKEN = `${b64({ alg: 'RS256', typ: 'JWT' })}.${b64({ iss: 'https://issuer.example.com', sub: 'a-subject', aud: CLIENT_ID })}.fake-signature`;

export type FailureKind = 'unavailable' | 'timeout';
export const FAILURE_KINDS: readonly FailureKind[] = ['unavailable', 'timeout'];

export type StoreMethod = 'get' | 'set' | 'delete' | 'putTransaction' | 'takeTransaction';

export function storeError(kind: FailureKind): Error {
  return kind === 'unavailable' ? new SessionStoreUnavailable(`session store unavailable (${SENTINEL_DSN})`) : new SessionStoreTimeout(SENTINEL_DSN, 4000, 'deadline');
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
  /** The parameters of every `buildEndSessionUrl` call, in order (SMA-681: does it carry a hint?). */
  endSessionCalls: BuildEndSessionUrlParams[];
  /** SMA-656: when set, `buildAuthorizationUrl` rejects with this error. */
  authorizationError?: Error;
  /** SMA-656: when set, `authorizationCodeGrant` rejects with this error. */
  codeGrantError?: Error;
}

/** No network. Unless `codeGrantError` is set, the code exchange succeeds and returns NEW_REFRESH_TOKEN. */
export function fakeOidc(): FakeOidc {
  const oidc: FakeOidc = {
    revokeCalls: [],
    failRevoke: false,
    endSessionCalls: [],
    buildAuthorizationUrl: (): Promise<AuthorizationRequest> =>
      oidc.authorizationError !== undefined
        ? Promise.reject(oidc.authorizationError)
        : Promise.resolve({ url: 'https://issuer.example.com/authorize?client_id=test', codeVerifier: 'a-verifier', nonce: 'a-nonce' }),
    authorizationCodeGrant: (): Promise<OidcTokens> =>
      oidc.codeGrantError !== undefined
        ? Promise.reject(oidc.codeGrantError)
        : Promise.resolve({
            accessToken: 'new-access-token',
            refreshToken: NEW_REFRESH_TOKEN,
            expiresIn: 300,
            idToken: FAKE_ID_TOKEN,
            idTokenClaims: { iss: 'https://issuer.example.com', sub: 'a-subject' },
          }),
    refresh: (): Promise<RefreshedTokens> => Promise.reject(new Error('refresh is not used by the auth routes')),
    revoke: (token: string): Promise<void> => {
      oidc.revokeCalls.push(token);
      return oidc.failRevoke ? Promise.reject(new Error(`revoke failed at ${SENTINEL_DSN}`)) : Promise.resolve();
    },
    buildEndSessionUrl: (params: BuildEndSessionUrlParams): Promise<string> => {
      oidc.endSessionCalls.push(params);
      return Promise.resolve(END_SESSION_URL);
    },
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
    clientId: CLIENT_ID,
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

/** Stores the callback rows' transaction under STATE, directly in the real store (no recorded call). */
export async function seedTransaction(h: Harness, returnTo: string): Promise<void> {
  await h.inner.putTransaction(STATE, { codeVerifier: 'a-verifier', nonce: 'a-nonce', returnTo, secretHash: hashSecret(TXN_SECRET), createdAt: Date.now() }, 600_000);
}

/** A callback for STATE with its transaction cookie, and a session cookie when `sid` is given. */
export function callbackRequest(sid?: string): Request {
  const cookies = [`${txnCookieName(STATE)}=${TXN_SECRET}`, ...(sid !== undefined ? [`${SESSION_COOKIE}=${sid}`] : [])];
  return new Request(`${ORIGIN}${BASE_PATH}/auth/callback?code=a-code&state=${STATE}`, { headers: { cookie: cookies.join('; ') } });
}

const HTML_ENTITIES: Readonly<Record<string, string>> = { '&amp;': '&', '&lt;': '<', '&gt;': '>', '&quot;': '"', '&#39;': "'" };

export function htmlDecode(value: string): string {
  return value.replace(/&(?:amp|lt|gt|quot|#39);/g, (entity) => HTML_ENTITIES[entity] ?? entity);
}

/** Asserts every § 3 property of a 503, that the retry target is exactly `target`, and that no `forbidden` string is in the body or a header. */
export async function expectStoreUnavailable(res: Response, expected: { kind: 'link' | 'post'; target: string }, forbidden: readonly string[] = STORE_SENTINELS): Promise<string> {
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
    for (const sentinel of forbidden) expect(text).not.toContain(sentinel);
  }
  return body;
}

/** No logged field of any event holds a `forbidden` string. */
export function expectEventsClean(events: ReadonlyArray<[AuthEventName, AuthEventFields]>, forbidden: readonly string[] = STORE_SENTINELS): void {
  const text = JSON.stringify(events);
  for (const sentinel of forbidden) expect(text).not.toContain(sentinel);
}

/** The fields of each `store.unavailable` event, in order. */
export function storeUnavailableEvents(events: ReadonlyArray<[AuthEventName, AuthEventFields]>): AuthEventFields[] {
  return events.filter(([name]) => name === 'store.unavailable').map(([, fields]) => fields);
}
```

In `PKG/tests/http/store-unavailable.test.ts`:
- Delete line 8: `import { hashSecret } from '../../src/core/ids.js';`
- Replace line 11 `import { SESSION_COOKIE, txnCookieName } from '../../src/http/cookies.js';` with `import { SESSION_COOKIE } from '../../src/http/cookies.js';`
- Replace the import block at lines 14–28 with:

```ts
import {
  BASE_PATH,
  END_SESSION_URL,
  FAILURE_KINDS,
  FAKE_ID_TOKEN,
  NEW_REFRESH_TOKEN,
  ORIGIN,
  SENTINEL_DSN,
  STATE,
  callbackRequest,
  expectEventsClean,
  expectStoreUnavailable,
  harness,
  seedTransaction,
  storeError,
  storeUnavailableEvents,
} from '../support/store-failure.js';
```

- Delete lines 133–143 (the constants `STATE` and `TXN_SECRET`, and the functions `seedTransaction` and `callbackRequest`, with the blank lines between them). Keep one blank line between the `describe('GET /auth/login — escaping and classification', …)` block and the `describe.each(FAILURE_KINDS)('GET /auth/callback with the store down (%s)', …)` block.
- Replace every `await seedTransaction(h);` with `await seedTransaction(h, RETURN_TO);` (four sites: the rows 3, 4, 5, and the failing-revoke row).

- [ ] **Step 2: Run the SMA-653 rows and see that the harness change did not change them**

Run (from `PKG`): `pnpm exec vitest run tests/http/store-unavailable.test.ts tests/http/route-handler.test.ts tests/core/single-flight.test.ts`
Expected: PASS, with the same test count as before this step.

- [ ] **Step 3: Write the failing login tests**

Create `PKG/tests/http/discovery-failed.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-656: /auth/login and /auth/callback answer an OIDC discovery failure with a 503 that names
// the identity provider (spec D1-D10; T1-T7, T12-T15). The header of store-unavailable.test.ts
// limits that file to store failures, so the discovery rows live here. The fake OIDC client rejects
// with a constructed error; tests/adapters/oidc.test.ts measures the real adapter's errors (T9).
//
// Every error a row throws carries SENTINEL_IDP_URL in its message, and every row that produces a
// response asserts that no IDP_SENTINELS string reaches the body, a header or a logged field (T4).
import { describe, expect, it, vi } from 'vitest';
import { OIDC_DISCOVERY_FAILURE_REASONS, OidcDiscoveryFailed, type OidcDiscoveryFailureReason } from '../../src/core/errors.js';
import type { SessionRecord } from '../../src/core/session.js';
import { SESSION_COOKIE } from '../../src/http/cookies.js';
import { createAuthRoutes } from '../../src/http/routes.js';
import { BASE_PATH, IDP_SENTINELS, ORIGIN, SENTINEL_IDP_URL, expectEventsClean, expectStoreUnavailable, harness } from '../support/store-failure.js';

const RETURN_TO = '/iam/orgs';
const HOSTILE = `/iam/a"b<c>d&e'f#g h`;
const OLD_SID = 'old-session-id-0123456789';
const LOGIN_TARGET = `/iam/auth/login?returnTo=${encodeURIComponent(RETURN_TO)}`;
const IDP_SENTENCE = 'The identity provider is not available. Try again in a few seconds.';
const SESSION_SENTENCE = 'The session service did not answer.';

/** No store call fails in these rows. The wrapper still records each call in `h.calls`. */
const noStoreFailure = (): Error => new Error('no store call fails in the SMA-656 rows');

function discoveryError(reason: OidcDiscoveryFailureReason): OidcDiscoveryFailed {
  return new OidcDiscoveryFailed(`oidc discovery failed at ${SENTINEL_IDP_URL}`, reason);
}

function sessionRecord(): SessionRecord {
  return {
    version: 2,
    rev: 0,
    accessToken: 'old-access-token',
    refreshToken: 'old-refresh-token',
    accessExpiresAt: Date.now() + 60_000,
    absoluteExpiresAt: Date.now() + 60_000,
    idToken: 'old-id-token',
    idTokenClaims: { iss: 'https://issuer.example.com', sub: 'a-subject' },
    principal: { principalPrn: null, issuer: 'https://issuer.example.com', subject: 'a-subject', memberships: [], roleGrants: [], grantsAvailable: false },
  };
}

function loginRequest(returnTo: string, sid?: string): Request {
  const init = sid !== undefined ? { headers: { cookie: `${SESSION_COOKIE}=${sid}` } } : undefined;
  return new Request(`${ORIGIN}${BASE_PATH}/auth/login?returnTo=${encodeURIComponent(returnTo)}`, init);
}

/** The IdP 503: every D5 header, no Set-Cookie, the target, the D4 sentence and NOT the session sentence. */
async function expectIdpUnavailable(res: Response, target: string): Promise<string> {
  const body = await expectStoreUnavailable(res, { kind: 'link', target }, IDP_SENTINELS);
  expect(body).toContain('<h1>Sign-in is temporarily unavailable</h1>');
  expect(body).toContain(IDP_SENTENCE);
  expect(body).not.toContain(SESSION_SENTENCE);
  return body;
}

describe('GET /auth/login — OIDC discovery failed (SMA-656)', () => {
  it.each(OIDC_DISCOVERY_FAILURE_REASONS)('T1, T3, T4, no session cookie (%s): 503, link to login with returnTo', async (reason) => {
    const h = harness([], noStoreFailure);
    h.oidc.authorizationError = discoveryError(reason);

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO));

    await expectIdpUnavailable(res, LOGIN_TARGET);
    // D9: no store call, so no transaction and no delete. Exactly one event, and no login.started.
    expect(h.calls).toEqual([]);
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'login', reason }]]);
    expectEventsClean(h.events, IDP_SENTINELS);
  });

  it.each(OIDC_DISCOVERY_FAILURE_REASONS)('T2, T3, T4, with a session cookie (%s): the link goes to returnTo, the session survives', async (reason) => {
    const h = harness([], noStoreFailure);
    await h.inner.set(OLD_SID, sessionRecord(), 60_000, null);
    h.oidc.authorizationError = discoveryError(reason);

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO, OLD_SID));

    await expectIdpUnavailable(res, RETURN_TO);
    expect(h.calls).toEqual([]);
    expect(await h.inner.get(OLD_SID)).not.toBeNull();
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'login', reason }]]);
    expectEventsClean(h.events, IDP_SENTINELS);
  });

  it('T5: a plain Error (the build_authorization_url case) propagates, and nothing is logged', async () => {
    const h = harness([], noStoreFailure);
    const boom = new Error('oidc build_authorization_url failed: TypeError');
    h.oidc.authorizationError = boom;

    await expect(createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO))).rejects.toBe(boom);

    expect(h.events).toEqual([]);
    expect(h.calls).toEqual([]);
  });

  it('T6a: an OidcDiscoveryFailed from a SECOND copy of core/errors gives the 503 (D1)', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    // Precondition: without this, the test passes vacuously if the import returns the same module.
    expect(foreign.OidcDiscoveryFailed).not.toBe(OidcDiscoveryFailed);
    const h = harness([], noStoreFailure);
    h.oidc.authorizationError = new foreign.OidcDiscoveryFailed(`oidc discovery failed at ${SENTINEL_IDP_URL}`, 'dns');

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO));

    await expectIdpUnavailable(res, LOGIN_TARGET);
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'login', reason: 'dns' }]]);
    expectEventsClean(h.events, IDP_SENTINELS);
  });

  it('T6b: a plain Error with the code and no reason gives the 503 with reason other', async () => {
    const h = harness([], noStoreFailure);
    h.oidc.authorizationError = Object.assign(new Error(`oidc discovery failed at ${SENTINEL_IDP_URL}`), { code: 'oidc_discovery_failed' });

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO));

    await expectIdpUnavailable(res, LOGIN_TARGET);
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'login', reason: 'other' }]]);
    expectEventsClean(h.events, IDP_SENTINELS);
  });

  it.each([
    ['a URL', 'https://idp.invalid/x'],
    ['a near miss', 'TIMEOUT'],
    ['an empty string', ''],
    ['a number', 42],
  ] as const)('T7: a reason outside the list (%s) is logged as other', async (_label, reason) => {
    const h = harness([], noStoreFailure);
    h.oidc.authorizationError = Object.assign(new Error(`oidc discovery failed at ${SENTINEL_IDP_URL}`), { code: 'oidc_discovery_failed', reason });

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(RETURN_TO));

    await expectIdpUnavailable(res, LOGIN_TARGET);
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'login', reason: 'other' }]]);
    expectEventsClean(h.events, IDP_SENTINELS);
  });

  // Review Focus 1.
  it('round-trips a hostile returnTo through the IdP login link', async () => {
    const h = harness([], noStoreFailure);
    h.oidc.authorizationError = discoveryError('network');

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(HOSTILE));

    const body = await expectIdpUnavailable(res, `/iam/auth/login?returnTo=${encodeURIComponent(HOSTILE)}`);
    // encodeURIComponent leaves ' as is, so the HTML layer must escape it.
    expect(body).toContain('&#39;');
  });

  it('keeps a hostile returnTo inert in the D6 link', async () => {
    const h = harness([], noStoreFailure);
    await h.inner.set(OLD_SID, sessionRecord(), 60_000, null);
    h.oidc.authorizationError = discoveryError('network');

    const res = await createAuthRoutes(h.runtime).handle(loginRequest(HOSTILE, OLD_SID));

    const body = await expectIdpUnavailable(res, HOSTILE);
    expect(body).toContain('href="/iam/a&quot;b&lt;c&gt;d&amp;e&#39;f#g h"');
  });
});
```

- [ ] **Step 4: Run the tests and see them fail**

Run (from `PKG`): `pnpm exec vitest run tests/http/discovery-failed.test.ts`
Expected: FAIL. Every T1, T2, T6a, T6b, T7 and hostile row fails because `handle()` rejects with the discovery error (today `handleLogin` has no catch). T5 PASSES already: a plain error propagates today. T5 is a guard; mutation M2 in Task 9 proves it bites.

- [ ] **Step 5: Implement the login route**

In `PKG/src/http/routes.ts`, replace lines 30–31:

```ts
// caller choice, and only this file knows WHICH store call failed — the logout route needs that to
// still attempt its delete when its read fails. Every other error still propagates.
```

with:

```ts
// caller choice, and only this file knows WHICH store call failed — the logout route needs that to
// still attempt its delete when its read fails.
//
// AN OIDC DISCOVERY FAILURE IS THE SECOND EXCEPTION (SMA-656). An `OidcDiscoveryFailed` from
// `buildAuthorizationUrl` (login) or `authorizationCodeGrant` (callback) becomes a 503 with a retry
// link that names the identity provider, and one `oidc.discovery_failed` event
// (`discoveryFailedResponse`). It is classified by its `code` (`isOidcDiscoveryFailed`), because the
// shared `runtime.oidc` builds it in whichever module copy created the runtime (SMA-657 D7). Every
// other error still propagates.
```

Replace line 32:

```ts
import type { OidcTokens } from '../adapters/oidc';
```

with:

```ts
import type { AuthorizationRequest, OidcTokens } from '../adapters/oidc';
```

Replace line 35:

```ts
import { CallbackRejected } from '../core/errors';
```

with:

```ts
import { CallbackRejected, isOidcDiscoveryFailed, oidcDiscoveryReason } from '../core/errors';
```

Replace line 37:

```ts
import { sidTag } from '../ports/logger';
```

with:

```ts
import { sidTag, type OidcDiscoveryStage } from '../ports/logger';
```

Replace line 41:

```ts
import { STORE_DOWN, loginRetryHref, storeStep, storeUnavailableResponse, type RetryAffordance } from './store-unavailable';
```

with:

```ts
import { STORE_DOWN, loginRetryHref, storeStep, storeUnavailableResponse, type RetryLink } from './store-unavailable';
```

Replace the line (line 123):

```ts
async function handleLogin(runtime: AuthRuntime, req: Request, url: URL): Promise<Response> {
```

with:

```ts
/**
 * The 503 for an OIDC discovery failure (SMA-656 D4, D7), shared by the login and the callback
 * route. Logs one `oidc.discovery_failed` event with the zone, the typed stage and the closed
 * `reason` — never the caught error, its message, its name or a URL (ports/logger.ts's redaction
 * contract; A2). `oidcDiscoveryReason` maps any value outside the closed list to 'other'.
 */
function discoveryFailedResponse(runtime: AuthRuntime, stage: OidcDiscoveryStage, err: unknown, href: string): Response {
  runtime.logger.event('oidc.discovery_failed', { zone: runtime.zone, stage, reason: oidcDiscoveryReason(err) });
  return storeUnavailableResponse({ kind: 'link', href, service: 'identity_provider' });
}

async function handleLogin(runtime: AuthRuntime, req: Request, url: URL): Promise<Response> {
```

In `handleLogin`, replace this block (lines 176–194 before this task):

```ts
  const txnId = newTransactionId();
  const secret = newTransactionSecret();

  const authorization = await runtime.oidc.buildAuthorizationUrl({
    redirectUri: runtime.redirectUri,
    scopes: runtime.scopes,
    // The CSRF `state` IS the transaction id, so the callback can find its cookie and its stored
    // transaction from the same value the IdP hands back.
    state: txnId,
  });

  // SMA-653 D9: read the presented sid BEFORE the first store call, because it decides where the
  // 503's retry link points. When this browser holds a session, the link goes to `returnTo`, NOT to
  // /auth/login: during a wedge `requireSession` sends a signed-in user here, the store call below
  // fails first, and the session record and cookie both survive. A retry link to /auth/login would
  // delete that still-valid session once Redis recovers (SMA-651 § 5). `returnTo` has already
  // passed validateReturnTo and the auth-route guard above, so it cannot loop back into this route.
  const presentedSid = readCookies(req.headers.get('cookie')).get(SESSION_COOKIE);
  const retry: RetryAffordance = { kind: 'link', href: presentedSid !== undefined ? returnTo : loginRetryHref(runtime.basePath, returnTo) };
```

with:

```ts
  const txnId = newTransactionId();
  const secret = newTransactionSecret();

  // SMA-653 D9: read the presented sid BEFORE the IdP call and the first store call, because it
  // decides where the 503's retry link points. When this browser holds a session, the link goes to
  // `returnTo`, NOT to /auth/login: during a wedge `requireSession` sends a signed-in user here, the
  // store call below fails first, and the session record and cookie both survive. A retry link to
  // /auth/login would delete that still-valid session once Redis recovers (SMA-651 § 5). The same
  // holds while the IdP cannot be discovered (SMA-656 D6). `returnTo` has already passed
  // validateReturnTo and the auth-route guard above, so it cannot loop back into this route.
  // `readCookies` is pure and cannot throw, so this read before the IdP call changes nothing else.
  const presentedSid = readCookies(req.headers.get('cookie')).get(SESSION_COOKIE);
  const retry: RetryLink = { kind: 'link', href: presentedSid !== undefined ? returnTo : loginRetryHref(runtime.basePath, returnTo) };

  // SMA-656 D1, D2: a discovery failure is the one IdP error that this route answers itself, with a
  // 503. It happens before putTransaction and before the session delete, so no store call runs and
  // nothing changes (D9). Any other error — for example `oidc build_authorization_url failed` when
  // the discovered metadata has no authorization_endpoint — still propagates.
  let authorization: AuthorizationRequest;
  try {
    authorization = await runtime.oidc.buildAuthorizationUrl({
      redirectUri: runtime.redirectUri,
      scopes: runtime.scopes,
      // The CSRF `state` IS the transaction id, so the callback can find its cookie and its stored
      // transaction from the same value the IdP hands back.
      state: txnId,
    });
  } catch (err) {
    if (!isOidcDiscoveryFailed(err)) throw err;
    return discoveryFailedResponse(runtime, 'login', err, retry.href);
  }
```

Everything after this block stays the same.

- [ ] **Step 6: Run the tests and see them pass**

Run (from `PKG`): `pnpm exec vitest run tests/http`
Expected: PASS, every file under `tests/http`, including `login.test.ts`, `login-returnto-table.test.ts` and `store-unavailable.test.ts`.

- [ ] **Step 7: Typecheck, lint, format, commit**

Run (from the worktree root): `moon run paigasus-auth-ts:typecheck`. Then `pnpm -C ts exec eslint packages/paigasus-auth/src/http/routes.ts packages/paigasus-auth/tests/support/store-failure.ts packages/paigasus-auth/tests/http/store-unavailable.test.ts packages/paigasus-auth/tests/http/discovery-failed.test.ts`. Then `pnpm -C ts exec prettier --write packages/paigasus-auth/src/http/routes.ts packages/paigasus-auth/tests/support/store-failure.ts packages/paigasus-auth/tests/http/store-unavailable.test.ts packages/paigasus-auth/tests/http/discovery-failed.test.ts`.
Expected: no error.

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-656 branch --show-current
git add ts/packages/paigasus-auth/src/http/routes.ts ts/packages/paigasus-auth/tests/support/store-failure.ts ts/packages/paigasus-auth/tests/http/store-unavailable.test.ts ts/packages/paigasus-auth/tests/http/discovery-failed.test.ts
git commit -m "fix(ts): answer an OIDC discovery failure on /auth/login with a 503 (SMA-656)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: The callback route (T12–T15)

**Files:**
- Modify: `PKG/tests/http/discovery-failed.test.ts` (two import lines; append a `describe`)
- Modify: `PKG/src/http/routes.ts` (`handleCallback`: the exchange block, lines 278–291 before Task 5; the session-fixation read, line 314 before Task 5. Find both by their text.)

**Interfaces:**
- Consumes: `discoveryFailedResponse` (Task 5); `seedTransaction`, `callbackRequest`, `STATE` (Task 5 harness); `CallbackRejected`.
- Produces: no new symbol. `handleCallback` returns the IdP 503 for an `OidcDiscoveryFailed` from `authorizationCodeGrant`.

- [ ] **Step 1: Write the failing callback tests**

In `PKG/tests/http/discovery-failed.test.ts`, replace the line:

```ts
import { OIDC_DISCOVERY_FAILURE_REASONS, OidcDiscoveryFailed, type OidcDiscoveryFailureReason } from '../../src/core/errors.js';
```

with:

```ts
import { CallbackRejected, OIDC_DISCOVERY_FAILURE_REASONS, OidcDiscoveryFailed, type OidcDiscoveryFailureReason } from '../../src/core/errors.js';
```

Replace the line:

```ts
import { BASE_PATH, IDP_SENTINELS, ORIGIN, SENTINEL_IDP_URL, expectEventsClean, expectStoreUnavailable, harness } from '../support/store-failure.js';
```

with:

```ts
import { BASE_PATH, IDP_SENTINELS, ORIGIN, SENTINEL_IDP_URL, STATE, callbackRequest, expectEventsClean, expectStoreUnavailable, harness, seedTransaction } from '../support/store-failure.js';
```

Append at the end of the file:

```ts

// Each row seeds a transaction and sends its cookie, so the callback reaches the code exchange.
describe('GET /auth/callback — OIDC discovery failed (SMA-656 D10)', () => {
  it.each(OIDC_DISCOVERY_FAILURE_REASONS)('T12, T13, no session cookie (%s): 503, link to login with the transaction returnTo', async (reason) => {
    const h = harness([], noStoreFailure);
    await seedTransaction(h, RETURN_TO);
    h.oidc.codeGrantError = discoveryError(reason);

    const res = await createAuthRoutes(h.runtime).handle(callbackRequest());

    await expectIdpUnavailable(res, LOGIN_TARGET);
    // Exactly one event: no login.callback_rejected, no session.created.
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'callback', reason }]]);
    // No store call after takeTransaction.
    expect(h.calls).toEqual([`takeTransaction:${STATE}`]);
    expectEventsClean(h.events, IDP_SENTINELS);
    // D10: the one state change on this path. The transaction is consumed, so a retry of this URL cannot complete.
    expect(await h.inner.takeTransaction(STATE)).toBeNull();
  });

  it('T12b, T13, with a session cookie: the link goes to the transaction returnTo, and the session survives', async () => {
    const h = harness([], noStoreFailure);
    await seedTransaction(h, RETURN_TO);
    await h.inner.set(OLD_SID, sessionRecord(), 60_000, null);
    h.oidc.codeGrantError = discoveryError('timeout');

    const res = await createAuthRoutes(h.runtime).handle(callbackRequest(OLD_SID));

    await expectIdpUnavailable(res, RETURN_TO);
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'callback', reason: 'timeout' }]]);
    expect(h.calls).toEqual([`takeTransaction:${STATE}`]);
    expect(await h.inner.get(OLD_SID)).not.toBeNull();
    expectEventsClean(h.events, IDP_SENTINELS);
  });

  it('T14: a plain Error from the exchange is still code_exchange_failed', async () => {
    const h = harness([], noStoreFailure);
    await seedTransaction(h, RETURN_TO);
    h.oidc.codeGrantError = new Error(`oidc authorization_code_grant failed at ${SENTINEL_IDP_URL}`);

    const err: unknown = await createAuthRoutes(h.runtime)
      .handle(callbackRequest())
      .catch((e: unknown) => e);

    expect(err).toBeInstanceOf(CallbackRejected);
    expect((err as CallbackRejected).reason).toBe('code_exchange_failed');
    expect(h.events).toEqual([['login.callback_rejected', { reason: 'code_exchange_failed', zone: 'iam' }]]);
  });

  it('T15: an OidcDiscoveryFailed from a SECOND copy of core/errors gives the T12 503', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    expect(foreign.OidcDiscoveryFailed).not.toBe(OidcDiscoveryFailed);
    const h = harness([], noStoreFailure);
    await seedTransaction(h, RETURN_TO);
    h.oidc.codeGrantError = new foreign.OidcDiscoveryFailed(`oidc discovery failed at ${SENTINEL_IDP_URL}`, 'tls');

    const res = await createAuthRoutes(h.runtime).handle(callbackRequest());

    await expectIdpUnavailable(res, LOGIN_TARGET);
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'callback', reason: 'tls' }]]);
  });

  // Review Focus 1.
  it('round-trips a hostile transaction returnTo through the IdP callback link', async () => {
    const h = harness([], noStoreFailure);
    await seedTransaction(h, HOSTILE);
    h.oidc.codeGrantError = discoveryError('network');

    const res = await createAuthRoutes(h.runtime).handle(callbackRequest());

    const body = await expectIdpUnavailable(res, `/iam/auth/login?returnTo=${encodeURIComponent(HOSTILE)}`);
    expect(body).toContain('&#39;');
  });

  // Review Focus 2, D10: a reload of the 503 page sends this callback URL again.
  it('a reload of the callback 503 gives state_unknown, and no second discovery event', async () => {
    const h = harness([], noStoreFailure);
    await seedTransaction(h, RETURN_TO);
    h.oidc.codeGrantError = discoveryError('network');
    const routes = createAuthRoutes(h.runtime);

    const first = await routes.handle(callbackRequest());
    expect(first.status).toBe(503);
    const err: unknown = await routes.handle(callbackRequest()).catch((e: unknown) => e);

    expect(err).toBeInstanceOf(CallbackRejected);
    expect((err as CallbackRejected).reason).toBe('state_unknown');
    expect(h.events).toEqual([
      ['oidc.discovery_failed', { zone: 'iam', stage: 'callback', reason: 'network' }],
      ['login.callback_rejected', { reason: 'state_unknown', zone: 'iam' }],
    ]);
  });
});
```

- [ ] **Step 2: Run the tests and see them fail**

Run (from `PKG`): `pnpm exec vitest run tests/http/discovery-failed.test.ts`
Expected: FAIL. Every T12 row, T12b, T15, the hostile callback row and the reload row fail: `handle()` rejects with `CallbackRejected('code_exchange_failed')` (today's bare `catch {}`), so no `Response` is returned. T14 PASSES already: it is a guard, and mutation M4 in Task 9 proves it bites. The Task 5 login rows still pass.

- [ ] **Step 3: Implement the callback route**

In `PKG/src/http/routes.ts`, in `handleCallback`, replace:

```ts
  const currentUrl = new URL(runtime.redirectUri);
  currentUrl.search = url.search;

  let tokens: OidcTokens;
  try {
    tokens = await runtime.oidc.authorizationCodeGrant({
      currentUrl,
      codeVerifier: tx.codeVerifier,
      expectedState: state,
      expectedNonce: tx.nonce,
    });
  } catch {
    return reject('code_exchange_failed');
  }
```

with:

```ts
  const currentUrl = new URL(runtime.redirectUri);
  currentUrl.search = url.search;

  // Read ONCE, before the exchange (SMA-656 § 4.6). The discovery 503 below picks its retry target
  // from it, and the session-fixation delete after the exchange reuses it.
  const presentedSid = cookies.get(SESSION_COOKIE);

  let tokens: OidcTokens;
  try {
    tokens = await runtime.oidc.authorizationCodeGrant({
      currentUrl,
      codeVerifier: tx.codeVerifier,
      expectedState: state,
      expectedNonce: tx.nonce,
    });
  } catch (err) {
    // SMA-656 D10. A discovery failure is NOT a failed code exchange, so it does not reject:
    //   - takeTransaction above has consumed the transaction, so a retry of this URL gives
    //     `state_unknown`. The retry link starts a new login instead (the SMA-653 row 3 rule).
    //   - The code is NOT spent, and no tokens exist: OidcDiscoveryFailed comes only from the
    //     adapter's getConfig(), which runs before any token request (the no-revoke invariant in
    //     adapters/oidc.ts's header). So nothing is revoked, unlike failAfterExchange below.
    //   - The retry target follows SMA-656 D6 (the SMA-653 D9 rule): with a session cookie, the
    //     link goes to tx.returnTo, so a valid session that a second tab created is not deleted by
    //     a retry through /auth/login. tx.returnTo passed validateReturnTo and the auth-route guard
    //     at login.
    //   - No `login.callback_rejected`: the callback was not rejected. `code_exchange_failed` stays
    //     for a real token-endpoint failure, so the two are different in the log.
    // Classified by `code`, not `instanceof`: the shared runtime.oidc builds the error in whichever
    // module copy created the runtime (SMA-657 D7).
    if (isOidcDiscoveryFailed(err)) {
      const href = presentedSid !== undefined ? tx.returnTo : loginRetryHref(runtime.basePath, tx.returnTo);
      return discoveryFailedResponse(runtime, 'callback', err, href);
    }
    return reject('code_exchange_failed');
  }
```

Then, in the session-fixation block of the same function, replace:

```ts
  // first tab's clearing response.
  const presentedSid = cookies.get(SESSION_COOKIE);
  if (presentedSid !== undefined) {
```

with:

```ts
  // first tab's clearing response. `presentedSid` was read before the exchange (SMA-656 § 4.6).
  if (presentedSid !== undefined) {
```

- [ ] **Step 4: Run the tests and see them pass**

Run (from `PKG`): `pnpm exec vitest run tests/http`
Expected: PASS, every file, including `callback.test.ts` (its two `code_exchange_failed` rows use real exchange errors, which are not discovery errors) and `store-unavailable.test.ts` rows 3–5.

- [ ] **Step 5: Typecheck, lint, format, commit**

Run (from the worktree root): `moon run paigasus-auth-ts:typecheck`. Then `pnpm -C ts exec eslint packages/paigasus-auth/src/http/routes.ts packages/paigasus-auth/tests/http/discovery-failed.test.ts`. Then `pnpm -C ts exec prettier --write packages/paigasus-auth/src/http/routes.ts packages/paigasus-auth/tests/http/discovery-failed.test.ts`.
Expected: no error.

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-656 branch --show-current
git add ts/packages/paigasus-auth/src/http/routes.ts ts/packages/paigasus-auth/tests/http/discovery-failed.test.ts
git commit -m "fix(ts): answer an OIDC discovery failure on /auth/callback with a 503 (SMA-656)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: Through the Next boundary (T11, T16) and the `server.ts` comments

These rows use the REAL adapter with the issuer `http://127.0.0.1:1`. Tasks 5 and 6 built the behavior, so the rows pass at once. Step 3 proves that they bite, with two temporary edits.

**Files:**
- Modify: `PKG/tests/http/route-handler.test.ts` (imports lines 7–15; two `it` rows before the closing `});` of the `describe` at line 114)
- Modify: `PKG/src/server.ts` (doc comment lines 85–86 and 93–96)

**Interfaces:**
- Consumes: `createAuthRouteHandler(runtime: AuthRuntime): AuthRoutes['handle']`; `createOidcClient(opts: CreateOidcClientOptions): OidcClient`; `expectStoreUnavailable(…, forbidden)` (Task 5); `hashSecret(secret: string): string`; `txnCookieName(txnId: string): string`.
- Produces: no new symbol.

- [ ] **Step 1: Write the tests**

In `PKG/tests/http/route-handler.test.ts`, replace lines 10–11:

```ts
import { createOidcClient } from '../../src/adapters/oidc.js';
import { SESSION_COOKIE, TXN_COOKIE_PREFIX } from '../../src/http/cookies.js';
```

with:

```ts
import { createOidcClient, type OidcClient } from '../../src/adapters/oidc.js';
import { hashSecret } from '../../src/core/ids.js';
import { SESSION_COOKIE, TXN_COOKIE_PREFIX, txnCookieName } from '../../src/http/cookies.js';
```

After line 18 (`const PUBLIC_ORIGIN = 'https://console.example.com';`), add:

```ts
const IDP_SENTENCE = 'The identity provider is not available. Try again in a few seconds.';

/**
 * The REAL adapter, on an issuer where discovery fails at once. Port 1 is on the Fetch "bad port"
 * list, so undici refuses it with no connect and no wait (SMA-656 plan, Review Focus 5).
 */
function unreachableOidc(): OidcClient {
  return createOidcClient({ issuer: 'http://127.0.0.1:1', clientId: 'paigasus-console', clientSecret: 'a-client-secret', httpTimeoutMs: 2000, clockToleranceSeconds: 30, allowInsecureRequests: true });
}
```

Before the closing `});` of `describe('createAuthRouteHandler under basePath /iam (SMA-511 spec § 7.1)', …)`, after the SMA-653 row, add:

```ts

  it('T11: a discovery failure on /auth/login is the IdP 503, not a 500 (SMA-656)', async () => {
    runtime = { ...runtime, oidc: unreachableOidc() };

    const res = await createAuthRouteHandler(runtime)(new Request(`${BIND}/auth/login?returnTo=%2Fiam%2Forgs`));

    const body = await expectStoreUnavailable(res, { kind: 'link', target: '/iam/auth/login?returnTo=%2Fiam%2Forgs' }, ['127.0.0.1']);
    expect(body).toContain(IDP_SENTENCE);
  });

  it('T16: a discovery failure on /auth/callback is the IdP 503, not the login-failed 502 (SMA-656)', async () => {
    runtime = { ...runtime, oidc: unreachableOidc() };
    // Seeded directly: a login first cannot seed it, because on this issuer the login gives a 503.
    const txnId = 'txn-sma-656-0123456789';
    const secret = 'callback-secret-value-32-bytes-x';
    await runtime.store.putTransaction(txnId, { codeVerifier: 'a-verifier', nonce: 'a-nonce', returnTo: '/iam/orgs', secretHash: hashSecret(secret), createdAt: Date.now() }, 600_000);

    const res = await createAuthRouteHandler(runtime)(new Request(`${BIND}/auth/callback?code=x&state=${txnId}`, { headers: { cookie: `${txnCookieName(txnId)}=${secret}` } }));

    const body = await expectStoreUnavailable(res, { kind: 'link', target: '/iam/auth/login?returnTo=%2Fiam%2Forgs' }, ['127.0.0.1']);
    expect(body).toContain(IDP_SENTENCE);
    expect(body).not.toContain('login failed');
  });
```

- [ ] **Step 2: Run the tests**

Run (from `PKG`): `pnpm exec vitest run tests/http/route-handler.test.ts`
Expected: PASS (Tasks 5 and 6 built the behavior).

- [ ] **Step 3: Prove that the two rows can fail, then restore by an edit**

3a. In `PKG/src/http/routes.ts`, in `handleLogin`'s catch, change `return discoveryFailedResponse(runtime, 'login', err, retry.href);` to `throw err;`. Run (from `PKG`): `pnpm exec vitest run tests/http/route-handler.test.ts`. Expected: T11 FAILS (the handler rethrows `OidcDiscoveryFailed`). Restore the line with the Edit tool to `return discoveryFailedResponse(runtime, 'login', err, retry.href);`.

3b. In `handleCallback`'s catch, change `return discoveryFailedResponse(runtime, 'callback', err, href);` to `return reject('code_exchange_failed');`. Run the same command. Expected: T16 FAILS with status 502. Restore the line with the Edit tool to `return discoveryFailedResponse(runtime, 'callback', err, href);`.

3c. Run the same command again. Expected: PASS. Run `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-656 diff --stat -- ts/packages/paigasus-auth/src`. Expected: no output (the source equals the Task 6 commit).

- [ ] **Step 4: Update the `server.ts` comments**

In `PKG/src/server.ts`, replace lines 85–86:

```ts
 * A session-store failure is NOT mapped here. `createAuthRoutes` already answers it with a 503
 * `Response` (SMA-653, http/store-unavailable.ts), which this handler returns unchanged.
```

with:

```ts
 * A session-store failure is NOT mapped here. `createAuthRoutes` already answers it with a 503
 * `Response` (SMA-653, http/store-unavailable.ts), which this handler returns unchanged. An OIDC
 * discovery failure on `/auth/login` or `/auth/callback` is the same: `createAuthRoutes` answers it
 * with the identity-provider 503 (SMA-656), which this handler returns unchanged.
```

Replace lines 93–96:

```ts
 * rather than straight back into another login attempt. `code_exchange_failed` is the one reason
 * that is a genuine failure (an unreachable or erroring token endpoint) rather than an expected
 * outcome, so it surfaces as a 502 rather than a silent redirect — an operator must be able to
 * tell it apart from ordinary traffic.
```

with:

```ts
 * rather than straight back into another login attempt. `code_exchange_failed` is the one reason
 * that is a genuine failure (an unreachable or erroring token endpoint) rather than an expected
 * outcome, so it surfaces as a 502 rather than a silent redirect — an operator must be able to
 * tell it apart from ordinary traffic. An OIDC discovery failure no longer reaches this 502:
 * http/routes.ts answers it with a 503 and throws no `CallbackRejected` (SMA-656 D10).
```

- [ ] **Step 5: Typecheck, lint, format, commit**

Run (from the worktree root): `moon run paigasus-auth-ts:typecheck`. Then `pnpm -C ts exec eslint packages/paigasus-auth/src/server.ts packages/paigasus-auth/tests/http/route-handler.test.ts`. Then `pnpm -C ts exec prettier --write packages/paigasus-auth/src/server.ts packages/paigasus-auth/tests/http/route-handler.test.ts`.
Expected: no error.

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-656 branch --show-current
git add ts/packages/paigasus-auth/tests/http/route-handler.test.ts ts/packages/paigasus-auth/src/server.ts
git commit -m "test(ts): prove the discovery 503 through createAuthRouteHandler (SMA-656)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: Documentation (spec § 7: README and the SMA-506 design)

The code comments of spec § 7 are already done: the `routes.ts` header and the callback catch (Tasks 5, 6), `server.ts` (Task 7), the `oidc.ts` header and the `errors.ts` site count (Task 4), the `store-unavailable.ts` header (Task 3), the harness header (Task 5). This task does the README and the SMA-506 design.

**Files:**
- Modify: `PKG/README.md` (lines 128–133 and 141–142; a new subsection before `## Cookies`, line 180)
- Modify: `docs/superpowers/specs/2026-09-09-sma-506-auth-design.md` (after line 426; line 1041; after line 1044)

**Interfaces:** none.

- [ ] **Step 1: Correct the store 503 text in the README**

In `PKG/README.md`, replace lines 128–133:

~~~markdown
- `/auth/login`: a link. If the browser holds a session cookie, the link goes to `returnTo`, so a
  session that survived the stall is not deleted by the retry. Otherwise it goes to `/auth/login`.
  If `returnTo` is a public page, the link opens that page directly, and the user must choose
  sign-in again.
- `/auth/callback`: a link to `/auth/login`. If the code exchange already succeeded, the route first
  revokes the new refresh token, best effort.
~~~

with:

~~~markdown
- `/auth/login`: a link. If the browser holds a session cookie, the link goes to `returnTo`, so a
  session that survived the stall is not deleted by the retry. Otherwise it goes to
  `/auth/login?returnTo=…`. If `returnTo` is a public page, the link opens that page directly, and
  the user must choose sign-in again.
- `/auth/callback`: a link to `/auth/login`. If the code exchange already succeeded, the link
  carries `?returnTo=…`, and the route first revokes the new refresh token, best effort.
~~~

Replace lines 141–142:

~~~markdown
ingress custom error page or a mesh retry policy for 503 in front of the auth routes: the first
removes the retry control, and the second replays logins and logouts.
~~~

with:

~~~markdown
ingress custom error page or a mesh retry policy for 503 in front of the auth routes: the first
removes the retry control, and the second replays logins and logouts. This applies to the
discovery 503 too (see "When the identity provider cannot be discovered" below).
~~~

- [ ] **Step 2: Add the discovery subsection to the README**

In `PKG/README.md`, directly before the line `## Cookies`, add:

~~~markdown
### When the identity provider cannot be discovered (SMA-656)

The OIDC client gets the IdP's discovery document on the first login, callback, refresh or logout
of a process, and keeps it for the life of the process. A failed discovery is not kept: the next
call tries again, and it can wait up to `PAIGASUS_OIDC_HTTP_TIMEOUT_MS`.

**`/auth/login` and `/auth/callback` answer a discovery failure with a 503.** The headers are the
same as the store 503 above: `Retry-After: 5`, `Cache-Control: no-store`,
`Referrer-Policy: no-referrer`, the strict CSP, and no `Set-Cookie`. The page says "The identity
provider is not available. Try again in a few seconds." The retry link:

- `/auth/login`: with a session cookie, the link goes to `returnTo`. Without one, it goes to
  `/auth/login?returnTo=…`. The route makes no store call, so the session record and its cookie
  stay as they were.
- `/auth/callback`: the route has already consumed the login transaction, so the link starts a new
  login. With a session cookie, the link goes to the transaction's `returnTo`, so a session that a
  second tab created is not deleted. Without one, it goes to `/auth/login?returnTo=…`. The
  authorization code is not spent and no tokens exist, so nothing is revoked. Before SMA-656 this
  case gave the `login failed` 502.

A reload of the callback 503 page sends the callback URL again. The transaction is gone, so that
request logs `login.callback_rejected` with `reason: 'state_unknown'` and redirects to
`/auth/login`. This is not a replay attack.

**The `oidc.discovery_failed` event.** Each such 503 logs one event, `{ zone, stage, reason }`.
`stage` is `login` or `callback`. The event never holds the caught error, its message, its name or
a URL. It means "this process has no discovered configuration, and a login or a callback needed
one". Its absence does NOT mean that the IdP is healthy. `reason` is one of:

| `reason`            | What failed                                                                   | Probably                                               |
| ------------------- | ----------------------------------------------------------------------------- | ------------------------------------------------------ |
| `timeout`           | no answer within `PAIGASUS_OIDC_HTTP_TIMEOUT_MS`, or the body timed out       | an outage                                              |
| `network`           | the connection failed (refused, reset, `EAI_AGAIN`, …)                        | an outage, but an egress block gives the same value    |
| `dns`               | the host name does not exist (`ENOTFOUND`)                                    | a defect                                               |
| `tls`               | the certificate was refused                                                   | a defect (this package has no CA-bundle setting)       |
| `http_server_error` | the document answered with a 5xx status                                       | an outage, but a misconfigured ingress gives the same value |
| `http_client_error` | the document answered with another status, for example 404 for a wrong realm  | a defect                                               |
| `invalid_metadata`  | the body is not JSON, is not valid metadata, or its `issuer` is not a URL     | a defect                                               |
| `issuer_mismatch`   | the document's `issuer` is not `PAIGASUS_OIDC_ISSUER` (a trailing slash is enough) | a defect                                          |
| `other`             | anything else                                                                 | unknown                                                |

So the 503 is NOT a promise that the fault is temporary. A configuration defect gives the same page,
and its retry link cannot work until the configuration is correct. Read `reason` first.

**Known limits.**

- An IdP outage that starts AFTER the first successful discovery does not give this 503.
  `/auth/login` then redirects to an IdP that does not answer.
- One pod that cannot discover, behind a round-robin balancer, fails the callbacks of logins that
  other pods started. Each retry then costs a full sign-in at the IdP. SMA-705 tracks a readiness
  gate or an eager discovery at start.
~~~

- [ ] **Step 3: Update the SMA-506 design**

In `docs/superpowers/specs/2026-09-09-sma-506-auth-design.md`, after line 426 (`| Redis command | \`PAIGASUS_SESSION_REDIS_TIMEOUT_MS\` | § 7.2 |`), add:

~~~markdown

The discovery row and the token-exchange row are refined by
`2026-09-26-sma-656-auth-login-oidc-503-design.md` (SMA-656): a discovery failure gives a 503 with
a retry link on `/auth/login` AND on `/auth/callback`, and the token-exchange row no longer covers
a discovery failure.
~~~

Replace line 1041:

~~~markdown
`session.resolve_failed`, `logout.completed`, `store.unavailable`.
~~~

with:

~~~markdown
`session.resolve_failed`, `logout.completed`, `store.unavailable`, `oidc.discovery_failed`.
~~~

After line 1044 (the end of the `session.resolve_failed` bullet), add:

~~~markdown
- `oidc.discovery_failed` — `{ zone, stage, reason }`: a login or a callback needed the OIDC
  discovery document, and this process could not get it (SMA-656). `stage` is `login` or
  `callback`; `reason` is a closed list. Its absence does not mean that the IdP is healthy.
~~~

- [ ] **Step 4: Format and commit**

Run: `pnpm -C ts exec prettier --write packages/paigasus-auth/README.md`, then `moon run ts:fmt` (from the worktree root).
Expected: PASS. Prettier aligns the new table; that is expected. The SMA-506 design is outside `ts/`, so no Prettier gate reads it.

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-656 branch --show-current
git add ts/packages/paigasus-auth/README.md docs/superpowers/specs/2026-09-09-sma-506-auth-design.md
git commit -m "docs(ts): document the OIDC discovery 503 (SMA-656)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: The mutation battery (spec § 5.7) and full verification

**Files:** none change at the end of this task. Every mutation is restored by an edit.

For each mutation: make the edit with the Edit tool, run the battery command, record which tests failed, then restore the exact original text with the Edit tool. Do NOT use `git checkout --`, `git stash` or `git reset`. A mutation that leaves every listed test green is a FINDING: stop and report it. Do not weaken a test to make it fail. Record each result for the PR description.

Battery command (from `PKG`): `pnpm exec vitest run tests/http tests/core/errors.test.ts tests/adapters/oidc.test.ts`

- [ ] **Step 1: Run the clean package suite once**

Run (from the worktree root): `moon run paigasus-auth-ts:test --force`
Expected: PASS.

- [ ] **Step 2: M1 — delete the catch in `handleLogin`**

Edit `PKG/src/http/routes.ts`: replace

```ts
  try {
    authorization = await runtime.oidc.buildAuthorizationUrl({
      redirectUri: runtime.redirectUri,
      scopes: runtime.scopes,
      // The CSRF `state` IS the transaction id, so the callback can find its cookie and its stored
      // transaction from the same value the IdP hands back.
      state: txnId,
    });
  } catch (err) {
    if (!isOidcDiscoveryFailed(err)) throw err;
    return discoveryFailedResponse(runtime, 'login', err, retry.href);
  }
```

with

```ts
  authorization = await runtime.oidc.buildAuthorizationUrl({
    redirectUri: runtime.redirectUri,
    scopes: runtime.scopes,
    // The CSRF `state` IS the transaction id, so the callback can find its cookie and its stored
    // transaction from the same value the IdP hands back.
    state: txnId,
  });
```

Run the battery. Must fail: T1–T4 (every `T1, T3, T4` and `T2, T3, T4` row), T6a, T6b, T7, both hostile login rows, T11. T5 passes. Restore the original block by an edit.

- [ ] **Step 3: M2 — `handleLogin` catches every error**

Edit: delete the line `    if (!isOidcDiscoveryFailed(err)) throw err;` in `handleLogin`. Run the battery. Must fail: T5 (the promise resolves to a 503). Restore the line by an edit.

- [ ] **Step 4: M3 — delete the discovery branch in `handleCallback`**

Edit: delete these four lines in `handleCallback`'s catch:

```ts
    if (isOidcDiscoveryFailed(err)) {
      const href = presentedSid !== undefined ? tx.returnTo : loginRetryHref(runtime.basePath, tx.returnTo);
      return discoveryFailedResponse(runtime, 'callback', err, href);
    }
```

Run the battery. Must fail: every T12 row, T12b, T15, the hostile callback row, the reload row, T16. Restore the four lines by an edit.

- [ ] **Step 5: M4 — the callback branch catches every error**

Edit: change `    if (isOidcDiscoveryFailed(err)) {` in `handleCallback` to `    if (isOidcDiscoveryFailed(err) || err instanceof Error) {`. Run the battery. Must fail: T14, and `callback.test.ts`'s two `code_exchange_failed` rows. Restore the line by an edit.

- [ ] **Step 6: M5 — the callback branch also logs `login.callback_rejected`**

Edit: directly before `      return discoveryFailedResponse(runtime, 'callback', err, href);`, insert the line `      runtime.logger.event('login.callback_rejected', { reason: 'code_exchange_failed', zone: runtime.zone });`. Run the battery. Must fail: T13 (every T12 row, T12b, T15), the reload row. Restore by deleting the inserted line.

- [ ] **Step 7: M6 — the callback link ignores the session cookie**

Edit: change `      const href = presentedSid !== undefined ? tx.returnTo : loginRetryHref(runtime.basePath, tx.returnTo);` to `      const href = loginRetryHref(runtime.basePath, tx.returnTo);`. Run the battery. Must fail: T12b. Restore the line by an edit.

- [ ] **Step 8: M7 — `isOidcDiscoveryFailed` always returns false**

Edit `PKG/src/core/errors.ts`: change `  return hasAuthErrorCode(err, 'oidc_discovery_failed');` to `  return false;`. Run the battery. Must fail: T1, T2, T6, T7, T9 (every row), T10 (the `true` rows), T11, T12, T15, T16. Restore the line by an edit.

- [ ] **Step 9: M8 — `oidcDiscoveryReason` without the list check**

Edit: replace the body line `  return OIDC_DISCOVERY_FAILURE_REASONS.find((known) => known === reason) ?? 'other';` with `  return (reason as OidcDiscoveryFailureReason | undefined) ?? 'other';`. Run the battery. Must fail: T7 (all four rows: the URL, `'TIMEOUT'`, `''` and `42` are logged as they are, and the URL row also fails `expectEventsClean`), T10 (`returns other for a URL`, `a near miss`, `a number`). Restore the line by an edit.

- [ ] **Step 10: M9 — the helper puts `String(err)` into the event**

Edit `PKG/src/http/routes.ts` `discoveryFailedResponse`: change `{ zone: runtime.zone, stage, reason: oidcDiscoveryReason(err) }` to `{ zone: runtime.zone, stage, reason: oidcDiscoveryReason(err), error: String(err) }`. Run the battery. Must fail: T4 (`expectEventsClean` in every T1 and T2 row), T13 (every T12 row and T12b; the `toEqual` and `expectEventsClean`). Restore by an edit.

- [ ] **Step 11: M10 — the default `service` becomes `'identity_provider'`**

Edit `PKG/src/http/store-unavailable.ts`: change `  const service = retry.service ?? 'session_store';` to `  const service = retry.service ?? 'identity_provider';`. Run the battery. Must fail: T0 (`the link page is byte-identical`). Restore the line by an edit.

- [ ] **Step 12: M11 — the adapter goes back to `wrapError('discovery', …)`**

Edit `PKG/src/adapters/oidc.ts` `getConfig`: replace the line

```ts
        throw new OidcDiscoveryFailed(`oidc discovery failed: ${libraryErrorName(err)}`, classifyDiscoveryError(err));
```

with

```ts
        throw wrapError('discovery', err);
```

Run the battery. Must fail: T9 (every row, including the retry row), T11, T16. Restore the original line by an edit.

- [ ] **Step 13: M12 — the adapter sets `cause`**

Edit: replace the same original line with

```ts
        throw Object.assign(new OidcDiscoveryFailed(`oidc discovery failed: ${libraryErrorName(err)}`, classifyDiscoveryError(err)), { cause: err });
```

Run the battery. Must fail: T9 (every row, at `expect(failed.cause).toBeUndefined()`). Restore the original line by an edit.

- [ ] **Step 14: M13 — `classifyDiscoveryError` always returns `'other'`**

Edit: insert `  return 'other';` as the first line of the body of `classifyDiscoveryError`. Run the battery. Must fail: T9 (every row), T9b (every row except the row-12 rows). Restore by deleting the inserted line.

- [ ] **Step 15: M14 — `classifyDiscoveryError` ignores the status range**

Edit: change `      return response instanceof Response && response.status >= 500 && response.status <= 599 ? 'http_server_error' : 'http_client_error';` to `      return 'http_client_error';` (keep the `const response = err.cause;` line). Run the battery. Must fail: T9 `status-503 -> http_server_error`, the T9 retry row, T9b rows `status 500`, `status 503`, `status 599`. Restore the line by an edit.

- [ ] **Step 16: M15 — a failed discovery is cached (Review Focus 3)**

Edit `PKG/src/adapters/oidc.ts` `getConfig`: delete the line `        configPromise = undefined;`. Run the battery. Must fail: T9 `the next call after a failed discovery runs discovery again` (`requests` is 1). Restore the line by an edit.

- [ ] **Step 17: Confirm that every mutation is restored**

Run: `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-656 status --short`
Expected: no output.

- [ ] **Step 18: Full verification**

Run (from the worktree root): `moon run paigasus-auth-ts:test paigasus-auth-ts:typecheck ts:lint ts:fmt paigasus-console-core-ts:typecheck iam-console-ts:typecheck gateway-console-ts:typecheck --force`
Expected: PASS. `paigasus-console-core` imports `AuthEventName`, which now has one more member; its typecheck proves that the wider union is accepted. If a task fails, capture `.moon/cache/ciReport.json` and `.moon/cache/states/<project>/<task>/` outside the repo BEFORE any re-run (root `CLAUDE.md`, "Diagnosing an unattributed `moon ci` failure", step 0), then report the failure. Before a push, run the full gate graph that the root `CLAUDE.md` names.

- [ ] **Step 19: Report**

Report the mutation table (M1–M15) with the tests that failed for each row, and the output summary of Step 18. There is nothing to commit in this task.

---

## Self-Review

**Spec coverage.**

| Spec item | Where |
|---|---|
| A1 (login 503 with a retry link) | Task 5 (T1, T2), Task 7 (T11) |
| A2 (no error object or URL in a response or a log) | Task 4 (`classifyDiscoveryError` returns fixed literals; T9 message and `cause` checks), Task 5 and 6 (T4, T13 with `IDP_SENTINELS`), Task 9 M9, M12 |
| A3 (a test proves the mapping) | Tasks 4–7 |
| A4 (callback 503, not the 502) | Task 6 (T12–T15), Task 7 (T16) |
| D1 (classify by code) | Task 2 (`isOidcDiscoveryFailed`, T10), Task 5 (T6a), Task 6 (T15) |
| D2 (every discovery failure is a 503; later failures unchanged) | Task 5 (T5), Task 6 (T14), Task 4 (every T9 mode) |
| D3 (same message, no `cause`) | Task 2 (class test), Task 4 (`libraryErrorName`, T9, M12) |
| D4 (the `service` field; the sentence; the post variant has no field) | Task 3 (T8, T0 unchanged), Task 9 M10 |
| D5 (headers unchanged; retry runs discovery again) | Task 3 (T8 headers), Task 5 (`expectStoreUnavailable`), Task 4 (retry row), Task 9 M15 |
| D6 (login retry target read before the IdP call) | Task 5 (the moved lines; T1, T2; hostile rows) |
| D7 (the event and its fields; typed `stage`) | Task 3 (`OidcDiscoveryStage`), Task 5 (`discoveryFailedResponse`; T3), Task 6 (T13) |
| D8 (the closed reason; the mapping) | Task 2 (the list, `oidcDiscoveryReason`, T10), Task 4 (`classifyDiscoveryError`, T9, T9b, the STOP rule), Task 5 (T7) |
| D9 (no store call on login) | Task 5 (`h.calls` is empty; T2 session survives) |
| D10 (callback 503; no revoke; D6 target; no rejection event; reload) | Task 6 (T12–T15, reload row), Task 2 and Task 4 (the invariant in the class doc and the `oidc.ts` header) |
| § 4.6 (read `presentedSid` before the exchange, reuse it) | Task 6 Step 3 |
| T0 | Task 1 |
| T1–T7 | Task 5 |
| T8 | Task 3 |
| T9, T9b | Task 4 |
| T10 | Task 2 |
| T11, T16 | Task 7 |
| T12, T12b, T13, T14, T15 | Task 6 |
| § 5.7 mutation table | Task 9 (M1–M14 are the 14 spec rows; M15 is added) |
| § 7: `routes.ts` header | Task 5 Step 5 |
| § 7: `routes.ts` callback catch comment | Task 6 Step 3 |
| § 7: `server.ts` two comments | Task 7 Step 4 |
| § 7: `oidc.ts` header, with the D10 invariant | Task 4 Step 5 |
| § 7: `errors.ts` site count (three → four) | Task 4 Step 5 |
| § 7: `store-unavailable.ts` header | Task 3 Step 4 |
| § 7: harness header | Task 5 Step 1 |
| § 7: README (both routes, the reasons with "probably", the event, the reload, the § 6 limits, the `?returnTo=` correction, the ingress and mesh warning) | Task 8 Steps 1–2 |
| § 7: SMA-506 § 7.1 pointer and § 12 event | Task 8 Step 3 |
| § 6 known limits | Task 8 Step 2 (README) |
| § 8 out of scope | Not implemented, by design: refresh and logout paths, readiness gate, lock budget, renames, heading and logout text, a Playwright e2e test |

**Placeholder scan.** No step says "TBD", "similar to Task N" or "add error handling". Every code step shows the full code. The one conditional instruction is the D8 STOP rule in Task 4 Step 6, which the spec requires.

**Type and name consistency.**
- `OIDC_DISCOVERY_FAILURE_REASONS`, `OidcDiscoveryFailureReason`, `OidcDiscoveryFailed`, `isOidcDiscoveryFailed`, `oidcDiscoveryReason`: defined in Task 2, used with the same names in Tasks 4, 5, 6 and 9.
- `OidcDiscoveryStage`, `RetryLink`: defined in Task 3, used in Task 5.
- `classifyDiscoveryError`, `libraryErrorName`: defined in Task 4, used in Task 4 tests and Task 9 M11–M14.
- `discoveryFailedResponse(runtime, stage, err, href)`: defined in Task 5, used in Task 6 and Task 9.
- Harness: `authorizationError`, `codeGrantError`, `STORE_SENTINELS`, `SENTINEL_IDP_URL`, `IDP_SENTINELS`, `STATE`, `TXN_SECRET`, `seedTransaction(h, returnTo)`, `callbackRequest(sid?)`: defined in Task 5 Step 1, used in Tasks 5, 6, 7.
- Fixture: `startDiscoveryFailureFixture`, `closedPortIssuer`, `DiscoveryFailureFixture`, `DiscoveryFailureMode`: defined and used in Task 4.
- The sentence `The identity provider is not available. Try again in a few seconds.` is identical in Task 3 (source and golden), Task 5 (`IDP_SENTENCE`), Task 7 (`IDP_SENTENCE`) and Task 8 (README).

**Deviations from the spec text, on purpose.**
- `errors.ts` placement: after `isRefreshRejected`, not directly after `SessionStoreTimeout` (the private `hasAuthErrorCode` and `isRefreshRejected` sit between).
- The link type is exported as `RetryLink`, so `routes.ts` can type `retry` as the link variant (§ 4.5 step 1).
- Added tests beyond the spec: a closed-port T9 row, the retry row (D5), more T9b boundary rows, the hostile-`returnTo` rows, and the callback reload row (Review Focus 1–5). Added mutation M15.
