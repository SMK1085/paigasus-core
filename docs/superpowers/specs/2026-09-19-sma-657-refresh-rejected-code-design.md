# SMA-657: classify RefreshRejected by its code, not by instanceof

- Issue: SMA-657 (found in the SMA-653 spec challenge, § 9 follow-up B).
- Related: SMA-653 (the same shape for `SessionStoreUnavailable`), SMA-626 § 2.3 (the three refresh
  outcomes), SMA-511 Task 22 (the measured module duplication).
- Package: `ts/packages/paigasus-auth` (`@paigasus/auth`).

## 1. Problem

`src/core/single-flight.ts:170` classifies a failed token refresh with `err instanceof
RefreshRejected`. That check decides between two very different outcomes, recorded in SMA-626 § 2.3:

- `true` means the identity provider REFUSED the refresh token (`invalid_grant`). The session is
  revoked. `resolveSession` deletes the record and the user is signed out.
- `false` means a transient failure. With a still-live access token the caller degrades and keeps
  working.

Next 16.3.4 gives a route handler and a page SEPARATE module graphs. That is MEASURED, on this
repository's own standalone build, and recorded in `src/runtime.ts` on `RUNTIME_KEY_PREFIX`. The
`AuthRuntime` is shared between the two copies through `globalThis`, precisely so that both layers
see one session store.

The consequence for this check:

- `src/next/get-session.ts:56` wires the refresh dependency as
  `refresh: (refreshToken) => runtime.oidc.refresh(refreshToken)`. It does not build a local OIDC
  client. It forwards to `runtime.oidc`.
- `runtime.oidc` is the object `createOidcClient()` built inside whichever copy called
  `getAuthRuntime` first — call it copy A. Its methods are closures over copy A's own
  `import { RefreshRejected } from '../core/errors'` (`src/adapters/oidc.ts:35`).
- `src/adapters/oidc.ts:153-157` builds the error with copy A's class:
  `return new RefreshRejected('invalid_grant');`.
- The other layer, copy B, calls `getAuthRuntime`, receives copy A's runtime, and runs copy B's own
  `resolveSession`. The catch at `single-flight.ts:143` therefore tests copy A's instance against
  copy B's class, imported at `single-flight.ts:2`.

`instanceof` is then `false`. A revoked session is logged as `reason: 'transient'`, the record is
NOT deleted, and the user is not signed out. With a live access token they keep working until it
expires. With an expired one the error re-throws instead of signing them out cleanly.

**What is measured and what is not.** The module duplication is measured (`src/runtime.ts`). That
one class identity differs between two module copies follows from JavaScript semantics, not from
Next behaviour. This specification does NOT measure the failure inside a real Next build; § 4
explains why the second-copy unit test is the honest control and what it does not prove.

SMA-653 fixed the identical shape for `SessionStoreUnavailable` with a `code`-based predicate
(`isSessionStoreUnavailable`, `src/core/errors.ts`) and recorded this site as follow-up B.

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
  Across two copies the value genuinely is NOT an instance of the local class. A guard would assert
  something false and would let a later edit read `err.oauthError` off a value the compiler only
  believes is narrowed. `isSessionStoreUnavailable` already returns `boolean` for this reason. The
  call site needs a flag, not a narrowed type.

- **D3. Both predicates share one internal helper.**

  ```ts
  function hasAuthErrorCode(err: unknown, code: string): boolean {
    return err instanceof Error && (err as { code?: unknown }).code === code;
  }
  ```

  `isSessionStoreUnavailable` moves onto it. Its behaviour does not change, and its existing
  second-copy test in `tests/core/errors.test.ts` covers the move. The rule — "an `Error` whose own
  `code` property equals this literal" — then exists in one place, and a third predicate cannot
  drift from it. The helper is NOT exported: the code literal belongs beside the class that
  declares it, so a call site never spells one out.

  The helper keeps the `err instanceof Error` test. That is an `instanceof` against a BUILTIN, which
  every realm in one process shares, so it does not have the defect this issue fixes. It rejects a
  plain object carrying a matching `code`, which the existing test asserts.

- **D4. `isRefreshRejected` is not re-exported from `src/server.ts`.** `RefreshRejected` itself is
  not exported either, for the reason its doc comment records: no consumer can produce or observe
  one, because `getSession()` swallows every failure into `null` and `CreateAuthRuntimeDeps` exposes
  no OIDC override. `isSessionStoreUnavailable` is likewise absent from `server.ts:141`. The
  predicate is internal to the package.

- **D5. The three other `instanceof` sites are recorded in a comment, not changed and not gated.**
  § 3 gives the audit. All three are safe by construction. A lint rule that banned `instanceof`
  against a class from `core/errors` would report those three correct lines, need three disable
  comments, and teach the next author to add a fourth disable instead of thinking. The comment at
  each site is what a future reader needs. § 7 states the residual this leaves.

- **D6. The evidence is a second-copy unit test.** § 4. No probe inside a real Next build, and no
  Keycloak e2e test. Reasons in § 4.

- **D7. The general rule, stated once so a future site can be judged against it.** A class thrown by
  a closure that is reachable through the shared `AuthRuntime`, and caught OUTSIDE that closure,
  must be classified by its `code`. A class thrown and caught inside one closure, or thrown and
  caught by two modules of the same copy, may use `instanceof`. § 3 applies this rule to every site
  that exists today. The comment added beside `isRefreshRejected` states it.

## 3. The audit: every `instanceof` on a package class

`src/` holds exactly four `instanceof` checks whose right side is a class this package defines. One
can cross the copies. This is the search acceptance criterion 3 asks for.

| Site | Class | Crosses? | Why |
| --- | --- | --- | --- |
| `core/single-flight.ts:170` | `RefreshRejected` | **YES** | `refresh` delegates to `runtime.oidc`, built by the copy that created the runtime. The catch runs in whichever copy serves the request. |
| `server.ts:111` | `CallbackRejected` | No | `createAuthRouteHandler` calls `createAuthRoutes(runtime)` at `server.ts:102`, from a static import. The throw at `routes.ts:238` and this catch are always one copy. Construction depends on local control flow, never on a runtime field. |
| `adapters/operation-deadline.ts:73` | `SessionStoreTimeout` | No | The throw (line 65) and the catch (line 73) are inside the same `bounded` closure, built once by one copy. A caller from the other copy runs that closure verbatim; it does not re-resolve the class. |
| `adapters/oidc.ts:153` | `client.ResponseBodyError` (third party) | No | Same closure shape. `openid-client` may itself be duplicated across the two graphs, and it does not matter: `client.refreshTokenGrant` (line 269) and this check are both inside the closure `createOidcClient()` returned, so both see one `client` binding. The classifier's OUTPUT is what crosses, as `RefreshRejected`. |

Which `AuthRuntime` fields can carry a copy-bound class at all (`src/runtime.ts:62-86`): `store`
(fixed by SMA-653), `oidc` (this issue), `resolver` and `logger`. Neither `resolver` nor `logger`
throws a package class that any site classifies — the four rows above are the complete set. Every
other field holds plain data.

The release-time catch in `single-flight.ts:218-221` is a bare `catch` with no classification, so it
is unaffected. The `SessionStoreTimeout` doc comment (`core/errors.ts:21`) lists it among "every
caller that classifies on that class"; that wording is inaccurate and § 5 corrects it.

## 4. Tests

Vitest unit tests in `ts/packages/paigasus-auth/tests/`. No Docker.

1. **`tests/core/errors.test.ts` — a new `describe('isRefreshRejected (SMA-657)')`,** mirroring the
   existing `isSessionStoreUnavailable` block:
   - true for a locally built `new RefreshRejected('invalid_grant')`;
   - true for one built from a SECOND copy of `src/core/errors.js`, loaded with `vi.resetModules()`
     and a dynamic `import`. The row asserts the precondition
     `expect(foreign.RefreshRejected).not.toBe(RefreshRejected)` FIRST, then
     `expect(err instanceof RefreshRejected).toBe(false)`, then the predicate is `true`. Without the
     precondition the row passes vacuously if the import returns the same module;
   - false for a `CallbackRejected`, for a `SessionStoreUnavailable`, for
     `new Error('oidc_refresh_rejected')`, for `{ code: 'oidc_refresh_rejected' }`, and for
     `undefined`.

2. **`tests/core/single-flight.test.ts` — the load-bearing test,** added to the
   `'a failing refresh (SMA-626 § 2.3)'` block. It is the only test that reds if line 170 reverts.

   It calls `vi.resetModules()`, dynamically imports a second `src/core/errors.js`, asserts the same
   precondition, and drives `resolveSession` with
   `refresh: () => Promise.reject(new foreign.RefreshRejected('invalid_grant'))`. `resolveSession`
   was statically imported, so it keeps its first-copy binding: this is the real two-copy shape, not
   a simulation of it. The record under test carries a LIVE access token, because that is the case
   the defect changes most — today it degrades, and it must sign out. The test asserts:
   - `session.refresh_failed` with `reason: 'rejected'` and `degraded: false`;
   - `session.deleted` with `reason: 'refresh_rejected'`;
   - `store.delete` was called with the sid.

   A second row keeps the existing same-copy `RefreshRejected` and asserts the identical outcome, so
   the fix is proved not to have changed the ordinary path.

3. **Red first, then mutation.** Item 2 runs against the UNFIXED code and must fail; the failure is
   recorded in the pull request. After the fix, line 170 is reverted to
   `err instanceof RefreshRejected` and item 2 must go red again. A test that passes only because
   the code did not exist yet is not proof. The same treatment applies to item 1 against an
   `instanceof` implementation of the predicate.

4. **Mutation check.** Each item below is reverted one at a time, and at least one test must red:
   - `isRefreshRejected` replaced by `instanceof` at `single-flight.ts:170` (item 2);
   - `hasAuthErrorCode`'s `err instanceof Error` test removed (the plain-object row of item 1 and
     the existing `isSessionStoreUnavailable` row);
   - the `code` literal in `isRefreshRejected` changed (item 1);
   - `isSessionStoreUnavailable` rewired to the wrong literal by the D3 move (its existing block).

**What these tests do NOT prove.** They prove the classification survives two module copies in one
V8 isolate. They do not prove that Next 16.3.4 puts the route handler and the page in two copies —
`src/runtime.ts` measured that separately, and this specification relies on that measurement. They
also do not prove the user-visible sign-out, which no unit test reaches.

**Rejected: a probe inside a real Next build.** It would build an app-level fixture and a throwaway
route in `iam-console` to show two different `RefreshRejected` identities. It would measure V8 and
Turbopack, which `src/runtime.ts` already measured for this module, and it would leave a fixture
nobody maintains. **Rejected: a Keycloak e2e revocation test.** The e2e tier runs one process and
cannot force the two-copy split, so the test would pass before and after the fix.

## 5. Documentation changes

- The doc comment above `isRefreshRejected` states D7's rule and names the `runtime.oidc` path that
  makes this site cross.
- A comment at each of the three safe sites (`server.ts:111`, `operation-deadline.ts:73`,
  `oidc.ts:153`) states why it cannot cross, in one or two lines, using § 3's reasons.
- The `SessionStoreTimeout` doc comment (`core/errors.ts:21`) is corrected: it lists
  "core/single-flight.ts's release-time catch" among the callers that classify on that class, and
  that catch is bare and classifies nothing (§ 3).
- No CLAUDE.md change. The rule lives beside the code it governs, and CLAUDE.md already records the
  module duplication through the SMA-511 entries.

## 6. Assumptions

- The two module copies run in ONE V8 isolate and share the builtin `Error`, so
  `err instanceof Error` in `hasAuthErrorCode` holds for an error from either copy. This is what
  makes the existing `isSessionStoreUnavailable` work, and its second-copy test already proves it.
- `vi.resetModules()` plus a dynamic import produces a genuinely distinct module instance under this
  package's vitest configuration. The existing `errors.test.ts` row proves it today, and every new
  row asserts it again rather than assuming it.
- No consumer outside this package catches a `RefreshRejected`. It is not exported from
  `src/server.ts`, and `getSession()` swallows every failure.

## 7. Out of scope, and the residual

- **`@paigasus/console-core`.** Its `instanceof` checks (`discovery.ts:102`, `:103`, `:274`) are the
  same-closure shape as `operation-deadline.ts:73`, and its runtime is built at module scope per
  copy rather than shared through `globalThis`. Not audited further here.
- **No automated gate (D5).** **Residual, stated plainly:** nothing reds when a future site
  classifies a package error class with `instanceof` across the copies. The comments of § 5 and this
  document are the only control. This is the same class of residual the repository records for its
  other hand-maintained rules. A gate would need to tell a crossing site from a non-crossing one,
  which needs the call-graph reasoning of § 3, not a syntactic rule.
- **The `adapters/oidc.ts` classifier** (`classifyRefreshError`) keeps its closed `invalid_grant`
  vocabulary. This issue changes how the result is read, never what is produced.

## 8. Rejected alternatives

- **A `Symbol.for`-keyed brand on `AuthError`.** It would work across copies the way
  `RUNTIME_KEY_PREFIX` does, and it adds a second identity mechanism beside `code`, which already
  exists, is already a public part of every error, and is already the pattern SMA-653 set. Two
  mechanisms would then have to agree.
- **Making `single-flight.ts` import the class from the runtime.** It would remove the duplication
  at this one site and would make `core/` depend on the adapter layer, which SMA-626 § 2.3
  deliberately forbids — the whole reason `RefreshRejected` lives in `core/errors.ts` is so that
  `core/` can classify without importing `adapters/`.
- **A type guard `err is RefreshRejected` (see D2).**
- **Duplicating the one-line predicate instead of D3's shared helper.** For two predicates the saving
  is small. The cost is that the rule then exists twice and a third copy can differ from both.
