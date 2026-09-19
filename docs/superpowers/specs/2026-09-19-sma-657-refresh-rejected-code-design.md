# SMA-657: classify RefreshRejected by its code, not by instanceof

- Issue: SMA-657 (found in the SMA-653 spec challenge, § 9 follow-up B).
- Related: SMA-653 (the same shape for `SessionStoreUnavailable`), SMA-626 § 2.3 (the three refresh
  outcomes), SMA-511 Task 22 (the measured module duplication).
- Package: `ts/packages/paigasus-auth` (`@paigasus/auth`).
- Revision 2: folds in the spec challenge. Every finding it raised was verified against the code
  before it was accepted; § 9 lists what changed.

## 1. Problem

`src/core/single-flight.ts:170` classifies a failed token refresh with `err instanceof
RefreshRejected`. That check decides between two outcomes, recorded in SMA-626 § 2.3:

- `true` means the identity provider REFUSED the refresh token (`invalid_grant`). The session is
  revoked, so `resolveSession` deletes the record and re-throws.
- `false` means a transient failure. With a still-live access token the caller degrades and keeps
  working.

Next 16.3.4 gives a route handler and a page SEPARATE module graphs. That is MEASURED, on this
repository's own standalone build, and recorded in `src/runtime.ts:175-198` on `RUNTIME_KEY_PREFIX`.
The `AuthRuntime` is shared between the two copies through `globalThis`, so that both layers see one
session store.

The consequence for this check:

- `src/next/get-session.ts:56` wires the refresh dependency as
  `refresh: (refreshToken) => runtime.oidc.refresh(refreshToken)`. It does not build a local OIDC
  client. It forwards to `runtime.oidc`.
- `runtime.oidc` is the object `createOidcClient()` built inside whichever copy called
  `getAuthRuntime` first — call it copy A. Its methods are closures over copy A's own
  `import { RefreshRejected } from '../core/errors'` (`src/adapters/oidc.ts:35`).
- `src/adapters/oidc.ts:154` builds the error with copy A's class:
  `return new RefreshRejected('invalid_grant');`.
- The other layer, copy B, calls `getAuthRuntime`, receives copy A's runtime, and runs copy B's own
  `resolveSession`. The catch at `single-flight.ts:143` therefore tests copy A's instance against
  copy B's class, imported at `single-flight.ts:2`.

`instanceof` is then `false`, and a revoked session is treated as a transient failure. What that
costs depends on the access token, and the two cases are NOT the same:

- **A live access token (inside the skew window).** `degraded` becomes `true`.
  `resolveSession` returns the record with `refreshState: 'failed'`, and the user keeps working on a
  token belonging to a session an administrator ended. This is the user-visible defect.
- **A hard-expired access token.** `degraded` is `false` either way, so `resolveSession` throws
  either way, `get-session.ts:83` returns `null`, and the user IS signed out today. What differs is
  that the record is NOT deleted: it survives for the full `ttlMs`, every later read re-takes the
  lock and re-calls the token endpoint, and the log says `reason: 'transient'` with no
  `session.deleted`. An operator then cannot tell a revocation from an outage.

**What is measured and what is not.** The module duplication is measured (`src/runtime.ts`). That
one class identity differs between two module copies follows from JavaScript semantics, not from
Next behaviour. This specification does NOT measure the failure inside a real Next build; § 4
explains why the second-copy unit test is the honest control and what it does not prove.

SMA-653 fixed the identical shape for `SessionStoreUnavailable` with a `code`-based predicate
(`isSessionStoreUnavailable`, `src/core/errors.ts:54`) and recorded this site as follow-up B.

## 2. Decisions

- **D1. `RefreshRejected` is classified by its `code`.** A new function in `src/core/errors.ts`:

  ```ts
  export function isRefreshRejected(err: unknown): boolean {
    return hasAuthErrorCode(err, 'oidc_refresh_rejected');
  }
  ```

  `single-flight.ts:170` becomes `const rejected = isRefreshRejected(err);`, and its line 2 imports
  `isRefreshRejected` in place of `RefreshRejected`. Line 170 is that class's ONLY use in the file,
  so the import swaps with no remainder. `code` is a class field, so it is an own property of every
  instance and survives the duplication.

- **D2. The predicate returns a plain `boolean`, never a type guard `err is RefreshRejected`.**
  Across two copies the value genuinely is NOT an instance of the local class, so a guard would
  assert something false. The lie is reachable at this exact call site, not hypothetical: line 170
  assigns to a `const` and line 178 tests it, so TypeScript's aliased-predicate narrowing WOULD
  narrow `err` inside that branch and let a later edit read `err.oauthError` off a foreign value.
  The call site needs a flag, not a narrowed type. `isSessionStoreUnavailable` already returns
  `boolean` (`core/errors.ts:54`).

- **D3. Both predicates share one internal helper.**

  ```ts
  function hasAuthErrorCode(err: unknown, code: string): boolean {
    return err instanceof Error && (err as { code?: unknown }).code === code;
  }
  ```

  `isSessionStoreUnavailable` moves onto it. Its behaviour does not change. THREE existing tests
  keep the move honest — `tests/core/errors.test.ts:17`, `tests/http/store-unavailable.test.ts:112`
  and `tests/next/get-session.test.ts:227` — and nothing pins the function's body: `tests/structure/`
  pins the package export map and the import graph, not `core/errors.ts`'s symbol list, and no `ci/`
  script mentions the predicate. The rule then exists in one place and a third predicate cannot
  drift from it. The helper is NOT exported: the code literal belongs beside the class that declares
  it, so a call site never spells one out.

  The helper keeps the `err instanceof Error` test. That is an `instanceof` against a BUILTIN, which
  both copies share in one isolate, so it does not have the defect this issue fixes. It rejects a
  plain object carrying a matching `code`, which the existing test asserts.

- **D4. `isRefreshRejected` is not re-exported from `src/server.ts`.** Not because no consumer can
  PRODUCE a `RefreshRejected` — `createOidcClient` is exported (`server.ts:156`), so one can — but
  because no consumer can CLASSIFY one: the class itself is absent from `server.ts`'s export list,
  and `getSession()` swallows every failure into `null`. Exporting a predicate for a class nobody
  can name would add surface with no use. § 7 records the asymmetry this leaves for
  `SessionStoreUnavailable`, which IS exported while its predicate is not.

- **D5. The three other package-class `instanceof` sites are recorded in a comment, not changed and
  not gated.** § 3 gives the audit. All three are safe by construction. A lint rule that banned
  `instanceof` against a class from `core/errors` would report those correct lines, need disable
  comments, and teach the next author to add another disable instead of thinking. The comment at
  each site is what a future reader needs. § 7 states the residual this leaves.

- **D6. The evidence is a second-copy unit test.** § 4. No probe inside a real Next build, and no
  Keycloak e2e test. Reasons in § 4.

- **D7. The general rule, stated once so a future site can be judged against it.** A class thrown by
  a closure that is reachable through shared state, and caught OUTSIDE that closure, must be
  classified by its `code`. A class thrown and caught inside one closure, or thrown and caught by
  two modules of one copy, may use `instanceof`. The comment added beside `isRefreshRejected` states
  it. Note what the rule turns on: it is the CLOSURE boundary, not whether a runtime is shared. § 3
  shows three sites that share a runtime and are still safe, because the throw and the catch are
  inside one closure.

- **D8. The `code` check is deliberately WIDER than `instanceof`.** `hasAuthErrorCode` is true for
  ANY `Error` carrying that `code`, not only for a `RefreshRejected` from some copy. That is
  intended: `code` is the wire-stable identity, and widening is the whole mechanism by which the
  foreign-copy instance is admitted. It also means an unrelated `Error` that set
  `code = 'oidc_refresh_rejected'` would delete the session. Nothing in this package or its
  dependencies sets that literal, and the same widening is already accepted for
  `session_store_unavailable`. § 4 item 1 pins the semantics with a true-row rather than leaving
  them to be discovered.

## 3. The audit: every `instanceof` in `src/`

`src/` holds SIX `instanceof` operators. Three test a class this package defines, one a third-party
class, and two the builtin `Error`. Exactly one can cross the copies. This is the search the issue's
third acceptance criterion asks for.

| Site | Right side | Category | Crosses? | Why |
| --- | --- | --- | --- | --- |
| `core/single-flight.ts:170` | `RefreshRejected` | package | **YES** | `refresh` delegates to `runtime.oidc`, built by the copy that created the runtime. The catch is OUTSIDE that closure, in whichever copy serves the request. |
| `server.ts:111` | `CallbackRejected` | package | No | The catch lives inside the `handle` function that `createAuthRouteHandler` RETURNED (`server.ts:107-130`), and that function closes over the `routes` object built at `:102`. So the copy that built the handler is the copy that throws at `routes.ts:238` and the copy that catches. Verified for all three consumers: `ts/apps/iam-console/app/auth/[...auth]/route.ts:20`, `ts/apps/gateway-console/app/auth/[...auth]/route.ts:20`, and `tests/e2e/fixture-server.ts:140`. Each calls `createAuthRouteHandler` from its own module and never passes a handler across a boundary. `@paigasus/console-core` does not call it. |
| `adapters/operation-deadline.ts:73` | `SessionStoreTimeout` | package | No | Throw and catch are inside the same `bounded` closure, built once by one copy. A caller from the other copy runs that closure verbatim and does not re-resolve the class. (Two throw sites exist, `:52` circuit-open and `:65` deadline; only `:65` is matched by this catch, and both are inside the closure.) |
| `adapters/oidc.ts:153` | `client.ResponseBodyError` | third party | No | Same closure shape. `openid-client` may itself be duplicated across the two graphs, and it does not matter: `client.refreshTokenGrant` (`:269`) and this check are both inside the closure `createOidcClient()` returned, so both see one `client` binding. The classifier's OUTPUT is what crosses, as `RefreshRejected`. |
| `adapters/oidc.ts:126` | `Error` | builtin | No | One isolate, one builtin. |
| `core/errors.ts:55` | `Error` | builtin | No | One isolate, one builtin. This is the line D3 moves into `hasAuthErrorCode`. |

Which `AuthRuntime` fields can carry a copy-bound class at all (`src/runtime.ts:62-86`): `store`
(fixed by SMA-653), `oidc` (this issue), `resolver` and `logger`. Neither `resolver` nor `logger`
throws a package class that any site classifies — the rows above are the complete set. Every other
field holds plain data.

The release-time catch in `single-flight.ts:218-222` is a bare `catch` that logs unconditionally, so
it classifies nothing and is unaffected.

## 4. Tests

Vitest unit tests in `ts/packages/paigasus-auth/tests/`. No Docker. All of them run under
`paigasus-auth-ts:test`, because both files sit in that project's `@group(tests)`
(`ts/packages/paigasus-auth/moon.yml`). No app tier is affected. § 5 edits source files, so `ts:fmt`
(the separate repo-wide Prettier gate) and `ts:lint` apply to the change as a whole.

1. **`tests/core/errors.test.ts` — a new `describe('isRefreshRejected (SMA-657)')`,** mirroring the
   existing `isSessionStoreUnavailable` block:
   - true for a locally built `new RefreshRejected('invalid_grant')`;
   - true for one built from a SECOND copy of `src/core/errors.js`, loaded with `vi.resetModules()`
     and a dynamic `import`. The row asserts the precondition
     `expect(foreign.RefreshRejected).not.toBe(RefreshRejected)` FIRST, then
     `expect(err instanceof RefreshRejected).toBe(false)`, then the predicate is `true`. Without the
     precondition the row passes vacuously if the import returns the same module;
   - **true** for `Object.assign(new Error('x'), { code: 'oidc_refresh_rejected' })`, which pins D8's
     intended widening;
   - false for a `CallbackRejected`, for a `SessionStoreUnavailable`, for
     `new Error('oidc_refresh_rejected')` (the literal in the MESSAGE, not in `code`), for
     `{ code: 'oidc_refresh_rejected' }` (a plain object, not an `Error`), and for `undefined`.

2. **`tests/core/single-flight.test.ts` — the load-bearing test,** added to the
   `'a failing refresh (SMA-626 § 2.3)'` block. It is the only test that reds if line 170 reverts.

   It calls `vi.resetModules()`, dynamically imports a second `src/core/errors.js`, asserts the same
   precondition, and drives `resolveSession` with
   `refresh: () => Promise.reject(new foreign.RefreshRejected('invalid_grant'))`. `resolveSession`
   was statically imported, so it keeps its first-copy binding: this is the real two-copy shape, not
   a simulation of it.

   **The fixture must both be inside the skew window and still be live**, or `single-flight.ts:108`
   returns early and no refresh is attempted at all. `shouldRefresh` is `now >= expiresAt - skewMs`
   (`core/refresh-policy.ts:11`), and `makeRecord`'s default `accessExpiresAt` is
   `Date.now() + 300_000` against `deps()`'s default `skewMs: 30_000`, which does NOT refresh. Use
   `accessExpiresAt: Date.now() + 30_000` with `skewMs: 60_000`, the same shape as the existing row
   at `tests/core/single-flight.test.ts:481`. The live token is the right choice because it is the
   case where the fix changes the RETURN path, not only the log (§ 1).

   `resolveSession` REJECTS on this path (`single-flight.ts:182`), so the test must await a
   rejection. The matcher must NOT be `rejects.toBeInstanceOf(RefreshRejected)`, which the
   neighbouring row at `:486` uses: that is false for a foreign-copy error, which is the entire
   point of the issue, and the test would then red before AND after the fix. Use
   `rejects.toBeInstanceOf(foreign.RefreshRejected)`, or identity against the thrown value. Do not
   loosen it to a bare `rejects.toThrow()`.

   Assertions: the rejection above; `session.refresh_failed` with `reason: 'rejected'` and
   `degraded: false`; `session.deleted` with `reason: 'refresh_rejected'`; and
   `expect(await store.get('s')).toBeNull()`, which is the idiom every existing row in that block
   uses (`:490`, `:501`) rather than a spy on `delete`.

   **The same-copy counterpart already exists** and needs no duplicate: `'never degrades a
   definitive rejection, even with a live access token'` (`:481-491`) has the same fixture and the
   same assertions. The new row is named so that the pair reads as one before/after contrast.

3. **`tests/next/get-session.test.ts` — the end-to-end half.** A row mirroring the same-copy
   rejection row at `:239`, but with a foreign-copy `RefreshRejected` and a LIVE access token. It
   asserts `getSession` resolves to `null`. This is the only unit-level place the user-visible half
   of the fix is observable: before the fix `resolveSession` degrades and `getSession` returns a
   session with `refreshState: 'failed'`; after it, the call throws and `getSession` returns `null`.
   With a hard-expired token the row would pass either way (§ 1), so the live token is required.

4. **A throwing `store.delete` on the rejection path.** The `if (rejected)` branch
   (`single-flight.ts:178-181`) runs today only in the copy that built the runtime; the fix makes it
   run in both, and no test covers a failure there. `store.delete` goes through the SMA-651
   decorator and can throw `SessionStoreTimeout`, which REPLACES the `RefreshRejected` before
   `throw err` at `:182`. A row with a `delete` that rejects asserts the resulting behaviour:
   `session.refresh_failed` with `reason: 'rejected'` still fires, `session.deleted` does NOT, the
   record survives, and the call rejects with the store error. § 7 records the consequence as an
   accepted residual rather than changing it.

5. **Red first, then mutation.** Items 2 and 3 run against the UNFIXED code and must fail; the
   failures are recorded in the pull request. A test that passes only because the code did not exist
   yet is not proof.

6. **Mutation check.** Each item below is reverted one at a time, and at least one test must red:
   - `single-flight.ts:170` returned to `err instanceof RefreshRejected` **with its line-2 import of
     `RefreshRejected` restored** — without the import the file fails typecheck, and a compile error
     is not the same evidence as a red test (items 2 and 3);
   - `hasAuthErrorCode`'s `err instanceof Error` test removed (item 1's plain-object row, and the
     existing `isSessionStoreUnavailable` rows);
   - the `code` literal in `isRefreshRejected` changed (item 1);
   - `isSessionStoreUnavailable` rewired to the wrong literal by the D3 move (its three existing
     rows, listed in D3).

**What these tests do NOT prove.** They prove the classification survives two module copies in one
V8 isolate. They do not prove that Next 16.3.4 puts the route handler and the page in two copies —
`src/runtime.ts` measured that separately, and this specification relies on that measurement. They
do not prove the browser-level sign-out, which no unit test reaches.

**Rejected: a probe inside a real Next build.** It would build an app-level fixture and a throwaway
route in `iam-console` to show two different `RefreshRejected` identities. It would measure V8 and
Turbopack, which `src/runtime.ts` already measured for this module, and it would leave a fixture
nobody maintains. **Rejected: a Keycloak e2e revocation test.** The e2e tier runs one process and
cannot force the two-copy split, so the test would pass before and after the fix.

## 5. Documentation changes

- The doc comment above `isRefreshRejected` states D7's rule and names the `runtime.oidc` path that
  makes this site cross.
- A comment at each of the three safe package-class sites (`server.ts:111`,
  `operation-deadline.ts:73`) and at `oidc.ts:153` states why it cannot cross, in one or two lines,
  using § 3's reasons. Each gives the CLOSURE argument, not a weaker one.
- The `SessionStoreTimeout` doc comment (`core/errors.ts:21`) is corrected in BOTH halves. It names
  "core/single-flight.ts's release-time catch" as a caller that classifies on that class, and that
  catch is bare and classifies nothing; and it names only `next/get-session.ts` as the other caller,
  while since SMA-653 the principal classifier is `http/store-unavailable.ts:102` (`storeStep`),
  reached from every store call in `routes.ts`.
- No CLAUDE.md change. The rule lives beside the code it governs, and CLAUDE.md already records the
  module duplication through the SMA-511 entries.

## 6. Assumptions

- The two module copies run in ONE V8 isolate and share the builtin `Error`, so
  `err instanceof Error` in `hasAuthErrorCode` holds for an error from either copy. This is what
  makes the shipped `isSessionStoreUnavailable` work, and its second-copy test already proves it.
- `vi.resetModules()` plus a dynamic import produces a genuinely distinct module instance under this
  package's vitest configuration. The existing `errors.test.ts` row proves it today, and every new
  row asserts it again rather than assuming it. Note the call mutates the module registry for the
  REST of the file; no other dynamic import exists in `single-flight.test.ts` or
  `get-session.test.ts` today, so there is no live interaction. That file uses REAL timers
  throughout (`single-flight.test.ts:19-22`) and no `vi.mock`, and `vitest.config.ts` sets no
  `setupFiles`, no `isolate: false` and no `restoreMocks`, so there is no timer or mock-hoisting
  interaction either.
- No consumer classifies a `RefreshRejected` (D4).

## 7. Out of scope, and the residuals

- **`@paigasus/console-core` is NOT audited here, and it is not obviously safe.** An earlier draft
  said its runtime is built at module scope per copy; that is FALSE.
  `ts/packages/paigasus-console-core/src/discovery.ts:57-65` holds its descriptor cache and Redis
  client on `globalThis` under `Symbol.for('paigasus.console-core.discovery-state.v1')`, and its own
  comment says it copies `@paigasus/auth`'s `getAuthRuntime` for the same reason. So console-core
  uses the exact mechanism this issue is about. Its `discovery.ts:274`
  (`DescriptorCacheTimeoutError`) is safe by the closure argument — thrown at `:258`/`:269` and
  caught at `:274`, all inside the `bounded` closure at `:248`. Its `errors.ts:23`
  (`err instanceof ConnectError`, inside `callIam`) classifies a third-party class on a client that
  comes from `createConsoleRuntime` and deserves its own audit, which this issue does not perform.
  A follow-up issue should carry it.
- **No automated gate (D5). Residual, stated plainly:** nothing reds when a future site classifies a
  package error class with `instanceof` across the copies. The comments of § 5 and this document are
  the only control. A gate would need to tell a crossing site from a non-crossing one, which needs
  the call-graph reasoning of § 3, not a syntactic rule.
- **An exported class with no exported predicate.** `SessionStoreUnavailable` is exported
  (`server.ts:141`) while `isSessionStoreUnavailable` is not, so a consumer holding one has no
  supported way to classify it across copies and would reach for `instanceof`. That predates this
  issue and belongs to SMA-653's surface, not this one. Recorded here so it is not lost.
- **A `store.delete` failure on the rejection path is accepted, not fixed** (§ 4 item 4). The sign-out
  still happens, because `getSession` returns `null` on any throw. The record survives and the next
  request retries the delete.
- **The `adapters/oidc.ts` classifier is untouched.** This issue changes how the result is READ,
  never what is produced. In particular the residual its own comment records at `:147-150` stands:
  an `invalid_grant` response that also carries a `WWW-Authenticate` header arrives as
  `WWWAuthenticateChallengeError`, has no `.error` field, and still falls through to transient. This
  fix does not widen what produces a `RefreshRejected`.

## 8. Rejected alternatives

- **A `Symbol.for`-keyed brand on `AuthError`.** It would work across copies the way
  `RUNTIME_KEY_PREFIX` does, and it adds a second identity mechanism beside `code`, which already
  exists, is already a field of every error, and is already the pattern SMA-653 set. Two mechanisms
  would then have to agree.
- **Making `single-flight.ts` import the class from the runtime.** It would remove the duplication
  at this one site and would make `core/` depend on the adapter layer, which SMA-626 § 2.3
  deliberately forbids — the whole reason `RefreshRejected` lives in `core/errors.ts` is so that
  `core/` can classify without importing `adapters/`.
- **A type guard `err is RefreshRejected`** (D2).
- **Duplicating the one-line predicate instead of D3's shared helper.** For two predicates the
  saving is small. The cost is that the rule then exists twice and a third copy can differ from both.
- **A lint gate** (D5).

## 9. What the spec challenge changed

Every finding below was verified against the code before it was accepted. None was rejected.

- **Blocker.** § 4 item 2 did not say that `resolveSession` rejects, and the obvious matcher
  (`rejects.toBeInstanceOf(RefreshRejected)`, copied from the neighbouring row) is FALSE for a
  foreign-copy error, so the test could not have gone green. Item 2 now names the matcher and
  forbids loosening it.
- § 1 claimed the hard-expired case changes from "re-throws" to "signs out". Wrong:
  `get-session.ts:83` returns `null` today, so the user is already signed out there. § 1 now
  separates the live-token case (the real defect) from the expired case (a lost delete and a
  misleading log).
- § 4 item 2's fixture would never have reached the catch: `makeRecord`'s default
  `accessExpiresAt: Date.now() + 300_000` against `skewMs: 30_000` does not refresh at all. Item 2
  now gives the exact numbers.
- § 4 item 2 asked for a same-copy "second row" that already ships at `single-flight.test.ts:481`.
  It now names the existing test instead.
- § 7 said console-core builds its runtime per copy. False — `discovery.ts:57-65` uses `globalThis`.
  § 7 is rewritten, its incomplete `instanceof` enumeration is dropped, and `errors.ts:23` is named
  as needing its own audit.
- § 3 said "exactly four `instanceof` checks on a class this package defines" and then listed a
  third-party class as the fourth. It is now six operators in three categories.
- The `server.ts:111` row gave a weaker reason than the real one. It now gives the closure argument
  and names the three verified consumers.
- D8 is new: the `code` check is wider than `instanceof`, that is intended, and § 4 item 1 now pins
  it with a true-row.
- D4's "no consumer can produce one" was too strong — `createOidcClient` is exported. Restated as
  "cannot classify one", with the `SessionStoreUnavailable` asymmetry recorded in § 7.
- Mutation 1 would not have compiled, because D1 removes the import it needs. It now restores both
  lines.
- New § 4 items 3 and 4: a `getSession`-level foreign-copy row, and a throwing `store.delete` on the
  newly reachable rejection path.
- D3 cited one test; there are three. § 4 now names the Moon task and the `ts:fmt`/`ts:lint` gates.
  Citation slips fixed (`oidc.ts:154`, not `:153`; two throw sites in `operation-deadline.ts`).
