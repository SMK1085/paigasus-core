# SMA-657: classify RefreshRejected by its code Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Classify a refused token refresh by the error's `code` field instead of `instanceof`, so the check still works when the error comes from Next's other copy of `@paigasus/auth`.

**Architecture:** Next 16 gives a route handler and a page separate module copies. The `AuthRuntime` is shared between them through `globalThis`, so `runtime.oidc` — and every error class it throws — belongs to whichever copy built the runtime first. `instanceof` in the other copy is then `false`. The fix adds a `code`-based predicate beside the class, mirroring `isSessionStoreUnavailable`, which SMA-653 added for the same reason.

**Tech Stack:** TypeScript, Vitest, Moon. Package `ts/packages/paigasus-auth` (`@paigasus/auth`).

**Spec:** `docs/superpowers/specs/2026-09-19-sma-657-refresh-rejected-code-design.md`

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`. Every file this plan touches already has one; do not add a second.
- Work in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-657-refresh-rejected-code`, on branch `feature/sma-657-refresh-rejected-code`. Do NOT `cd` to the main checkout.
- Prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so `pnpm` and `moon` resolve to the repo-pinned versions.
- Run tests from `ts/packages/paigasus-auth` with `pnpm exec vitest run <path>`.
- Conventional commits with a workspace scope: `fix(ts): …`. End every commit message with `Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`.
- Do NOT bypass the lefthook `commit-msg` hook with `--no-verify`.
- Relative imports inside this package use the `.js` extension in test files and NO extension in `src/` — follow whatever the file you are editing already does. Every file this plan touches already has its imports; copy their style.
- The predicate returns `boolean`, never a type guard `err is RefreshRejected` (spec D2).
- `hasAuthErrorCode` is NOT exported from `core/errors.ts` (spec D3). `isRefreshRejected` IS exported from `core/errors.ts` but is NOT added to `src/server.ts` (spec D4).

## File Structure

| File | Change | Responsibility |
| --- | --- | --- |
| `ts/packages/paigasus-auth/src/core/errors.ts` | Modify | Add `hasAuthErrorCode` (private) and `isRefreshRejected` (exported). Move `isSessionStoreUnavailable` onto the helper. Correct the `SessionStoreTimeout` doc comment. |
| `ts/packages/paigasus-auth/src/core/single-flight.ts` | Modify lines 2 and 170 | Swap the import and the classification. |
| `ts/packages/paigasus-auth/src/server.ts` | Modify near line 111 | Comment only: why this `instanceof` cannot cross. |
| `ts/packages/paigasus-auth/src/adapters/operation-deadline.ts` | Modify near line 73 | Comment only. |
| `ts/packages/paigasus-auth/src/adapters/oidc.ts` | Modify near line 153 | Comment only. |
| `ts/packages/paigasus-auth/tests/core/errors.test.ts` | Modify | A new `isRefreshRejected` describe block. |
| `ts/packages/paigasus-auth/tests/core/single-flight.test.ts` | Modify | Two new rows in the `'a failing refresh (SMA-626 § 2.3)'` block. |
| `ts/packages/paigasus-auth/tests/next/get-session.test.ts` | Modify | One new row proving the user-visible half. |

---

### Task 1: The `code`-based predicate

**Files:**
- Modify: `ts/packages/paigasus-auth/src/core/errors.ts`
- Test: `ts/packages/paigasus-auth/tests/core/errors.test.ts`

**Interfaces:**
- Consumes: nothing.
- Produces: `export function isRefreshRejected(err: unknown): boolean` from `src/core/errors.ts`. Task 2 imports it. A private `function hasAuthErrorCode(err: unknown, code: string): boolean` in the same file, not exported.

- [ ] **Step 1: Write the failing test**

Append this block to `ts/packages/paigasus-auth/tests/core/errors.test.ts`, after the existing `describe('isSessionStoreUnavailable (SMA-653 D2)', …)` block. Add `RefreshRejected` and `isRefreshRejected` to that file's existing `../../src/core/errors.js` import (NOT the `vitest` import on the line above it), so it reads:

```ts
import { CallbackRejected, RefreshRejected, SessionStoreTimeout, SessionStoreUnavailable, isRefreshRejected, isSessionStoreUnavailable } from '../../src/core/errors.js';
```

Then append:

```ts
// SMA-657. The same defect as the block above, on the OTHER class that crosses the two copies:
// `refresh` delegates to the shared `runtime.oidc`, so adapters/oidc.ts builds a RefreshRejected
// with the class of whichever copy built the runtime, and core/single-flight.ts classifies it in
// whichever copy serves the request.
describe('isRefreshRejected (SMA-657)', () => {
  it('is true for a RefreshRejected', () => {
    expect(isRefreshRejected(new RefreshRejected('invalid_grant'))).toBe(true);
  });

  it('is true for an error from a SECOND copy of core/errors', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    // Precondition: without this, the test passes vacuously if the import returns the same module.
    expect(foreign.RefreshRejected).not.toBe(RefreshRejected);
    const err = new foreign.RefreshRejected('invalid_grant');
    expect(err instanceof RefreshRejected).toBe(false);
    expect(isRefreshRejected(err)).toBe(true);
  });

  // D8. The check is deliberately WIDER than `instanceof`: `code` is the identity, and admitting
  // any Error that carries it is the whole mechanism by which the foreign-copy instance passes.
  // Pinned here so the semantics are stated rather than discovered.
  it('is true for any Error carrying the code, not only a RefreshRejected', () => {
    expect(isRefreshRejected(Object.assign(new Error('x'), { code: 'oidc_refresh_rejected' }))).toBe(true);
  });

  it('is false for other errors and for non-errors', () => {
    expect(isRefreshRejected(new CallbackRejected('txn_missing'))).toBe(false);
    expect(isRefreshRejected(new SessionStoreUnavailable('down'))).toBe(false);
    expect(isRefreshRejected(new SessionStoreTimeout('get', 4000, 'deadline'))).toBe(false);
    // The literal in the MESSAGE, not in `code`.
    expect(isRefreshRejected(new Error('oidc_refresh_rejected'))).toBe(false);
    // A plain object, not an Error.
    expect(isRefreshRejected({ code: 'oidc_refresh_rejected' })).toBe(false);
    expect(isRefreshRejected(undefined)).toBe(false);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-auth && pnpm exec vitest run tests/core/errors.test.ts
```

Expected: FAIL. TypeScript reports that `isRefreshRejected` is not exported from `../../src/core/errors.js`.

- [ ] **Step 3: Write the implementation**

In `ts/packages/paigasus-auth/src/core/errors.ts`, replace the whole existing `isSessionStoreUnavailable` function AND its doc comment (they currently sit between the `SessionStoreTimeout` class and the `AuthConfigError` class) with this:

```ts
/**
 * True when `err` is an `Error` whose own `code` property equals `code`, from ANY copy of this
 * module.
 *
 * Why not `instanceof`: Next 16 gives a route handler and a page SEPARATE copies of this package
 * (measured, see src/runtime.ts's comment on RUNTIME_KEY_PREFIX), and state is shared between them
 * through globalThis. An object built by one copy is therefore routinely classified by the other,
 * where `instanceof` against the local class is false. The `code` class field is an own property of
 * every instance, and a subclass inherits it, so it survives the duplication.
 *
 * THE RULE, stated once (SMA-657 D7). A class thrown by a closure that is reachable through shared
 * state, and caught OUTSIDE that closure, must be classified by its `code`. A class thrown and
 * caught inside one closure, or thrown and caught by two modules of one copy, may use `instanceof`
 * — the boundary is the CLOSURE, not whether state is shared.
 *
 * The `err instanceof Error` test is an `instanceof` against a BUILTIN, which both copies share in
 * one isolate, so it does not have the defect this function exists to avoid. It is what rejects a
 * plain object that happens to carry a matching `code`.
 *
 * This is deliberately WIDER than `instanceof`: any `Error` carrying the code passes, not only an
 * instance of the declaring class. That widening IS the mechanism.
 */
function hasAuthErrorCode(err: unknown, code: string): boolean {
  return err instanceof Error && (err as { code?: unknown }).code === code;
}

/** True for a SessionStoreUnavailable, or a subclass such as SessionStoreTimeout, from ANY copy of
 * this module (SMA-653 D2). See `hasAuthErrorCode` for why this is not an `instanceof`. */
export function isSessionStoreUnavailable(err: unknown): boolean {
  return hasAuthErrorCode(err, 'session_store_unavailable');
}

/** True for a RefreshRejected from ANY copy of this module (SMA-657 D1). The crossing path is
 * real and not hypothetical: next/get-session.ts wires `refresh` to the shared `runtime.oidc`, so
 * adapters/oidc.ts builds this class in whichever copy created the runtime, and
 * core/single-flight.ts classifies it in whichever copy serves the request. See
 * `hasAuthErrorCode`. */
export function isRefreshRejected(err: unknown): boolean {
  return hasAuthErrorCode(err, 'oidc_refresh_rejected');
}
```

Note `isRefreshRejected` is placed here, beside its sibling, even though `RefreshRejected` is declared at the bottom of the file. A function declaration is hoisted, so the forward reference in the doc comment is fine and the two predicates stay together.

- [ ] **Step 4: Run the test to verify it passes**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-auth && pnpm exec vitest run tests/core/errors.test.ts
```

Expected: PASS, including every pre-existing `isSessionStoreUnavailable` row.

- [ ] **Step 5: Run the two other suites that pin `isSessionStoreUnavailable`**

The spec's D3 names three existing tests that keep the move onto the shared helper honest. Run the other two:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-auth && pnpm exec vitest run tests/http/store-unavailable.test.ts tests/next/get-session.test.ts
```

Expected: PASS. If either fails, the D3 refactor changed behaviour — stop and report; do not adjust the tests.

- [ ] **Step 6: Commit**

```bash
git add ts/packages/paigasus-auth/src/core/errors.ts ts/packages/paigasus-auth/tests/core/errors.test.ts
git commit -m "$(cat <<'EOF'
fix(ts): add isRefreshRejected and share one code predicate (SMA-657)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 2: The fix, proved red first

Both new tests here red against the unfixed code, and the single one-line change greens both. They share a task because they share that fix — committing either test alone would commit a red test.

**Files:**
- Modify: `ts/packages/paigasus-auth/src/core/single-flight.ts:2` and `:170`
- Test: `ts/packages/paigasus-auth/tests/core/single-flight.test.ts`
- Test: `ts/packages/paigasus-auth/tests/next/get-session.test.ts`

**Interfaces:**
- Consumes: `isRefreshRejected` from Task 1.
- Produces: nothing new. `resolveSession`'s signature and behaviour contract are unchanged.

- [ ] **Step 1: Write the first failing test (the classification)**

In `ts/packages/paigasus-auth/tests/core/single-flight.test.ts`, inside the existing
`describe('a failing refresh (SMA-626 § 2.3)', …)` block, add this row immediately after the
existing `it('never degrades a definitive rejection, even with a live access token', …)`:

```ts
  // SMA-657. The SAME case as the row above, with the error built by a SECOND copy of core/errors
  // — the shape Next 16 produces, because `refresh` delegates to the shared `runtime.oidc` while
  // `resolveSession` runs in whichever copy serves the request. `resolveSession` was imported
  // statically at the top of this file, so it keeps its FIRST-copy binding: this is the real
  // two-copy shape, not a simulation of it.
  //
  // The fixture must be inside the skew window AND still live, or single-flight.ts returns early
  // and never attempts a refresh at all (shouldRefresh is `now >= expiresAt - skewMs`). The live
  // token is the point: it is the case where the defect changes the RETURN path, not just the log.
  it('classifies a definitive rejection from a SECOND module copy', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    // Precondition: without this, the test passes vacuously if the import returns the same module.
    expect(foreign.RefreshRejected).not.toBe(RefreshRejected);
    const foreignRejected = () => Promise.reject(new foreign.RefreshRejected('invalid_grant'));

    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() + 30_000 }), 60_000, null);
    const { logger, events } = recordingLogger();

    // NOT `toBeInstanceOf(RefreshRejected)`: that is FALSE for a foreign-copy error, which is the
    // entire defect. Asserting it would red this test both before AND after the fix.
    await expect(resolveSession({ ...deps(store, foreignRejected), logger, skewMs: 60_000 }, 's')).rejects.toBeInstanceOf(foreign.RefreshRejected);
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'rejected', degraded: false }]);
    expect(events).toContainEqual(['session.deleted', { sid: sidTag('s'), reason: 'refresh_rejected' }]);
    expect(await store.get('s')).toBeNull();
  });
```

- [ ] **Step 2: Write the second failing test (the user-visible half)**

In `ts/packages/paigasus-auth/tests/next/get-session.test.ts`, inside the existing
`describe('getSession failure attribution (SMA-626 § 2.4)', …)` block, add this row immediately
after the existing `it('logs session.resolve_failed, NOT store.unavailable, when the IdP definitively rejects', …)`:

```ts
  // SMA-657. The end-to-end half of the fix, and the only unit-level place it is observable.
  // A LIVE access token inside the skew window is required: before the fix `resolveSession`
  // degrades and getSession returns a session with refreshState 'failed'; after it, the call
  // throws and getSession returns null. With a hard-expired token the row passes either way.
  it('returns null when the IdP rejects with a RefreshRejected from a SECOND module copy', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    expect(foreign.RefreshRejected).not.toBe(RefreshRejected);
    cookiesMock.mockResolvedValue(cookieJar('sid-needs-refresh'));
    const store = new MemorySessionStore();
    // baseRuntime's skewMs is 30_000, so +15_000 is inside the window and still live.
    await store.set('sid-needs-refresh', { ...liveRecord(), accessExpiresAt: Date.now() + 15_000, refreshToken: 'RT' }, 999_000, null);
    const { logger, events } = recordingLogger();

    await expect(
      getSession({
        ...baseRuntime(store),
        logger,
        oidc: { ...unusedOidc(), refresh: () => Promise.reject(new foreign.RefreshRejected('invalid_grant')) },
      }),
    ).resolves.toBeNull();

    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('sid-needs-refresh'), reason: 'rejected', degraded: false }]);
    expect(await store.get('sid-needs-refresh')).toBeNull();
  });
```

- [ ] **Step 3: Run both tests to verify they FAIL**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-auth && pnpm exec vitest run tests/core/single-flight.test.ts tests/next/get-session.test.ts
```

Expected: BOTH new rows FAIL, and every other row passes.

- The `single-flight` row fails on the event assertion: the logged event carries
  `reason: 'transient', degraded: true` instead of `reason: 'rejected', degraded: false`, and the
  `rejects` assertion fails too because `resolveSession` RESOLVES with the degraded record rather
  than rejecting.
- The `get-session` row fails because `getSession` resolves to a session object, not `null`.

**Record the exact failure output.** It is the red-first evidence the spec requires (§ 4 item 5) and it goes in the pull request body. If either row passes here, STOP: the test is not proving what it claims, and the likely cause is a fixture outside the skew window.

- [ ] **Step 4: Write the implementation**

Two edits in `ts/packages/paigasus-auth/src/core/single-flight.ts`.

Line 2, replace:

```ts
import { RefreshRejected } from './errors';
```

with:

```ts
import { isRefreshRejected } from './errors';
```

Line 170, replace:

```ts
          const rejected = err instanceof RefreshRejected;
```

with:

```ts
          // NOT `instanceof` (SMA-657). `refresh` delegates to the shared `runtime.oidc`
          // (next/get-session.ts), so this error was built by whichever copy of this package
          // created the runtime, while this line runs in whichever copy serves the request. See
          // hasAuthErrorCode in core/errors.ts.
          const rejected = isRefreshRejected(err);
```

Line 170 is the only use of `RefreshRejected` in this file, so nothing else needs changing. Leave
the long explanatory comment above it (lines 143-169) exactly as it is.

- [ ] **Step 5: Run both tests to verify they PASS**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-auth && pnpm exec vitest run tests/core/single-flight.test.ts tests/next/get-session.test.ts
```

Expected: PASS, every row in both files.

- [ ] **Step 6: Typecheck**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-auth && pnpm exec tsc --noEmit -p tsconfig.json
```

Expected: no errors. This catches an orphaned `RefreshRejected` import.

- [ ] **Step 7: Commit**

```bash
git add ts/packages/paigasus-auth/src/core/single-flight.ts ts/packages/paigasus-auth/tests/core/single-flight.test.ts ts/packages/paigasus-auth/tests/next/get-session.test.ts
git commit -m "$(cat <<'EOF'
fix(ts): classify RefreshRejected by code, not instanceof (SMA-657)

Next 16 gives a route handler and a page separate copies of this package
while the AuthRuntime is shared through globalThis, so the OIDC adapter
throws the class of the copy that built the runtime. `instanceof` in the
other copy is false, and a revoked session degraded instead of signing
the user out.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 3: The newly reachable delete failure

The `if (rejected)` branch at `single-flight.ts:178-181` runs today only in the copy that built the runtime. Task 2 makes it run in both, and no test covers a failure of the `store.delete` inside it. That delete goes through the SMA-651 deadline decorator and can throw `SessionStoreTimeout`, which replaces the `RefreshRejected` before the `throw err` at line 182.

This task adds a test and changes no source. It pins the current outcome rather than altering it (spec § 7).

**Files:**
- Test: `ts/packages/paigasus-auth/tests/core/single-flight.test.ts`

**Interfaces:**
- Consumes: nothing from Task 2 beyond the landed fix.
- Produces: nothing.

- [ ] **Step 1: Write the test**

Add this row to the same `describe('a failing refresh (SMA-626 § 2.3)', …)` block, after the row
Task 2 added.

Two imports are needed. Extend that file's existing `../../src/core/errors.js` import to read:

```ts
import { RefreshRejected, SessionStoreTimeout } from '../../src/core/errors.js';
```

and add a new import for the store helper this package already uses for exactly this shape:

```ts
import { failingStore, type StoreMethod } from '../support/store-failure.js';
```

`failingStore(inner, failOn, makeError, calls)` wraps a real store, rejects on the named methods,
passes every other method through, and records `method:arg` for each guarded call. Use it rather
than hand-building an object: spreading a `MemorySessionStore` instance does NOT copy its prototype
methods, and a hand-written literal must satisfy all eight `SessionStore` members or it fails
typecheck.

Then add:

```ts
  // SMA-657 § 7. The `if (rejected)` delete became reachable from BOTH module copies with this
  // fix, and that delete goes through the SMA-651 deadline decorator. A store that fails there
  // REPLACES the RefreshRejected as the thrown value, so `session.deleted` never fires and the
  // record survives for its TTL. That outcome is accepted, not fixed: getSession returns null on
  // any throw, so the user is still signed out, and the next request retries the delete. This row
  // exists so the accepted outcome is pinned rather than discovered later as a surprise.
  it('a failing delete on the rejection path replaces the error and leaves the record', async () => {
    const inner = new MemorySessionStore();
    await inner.set('s', makeRecord({ accessExpiresAt: Date.now() + 30_000 }), 60_000, null);
    const calls: string[] = [];
    const store = failingStore(inner, new Set<StoreMethod>(['delete']), () => new SessionStoreTimeout('delete', 4000, 'deadline'), calls);
    const { logger, events } = recordingLogger();

    await expect(resolveSession({ ...deps(store, rejected), logger, skewMs: 60_000 }, 's')).rejects.toBeInstanceOf(SessionStoreTimeout);
    // The classification still happened and was logged...
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'rejected', degraded: false }]);
    // ...the delete WAS attempted...
    expect(calls).toContain('delete:s');
    // ...but it did not land, so there is no session.deleted and the record survives.
    expect(events.some(([name]) => name === 'session.deleted')).toBe(false);
    expect(await inner.get('s')).not.toBeNull();
  });
```

- [ ] **Step 2: Run the test**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-auth && pnpm exec vitest run tests/core/single-flight.test.ts
```

Expected: PASS. This row pins existing behaviour, so it passes on the first run — that is correct
here and is NOT a red-first violation, because it is documenting an accepted outcome rather than
driving a change. If it FAILS, the real behaviour differs from the spec's § 7 claim: stop and
report which of the four assertions failed, because the spec would then be wrong.

- [ ] **Step 3: Commit**

```bash
git add ts/packages/paigasus-auth/tests/core/single-flight.test.ts
git commit -m "$(cat <<'EOF'
test(ts): pin the delete-failure outcome on the rejection path (SMA-657)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 4: Record the audit in the code

Comments only. No behaviour changes and no new tests. This is the issue's third acceptance criterion: every other `instanceof` was searched, and each one found is fixed or recorded.

**Files:**
- Modify: `ts/packages/paigasus-auth/src/server.ts` (near line 111)
- Modify: `ts/packages/paigasus-auth/src/adapters/operation-deadline.ts` (near line 73)
- Modify: `ts/packages/paigasus-auth/src/adapters/oidc.ts` (near line 153)
- Modify: `ts/packages/paigasus-auth/src/core/errors.ts` (the `SessionStoreTimeout` doc comment)

**Interfaces:**
- Consumes: nothing.
- Produces: nothing.

- [ ] **Step 1: Comment the `server.ts` site**

In `ts/packages/paigasus-auth/src/server.ts`, insert directly above the line
`      if (!(err instanceof CallbackRejected)) throw err;`:

```ts
      // `instanceof` is SAFE here, unlike core/single-flight.ts's (SMA-657). This catch lives
      // inside the `handle` function createAuthRouteHandler RETURNED, closing over the `routes`
      // object built above — so the copy of this package that built the handler is the copy that
      // throws at http/routes.ts's `reject()` and the copy that catches here. Verified for all
      // three consumers: both apps' app/auth/[...auth]/route.ts and the e2e fixture server each
      // call createAuthRouteHandler from their own module and never pass a handler across a
      // boundary.
```

- [ ] **Step 2: Comment the `operation-deadline.ts` site**

In `ts/packages/paigasus-auth/src/adapters/operation-deadline.ts`, extend the existing comment
directly above `if (error instanceof SessionStoreTimeout && error.phase === 'deadline') {`. That
comment currently reads:

```ts
      // Only a DEADLINE expiry opens the circuit. A fast failure (ClientOfflineError during a
      // reconnect) passes through and leaves the circuit as it is.
```

Append to it:

```ts
      //
      // `instanceof` is SAFE here, unlike core/single-flight.ts's (SMA-657). Both throw sites and
      // this catch are inside the SAME `bounded` closure, built once by one copy of this package.
      // The returned store is shared to the other copy through the runtime, and calling it runs
      // this closure verbatim — it never re-resolves SessionStoreTimeout against the other copy's
      // module registry.
```

- [ ] **Step 3: Comment the `oidc.ts` site**

In `ts/packages/paigasus-auth/src/adapters/oidc.ts`, insert directly above the line
`  if (cause instanceof client.ResponseBodyError && cause.error === 'invalid_grant') {`:

```ts
  // `instanceof` is SAFE here, unlike core/single-flight.ts's (SMA-657), and it stays safe even
  // though openid-client may itself be duplicated across Next's two module graphs: the
  // client.refreshTokenGrant call that can throw this and this check are both inside the closure
  // createOidcClient returned, so both see one `client` binding. It is this function's OUTPUT that
  // crosses copies, as RefreshRejected — which is exactly why that one needs a code check.
```

- [ ] **Step 4: Add the cross-reference the three comments now earn**

Task 1 deliberately left this sentence OUT of `core/errors.ts`, because it was not yet true: the
three comments did not exist. Steps 1-3 just created them, so add it now. In
`ts/packages/paigasus-auth/src/core/errors.ts`, find this line in the `hasAuthErrorCode` doc
comment:

```
 * — the boundary is the CLOSURE, not whether state is shared.
```

and extend it to:

```
 * — the boundary is the CLOSURE, not whether state is shared. The three remaining `instanceof`
 * sites in this package each carry a comment saying which side of that line they fall on.
```

- [ ] **Step 5: Correct the `SessionStoreTimeout` doc comment**

In `ts/packages/paigasus-auth/src/core/errors.ts`, that class's doc comment currently says:

```
 * A SUBCLASS of SessionStoreUnavailable, deliberately: every caller that classifies on that class
 * (next/get-session.ts, core/single-flight.ts's release-time catch) keeps working unchanged, and
```

Both halves of the parenthesis are wrong. `core/single-flight.ts`'s release-time catch is a bare
`catch` that logs unconditionally and classifies nothing, and since SMA-653 the principal
classifier is `http/store-unavailable.ts`'s `storeStep`, reached from every store call in
`http/routes.ts`. Replace those two lines with:

```
 * A SUBCLASS of SessionStoreUnavailable, deliberately: every caller that classifies on that class
 * (http/store-unavailable.ts's storeStep, reached from every store call in http/routes.ts, and
 * next/get-session.ts) keeps working unchanged, and
```

- [ ] **Step 6: Verify nothing changed behaviourally**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-auth && pnpm exec tsc --noEmit -p tsconfig.json && pnpm exec vitest run
```

Expected: typecheck clean, every test passes. These are comments; a failure means an edit landed
inside a statement.

- [ ] **Step 7: Commit**

```bash
git add ts/packages/paigasus-auth/src/server.ts ts/packages/paigasus-auth/src/adapters/operation-deadline.ts ts/packages/paigasus-auth/src/adapters/oidc.ts ts/packages/paigasus-auth/src/core/errors.ts
git commit -m "$(cat <<'EOF'
docs(ts): record why the three remaining instanceof sites are safe (SMA-657)

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
)"
```

---

### Task 5: Mutation check and the full gate

Proves the new tests bite, then runs what CI runs. A test that passes only because the code did not exist yet is not proof.

**Files:** none changed permanently. Every mutation below is reverted before the next one.

**Interfaces:**
- Consumes: everything from Tasks 1-4.
- Produces: the evidence recorded in the pull request body.

- [ ] **Step 1: Mutation A — revert the classification**

Restore BOTH lines in `src/core/single-flight.ts`: line 2 back to
`import { RefreshRejected } from './errors';` and line 170 back to
`const rejected = err instanceof RefreshRejected;` (keep the new comment or delete it, either is
fine). Reverting only line 170 leaves the file failing TYPECHECK on an unused-and-missing import,
and a compile error is not the same evidence as a red test.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-auth && pnpm exec vitest run tests/core/single-flight.test.ts tests/next/get-session.test.ts
```

Expected: the two rows Task 2 added FAIL. Record which. Then revert the mutation with
`git checkout -- ts/packages/paigasus-auth/src/core/single-flight.ts`.

- [ ] **Step 2: Mutation B — remove the builtin-Error guard**

In `src/core/errors.ts`, change `hasAuthErrorCode`'s body to
`return (err as { code?: unknown })?.code === code;`.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-auth && pnpm exec vitest run tests/core/errors.test.ts
```

Expected: FAIL — the plain-object row of `isRefreshRejected` and the equivalent
`isSessionStoreUnavailable` row. Revert with `git checkout -- ts/packages/paigasus-auth/src/core/errors.ts`.

- [ ] **Step 3: Mutation C — wrong code literal**

In `src/core/errors.ts`, change `isRefreshRejected`'s literal to `'oidc_refresh_rejected_x'`.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-auth && pnpm exec vitest run tests/core/errors.test.ts tests/core/single-flight.test.ts tests/next/get-session.test.ts
```

Expected: FAIL in all three files. Revert.

- [ ] **Step 4: Mutation D — break the D3 move**

In `src/core/errors.ts`, change `isSessionStoreUnavailable` to pass `'oidc_refresh_rejected'`.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-auth && pnpm exec vitest run tests/core/errors.test.ts tests/http/store-unavailable.test.ts tests/next/get-session.test.ts
```

Expected: FAIL in all three — the three suites D3 names. Revert.

- [ ] **Step 5: Confirm the tree is clean after the mutations**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-657-refresh-rejected-code
git status --short
```

Expected: EMPTY. A non-empty result means a mutation survived — revert it before continuing. Do not
use a bare `git stash`; other sessions share the stash stack.

- [ ] **Step 6: Run the package's own gates**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-657-refresh-rejected-code
moon run paigasus-auth-ts:test ts:lint ts:fmt paigasus-auth-ts:typecheck
```

Expected: all pass. `ts:fmt` is a separate repo-wide Prettier gate and is easy to forget after
editing four source files. If `ts:fmt` fails, run `pnpm -C ts exec prettier --write` on the files it
names, then re-run.

Do NOT run `paigasus-auth-ts:test-e2e`: it is Docker-backed and this change does not touch its
surface.

- [ ] **Step 7: Run the affected graph the way CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-657-refresh-rejected-code
moon ci :build :test :lint :fmt :typecheck --base origin/main --include-relations
```

Expected: pass. Known local traps, none of which is a finding about this change:

- A failure whose `stdout.log` is EMPTY with a one-line `declare: -A: invalid option` or
  `mapfile: command not found` on stderr is the wrong bash, not a defect. Check `which -a bash`.
- A `repo:actionlint` rc 2 saying a pipe "holds only 512 bytes" is a host condition (SMA-612).
- A sub-3s `repo:affected-smoke` failure: capture the full task output BEFORE re-running, because a
  re-run passes and destroys the evidence.

If a task fails for a reason you cannot attribute, follow the diagnosis procedure in CLAUDE.md
(copy `.moon/cache` artifacts BEFORE re-running anything).

- [ ] **Step 8: No commit**

This task changes no files. Report the mutation results — which rows red for each of A, B, C and D
— and the gate results. They go in the pull request body together with Task 2 Step 3's red-first
output.

---

## Self-Review

**Spec coverage.** Every spec section maps to a task: D1/D2/D3/D8 → Task 1; the § 4 item 2 and item
3 tests and the line-170 fix → Task 2; § 4 item 4 and § 7's accepted residual → Task 3; D5, § 3 and
§ 5 → Task 4; § 4 items 5 and 6 → Task 5. D4 (do not re-export) and D6 (no Next probe, no e2e) are
constraints rather than work, and both are stated in Global Constraints and in Task 5 Step 6.

**Placeholder scan.** No TBD, TODO, "similar to Task N", or "add appropriate error handling". Every
code step carries the literal code.

**Type consistency.** `hasAuthErrorCode(err: unknown, code: string): boolean` is defined in Task 1
and used only there. `isRefreshRejected(err: unknown): boolean` is defined in Task 1 and used in
Task 2's `single-flight.ts` edit. The event names and field shapes in Tasks 2 and 3
(`session.refresh_failed` with `reason`/`degraded`, `session.deleted` with `reason`) are copied from
the existing rows in the same files, not invented.
