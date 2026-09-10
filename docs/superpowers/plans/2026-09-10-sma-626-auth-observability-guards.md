<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-626 `@paigasus/auth` observability and guards — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make an identity-provider failure distinguishable from a store failure end to end, stop a
transient IdP outage from signing users out, bind `authRoutePaths` to the real route table, make the
corrupt-record policy total, and put a red-on-deletion test behind four load-bearing guards.

**Architecture:** The OIDC adapter maps its library's error onto one core error type
(`RefreshRejected`), so `core/single-flight.ts` can classify without importing `adapters/`. The four
HTTP routes move from four hand-written `if` arms to one `Record` keyed by a shared suffix table, so
the type system closes the drift in both directions. One `isSessionRecord` predicate replaces both
adapters' `version !== 1` check and becomes the single statement of the absent-and-deleted policy.

**Tech Stack:** TypeScript 6, vitest 5, `openid-client` 6.8.8 (over `oauth4webapi` 3.8.8),
`redis` 6.2.1 (node-redis v6), Next 16 (`next/headers`, `next/server`).

**Spec:** `docs/superpowers/specs/2026-09-10-sma-626-auth-observability-guards-design.md`

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- **Redaction is absolute.** No event, error message, or log line may carry an access token, a
  refresh token, an authorization code, an id_token, the client secret, or the Redis DSN. `sid` is
  always logged through `sidTag()`, which truncates to 8 characters. Never pass a caught library
  error object into `fields`.
- `src/core/**` may NOT import from `src/adapters/**`.
- `src/middleware.ts` is its own package entry point. Its transitive import graph must reach no
  store, no resolver and no `openid-client` (AC 4). `src/client.ts` must reach nothing under
  `adapters/`, `core/`, `ports/`, `http/` or `next/` (AC 5).
- Relative imports always carry the `.js` suffix (`./runtime.js`, never `./runtime`).
- Run every command from `ts/packages/paigasus-auth` unless a step says otherwise. Prefix Moon
  commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Branch: `feature/sma-626-auth-observability-guards`. Conventional commits scoped `(ts)`.
- **Baseline:** `pnpm exec vitest run` reports `Test Files 22 passed (22)` /
  `Tests 234 passed (234)`.
- **Three existing assertions are deliberate re-baselines**, not regressions:
  `tests/core/single-flight.test.ts:219`, `tests/next/get-session.test.ts:165`, and
  `tests/core/single-flight.test.ts:163`. They are updated in Task 6. Until then they stay green.
- **Ordering constraint from the spec § 2.4:** Task 4 (the shape predicate, applied) MUST land
  before Task 7 (narrowing `get-session`'s catch). A stored literal `null` otherwise mints a
  `SessionStoreUnavailable` against a healthy Redis, and the narrowing's central claim is false.

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `tests/fixtures/jwks.ts` | gains `setNextTokenError` so a test can force a token-endpoint OAuth error | 1 |
| `tests/adapters/oidc.test.ts` | the MEASUREMENT of which error class arrives, then the classifier's behaviour | 1, 2 |
| `src/core/errors.ts` | adds `RefreshRejected` — the core vocabulary the adapter maps onto | 2 |
| `src/adapters/oidc.ts` | classifies a refresh failure; nothing else changes | 2 |
| `src/core/session.ts` | adds `isSessionRecord`, the one statement of the record policy | 3 |
| `src/adapters/redis-store.ts` | applies the predicate; drops its standalone version check | 4 |
| `src/adapters/memory-store.ts` | applies the predicate; drops its standalone version check | 4 |
| `src/core/single-flight.ts` | the CAS-retry guard, then the degrade/delete/classify behaviour | 5, 6 |
| `src/ports/logger.ts` | adds `session.resolve_failed` to the closed event vocabulary | 7 |
| `src/ports/session-store.ts` | writes down the `SessionStoreUnavailable` requirement | 7 |
| `src/next/get-session.ts` | narrows `store.unavailable` to genuine store failures | 7 |
| `src/http/route-table.ts` | NEW. The four route suffixes. Imports nothing. | 8 |
| `src/http/routes.ts` | table-driven dispatch through `Record<AuthRouteSuffix, …>` | 8 |
| `src/middleware.ts` | `authRoutePaths` derives from the shared table | 8 |
| `tests/adapters/redis-store.test.ts` | guards 1 and 2, via `vi.mock('redis')` | 9 |
| `tests/runtime.test.ts` | guard 3, `getAuthRuntime`'s failure reset | 10 |
| `docs/superpowers/specs/2026-09-09-sma-506-auth-design.md` | § 12's minimum event set | 11 |

---

### Task 1: Measure which error class a refused refresh produces

The spec (§ 2.3) refuses to assume this. `oauth4webapi@3.8.8`'s `checkOAuthBodyError` calls
`checkAuthenticationChallenges(response)` BEFORE it parses the body, so a token endpoint answering
with a `WWW-Authenticate` header throws `WWWAuthenticateChallengeError`, which carries no `.error`
field at all. If `invalid_grant` arrived that way, a classifier keyed on `ResponseBodyError` would
silently never fire and every rejection would read as transient.

This task adds the fixture capability and pins the measured answer as a permanent test, in the
style of this repo's other library measurements.

**Files:**
- Modify: `tests/fixtures/jwks.ts`
- Test: `tests/adapters/oidc.test.ts`

**Interfaces:**
- Consumes: `startOidcFixture()` and `OidcFixture` from `tests/fixtures/jwks.ts`.
- Produces: `OidcFixture.setNextTokenError(error: string | undefined, wwwAuthenticate?: string): void`
  — a ONE-SHOT override, cleared on use, matching `setNextExpiresIn`'s existing contract.

- [ ] **Step 1: Add the one-shot override to the fixture's interface**

In `tests/fixtures/jwks.ts`, add to the `OidcFixture` interface (beside `setNextExpiresIn`):

```ts
  /**
   * Force the NEXT /token request to answer with an OAuth error instead of tokens. ONE-SHOT,
   * exactly like setNextExpiresIn — left set, it would also break the next test's token request.
   * `wwwAuthenticate` sets a WWW-Authenticate header, which is what makes oauth4webapi throw
   * WWWAuthenticateChallengeError instead of ResponseBodyError (it checks challenges before it
   * parses the body).
   */
  setNextTokenError(error: string | undefined, wwwAuthenticate?: string): void;
```

- [ ] **Step 2: Implement it in the fixture server**

In `startOidcFixture`, beside `let expiresInOverride`, add:

```ts
  let nextTokenError: { error: string; wwwAuthenticate?: string } | undefined;
```

In the `POST /token` handler, immediately after `const rawBody = await readBody(req);` and BEFORE
the `nextCodeChallenge` block, insert:

```ts
        if (nextTokenError !== undefined) {
          const forced = nextTokenError;
          nextTokenError = undefined; // one-shot
          const headers: Record<string, string> = { 'content-type': 'application/json' };
          if (forced.wwwAuthenticate !== undefined) {
            headers['www-authenticate'] = forced.wwwAuthenticate;
          }
          res.writeHead(forced.error === 'invalid_client' ? 401 : 400, headers);
          res.end(JSON.stringify({ error: forced.error, error_description: 'fixture-forced' }));
          return;
        }
```

In the returned object, beside `setNextExpiresIn`, add:

```ts
    setNextTokenError(error: string | undefined, wwwAuthenticate?: string) {
      nextTokenError = error === undefined ? undefined : { error, ...(wwwAuthenticate !== undefined ? { wwwAuthenticate } : {}) };
    },
```

- [ ] **Step 3: Write the measurement test**

Append to `tests/adapters/oidc.test.ts`. Note it imports `openid-client` DIRECTLY and calls
`refreshTokenGrant` itself — that is the point: `createOidcClient.refresh()` wraps the error and
would hide exactly the fact being measured.

```ts
// ---------------------------------------------------------------------------------------------
// MEASUREMENT (SMA-626 § 2.3). The refresh classifier keys on `ResponseBodyError.error`, and that
// only works if a refused refresh actually arrives as that class. oauth4webapi@3.8.8's
// checkOAuthBodyError calls checkAuthenticationChallenges(response) BEFORE parsing the body
// (build/index.js:917-937), so a response carrying WWW-Authenticate throws
// WWWAuthenticateChallengeError instead — which has no `.error` field, and would make a
// ResponseBodyError-keyed classifier silently never fire.
//
// These two tests pin the measured answer. They call openid-client DIRECTLY, because
// createOidcClient.refresh() wraps its error and would hide the class.
// ---------------------------------------------------------------------------------------------
describe('MEASUREMENT: the error class a refused refresh produces', () => {
  async function rawConfig(): Promise<client.Configuration> {
    return client.discovery(new URL(fixture.issuer), fixture.clientId, { client_secret: fixture.clientSecret }, undefined, {
      execute: [client.allowInsecureRequests],
    });
  }

  it('invalid_grant with no WWW-Authenticate arrives as ResponseBodyError carrying .error', async () => {
    const config = await rawConfig();
    fixture.setNextTokenError('invalid_grant');

    const err: unknown = await client.refreshTokenGrant(config, 'rt').catch((e: unknown) => e);

    expect(err).toBeInstanceOf(client.ResponseBodyError);
    expect((err as InstanceType<typeof client.ResponseBodyError>).error).toBe('invalid_grant');
  });

  // The counter-example, and the reason the DEFINITIVE set is `invalid_grant` alone. RFC 6749
  // § 5.2 says a token endpoint SHOULD send WWW-Authenticate with invalid_client, and when it
  // does, the error never reaches the body parser at all.
  it('a WWW-Authenticate response arrives as WWWAuthenticateChallengeError, with no .error field', async () => {
    const config = await rawConfig();
    fixture.setNextTokenError('invalid_client', 'Basic realm="idp"');

    const err: unknown = await client.refreshTokenGrant(config, 'rt').catch((e: unknown) => e);

    expect(err).toBeInstanceOf(client.WWWAuthenticateChallengeError);
    expect(err).not.toBeInstanceOf(client.ResponseBodyError);
  });
});
```

Add to the file's import block at the top:

```ts
import * as client from 'openid-client';
```

- [ ] **Step 4: Run the measurement**

Run: `pnpm exec vitest run tests/adapters/oidc.test.ts -t 'MEASUREMENT'`

Expected: both PASS.

**If either fails, STOP and report the real shape before writing Task 2's classifier.** That is
this task's whole purpose — the assertion is the measurement, and a red here means the spec's
premise is wrong, not that the test is wrong.

- [ ] **Step 5: Run the whole package suite**

Run: `pnpm exec vitest run`
Expected: `Test Files 22 passed (22)`, `Tests 236 passed (236)` — the baseline plus these two.

- [ ] **Step 6: Commit**

```bash
git add tests/fixtures/jwks.ts tests/adapters/oidc.test.ts
git commit -m "test(ts): measure the error class a refused OIDC refresh produces"
```

---

### Task 2: `RefreshRejected` and the refresh classifier

**Files:**
- Modify: `src/core/errors.ts`
- Modify: `src/adapters/oidc.ts`
- Test: `tests/adapters/oidc.test.ts`

**Interfaces:**
- Consumes: Task 1's `fixture.setNextTokenError`, and its measured finding that `invalid_grant`
  arrives as `client.ResponseBodyError` with `.error === 'invalid_grant'`.
- Produces: `export class RefreshRejected extends AuthError` in `src/core/errors.ts`, with
  `readonly code = 'oidc_refresh_rejected'` and `constructor(readonly oauthError: 'invalid_grant')`.
  Task 6 catches it by `instanceof`.

- [ ] **Step 1: Write the failing tests**

Append to `tests/adapters/oidc.test.ts`:

```ts
describe('createOidcClient — refresh failure classification (SMA-626 § 2.2)', () => {
  it('maps invalid_grant to RefreshRejected, carrying only the OAuth code', async () => {
    const oidc = makeClient();
    fixture.setNextTokenError('invalid_grant');

    const err: unknown = await oidc.refresh('some-refresh-token').catch((e: unknown) => e);

    expect(err).toBeInstanceOf(RefreshRejected);
    expect((err as RefreshRejected).oauthError).toBe('invalid_grant');
    expect((err as RefreshRejected).code).toBe('oidc_refresh_rejected');
    // Redaction: the refresh token, the client secret and the issuer URL must not appear.
    expect((err as Error).message).not.toContain('some-refresh-token');
    expect((err as Error).message).not.toContain(fixture.clientSecret);
    expect((err as Error).message).not.toContain(fixture.issuer);
  });

  // The whole point of § 2.2's narrowing: these are per-DEPLOYMENT faults, not session
  // revocations. Classifying them as definitive would sign out every session at once when
  // someone rotates the client secret without updating PAIGASUS_OIDC_CLIENT_SECRET.
  it.each(['invalid_client', 'unauthorized_client', 'invalid_scope', 'invalid_request', 'server_error', 'temporarily_unavailable'])('treats %s as transient, NOT as RefreshRejected', async (code) => {
    const oidc = makeClient();
    fixture.setNextTokenError(code);

    const err: unknown = await oidc.refresh('some-refresh-token').catch((e: unknown) => e);

    expect(err).not.toBeInstanceOf(RefreshRejected);
    expect((err as Error).message).toMatch(/oidc refresh_token_grant failed/);
  });

  // A non-OAuth failure raised INSIDE the try block must not be classified either.
  it('treats a response missing expires_in as transient', async () => {
    const oidc = makeClient();
    fixture.setNextExpiresIn(undefined);

    const err: unknown = await oidc.refresh('some-refresh-token').catch((e: unknown) => e);

    expect(err).not.toBeInstanceOf(RefreshRejected);
  });
});
```

Add `RefreshRejected` to the file's existing import of `../../src/core/errors.js` (create the
import if the file has none):

```ts
import { RefreshRejected } from '../../src/core/errors.js';
```

- [ ] **Step 2: Run to verify they fail**

Run: `pnpm exec vitest run tests/adapters/oidc.test.ts -t 'refresh failure classification'`
Expected: FAIL — `RefreshRejected` is not exported from `src/core/errors.js`.

- [ ] **Step 3: Add the error type**

Append to `src/core/errors.ts`:

```ts
/**
 * The identity provider REFUSED this refresh token, as opposed to failing to answer. That
 * distinction decides whether the user is signed out or kept on a still-live access token, so it
 * is a core concept, not an adapter detail — adapters/oidc.ts maps its library's error onto this,
 * which is what lets core/single-flight.ts classify without importing adapters/ (SMA-626 § 2.3).
 *
 * `oauthError` is typed as the CLOSED set this package admits, not `string`. RFC 6749 § 5.2 does
 * not bound the value — it is whatever the server's body carried — so a `string` here would leave
 * the redaction claim resting on the call site, and a later edit widening the set would silently
 * widen what may be logged. The type and the classifier's membership test are one fact.
 *
 * NOT exported from src/server.ts, deliberately: no consumer can produce or observe one.
 * getSession() swallows every failure into `null`, and CreateAuthRuntimeDeps exposes no OIDC
 * override.
 */
export class RefreshRejected extends AuthError {
  readonly code = 'oidc_refresh_rejected';
  constructor(readonly oauthError: 'invalid_grant') {
    super(`oidc refresh rejected: ${oauthError}`);
  }
}
```

- [ ] **Step 4: Add the classifier**

In `src/adapters/oidc.ts`, add to the existing `import { ... } from '../core/errors.js'` line (or
create it):

```ts
import { RefreshRejected } from '../core/errors.js';
```

Directly below `wrapError` (`:124-127`), add:

```ts
/**
 * Definitive rejection, or a failure to answer? Only `invalid_grant` means "this refresh token is
 * revoked" — an administrator ended the session, or the user signed out elsewhere. Every other
 * OAuth code is a per-DEPLOYMENT fault that hits every session at the same instant:
 * `invalid_client` after a client-secret rotation, `invalid_scope` after a misconfiguration.
 * Treating those as definitive would sign out the whole fleet into a re-login that fails the same
 * way (handleCallback -> code_exchange_failed -> HTTP 502). See SMA-626 § 2.2.
 *
 * MEASURED against openid-client 6.8.8 / oauth4webapi 3.8.8, pinned by this file's test's
 * "MEASUREMENT" block: a 400 carrying `{"error":"invalid_grant"}` and no WWW-Authenticate header
 * arrives here as `client.ResponseBodyError` with `.error === 'invalid_grant'`. A response that
 * DOES carry WWW-Authenticate arrives as `WWWAuthenticateChallengeError`, which has no `.error`
 * field at all — that is a second reason `invalid_client` is not in this set, since RFC 6749 § 5.2
 * says a token endpoint SHOULD send that header with it.
 *
 * Reads the error CODE only. Never the error object, never its message, never a URL.
 */
function classifyRefreshError(cause: unknown): Error {
  if (cause instanceof client.ResponseBodyError && cause.error === 'invalid_grant') {
    return new RefreshRejected('invalid_grant');
  }
  return wrapError('refresh_token_grant', cause);
}
```

In `refresh()`, replace the catch body:

```ts
      } catch (err) {
        throw classifyRefreshError(err);
      }
```

- [ ] **Step 5: Run the tests**

Run: `pnpm exec vitest run tests/adapters/oidc.test.ts`
Expected: PASS, including the pre-existing "rejects a refresh response missing expires_in" test,
whose `/oidc refresh_token_grant failed/` message is unchanged for every transient case.

- [ ] **Step 6: Run the whole suite and typecheck**

```bash
pnpm exec vitest run
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run paigasus-auth-ts:typecheck
```
Expected: all green.

- [ ] **Step 7: Commit**

```bash
git add src/core/errors.ts src/adapters/oidc.ts tests/adapters/oidc.test.ts
git commit -m "feat(ts): classify a refused OIDC refresh as RefreshRejected"
```

---

### Task 3: The `isSessionRecord` predicate

**Files:**
- Modify: `src/core/session.ts`
- Test: `tests/core/session.test.ts`

**Interfaces:**
- Consumes: `SessionRecord` from `src/core/session.ts`, `makeRecord` from `tests/store-contract.ts:6`.
- Produces: `export function isSessionRecord(value: unknown): value is SessionRecord` in
  `src/core/session.ts`. Task 4 calls it from both adapters.

**Design note for the implementer.** The predicate checks EVERY required field of `SessionRecord`
and of its `principal` (`ResolvedPrincipal`), not only the ones the read path happens to touch. A
`value is SessionRecord` predicate that under-checks is a lie to the type system that compiles
silently; one that over-checks makes `MemorySessionStore.get` return `null` for a valid record,
which reads to a user as "I was logged out" rather than as a type error. Step 1's table is the
control on both directions.

- [ ] **Step 1: Write the failing tests**

Append to `tests/core/session.test.ts`:

```ts
import { isSessionRecord } from '../../src/core/session.js';
import { makeRecord } from '../store-contract.js';

// ---------------------------------------------------------------------------------------------
// SMA-626 § 4.2. A `value is SessionRecord` predicate can drift from the type in two directions,
// and TypeScript reports neither: under-checking compiles silently and lets a poisoned record
// through, over-checking makes MemorySessionStore.get return null for a VALID record — which a
// user reads as "I was logged out", not as a type error. The field table below is the control on
// both directions: every required field must make the predicate false when it is missing, and the
// complete record must make it true.
// ---------------------------------------------------------------------------------------------
describe('isSessionRecord (SMA-626 § 4.2)', () => {
  it('accepts a complete record', () => {
    expect(isSessionRecord(makeRecord())).toBe(true);
  });

  it('accepts a record with no refreshToken (the field is optional)', () => {
    const rec: Record<string, unknown> = { ...makeRecord() };
    delete rec['refreshToken'];
    expect(isSessionRecord(rec)).toBe(true);
  });

  // `exactOptionalPropertyTypes`: an ABSENT refreshToken is legal, an explicit null is not.
  it('rejects an explicit null refreshToken', () => {
    expect(isSessionRecord({ ...makeRecord(), refreshToken: null })).toBe(false);
  });

  it.each(['version', 'rev', 'accessToken', 'accessExpiresAt', 'absoluteExpiresAt', 'idTokenClaims', 'principal'])('rejects a record missing %s', (field) => {
    const rec: Record<string, unknown> = { ...makeRecord() };
    delete rec[field];
    expect(isSessionRecord(rec)).toBe(false);
  });

  it.each(['iss', 'sub'])('rejects a record whose idTokenClaims is missing %s', (claim) => {
    const claims: Record<string, unknown> = { ...makeRecord().idTokenClaims };
    delete claims[claim];
    expect(isSessionRecord({ ...makeRecord(), idTokenClaims: claims })).toBe(false);
  });

  it.each(['principalPrn', 'issuer', 'subject', 'memberships', 'roleGrants', 'grantsAvailable'])('rejects a record whose principal is missing %s', (field) => {
    const principal: Record<string, unknown> = { ...makeRecord().principal };
    delete principal[field];
    expect(isSessionRecord({ ...makeRecord(), principal })).toBe(false);
  });

  // HOLE 3 (§ 4.1), stated as its own test because it is the one with a security consequence:
  // can() FAILS OPEN on grantsAvailable (src/client.ts:67), so a principal missing that field
  // makes every browser-side capability check return true.
  it('rejects a principal shaped { roleGrants: [] } — the can() fail-open case', () => {
    expect(isSessionRecord({ ...makeRecord(), principal: { roleGrants: [] } })).toBe(false);
  });

  // HOLE 1 (§ 4.1): JSON.parse('null') SUCCEEDS, so the parse guard never fires on this one.
  it.each([null, undefined, 3, 'a string', [], true])('rejects the non-object value %p', (value) => {
    expect(isSessionRecord(value)).toBe(false);
  });

  // HOLE 2 (§ 4.1): two NaN comparisons in resolveSession let this through as a LIVE session
  // carrying accessToken: undefined.
  it('rejects a body of { version: 1 } and nothing else', () => {
    expect(isSessionRecord({ version: 1 })).toBe(false);
  });

  it('rejects a version other than 1', () => {
    expect(isSessionRecord({ ...makeRecord(), version: 2 })).toBe(false);
  });

  it.each(['rev', 'accessExpiresAt', 'absoluteExpiresAt'])('rejects a non-finite %s', (field) => {
    expect(isSessionRecord({ ...makeRecord(), [field]: Number.NaN })).toBe(false);
  });

  it('rejects a roleGrants element that is not a { scopePrn, roleKey } object', () => {
    const principal = { ...makeRecord().principal, roleGrants: ['not-an-object'] };
    expect(isSessionRecord({ ...makeRecord(), principal })).toBe(false);
  });
});
```

- [ ] **Step 2: Run to verify they fail**

Run: `pnpm exec vitest run tests/core/session.test.ts -t 'isSessionRecord'`
Expected: FAIL — `isSessionRecord` is not exported.

- [ ] **Step 3: Implement the predicate**

Append to `src/core/session.ts`:

```ts
function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/**
 * The ONE statement of what a stored session record must look like (SMA-626 § 4.2). Both adapters
 * call it, replacing the standalone `version !== 1` check each of them used to carry, so the
 * absent-and-deleted policy is stated once rather than twice.
 *
 * IT CHECKS EVERY REQUIRED FIELD, not only the ones the read path touches. Three measured holes
 * are the reason:
 *
 *   1. `JSON.parse('null')` SUCCEEDS, so redis-store's parse guard never fires on a stored
 *      literal `null`; reading `.version` off it then throws a TypeError that #guarded converts
 *      into SessionStoreUnavailable — a store-outage signal against a healthy Redis.
 *   2. A body of `{ version: 1 }` passes two NaN comparisons in a row in resolveSession
 *      (`now >= undefined` and `now >= NaN` are both false), so it is returned as a LIVE session
 *      carrying `accessToken: undefined`.
 *   3. A principal shaped `{ roleGrants: [] }` yields `grantsAvailable: undefined` through
 *      toSessionView, and `can()` FAILS OPEN on that field (src/client.ts:67) — so every
 *      browser-side capability check returns true. The fail-open is correct and deliberate; this
 *      predicate is what keeps a poisoned record from reaching it.
 *
 * `refreshToken` is the one optional field: absent is legal, an explicit `null` is not
 * (`exactOptionalPropertyTypes`).
 */
export function isSessionRecord(value: unknown): value is SessionRecord {
  if (!isObject(value)) return false;
  if (value['version'] !== 1) return false;
  if (!Number.isFinite(value['rev'])) return false;
  if (!Number.isFinite(value['accessExpiresAt'])) return false;
  if (!Number.isFinite(value['absoluteExpiresAt'])) return false;
  if (typeof value['accessToken'] !== 'string') return false;
  if ('refreshToken' in value && typeof value['refreshToken'] !== 'string') return false;

  const claims = value['idTokenClaims'];
  if (!isObject(claims)) return false;
  if (typeof claims['iss'] !== 'string' || typeof claims['sub'] !== 'string') return false;

  const principal = value['principal'];
  if (!isObject(principal)) return false;
  if (principal['principalPrn'] !== null && typeof principal['principalPrn'] !== 'string') return false;
  if (typeof principal['issuer'] !== 'string' || typeof principal['subject'] !== 'string') return false;
  if (typeof principal['grantsAvailable'] !== 'boolean') return false;
  if (!Array.isArray(principal['memberships'])) return false;
  const grants = principal['roleGrants'];
  if (!Array.isArray(grants)) return false;
  return grants.every((g) => isObject(g) && typeof g['scopePrn'] === 'string' && typeof g['roleKey'] === 'string');
}
```

- [ ] **Step 4: Run the tests**

Run: `pnpm exec vitest run tests/core/session.test.ts`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/core/session.ts tests/core/session.test.ts
git commit -m "feat(ts): add isSessionRecord, one statement of the record policy"
```

---

### Task 4: Apply the predicate in both adapters

**Files:**
- Modify: `src/adapters/redis-store.ts:150-172`
- Modify: `src/adapters/memory-store.ts:33-41`
- Test: `tests/adapters/redis-store.test.ts`, `tests/adapters/memory-store.test.ts`

**Interfaces:**
- Consumes: `isSessionRecord` from Task 3.
- Produces: nothing new. Task 7 depends on this landing first (§ 2.4's ordering constraint).

**Design note.** `RedisSessionStore.get` is private to the module — it is only reachable through
`createRedisSessionStore`, which connects to a real Redis. The unit tests therefore drive it
through the `vi.mock('redis')` harness Task 9 also uses. **Write this task's tests with that mock;
Task 9 reuses the same factory.**

- [ ] **Step 1: Write the failing tests for the three holes**

Create `tests/adapters/redis-store-parse.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-626 § 4. The parse guard in redis-store.ts's `get` is defeated by three stored values, each
// of which restores the failure it was written to close. These tests are written BEFORE the fix,
// and each one is a measurement: if it passes against unmodified code, the hole is not real and
// the spec is wrong.
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { makeRecord } from '../store-contract.js';

const state = { value: null as string | null, deleted: [] as string[] };

vi.mock('redis', () => ({
  createClient: () => ({
    connect: () => Promise.resolve(),
    on: () => undefined,
    get: () => Promise.resolve(state.value),
    del: (key: string) => {
      state.deleted.push(key);
      state.value = null;
      return Promise.resolve(1);
    },
    set: () => Promise.resolve('OK'),
    eval: () => Promise.resolve(1),
    close: () => Promise.resolve(),
  }),
}));

const { createRedisSessionStore } = await import('../../src/adapters/redis-store.js');

async function store() {
  return createRedisSessionStore({ url: 'redis://127.0.0.1:6379', commandTimeoutMs: 1000, keyPrefix: '' });
}

beforeEach(() => {
  state.value = null;
  state.deleted = [];
});

describe('a poisoned session record is absent-and-deleted, never a store failure', () => {
  it('accepts a well-formed record (the positive control)', async () => {
    state.value = JSON.stringify(makeRecord());
    await expect((await store()).get('s')).resolves.toMatchObject({ version: 1, accessToken: 'AT' });
    expect(state.deleted).toEqual([]);
  });

  // HOLE 1. JSON.parse('null') SUCCEEDS, so the parse guard never fires. Before the fix, reading
  // `.version` off null throws a TypeError that #guarded turns into SessionStoreUnavailable — a
  // store-outage signal raised against a perfectly healthy Redis.
  it('treats a stored literal null as absent and deletes it', async () => {
    state.value = 'null';
    await expect((await store()).get('s')).resolves.toBeNull();
    expect(state.deleted).toEqual(['pgs:sess:s']);
  });

  // HOLE 2. Parses fine, version is 1, everything else is missing. Before the fix this is
  // returned as a LIVE session carrying accessToken: undefined.
  it('treats a { version: 1 } body as absent and deletes it', async () => {
    state.value = '{"version":1}';
    await expect((await store()).get('s')).resolves.toBeNull();
    expect(state.deleted).toEqual(['pgs:sess:s']);
  });

  // HOLE 3. The one with a security consequence: can() fails OPEN on grantsAvailable
  // (src/client.ts:67), so this record makes every browser-side capability check return true.
  it('treats a principal shaped { roleGrants: [] } as absent and deletes it', async () => {
    state.value = JSON.stringify({ ...makeRecord(), principal: { roleGrants: [] } });
    await expect((await store()).get('s')).resolves.toBeNull();
    expect(state.deleted).toEqual(['pgs:sess:s']);
  });

  it('still treats unparseable JSON as absent and deletes it', async () => {
    state.value = '{not valid json';
    await expect((await store()).get('s')).resolves.toBeNull();
    expect(state.deleted).toEqual(['pgs:sess:s']);
  });

  it('still treats a version mismatch as absent and deletes it', async () => {
    state.value = JSON.stringify({ ...makeRecord(), version: 2 });
    await expect((await store()).get('s')).resolves.toBeNull();
    expect(state.deleted).toEqual(['pgs:sess:s']);
  });
});
```

Append to `tests/adapters/memory-store.test.ts`:

```ts
// SMA-626 § 4.3: the memory adapter never parses bytes, so only a caller violating the type could
// poison it. It adopts the same predicate anyway, so ONE rule covers both adapters — and this
// test is what proves the shared predicate does not start deleting VALID records here, which is
// the over-checking direction of the drift Task 3's table also guards.
describe('MemorySessionStore and the shared record predicate (SMA-626 § 4)', () => {
  it('returns a well-formed record unchanged', async () => {
    const s = new MemorySessionStore();
    await s.set('s', makeRecord(), 60_000, null);
    await expect(s.get('s')).resolves.toMatchObject({ version: 1, accessToken: 'AT' });
  });

  it('treats a type-violating record as absent and deletes it', async () => {
    const s = new MemorySessionStore();
    await s.set('s', { version: 1 } as unknown as SessionRecord, 60_000, null);
    await expect(s.get('s')).resolves.toBeNull();
    await expect(s.get('s')).resolves.toBeNull(); // deleted, not merely reported absent
  });
});
```

Make sure that file imports `makeRecord` from `../store-contract.js` and `type SessionRecord`
from `../../src/core/session.js`.

- [ ] **Step 2: Run to verify the three holes are REAL**

Run: `pnpm exec vitest run tests/adapters/redis-store-parse.test.ts`

Expected: the two positive controls and the two already-covered cases PASS; the three hole tests
FAIL. Record HOW each fails — hole 1 as a thrown `SessionStoreUnavailable`, holes 2 and 3 as a
resolved record instead of `null`.

**If a hole test PASSES here, stop and report it.** The spec's § 4.1 claim would be wrong, and the
plan needs correcting before the fix goes in.

- [ ] **Step 3: Apply the predicate in the Redis adapter**

In `src/adapters/redis-store.ts`, add to the `core/session.js` import:

```ts
import { isSessionRecord } from '../core/session.js';
```

Replace the body of `get` (`:150-172`) with:

```ts
  get(sid: string): Promise<SessionRecord | null> {
    return this.#guarded(async () => {
      const raw = await this.#client.get(sessKey(this.#keyPrefix, sid));
      if (raw === null) return null;
      let parsed: unknown;
      try {
        parsed = JSON.parse(raw);
      } catch {
        // An unparseable stored value is ABSENT-AND-DELETED. Without this, JSON.parse throwing
        // surfaces as SessionStoreUnavailable through #guarded's catch-all, which turns one
        // poisoned key into a false store-outage signal against a healthy Redis.
        await this.#client.del(sessKey(this.#keyPrefix, sid));
        return null;
      }
      // ONE rule for every shape that is not a session record — a version mismatch, a stored
      // literal `null` (which JSON.parse accepts), a body missing the fields the read path needs,
      // and a principal missing `grantsAvailable`, which can() fails OPEN on. See
      // isSessionRecord's doc comment for the three measured holes this closes (SMA-626 § 4).
      if (!isSessionRecord(parsed)) {
        await this.#client.del(sessKey(this.#keyPrefix, sid));
        return null;
      }
      return parsed;
    });
  }
```

Note `parsed` is now `unknown`, not `SessionRecord` — the `as SessionRecord` cast is deleted, and
`isSessionRecord` is what narrows it.

- [ ] **Step 4: Apply the predicate in the memory adapter**

In `src/adapters/memory-store.ts`, add the import and replace `get` (`:33-41`):

```ts
  get(sid: string): Promise<SessionRecord | null> {
    const rec = this.#live(this.#records, sid);
    // The SAME predicate the Redis adapter uses (SMA-626 § 4). This adapter never parses bytes,
    // so only a caller violating the type could poison it — sharing the rule means the policy is
    // stated once rather than twice and drifting.
    if (rec !== null && !isSessionRecord(rec)) {
      this.#records.delete(sid);
      return Promise.resolve(null);
    }
    return Promise.resolve(rec);
  }
```

- [ ] **Step 5: Run the tests**

Run: `pnpm exec vitest run tests/adapters/`
Expected: PASS, all files.

- [ ] **Step 6: Run the whole suite and typecheck**

```bash
pnpm exec vitest run
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run paigasus-auth-ts:typecheck
```
Expected: green. The store contract suite (`tests/store-contract.ts`, run against both adapters)
must still pass — that is the check on the over-checking direction.

- [ ] **Step 7: Commit**

```bash
git add src/adapters/redis-store.ts src/adapters/memory-store.ts tests/adapters/
git commit -m "fix(ts): make the absent-and-deleted record policy total"
```

---

### Task 5: The CAS retry guard (test only, no behaviour change)

The spec § 5 guard 4. Today's test stubs `set` to fail forever, so deleting the retry entirely
still yields `null` and the test stays green. This task closes that BEFORE Task 6 modifies the same
file, so the retry is protected while that edit happens.

**Files:**
- Test: `tests/core/single-flight.test.ts`

**Interfaces:**
- Consumes: `resolveSession`, `MemorySessionStore`, and the file's existing `deps()` and
  `makeRecord()` helpers.
- Produces: nothing. Test-only.

- [ ] **Step 1: Write the failing test**

Append to `tests/core/single-flight.test.ts`, inside the same describe block as the other F1 tests:

```ts
  // SMA-626 § 5 guard 4. The existing persist-failure test stubs `set` to fail FOREVER, so the
  // retry at single-flight.ts:151-153 could be deleted entirely and that test would still see
  // `null` and stay green. This one fails the CAS exactly ONCE against UNCHANGED state, which is
  // the only path that reaches the retry — delete the retry and this test sees `null` instead of
  // a refreshed record.
  //
  // The stub is installed AFTER the setup write, because every fixture here seeds the record with
  // `store.set(..., null)` and a stub counting "the first set call" would break that seed.
  it('retries the compare-and-set ONCE against unchanged state, and persists (F1, guard 4)', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ rev: 7, accessExpiresAt: Date.now() - 1 }), 60_000, null);

    const originalSet = store.set.bind(store);
    let refused = false;
    store.set = (sid, rec, ttl, expectedRev) => {
      // Refuse exactly the first fenced write, and write NOTHING — so the record stays at rev 7
      // and resolveSession's re-read finds `winner.rev === fresh.rev`, the retry branch.
      if (!refused && expectedRev === 7) {
        refused = true;
        return Promise.resolve(false);
      }
      return originalSet(sid, rec, ttl, expectedRev);
    };

    const out = await resolveSession(deps(store, () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300 })), 's');

    expect(refused).toBe(true); // the retry branch really was reached
    expect(out?.accessToken).toBe('AT2');
    expect(out?.rev).toBe(8);
    expect((await store.get('s'))?.accessToken).toBe('AT2');
  });
```

- [ ] **Step 2: Run it — it must PASS against unmodified code**

Run: `pnpm exec vitest run tests/core/single-flight.test.ts -t 'retries the compare-and-set'`
Expected: PASS. The retry exists today; this test simply describes it.

- [ ] **Step 3: Prove the test bites — delete the guard**

In `src/core/single-flight.ts`, comment out the two retry lines (`:152-153`) with a marker so the
restore is an edit, never a `git checkout --`:

```ts
          // MUTATION-PROBE-SMA626 (delete these two lines to restore):
          // written = { ...next, rev: winner.rev + 1 };
          // ok = await store.set(sid, written, ttlMs, winner.rev);
```

Run: `pnpm exec vitest run tests/core/single-flight.test.ts -t 'retries the compare-and-set'`
Expected: **FAIL** — `out` is `null`.

Also confirm the OLD test still passes with the guard deleted, which is the defect this task
closes:

Run: `pnpm exec vitest run tests/core/single-flight.test.ts -t 'persist'`
Expected: PASS even with the retry deleted.

- [ ] **Step 4: Restore the guard**

Delete the three commented `MUTATION-PROBE-SMA626` lines and restore the two real lines. Do NOT
use `git checkout --` — it would discard the new test too.

Run: `pnpm exec vitest run tests/core/single-flight.test.ts`
Expected: PASS.

- [ ] **Step 5: Record the measurement in the test**

Add one line to the new test's comment block:

```ts
  // MEASURED 2026-09-10: with the two retry lines deleted this test reds (`out` is null) while
  // the fail-forever test above stays green.
```

- [ ] **Step 6: Commit**

```bash
git add tests/core/single-flight.test.ts
git commit -m "test(ts): make the single-flight CAS retry red when deleted"
```

---

### Task 6: The refresh degrade, the delete, and the event fields

**Files:**
- Modify: `src/core/single-flight.ts:25` (`ResolvedSession`), `:120-135` (the refresh catch),
  `:177-186` (the timeout branch)
- Test: `tests/core/single-flight.test.ts`, `tests/next/get-session.test.ts`

**Interfaces:**
- Consumes: `RefreshRejected` from Task 2 (`src/core/errors.js`).
- Produces:
  - `export type ResolvedSession = SessionRecord & { refreshState?: 'pending' | 'failed' }`
  - `session.refresh_failed` now carries `{ sid, reason: 'rejected' | 'transient', degraded: boolean }`
  - `session.deleted` gains the reason value `'refresh_rejected'`

**This task re-baselines three existing assertions** (see Global Constraints). They are listed in
Step 5.

- [ ] **Step 1: Write the failing tests**

Append to `tests/core/single-flight.test.ts`:

```ts
// ---------------------------------------------------------------------------------------------
// SMA-626 § 2.3. The lock-timeout path already degrades to a still-live access token; a THROWN
// refresh did not, so a transient IdP outage signed users out with up to skewMs of token life
// left. Classification is what makes the degrade safe: only invalid_grant means the refresh token
// is actually revoked.
// ---------------------------------------------------------------------------------------------
describe('a failing refresh (SMA-626 § 2.3)', () => {
  const transient = () => Promise.reject(new Error('oidc refresh_token_grant failed: TypeError'));
  const rejected = () => Promise.reject(new RefreshRejected('invalid_grant'));

  it('degrades to the live access token when the failure is transient', async () => {
    const store = new MemorySessionStore();
    const rec = makeRecord({ accessExpiresAt: Date.now() + 30_000 }); // inside skew, still live
    await store.set('s', rec, 60_000, null);
    const { logger, events } = recordingLogger();

    const out = await resolveSession({ ...deps(store, transient), logger, skewMs: 60_000 }, 's');

    expect(out?.accessToken).toBe('AT');
    expect(out?.refreshState).toBe('failed');
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'transient', degraded: true }]);
    expect(await store.get('s')).not.toBeNull(); // a transient failure must NOT delete
  });

  it('signs out when the failure is transient and the token is hard-expired', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { logger, events } = recordingLogger();

    await expect(resolveSession({ ...deps(store, transient), logger }, 's')).rejects.toThrow(/refresh_token_grant/);
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'transient', degraded: false }]);
  });

  // A revoked refresh token means the session is genuinely dead. Degrading would keep it alive for
  // up to skewMs, which § 2.1 calls a real exposure.
  it('never degrades a definitive rejection, even with a live access token', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() + 30_000 }), 60_000, null);
    const { logger, events } = recordingLogger();

    await expect(resolveSession({ ...deps(store, rejected), logger, skewMs: 60_000 }, 's')).rejects.toBeInstanceOf(RefreshRejected);
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'rejected', degraded: false }]);
  });

  // Without this, a revoked refresh token and a live access token sit in Redis for the full ttlMs
  // and every later getSession() re-takes the lock and re-calls the token endpoint.
  it('DELETES the record on a definitive rejection, and says why', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { logger, events } = recordingLogger();

    await expect(resolveSession({ ...deps(store, rejected), logger }, 's')).rejects.toBeInstanceOf(RefreshRejected);
    expect(await store.get('s')).toBeNull();
    expect(events).toContainEqual(['session.deleted', { sid: sidTag('s'), reason: 'refresh_rejected' }]);
  });

  // `reason` is what keeps the two apart. With `degraded` alone, both of these log the identical
  // line, which is the conflation § 2 exists to remove.
  it('a rejection and a hard-expired transient failure log DIFFERENT reasons', async () => {
    const run = async (refresh: () => Promise<never>) => {
      const store = new MemorySessionStore();
      await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
      const { logger, events } = recordingLogger();
      await resolveSession({ ...deps(store, refresh), logger }, 's').catch(() => undefined);
      return events.find(([name]) => name === 'session.refresh_failed')?.[1];
    };

    expect(await run(rejected)).toMatchObject({ reason: 'rejected' });
    expect(await run(transient)).toMatchObject({ reason: 'transient' });
  });

  // § 2.3's cap re-check. handleCallback sets accessExpiresAt and absoluteExpiresAt
  // INDEPENDENTLY (routes.ts:233-234), and accessExpiresAt is clamped to the cap only on a
  // refresh write — so an IdP whose expires_in exceeds the absolute TTL mints a first record where
  // the access token outlives the cap. The degrade must not resurrect it.
  it('does not degrade past the absolute cap, even with a live access token', async () => {
    const store = new MemorySessionStore();
    const now = Date.now();
    await store.set('s', makeRecord({ accessExpiresAt: now + 30_000, absoluteExpiresAt: now + 1_000 }), 60_000, null);

    await expect(resolveSession({ ...deps(store, transient), skewMs: 60_000 }, 's')).rejects.toThrow(/refresh_token_grant/);
  });
});
```

Add `RefreshRejected` to the file's `../../src/core/errors.js` import.

**If the file has no `recordingLogger` helper, copy the one in `tests/next/get-session.test.ts`.**

Also append the matching cap test for the TIMEOUT branch, which holds the same hole:

```ts
  // The timeout branch at single-flight.ts:180-185 has the identical hole and gets the identical
  // fix — a record whose access token outlives its absolute cap must not be returned there either.
  it('the lock-timeout branch does not degrade past the absolute cap', async () => {
    const store = new MemorySessionStore();
    const now = Date.now();
    await store.set('s', makeRecord({ accessExpiresAt: now + 30_000, absoluteExpiresAt: now + 1_000 }), 60_000, null);
    store.tryAcquireLock = () => Promise.resolve(false); // never win the lock

    const out = await resolveSession({ ...deps(store, () => Promise.reject(new Error('unused'))), skewMs: 60_000, lockWaitMs: 10 }, 's');

    expect(out).toBeNull();
  });
```

- [ ] **Step 2: Run to verify they fail**

Run: `pnpm exec vitest run tests/core/single-flight.test.ts -t 'a failing refresh'`
Expected: FAIL — `refreshState` is undefined, the events lack `reason`/`degraded`, no delete.

- [ ] **Step 3: Change `ResolvedSession`**

In `src/core/single-flight.ts:25`:

```ts
/**
 * `refreshState` says WHY the returned record may be stale, and the two values are opposites:
 *
 *   'pending' — another holder is refreshing right now (the lock wait timed out). The next
 *               request very likely sees a fresh record.
 *   'failed'  — the refresh itself failed transiently and NOBODY is refreshing. The next request
 *               fails the same way.
 *
 * One boolean cannot carry both, and a consumer reading it as "retry shortly" would hot-loop
 * through an identity-provider outage. It replaced `refreshPending` in SMA-626 § 2.3, while that
 * flag still had no consumer anywhere in the repository.
 */
export type ResolvedSession = SessionRecord & { refreshState?: 'pending' | 'failed' };
```

- [ ] **Step 4: Rewrite the refresh catch**

Replace `src/core/single-flight.ts`'s refresh try/catch (the block around `:129-135`) with:

```ts
        let tokens: RefreshedTokens;
        try {
          tokens = await refresh(refreshToken);
        } catch (err) {
          // SMA-626 § 2.3. THREE outcomes, not one.
          //
          // A definitive rejection (RefreshRejected — only `invalid_grant`, see
          // adapters/oidc.ts's classifier) means the refresh token is revoked: an administrator
          // ended this session, or the user signed out elsewhere. Sign out, and DELETE — leaving
          // the record would keep a revoked refresh token and a live access token in the store for
          // the full ttlMs, and every later read would re-take the lock and re-call the token
          // endpoint until then. The same reasoning as the `no_refresh_token` and
          // `session.refresh.persist_failed` deletes above and below.
          //
          // A transient failure (a network error, a timeout, a 5xx, an unknown OAuth code) with a
          // still-live access token degrades exactly the way the lock-timeout branch does: the
          // token works, so proceed on it and let the next request retry. Signing the user out
          // there would throw away up to skewMs of perfectly good session because someone else's
          // service blipped.
          //
          // A transient failure with a hard-expired token has nothing left to proceed on.
          //
          // `liveUntil` takes the MINIMUM of the two expiries. handleCallback sets them
          // independently (http/routes.ts:233-234) and only a refresh write clamps accessExpiresAt
          // to the cap, so an IdP whose `expires_in` exceeds PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS
          // mints a first record whose access token outlives its own absolute cap.
          //
          // `reason` is REQUIRED, not decoration. With `degraded` alone, a benign single-session
          // revocation and an outage that signs users out produce the identical line — the exact
          // conflation this whole section exists to remove.
          const rejected = err instanceof RefreshRejected;
          const liveUntil = Math.min(fresh.accessExpiresAt, fresh.absoluteExpiresAt);
          const degraded = !rejected && Date.now() < liveUntil;

          logger.event('session.refresh_failed', { sid: sidTag(sid), reason: rejected ? 'rejected' : 'transient', degraded });

          if (degraded) return { ...fresh, refreshState: 'failed' };
          if (rejected) {
            await store.delete(sid);
            logger.event('session.deleted', { sid: sidTag(sid), reason: 'refresh_rejected' });
          }
          throw err;
        }
```

Add to the imports at the top of the file:

```ts
import { RefreshRejected } from './errors.js';
```

- [ ] **Step 5: Fix the timeout branch's identical cap hole and rename the flag**

Replace `src/core/single-flight.ts:180-185`'s return with:

```ts
      // Still inside the skew window means the access token is live: proceed on it and let the
      // next request refresh. `Math.min` for the same reason the refresh catch uses it — a first
      // record's accessExpiresAt is not clamped to its absolute cap (SMA-626 § 2.3).
      return Date.now() < Math.min(last.accessExpiresAt, last.absoluteExpiresAt) ? { ...last, refreshState: 'pending' } : null;
```

- [ ] **Step 6: Re-baseline the three existing assertions**

These three are expected to red, and each is a deliberate update:

1. `tests/core/single-flight.test.ts:163` — `expect(out?.refreshPending).toBe(true);` becomes
   `expect(out?.refreshState).toBe('pending');`
2. `tests/core/single-flight.test.ts:219` —
   `expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s') }]);` becomes
   `expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'transient', degraded: false }]);`
   (verify the fixture's record is hard-expired; if it is live, the expected `degraded` is `true`
   and the call no longer rejects — adjust the test's own assertion accordingly and say so in a
   comment)
3. `tests/next/get-session.test.ts:165` — the same field addition.

- [ ] **Step 7: Run everything**

```bash
pnpm exec vitest run
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run paigasus-auth-ts:typecheck
```
Expected: all green. `refreshPending` must appear NOWHERE in `src/` or `tests/` — confirm with
`grep -rn refreshPending src tests`, which must print nothing.

- [ ] **Step 8: Commit**

```bash
git add src/core/single-flight.ts tests/
git commit -m "fix(ts): degrade a transient refresh failure instead of signing out"
```

---

### Task 7: Narrow the store signal

**Depends on Task 4.** Until the shape predicate is applied, a stored literal `null` mints a
`SessionStoreUnavailable` against a healthy Redis, and this task's central claim is false.

**Files:**
- Modify: `src/ports/logger.ts:14-24`
- Modify: `src/ports/session-store.ts`
- Modify: `src/next/get-session.ts:63-73`
- Test: `tests/next/get-session.test.ts`

**Interfaces:**
- Consumes: `SessionStoreUnavailable` from `src/core/errors.js`.
- Produces: `'session.resolve_failed'` as a member of `AuthEventName`.

- [ ] **Step 1: Write the failing tests**

Append to `tests/next/get-session.test.ts`:

```ts
// ---------------------------------------------------------------------------------------------
// SMA-626 § 2.4. This catch used to log `store.unavailable` for EVERY failure, so an identity-
// provider outage raised the store-outage rate while Redis was perfectly healthy and an operator
// went and inspected the wrong system. `store.unavailable` now means the store, and nothing else.
// ---------------------------------------------------------------------------------------------
describe('getSession failure attribution (SMA-626 § 2.4)', () => {
  it('logs store.unavailable ONLY for a SessionStoreUnavailable', async () => {
    cookiesMock.mockResolvedValue(cookieJar('some-sid'));
    const { logger, events } = recordingLogger();

    await getSession({ ...baseRuntime(unavailableStore()), logger });

    expect(events).toEqual([['store.unavailable', { sid: sidTag('some-sid'), stage: 'get_session' }]]);
  });

  it('logs session.resolve_failed, NOT store.unavailable, when the IdP definitively rejects', async () => {
    cookiesMock.mockResolvedValue(cookieJar('sid-needs-refresh'));
    const store = new MemorySessionStore();
    await store.set('sid-needs-refresh', { ...liveRecord(), accessExpiresAt: Date.now() - 1, refreshToken: 'RT' }, 999_000, null);
    const { logger, events } = recordingLogger();

    await expect(
      getSession({
        ...baseRuntime(store),
        logger,
        oidc: { ...unusedOidc(), refresh: () => Promise.reject(new RefreshRejected('invalid_grant')) },
      }),
    ).resolves.toBeNull();

    expect(events).toContainEqual(['session.resolve_failed', { sid: sidTag('sid-needs-refresh'), stage: 'get_session' }]);
    expect(events.some(([name]) => name === 'store.unavailable')).toBe(false);
  });

  // The SessionStore port is exported publicly and an injected store, a decorator, or a future
  // adapter may fail with any error class. This outcome is DELIBERATE and documented in
  // ports/session-store.ts, not an accident — the test exists so the decision is visible rather
  // than surviving only because every current fixture happens to throw the right class.
  it('a store failing with some OTHER error class logs session.resolve_failed', async () => {
    cookiesMock.mockResolvedValue(cookieJar('some-sid'));
    const store = new MemorySessionStore();
    store.get = () => Promise.reject(new Error('a decorator blew up'));
    const { logger, events } = recordingLogger();

    await expect(getSession({ ...baseRuntime(store), logger })).resolves.toBeNull();

    expect(events).toEqual([['session.resolve_failed', { sid: sidTag('some-sid'), stage: 'get_session' }]]);
  });
});
```

Add `RefreshRejected` to the file's `../../src/core/errors.js` import.

- [ ] **Step 2: Run to verify they fail**

Run: `pnpm exec vitest run tests/next/get-session.test.ts -t 'failure attribution'`
Expected: FAIL — every failure still logs `store.unavailable`.

- [ ] **Step 3: Add the event to the closed vocabulary**

In `src/ports/logger.ts`, add to the `AuthEventName` union, after `'session.refresh_timeout'`:

```ts
  | 'session.resolve_failed'
```

- [ ] **Step 4: Write down the port contract**

In `src/ports/session-store.ts`, add above `export interface SessionStore`:

```ts
/**
 * FAILURE SIGNALLING IS PART OF THIS CONTRACT (SMA-626 § 2.4). An implementation that cannot
 * reach its backing store MUST reject with `SessionStoreUnavailable` (core/errors.ts). Callers
 * classify on that class: `next/get-session.ts` logs `store.unavailable` for it and
 * `session.resolve_failed` for anything else, so a store that fails with some other error class
 * is reported as a resolve failure rather than a store outage. That is a defined outcome, not a
 * bug — but an adapter, a decorator, or a test double that wants the store signal has to raise
 * the right class.
 */
```

- [ ] **Step 5: Narrow the catch**

In `src/next/get-session.ts`, replace the catch block (`:63-73`) with:

```ts
  } catch (err) {
    // A store blip degrades to signed-out rather than a 500 — see the file header. That degrade
    // must not also be SILENT, so every failure still logs exactly one line.
    //
    // WHICH line is the point (SMA-626 § 2.4). This catch is deliberately broad, and it used to
    // log `store.unavailable` for every failure — so an identity-provider outage raised the
    // store-outage rate while Redis was healthy, and an operator investigating a mass sign-out
    // went and inspected a perfectly good store. Classifying here costs one `instanceof` and makes
    // `store.unavailable` mean the store.
    //
    // Same field discipline either way: a truncated sid, a fixed stage name, never the caught
    // error object (it may embed a DSN).
    if (err instanceof SessionStoreUnavailable) {
      runtime.logger.event('store.unavailable', { sid: sidTag(sid), stage: 'get_session' });
    } else {
      runtime.logger.event('session.resolve_failed', { sid: sidTag(sid), stage: 'get_session' });
    }
    return null;
  }
```

Add to the imports:

```ts
import { SessionStoreUnavailable } from '../core/errors.js';
```

- [ ] **Step 6: Run everything**

```bash
pnpm exec vitest run
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run paigasus-auth-ts:typecheck
```
Expected: green.

- [ ] **Step 7: Commit**

```bash
git add src/ports/logger.ts src/ports/session-store.ts src/next/get-session.ts tests/next/get-session.test.ts
git commit -m "fix(ts): report a store outage only when the store is the failure"
```

---

### Task 8: One route table, and a table-driven dispatch

**Files:**
- Create: `src/http/route-table.ts`
- Modify: `src/http/routes.ts:43-84`
- Modify: `src/middleware.ts:38-65`
- Test: `tests/middleware.test.ts`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces:
  - `export const AUTH_ROUTE_SUFFIXES` — a readonly tuple of the four suffixes.
  - `export type AuthRouteSuffix = (typeof AUTH_ROUTE_SUFFIXES)[number]`.
  - `authRoutePaths` keeps its existing signature: `(runtime: { basePath: string }) => readonly string[]`.

- [ ] **Step 1: Write the failing tests**

Append to `tests/middleware.test.ts`:

```ts
// ---------------------------------------------------------------------------------------------
// SMA-626 § 3. authRoutePaths and createAuthRoutes's dispatch used to be two independent lists.
// The existing cross-check above catches a STALE path; nothing caught a route ADDED to the
// dispatch and forgotten in authRoutePaths, which is the infinite-redirect-loop failure the helper
// exists to prevent. Both now derive from one table, so the type system closes it — a fifth suffix
// with no handler, or a handler with no suffix, fails typecheck.
// ---------------------------------------------------------------------------------------------
describe('the shared route table (SMA-626 § 3)', () => {
  it('authRoutePaths is exactly the table mapped over basePath', () => {
    expect(authRoutePaths({ basePath: '/iam' })).toEqual(AUTH_ROUTE_SUFFIXES.map((s) => `/iam${s}`));
  });

  it('holds four suffixes, each starting with /auth/', () => {
    expect(AUTH_ROUTE_SUFFIXES).toHaveLength(4);
    for (const suffix of AUTH_ROUTE_SUFFIXES) expect(suffix.startsWith('/auth/')).toBe(true);
  });

  it('works for a root-mounted zone, where basePath is the empty string', () => {
    expect(authRoutePaths({ basePath: '' })).toEqual(['/auth/login', '/auth/callback', '/auth/logout', '/auth/logout/callback']);
  });

  // THE BACKSTOP. The Record is the primary control, but a hand-written `if (pathname === ...)`
  // arm placed before the lookup would still bypass it. After this task there must be ZERO direct
  // comparisons of `pathname` in routes.ts, so any occurrence reds — a much stronger assertion
  // than counting to four, which `==`, reversed operands, or a comment all defeat.
  //
  // RESIDUAL, stated: a comparison written another way — a `switch (pathname)`, an aliased
  // variable, a dispatch in a delegated file — escapes this. The Record is what closes the case
  // that actually happens.
  it('routes.ts contains no direct pathname comparison', () => {
    const source = readFileSync(resolve(SRC, 'http/routes.ts'), 'utf8');
    const matches = source.match(/pathname\s*===?|===?\s*pathname/g) ?? [];
    expect(matches).toEqual([]);
  });
});
```

Add to the file's imports:

```ts
import { readFileSync } from 'node:fs';
import { AUTH_ROUTE_SUFFIXES } from '../src/http/route-table.js';
```

- [ ] **Step 2: Run to verify they fail**

Run: `pnpm exec vitest run tests/middleware.test.ts -t 'shared route table'`
Expected: FAIL — `src/http/route-table.js` does not exist.

- [ ] **Step 3: Create the table**

Create `src/http/route-table.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The four route suffixes, in ONE place (SMA-626 § 3).
//
// THIS MODULE IMPORTS NOTHING, and that is load-bearing rather than incidental. `src/middleware.ts`
// is its own package entry point whose transitive import graph must reach no store, no resolver
// and no `openid-client` (AC 4, asserted by tests/middleware.test.ts). A leaf module with no edges
// of its own can be shared with `src/http/routes.ts` — which does reach all of those — without
// putting a single new edge into the middleware graph.
//
// WHY IT EXISTS. `authRoutePaths` and `createAuthRoutes`'s dispatch used to be two hand-written
// lists. A route missing from `publicPaths` means `/auth/login` clears the session cookie, the
// identity provider's redirect arrives without one, middleware bounces it back to `/auth/login`,
// and the user loops forever with no error anywhere. `routes.ts` builds a
// `Record<AuthRouteSuffix, …>` from this tuple, so a fifth suffix with no handler and a handler
// with no suffix BOTH fail typecheck.
//
// A third, ungated copy of this list lives in README.md's route table. It is prose; nothing binds
// it, and that is a stated residual rather than an oversight.

export const AUTH_ROUTE_SUFFIXES = ['/auth/login', '/auth/callback', '/auth/logout', '/auth/logout/callback'] as const;

export type AuthRouteSuffix = (typeof AUTH_ROUTE_SUFFIXES)[number];
```

- [ ] **Step 4: Make the dispatch table-driven**

In `src/http/routes.ts`, add the import:

```ts
import { AUTH_ROUTE_SUFFIXES, type AuthRouteSuffix } from './route-table.js';
```

Replace `createAuthRoutes` (`:43-84`) with:

```ts
interface RouteEntry {
  method: 'GET' | 'POST';
  run(runtime: AuthRuntime, req: Request, url: URL): Promise<Response>;
}

/**
 * The route table, keyed by the shared suffix tuple (SMA-626 § 3). `Record<AuthRouteSuffix, …>`
 * closes the drift in BOTH directions at typecheck time: a suffix added to
 * `AUTH_ROUTE_SUFFIXES` with no entry here is a missing key, and an entry here for a suffix not in
 * the tuple is an excess key. That is what binds `middleware.ts`'s `authRoutePaths` to what this
 * file actually serves, rather than to a second hand-written list that could drift from it.
 *
 * Logout is POST, not GET (design doc § 9.5): a GET route that mutates server-side state is
 * triggerable by an `<img src>` from any page on the internet. No CSRF token is added on top —
 * `SameSite=Lax` already withholds __Host-pgs_sid from a cross-site form POST, so a forged POST
 * arrives with no session and does nothing. Recorded here so a later reader does not "fix" it.
 */
const ROUTES: Record<AuthRouteSuffix, RouteEntry> = {
  '/auth/login': { method: 'GET', run: (runtime, req, url) => handleLogin(runtime, req, url) },
  '/auth/callback': { method: 'GET', run: (runtime, req, url) => handleCallback(runtime, req, url) },
  '/auth/logout': { method: 'POST', run: (runtime, req) => handleLogout(runtime, req) },
  '/auth/logout/callback': { method: 'GET', run: (runtime) => handleLogoutCallback(runtime) },
};

export function createAuthRoutes(runtime: AuthRuntime): AuthRoutes {
  // Built ONCE per runtime, and matched EXACTLY against this zone's own base path — an `endsWith`
  // test previously matched `/anything/auth/login` too, harmless only by accident (review round 1,
  // M5).
  const table = new Map<string, RouteEntry>(AUTH_ROUTE_SUFFIXES.map((suffix) => [`${runtime.basePath}${suffix}`, ROUTES[suffix]]));

  return {
    async handle(req: Request): Promise<Response> {
      const url = new URL(req.url);
      const entry = table.get(url.pathname);
      if (entry === undefined) return new Response(null, { status: 404 });
      if (req.method !== entry.method) return new Response(null, { status: 405 });
      return entry.run(runtime, req, url);
    },
  };
}
```

`handleLogoutCallback` already returns `Promise<Response>`, so no signature changes anywhere.

- [ ] **Step 5: Derive `authRoutePaths` from the table**

In `src/middleware.ts`, add the import and replace the function body (`:63-65`):

```ts
import { AUTH_ROUTE_SUFFIXES } from './http/route-table.js';
```

```ts
export function authRoutePaths(runtime: { basePath: string }): readonly string[] {
  return AUTH_ROUTE_SUFFIXES.map((suffix) => `${runtime.basePath}${suffix}`);
}
```

Update that function's doc comment: the list is no longer "a pure derivation from `basePath`
only" — it now derives from the shared table, which is what makes it correct rather than merely
duplicated. Keep the paragraph explaining the redirect loop; it is still the reason the function
exists. `route-table.ts` imports nothing, so the AC 4 guarantee is unchanged — say so.

- [ ] **Step 6: Prove the Record closes both directions**

Temporarily add a fifth entry to `AUTH_ROUTE_SUFFIXES`:

```ts
export const AUTH_ROUTE_SUFFIXES = ['/auth/login', '/auth/callback', '/auth/logout', '/auth/logout/callback', '/auth/probe'] as const;
```

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run paigasus-auth-ts:typecheck`
Expected: **FAIL** — `Property '/auth/probe' is missing in type ... Record<AuthRouteSuffix, RouteEntry>`.

Now revert the tuple and instead add a spurious entry to `ROUTES`:

```ts
  '/auth/probe': { method: 'GET', run: (runtime) => handleLogoutCallback(runtime) },
```

Run the same typecheck.
Expected: **FAIL** — object literal may only specify known properties.

Revert both probes. Record the two results in a comment above `ROUTES`:

```ts
 * MEASURED 2026-09-10: adding a fifth suffix with no entry fails typecheck ("Property
 * '/auth/probe' is missing"), and adding an entry with no suffix fails typecheck ("may only
 * specify known properties"). Both directions, at build time.
```

- [ ] **Step 7: Run everything**

```bash
pnpm exec vitest run
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run paigasus-auth-ts:typecheck && moon run paigasus-auth-ts:lint
```
Expected: green, including `tests/middleware.test.ts`'s existing 405/404 cross-check and the AC 4
import-graph assertion.

- [ ] **Step 8: Commit**

```bash
git add src/http/route-table.ts src/http/routes.ts src/middleware.ts tests/middleware.test.ts
git commit -m "fix(ts): bind authRoutePaths to the real route table"
```

---

### Task 9: The two Redis-adapter guards

**Files:**
- Test: `tests/adapters/redis-client-options.test.ts` (create)

**Interfaces:**
- Consumes: `createRedisSessionStore` from `src/adapters/redis-store.js`, and the `vi.mock('redis')`
  pattern Task 4 established.
- Produces: nothing. Test-only, no production change.

**Design note.** Guard 2 asserts SILENCE, not survival. "Emit `'error'`, assert no throw" against a
mock is vacuous — the test invokes `() => undefined`, which cannot throw. The property the code
claims is that the handler is intentionally silent BECAUSE the node-redis error object embeds the
DSN. Replacing `() => undefined` with `(e) => console.error(e)` would keep a survival-only test
green while leaking the DSN on every connection blip.

- [ ] **Step 1: Write the tests**

Create `tests/adapters/redis-client-options.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-626 § 5, guards 1 and 2. Both are commented LOAD-BEARING in redis-store.ts and neither would
// have red if deleted. These tests are the answer to "what would fail if this were deleted?".
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

interface CapturedClient {
  on: ReturnType<typeof vi.fn>;
  connect: () => Promise<void>;
  get: () => Promise<string | null>;
  set: () => Promise<string | null>;
  del: () => Promise<number>;
  eval: () => Promise<unknown>;
  close: () => Promise<void>;
}

const captured = { options: undefined as Record<string, unknown> | undefined, client: undefined as CapturedClient | undefined };

vi.mock('redis', () => ({
  createClient: (options: Record<string, unknown>) => {
    // The production RedisClient interface (redis-store.ts:104-110) deliberately omits `on`, so
    // this factory satisfies that interface PLUS the `on` the real node-redis client carries, and
    // is cast once here rather than widening the production type.
    const client: CapturedClient = {
      on: vi.fn(),
      connect: () => Promise.resolve(),
      get: () => Promise.resolve(null),
      set: () => Promise.resolve('OK'),
      del: () => Promise.resolve(1),
      eval: () => Promise.resolve(1),
      close: () => Promise.resolve(),
    };
    captured.options = options;
    captured.client = client;
    return client;
  },
}));

const { createRedisSessionStore } = await import('../../src/adapters/redis-store.js');

beforeEach(async () => {
  captured.options = undefined;
  captured.client = undefined;
  await createRedisSessionStore({ url: 'redis://user:pw@redis.internal:6379', commandTimeoutMs: 1234, keyPrefix: '' });
});

afterEach(() => {
  vi.restoreAllMocks();
});

// GUARD 1. node-redis QUEUES commands while disconnected by default, so an outage becomes hung
// requests instead of fast failures and every page in the console stalls rather than erroring.
// Deleting the flag reds this test.
//
// NAMED RESIDUAL: this proves the flag is SET. It does NOT prove node-redis then fails fast during
// a real outage — that needs a container test that stops Redis mid-suite, rejected as slow and
// flake-prone (spec § 5).
describe('guard 1: disableOfflineQueue', () => {
  it('is passed to createClient as true', () => {
    expect(captured.options?.['disableOfflineQueue']).toBe(true);
  });

  it('passes the command timeout and the reconnect strategy alongside it', () => {
    expect(captured.options?.['commandOptions']).toEqual({ timeout: 1234 });
    const socket = captured.options?.['socket'] as { connectTimeout: number; reconnectStrategy: unknown };
    expect(socket.connectTimeout).toBe(1234);
    expect(typeof socket.reconnectStrategy).toBe('function');
  });
});

// GUARD 2. node-redis emits 'error' on the client's EventEmitter for connection-level failures;
// with no listener that is an unhandled event that crashes the process.
//
// The listener must also be SILENT. node-redis's own connection errors embed the DSN, which is why
// this package never logs a caught node-redis error object. A test that only asserted "does not
// throw" would stay green if `() => undefined` became `(e) => console.error(e)`, which leaks the
// DSN on every connection blip and breaks the absolute redaction rule.
describe('guard 2: the silent error listener', () => {
  it('registers a listener for the error event', () => {
    expect(captured.client?.on).toHaveBeenCalledWith('error', expect.any(Function));
  });

  it('the handler neither throws nor reports the DSN anywhere', () => {
    const spies = {
      error: vi.spyOn(console, 'error').mockImplementation(() => undefined),
      warn: vi.spyOn(console, 'warn').mockImplementation(() => undefined),
      log: vi.spyOn(console, 'log').mockImplementation(() => undefined),
      info: vi.spyOn(console, 'info').mockImplementation(() => undefined),
      debug: vi.spyOn(console, 'debug').mockImplementation(() => undefined),
    };

    const handler = captured.client?.on.mock.calls.find(([event]) => event === 'error')?.[1] as (err: Error) => void;
    expect(handler).toBeTypeOf('function');

    expect(() => {
      handler(new Error('connect ECONNREFUSED redis://user:pw@redis.internal:6379'));
    }).not.toThrow();

    for (const spy of Object.values(spies)) expect(spy).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run them**

Run: `pnpm exec vitest run tests/adapters/redis-client-options.test.ts`
Expected: PASS.

- [ ] **Step 3: Prove both guards bite**

Delete `disableOfflineQueue: true,` from `src/adapters/redis-store.ts:241`.
Run the file. Expected: **FAIL** on guard 1's first test. Restore the line.

Delete `client.on('error', () => undefined);` from `:254`.
Run the file. Expected: **FAIL** on both guard-2 tests. Restore the line.

Replace `() => undefined` with `(e: unknown) => { console.error(e); }`.
Run the file. Expected: **FAIL** on "neither throws nor reports the DSN anywhere". Restore.

Restore every edit by reverting it directly. Do NOT use `git checkout --` — it would discard the
new test file.

- [ ] **Step 4: Record the measurements**

Add to the file header:

```
// MEASURED 2026-09-10. Deleting `disableOfflineQueue: true` reds guard 1. Deleting the
// `client.on('error', ...)` line reds both guard-2 tests. Replacing the silent handler with
// `(e) => console.error(e)` reds the DSN assertion while leaving the registration assertion green,
// which is exactly the case a survival-only test would have missed.
```

- [ ] **Step 5: Run everything and commit**

```bash
pnpm exec vitest run
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" && moon run paigasus-auth-ts:typecheck
git add tests/adapters/redis-client-options.test.ts
git commit -m "test(ts): make the Redis adapter's two load-bearing guards red when deleted"
```

---

### Task 10: `getAuthRuntime`'s failure reset

**Files:**
- Test: `tests/runtime.test.ts`

**Interfaces:**
- Consumes: `createAuthRuntime`'s `BASE` fixture already in the file.
- Produces: nothing. Test-only.

**Design note — the vacuous mode.** `vi.resetModules()` must run **ONCE, before both dynamic
imports**, so both calls reach the SAME fresh module instance. A reset between the two calls hands
the second call a module whose `sharedRuntime` is already `undefined`, and the test then passes
with `sharedRuntime = undefined` deleted from `src/runtime.ts:180` — proving nothing.

- [ ] **Step 1: Write the test**

Append to `tests/runtime.test.ts`:

```ts
// SMA-626 § 5, guard 3. getAuthRuntime's doc comment claims a misconfigured-at-boot process can
// recover once its config is fixed, and only the SUCCESS path was exercised — delete
// `sharedRuntime = undefined` from the catch and every existing test still passed.
//
// THE VACUOUS MODE THIS AVOIDS: vi.resetModules() runs exactly ONCE, before BOTH imports, so both
// calls reach the same fresh module instance. Resetting between them would give the second call a
// module whose sharedRuntime is already undefined, and the test would pass with the reset deleted.
describe('getAuthRuntime failure reset (SMA-626 § 5, guard 3)', () => {
  it('lets a later call succeed after the first one rejected', async () => {
    vi.resetModules(); // ONCE — see the block comment above.
    const mod = await import('../src/runtime.js');

    await expect(mod.getAuthRuntime({ ...BASE, PAIGASUS_ZONE: 'ghost' })).rejects.toMatchObject({
      message: 'PAIGASUS_ZONE has no entry in PAIGASUS_ZONES',
    });

    // The SAME module instance. Without the reset in the catch, this replays the rejection above.
    const recovered = await mod.getAuthRuntime(BASE);
    expect(recovered.zone).toBe(BASE.PAIGASUS_ZONE);
  });

  it('caches the recovered runtime, so the reset does not disable memoisation', async () => {
    vi.resetModules();
    const mod = await import('../src/runtime.js');

    await expect(mod.getAuthRuntime({ ...BASE, PAIGASUS_ZONE: 'ghost' })).rejects.toThrow();
    const first = await mod.getAuthRuntime(BASE);
    const second = await mod.getAuthRuntime({ ...BASE, PAIGASUS_ZONE: 'ghost' });

    expect(second).toBe(first);
  });
});
```

Ensure the file imports `vi` from `vitest`.

- [ ] **Step 2: Run it**

Run: `pnpm exec vitest run tests/runtime.test.ts`
Expected: PASS — including the pre-existing memoisation test at `:108`, which uses the
statically-imported module and is unaffected by `vi.resetModules()`.

- [ ] **Step 3: Prove the guard bites**

In `src/runtime.ts:180`, comment out `sharedRuntime = undefined;`.
Run: `pnpm exec vitest run tests/runtime.test.ts`
Expected: **FAIL** on both new tests — the second call replays the first rejection.

Restore the line by reverting the edit. Add to the describe block:

```ts
  // MEASURED 2026-09-10: commenting out `sharedRuntime = undefined` in runtime.ts's catch reds
  // both tests here and leaves every other test in this file green.
```

- [ ] **Step 4: Run everything and commit**

```bash
pnpm exec vitest run
git add tests/runtime.test.ts
git commit -m "test(ts): make getAuthRuntime's failure reset red when deleted"
```

---

### Task 11: Documentation and full verification

**Files:**
- Modify: `docs/superpowers/specs/2026-09-09-sma-506-auth-design.md:1027-1031`
- Modify: `ts/packages/paigasus-auth/README.md` (only if it documents the event set or
  `refreshPending`)

- [ ] **Step 1: Update the SMA-506 design doc's minimum event set**

Add `session.resolve_failed` to the enumerated list at `:1027-1031`, with a one-line gloss:

```
- `session.resolve_failed` — `getSession` could not resolve the session for a reason that is NOT a
  store outage (SMA-626 § 2.4). `store.unavailable` is reserved for the store itself.
```

- [ ] **Step 2: Check the README for stale claims**

Run: `grep -n 'refreshPending\|store.unavailable\|refresh_failed' ts/packages/paigasus-auth/README.md`

Update anything the change made false. If nothing matches, make no edit.

- [ ] **Step 3: Confirm no stale identifier survives**

```bash
cd ts/packages/paigasus-auth && grep -rn 'refreshPending' src tests
```
Expected: no output.

- [ ] **Step 4: Full package verification**

```bash
cd ts/packages/paigasus-auth
pnpm exec vitest run
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-auth-ts:lint
moon run paigasus-auth-ts:typecheck
moon run paigasus-auth-ts:fmt
```
Expected: all green. Record the final test count; it must EXCEED the 234 baseline, and the three
re-baselined assertions from Task 6 must be the only pre-existing tests whose text changed.

- [ ] **Step 5: Run the full graph the way CI does**

From the repository root, run the marker-delimited command in `CLAUDE.md` between
`<!-- ci-targets:begin -->` and `<!-- ci-targets:end -->`, with `--base origin/main`.

Docker and an installed Chromium are required (`CONTRIBUTING.md` has the provisioning step). If
`repo:affected-smoke` aborts in under 3 seconds, CAPTURE THE FULL TASK OUTPUT before re-running —
a passing re-run overwrites the evidence (CLAUDE.md's diagnosis procedure).

- [ ] **Step 6: Commit**

```bash
git add docs/ ts/packages/paigasus-auth/README.md
git commit -m "docs(ts): record session.resolve_failed in the SMA-506 event set"
```

---

## Self-review

**Spec coverage.**

| Spec section | Task |
|---|---|
| § 2.2 the DEFINITIVE set is `invalid_grant` alone | 2 |
| § 2.3 `RefreshRejected`, internal, typed literal | 2 |
| § 2.3 the classifier, MEASURED not assumed | 1, 2 |
| § 2.3 degrade on transient, `reason` + `degraded` | 6 |
| § 2.3 DELETE on a definitive rejection | 6 |
| § 2.3 cap re-check in BOTH branches | 6 |
| § 2.3 `refreshPending` -> `refreshState` | 6 |
| § 2.4 narrow `store.unavailable`, add `session.resolve_failed` | 7 |
| § 2.4 the `SessionStore` port contract, written down and tested | 7 |
| § 2.4 the ordering constraint against § 4 | Task 7's header, and Global Constraints |
| § 3.2 the shared table, table-driven dispatch, both typecheck directions | 8 |
| § 3.2 the zero-occurrence backstop | 8 |
| § 4.1 holes 1, 2 and 3 proven before the fix | 4 |
| § 4.2 the predicate, and the drift table test | 3 |
| § 4.2 both adapters adopt it | 4 |
| § 5 guard 1 `disableOfflineQueue` | 9 |
| § 5 guard 2 the silent listener, asserting SILENCE | 9 |
| § 5 guard 3 `getAuthRuntime`'s reset, with the vacuous mode named | 10 |
| § 5 guard 4 the CAS retry | 5 |
| § 5 mutation evidence recorded beside each test | 5, 8, 9, 10 |
| § 6 the design doc's event set | 11 |
| § 7 the three re-baselines | 6, Global Constraints |

No spec section is unassigned.

**Placeholder scan.** No "TBD", no "handle edge cases", no "similar to Task N". Every code step
carries the actual code.

**Type consistency.** `isSessionRecord` (Tasks 3, 4) — one name throughout. `RefreshRejected` with
`oauthError: 'invalid_grant'` (Tasks 2, 6, 7). `refreshState: 'pending' | 'failed'` (Task 6 only;
`refreshPending` appears nowhere after it, asserted in Task 11 Step 3). `AUTH_ROUTE_SUFFIXES` /
`AuthRouteSuffix` (Task 8). `setNextTokenError(error, wwwAuthenticate?)` (Tasks 1, 2).
`session.refresh_failed` carries `{ sid, reason, degraded }` in every task that asserts it.
