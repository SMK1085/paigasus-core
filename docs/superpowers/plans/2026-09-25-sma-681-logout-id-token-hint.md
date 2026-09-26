# SMA-681: send `id_token_hint` on logout — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Logout sends the stored raw ID token as `id_token_hint`, so Keycloak does not show "Do you want to log out?" when an SSO session is live.

**Architecture:** `SessionRecord` changes to `version: 2` with a required raw `idToken`. The login stores the token from the code exchange. A refresh replaces it only when the new token has the same `iss` and `sub` as the login claims. A mismatch is a definitive refresh failure. `handleLogout` already reads the record before it deletes it, so the same read gives the hint. Three test tiers prove that the hint is sent: unit, the package Keycloak e2e and the iam-console fake-IdP e2e (a required tier). The kind J1 journey also proves it, in the chart job, which is not a required check.

**Tech Stack:** TypeScript (strict, `exactOptionalPropertyTypes`), openid-client 6.8.8 / oauth4webapi 3.8.8, vitest, Playwright 1.63, Moon 2.5.3, pnpm.

**Spec:** `docs/superpowers/specs/2026-09-25-sma-681-logout-id-token-hint-design.md` (read it first; it is the source of truth).

**Worktree:** `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint`, branch `feature/sma-681-logout-id-token-hint`. All paths below are relative to this root. Use absolute paths in every tool call.

## Global Constraints

- Every new source file starts with `// SPDX-License-Identifier: Apache-2.0`. This plan creates no new source file.
- Every shell command starts with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- `SessionRecord.version` is `2`. `idToken` is **required**, a string with length above zero (spec D3, § 4.1).
- An expired stored token is sent unchanged. No code does a local `exp` check (spec D4).
- A refreshed ID token is stored only when `idTokenClaims.iss === fresh.idTokenClaims.iss` and `idTokenClaims.sub === fresh.idTokenClaims.sub` (spec D5). `sid` is not compared.
- A refreshed ID token with a different `iss` or `sub` is a definitive refresh failure: no write, delete the record, revoke the NEW refresh token best effort, log `session.refresh.id_token_mismatch` and `session.deleted` with `reason: 'id_token_mismatch'` (both carry `{ sid: sidTag(sid) }` plus `reason` only), return `null` (spec D6).
- A refresh never changes `idTokenClaims` (spec § 4.1).
- `session.refreshed` gets `idTokenRotated: boolean`. `logout.completed` gets `idTokenHintSent: boolean`. Never name a field `idTokenHint`, and never put a token in any event (spec § 4.3, § 4.5, `src/ports/logger.ts`).
- `toSessionView` does not change. The raw token never crosses to the browser.
- Core (`src/core/**`) must not import from `src/adapters/**`. `IdTokenClaims` comes from `src/ports/principal-resolver`.
- The J1 step title `'logout: the shell form ends at the IdP and returns to /iam/ with no session cookie'` does not change (`ci/kind/journeys-report.mjs:42` pins it).
- Rewritten comments state facts, with a source: the spec section, a measurement row, or a `file:line`. Do not repeat the unmeasured Entra ID claim. The spec measured only Keycloak.
- Commits: conventional, workspace scope, lower-case subject, for example `feat(ts): store the raw id token in the session record (SMA-681)`. End every message with a blank line, then `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Do not put a `#NNN` line or a `token: value` line in a commit body. Never `--amend`, never `git reset`, never `--no-verify`. Do not commit during another task's run.
- Each task ends green: typecheck and tests of every package the task touched. Do not add code "to wire later": unused code fails lint.

## Review Focus

These five conditions follow from the spec, but no spec test covers them. A user is most likely to meet them. Each one has a test in the task that owns the code.

1. **Redaction.** No logged event carries the raw ID token: not on logout, not on a rotated refresh, not on a mismatch. Tests: Task 4 (refresh), Task 5 (logout).
2. **A failed delete at logout (the SMA-653 503 branch)** makes no end-session call, so it sends no hint. Test: Task 5, row 7 in `tests/http/store-unavailable.test.ts`.
3. **The D6 mismatch path when `store.delete` fails.** The store error propagates, and no `session.deleted` is logged. The new refresh token is still revoked, because the revoke runs in a `finally`. The spec does not say which comes first. This plan decides "delete first, revoke always", the same rule as logout. Test: Task 4.
4. **Concurrent callers during a refresh that rotates the ID token.** There is exactly one refresh, and every caller sees the new token. Test: Task 4.
5. **The deploy moment.** A `version: 1` record in the store at logout is deleted. Logout then redirects to the IdP with no hint (AC 3 "no record"). Test: Task 5.

## Findings from reading the code (the spec is otherwise exact)

Every `file:line` that the spec cites matches the code on this branch. The three gaps below change what the tasks must do:

- **F1.** The spec lists `tests/adapters/redis-store-parse.test.ts:56-62` and `tests/adapters/memory-store.test.ts:22` for the HOLE 2 change. It does not list the positive controls `redis-store-parse.test.ts:43` and `memory-store.test.ts:17`. Both use `toMatchObject({ version: 1, … })`, and both fail at runtime after the bump. The compiler does not flag them. Task 2 changes both.
- **F2.** § 4.3 requires a best-effort revoke inside `resolveSession`, but `ResolveDeps` has no revoke function. This plan adds a **required** `revoke: (token: string) => Promise<void>` to `ResolveDeps`. The compiler then flags all six call sites, and Task 4 changes them: `src/next/get-session.ts`, `tests/e2e/fixture-server.ts`, `tests/fixtures/refresh-worker.ts`, `tests/containers/single-flight-redis.test.ts`, `tests/core/single-flight.test.ts` and `tests/http/logout.test.ts`.
- **F3.** The task brief names `paigasus-auth-ts:lint`. That target does not exist. Lint and Prettier are whole-tree tasks on the `ts` root project: `ts:lint` (`pnpm exec eslint .`) and `ts:fmt` (`pnpm exec prettier --check .`).

## File Structure

| File | Change | Task |
|---|---|---|
| `ts/packages/paigasus-auth/src/adapters/oidc.ts` | `OidcTokens.idToken`, `RefreshedTokens.idToken?`/`idTokenClaims?`, the code grant and the refresh return them | 1, 3 |
| `ts/packages/paigasus-auth/src/core/session.ts` | `version: 2`, `idToken`, `isSessionRecord` | 2 |
| `ts/packages/paigasus-auth/src/http/routes.ts` | `handleCallback` stores `idToken`. `handleLogout` sends the hint and logs `idTokenHintSent`. The comment is rewritten | 2, 5 |
| `ts/packages/paigasus-auth/src/core/single-flight.ts` | Port fields, `ResolveDeps.revoke`, the D5/D6 rule, `idTokenRotated` | 4 |
| `ts/packages/paigasus-auth/src/ports/logger.ts` | `'session.refresh.id_token_mismatch'` | 4 |
| `ts/packages/paigasus-auth/src/next/get-session.ts` | Passes `revoke` | 4 |
| `ts/packages/paigasus-auth/README.md` | One paragraph under "Logout revokes, not just clears" | 5 |
| `ts/packages/paigasus-auth/tests/**` | Fixtures and tests (listed per task) | 1-5 |
| `ts/apps/gateway-console/tests/integration/support.ts` | Typed fixture: `version: 2`, `idToken` | 2 |
| `ts/apps/iam-console/tests/unit/action-session.test.ts` | Cast fixture: `version: 2` (for consistency only) | 2 |
| `ts/packages/paigasus-console-core/testing/fake-idp.ts` | Records `idTokens` and `endSessionHints` | 6 |
| `ts/apps/iam-console/tests/integration/doubles/fake-idp.test.ts` | Tests the recording | 6 |
| `ts/apps/iam-console/tests/e2e/login.spec.ts` | R12 asserts the hint | 6 |
| `ts/apps/iam-console/tests/cluster/journeys/auth-roundtrip.spec.ts` | J1 step 4 asserts the hint. The D9 comment is rewritten | 7 |

## Which tier runs where

| Tier | Needs | Who runs it |
|---|---|---|
| `paigasus-auth-ts:test`, `:typecheck`, `ts:lint`, `ts:fmt`, `*-console-ts:typecheck` | nothing | the implementer, every task |
| `iam-console-ts:test`, `iam-console-ts:test-e2e` (R12) | a Next build, no Docker | the implementer (Task 6, Task 8) |
| `gateway-console-ts:test` | a Next build, no Docker | the implementer (Task 8) |
| `paigasus-auth-ts:test-e2e` (Keycloak + Redis) | Docker | the implementer if `docker info` succeeds, otherwise CI only |
| J1 (`ci/kind/run.sh specs journeys`) | kind, CI secrets | CI only: the `chart.yml` job (not a required check) |

Never mark a step done on an e2e run that did not happen. Write "NOT RUN LOCALLY: needs Docker" (or "CI only") in the task report.

---

### Task 1: The OIDC adapter returns the raw ID token from the code exchange

**Files:**
- Modify: `ts/packages/paigasus-auth/src/adapters/oidc.ts:43-45` (`OidcTokens`), `:238-271` (`authorizationCodeGrant`)
- Modify: `ts/packages/paigasus-auth/tests/support/store-failure.ts:20-24, 59-80` (the fake returns an `idToken`)
- Test: `ts/packages/paigasus-auth/tests/adapters/oidc.test.ts` (a new case after `:63`)

**Interfaces:**
- Consumes: nothing.
- Produces: `OidcTokens` gets `idToken: string` (required). `tests/support/store-failure.ts` exports `FAKE_ID_TOKEN = 'fake-header.fake-payload.fake-signature'`, and `fakeOidc().authorizationCodeGrant` returns it.

- [ ] **Step 1: Write the failing test**

In `tests/adapters/oidc.test.ts`, add this case inside `describe('createOidcClient — authorizationCodeGrant ID Token validation', …)`, directly after the `'accepts a well-formed token'` case:

```ts
  // SMA-681 § 4.2: logout sends this exact string as `id_token_hint`, so the adapter must hand back
  // the raw JWT the IdP issued, not a re-encoding of its claims.
  it('returns the raw id_token string, equal to the minted token', async () => {
    const minted = await fixture.mintIdToken({ nonce: NONCE });
    fixture.setNextIdToken(minted);
    const tokens = await grant(makeClient());
    expect(tokens.idToken).toBe(minted);
  });
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint/ts/packages/paigasus-auth exec vitest run tests/adapters/oidc.test.ts -t 'returns the raw id_token'
```

Expected: FAIL, `expected undefined to be 'eyJ…'`.

- [ ] **Step 3: Write the implementation**

In `src/adapters/oidc.ts`, replace the `OidcTokens` interface (`:43-45`) with:

```ts
export interface OidcTokens extends RefreshedTokens {
  /**
   * The raw, signed ID token JWT from the code exchange (SMA-681). The session record stores it,
   * and logout sends it as `id_token_hint`. Never log it: see ports/logger.ts.
   */
  idToken: string;
  idTokenClaims: IdTokenClaims;
}
```

In `authorizationCodeGrant`, directly after the existing `if (claims === undefined) { … }` block (`:248-250`), add:

```ts
        // SMA-681. This check only narrows the type of `tokens.id_token`. With
        // `idTokenExpected: true`, oauth4webapi already rejects a response whose `id_token` is
        // missing or not a string (oauth4webapi/build/index.js:1480-1482), so this line cannot
        // run, and no test covers it. It stays inside this `try`, so it goes through
        // wrapError('authorization_code_grant', …) like every other failure here.
        if (typeof tokens.id_token !== 'string') {
          throw new Error('no id_token in the token response');
        }
```

In the same method's `return` object (`:262-267`), add `idToken` before `idTokenClaims`:

```ts
        return {
          accessToken: tokens.access_token,
          ...(tokens.refresh_token !== undefined ? { refreshToken: tokens.refresh_token } : {}),
          expiresIn,
          idToken: tokens.id_token,
          idTokenClaims: toIdTokenClaims(claims),
        };
```

In `tests/support/store-failure.ts`, add after `export const NEW_REFRESH_TOKEN = 'new-refresh-token';` (`:24`):

```ts
/** The raw ID token that `fakeOidc().authorizationCodeGrant` returns. JWT-shaped, not signed. */
export const FAKE_ID_TOKEN = 'fake-header.fake-payload.fake-signature';
```

and in `fakeOidc().authorizationCodeGrant` (`:65-71`) add `idToken: FAKE_ID_TOKEN,` before `idTokenClaims`:

```ts
    authorizationCodeGrant: (): Promise<OidcTokens> =>
      Promise.resolve({
        accessToken: 'new-access-token',
        refreshToken: NEW_REFRESH_TOKEN,
        expiresIn: 300,
        idToken: FAKE_ID_TOKEN,
        idTokenClaims: { iss: 'https://issuer.example.com', sub: 'a-subject' },
      }),
```

- [ ] **Step 4: Run the tests and the typecheck**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint/ts/packages/paigasus-auth
pnpm exec vitest run tests/adapters/oidc.test.ts tests/http
pnpm exec tsc -p tsconfig.json --noEmit
```

Expected: all PASS. tsc exits 0.

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint
git add ts/packages/paigasus-auth/src/adapters/oidc.ts ts/packages/paigasus-auth/tests/adapters/oidc.test.ts ts/packages/paigasus-auth/tests/support/store-failure.ts
git commit -m "$(cat <<'EOF'
feat(ts): return the raw id token from the code exchange (SMA-681)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
git log --oneline -1
```

---

### Task 2: `SessionRecord` version 2 with a required `idToken`, stored at login

**Files:**
- Modify: `ts/packages/paigasus-auth/src/core/session.ts:15-36` (the interface), `:63-117` (the doc and `isSessionRecord`)
- Modify: `ts/packages/paigasus-auth/src/http/routes.ts:322-331` (`handleCallback`'s record)
- Modify (typed fixtures): `ts/packages/paigasus-auth/tests/store-contract.ts:6-18`, `tests/core/session.test.ts:7-23`, `tests/http/logout.test.ts:45, 56-68`, `tests/http/store-unavailable.test.ts:32-44`, `tests/http/callback.test.ts:156-167`, `tests/next/get-session.test.ts:87-98`, `ts/apps/gateway-console/tests/integration/support.ts:73-81`, `ts/apps/iam-console/tests/unit/action-session.test.ts:61`
- Modify (untyped cases, spec § 4.6 plus F1): `tests/core/session.test.ts:32-39, 107, 137-145`, `tests/adapters/redis-store-parse.test.ts:41-45, 56-62, 78-82`, `tests/adapters/memory-store.test.ts:14-22`
- Modify (a stale comment): `tests/containers/single-flight-multiprocess.test.ts:78`
- Test: `tests/core/session.test.ts`, `tests/http/callback.test.ts`

**Interfaces:**
- Consumes: `OidcTokens.idToken` (Task 1).
- Produces: `SessionRecord { version: 2; …; idToken: string; idTokenClaims: IdTokenClaims; … }`. `isSessionRecord` requires `version === 2` and a non-empty string `idToken`. `makeRecord()` returns `idToken: 'IDT'`. `tests/http/logout.test.ts` has `const STORED_ID_TOKEN = 'stored-header.stored-payload.stored-signature';`, and `seededRecord()` uses it.

- [ ] **Step 1: Write the failing tests**

In `tests/core/session.test.ts`:

(a) Replace `RECORD` (`:7-23`). The only changes are `version: 2` and the new `idToken` line:

```ts
const RECORD: SessionRecord = {
  version: 2,
  rev: 3,
  accessToken: 'AT-secret',
  refreshToken: 'RT-secret',
  accessExpiresAt: 2_000_000,
  absoluteExpiresAt: 9_000_000,
  idToken: 'IDT-secret',
  idTokenClaims: { iss: 'https://idp', sub: 'u1', email: 'a@b.c', name: 'Alice' },
  principal: {
    principalPrn: 'prn:pgs:iam::org1:user/8f1a2b3c-4d5e-6f70-8192-a3b4c5d6e7f8',
    issuer: 'https://idp',
    subject: 'u1',
    memberships: [],
    roleGrants: [{ scopePrn: 'prn:pgs:iam::org1:org/8f1a2b3c-4d5e-6f70-8192-a3b4c5d6e7f8', roleKey: 'admin' }],
    grantsAvailable: false,
  },
};
```

(b) In `'carries no token field under any name'` (`:32-39`), add the ID token to the must-not-contain list, after the `'RT-secret'` line:

```ts
    // SMA-681: the raw ID token is in the record now. It must not reach the browser either.
    expect(serialised).not.toContain('IDT-secret');
```

(c) Add `'idToken'` to the missing-field table (`:107`):

```ts
  it.each(['version', 'rev', 'accessToken', 'accessExpiresAt', 'absoluteExpiresAt', 'idToken', 'idTokenClaims', 'principal'])('rejects a record missing %s', (field) => {
```

(d) Replace the two cases at `:137-145` (HOLE 2 and the version mismatch) with:

```ts
  // HOLE 2 (§ 4.1): two NaN comparisons in resolveSession let this through as a LIVE session
  // carrying accessToken: undefined. The body carries the CURRENT version, so the test reaches the
  // field checks. With any other version it would fail at the version check and prove nothing.
  it('rejects a body of { version: 2 } and nothing else', () => {
    expect(isSessionRecord({ version: 2 })).toBe(false);
  });

  // SMA-681 D3. Version 1 is every record written before the SMA-681 deploy. Rejecting it is the
  // forced logout that the spec accepts.
  it.each([1, 3])('rejects a version other than 2 (%p)', (version) => {
    expect(isSessionRecord({ ...makeRecord(), version })).toBe(false);
  });

  // SMA-681 § 4.1: logout sends idToken as id_token_hint, so an empty or non-string value is a
  // poisoned record, not a record with no hint.
  it.each([
    ['a number', 42],
    ['null', null],
    ['an empty string', ''],
  ])('rejects an idToken that is %s', (_label, idToken) => {
    expect(isSessionRecord({ ...makeRecord(), idToken })).toBe(false);
  });
```

In `tests/http/callback.test.ts`, add this case directly after `'writes a record whose absoluteExpiresAt is now + ABSOLUTE_TTL'` (it ends at `:336`):

```ts
  // SMA-681 § 4.4: the login writes a version 2 record that carries the raw ID token the IdP
  // returned, byte for byte. Logout sends it as id_token_hint.
  it('writes a version 2 record carrying the raw id_token from the code exchange', async () => {
    const state = 'state-success-id-token';
    await seedTransaction(state);
    const minted = await fixture.mintIdToken({ nonce: NONCE });
    fixture.setNextIdToken(minted);

    const res = await createAuthRoutes(runtime).handle(callbackRequest(state, cookieHeaderFor(state, CORRECT_SECRET)));

    const setCookie = res.headers.getSetCookie().find((c) => c.startsWith(`${SESSION_COOKIE}=`));
    const sid = setCookie?.split(';')[0]?.split('=')[1] ?? '';
    const record = await store.get(sid);
    expect(record?.version).toBe(2);
    expect(record?.idToken).toBe(minted);
  });
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint/ts/packages/paigasus-auth
pnpm exec vitest run tests/core/session.test.ts tests/http/callback.test.ts
```

Expected: FAIL. `rejects a version other than 2 (1)` fails, because the predicate accepts version 1. The `idToken` rows fail. `writes a version 2 record …` fails with `expected 1 to be 2`.

- [ ] **Step 3: Write the implementation**

In `src/core/session.ts`, replace the `version` doc and field (`:16-22`) with:

```ts
  /**
   * A mismatch on read is treated as ABSENT and the record is deleted. There is no migration
   * path by design; the alternative is deserialising into a wrong-typed object.
   * OPERATIONAL CONSEQUENCE: the deploy that bumps this logs out every active user. Say so in
   * the release note.
   *
   * Version 2 (SMA-681) added the required `idToken`. A bump has two more costs. During a rolling
   * update, old and new pods read each other's records as absent and delete them, so a user can
   * see a login loop until the rollout ends. A rollback forces a second logout. Both zones default
   * their image tag to `.Chart.AppVersion` (charts/paigasus/values.yaml), so the mixed state lasts
   * only for the rollout. No deployment existed on 2026-09-25 (spec § 4.1).
   */
  version: 2;
```

Replace the line `idTokenClaims: IdTokenClaims;` (`:34`) with:

```ts
  /**
   * The raw, signed ID token JWT: the newest one the IdP issued for this session (SMA-681). The
   * login sets it. A refresh replaces it only when the new token has the same `iss` and `sub` as
   * `idTokenClaims` (core/single-flight.ts). Only logout reads it, as `id_token_hint`. So after a
   * refresh it can come from a DIFFERENT token than `idTokenClaims`. `toSessionView` does not
   * read it, so it never crosses to the browser.
   */
  idToken: string;
  /**
   * The DECODED claims of the LOGIN ID token. A refresh does not change them (SMA-681 spec § 4.1).
   * The principal and the display name read these, never `idToken`.
   */
  idTokenClaims: IdTokenClaims;
```

In the `isSessionRecord` doc comment, replace item 2's first line (`:74`):

```ts
 *   2. A body of `{ version: 1 }` (the version then; `{ version: 2 }` today) passes two NaN
 *      comparisons in a row in resolveSession
```

and replace the sentence at `:82-83` with:

```ts
 * `refreshToken` is the one optional field: absent is legal, an explicit `null` is not
 * (`exactOptionalPropertyTypes`). `idToken` is required and must not be empty: logout sends it as
 * `id_token_hint` (SMA-681).
```

In `isSessionRecord`, replace `if (value['version'] !== 1) return false;` (`:95`) with `if (value['version'] !== 2) return false;`, and add after the `refreshToken` line (`:100`):

```ts
  const idToken = value['idToken'];
  if (typeof idToken !== 'string' || idToken.length === 0) return false;
```

In `src/http/routes.ts` `handleCallback`, change the record (`:322-331`) to:

```ts
  const record: SessionRecord = {
    version: 2,
    rev: 0,
    accessToken: tokens.accessToken,
    ...(tokens.refreshToken !== undefined ? { refreshToken: tokens.refreshToken } : {}),
    accessExpiresAt: now + tokens.expiresIn * 1000,
    absoluteExpiresAt: now + runtime.absoluteTtlMs,
    idToken: tokens.idToken,
    idTokenClaims: tokens.idTokenClaims,
    principal,
  };
```

- [ ] **Step 4: Change every typed fixture and every untyped version case**

`tests/store-contract.ts` `makeRecord` (`:7-17`): set `version: 2`, and add `idToken: 'IDT',` before `idTokenClaims`.

`tests/http/logout.test.ts`: add after `const END_SESSION_URL = …` (`:45`):

```ts
/** The raw ID token that seededRecord() stores. JWT-shaped, not signed. */
const STORED_ID_TOKEN = 'stored-header.stored-payload.stored-signature';
```

and in `seededRecord` (`:57-67`) set `version: 2` and add `idToken: STORED_ID_TOKEN,` before `idTokenClaims`.

`tests/http/store-unavailable.test.ts` `record()` (`:33-43`): `version: 2`, and add `idToken: 'old-id-token',` before `idTokenClaims`.

`tests/http/callback.test.ts` `seededRecord` (`:157-166`): `version: 2`, and add `idToken: 'old-id-token',` before `idTokenClaims`.

`tests/next/get-session.test.ts` `liveRecord` (`:89-97`): `version: 2`, and add `idToken: 'IDT-live',` before `idTokenClaims`.

`ts/apps/gateway-console/tests/integration/support.ts` `installSession` (`:73-81`): `version: 2`, and add `idToken: 'id-token-integration',` before `idTokenClaims`. This record goes through the real store's `isSessionRecord`, so without the change the gateway integration tests read no session.

`ts/apps/iam-console/tests/unit/action-session.test.ts:61`: change `version: 1,` to `version: 2,`. Add this comment line above `const record`:

```ts
  // Cast through `unknown`, and this store never calls isSessionRecord: `version` changes only for
  // consistency with SessionRecord (SMA-681). It proves nothing.
```

`tests/adapters/redis-store-parse.test.ts`:
- `:43` → `.resolves.toMatchObject({ version: 2, accessToken: 'AT' });`
- Replace the HOLE 2 case (`:56-62`) with:

```ts
  // HOLE 2. Parses fine, version is CURRENT, everything else is missing. Before the fix this is
  // returned as a LIVE session carrying accessToken: undefined. The body must carry the current
  // version, or it fails at the version check and never reaches the field checks.
  it('treats a { version: 2 } body as absent and deletes it', async () => {
    state.value = '{"version":2}';
    await expect((await store()).get('s')).resolves.toBeNull();
    expect(state.deleted).toEqual(['pgs:sess:s']);
  });
```

- Replace the mismatch case (`:78-82`) with:

```ts
  // SMA-681 D3, at the adapter level: a version 1 record written before the deploy is absent and
  // deleted. This is the forced logout the spec accepts.
  it('still treats a version mismatch (a version 1 record) as absent and deletes it', async () => {
    state.value = JSON.stringify({ ...makeRecord(), version: 1 });
    await expect((await store()).get('s')).resolves.toBeNull();
    expect(state.deleted).toEqual(['pgs:sess:s']);
  });
```

`tests/adapters/memory-store.test.ts`: `:17` → `.resolves.toMatchObject({ version: 2, accessToken: 'AT' });` and `:22` → `await s.set('s', { version: 2 } as unknown as SessionRecord, 60_000, null);`.

`tests/containers/single-flight-multiprocess.test.ts:78`: change `` `version: 1` `` to `` `version: 2` `` in the comment.

- [ ] **Step 5: Run the tests and typechecks**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ROOT=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint/ts
pnpm -C "$ROOT/packages/paigasus-auth" exec vitest run
pnpm -C "$ROOT/packages/paigasus-auth" exec tsc -p tsconfig.json --noEmit
pnpm -C "$ROOT/apps/gateway-console" exec tsc -p tsconfig.json --noEmit
pnpm -C "$ROOT/apps/iam-console" exec tsc -p tsconfig.json --noEmit
pnpm -C "$ROOT/apps/iam-console" exec vitest run tests/unit/action-session.test.ts
pnpm -C "$ROOT/apps/gateway-console" exec vitest run tests/integration
```

Expected: every vitest run passes, and every tsc exits 0. If `tests/integration` in gateway-console needs a Next build that is missing, write that in the report. Task 8 runs `gateway-console-ts:test` through Moon.

- [ ] **Step 6: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint
git add ts/packages/paigasus-auth ts/apps/gateway-console/tests/integration/support.ts ts/apps/iam-console/tests/unit/action-session.test.ts
git commit -m "$(cat <<'EOF'
feat(ts): store the raw id token in the session record (SMA-681)

SessionRecord is now version 2 with a required idToken. The deploy
logs out every active user.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
git log --oneline -1
```

---

### Task 3: The OIDC adapter returns a refreshed ID token when the IdP sends one

**Files:**
- Modify: `ts/packages/paigasus-auth/src/adapters/oidc.ts:37-41` (`RefreshedTokens`), `:273-291` (`refresh`)
- Modify: `ts/packages/paigasus-auth/tests/adapters/oidc.test.ts:167-182` (`client_id` with a hint, the witness comment)
- Test: `ts/packages/paigasus-auth/tests/adapters/oidc.test.ts` (a new `describe`)

**Interfaces:**
- Consumes: `OidcTokens.idToken` (Task 1).
- Produces: the adapter's `RefreshedTokens` gets `idToken?: string; idTokenClaims?: IdTokenClaims`. The adapter sets both or neither. `OidcTokens` keeps both required.

- [ ] **Step 1: Write the failing tests**

In `tests/adapters/oidc.test.ts`, add a new `describe` after `describe('createOidcClient — the rest of the surface', …)` (it ends at `:201`):

```ts
// SMA-681 § 4.2. Keycloak returns a new ID token on a refresh (spec § 3 row M-e). The adapter hands
// it back only when the response carries one. It does not compare `sub` with the login token:
// core/single-flight.ts does that. `setNextIdToken` stays set across requests
// (tests/fixtures/jwks.ts:171), so each case uses a fresh fixture (beforeEach) and sets it itself.
describe('createOidcClient — refresh and the ID token (SMA-681)', () => {
  it('returns the raw refreshed id_token and its claims', async () => {
    // A non-default sub: the fixture default is 'user-1' (tests/fixtures/jwks.ts:216), so a
    // hard-coded claim in the adapter could not pass this.
    const minted = await fixture.mintIdToken({ sub: 'refreshed-subject' });
    fixture.setNextIdToken(minted);
    const refreshed = await makeClient().refresh('some-refresh-token');
    expect(refreshed.idToken).toBe(minted);
    expect(refreshed.idTokenClaims?.sub).toBe('refreshed-subject');
    expect(refreshed.idTokenClaims?.iss).toBe(fixture.issuer);
  });

  it('returns neither field when the refresh response carries no id_token', async () => {
    const refreshed = await makeClient().refresh('some-refresh-token');
    expect(refreshed).not.toHaveProperty('idToken');
    expect(refreshed).not.toHaveProperty('idTokenClaims');
  });

  // A GUARD, not red-first: openid-client's non-repudiation hook runs on every refresh response
  // that carries an id_token (openid-client/build/index.js:1029), so this passes before and after
  // the change. It fails if a later change disables the hook, or if it stops covering a refresh.
  it('rejects a refreshed id_token signed by a key the JWKS does not publish', async () => {
    fixture.setNextIdToken(await fixture.mintIdToken({ sub: 'refreshed-subject', wrongKey: true }));
    await expect(makeClient().refresh('some-refresh-token')).rejects.toThrow(/oidc refresh_token_grant failed/);
  });
});
```

In the existing case `'buildEndSessionUrl carries the post-logout redirect and the id token hint'` (`:167-173`), add as its last line:

```ts
    // § 3 row M-g: Keycloak rejects a hint whose `aud` is not the request's client_id. openid-client
    // still appends client_id when a hint is present, and it is the client the hint was issued to.
    expect(parsed.searchParams.get('client_id')).toBe(fixture.clientId);
```

Replace the witness comment (`:175-182`) with:

```ts
  // WITNESS TEST (task 9 review): openid-client@6.8.8 appends `client_id` to the end-session
  // parameters whenever the caller does not supply one (build/index.js:1129-1141 — `if
  // (!parameters.has('client_id')) parameters.set('client_id', c.client_id);`). Since SMA-681 the
  // logout route also sends `id_token_hint` when the session record holds one. With no record, the
  // request has `client_id` and `post_logout_redirect_uri` only, and Keycloak then shows its
  // confirmation page when an SSO session is live (SMA-681 spec § 3 row M-b). With a hint,
  // Keycloak needs the `client_id` to equal the hint's `aud` (§ 3 row M-g). Nothing else in this
  // package would notice a future major version dropping this default, so it is asserted here.
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint/ts/packages/paigasus-auth exec vitest run tests/adapters/oidc.test.ts
```

Expected: `returns the raw refreshed id_token and its claims` FAILS (`expected undefined to be 'eyJ…'`). The other two new cases pass (see the GUARD comment). The `client_id` line passes.

- [ ] **Step 3: Write the implementation**

In `src/adapters/oidc.ts`, replace `RefreshedTokens` (`:37-41`) with:

```ts
export interface RefreshedTokens {
  accessToken: string;
  refreshToken?: string;
  expiresIn: number; // seconds
  /**
   * SMA-681. `refresh` sets both of these, and only when the response carries an ID token.
   * openid-client has then validated that token as it validates a login token, signature included
   * (see `refresh` below). Nothing in this file compares its `sub` with the login token:
   * core/single-flight.ts does that.
   */
  idToken?: string;
  idTokenClaims?: IdTokenClaims;
}
```

`OidcTokens` (from Task 1) redeclares both fields as required. Keep it as it is.

In `refresh`, replace the `return { … }` (`:283-287`) with:

```ts
        // SMA-681 § 4.2. When the response carries an ID token, oauth4webapi has checked its
        // presence, `iss` against the discovered issuer, `aud`, `exp`/`iat`/`nbf` and the type of
        // `sub` (oauth4webapi/build/index.js:1321-1331, 1393-1398). openid-client's
        // non-repudiation hook has checked its signature (openid-client/build/index.js:1029),
        // because getConfig() enables that hook. Nothing here compares `sub` with the login token.
        const claims = tokens.claims();
        const refreshedIdToken = typeof tokens.id_token === 'string' && claims !== undefined ? { idToken: tokens.id_token, idTokenClaims: toIdTokenClaims(claims) } : {};
        return {
          accessToken: tokens.access_token,
          ...(tokens.refresh_token !== undefined ? { refreshToken: tokens.refresh_token } : {}),
          expiresIn,
          ...refreshedIdToken,
        };
```

- [ ] **Step 4: Run the tests and the typecheck**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint/ts/packages/paigasus-auth
pnpm exec vitest run
pnpm exec tsc -p tsconfig.json --noEmit
```

Expected: all PASS. tsc exits 0.

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint
git add ts/packages/paigasus-auth/src/adapters/oidc.ts ts/packages/paigasus-auth/tests/adapters/oidc.test.ts
git commit -m "$(cat <<'EOF'
feat(ts): return a refreshed id token from the oidc adapter (SMA-681)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
git log --oneline -1
```

---

### Task 4: The refresh stores a matching ID token and signs out on a mismatch

> **Superseded in part (2026-09-26).** This task is complete. A later review changed its mismatch
> path. Do not use the mismatch code and tests below. The mismatch path now revokes the new refresh
> token and the record's old one, after the lock release. `RefreshedTokens` now has one
> `rotatedIdToken` field. The spec is the authority: see § 4.3 and "Changes after review (2026-09-26)".
> The code is in commits `c90a99e5`, `40067298` and `bd1f4028`.

**Files:**
- Modify: `ts/packages/paigasus-auth/src/core/single-flight.ts:1-24` (imports, `RefreshedTokens`, `ResolveDeps`), `:98-99` (destructure `revoke`), `:188-217` (the rule, the write, the event)
- Modify: `ts/packages/paigasus-auth/src/ports/logger.ts:22` (a new event name)
- Modify (the `revoke` call sites, finding F2): `src/next/get-session.ts:53-63`, `tests/e2e/fixture-server.ts:152-162`, `tests/fixtures/refresh-worker.ts:93`, `tests/containers/single-flight-redis.test.ts:32-34`, `tests/core/single-flight.test.ts:24-26`, `tests/http/logout.test.ts:233-245`
- Test: `ts/packages/paigasus-auth/tests/core/single-flight.test.ts` (a new `describe` at the end)

**Interfaces:**
- Consumes: `SessionRecord.idToken` (Task 2). The adapter's `RefreshedTokens.idToken?`/`idTokenClaims?` (Task 3). The adapter type must stay assignable to the core port, and it is, because the fields are the same.
- Produces: the core `RefreshedTokens` gets `idToken?: string; idTokenClaims?: IdTokenClaims`. `ResolveDeps` gets a required `revoke: (token: string) => Promise<void>`. `AuthEventName` gets `'session.refresh.id_token_mismatch'`. `session.refreshed` fields: `{ sid, rev, idTokenRotated }`. `session.deleted` gets the new reason `'id_token_mismatch'`.

- [ ] **Step 1: Write the failing tests**

In `tests/core/single-flight.test.ts`:

(a) Change the imports. Replace `import { resolveSession } from '../../src/core/single-flight.js';` (`:8`) with:

```ts
import { resolveSession, type ResolveDeps } from '../../src/core/single-flight.js';
```

(b) Replace `deps` (`:24-26`) with:

```ts
function deps(store: SessionStore, refresh: ResolveDeps['refresh']) {
  // `revoke` is called only on the SMA-681 ID-token-mismatch path. A test that checks it passes
  // its own recording function over this one.
  return { store, refresh, revoke: () => Promise.resolve(), logger: noopLogger, skewMs: 30_000, lockTtlMs: 5_000, lockWaitMs: 3_000, ttlMs: 60_000 };
}
```

(c) Add at the end of the file:

```ts
// ---------------------------------------------------------------------------------------------
// SMA-681 § 4.3 (D5, D6). A refresh response MAY carry a new ID token. OIDC Core § 12.2 requires
// its `iss` and `sub` to equal the login token's. Nothing upstream compares them (spec § 4.2), so
// resolveSession does. A match is stored, for logout's id_token_hint. A mismatch is a definitive
// refresh failure: nothing is written, the record is deleted and the new refresh token is revoked.
// ---------------------------------------------------------------------------------------------
describe('a refreshed ID token (SMA-681 D5, D6)', () => {
  // makeRecord()'s own login claims (tests/store-contract.ts).
  const LOGIN_CLAIMS = { iss: 'https://idp', sub: 'u1' };

  function recordingRevoke(): { revoke: (token: string) => Promise<void>; revoked: string[] } {
    const revoked: string[] = [];
    const revoke = (token: string): Promise<void> => {
      revoked.push(token);
      return Promise.resolve();
    };
    return { revoke, revoked };
  }

  it('stores a refreshed ID token whose iss and sub match, and leaves idTokenClaims unchanged', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { logger, events } = recordingLogger();
    // An extra claim on the refreshed token: if the code copied these claims into the record, the
    // `toEqual` below would see `email`.
    const refresh = () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300, idToken: 'IDT2', idTokenClaims: { ...LOGIN_CLAIMS, email: 'new@example.com' } });

    const out = await resolveSession({ ...deps(store, refresh), logger }, 's');

    expect(out?.idToken).toBe('IDT2');
    const stored = await store.get('s');
    expect(stored?.idToken).toBe('IDT2');
    expect(stored?.idTokenClaims).toEqual(LOGIN_CLAIMS);
    expect(events).toContainEqual(['session.refreshed', { sid: sidTag('s'), rev: 1, idTokenRotated: true }]);
    // Review Focus 1: no event carries the token.
    expect(JSON.stringify(events)).not.toContain('IDT2');
  });

  it('keeps the stored ID token when the refresh response carries none', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { logger, events } = recordingLogger();

    const out = await resolveSession({ ...deps(store, () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300 })), logger }, 's');

    expect(out?.idToken).toBe('IDT');
    expect((await store.get('s'))?.idToken).toBe('IDT');
    expect(events).toContainEqual(['session.refreshed', { sid: sidTag('s'), rev: 1, idTokenRotated: false }]);
  });

  it.each([
    ['sub', { iss: 'https://idp', sub: 'someone-else' }],
    ['iss', { iss: 'https://other-idp', sub: 'u1' }],
  ])('a refreshed ID token with a different %s signs out: delete, revoke the new refresh token, return null', async (_field, claims) => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { logger, events } = recordingLogger();
    const { revoke, revoked } = recordingRevoke();
    const refresh = () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300, idToken: 'IDT-OTHER', idTokenClaims: claims });

    const out = await resolveSession({ ...deps(store, refresh), revoke, logger }, 's');

    expect(out).toBeNull();
    expect(await store.get('s')).toBeNull();
    expect(revoked).toEqual(['RT2']);
    expect(events).toContainEqual(['session.refresh.id_token_mismatch', { sid: sidTag('s') }]);
    expect(events).toContainEqual(['session.deleted', { sid: sidTag('s'), reason: 'id_token_mismatch' }]);
    expect(events.some(([name]) => name === 'session.refreshed')).toBe(false);
    // Review Focus 1: neither the new ID token nor the new refresh token reaches a log line.
    expect(JSON.stringify(events)).not.toContain('IDT-OTHER');
    expect(JSON.stringify(events)).not.toContain('RT2');
  });

  it('a mismatch whose response has no refresh token revokes nothing', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { revoke, revoked } = recordingRevoke();
    const refresh = () => Promise.resolve({ accessToken: 'AT2', expiresIn: 300, idToken: 'IDT-OTHER', idTokenClaims: { iss: 'https://idp', sub: 'someone-else' } });

    expect(await resolveSession({ ...deps(store, refresh), revoke }, 's')).toBeNull();
    expect(revoked).toEqual([]);
  });

  it('a failing revoke on the mismatch path still returns null', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const refresh = () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300, idToken: 'IDT-OTHER', idTokenClaims: { iss: 'https://idp', sub: 'someone-else' } });

    const out = await resolveSession({ ...deps(store, refresh), revoke: () => Promise.reject(new Error('idp unreachable')) }, 's');

    expect(out).toBeNull();
    expect(await store.get('s')).toBeNull();
  });

  // Review Focus 3. The delete goes through the SMA-651 deadline decorator, so it can fail. The
  // store error then propagates, as on the refresh_rejected path. The new refresh token is still
  // revoked, because nothing else would ever revoke it.
  it('a failing delete on the mismatch path propagates the store error and still revokes', async () => {
    const inner = new MemorySessionStore();
    await inner.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const calls: string[] = [];
    const store = failingStore(inner, new Set<StoreMethod>(['delete']), () => new SessionStoreTimeout('delete', 4000, 'deadline'), calls);
    const { logger, events } = recordingLogger();
    const { revoke, revoked } = recordingRevoke();
    const refresh = () => Promise.resolve({ accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300, idToken: 'IDT-OTHER', idTokenClaims: { iss: 'https://idp', sub: 'someone-else' } });

    await expect(resolveSession({ ...deps(store, refresh), revoke, logger }, 's')).rejects.toBeInstanceOf(SessionStoreTimeout);
    expect(calls).toContain('delete:s');
    expect(revoked).toEqual(['RT2']);
    expect(events).toContainEqual(['session.refresh.id_token_mismatch', { sid: sidTag('s') }]);
    expect(events.some(([name]) => name === 'session.deleted')).toBe(false);
  });

  // Review Focus 4: the single-flight guarantee holds when the refresh rotates the ID token.
  it('two concurrent callers during a rotating refresh: one refresh, both see the new ID token', async () => {
    const store = new MemorySessionStore();
    let calls = 0;
    const refresh = async () => {
      calls += 1;
      await new Promise((r) => setTimeout(r, 40)); // hold the lock long enough to force contention
      return { accessToken: 'AT2', refreshToken: 'RT2', expiresIn: 300, idToken: 'IDT2', idTokenClaims: LOGIN_CLAIMS };
    };
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);

    const results = await Promise.all([resolveSession(deps(store, refresh), 's'), resolveSession(deps(store, refresh), 's')]);

    expect(calls).toBe(1);
    expect(results.map((r) => r?.idToken)).toEqual(['IDT2', 'IDT2']);
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint/ts/packages/paigasus-auth exec vitest run tests/core/single-flight.test.ts
```

Expected: the new cases FAIL. The stored token stays `'IDT'`. `session.refreshed` has no `idTokenRotated`. The mismatch cases return a record, not `null`. The earlier cases still pass.

- [ ] **Step 3: Write the implementation**

In `src/ports/logger.ts`, add the event name after `'session.refresh.persist_failed'` (`:22`):

```ts
  | 'session.refresh.persist_failed'
  | 'session.refresh.id_token_mismatch'
```

In `src/core/single-flight.ts`, add the import after `import type { SessionRecord } from './session';` (`:5`):

```ts
import type { IdTokenClaims } from '../ports/principal-resolver';
```

Replace `RefreshedTokens` and `ResolveDeps` (`:10-24`) with:

```ts
export interface RefreshedTokens {
  accessToken: string;
  refreshToken?: string;
  expiresIn: number; // seconds
  /**
   * SMA-681. Both are set, or neither: only when the refresh response carries an ID token.
   * adapters/oidc.ts validates the token itself. resolveSession compares its `iss` and `sub` with
   * the login claims.
   */
  idToken?: string;
  idTokenClaims?: IdTokenClaims;
}

export interface ResolveDeps {
  store: SessionStore;
  refresh: (refreshToken: string) => Promise<RefreshedTokens>;
  /**
   * RFC 7009 revocation (SMA-681). Called only on the ID-token-mismatch path, for the NEW refresh
   * token that the refresh just issued. Best effort: a rejection is swallowed.
   */
  revoke: (token: string) => Promise<void>;
  logger: AuthLogger;
  skewMs: number;
  lockTtlMs: number;
  lockWaitMs: number;
  ttlMs: number;
}
```

Change the destructure (`:99`) to:

```ts
  const { store, refresh, revoke, logger, skewMs, lockTtlMs, lockWaitMs, ttlMs } = deps;
```

Replace the block from `const accessTtlMs = …` (`:188`) through the closing `};` of `next` (`:195`) with:

```ts
        // SMA-681 D5, D6 (spec § 4.3). A refresh response MAY carry a new ID token. OIDC Core
        // § 12.2 requires its `iss` and `sub` to equal the login token's, and nothing upstream
        // compares them: adapters/oidc.ts validates the new token on its own only. The `iss` half
        // cannot fail in production, because oauth4webapi requires `iss === as.issuer` on every
        // response, and the login token has the same value. It stays because it costs nothing.
        // `sid` is not compared: § 12.2 names only `iss` and `sub`, and only Keycloak is measured.
        //
        // A MISMATCH is a definitive refresh failure, with the shape of the refresh_rejected branch
        // above. Nothing is written: the record would join the login principal with an access
        // token issued for a different subject. The delete runs first, then the revoke of the NEW
        // refresh token, best effort. The revoke is in `finally`, so a failed delete still revokes
        // it: nothing else would ever revoke that token. A failed delete then propagates, as it
        // does on the refresh_rejected path. Neither event carries a token.
        //
        // `idTokenClaims` never changes here (spec § 4.1): the principal and the display name read
        // the login claims, and only logout reads `idToken`.
        let idToken = fresh.idToken;
        let idTokenRotated = false;
        if (tokens.idToken !== undefined && tokens.idTokenClaims !== undefined) {
          if (tokens.idTokenClaims.iss !== fresh.idTokenClaims.iss || tokens.idTokenClaims.sub !== fresh.idTokenClaims.sub) {
            logger.event('session.refresh.id_token_mismatch', { sid: sidTag(sid) });
            const newRefreshToken = tokens.refreshToken;
            try {
              await store.delete(sid);
              logger.event('session.deleted', { sid: sidTag(sid), reason: 'id_token_mismatch' });
            } finally {
              if (newRefreshToken !== undefined) await revoke(newRefreshToken).catch(() => undefined);
            }
            return null;
          }
          idToken = tokens.idToken;
          idTokenRotated = true;
        }

        const accessTtlMs = Math.max(tokens.expiresIn * 1000, skewMs + MIN_ACCESS_TTL_BUFFER_MS); // F7
        const next: SessionRecord = {
          ...fresh,
          rev: fresh.rev + 1,
          accessToken: tokens.accessToken,
          refreshToken: tokens.refreshToken ?? fresh.refreshToken,
          accessExpiresAt: Math.min(Date.now() + accessTtlMs, fresh.absoluteExpiresAt),
          idToken,
        };
```

Replace the `session.refreshed` line (`:216`) with:

```ts
        // `idTokenRotated` (SMA-681) shows in production whether refreshes deliver ID tokens. D5
        // exists only for IdPs that do.
        logger.event('session.refreshed', { sid: sidTag(sid), rev: written.rev, idTokenRotated }); // F6
```

The retry write (`written = { ...next, rev: winner.rev + 1 }`) copies `idToken` from `next`. No change is needed there.

- [ ] **Step 4: Add `revoke` at every call site (finding F2)**

`src/next/get-session.ts` (`:55-56`): add after the `refresh:` line:

```ts
        revoke: (token) => runtime.oidc.revoke(token),
```

`tests/e2e/fixture-server.ts` (`:154-155`): add after the `refresh:` line:

```ts
              revoke: (token) => runtime.oidc.revoke(token),
```

`tests/fixtures/refresh-worker.ts:93`:

```ts
  const result = await resolveSession({ store, refresh, revoke: () => Promise.resolve(), logger: noopLogger, skewMs: 30_000, lockTtlMs: 10_000, lockWaitMs: 5_000, ttlMs: 60_000 }, sid);
```

`tests/containers/single-flight-redis.test.ts:33`:

```ts
  return { store, refresh, revoke: () => Promise.resolve(), logger: noopLogger, skewMs: 30_000, lockTtlMs: 5_000, lockWaitMs: 3_000, ttlMs: 60_000 };
```

`tests/http/logout.test.ts` (`:236-238`): add after the `refresh:` line:

```ts
        revoke: () => Promise.reject(new Error('must not be called: the session is already deleted')),
```

- [ ] **Step 5: Run the tests and the typecheck**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint/ts/packages/paigasus-auth
pnpm exec vitest run
pnpm exec tsc -p tsconfig.json --noEmit
```

Expected: all PASS. tsc exits 0. tsc covers the container, e2e and fixture call sites, because `tsconfig.json` includes `tests/**`. `tests/structure/import-graph.test.ts` must pass: core imports `ports/`, not `adapters/`.

Optional, only if `docker info` succeeds: `pnpm exec vitest run --config vitest.containers.config.ts`. It runs the Redis single-flight suites and the two-process worker. If Docker is not available, write "container suites NOT RUN LOCALLY: needs Docker" in the report.

- [ ] **Step 6: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint
git add ts/packages/paigasus-auth
git commit -m "$(cat <<'EOF'
feat(ts): keep a refreshed id token and sign out on a subject mismatch (SMA-681)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
git log --oneline -1
```

---

### Task 5: Logout sends `id_token_hint`, and the package e2e asserts it

**Files:**
- Modify: `ts/packages/paigasus-auth/src/http/routes.ts:378-399` (the comment), `:415-419` (the read), `:458-474` (step 4, the event)
- Modify: `ts/packages/paigasus-auth/tests/support/store-failure.ts:53-80` (the fake records its end-session parameters)
- Modify: `ts/packages/paigasus-auth/tests/http/logout.test.ts:16-27` (the header), `:307-326` (the positive case), and new cases
- Modify: `ts/packages/paigasus-auth/tests/http/store-unavailable.test.ts:205-235` (rows 6 and 7)
- Modify: `ts/packages/paigasus-auth/tests/e2e/logout.spec.ts:24-27, 41-44`
- Modify: `ts/packages/paigasus-auth/README.md` (one new subsection after `:193`)

**Interfaces:**
- Consumes: `SessionRecord.idToken` (Task 2). `STORED_ID_TOKEN` in `tests/http/logout.test.ts` (Task 2). `BuildEndSessionUrlParams.idTokenHint?: string` (already in `src/adapters/oidc.ts:69`).
- Produces: `logout.completed` fields `{ zone, sid?, revoked, endSessionRedirected, idTokenHintSent }`. `FakeOidc.endSessionCalls: BuildEndSessionUrlParams[]` in `tests/support/store-failure.ts`.

- [ ] **Step 1: Write the failing unit tests**

In `tests/http/logout.test.ts`, replace the header paragraph at `:16-27` with:

```ts
// `id_token_hint` IS SENT WHEN THE SESSION RECORD HOLDS AN ID TOKEN (SMA-681). This reverses the
// SMA-506 decision that this header used to record. OpenID Connect RP-Initiated Logout 1.0,
// section 2, says the OP MUST ask the user to confirm when no hint is provided, and Keycloak 26.4
// does when an SSO session is live (SMA-681 spec § 3 row M-b). The SMA-506 measurement (M12) saw no
// page only because its realm created no online SSO session (SMA-682). The tests below assert the
// hint equals the stored token. With no cookie, no record, or a failed read (store-unavailable.test.ts
// row 6), there is no hint and the redirect still goes to the IdP (spec AC 3).
```

Replace the case `'redirects to end_session_endpoint carrying post_logout_redirect_uri and a state'` (`:307-326`) with:

```ts
  it('redirects to end_session_endpoint carrying post_logout_redirect_uri, a state and the stored id_token_hint', async () => {
    const sid = 'sid-redirect';
    await store.set(sid, seededRecord(), 60_000, null);
    const oidc = fakeOidc({ endSessionUrl: END_SESSION_URL });
    runtime = baseRuntime(oidc);

    const res = await createAuthRoutes(runtime).handle(logoutRequest(`${SESSION_COOKIE}=${sid}`));

    expect(res.status).toBe(302);
    expect(res.headers.get('location')).toBe(END_SESSION_URL);
    expect(oidc.buildEndSessionUrlCalls).toHaveLength(1);
    const call = oidc.buildEndSessionUrlCalls[0];
    expect(call?.postLogoutRedirectUri).toBe(POST_LOGOUT_REDIRECT_URI);
    expect(call?.state).toBeTruthy();
    // SMA-681 AC 1: the hint is the raw ID token the record holds, byte for byte.
    expect(call?.idTokenHint).toBe(STORED_ID_TOKEN);
  });
```

Add a new `describe` directly after `describe('POST /auth/logout — end-session redirect', …)` (it ends at `:387`):

```ts
// SMA-681 § 4.5. The hint comes from the step 1 read. With no token to send, the request is the
// pre-SMA-681 request (client_id, post_logout_redirect_uri, state), and logout still completes.
describe('POST /auth/logout — id_token_hint (SMA-681)', () => {
  function completed(): Readonly<Record<string, string | number | boolean>> | undefined {
    return events.find(([name]) => name === 'logout.completed')?.[1];
  }

  it('logs idTokenHintSent: true when the redirect carries the hint, and never logs the token', async () => {
    const sid = 'sid-hint-sent';
    await store.set(sid, seededRecord(), 60_000, null);
    runtime = baseRuntime(fakeOidc());

    await createAuthRoutes(runtime).handle(logoutRequest(`${SESSION_COOKIE}=${sid}`));

    expect(completed()?.['idTokenHintSent']).toBe(true);
    // Review Focus 1: the event says THAT a hint went out, never WHAT it was.
    expect(JSON.stringify(events)).not.toContain(STORED_ID_TOKEN);
  });

  it('with no session cookie: no hint, the redirect still goes to the IdP, idTokenHintSent: false', async () => {
    const oidc = fakeOidc();
    runtime = baseRuntime(oidc);

    const res = await createAuthRoutes(runtime).handle(logoutRequest());

    expect(res.headers.get('location')).toBe(END_SESSION_URL);
    expect(oidc.buildEndSessionUrlCalls).toHaveLength(1);
    expect(oidc.buildEndSessionUrlCalls[0]).not.toHaveProperty('idTokenHint');
    expect(completed()?.['idTokenHintSent']).toBe(false);
  });

  it('with a cookie but no record: no hint, the redirect still goes to the IdP, idTokenHintSent: false', async () => {
    const oidc = fakeOidc();
    runtime = baseRuntime(oidc);

    const res = await createAuthRoutes(runtime).handle(logoutRequest(`${SESSION_COOKIE}=sid-with-no-record`));

    expect(res.headers.get('location')).toBe(END_SESSION_URL);
    expect(oidc.buildEndSessionUrlCalls[0]).not.toHaveProperty('idTokenHint');
    expect(completed()?.['idTokenHintSent']).toBe(false);
  });

  // Review Focus 5: the deploy moment. A record written before SMA-681 is version 1. The store reads
  // it as absent and deletes it, so logout sends no hint, and still completes at the IdP.
  it('a version 1 record from before the deploy: deleted, no hint, the redirect still goes to the IdP', async () => {
    const sid = 'sid-version-1';
    await store.set(sid, { ...seededRecord(), version: 1 } as unknown as SessionRecord, 60_000, null);
    const oidc = fakeOidc();
    runtime = baseRuntime(oidc);

    const res = await createAuthRoutes(runtime).handle(logoutRequest(`${SESSION_COOKIE}=${sid}`));

    expect(res.headers.get('location')).toBe(END_SESSION_URL);
    expect(await store.get(sid)).toBeNull();
    expect(oidc.buildEndSessionUrlCalls[0]).not.toHaveProperty('idTokenHint');
    expect(completed()?.['idTokenHintSent']).toBe(false);
  });

  // D4: there is no local `exp` check. Keycloak 26.4 accepts a hint 15 s past `exp` (spec § 3
  // row M-d), and RP-Initiated Logout 1.0 § 2 says the OP SHOULD accept an expired hint.
  it('sends an expired JWT-shaped stored token unchanged', async () => {
    const b64 = (value: unknown): string => Buffer.from(JSON.stringify(value)).toString('base64url');
    const expired = `${b64({ alg: 'RS256', typ: 'JWT' })}.${b64({ sub: 'a-subject', exp: Math.floor(Date.now() / 1000) - 3600 })}.signature`;
    const sid = 'sid-expired-hint';
    await store.set(sid, seededRecord({ idToken: expired }), 60_000, null);
    const oidc = fakeOidc();
    runtime = baseRuntime(oidc);

    await createAuthRoutes(runtime).handle(logoutRequest(`${SESSION_COOKIE}=${sid}`));

    expect(oidc.buildEndSessionUrlCalls[0]?.idTokenHint).toBe(expired);
    expect(completed()?.['idTokenHintSent']).toBe(true);
  });

  it('logs idTokenHintSent: false when buildEndSessionUrl throws, although the record held a token', async () => {
    const sid = 'sid-hint-degraded';
    await store.set(sid, seededRecord(), 60_000, null);
    const oidc = fakeOidc();
    oidc.buildEndSessionUrl = (): Promise<string> => Promise.reject(new Error('no end_session_endpoint'));
    runtime = baseRuntime(oidc);

    const res = await createAuthRoutes(runtime).handle(logoutRequest(`${SESSION_COOKIE}=${sid}`));

    expect(res.headers.get('location')).toBe(POST_LOGOUT_REDIRECT_URI);
    expect(completed()?.['endSessionRedirected']).toBe(false);
    expect(completed()?.['idTokenHintSent']).toBe(false);
  });
});
```

In `tests/support/store-failure.ts`:
- Change the type import (`:13`) to `import type { AuthorizationRequest, BuildEndSessionUrlParams, OidcClient, OidcTokens, RefreshedTokens } from '../../src/adapters/oidc.js';`
- Add to `FakeOidc` (`:53-57`), after `failRevoke`:

```ts
  /** The parameters of every `buildEndSessionUrl` call, in order (SMA-681: does it carry a hint?). */
  endSessionCalls: BuildEndSessionUrlParams[];
```

- In `fakeOidc()` add `endSessionCalls: [],` after `failRevoke: false,`, and replace the `buildEndSessionUrl` line (`:77`) with:

```ts
    buildEndSessionUrl: (params: BuildEndSessionUrlParams): Promise<string> => {
      oidc.endSessionCalls.push(params);
      return Promise.resolve(END_SESSION_URL);
    },
```

In `tests/http/store-unavailable.test.ts`, row 6 (`:205-222`): replace the `logout.completed` line in the `toEqual` (`:219`) with:

```ts
      ['logout.completed', { zone: 'iam', sid: sidTag(OLD_SID), revoked: false, endSessionRedirected: true, idTokenHintSent: false }],
```

and add after `expectEventsClean(h.events);` in row 6:

```ts
    // SMA-681 AC 3: a failed read gives no token, so the end-session request carries no hint.
    expect(h.oidc.endSessionCalls).toHaveLength(1);
    expect(h.oidc.endSessionCalls[0]).not.toHaveProperty('idTokenHint');
```

In row 7 (`'row 7: the delete fails -> revoke the read token, 503 with a POST form, cookie kept (D6)'`), add after `expectEventsClean(h.events);`:

```ts
    // Review Focus 2: the 503 branch does not redirect to the IdP, so no hint leaves the server.
    expect(h.oidc.endSessionCalls).toEqual([]);
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint/ts/packages/paigasus-auth exec vitest run tests/http/logout.test.ts tests/http/store-unavailable.test.ts
```

Expected: FAIL. `idTokenHint` is `undefined`, `idTokenHintSent` is missing, and row 6's strict `toEqual` fails. The two "no hint" `not.toHaveProperty` lines and row 7 already pass.

- [ ] **Step 3: Write the implementation**

In `src/http/routes.ts`, replace the comment block from `// \`id_token_hint\` IS DELIBERATELY OMITTED` (`:378`) through the NAMED RESIDUAL paragraph (it ends at `:399`) with:

```ts
// `id_token_hint` IS SENT WHEN THE RECORD HOLDS AN ID TOKEN (SMA-681). This reverses SMA-506.
// SMA-506 (design doc § 9.5) did not store the raw ID token. It argued that `client_id` plus a
// registered `post_logout_redirect_uri` is sufficient for Keycloak, and that the raw token is a
// third bearer credential in Redis. The SMA-506 measurement saw no confirmation page only because
// its realm grants `offline_access` as a default client scope: no online SSO session existed, so
// Keycloak had no session to ask about (SMA-682).
//
// OpenID Connect RP-Initiated Logout 1.0, section 2: the OP MUST ask the End-User whether to log
// out if an `id_token_hint` was not provided. Measured on Keycloak 26.4.7 (SMA-681 spec § 3): with a
// live SSO session and no hint, it answers 200 "Do you want to log out?" (row M-b). With the hint it
// redirects to `post_logout_redirect_uri` and ends the session (row M-c), also with a hint 15 s past
// its `exp` (row M-d) and with a refreshed hint (row M-e). With the shipped `offline_access` scope
// it also redirects at once (§ 3.1, rows M-i1, M-i2, M-i4). So this file sends the stored token
// unchanged and does no `exp` check (spec D4).
//
// `openid-client@6.8.8` still appends `client_id` whenever the caller supplies none
// (`build/index.js:1129-1141`), asserted in tests/adapters/oidc.test.ts. Keycloak rejects a hint
// whose `aud` is a different client (§ 3 row M-g). The adapter's `client_id` is the client the hint
// was issued to, so that case does not apply here.
//
// RESIDUAL RISK (spec § 5). The ID token is now in the redirect URL, so it goes into the browser
// history, the IdP's access log and the log of any TLS-terminating proxy. It carries the user's
// email, name and username in base64url, which is not encryption. A person who has it can end the
// user's Keycloak SSO session with no cookies (§ 3 row M-h). It gives no access to an API: its `aud`
// is the console client. Logout by POST would remove this exposure, but it needs an auto-submitting
// form and a CSP `form-action` change, and it is out of scope. An IdP with large ID tokens can
// exceed a request-line limit; that is not measured.
//
// NAMED RESIDUAL: an identity provider that REQUIRES `id_token_hint` still fails when there is no
// token to send: no session cookie, no record (absolute expiry, or the `version: 2` deploy removed
// it), or a failed store read. The server-side logout still succeeds then (step 1 already ran);
// only the end-session redirect does not complete.
```

In step 1, replace the two lines `let refreshToken: string | undefined;` … `refreshToken = rec === STORE_DOWN ? undefined : rec?.refreshToken;` (`:415-418`) with:

```ts
  let refreshToken: string | undefined;
  // SMA-681: the same read gives the hint for step 4. A failed read or no record gives none.
  let idToken: string | undefined;
  if (sid !== undefined) {
    const rec = await storeStep(runtime, 'logout_get', sid, () => runtime.store.get(sid));
    refreshToken = rec === STORE_DOWN ? undefined : rec?.refreshToken;
    idToken = rec === STORE_DOWN ? undefined : rec?.idToken;
```

Replace step 4's code (`:458-474`, from `const state = newTransactionId();` through the end of the `logout.completed` call) with:

```ts
  // `idTokenHintSent` (SMA-681) is true only when the Location is an end-session URL that carries
  // the hint. It is NOT named `idTokenHint`, so that no reader and no redaction rule takes it for the
  // token. It never contains the token.
  const state = newTransactionId();
  let endSessionRedirected = true;
  let idTokenHintSent = false;
  try {
    const endSessionUrl = await runtime.oidc.buildEndSessionUrl({
      postLogoutRedirectUri: runtime.postLogoutRedirectUri,
      state,
      ...(idToken !== undefined ? { idTokenHint: idToken } : {}),
    });
    headers.set('Location', endSessionUrl);
    idTokenHintSent = idToken !== undefined;
  } catch {
    // Never rethrown and never logged as a raw caught error object — same rule as step 3's catch.
    endSessionRedirected = false;
    headers.set('Location', runtime.postLogoutRedirectUri);
  }

  runtime.logger.event('logout.completed', {
    zone: runtime.zone,
    ...(sid !== undefined ? { sid: sidTag(sid) } : {}),
    revoked,
    endSessionRedirected,
    idTokenHintSent,
  });
```

Also add one sentence at the end of the STEP 4 comment paragraph (after the line that ends `…unlike step 3's revocation outcome.`, `:457`):

```ts
  // The hint is added only when step 1's read found a token (SMA-681).
```

- [ ] **Step 4: Change the package e2e spec**

In `tests/e2e/logout.spec.ts`, after `await expect(page.getByTestId('guarded-heading')).toBeVisible();` (`:17`), add:

```ts
  // SMA-681: the realm does not pin a user id, so the subject comes from the page the fixture
  // server renders (tests/e2e/fixture-server.ts:182).
  const subject = ((await page.getByTestId('principal-subject').textContent()) ?? '').trim();
  expect(subject, 'the guarded page must show the principal subject').not.toBe('');
```

Replace the comment paragraph at `:24-27` with:

```ts
  // SMA-681: routes.ts's handleLogout sends the stored raw ID token as `id_token_hint`, and
  // openid-client appends `client_id`. Keycloak 26.4 needs the hint to skip its confirmation page
  // when an SSO session is live (SMA-681 spec § 3). This realm grants `offline_access` as a default
  // scope, so no SSO session exists here and no page appears either way (SMA-682): this test proves
  // the hint is SENT, and that Keycloak still completes the redirect with it (§ 3.1 row M-i1).
```

Replace line `:44` (the `id_token_hint must be absent` assertion) with:

```ts
  const hint = endSessionUrl.searchParams.get('id_token_hint');
  expect(hint, 'id_token_hint must be present: the session record stores the raw ID token (SMA-681)').not.toBeNull();
  const parts = (hint ?? '').split('.');
  expect(parts, 'id_token_hint must be a three-part JWT').toHaveLength(3);
  const payload = JSON.parse(Buffer.from(parts[1] ?? '', 'base64url').toString('utf8')) as { sub?: unknown; aud?: unknown };
  expect(payload.sub, 'the hint belongs to the signed-in user').toBe(subject);
  const audiences = Array.isArray(payload.aud) ? payload.aud : [payload.aud];
  expect(audiences, 'the hint was issued to this client').toContain(KEYCLOAK_CLIENT_ID);
```

- [ ] **Step 5: Add the README paragraph**

In `ts/packages/paigasus-auth/README.md`, after the paragraph that ends `own end-session endpoint.` (`:193`), add:

```markdown

### Logout sends `id_token_hint`

The session record stores the raw ID token (`idToken`, SMA-681), and logout sends it to the IdP's
end-session endpoint as `id_token_hint`. Without the hint, OpenID Connect RP-Initiated Logout 1.0
requires the IdP to ask "Do you want to log out?", and Keycloak 26.4 does when an SSO session is
live. The token is sent also after its `exp`, because Keycloak accepts an expired hint. With no
session cookie, no record, or a failed store read, logout sends no hint and still redirects to the
IdP. The hint puts the ID token into the redirect URL, so the browser history and the IdP's access
log hold it. The record changed to `version: 2` for this: the deploy logs out every active user.
```

- [ ] **Step 6: Run the tests, the typecheck and Prettier on the changed files**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ROOT=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint/ts
pnpm -C "$ROOT/packages/paigasus-auth" exec vitest run
pnpm -C "$ROOT/packages/paigasus-auth" exec tsc -p tsconfig.json --noEmit
pnpm -C "$ROOT" exec prettier --check packages/paigasus-auth
```

Expected: all PASS. tsc exits 0. Prettier reports no file (run `pnpm -C "$ROOT" exec prettier --write <file>` on any file it names, then check again).

- [ ] **Step 7: Run the package e2e tier if Docker is available**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
docker info >/dev/null 2>&1 && echo DOCKER_OK || echo DOCKER_MISSING
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint
moon run paigasus-auth-ts:test-e2e
```

Run the `moon run` line only when the first command prints `DOCKER_OK`. Expected: `logout.spec.ts` passes with the new hint assertions, and the container suites pass. When Docker is missing, write in the report: "paigasus-auth-ts:test-e2e NOT RUN LOCALLY: needs Docker. CI runs it." Do not call the task fully verified in that case.

- [ ] **Step 8: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint
git add ts/packages/paigasus-auth
git commit -m "$(cat <<'EOF'
feat(ts): send id_token_hint on logout (SMA-681)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
git log --oneline -1
```

---

### Task 6: The console fake IdP records the hint, and iam-console R12 asserts it

This is the proof in a REQUIRED CI tier (spec § 4.6). The chart job that runs J1 is not a required check (`.github/workflows/chart.yml:57`).

**Files:**
- Modify: `ts/packages/paigasus-console-core/testing/fake-idp.ts:19-30` (the type), `:71-76` (state), `:122-123` (the code grant), `:193-206` (`/logout`), `:234-246` (the return)
- Test: `ts/apps/iam-console/tests/integration/doubles/fake-idp.test.ts` (a new case)
- Modify: `ts/apps/iam-console/tests/e2e/login.spec.ts:60-80` (R12)

**Interfaces:**
- Consumes: logout sends the hint (Task 5).
- Produces: `FakeIdp.idTokens: string[]` (every `id_token` that `/token` issued, in order; a refresh issues none) and `FakeIdp.endSessionHints: (string | null)[]` (the `id_token_hint` of every `/logout` request, in order; `null` when absent).

- [ ] **Step 1: Write the failing test**

In `ts/apps/iam-console/tests/integration/doubles/fake-idp.test.ts`, add this case at the end of the `describe`:

```ts
  // SMA-681: the e2e tier asserts that logout sends the login ID token as id_token_hint. It can
  // only do that when the fake records what it issued and what /logout received.
  it('records the id_token it issues, and the id_token_hint of each /logout request', async () => {
    const verifier = randomBytes(32).toString('base64url');
    const body = JSON.parse((await exchange(await authorize(REDIRECT, verifier), REDIRECT, verifier)).body) as { id_token: string };
    expect(idp.idTokens.at(-1)).toBe(body.id_token);

    const logout = (params: Record<string, string>): Promise<{ status: number }> => {
      const url = new URL(`${idp.issuer}/logout`);
      url.search = new URLSearchParams(params).toString();
      return httpsRequest(url.toString(), tls);
    };
    expect((await logout({ post_logout_redirect_uri: 'https://127.0.0.1:9/iam/', id_token_hint: body.id_token, state: 's' })).status).toBe(302);
    expect(idp.endSessionHints.at(-1)).toBe(body.id_token);

    expect((await logout({ post_logout_redirect_uri: 'https://127.0.0.1:9/iam/' })).status).toBe(302);
    expect(idp.endSessionHints.at(-1)).toBeNull();
  });
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint/ts/apps/iam-console exec vitest run tests/integration/doubles/fake-idp.test.ts
```

Expected: FAIL, `Cannot read properties of undefined (reading 'at')`.

- [ ] **Step 3: Write the implementation**

In `ts/packages/paigasus-console-core/testing/fake-idp.ts`, add to the `FakeIdp` type after `issued` (`:28`):

```ts
  /** Every id_token /token issued, in order. Only an authorization_code grant issues one. */
  readonly idTokens: string[];
  /** The `id_token_hint` of every /logout request, in order: `null` when a request had none (SMA-681). */
  readonly endSessionHints: (string | null)[];
```

Add the state after `const issued: … = [];` (`:75`):

```ts
  const idTokens: string[] = [];
  const endSessionHints: (string | null)[] = [];
```

In `token()`, replace the two lines at `:122-123` (`const pair = mint();` and the `json(res, 200, …)` of the authorization_code branch) with:

```ts
      const pair = mint();
      const issuedIdToken = await idToken(pending.nonce);
      idTokens.push(issuedIdToken);
      json(res, 200, { access_token: pair.accessToken, refresh_token: pair.refreshToken, id_token: issuedIdToken, token_type: 'Bearer', expires_in: 3600 });
```

In `handle()`, make the first statement of the `/logout` branch (`:193`) record the hint:

```ts
    if (req.method === 'GET' && url.pathname === '/logout') {
      endSessionHints.push(url.searchParams.get('id_token_hint'));
      const target = url.searchParams.get('post_logout_redirect_uri');
```

Add both arrays to the returned object after `issued,` (`:240`):

```ts
    idTokens,
    endSessionHints,
```

In `ts/apps/iam-console/tests/e2e/login.spec.ts` R12, add after `expect((await request.response())?.status()).toBe(302);` (`:74`):

```ts
  // SMA-681 AC 1, in a REQUIRED tier: the end-session request carried the login ID token as
  // id_token_hint. The fake IdP issues no ID token on a refresh, so the last one issued is the
  // login's. J1 (tests/cluster/journeys/auth-roundtrip.spec.ts) proves the same against Keycloak,
  // in the chart job, which is not a required check.
  const loginIdToken = harness.idp.idTokens.at(-1);
  expect(loginIdToken, 'the fake IdP issued an id_token at login').toBeDefined();
  expect(harness.idp.endSessionHints.at(-1), 'the end-session request carries the login id_token as id_token_hint').toBe(loginIdToken);
```

- [ ] **Step 4: Run the tests and the typechecks**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ROOT=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint/ts
pnpm -C "$ROOT/apps/iam-console" exec vitest run tests/integration/doubles/fake-idp.test.ts
pnpm -C "$ROOT/packages/paigasus-console-core" exec tsc -p tsconfig.json --noEmit
pnpm -C "$ROOT/apps/iam-console" exec tsc -p tsconfig.json --noEmit
pnpm -C "$ROOT/apps/gateway-console" exec tsc -p tsconfig.json --noEmit
```

Expected: PASS. All three tsc runs exit 0. gateway-console imports `FakeIdp` too; the new fields only add to the type.

- [ ] **Step 5: Run the iam-console e2e tier (no Docker needed)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint
moon run iam-console-ts:test-e2e
```

Expected: every spec passes, R12 included. This task depends on `~:build`, so the first run makes a Next production build. If a build or an e2e hang blocks it, write the exact error in the report. Do not mark R12 as verified.

- [ ] **Step 6: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint
git add ts/packages/paigasus-console-core/testing/fake-idp.ts ts/apps/iam-console/tests/integration/doubles/fake-idp.test.ts ts/apps/iam-console/tests/e2e/login.spec.ts
git commit -m "$(cat <<'EOF'
test(ts): assert id_token_hint on logout in the iam-console e2e tier (SMA-681)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
git log --oneline -1
```

---

### Task 7: J1 asserts the hint against Keycloak on the kind stack

J1 runs only in CI, in `chart.yml` (`ci/kind/run.sh specs journeys`), which needs kind and CI secrets. The implementer runs only the static checks below. The PR must link a green chart run as the J1 evidence (spec § 4.6).

**Files:**
- Modify: `ts/apps/iam-console/tests/cluster/journeys/auth-roundtrip.spec.ts:24-28` (the D9 comment), `:130-134` (step 4)

**Interfaces:**
- Consumes: logout sends the hint (Task 5).
- Produces: nothing for later tasks.

- [ ] **Step 1: Rewrite the D9 comment**

Replace `:24-28` with:

```ts
// D9: step 4 does not click a logout confirmation page. On the kind stack Keycloak shows none,
// because no SSO session exists (SMA-682). J1 never had a click to remove. Since SMA-681, logout
// sends `id_token_hint`, and step 4 asserts it: a three-part JWT issued to `paigasus-console`. With
// the hint, Keycloak 26.4 redirects at once also when an SSO session is live (SMA-681 spec § 3 rows
// M-c, M-e), so the page does not return when SMA-682 restores the session. SMA-682's SSO check is
// the end-to-end proof of that.
```

- [ ] **Step 2: Add the assertion to step 4**

In the step `'logout: the shell form ends at the IdP and returns to /iam/ with no session cookie'` (do NOT change this title), add directly after the `client_id` assertion (`:131`):

```ts
    // SMA-681 AC 2: the end-session request carries the stored ID token. Keycloak's ID token names
    // the client in `aud`, and also in `azp`.
    const hint = new URL(endSessionRequest.url()).searchParams.get('id_token_hint');
    expect(hint, 'end-session id_token_hint (SMA-681)').not.toBeNull();
    const parts = (hint ?? '').split('.');
    expect(parts, 'id_token_hint is a three-part JWT').toHaveLength(3);
    const claims = JSON.parse(Buffer.from(parts[1] ?? '', 'base64url').toString('utf8')) as { aud?: unknown; azp?: unknown };
    const audiences = Array.isArray(claims.aud) ? claims.aud : [claims.aud];
    expect(audiences.includes('paigasus-console') || claims.azp === 'paigasus-console', 'id_token_hint was issued to paigasus-console (aud or azp)').toBe(true);
```

Replace the comment at `:133-134` with:

```ts
    // D9: on the kind stack no SSO session exists (SMA-682), so Keycloak redirects at once. With
    // the hint asserted above, it also redirects at once when a session exists (SMA-681 spec § 3).
```

- [ ] **Step 3: Run the static checks**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
ROOT=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint
pnpm -C "$ROOT/ts/apps/iam-console" exec tsc -p tsconfig.json --noEmit
node "$ROOT/ci/kind/journeys-report.mjs" sources "$ROOT/ts/apps/iam-console/tests/cluster/journeys"; echo "rc=$?"
node --test "$ROOT/ci/kind/journeys-report.test.mjs"
pnpm -C "$ROOT/ts" exec prettier --check apps/iam-console/tests/cluster/journeys/auth-roundtrip.spec.ts
```

Expected: tsc exits 0. `journeys-report.mjs sources` prints its pass line with `rc=0` (the step titles did not change). The node tests pass. Prettier reports nothing. Write in the report: "J1 NOT RUN LOCALLY: CI only (chart.yml)".

- [ ] **Step 4: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint
git add ts/apps/iam-console/tests/cluster/journeys/auth-roundtrip.spec.ts
git commit -m "$(cat <<'EOF'
test(ts): assert id_token_hint in the kind j1 logout step (SMA-681)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
EOF
)"
git log --oneline -1
```

---

### Task 8: Final verification through Moon

**Files:** none changed, unless a gate finds a defect. In that case, fix it in the file it names and commit it as `fix(ts): …`.

- [ ] **Step 1: Run the per-project targets and the whole-tree lint and format gates**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint
moon run paigasus-auth-ts:typecheck paigasus-auth-ts:test \
  paigasus-console-core-ts:typecheck paigasus-console-core-ts:test \
  gateway-console-ts:typecheck gateway-console-ts:test \
  iam-console-ts:typecheck iam-console-ts:test iam-console-ts:test-e2e \
  ts:lint ts:fmt
```

Expected: every target passes. `ts:lint` is the only lint target (finding F3: there is no `paigasus-auth-ts:lint`). `ts:fmt` is the separate whole-tree Prettier gate. If a target fails, read `.moon/cache/states/<project>/<task>/stderr.log` and `stdout.log` before any re-run (root CLAUDE.md, "Diagnosing an unattributed `moon ci` failure").

- [ ] **Step 2: Run the Docker tiers if Docker is available**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint
if docker info >/dev/null 2>&1; then
  moon run paigasus-auth-ts:test-e2e gateway-console-ts:test-e2e
else
  echo "DOCKER_MISSING: the Docker tiers are NOT RUN LOCALLY; CI runs them"
fi
```

Expected with Docker: both pass. Without Docker: record the line in the report.

- [ ] **Step 3: Check the branch history**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-681-id-token-hint
git log --oneline origin/main..HEAD
git status --short
```

Expected: the spec commit, this plan's commit (added by the controller), and seven task commits. No uncommitted file.

The full `repo:*` gate graph (the `ci-targets` block in the root `CLAUDE.md`) runs in the open-pr stage, not here.

---

### Task 9 (CONTROLLER ONLY — not for an implementer subagent): the ADR note, the SMA-682 comment and the PR description

An implementer does not do this task. The controller does it after Task 8 is green (spec § 4.7).

- [ ] **Step 1: Add a dated note under ADR-0017, Decision 3, in Notion**

Find the page with `notion-search` for "ADR-0017". Fetch it and find Decision 3 (the Redis session contents) and the SMA-511 note already on it. Add this note directly under Decision 3, in the same style as the SMA-511 note:

> **Note, 2026-09-25 (SMA-681).** The Redis session record also holds the raw ID token (`idToken`), next to the access token and the refresh token. Logout sends it to the IdP as `id_token_hint`. Without the hint, OpenID Connect RP-Initiated Logout 1.0 requires the IdP to ask "Do you want to log out?", and Keycloak 26.4 does when an SSO session is live. The record version changed from 1 to 2. The deploy of this change logs out every active user. The ID token adds personal data to the record, and a person who has it can end the user's Keycloak SSO session. It gives no access to an API. Spec: `docs/superpowers/specs/2026-09-25-sma-681-logout-id-token-hint-design.md`.

- [ ] **Step 2: Add a comment on Linear SMA-682**

Use `save_comment` on issue SMA-682 with this text:

> SMA-681 changes logout. Logout now sends the stored ID token as `id_token_hint`. When SMA-682 restores the SSO session, add one assertion to the J1 SSO check: after "Sign out", Keycloak must not show the page "Do you want to log out?". The browser must go from the end-session request directly back to `/iam/`. This assertion proves SMA-681 acceptance criterion 1 end to end. The SMA-681 measurement (spec § 3, rows M-b and M-c) shows that Keycloak 26.4 shows the page when the hint is absent, and skips it when the hint is present.

- [ ] **Step 3: Put these points in the PR description (open-pr stage)**

- The deploy logs out every active user: `SessionRecord` changed to `version: 2`. During a rolling update, users can see a login loop until the rollout ends. A rollback forces a second logout.
- The ID token is now in the end-session redirect URL (spec § 5).
- A link to a green `chart.yml` run as the J1 evidence. That job is not a required check.
- Which e2e tiers ran locally, and which ran only in CI.

---

## Self-Review

**1. Spec coverage.**

| Spec | Task |
|---|---|
| § 4.1 `SessionRecord` v2, `idToken`, field comments, the `version` comment, `isSessionRecord`, `toSessionView` unchanged | 2 |
| § 4.2 `OidcTokens.idToken`, the unreachable type check with no test | 1 |
| § 4.2 the core and adapter `RefreshedTokens`, `refresh` sets both only when present, validation comment | 3 (adapter), 4 (core) |
| § 4.3 the D5 store, the D6 mismatch (delete, revoke, two events, `null`), no-token keeps, `idTokenRotated`, the union, no `sid` compare | 4 |
| § 4.4 the callback stores `version: 2` and `idToken` | 2 |
| § 4.5 the step 1 read, the conditional hint, the fallback, the 503 branch unchanged, `idTokenHintSent`, the comment rewrite | 5 |
| § 4.6 unit tests (session, parse, memory, store-failure, store-unavailable row 6, callback, logout, single-flight, oidc), the fixtures, the stale comments | 1-5 |
| § 4.6 the package e2e, J1, the fake IdP plus R12 | 5, 7, 6 |
| § 4.7 the ADR note, the SMA-682 comment, the PR description | 9 |
| § 6 AC 1-4 | AC 1: 5, 6, 7. AC 2: 7. AC 3: 5. AC 4: 2 |

**2. Placeholder scan.** Every code step shows the code. No step says "TBD", "similar to", or "add error handling".

**3. Type consistency.** `idToken` / `idTokenClaims` (record and both `RefreshedTokens`), `idTokenHint` (the adapter parameter, unchanged), `idTokenHintSent` / `idTokenRotated` (the events), `endSessionCalls` (the auth test fake), `idTokens` / `endSessionHints` (the console fake IdP), `STORED_ID_TOKEN`, `FAKE_ID_TOKEN` and `ResolveDeps.revoke` are each spelled the same way in every task that uses them.

**4. Review Focus.** Each of the five lines has a test in its owning task: 1 → Tasks 4 and 5. 2 → Task 5 (row 7). 3 → Task 4. 4 → Task 4. 5 → Task 5.
