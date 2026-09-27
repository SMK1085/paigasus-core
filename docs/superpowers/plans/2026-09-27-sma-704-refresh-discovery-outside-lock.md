# SMA-704 Discovery Outside the Session Lock Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** OIDC discovery runs before `resolveSession` takes the session lock, so the IdP calls under the lock are at most 2T and the existing `2 * PAIGASUS_OIDC_HTTP_TIMEOUT_MS < PAIGASUS_SESSION_LOCK_TTL_MS` rule covers all of them.

**Architecture:** `OidcClient` gets `ensureDiscovered()`, which awaits the adapter's cached `getConfig()` and sends nothing else. `ResolveDeps` gets a required `prepareRefresh`, which `resolveSession` awaits before the first `tryAcquireLock` when a refresh is due and the record has a refresh token. A rejection there re-reads the record and goes through one shared transient helper that logs `stage: 'discovery'`; production wires `prepareRefresh` through a new `resolveDepsFor(runtime)` that the e2e fixture server also uses.

**Tech Stack:** TypeScript (strict, `exactOptionalPropertyTypes`, `verbatimModuleSyntax`), Node >= 24, vitest ^5, `openid-client@6.8.8`, `jose` (test fixture), Moon 2.5.3, pnpm, prettier (printWidth 200), eslint.

**Spec:** `docs/superpowers/specs/2026-09-27-sma-704-refresh-discovery-outside-lock-design.md`

## Global Constraints

- Worktree root: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-704`, branch `feature/sma-704-refresh-lock-ttl-discovery`. Run every command from the worktree root. Do not `cd` to the main checkout.
- Every shell starts with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Package: `ts/packages/paigasus-auth`. Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- Relative value imports in `src/` are EXTENSIONLESS (`'../core/single-flight'`). Test files keep their `.js` imports.
- New port method name: `ensureDiscovered(): Promise<void>` on `OidcClient`. It is `await getConfig();` and nothing else.
- New `ResolveDeps` member: `prepareRefresh: () => Promise<void>`, REQUIRED, not optional.
- New wiring function: `export function resolveDepsFor(runtime: AuthRuntime): ResolveDeps` in `src/next/get-session.ts`.
- New transient helper: `function transientRefreshFailure(logger: AuthLogger, sid: string, record: SessionRecord, err: unknown, detail: TransientRefreshDetail): ResolvedSession` in `src/core/single-flight.ts`, not exported.
- New log field: `stage: 'discovery'` on `session.refresh_failed`, typed `RefreshFailedStage = 'discovery'` in `src/ports/logger.ts`. The new site never sets `oauthError`.
- Runtime cache key prefix: `paigasus.auth.runtime.v2` (was `paigasus.auth.runtime.v1`).
- No default changes: `PAIGASUS_OIDC_HTTP_TIMEOUT_MS` stays `3500`, `PAIGASUS_SESSION_LOCK_TTL_MS` stays `10000`. The 2T check and its message (`2x PAIGASUS_OIDC_HTTP_TIMEOUT_MS must be strictly below PAIGASUS_SESSION_LOCK_TTL_MS`) do not change. `authEnvShape` and the Helm chart do not change.
- Test 13 values: T = 1000 ms; response delays discovery 600 ms (0.6T), token 300 ms (0.3T), JWKS 300 ms (0.3T). Changed from the spec's first values (0.9T/0.55T) at plan review: 0.9T left only 100 ms before the client timeout. The hanging case uses `startDiscoveryFailureFixture('hang')`.
- Test numbering follows the spec: 1-8 unit (single-flight), 9 wiring (get-session), 10-12 adapter, 13 measurement. Put `SMA-704 test N` in each test name. Review Focus pins are named `SMA-704 R1` … `SMA-704 R5`.
- A fake with no discovery passes `() => Promise.resolve()`, not `async () => {}` (an `async` function with no `await` can trip lint).
- Focused test run: `pnpm -C ts/packages/paigasus-auth exec vitest run <path> [-t "<regex>"]`. Never `pnpm --filter <pkg> test`: the package has no `test` script, so it is a silent no-op.
- Type check: `pnpm -C ts/packages/paigasus-auth exec tsc -p tsconfig.json --noEmit` (the same command as `moon run paigasus-auth-ts:typecheck`). vitest does NOT type-check, so run tsc in every task.
- Format: after each task, run `pnpm -C ts exec prettier --write <each changed file under ts/>`. `ts:fmt` is a separate whole-tree gate.
- Docker-backed suites (`tests/containers/**` and the Playwright tier) run only in `moon run paigasus-auth-ts:test-e2e`. It needs Docker and has no skip.
- Conventional commits with scope `ts`. Write the message to a file in your scratchpad directory with the Write tool, then `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-704 commit -F <file>`. The message ends with one blank line and then `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- No `#NNN` line and no `token: value` line in the commit BODY (commitlint `footer-leading-blank`). Write "SMA-704" inside a sentence.
- Never `git commit --amend`, never `git reset`, never `--no-verify`, never `git stash`. Never restore a mutation with `git checkout --`: it also discards the uncommitted work. Restore with the Edit tool.
- If a commit fails with "failed to fill whole buffer", 1Password is locked. Ask the user to unlock it.
- Do not install host software. Do not run Docker except for `paigasus-auth-ts:test-e2e` in Task 5.
- Documentation text and code comments are in ASD-STE100 Simplified Technical English: short sentences, active voice, one idea in one sentence.

## Review Focus

These five input classes follow from the spec, and no numbered spec test exercises them. Each one gets a pinning test in Task 3.

| # | Input class or failure mode | Why it can break | Pinning test (Task 3) |
|---|---|---|---|
| R1 | `prepareRefresh` rejects with an error that carries the `oidc_refresh_failed` code and an `oauthError` | A helper that calls `refreshFailureCode(err)` itself would add `oauthError` to the discovery line, which the spec forbids | third row of the `it.each` in tests 4/6 and 5/6, `…the RefreshFailed code (R1)` |
| R2 | The absolute cap passes during the discovery wait while the access token is still live | Without `Math.min` in the helper, the pre-lock site degrades a session past its own cap; a copy of the lock-site code would delete | `SMA-704 R2: the absolute cap passes during the discovery wait` |
| R3 | `prepareRefresh` RESOLVES, and another process refreshed the record during the wait | The lock loop must still double-check (invariant 1) and must not call `refresh` with a rotated token | `SMA-704 R3: prepareRefresh resolves after another writer refreshed the record` |
| R4 | A custom `prepareRefresh` that throws synchronously instead of rejecting | `prepareRefresh().catch(…)` would let the throw escape past the transient path | `SMA-704 R4: a prepareRefresh that throws synchronously` |
| R5 | Two concurrent requests on a cold process against the real adapter | They must share one discovery request (the adapter cache) and one token request (the lock) | `SMA-704 R5: two concurrent refreshes on a cold client` (guard, passes before the fix too) |

## Notes and choices

1. **Red-first for test 13.** Task 1 adds `ensureDiscovered` first. Task 2 writes test 13 against the pre-fix `resolveSession` with a `ResolveDeps`-shaped object that already carries `prepareRefresh: () => oidc.ensureDiscovered()`. It is returned from a helper function, so the object is not a fresh literal, and tsc accepts the extra member before `ResolveDeps` declares it. The pre-fix code ignores it. Task 2 runs the test as `it`, records the red and the lock hold time in spec § 7, and commits it as `it.fails`. Task 3 changes `it.fails` to `it` and makes no other change to the test. Reason: every commit stays green and passes the hooks, and the test text that went red is the text that goes green.
2. **Test 13 file.** The spec names no file. It is `ts/packages/paigasus-auth/tests/core/single-flight-discovery.test.ts`, in the default `test` task (`vitest.config.ts` includes `tests/**/*.test.ts` and excludes only `tests/containers/**` and `tests/e2e/**`).
3. **`resolveDepsFor` stays in `src/next/get-session.ts`.** The e2e fixture server already loads that module through `src/server.js`, so importing it there adds no new module to the plain-Node server.
4. **Wrappers delegate.** `tests/http/callback.test.ts`'s `countingOidc` and the inline client in `tests/http/route-handler.test.ts` wrap a real adapter, so their `ensureDiscovered` delegates to `inner.ensureDiscovered()`. The pure fakes resolve.
5. **Test 1 uses an order log.** The fake appends `prepareRefresh:resolved` after a 20 ms `setTimeout`, and the store appends `tryAcquireLock`. The order of the two entries is the resolution-time comparison of the spec, without clock granularity.
6. **`makeRecord` lives in `tests/store-contract.ts`**, not `tests/support/`.
7. **Timing margin in test 13.** Each delay leaves at least 400 ms before the 1000 ms client timeout. If test 13 still fails on `session.refreshed` because a call timed out, report it; do not re-run it away.

---

## File Structure

| File | Task | Change |
|---|---|---|
| `ts/packages/paigasus-auth/src/adapters/oidc.ts` | 1 | `OidcClient.ensureDiscovered`, its implementation, the "DISCOVERY IS LAZY" header paragraph |
| `ts/packages/paigasus-auth/tests/fixtures/jwks.ts` | 1, 2 | Task 1: `FixtureEndpoint`, `FixtureRequest`, `requests()` log. Task 2: `setResponseDelay` |
| `ts/packages/paigasus-auth/tests/adapters/oidc.test.ts` | 1 | tests 10, 11, 12 |
| `ts/packages/paigasus-auth/tests/support/store-failure.ts` | 1 | `fakeOidc().ensureDiscovered` resolves |
| `ts/packages/paigasus-auth/tests/http/logout.test.ts` | 1, 3 | Task 1: fake `ensureDiscovered`. Task 3: `prepareRefresh` in the `resolveSession` call at line 236 |
| `ts/packages/paigasus-auth/tests/next/get-session.test.ts` | 1, 3 | Task 1: `unusedOidc().ensureDiscovered` resolves. Task 3: test 9 |
| `ts/packages/paigasus-auth/tests/http/callback.test.ts` | 1 | `countingOidc` delegates `ensureDiscovered` |
| `ts/packages/paigasus-auth/tests/http/route-handler.test.ts` | 1 | inline wrapper delegates `ensureDiscovered` |
| `ts/packages/paigasus-auth/tests/core/single-flight-discovery.test.ts` | 2, 3 | NEW. Test 13 (a: measurement, b: hanging discovery). Task 3: `it.fails` → `it`, plus R5 |
| `docs/superpowers/specs/2026-09-27-sma-704-refresh-discovery-outside-lock-design.md` | 2, 3, 5 | § 7 measurements: before, after, mutation battery |
| `ts/packages/paigasus-auth/src/ports/logger.ts` | 3 | `RefreshFailedStage`, header paragraph |
| `ts/packages/paigasus-auth/src/core/single-flight.ts` | 3 | `prepareRefresh`, `transientRefreshFailure`, the new order, the re-read, invariant 5 comment |
| `ts/packages/paigasus-auth/src/next/get-session.ts` | 3 | `resolveDepsFor`; `getSession` calls it |
| `ts/packages/paigasus-auth/tests/e2e/fixture-server.ts` | 3 | calls `resolveDepsFor(runtime)` |
| `ts/packages/paigasus-auth/tests/fixtures/refresh-worker.ts` | 3 | `prepareRefresh` in its `resolveSession` literal |
| `ts/packages/paigasus-auth/tests/containers/single-flight-redis.test.ts` | 3 | `prepareRefresh` in its `deps` helper |
| `ts/packages/paigasus-auth/tests/core/single-flight.test.ts` | 3 | `deps` helper gets `prepareRefresh`; tests 1-8 and R1-R4 |
| `ts/packages/paigasus-auth/src/runtime.ts` | 4 | `RUNTIME_KEY_PREFIX` v2 and its comment; invariant 3 comment |
| `ts/packages/paigasus-auth/tests/runtime.test.ts` | 4 | key v2 in `clearSharedRuntime`; a v2-key test |
| `ts/packages/paigasus-auth/README.md` | 4 | the two table rows; the "Known limits" bullet |

---

### Task 1: `OidcClient.ensureDiscovered` (tests 10-12)

**Files:**
- Modify: `ts/packages/paigasus-auth/tests/fixtures/jwks.ts` (header 16-24; after line 32; interface 57-92; after line 127; handler 129-132; return object 244-269)
- Modify: `ts/packages/paigasus-auth/src/adapters/oidc.ts` (header 7-12; interface 91-99; returned object 349)
- Modify: `ts/packages/paigasus-auth/tests/support/store-failure.ts:93-123`
- Modify: `ts/packages/paigasus-auth/tests/http/logout.test.ts:73-106`
- Modify: `ts/packages/paigasus-auth/tests/next/get-session.test.ts:59-64`
- Modify: `ts/packages/paigasus-auth/tests/http/callback.test.ts:58-70`
- Modify: `ts/packages/paigasus-auth/tests/http/route-handler.test.ts:60-66`
- Test: `ts/packages/paigasus-auth/tests/adapters/oidc.test.ts` (new describe after line 240; new `it` at the end of the SMA-656 T9 describe, after line 435)

**Interfaces:**
- Consumes: `getConfig(): Promise<client.Configuration>` (inside `createOidcClient`), `startDiscoveryFailureFixture('status-503')`.
- Produces: `OidcClient.ensureDiscovered(): Promise<void>`; `export type FixtureEndpoint = 'discovery' | 'jwks' | 'token' | 'revoke'`; `export interface FixtureRequest { readonly endpoint: FixtureEndpoint; readonly at: number }`; `OidcFixture.requests(): readonly FixtureRequest[]`.

- [ ] **Step 1: Add the request log to the fixture IdP**

In `tests/fixtures/jwks.ts`, append to the header comment (after line 24, `// is unaffected.`):

```ts
//
// THE REQUEST LOG (SMA-704). `requests()` returns each discovery, JWKS, token and revocation
// request in arrival order, with `Date.now()` at arrival. tests/adapters/oidc.test.ts counts the
// discovery requests with it, and tests/core/single-flight-discovery.test.ts compares the arrival
// times with the time the session lock was held.
```

After line 32 (`const PRIMARY_KID = 'primary';`) add:

```ts

/** SMA-704. The four endpoints that the request log records. */
export type FixtureEndpoint = 'discovery' | 'jwks' | 'token' | 'revoke';

/** SMA-704. One logged request. `at` is `Date.now()` when the request arrived. */
export interface FixtureRequest {
  readonly endpoint: FixtureEndpoint;
  readonly at: number;
}

function endpointOf(method: string | undefined, pathname: string): FixtureEndpoint | undefined {
  if (method === 'GET' && pathname === '/.well-known/openid-configuration') return 'discovery';
  if (method === 'GET' && pathname === '/jwks') return 'jwks';
  if (method === 'POST' && pathname === '/token') return 'token';
  if (method === 'POST' && pathname === '/revoke') return 'revoke';
  return undefined;
}
```

In `interface OidcFixture`, before `close(): Promise<void>;` add:

```ts
  /** SMA-704. Each discovery, JWKS, token and revocation request, in arrival order. See the file header. */
  requests(): readonly FixtureRequest[];
```

After line 127 (`const tokenRequestBodies: URLSearchParams[] = [];`) add:

```ts
  const requestLog: FixtureRequest[] = [];
```

In the handler, after `const url = new URL(req.url ?? '/', 'http://placeholder');` add:

```ts
      const endpoint = endpointOf(req.method, url.pathname);
      if (endpoint !== undefined) requestLog.push({ endpoint, at: Date.now() });
```

In the returned object, before `close(): Promise<void> {` add:

```ts
    requests(): readonly FixtureRequest[] {
      return [...requestLog];
    },
```

- [ ] **Step 2: Write the failing tests 10, 11 and 12**

In `tests/adapters/oidc.test.ts`, after the closing `});` of `describe('createOidcClient — the rest of the surface', …)` (line 240), add:

```ts
// SMA-704. resolveSession calls ensureDiscovered (as `prepareRefresh`) before it takes the session
// lock. It must send the discovery request, and nothing else, so that `refresh` under the lock finds
// the cached configuration and sends no discovery request of its own.
describe('createOidcClient — ensureDiscovered (SMA-704)', () => {
  const count = (endpoint: FixtureEndpoint): number => fixture.requests().filter((r) => r.endpoint === endpoint).length;

  it('SMA-704 test 10: sends one discovery request, and a later refresh sends no second one', async () => {
    const oidc = makeClient();

    await oidc.ensureDiscovered();
    // A no-op ensureDiscovered fails here.
    expect(count('discovery')).toBe(1);
    expect(count('token')).toBe(0); // it sends no token request

    await oidc.refresh('some-refresh-token');
    expect(count('discovery')).toBe(1);
    expect(count('token')).toBe(1);
  });

  it('SMA-704 test 11: two concurrent calls on a cold client send one discovery request', async () => {
    const oidc = makeClient();

    await Promise.all([oidc.ensureDiscovered(), oidc.ensureDiscovered()]);

    expect(count('discovery')).toBe(1);
  });
});
```

Change the import on line 15 to:

```ts
import { startOidcFixture, type FixtureEndpoint, type OidcFixture } from '../fixtures/jwks.js';
```

At the end of `describe('createOidcClient — a discovery failure is an OidcDiscoveryFailed with a closed reason (SMA-656 T9)', …)`, after the `it('the next call after a failed discovery runs discovery again', …)` block (it ends at line 435), add:

```ts

  // SMA-704 test 12. ensureDiscovered throws the same OidcDiscoveryFailed as getConfig, and a failed
  // discovery is not cached, so the next call sends a second discovery request.
  it('SMA-704 test 12: a failed ensureDiscovered rejects with OidcDiscoveryFailed, and a second call retries', async () => {
    failing = await startDiscoveryFailureFixture('status-503');
    const oidc = clientFor(failing.issuer);

    expectDiscoveryFailed(await oidc.ensureDiscovered().catch((e: unknown) => e), 'http_server_error');
    expectDiscoveryFailed(await oidc.ensureDiscovered().catch((e: unknown) => e), 'http_server_error');

    expect(failing.requests).toBe(2);
  });
```

- [ ] **Step 3: Run the new tests and see them fail**

Run: `pnpm -C ts/packages/paigasus-auth exec vitest run tests/adapters/oidc.test.ts -t "SMA-704 test 1[012]"`

Expected: FAIL. All three tests fail with `TypeError: oidc.ensureDiscovered is not a function`.

- [ ] **Step 4: Add the port method and the adapter implementation**

In `src/adapters/oidc.ts`, replace the header paragraph at lines 7-12 (from `// DISCOVERY IS LAZY.` to `// cleared, so the NEXT call retries rather than replaying the same rejection forever.`) with:

```ts
// DISCOVERY IS LAZY. `createOidcClient` performs no I/O; the first call to any method below
// triggers `client.discovery(...)` once and caches the resulting `Configuration` for every later
// call. This is what lets `createAuthRuntime` validate configuration and fail fast on a bad
// cross-field rule (§ runtime.ts) WITHOUT making a network call — discovery only happens when a
// login, refresh, or logout actually occurs. On a discovery failure the cached promise is
// cleared, so the NEXT call retries rather than replaying the same rejection forever.
//
// `ensureDiscovered` (SMA-704) runs that discovery and sends no other request.
// core/single-flight.ts's resolveSession calls it, as `prepareRefresh`, BEFORE it takes the
// session lock. After it resolves, `refresh` finds a resolved `configPromise` and sends no
// discovery request under the lock. Without it, the first refresh of a cold process sends a third
// IdP request under the lock, and runtime.ts invariant 3 counts only two.
```

In `interface OidcClient` (lines 91-99), add as the first member:

```ts
  /**
   * Runs OIDC discovery if this process has not completed it, and waits for it. Sends no token
   * request. resolveSession calls it before it takes the session lock (SMA-704).
   */
  ensureDiscovered(): Promise<void>;
```

In the object that `createOidcClient` returns (line 349, `return {`), add as the first member, before `async buildAuthorizationUrl(params)`:

```ts
    async ensureDiscovered(): Promise<void> {
      // The same OidcDiscoveryFailed as every other method, from getConfig() only (SMA-656 D10).
      await getConfig();
    },

```

- [ ] **Step 5: Give every `OidcClient` fake and wrapper an `ensureDiscovered`**

`tests/support/store-failure.ts`, in `fakeOidc()`'s object literal, before `buildAuthorizationUrl:` add:

```ts
    ensureDiscovered: (): Promise<void> => Promise.resolve(),
```

`tests/http/logout.test.ts`: replace the doc comment at lines 73-77 with:

```ts
/**
 * A minimal fake OidcClient. `revoke` and `buildEndSessionUrl` are the only two methods logout
 * ever reaches; three others throw if called, which would fail any test that mistakenly
 * exercises the login/callback/refresh paths through this fake. `ensureDiscovered` resolves: a
 * fake with no discovery (SMA-704).
 */
```

and in the returned object, before `buildAuthorizationUrl(): Promise<AuthorizationRequest> {` add:

```ts
    ensureDiscovered(): Promise<void> {
      return Promise.resolve();
    },
```

`tests/next/get-session.test.ts`: replace lines 59-64 with:

```ts
/** Every method but `ensureDiscovered` throws: getSession never needs to call the OIDC client or
 * the resolver in this suite (the test records are never near their skew window), so a call here
 * is a defect. `ensureDiscovered` resolves, so a test that overrides `refresh` keeps its path
 * (SMA-704). */
function unusedOidc(): AuthRuntime['oidc'] {
  const fail = () => Promise.reject(new Error('unexpectedly called'));
  return { ensureDiscovered: () => Promise.resolve(), buildAuthorizationUrl: fail, authorizationCodeGrant: fail, refresh: fail, revoke: fail, buildEndSessionUrl: fail };
}
```

`tests/http/callback.test.ts`, in `countingOidc`'s object, before `buildAuthorizationUrl:` add:

```ts
    ensureDiscovered: () => inner.ensureDiscovered(),
```

`tests/http/route-handler.test.ts`, in the inline `oidc: {` object (line 60), before `buildAuthorizationUrl:` add:

```ts
      ensureDiscovered: () => inner.ensureDiscovered(),
```

- [ ] **Step 6: Run the tests and the type check**

Run: `pnpm -C ts/packages/paigasus-auth exec vitest run tests/adapters/oidc.test.ts tests/http tests/next`
Expected: PASS, with tests 10, 11 and 12 among the passes.

Run: `pnpm -C ts/packages/paigasus-auth exec tsc -p tsconfig.json --noEmit`
Expected: exit 0, no output. If tsc names another object that lacks `ensureDiscovered`, give it one by the rule of Step 5 (a fake resolves, a wrapper delegates) and list it in the commit body.

Run: `pnpm -C ts exec prettier --write packages/paigasus-auth/src/adapters/oidc.ts packages/paigasus-auth/tests/fixtures/jwks.ts packages/paigasus-auth/tests/adapters/oidc.test.ts packages/paigasus-auth/tests/support/store-failure.ts packages/paigasus-auth/tests/http/logout.test.ts packages/paigasus-auth/tests/next/get-session.test.ts packages/paigasus-auth/tests/http/callback.test.ts packages/paigasus-auth/tests/http/route-handler.test.ts`

- [ ] **Step 7: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-704 add ts/packages/paigasus-auth/src/adapters/oidc.ts ts/packages/paigasus-auth/tests/fixtures/jwks.ts ts/packages/paigasus-auth/tests/adapters/oidc.test.ts ts/packages/paigasus-auth/tests/support/store-failure.ts ts/packages/paigasus-auth/tests/http/logout.test.ts ts/packages/paigasus-auth/tests/next/get-session.test.ts ts/packages/paigasus-auth/tests/http/callback.test.ts ts/packages/paigasus-auth/tests/http/route-handler.test.ts
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-704 commit -F <scratchpad>/msg-task1.txt
```

Message file content:

```text
feat(ts): add OidcClient.ensureDiscovered for SMA-704

ensureDiscovered awaits the cached discovery configuration and sends no
other request. A later refresh then sends no discovery request. The
fixture IdP now logs each request with its arrival time. Every OidcClient
fake resolves the new method, and every wrapper delegates it.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

### Task 2: Test 13 measured red on the pre-fix code

**Files:**
- Modify: `ts/packages/paigasus-auth/tests/fixtures/jwks.ts` (interface `OidcFixture`; after the `requestLog` line; the handler lines from Task 1 Step 1; the returned object)
- Create: `ts/packages/paigasus-auth/tests/core/single-flight-discovery.test.ts`
- Modify: `docs/superpowers/specs/2026-09-27-sma-704-refresh-discovery-outside-lock-design.md` (§ 7, lines 341-344)

**Interfaces:**
- Consumes: `createOidcClient`, `OidcClient.ensureDiscovered` (Task 1), `OidcFixture.requests()` (Task 1), `startDiscoveryFailureFixture('hang')`, `resolveSession(deps, sid)` (pre-fix signature), `makeRecord`.
- Produces: `OidcFixture.setResponseDelay(endpoint: FixtureEndpoint, ms: number): void`.

- [ ] **Step 1: Add the per-endpoint response delay to the fixture IdP**

In `interface OidcFixture`, after the `requests()` member, add:

```ts
  /**
   * SMA-704. Waits `ms` before it answers each later request to `endpoint`. The arrival time in
   * `requests()` is taken before the wait. Not one-shot: each test starts a fresh fixture.
   */
  setResponseDelay(endpoint: FixtureEndpoint, ms: number): void;
```

After `const requestLog: FixtureRequest[] = [];` add:

```ts
  const responseDelays = new Map<FixtureEndpoint, number>();
```

Replace the two handler lines from Task 1 Step 1:

```ts
      const endpoint = endpointOf(req.method, url.pathname);
      if (endpoint !== undefined) requestLog.push({ endpoint, at: Date.now() });
```

with:

```ts
      const endpoint = endpointOf(req.method, url.pathname);
      if (endpoint !== undefined) {
        requestLog.push({ endpoint, at: Date.now() });
        const delayMs = responseDelays.get(endpoint) ?? 0;
        if (delayMs > 0) await new Promise((resolve) => setTimeout(resolve, delayMs));
      }
```

In the returned object, after `requests()`, add:

```ts
    setResponseDelay(endpoint: FixtureEndpoint, ms: number) {
      responseDelays.set(endpoint, ms);
    },
```

- [ ] **Step 2: Write test 13 as `it`**

Create `tests/core/single-flight-discovery.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-704 test 13, the measurement (spec § 5.4). A REAL createOidcClient on a COLD client, driven
// through resolveSession, against the fixture IdP (tests/fixtures/jwks.ts). The fixture logs the
// arrival time of each discovery, token and JWKS request. A store wrapper logs when the session
// lock was taken and released.
//
// The pass or fail assertions are CAUSAL, not durations: which requests arrived while the lock was
// held. The delays (discovery 0.6T, token and JWKS 0.3T each, T = 1000 ms) only make the
// durations readable. The durations are printed, not asserted; the spec's § 7 records them.
//
// `depsFor` returns its object from a function, so tsc does not check it for excess members. That
// is how this file compiled against the code BEFORE the fix, when ResolveDeps had no
// `prepareRefresh` and resolveSession ignored it (plan Task 2).
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { MemorySessionStore } from '../../src/adapters/memory-store.js';
import { createOidcClient, type OidcClient } from '../../src/adapters/oidc.js';
import { isOidcDiscoveryFailed } from '../../src/core/errors.js';
import { resolveSession } from '../../src/core/single-flight.js';
import type { AuthEventFields, AuthEventName, AuthLogger } from '../../src/ports/logger.js';
import { sidTag } from '../../src/ports/logger.js';
import type { SessionStore } from '../../src/ports/session-store.js';
import { startDiscoveryFailureFixture, type DiscoveryFailureFixture } from '../fixtures/discovery-failures.js';
import { FIXTURE_CLIENT_ID, FIXTURE_CLIENT_SECRET, startOidcFixture, type FixtureEndpoint, type OidcFixture } from '../fixtures/jwks.js';
import { makeRecord } from '../store-contract.js';

const T = 1000;
const DISCOVERY_DELAY_MS = 0.6 * T;
const TOKEN_DELAY_MS = 0.3 * T;
const JWKS_DELAY_MS = 0.3 * T;

/** When the lock was taken and released, and how often tryAcquireLock was called. */
interface LockWindow {
  acquiredAt: number | undefined;
  releasedAt: number | undefined;
  attempts: number;
}

function newLockWindow(): LockWindow {
  return { acquiredAt: undefined, releasedAt: undefined, attempts: 0 };
}

/** Wraps a real store. Records the lock window in `window`. */
function lockTimingStore(inner: MemorySessionStore, window: LockWindow): SessionStore {
  return {
    get: (sid) => inner.get(sid),
    set: (sid, rec, ttlMs, expectedRev) => inner.set(sid, rec, ttlMs, expectedRev),
    delete: (sid) => inner.delete(sid),
    tryAcquireLock: async (sid, token, ttlMs) => {
      window.attempts += 1;
      const won = await inner.tryAcquireLock(sid, token, ttlMs);
      if (won) window.acquiredAt = Date.now();
      return won;
    },
    releaseLock: (sid, token) => {
      window.releasedAt = Date.now();
      return inner.releaseLock(sid, token);
    },
    putTransaction: (txnId, tx, ttlMs) => inner.putTransaction(txnId, tx, ttlMs),
    takeTransaction: (txnId) => inner.takeTransaction(txnId),
    close: () => inner.close(),
  };
}

function recordingLogger(): { logger: AuthLogger; events: Array<[AuthEventName, AuthEventFields]> } {
  const events: Array<[AuthEventName, AuthEventFields]> = [];
  return { logger: { event: (name, fields) => void events.push([name, { ...fields }]) }, events };
}

/** A new, cold client: it has sent no discovery request yet. */
function coldClient(issuer: string, httpTimeoutMs: number): OidcClient {
  return createOidcClient({
    issuer,
    clientId: FIXTURE_CLIENT_ID,
    clientSecret: FIXTURE_CLIENT_SECRET,
    httpTimeoutMs,
    clockToleranceSeconds: 30,
    scopes: 'openid profile email offline_access',
    allowInsecureRequests: true, // the fixtures are plain http on localhost — never set in production
  });
}

/** The production wiring's shape (next/get-session.ts resolveDepsFor): all three IdP calls go to ONE client. */
function depsFor(store: SessionStore, oidc: OidcClient, logger: AuthLogger) {
  return {
    store,
    prepareRefresh: () => oidc.ensureDiscovered(),
    refresh: (refreshToken: string) => oidc.refresh(refreshToken),
    revoke: (token: string) => oidc.revoke(token),
    logger,
    skewMs: 30_000,
    lockTtlMs: 5_000,
    lockWaitMs: 3_000,
    ttlMs: 60_000,
  };
}

describe('OIDC discovery and the session lock (SMA-704 test 13)', () => {
  let fixture: OidcFixture;
  let hanging: DiscoveryFailureFixture | undefined;

  beforeEach(async () => {
    fixture = await startOidcFixture();
  });

  afterEach(async () => {
    await fixture.close();
    await hanging?.close();
    hanging = undefined;
  });

  it(
    'SMA-704 test 13a: no discovery request reaches the IdP while the lock is held',
    async () => {
      fixture.setResponseDelay('discovery', DISCOVERY_DELAY_MS);
      fixture.setResponseDelay('token', TOKEN_DELAY_MS);
      fixture.setResponseDelay('jwks', JWKS_DELAY_MS);
      // The refresh response carries an ID token, so the non-repudiation hook fetches JWKS. Its `iss`
      // and `sub` equal the record's login claims, so the D5 id_token_mismatch branch does not run.
      fixture.setNextIdToken(await fixture.mintIdToken());
      const inner = new MemorySessionStore();
      await inner.set('s', makeRecord({ accessExpiresAt: Date.now() - 1, idTokenClaims: { iss: fixture.issuer, sub: 'user-1' } }), 60_000, null);
      const window = newLockWindow();
      const { logger, events } = recordingLogger();

      const startedAt = Date.now();
      await resolveSession(depsFor(lockTimingStore(inner, window), coldClient(fixture.issuer, T), logger), 's');
      const totalMs = Date.now() - startedAt;

      const { acquiredAt, releasedAt } = window;
      if (acquiredAt === undefined || releasedAt === undefined) throw new Error('the lock was never taken and released');
      const log = fixture.requests();
      const underLock = (endpoint: FixtureEndpoint): number => log.filter((r) => r.endpoint === endpoint && r.at >= acquiredAt && r.at <= releasedAt).length;
      const beforeLock = (endpoint: FixtureEndpoint): number => log.filter((r) => r.endpoint === endpoint && r.at < acquiredAt).length;

      // Printed BEFORE the assertions, so a red run shows the numbers too. The spec's § 7 records them.
      console.info(
        `SMA-704 test 13a: lock hold ${String(releasedAt - acquiredAt)} ms; resolveSession ${String(totalMs)} ms; ` +
          `under the lock: discovery ${String(underLock('discovery'))}, token ${String(underLock('token'))}, jwks ${String(underLock('jwks'))}; ` +
          `before the lock: discovery ${String(beforeLock('discovery'))}`,
      );

      expect(underLock('discovery'), 'discovery requests while the lock is held').toBe(0);
      expect(underLock('token'), 'token requests while the lock is held').toBe(1);
      expect(underLock('jwks'), 'JWKS requests while the lock is held').toBe(1);
      expect(beforeLock('discovery'), 'discovery requests before the lock is taken').toBe(1);
      expect(events).toContainEqual(['session.refreshed', { sid: sidTag('s'), rev: 1, idTokenRotated: true }]);
    },
    15_000,
  );

  // The worst case with every call AT its timeout is a failed refresh, so this case covers the
  // timeout. Its own client uses T = 200 ms, to keep the run short.
  it('SMA-704 test 13b: a hanging discovery never reaches tryAcquireLock', async () => {
    hanging = await startDiscoveryFailureFixture('hang');
    const inner = new MemorySessionStore();
    await inner.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const window = newLockWindow();
    const { logger } = recordingLogger();

    const err: unknown = await resolveSession(depsFor(lockTimingStore(inner, window), coldClient(hanging.issuer, 200), logger), 's').catch((e: unknown) => e);

    console.info(`SMA-704 test 13b: tryAcquireLock calls ${String(window.attempts)}`);
    expect(window.attempts, 'tryAcquireLock calls').toBe(0);
    expect(isOidcDiscoveryFailed(err)).toBe(true);
    expect(hanging.requests).toBe(1);
    expect(await inner.get('s')).not.toBeNull();
  });
});
```

- [ ] **Step 3: Run test 13 against the pre-fix code and see it fail**

Run: `pnpm -C ts/packages/paigasus-auth exec vitest run tests/core/single-flight-discovery.test.ts`

Expected: FAIL, 2 failed.
- 13a fails on its FIRST assertion: `discovery requests while the lock is held: expected 1 to be +0`. If it fails on any other line, stop: the measurement is not valid. Report it.
- 13b fails with `tryAcquireLock calls: expected 1 to be +0`.

Copy the two `SMA-704 test 13a:` and `SMA-704 test 13b:` console lines. Also run `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-704 rev-parse --short HEAD` and keep the value (the pre-fix commit).

- [ ] **Step 4: Record the red run in spec § 7**

Replace spec lines 343-344 (`To be filled in by the implementation: test 13 before the fix (lock hold time, discovery requests` / `under the lock) and after the fix.`) with the text below. Put the numbers from Step 3 into the "Before the fix" row. Each cell holds a measured value; none is left empty.

```markdown
Test 13 (`ts/packages/paigasus-auth/tests/core/single-flight-discovery.test.ts`). 13a uses T = 1000 ms
and the response delays discovery 600 ms, token 300 ms, JWKS 300 ms. 13b uses a hanging discovery and
T = 200 ms. Measured on the development Mac with `vitest run` on that one file.

| Run | Commit | 13a: discovery under the lock | 13a: token / JWKS under the lock | 13a: discovery before the lock | 13a: lock hold (ms) | 13a: `resolveSession` (ms) | 13b: `tryAcquireLock` calls |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Before the fix | `<the short SHA from Step 3>` | <measured> | <measured> / <measured> | <measured> | <measured> | <measured> | <measured> |

Before the fix, 13a failed on its first assertion (a discovery request while the lock was held), as
§ 5.4 requires.
```

- [ ] **Step 5: Mark both tests `it.fails` for the commit**

In `tests/core/single-flight-discovery.test.ts`, change the `it(` call of test 13a and the `it(` call of test 13b to `it.fails(`. Change nothing else in them. Directly above the `describe(` line add:

```ts
// RED-FIRST (SMA-704 plan, Task 2). Both cases are `it.fails` in the commit that measured them on
// the code before the fix: `it.fails` passes only while its test fails. The fix commit (Task 3)
// changes them to `it` and changes nothing else in them.
```

Run: `pnpm -C ts/packages/paigasus-auth exec vitest run tests/core/single-flight-discovery.test.ts`
Expected: PASS, 2 passed (both expected failures).

Run: `pnpm -C ts/packages/paigasus-auth exec tsc -p tsconfig.json --noEmit`
Expected: exit 0.

Run: `pnpm -C ts exec prettier --write packages/paigasus-auth/tests/fixtures/jwks.ts packages/paigasus-auth/tests/core/single-flight-discovery.test.ts`

- [ ] **Step 6: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-704 add ts/packages/paigasus-auth/tests/fixtures/jwks.ts ts/packages/paigasus-auth/tests/core/single-flight-discovery.test.ts docs/superpowers/specs/2026-09-27-sma-704-refresh-discovery-outside-lock-design.md
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-704 commit -F <scratchpad>/msg-task2.txt
```

Message file content:

```text
test(ts): measure OIDC discovery under the session lock for SMA-704

Test 13 drives a cold createOidcClient through resolveSession against the
fixture IdP, which now takes a per-endpoint response delay. On the code
before the fix, a discovery request reached the IdP while the lock was
held. The spec records that run and its lock hold time. Both cases are
it.fails until the fix lands.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

### Task 3: `prepareRefresh` before the lock, and its production wiring (tests 1-9, R1-R5)

**Files:**
- Modify: `ts/packages/paigasus-auth/src/ports/logger.ts` (header 14-16; after line 49)
- Modify: `ts/packages/paigasus-auth/src/core/single-flight.ts` (imports 2 and 6; `ResolveDeps` 23-38; after `revokeAll`, line 70; invariant 5 comment 103-108; `resolveSession` 128-141; the refresh catch 177-225)
- Modify: `ts/packages/paigasus-auth/src/next/get-session.ts` (import 36; new function before `getSession`, line 42; lines 52-65)
- Modify: `ts/packages/paigasus-auth/tests/e2e/fixture-server.ts` (header 22-27; imports 30-33; lines 151-162)
- Modify: `ts/packages/paigasus-auth/tests/fixtures/refresh-worker.ts:93`
- Modify: `ts/packages/paigasus-auth/tests/containers/single-flight-redis.test.ts:30-32`
- Modify: `ts/packages/paigasus-auth/tests/http/logout.test.ts:236-248`
- Test: `ts/packages/paigasus-auth/tests/core/single-flight.test.ts` (`deps` 24-28; new describe at the end of the file, after line 1061)
- Test: `ts/packages/paigasus-auth/tests/next/get-session.test.ts` (new helper after line 57; new `it` at the end of `describe('getSession', …)`, after line 188)
- Test: `ts/packages/paigasus-auth/tests/core/single-flight-discovery.test.ts` (`it.fails` → `it`; R5)
- Modify: `docs/superpowers/specs/2026-09-27-sma-704-refresh-discovery-outside-lock-design.md` (§ 7 table)

**Interfaces:**
- Consumes: `OidcClient.ensureDiscovered(): Promise<void>` (Task 1); `OidcFixture.setResponseDelay` (Task 2); `isRefreshRejected`, `refreshFailureCode`, `type TokenErrorCode` from `core/errors`.
- Produces: `ResolveDeps.prepareRefresh: () => Promise<void>` (required); `export type RefreshFailedStage = 'discovery'`; `function transientRefreshFailure(logger: AuthLogger, sid: string, record: SessionRecord, err: unknown, detail: TransientRefreshDetail): ResolvedSession`; `interface TransientRefreshDetail { readonly oauthError?: TokenErrorCode | 'other'; readonly stage?: RefreshFailedStage }`; `export function resolveDepsFor(runtime: AuthRuntime): ResolveDeps`.

- [ ] **Step 1: Give the unit-test `deps` helper a `prepareRefresh`**

In `tests/core/single-flight.test.ts`, replace lines 24-28 with:

```ts
function deps(store: SessionStore, refresh: ResolveDeps['refresh']) {
  // `revoke` is called only for a refresh token that no record holds (SMA-681). A test that checks
  // it passes its own recording function over this one. `prepareRefresh` (SMA-704) is a fake with
  // no discovery; a test that checks it passes its own.
  return { store, prepareRefresh: () => Promise.resolve(), refresh, revoke: () => Promise.resolve(), logger: noopLogger, skewMs: 30_000, lockTtlMs: 5_000, lockWaitMs: 3_000, ttlMs: 60_000 };
}
```

- [ ] **Step 2: Write the failing unit tests 1-8 and R1-R4**

Append to the end of `tests/core/single-flight.test.ts`:

```ts

// ---------------------------------------------------------------------------------------------
// SMA-704. resolveSession calls `prepareRefresh` (production: OidcClient.ensureDiscovered) BEFORE it
// takes the lock, so the OIDC discovery request never runs under the lock. The test numbers are the
// spec's (§ 5.1); the § 5.5 mutation battery refers to them. R1-R4 are the plan's Review Focus.
// ---------------------------------------------------------------------------------------------
describe('prepareRefresh runs before the lock (SMA-704)', () => {
  /** Wraps a real store. Appends `tryAcquireLock` and `delete` to `order`, in call order. */
  function orderStore(inner: MemorySessionStore, order: string[]): SessionStore {
    return {
      get: (sid) => inner.get(sid),
      set: (sid, rec, ttlMs, expectedRev) => inner.set(sid, rec, ttlMs, expectedRev),
      delete: (sid) => {
        order.push('delete');
        return inner.delete(sid);
      },
      tryAcquireLock: (sid, token, ttlMs) => {
        order.push('tryAcquireLock');
        return inner.tryAcquireLock(sid, token, ttlMs);
      },
      releaseLock: (sid, token) => inner.releaseLock(sid, token),
      putTransaction: (txnId, tx, ttlMs) => inner.putTransaction(txnId, tx, ttlMs),
      takeTransaction: (txnId) => inner.takeTransaction(txnId),
      close: () => inner.close(),
    };
  }

  /** Resolves after a macrotask, so a missing `await` lets tryAcquireLock run first. */
  function recordingPrepare(order: string[]): ResolveDeps['prepareRefresh'] {
    return async () => {
      order.push('prepareRefresh:called');
      await new Promise((r) => setTimeout(r, 20));
      order.push('prepareRefresh:resolved');
    };
  }

  const refreshed = () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300 });
  const mustNotRefresh = () => Promise.reject(new Error('must not refresh'));

  // Test 6: each failure row runs with a plain Error AND with errors that carry this package's
  // refresh codes. The site before the lock must not classify: a RefreshRejected must not delete,
  // and a RefreshFailed must not add `oauthError` (R1).
  const discoveryFailures = [
    ['a plain Error', () => new Error('oidc discovery failed: TypeError')],
    ['an error with the RefreshRejected code', () => new RefreshRejected('invalid_grant')],
    ['an error with the RefreshFailed code (R1)', () => new RefreshFailed('invalid_scope', 'ResponseBodyError')],
  ] as const;

  it('SMA-704 test 1: prepareRefresh resolves BEFORE the first tryAcquireLock', async () => {
    const inner = new MemorySessionStore();
    await inner.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const order: string[] = [];

    const out = await resolveSession({ ...deps(orderStore(inner, order), refreshed), prepareRefresh: recordingPrepare(order) }, 's');

    expect(out?.accessToken).toBe('AT2');
    // Called once, resolved, and only then the lock. A missing `await` puts tryAcquireLock second.
    expect(order).toEqual(['prepareRefresh:called', 'prepareRefresh:resolved', 'tryAcquireLock']);
  });

  it('SMA-704 test 2: a read that needs no refresh does not call prepareRefresh', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() + 600_000 }), 60_000, null);
    const prepareRefresh = vi.fn(() => Promise.resolve());

    const out = await resolveSession({ ...deps(store, mustNotRefresh), prepareRefresh }, 's');

    expect(out?.accessToken).toBe('AT');
    expect(prepareRefresh).not.toHaveBeenCalled();
  });

  it('SMA-704 test 3: a record with no refresh token skips prepareRefresh and is deleted under the lock', async () => {
    const inner = new MemorySessionStore();
    const rec = makeRecord({ accessExpiresAt: Date.now() - 1 });
    delete rec.refreshToken;
    await inner.set('s', rec, 60_000, null);
    const order: string[] = [];
    const { logger, events } = recordingLogger();
    const prepareRefresh = vi.fn(() => Promise.resolve());

    expect(await resolveSession({ ...deps(orderStore(inner, order), mustNotRefresh), logger, prepareRefresh }, 's')).toBeNull();

    expect(prepareRefresh).not.toHaveBeenCalled();
    expect(order).toEqual(['tryAcquireLock', 'delete']);
    expect(events).toContainEqual(['session.deleted', { sid: sidTag('s'), reason: 'no_refresh_token' }]);
  });

  it.each(discoveryFailures)('SMA-704 tests 4 and 6: prepareRefresh rejects with %s and the token is live: degrade, no lock, no delete', async (_label, makeError) => {
    const inner = new MemorySessionStore();
    // Inside deps()'s 30 s skew window, and still live.
    await inner.set('s', makeRecord({ accessExpiresAt: Date.now() + 15_000 }), 60_000, null);
    const order: string[] = [];
    const { logger, events } = recordingLogger();

    const out = await resolveSession({ ...deps(orderStore(inner, order), mustNotRefresh), logger, prepareRefresh: () => Promise.reject(makeError()) }, 's');

    expect(out?.accessToken).toBe('AT');
    expect(out?.refreshState).toBe('failed');
    // Exactly these fields: no `oauthError` on the discovery line (R1).
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'transient', degraded: true, stage: 'discovery' }]);
    expect(order).not.toContain('tryAcquireLock');
    expect(order).not.toContain('delete');
    expect(events.some(([name]) => name === 'session.deleted')).toBe(false);
    expect(await inner.get('s')).not.toBeNull();
  });

  it.each(discoveryFailures)('SMA-704 tests 5 and 6: prepareRefresh rejects with %s and the token is expired: rethrow, no lock, no delete', async (_label, makeError) => {
    const inner = new MemorySessionStore();
    await inner.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const order: string[] = [];
    const { logger, events } = recordingLogger();
    const error = makeError();

    await expect(resolveSession({ ...deps(orderStore(inner, order), mustNotRefresh), logger, prepareRefresh: () => Promise.reject(error) }, 's')).rejects.toBe(error);

    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'transient', degraded: false, stage: 'discovery' }]);
    expect(order).not.toContain('tryAcquireLock');
    expect(order).not.toContain('delete');
    expect(await inner.get('s')).not.toBeNull();
  });

  it('SMA-704 test 7a: prepareRefresh rejects after another writer refreshed the record: return that record', async () => {
    const inner = new MemorySessionStore();
    await inner.set('s', makeRecord({ rev: 0, accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const order: string[] = [];
    const prepareRefresh = async (): Promise<void> => {
      // Another process refreshes the record while this one waits for discovery.
      await inner.set('s', makeRecord({ rev: 1, accessToken: 'AT-OTHER', refreshToken: 'RT-OTHER', accessExpiresAt: Date.now() + 600_000 }), 60_000, 0);
      throw new Error('oidc discovery failed: TypeError');
    };

    const out = await resolveSession({ ...deps(orderStore(inner, order), mustNotRefresh), prepareRefresh }, 's');

    expect(out?.accessToken).toBe('AT-OTHER');
    expect(out).not.toHaveProperty('refreshState');
    expect(order).not.toContain('tryAcquireLock');
  });

  it('SMA-704 test 7b: prepareRefresh rejects after another writer deleted the record: return null', async () => {
    const inner = new MemorySessionStore();
    await inner.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const order: string[] = [];
    const prepareRefresh = async (): Promise<void> => {
      await inner.delete('s'); // a logout in another tab
      throw new Error('oidc discovery failed: TypeError');
    };

    expect(await resolveSession({ ...deps(orderStore(inner, order), mustNotRefresh), prepareRefresh }, 's')).toBeNull();
    expect(order).not.toContain('tryAcquireLock');
  });

  it('SMA-704 test 8: the lockWaitMs deadline starts after prepareRefresh resolves', async () => {
    const store = new MemorySessionStore();
    // Inside the skew window and live, so the timeout branch returns 'pending', not null.
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() + 15_000 }), 60_000, null);
    await store.tryAcquireLock('s', 'someone-else', 60_000); // another holder, for the whole test
    const original = store.tryAcquireLock.bind(store);
    let attempts = 0;
    store.tryAcquireLock = (sid, token, ttlMs) => {
      attempts += 1;
      return original(sid, token, ttlMs);
    };
    const lockWaitMs = 120;
    const marks: { preparedAt: number | undefined } = { preparedAt: undefined };
    const prepareRefresh = async (): Promise<void> => {
      await new Promise((r) => setTimeout(r, 300)); // longer than lockWaitMs
      marks.preparedAt = Date.now();
    };

    const out = await resolveSession({ ...deps(store, mustNotRefresh), lockWaitMs, prepareRefresh }, 's');
    const returnedAt = Date.now();

    expect(out?.refreshState).toBe('pending');
    if (marks.preparedAt === undefined) throw new Error('prepareRefresh never resolved');
    // Date.now() is the clock resolveSession reads. With the deadline set before prepareRefresh,
    // the waiter makes exactly one attempt and returns at once.
    expect(returnedAt - marks.preparedAt).toBeGreaterThanOrEqual(lockWaitMs);
    expect(attempts).toBeGreaterThan(1);
  });

  it('SMA-704 R2: the absolute cap passes during the discovery wait: throw, and do not delete', async () => {
    const inner = new MemorySessionStore();
    await inner.set('s', makeRecord({ accessExpiresAt: Date.now() + 15_000 }), 60_000, null);
    const order: string[] = [];
    const store = orderStore(inner, order);
    const originalGet = store.get.bind(store);
    let getCalls = 0;
    store.get = async (sid) => {
      getCalls += 1;
      const rec = await originalGet(sid);
      // Call 2 is the re-read after prepareRefresh rejected. Its cap is already past; its access
      // token is still live, so only Math.min stops a degrade.
      return getCalls === 2 && rec !== null ? { ...rec, absoluteExpiresAt: Date.now() - 1 } : rec;
    };
    const { logger, events } = recordingLogger();
    const error = new Error('oidc discovery failed: TimeoutError');

    await expect(resolveSession({ ...deps(store, mustNotRefresh), logger, prepareRefresh: () => Promise.reject(error) }, 's')).rejects.toBe(error);

    expect(getCalls).toBe(2);
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'transient', degraded: false, stage: 'discovery' }]);
    // Not deleted on this path (spec § 3.2): the next read deletes it through the outer check.
    expect(order).not.toContain('delete');
    expect(order).not.toContain('tryAcquireLock');
  });

  it('SMA-704 R3: prepareRefresh resolves after another writer refreshed the record: no refresh call', async () => {
    const inner = new MemorySessionStore();
    await inner.set('s', makeRecord({ rev: 0, accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const prepareRefresh = async (): Promise<void> => {
      await inner.set('s', makeRecord({ rev: 1, accessToken: 'AT-OTHER', refreshToken: 'RT-OTHER', accessExpiresAt: Date.now() + 600_000 }), 60_000, 0);
    };
    const refresh = vi.fn(mustNotRefresh);

    const out = await resolveSession({ ...deps(inner, refresh), prepareRefresh }, 's');

    expect(out?.accessToken).toBe('AT-OTHER');
    expect(refresh).not.toHaveBeenCalled(); // invariant 1 still holds after the wait
  });

  it('SMA-704 R4: a prepareRefresh that throws synchronously takes the same transient path', async () => {
    const inner = new MemorySessionStore();
    await inner.set('s', makeRecord({ accessExpiresAt: Date.now() + 15_000 }), 60_000, null);
    const order: string[] = [];
    const { logger, events } = recordingLogger();
    const prepareRefresh = (): Promise<void> => {
      throw new Error('oidc discovery failed: TypeError');
    };

    const out = await resolveSession({ ...deps(orderStore(inner, order), mustNotRefresh), logger, prepareRefresh }, 's');

    expect(out?.refreshState).toBe('failed');
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'transient', degraded: true, stage: 'discovery' }]);
    expect(order).not.toContain('tryAcquireLock');
  });
});
```

- [ ] **Step 3: Write the failing wiring test 9**

In `tests/next/get-session.test.ts`, after `cookieJar` (line 57) add:

```ts

/** SMA-704. Wraps a real store and appends each `tryAcquireLock` call to `order`. */
function lockOrderStore(inner: SessionStore, order: string[]): SessionStore {
  return {
    get: (sid) => inner.get(sid),
    set: (sid, rec, ttlMs, expectedRev) => inner.set(sid, rec, ttlMs, expectedRev),
    delete: (sid) => inner.delete(sid),
    tryAcquireLock: (sid, token, ttlMs) => {
      order.push('tryAcquireLock');
      return inner.tryAcquireLock(sid, token, ttlMs);
    },
    releaseLock: (sid, token) => inner.releaseLock(sid, token),
    putTransaction: (txnId, tx, ttlMs) => inner.putTransaction(txnId, tx, ttlMs),
    takeTransaction: (txnId) => inner.takeTransaction(txnId),
    close: () => inner.close(),
  };
}
```

At the end of `describe('getSession', …)`, after the `it('revokes through runtime.oidc and returns null when a refreshed ID token names another subject', …)` block (it ends at line 188), add:

```ts

  // SMA-704 test 9. The guard on the PRODUCTION line in resolveDepsFor: getSession must wire
  // `prepareRefresh` to runtime.oidc.ensureDiscovered, and resolveSession must await it before the
  // lock. `ensureDiscovered` resolves after a macrotask, so a missing `await` fails too.
  it('SMA-704 test 9: runtime.oidc.ensureDiscovered resolves before the first tryAcquireLock', async () => {
    cookiesMock.mockResolvedValue(cookieJar('sid-cold'));
    const inner = new MemorySessionStore();
    await inner.set('sid-cold', { ...liveRecord(), accessExpiresAt: Date.now() - 1, refreshToken: 'RT' }, 999_000, null);
    const order: string[] = [];
    const runtime = {
      ...baseRuntime(lockOrderStore(inner, order)),
      oidc: {
        ...unusedOidc(),
        ensureDiscovered: async (): Promise<void> => {
          await new Promise((r) => setTimeout(r, 20));
          order.push('ensureDiscovered:resolved');
        },
        refresh: () => {
          order.push('refresh');
          return Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300 });
        },
      },
    };

    await expect(getSession(runtime)).resolves.toMatchObject({ accessToken: 'AT2' });

    expect(order).toEqual(['ensureDiscovered:resolved', 'tryAcquireLock', 'refresh']);
  });
```

- [ ] **Step 4: Turn test 13 back into `it`**

In `tests/core/single-flight-discovery.test.ts`, change `it.fails(` to `it(` on both test 13 cases. Replace the `RED-FIRST` comment block above `describe(` with:

```ts
// RED-FIRST (SMA-704 plan, Task 2). Both cases were `it.fails` in the commit that measured them on
// the code before the fix. The fix commit changed them to `it` and changed nothing else in them.
```

- [ ] **Step 5: Run the new tests and see them fail**

Run: `pnpm -C ts/packages/paigasus-auth exec vitest run tests/core/single-flight.test.ts tests/next/get-session.test.ts tests/core/single-flight-discovery.test.ts -t "SMA-704"`

Expected: FAIL. vitest does not type-check, so the pre-fix `resolveSession` runs and ignores `prepareRefresh`.
- Fail: test 1 (`order` has no `prepareRefresh:` entries), tests 4/6 and 5/6 (all six rows: `tryAcquireLock` was called, or the error is `must not refresh`), 7a, 7b, 8 (`prepareRefresh never resolved`), R2, R3, R4, test 9 (`order` lacks `ensureDiscovered:resolved`), 13a (`discovery requests while the lock is held: expected 1 to be +0`), 13b (`tryAcquireLock calls: expected 1 to be +0`).
- Pass before the fix (guards): test 2, test 3. Task 5's mutation battery proves that test 3 bites.

- [ ] **Step 6: Add the `stage` type to the logger port**

In `src/ports/logger.ts`, after line 16 (`// it. This type admits any string key, so that function is the control, not this port.`) add:

```ts
//
// `session.refresh_failed` may also carry `stage` (SMA-704), from RefreshFailedStage below. It is
// present only when the refresh failed BEFORE the session lock. The line never carries both `stage`
// and `oauthError`: a discovery failure has no OAuth code.
```

After the `OidcDiscoveryStage` type (line 49) add:

```ts

/**
 * The closed set of `stage` values for `session.refresh_failed` (SMA-704). `'discovery'` means
 * `prepareRefresh` (OIDC discovery) failed before the session lock was taken, so no token request
 * was sent. A failure of the token call under the lock has no `stage`.
 */
export type RefreshFailedStage = 'discovery';
```

- [ ] **Step 7: Implement `prepareRefresh` and the transient helper in single-flight**

In `src/core/single-flight.ts`:

Line 2 becomes:

```ts
import { isRefreshRejected, refreshFailureCode, type TokenErrorCode } from './errors';
```

Line 6 becomes:

```ts
import type { AuthLogger, RefreshFailedStage, StoreUnavailableStage } from '../ports/logger';
```

In `interface ResolveDeps`, after `store: SessionStore;` add:

```ts
  /**
   * Called at most once per resolveSession, only when a refresh is due and the record has a refresh
   * token, BEFORE the first tryAcquireLock (SMA-704). Production wires it to
   * OidcClient.ensureDiscovered, so the OIDC discovery request never runs under the lock. A
   * rejection is a transient refresh failure.
   */
  prepareRefresh: () => Promise<void>;
```

After the closing `}` of `revokeAll` (line 70) add:

```ts

/** SMA-704. What the transient `session.refresh_failed` line adds: the OAuth code (under the lock) or the stage (before it). */
interface TransientRefreshDetail {
  readonly oauthError?: TokenErrorCode | 'other';
  readonly stage?: RefreshFailedStage;
}

/**
 * SMA-704. The TRANSIENT outcome of a failed refresh, in one place, for the two sites that can see
 * one: the refresh call under the lock, and `prepareRefresh` before the lock. It logs
 * `session.refresh_failed` with `reason: 'transient'`. It returns the record with
 * `refreshState: 'failed'` while the access token is live. Otherwise it throws `err`, and
 * next/get-session.ts's catch handles it.
 *
 * `liveUntil` takes the MINIMUM of the two expiries. handleCallback sets them independently
 * (http/routes.ts:240-241) and only a refresh write clamps accessExpiresAt to the cap, so an IdP
 * whose `expires_in` exceeds PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS mints a first record whose access
 * token outlives its own absolute cap.
 *
 * It covers ONLY the transient outcome, and it never deletes. The site under the lock handles a
 * rejection itself. The site before the lock never classifies one: a discovery failure says nothing
 * about the refresh token.
 */
function transientRefreshFailure(logger: AuthLogger, sid: string, record: SessionRecord, err: unknown, detail: TransientRefreshDetail): ResolvedSession {
  const liveUntil = Math.min(record.accessExpiresAt, record.absoluteExpiresAt);
  const degraded = Date.now() < liveUntil;
  logger.event('session.refresh_failed', {
    sid: sidTag(sid),
    reason: 'transient',
    degraded,
    ...(detail.oauthError !== undefined ? { oauthError: detail.oauthError } : {}),
    ...(detail.stage !== undefined ? { stage: detail.stage } : {}),
  });
  if (degraded) return { ...record, refreshState: 'failed' };
  throw err;
}
```

Replace the invariant 5 paragraph (lines 103-108, from ` *  5. The write is FENCED on \`rev\`.` to ` *     is signed out — both bounds matter, not just the holder's.`) with:

```ts
 *  5. The write is FENCED on `rev`. A refresh that outlives its lock TTL would otherwise write
 *     its now-revoked token over a newer valid one; compare-and-delete protects the LOCK, never
 *     the WRITE. The caller additionally asserts the refresh HTTP timeout is below `lockTtlMs`
 *     for a LOCK HOLDER. For a WAITER the binding bound is `lockWaitMs`: if a refresh outlasts it
 *     while the token is hard-expired, the timeout branch below returns `null` and every waiter
 *     is signed out — both bounds matter, not just the holder's.
 *     OIDC discovery is NOT one of the calls under the lock (SMA-704). `prepareRefresh` runs it
 *     before the first `tryAcquireLock`, so runtime.ts's 2x bound counts only the token call and
 *     the JWKS call. A waiter's `lockWaitMs` starts after `prepareRefresh`, so a waiter keeps its
 *     full wait after discovery. The bound counts the IdP calls only. The store calls under the
 *     lock (the post-lock `get` and the fenced `set`) are not in it (SMA-704 spec § 6).
```

Replace lines 129-141 (from `  const { store, refresh, revoke, logger, skewMs, lockTtlMs, lockWaitMs, ttlMs } = deps;` to `  const deadline = Date.now() + lockWaitMs;`) with:

```ts
  const { store, prepareRefresh, refresh, revoke, logger, skewMs, lockTtlMs, lockWaitMs, ttlMs } = deps;

  const rec = await store.get(sid);
  if (rec === null) return null;
  if (Date.now() >= rec.absoluteExpiresAt) {
    await store.delete(sid);
    logger.event('session.deleted', { sid: sidTag(sid), reason: 'absolute_expiry' });
    return null;
  }
  if (!shouldRefresh(Date.now(), rec.accessExpiresAt, skewMs)) return rec;

  // SMA-704. Discovery BEFORE the lock. In production `prepareRefresh` is
  // OidcClient.ensureDiscovered (next/get-session.ts's resolveDepsFor), so under the lock the
  // discovery inside `refresh()` is a resolved promise and sends nothing. A record with no refresh
  // token skips this step: it reaches the `no_refresh_token` delete under the lock, as before, and
  // waits for no IdP.
  //
  // On a rejection this path takes no lock. It re-reads the record first, as the lock-timeout
  // branch does: up to PAIGASUS_OIDC_HTTP_TIMEOUT_MS has passed, and another process may have
  // refreshed or deleted the record in that time. It never classifies a rejection, so it never
  // deletes. A record whose cap passed during the wait is not deleted here either: the helper finds
  // it not live and throws, and the next read deletes it through the absolute-expiry check above.
  // A store failure during the re-read propagates, as every store failure here does.
  if (rec.refreshToken !== undefined) {
    try {
      await prepareRefresh();
    } catch (err) {
      const reread = await store.get(sid);
      if (reread === null) return null;
      if (!shouldRefresh(Date.now(), reread.accessExpiresAt, skewMs)) return reread; // invariant 1
      return transientRefreshFailure(logger, sid, reread, err, { stage: 'discovery' });
    }
  }

  const lockToken = newLockToken();
  // AFTER prepareRefresh (SMA-704): a waiter keeps its full lockWaitMs after discovery.
  const deadline = Date.now() + lockWaitMs;
```

Replace the whole refresh `catch` block, lines 177-225 (from `        } catch (err) {` to the `        }` that closes it, directly above `        // SMA-681 D5, D6 (spec § 4.3).`), with:

```ts
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
          // transientRefreshFailure (above) handles both transient outcomes. The site before the
          // lock (`prepareRefresh`, SMA-704) uses it too.
          //
          // `reason` is REQUIRED, not decoration. With `degraded` alone, a benign single-session
          // revocation and an outage that signs users out produce the identical line — the exact
          // conflation this whole section exists to remove.
          // NOT `instanceof` (SMA-657). `refresh` delegates to the shared `runtime.oidc`
          // (next/get-session.ts), so this error was built by whichever copy of this package
          // created the runtime, while this line runs in whichever copy serves the request. See
          // hasAuthErrorCode in core/errors.ts.
          if (isRefreshRejected(err)) {
            logger.event('session.refresh_failed', { sid: sidTag(sid), reason: 'rejected', degraded: false });
            await store.delete(sid);
            logger.event('session.deleted', { sid: sidTag(sid), reason: 'refresh_rejected' });
            throw err;
          }
          // SMA-692 D10. The OAuth code of a transient failure, from a closed set. It is never the
          // error object, its message or a URL. A failure with no OAuth code (a network error, a
          // timeout) gets no field. A rejection is always `invalid_grant`, so `reason` says it.
          const oauthError = refreshFailureCode(err);
          // The early return still runs the `finally` below, so the lock is released either way.
          return transientRefreshFailure(logger, sid, fresh, err, oauthError !== undefined ? { oauthError } : {});
        }
```

- [ ] **Step 8: Add `resolveDepsFor` and wire `getSession` through it**

In `src/next/get-session.ts`, line 36 becomes:

```ts
import { resolveSession, type ResolveDeps, type ResolvedSession } from '../core/single-flight';
```

Before the `getSession` doc comment (line 42) add:

```ts
/**
 * The ResolveDeps for one runtime (SMA-704). getSession uses it, and so does the e2e fixture server
 * (tests/e2e/fixture-server.ts), so the e2e tier runs this wiring and not a copy. All three IdP
 * functions go to the one `runtime.oidc`, so `prepareRefresh` and `refresh` share one discovery
 * cache: after `prepareRefresh` resolves, `refresh` sends no discovery request under the lock.
 * tests/next/get-session.test.ts (SMA-704 test 9) fails if `prepareRefresh` is not wired here.
 */
export function resolveDepsFor(runtime: AuthRuntime): ResolveDeps {
  return {
    store: runtime.store,
    prepareRefresh: () => runtime.oidc.ensureDiscovered(),
    refresh: (refreshToken) => runtime.oidc.refresh(refreshToken),
    revoke: (token) => runtime.oidc.revoke(token),
    logger: runtime.logger,
    skewMs: runtime.skewMs,
    lockTtlMs: runtime.lockTtlMs,
    lockWaitMs: runtime.lockWaitMs,
    ttlMs: runtime.ttlMs,
  };
}

```

Replace lines 52-65 (from `  try {` to `    );`, the closing of the `resolveSession(` call) with:

```ts
  try {
    return await resolveSession(resolveDepsFor(runtime), sid);
```

- [ ] **Step 9: Update the other `resolveSession` callers**

`tests/e2e/fixture-server.ts`: after line 32 (`import { resolveSession } from '../../src/core/single-flight.js';`) add:

```ts
import { resolveDepsFor } from '../../src/next/get-session.js';
```

Replace lines 151-162:

```ts
        : await resolveSession(
            {
              store: runtime.store,
              refresh: (refreshToken) => runtime.oidc.refresh(refreshToken),
              revoke: (token) => runtime.oidc.revoke(token),
              logger: runtime.logger,
              skewMs: runtime.skewMs,
              lockTtlMs: runtime.lockTtlMs,
              lockWaitMs: runtime.lockWaitMs,
              ttlMs: runtime.ttlMs,
            },
            sid,
          ).catch((err: unknown) => {
```

with:

```ts
        : await resolveSession(resolveDepsFor(runtime), sid).catch((err: unknown) => {
```

In the header, after line 27 (`// that cannot run outside a Next server and that none of task 13's specs are about.`) add:

```ts
//
// It builds the ResolveDeps with `resolveDepsFor` (src/next/get-session.ts), the function
// getSession itself uses, so this tier runs the production wiring and not a copy (SMA-704).
// src/server.ts already loads that module, so this import adds no module to this server.
```

`tests/fixtures/refresh-worker.ts` line 93 becomes:

```ts
  const result = await resolveSession({ store, prepareRefresh: () => Promise.resolve(), refresh, revoke: () => Promise.resolve(), logger: noopLogger, skewMs: 30_000, lockTtlMs: 10_000, lockWaitMs: 5_000, ttlMs: 60_000 }, sid);
```

`tests/containers/single-flight-redis.test.ts` lines 30-32 become:

```ts
function deps(store: SessionStore, refresh: (rt: string) => Promise<{ accessToken: string; refreshToken?: string; expiresIn: number }>) {
  // `prepareRefresh` (SMA-704): a fake with no discovery.
  return { store, prepareRefresh: () => Promise.resolve(), refresh, revoke: () => Promise.resolve(), logger: noopLogger, skewMs: 30_000, lockTtlMs: 5_000, lockWaitMs: 3_000, ttlMs: 60_000 };
}
```

`tests/http/logout.test.ts`, in the `resolveSession({ … })` literal at line 236, before `refresh:` add:

```ts
        prepareRefresh: () => Promise.reject(new Error('must not be called: the session is already deleted')),
```

- [ ] **Step 10: Add the R5 guard to the measurement file**

In `tests/core/single-flight-discovery.test.ts`, inside the `describe`, after test 13b, add:

```ts

  // Review Focus R5. On a cold process, concurrent requests that need a refresh share ONE
  // discovery request (the adapter's cache) and ONE token request (the lock). A GUARD: it passes
  // before the fix too, because discovery then ran under the lock of the one holder.
  it('SMA-704 R5: two concurrent refreshes on a cold client send one discovery and one token request', async () => {
    fixture.setResponseDelay('discovery', 100);
    const oidc = coldClient(fixture.issuer, T);
    const inner = new MemorySessionStore();
    await inner.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { logger } = recordingLogger();

    const [first, second] = await Promise.all([resolveSession(depsFor(inner, oidc, logger), 's'), resolveSession(depsFor(inner, oidc, logger), 's')]);

    const log = fixture.requests();
    expect(log.filter((r) => r.endpoint === 'discovery')).toHaveLength(1);
    expect(log.filter((r) => r.endpoint === 'token')).toHaveLength(1);
    expect(first?.accessToken).toBeDefined();
    expect(second?.accessToken).toBe(first?.accessToken);
  });
```

- [ ] **Step 11: Run the tests and the type check**

Run: `pnpm -C ts/packages/paigasus-auth exec vitest run tests/core tests/next tests/http tests/adapters`
Expected: PASS. Every SMA-704 test passes, and every earlier test passes unchanged.

Run: `pnpm -C ts/packages/paigasus-auth exec vitest run tests/core/single-flight-discovery.test.ts`
Expected: PASS, 3 passed. Copy the `SMA-704 test 13a:` and `SMA-704 test 13b:` console lines.

Run: `pnpm -C ts/packages/paigasus-auth exec tsc -p tsconfig.json --noEmit`
Expected: exit 0. This also compiles `tests/containers/single-flight-redis.test.ts`, `tests/fixtures/refresh-worker.ts` and `tests/e2e/fixture-server.ts`.

Run: `pnpm -C ts exec prettier --write packages/paigasus-auth/src/ports/logger.ts packages/paigasus-auth/src/core/single-flight.ts packages/paigasus-auth/src/next/get-session.ts packages/paigasus-auth/tests/e2e/fixture-server.ts packages/paigasus-auth/tests/fixtures/refresh-worker.ts packages/paigasus-auth/tests/containers/single-flight-redis.test.ts packages/paigasus-auth/tests/http/logout.test.ts packages/paigasus-auth/tests/core/single-flight.test.ts packages/paigasus-auth/tests/next/get-session.test.ts packages/paigasus-auth/tests/core/single-flight-discovery.test.ts`

- [ ] **Step 12: Record the after-fix run in spec § 7**

Add a row under the "Before the fix" row of the § 7 table, with the numbers from Step 11. The commit cell says `this commit (Task 3)`:

```markdown
| After the fix | this commit (Task 3) | <measured> | <measured> / <measured> | <measured> | <measured> | <measured> | <measured> |
```

Under the table, add: `After the fix, 13a and 13b pass, and the lock hold time no longer contains the discovery delay.` Use the measured hold times to check that statement. If the "after" hold time is not about 600 ms below the "before" hold time, stop and report it.

- [ ] **Step 13: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-704 add ts/packages/paigasus-auth/src/ports/logger.ts ts/packages/paigasus-auth/src/core/single-flight.ts ts/packages/paigasus-auth/src/next/get-session.ts ts/packages/paigasus-auth/tests/e2e/fixture-server.ts ts/packages/paigasus-auth/tests/fixtures/refresh-worker.ts ts/packages/paigasus-auth/tests/containers/single-flight-redis.test.ts ts/packages/paigasus-auth/tests/http/logout.test.ts ts/packages/paigasus-auth/tests/core/single-flight.test.ts ts/packages/paigasus-auth/tests/next/get-session.test.ts ts/packages/paigasus-auth/tests/core/single-flight-discovery.test.ts docs/superpowers/specs/2026-09-27-sma-704-refresh-discovery-outside-lock-design.md
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-704 commit -F <scratchpad>/msg-task3.txt
```

Message file content:

```text
fix(ts): run OIDC discovery before resolveSession takes the session lock

SMA-704. resolveSession now awaits a required prepareRefresh before the
first tryAcquireLock, when a refresh is due and the record has a refresh
token. getSession and the e2e fixture server wire it to
runtime.oidc.ensureDiscovered through the new resolveDepsFor. A rejection
takes no lock: it re-reads the record and goes through the shared
transient helper, which logs stage discovery and never deletes. The
lockWaitMs deadline now starts after prepareRefresh. Test 13 is it again
and passes.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

### Task 4: Runtime cache key v2 and the documented bound

**Files:**
- Modify: `ts/packages/paigasus-auth/src/runtime.ts` (invariant 3 comment, after line 140; `RUNTIME_KEY_PREFIX` comment and value, lines 217-224)
- Modify: `ts/packages/paigasus-auth/README.md` (rows at lines 40 and 47; bullet at lines 239-241)
- Test: `ts/packages/paigasus-auth/tests/runtime.test.ts` (lines 150-157; new `it` in `describe('getAuthRuntime', …)` after line 208)

**Interfaces:**
- Consumes: `getAuthRuntime(cfg)`, the `BASE` config of `tests/runtime.test.ts`.
- Produces: the global slot `Symbol.for('paigasus.auth.runtime.v2:<zone>')`.

- [ ] **Step 1: Write the failing key test**

In `tests/runtime.test.ts`, replace lines 150-157 (from the `// The cache lives on globalThis` comment through the closing `}` of `clearSharedRuntime`) with:

```ts
// The cache lives on globalThis (runtime.ts), so `vi.resetModules()` alone no longer clears it.
// The key comes from the GLOBAL symbol registry, the same way runtime.ts builds it, so this test
// file needs no export of its own from the source. It is keyed BY ZONE (SMA-511 final review,
// minor 8), so clearing it takes the zone as well. The shape version is v2 since SMA-704.
const ZONES = ['iam', 'gateway', 'ghost'] as const;
const RUNTIME_KEY_PREFIX = 'paigasus.auth.runtime.v2';

function clearSharedRuntime(): void {
  for (const zone of ZONES) {
    delete (globalThis as typeof globalThis & Record<symbol, unknown>)[Symbol.for(`${RUNTIME_KEY_PREFIX}:${zone}`)];
  }
}
```

In `describe('getAuthRuntime', …)`, after the `it('shares one runtime between two module instances, as two Next layers get', …)` block, add:

```ts

  // SMA-704. AuthRuntime.oidc gained a required method (ensureDiscovered), so the key's shape
  // version moved to v2: a runtime that an older module copy cached under v1 has no such method.
  // This row reds if the prefix moves back.
  it('caches the runtime under the v2 shape key, not v1 (SMA-704)', async () => {
    const rt = await getAuthRuntime(BASE);
    const holder = globalThis as typeof globalThis & Record<symbol, Promise<unknown> | undefined>;

    await expect(holder[Symbol.for('paigasus.auth.runtime.v2:iam')]).resolves.toBe(rt);
    expect(holder[Symbol.for('paigasus.auth.runtime.v1:iam')]).toBeUndefined();
  });
```

- [ ] **Step 2: Run the runtime tests and see them fail**

Run: `pnpm -C ts/packages/paigasus-auth exec vitest run tests/runtime.test.ts`

Expected: FAIL. The new row fails with `TypeError: You must provide a Promise to expect() when using .resolves, not 'undefined'`. The SMA-626 guard-3 rows can fail too, because `clearSharedRuntime` no longer clears the v1 slot that the source still writes.

- [ ] **Step 3: Move the key to v2 and document the bound in runtime.ts**

In `src/runtime.ts`, line 224 becomes:

```ts
const RUNTIME_KEY_PREFIX = 'paigasus.auth.runtime.v2';
```

At the end of the doc comment above it (after ` * segment keeps a future, incompatible \`AuthRuntime\` off this slot rather than on it.`) add:

```ts
 *
 * v2 SINCE SMA-704. `OidcClient` gained the required `ensureDiscovered`, and `AuthRuntime.oidc` is
 * one. A runtime that an older module copy cached under v1 (for example across a dev HMR reload)
 * has no such method, and every refresh would fail on it. The new version keeps this copy off that
 * slot.
```

In the invariant 3 comment, after line 140 (`  // them after it releases the lock.`) add:

```ts
  //
  // DISCOVERY IS NOT ONE OF THE CALLS UNDER THE LOCK (SMA-704). core/single-flight.ts's
  // resolveSession calls `prepareRefresh` before it takes the lock, and next/get-session.ts's
  // resolveDepsFor wires that to OidcClient.ensureDiscovered. Under the lock, the `getConfig()` in
  // `refresh()` is then a resolved promise and sends nothing. With discovery under the lock, the
  // shipped defaults would need 3x (10500 ms) inside a 10000 ms TTL.
  //
  // This rule covers the IdP calls only. The store calls under the lock (the post-lock `get` and
  // the fenced `set`) are each bounded by 4x PAIGASUS_SESSION_REDIS_TIMEOUT_MS
  // (adapters/operation-deadline.ts), and they are not in this rule. One slow `get` plus 2x the
  // OIDC timeout is 11000 ms at the defaults. No issue tracks this residual. The SMA-704 spec § 6
  // and the README record it.
```

Do not change the check on the next lines or its message.

- [ ] **Step 4: Update the README**

In `README.md`, in the `PAIGASUS_OIDC_HTTP_TIMEOUT_MS` row (line 40), replace the description cell text `A positive integer. Must satisfy \`2 * PAIGASUS_OIDC_HTTP_TIMEOUT_MS < PAIGASUS_SESSION_LOCK_TTL_MS\` — see that variable.` with:

```text
A positive integer. Must satisfy `2 * PAIGASUS_OIDC_HTTP_TIMEOUT_MS < PAIGASUS_SESSION_LOCK_TTL_MS` — see that variable. OIDC discovery runs before the refresh path takes the session lock, so discovery is not in this 2x budget (SMA-704).
```

In the `PAIGASUS_SESSION_LOCK_TTL_MS` row (line 47), replace the description cell text with:

```text
A positive integer. Must stay strictly above `2 * PAIGASUS_OIDC_HTTP_TIMEOUT_MS` — a refresh makes two sequential bounded calls (token exchange, then a JWKS fetch) under this lock, and a refresh that outlives its lock is exactly what the single-flight guarantee (AC 2) depends on not happening. The shipped defaults (`3500`, `10000`) already satisfy this. OIDC discovery is not one of these calls: on a cold process it runs before the lock is taken (SMA-704). The store calls under the lock are not in this rule either; see "Known limits" under "When the identity provider cannot be discovered".
```

Replace the bullet at lines 239-241 (from `- The refresh path does not use this 503;` to `exceed the lock's TTL under the shipped defaults. SMA-704 tracks this.`) with:

```markdown
- The refresh path does not use this 503; a discovery failure there stays a transient failure.
  Discovery runs before the refresh path takes its per-session lock (SMA-704). So the lock's
  budget, `2 * PAIGASUS_OIDC_HTTP_TIMEOUT_MS`, holds only the token call and the JWKS call. On a
  cold process, a request that must refresh first waits up to `PAIGASUS_OIDC_HTTP_TIMEOUT_MS` for
  discovery, outside the lock.
- The store calls under the refresh lock are not in that budget. Each one can take up to 4 ×
  `PAIGASUS_SESSION_REDIS_TIMEOUT_MS` (4000 ms at the default). One slow store read plus two slow
  IdP calls (11000 ms at the defaults) can outlast the 10000 ms lock TTL. No issue tracks this.
```

Run: `pnpm -C ts exec prettier --write packages/paigasus-auth/README.md packages/paigasus-auth/src/runtime.ts packages/paigasus-auth/tests/runtime.test.ts`. Prettier re-pads the table.

- [ ] **Step 5: Run the tests and the type check**

Run: `pnpm -C ts/packages/paigasus-auth exec vitest run tests/runtime.test.ts tests/runtime-redis-wiring.test.ts`
Expected: PASS.

Run: `pnpm -C ts/packages/paigasus-auth exec tsc -p tsconfig.json --noEmit`
Expected: exit 0.

Run: `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-704 grep -n "paigasus.auth.runtime.v1" -- ts`
Expected: only the one line in the new test's `toBeUndefined()` assertion. (The spec and this plan under `docs/` name v1 on purpose.)

- [ ] **Step 6: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-704 add ts/packages/paigasus-auth/src/runtime.ts ts/packages/paigasus-auth/tests/runtime.test.ts ts/packages/paigasus-auth/README.md
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-704 commit -F <scratchpad>/msg-task4.txt
```

Message file content:

```text
fix(ts): move the auth runtime cache key to v2 for SMA-704

The OidcClient inside the cached runtime gained a required method, so a
runtime that an older module copy cached under v1 lacks it. The runtime.ts
invariant 3 comment and the README now say that discovery runs before the
session lock and is not in the 2x budget. They also state the store-call
residual that no issue tracks.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

---

### Task 5: Prove the tests bite, and run every gate

**Files:**
- Modify (temporarily, restored in each step): `ts/packages/paigasus-auth/src/core/single-flight.ts`, `ts/packages/paigasus-auth/src/next/get-session.ts`, `ts/packages/paigasus-auth/src/adapters/oidc.ts`
- Modify: `docs/superpowers/specs/2026-09-27-sma-704-refresh-discovery-outside-lock-design.md` (§ 7, the battery table)

**Interfaces:**
- Consumes: the code and tests of Tasks 1-4.
- Produces: the § 5.5 battery result in spec § 7.

For EACH mutation M1-M6: make the change with the Edit tool; run tsc and expect exit 0 (the red must come from a test, not from tsc); run the named test and expect FAIL on the named test; restore the line with the Edit tool (NEVER `git checkout`); run the named test again and expect PASS. After M6, `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-704 status --porcelain` must be empty.

tsc command for every step: `pnpm -C ts/packages/paigasus-auth exec tsc -p tsconfig.json --noEmit`

- [ ] **Step 1: M1 — delete `await prepareRefresh()`**

In `src/core/single-flight.ts`, delete the line `      await prepareRefresh();` (the `try` block is then empty).
Run: `pnpm -C ts/packages/paigasus-auth exec vitest run tests/core/single-flight.test.ts tests/core/single-flight-discovery.test.ts -t "SMA-704 test 1"`
Expected: FAIL on `SMA-704 test 1:` and on `SMA-704 test 13a` and `13b`. Restore the line, re-run, expect PASS.

- [ ] **Step 2: M2 — unwire `prepareRefresh` in `resolveDepsFor`**

In `src/next/get-session.ts`, change `    prepareRefresh: () => runtime.oidc.ensureDiscovered(),` to `    prepareRefresh: async () => {},`.
Run: `pnpm -C ts/packages/paigasus-auth exec vitest run tests/next/get-session.test.ts -t "SMA-704 test 9"`
Expected: FAIL on test 9. Restore, re-run, expect PASS.

- [ ] **Step 3: M3 — set the deadline before `prepareRefresh`**

In `src/core/single-flight.ts`, move the two lines

```ts
  // AFTER prepareRefresh (SMA-704): a waiter keeps its full lockWaitMs after discovery.
  const deadline = Date.now() + lockWaitMs;
```

to directly above `  if (rec.refreshToken !== undefined) {`.
Run: `pnpm -C ts/packages/paigasus-auth exec vitest run tests/core/single-flight.test.ts -t "SMA-704 test 8"`
Expected: FAIL on test 8 (`attempts` is 1, or the elapsed time is below 120). Move the lines back, re-run, expect PASS.

- [ ] **Step 4: M4 — make `ensureDiscovered` a no-op**

In `src/adapters/oidc.ts`, delete the line `      await getConfig();` inside `ensureDiscovered`.
Run: `pnpm -C ts/packages/paigasus-auth exec vitest run tests/adapters/oidc.test.ts -t "SMA-704 test 10"`
Expected: FAIL on test 10 (`expected 0 to be 1`). Restore, re-run, expect PASS.

- [ ] **Step 5: M5 — remove the re-read on failure**

In `src/core/single-flight.ts`, change `      const reread = await store.get(sid);` to `      const reread: SessionRecord | null = rec;`.
Run: `pnpm -C ts/packages/paigasus-auth exec vitest run tests/core/single-flight.test.ts -t "SMA-704 test 7"`
Expected: FAIL on 7a and 7b. Restore, re-run, expect PASS.

- [ ] **Step 6: M6 — remove the refresh-token condition**

In `src/core/single-flight.ts`, change `  if (rec.refreshToken !== undefined) {` to `  if (true) {`.
Run: `pnpm -C ts/packages/paigasus-auth exec vitest run tests/core/single-flight.test.ts -t "SMA-704 test 3"`
Expected: FAIL on test 3 (`prepareRefresh` was called). Restore, re-run, expect PASS.

Run: `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-704 status --porcelain`
Expected: no output.

- [ ] **Step 7: Record the battery in spec § 7**

Append to spec § 7:

```markdown
Mutation battery (§ 5.5), run after the fix. Each mutation compiled (`tsc --noEmit` exit 0), and
each was restored with an edit, not with `git checkout`.

| Mutation | Red tests |
| --- | --- |
| Delete `await prepareRefresh()` | <the failing test names from Step 1> |
| `resolveDepsFor`'s `prepareRefresh` is `async () => {}` | <from Step 2> |
| The `deadline` line before `prepareRefresh` | <from Step 3> |
| `ensureDiscovered` is a no-op | <from Step 4> |
| The failure path uses `rec`, not a re-read | <from Step 5> |
| No `rec.refreshToken !== undefined` condition | <from Step 6> |
```

Fill each cell with the test names that went red in that step. If a step's named test did NOT go red, stop and report it: the test does not bite.

- [ ] **Step 8: Run the package gates**

Run: `moon run paigasus-auth-ts:test paigasus-auth-ts:typecheck ts:lint ts:fmt`
Expected: all four tasks pass. If `ts:fmt` fails, run `pnpm -C ts exec prettier --write <the files it names>` and re-run.

- [ ] **Step 9: Run the Docker-backed tier**

This tier runs `tests/containers/**` (including `single-flight-redis.test.ts` and the multiprocess test with `refresh-worker.ts`) and the Playwright specs against the fixture server, which now uses `resolveDepsFor`.

Run: `moon run paigasus-auth-ts:test-e2e`
Expected: PASS. It needs Docker. If Docker is not reachable, report that and do not claim a pass. A Chromium `net::ERR_NETWORK_CHANGED` failure is a known flake: re-run it once and report both runs.

- [ ] **Step 10: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-704 add docs/superpowers/specs/2026-09-27-sma-704-refresh-discovery-outside-lock-design.md
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-704 commit -F <scratchpad>/msg-task5.txt
```

Message file content:

```text
docs(ts): record the SMA-704 mutation battery

Each of the six mutations of the spec compiled and turned its named test
red. Each was restored with an edit.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

The full-graph `moon ci` run before the push belongs to the open-pr stage. Use the command in the root `CLAUDE.md` `ci-targets` block, and read the bash-version notes there first.

---

## Spec coverage

| Spec section | Task |
|---|---|
| § 1, § 2 (decision, no default change) | Global Constraints; Task 4 keeps the check |
| § 3.1 `ensureDiscovered` | Task 1 |
| § 3.2 `prepareRefresh`, order, re-read, transient helper, `stage` | Task 3 |
| § 3.3 `resolveDepsFor`, callers, fakes | Task 1 (fakes), Task 3 (callers, wiring) |
| § 3.4 cache key v2 | Task 4 |
| § 3.5 comments and README | Task 1 (oidc.ts header), Task 3 (invariant 5, logger port), Task 4 (runtime.ts, README) |
| § 4 what does not change | Global Constraints; Task 4 Step 3 |
| § 5.1 tests 1-8 | Task 3 |
| § 5.2 test 9 | Task 3 |
| § 5.3 tests 10-12 | Task 1 |
| § 5.4 test 13 (red first, hanging case) | Task 2, flipped in Task 3 |
| § 5.5 battery | Task 5 |
| § 6 residuals | Task 3 (invariant 5), Task 4 (runtime.ts, README) |
| § 7 measurements | Task 2 (before), Task 3 (after), Task 5 (battery) |
