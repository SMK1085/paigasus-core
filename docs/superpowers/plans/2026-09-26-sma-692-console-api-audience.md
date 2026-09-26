# Console API Audience Request (SMA-692) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An Auth0 or Entra ID operator can make both consoles request an access token for a dedicated API audience. Auth0 gets an optional `audience` authorization parameter. Entra ID gets a chart-settable scope list. An operator who sets neither value gets the same authorization request and the same refresh request as today.

**Architecture:** `@paigasus/auth` gets one new optional env (`PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE`). `PAIGASUS_OIDC_SCOPES` becomes optional with no default and must hold the token `openid`. `createAuthRuntime` applies the default scope list for the authorization request only, and injects `scopes`, `audience` and `refreshScope` into `createOidcClient` through a new factory dependency (spec D3-a, D4). The adapter sends `audience` and the refresh `scope` only when they are set. A transient refresh failure now carries its OAuth error code from a closed set, and the `session.refresh_failed` log line shows it (D10). The Helm chart gets `oidc.scopes` and `oidc.authorizationAudience`, rendered into the `console-env` ConfigMap only when set, with two refusing helpers in `templates/_audience.tpl` (D6-D9). The runbook states the Auth0 and Entra ID setup.

**Tech Stack:** TypeScript 5 (strict, `exactOptionalPropertyTypes`), zod 4, openid-client 6.8.8, vitest 5, Moon 2.5.3 through proto shims, Helm 3 (proto-pinned) with sprig, bash (chart scripts run under `/bin/bash` 3.2.57 and bash 5), Python 3 with PyYAML (inline in the chart scripts).

**Spec:** docs/superpowers/specs/2026-09-26-sma-692-console-api-audience-design.md

## Global Constraints

- Worktree: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692`, branch `feature/sma-692-console-api-audience`. A subagent starts in the MAIN checkout. It must `cd` to the worktree and run `git branch --show-current` first. The output must be `feature/sma-692-console-api-audience`.
- Every command block starts with `cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692` and `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Run every `helm` and chart-script command from the worktree root. Outside the repository the proto shim finds no `.prototools` and runs another helm, and the goldens then differ for a reason that is not this change.
- In this sandbox a Bash call with a shell loop or a variable can be refused. If so, write the commands to a script file in your session scratchpad and run it with `/bin/bash <script>`.
- Every new source file opens with an SPDX header. This plan creates no new source file. Keep the existing headers.
- Rust edition rules do not apply: this plan changes no Rust.
- Commit scopes: `feat(ts)` for `ts/`, `feat(repo)` for `charts/` and `ci/helm-render/fixtures/`, `docs(repo)` for `docs/ops/`. `charts` and `ops` are NOT valid scopes (`ts/packages/commitlint-config/index.cjs:42`).
- Commit subjects are lowercase after the scope. Header max 100 characters. Body lines max 100 characters.
- No commit body line starts with `#` followed by a number. No body line has the form `Word: value`. Only the trailer has that form. Such lines fail `footer-leading-blank`.
- Every commit message ends with one blank line and then `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never use `--no-verify`. If `commitlint` is not found, provision the worktree (`proto install`, then `pnpm -C ts install`) and commit again.
- Never use `git commit --amend`. Never use `git reset HEAD~1` or any `git reset`. Never use `git stash`. Each task makes one new commit.
- Remove every mutation with the Edit tool (the inverse Edit). Never use `git checkout --` or `git restore`: they also discard the uncommitted change under test.
- Stage exact paths with `git add <path>`. Never `git add -A` or `git add .`.
- No background jobs. Run every command in the foreground (no `run_in_background`, no `&`).
- Do not install software (no `brew`, no `pip install`). If a tool is missing, stop and report.
- The golden files `charts/paigasus/tests/golden/iam-only.yaml` and `iam-and-gateway.yaml` stay byte-identical. Never run `render.sh --update`. `git diff --stat charts/paigasus/tests/golden` must print nothing at the end of every task. A diff there is a defect: stop and report.
- Write every new comment, doc text and commit message in ASD-STE100 Simplified Technical English: short sentences, active voice, no idiom. Keep technical names as they are.
- The log contract (`src/ports/logger.ts:6-12`) stays: a log field never holds an error object, an error message, a URL or a token. `AuthEventFields` is a `Record<string, string | number | boolean>` with NO per-key allow-list, so the closed value set of the new field comes from the `TokenErrorCode` type and `toTokenErrorCode`, not from the logger.

## Review Focus

1. **An absent value sent as the string `"undefined"` (spec F7).** openid-client turns `{ audience: undefined }` or `{ scope: undefined }` into the literal string `"undefined"`. The code must leave the key out. Task 2 pins it: the authorization URL has no `audience` key and no `undefined` substring, and the refresh body with no `refreshScope` has exactly the four keys of today. Mutation 2c (always pass `{ scope: opts.refreshScope }`) must red the four-key row.
2. **"Absent" and "default" confused (D3-a).** If `authEnvShape` keeps a `.default(...)`, or the runtime sets `refreshScope` from the defaulted list, every deployment starts to send a refresh `scope` (F8, F9). Task 3 pins it: the parsed config has `PAIGASUS_OIDC_SCOPES === undefined` when the env is absent, and the factory receives no `refreshScope` and no `audience` then. Mutations 3a and 3b must red.
3. **Token-exact `openid`, on both sides.** `openidx`, `openid-connect`, `openid` in the last position, a tab separator and a whitespace-only value. The TS parse (Task 3) and the chart helper (Task 4) must give the same answer. Task 3 pins it with accept and refuse tables. Task 4 pins `openidx` and `profile openid` in `refusals.sh`.
4. **Helm value shapes.** A key that is absent under `--reuse-values` (`--set x=null`), a number from `--set` on both sides of D7, and `oidc.audience` empty while `oidc.authorizationAudience` is set. Task 4 pins them with rows `O3 reuse-values-no-key`, `O4 number` and the refusal `authorizationAudience with oidc.audience empty`.
5. **The D10 code crosses a module copy and comes from the IdP.** The error is built in one copy of `core/errors.ts` and read in another (SMA-657), and the `.error` string is the IdP's. A forged or foreign error must log a value from the closed set only: an unknown string logs `other`. Task 1 pins it with a second-copy row and a forged-object row. Mutation 1b (read `oauthError` without `toTokenErrorCode`) must red the forged row.

## File Structure

| File | Task | Change |
| -- | -- | -- |
| `ts/packages/paigasus-auth/src/core/errors.ts` | 1, 3 | `TOKEN_ERROR_CODES`, `TokenErrorCode`, `toTokenErrorCode`, `RefreshFailed`, `refreshFailureCode` (1). A stale sentence in the `RefreshRejected` comment (3) |
| `ts/packages/paigasus-auth/src/core/single-flight.ts` | 1 | `oauthError` field on `session.refresh_failed` |
| `ts/packages/paigasus-auth/src/ports/logger.ts` | 1 | One comment line on `oauthError` |
| `ts/packages/paigasus-auth/tests/core/errors.test.ts` | 1 | Rows for the two new functions |
| `ts/packages/paigasus-auth/tests/core/single-flight.test.ts` | 1 | A new describe block for D10 |
| `ts/packages/paigasus-auth/README.md` | 1, 3 | Redaction paragraph (1). The env table rows (3) |
| `ts/packages/paigasus-auth/src/adapters/oidc.ts` | 2 | `scopes`, `audience`, `refreshScope` options; `BuildAuthorizationUrlParams` loses `scopes`; the classifier returns `RefreshFailed` |
| `ts/packages/paigasus-auth/tests/fixtures/jwks.ts` | 2 | The `tokenRequests()` recorder |
| `ts/packages/paigasus-auth/tests/adapters/oidc.test.ts` | 2 | New rows; `scopes` in the two `createOidcClient` calls |
| `ts/packages/paigasus-auth/src/http/routes.ts` | 2 | The `buildAuthorizationUrl` call drops `scopes` |
| `ts/packages/paigasus-auth/src/runtime.ts` | 2, 3 | Passes `scopes` (2). Factory dependency, D3-a, audience, remove `AuthRuntime.scopes` (3) |
| `ts/packages/paigasus-auth/tests/http/{login,callback,route-handler}.test.ts` | 2, 3 | `scopes` in `createOidcClient` (2); drop `AuthRuntime.scopes` (3) |
| `ts/packages/paigasus-auth/src/config.ts` | 3 | The two env keys |
| `ts/packages/paigasus-auth/tests/config.test.ts` | 3 | New rows; the default row changes |
| `ts/packages/paigasus-auth/tests/runtime.test.ts` | 3 | Factory rows replace the `rt.scopes` row |
| `ts/packages/paigasus-auth/tests/{server.test.ts,support/store-failure.ts,next/get-session.test.ts,http/logout.test.ts,http/login-returnto-table.test.ts}` | 3 | Drop `scopes` from the `AuthRuntime` literal |
| `ts/packages/paigasus-auth/tests/e2e/fixture-server.ts` | 3 | Drop the explicit `PAIGASUS_OIDC_SCOPES`, so the e2e tier runs the default path |
| `charts/paigasus/values.yaml` | 4 | `oidc.scopes`, `oidc.authorizationAudience` |
| `charts/paigasus/templates/_audience.tpl` | 4 | `paigasus.consoleScopes`, `paigasus.consoleAuthorizationAudience` |
| `charts/paigasus/templates/console-env-configmap.yaml` | 4 | Two conditional keys |
| `ci/helm-render/fixtures/leaked-value/templates/console-env-configmap.yaml` | 4 | Re-sync with the live file, keep the mutation |
| `charts/CLAUDE.md` | 4 | The fixture-copy bullet names all six fixtures |
| `charts/paigasus/tests/env.sh` | 4 | Rows O1-O6 and a row counter |
| `charts/paigasus/tests/refusals.sh` | 4 | D7 and D9 rows |
| `charts/paigasus/README.md` | 4 | A new section for the two values |
| `docs/ops/RUNBOOK-chart.md` | 5 | §§ 1, 2, 5, 6 |

The apps (`ts/apps/*/lib/config.ts`) spread `authEnvShape` and read neither key. They need no edit. Their typecheck runs in Task 6.

---

### Task 1: The OAuth error code of a transient refresh failure (D10, core side)

**Files:**
- Modify: `ts/packages/paigasus-auth/src/core/errors.ts:129` (append after the `RefreshRejected` class, end of file)
- Modify: `ts/packages/paigasus-auth/src/core/single-flight.ts:2` (import), `:208-212` (the log line)
- Modify: `ts/packages/paigasus-auth/src/ports/logger.ts:11-12` (comment)
- Modify: `ts/packages/paigasus-auth/README.md:149-156` (Redaction)
- Test: `ts/packages/paigasus-auth/tests/core/errors.test.ts:8` (import), append after `:71`
- Test: `ts/packages/paigasus-auth/tests/core/single-flight.test.ts:5` (import), append after `:1007`

**Interfaces:**
- Consumes: `hasAuthErrorCode(err: unknown, code: string): boolean` (`errors.ts:68`, module-private), `AuthError` (`errors.ts:4`).
- Produces:
  - `export const TOKEN_ERROR_CODES: readonly ['invalid_request', 'invalid_client', 'invalid_grant', 'unauthorized_client', 'unsupported_grant_type', 'invalid_scope']`
  - `export type TokenErrorCode = (typeof TOKEN_ERROR_CODES)[number]`
  - `export function toTokenErrorCode(value: unknown): TokenErrorCode | 'other'`
  - `export class RefreshFailed extends AuthError { readonly code: 'oidc_refresh_failed'; readonly oauthError: TokenErrorCode | 'other'; constructor(oauthError: TokenErrorCode | 'other', causeName: string) }` with `message === 'oidc refresh_token_grant failed: ' + causeName`
  - `export function refreshFailureCode(err: unknown): TokenErrorCode | 'other' | undefined`
  - The event `session.refresh_failed` gets the field `oauthError: TokenErrorCode | 'other'` only when `refreshFailureCode(err)` is defined and the failure is not a rejection.

- [ ] **Step 1: Write the failing errors tests**

In `tests/core/errors.test.ts`, replace the import line 8:

```ts
import { CallbackRejected, RefreshRejected, SessionStoreTimeout, SessionStoreUnavailable, isRefreshRejected, isSessionStoreUnavailable } from '../../src/core/errors.js';
```

with:

```ts
import {
  CallbackRejected,
  RefreshFailed,
  RefreshRejected,
  SessionStoreTimeout,
  SessionStoreUnavailable,
  TOKEN_ERROR_CODES,
  isRefreshRejected,
  isSessionStoreUnavailable,
  refreshFailureCode,
  toTokenErrorCode,
} from '../../src/core/errors.js';
```

Append at the end of the file:

```ts

// SMA-692 D10. The refresh log line carries the OAuth code of a transient failure. The IdP writes
// that code, so only a value of the RFC 6749 § 5.2 list may reach the log. Any other value is
// 'other'.
describe('toTokenErrorCode (SMA-692 D10)', () => {
  it('keeps each code of the closed list', () => {
    for (const code of TOKEN_ERROR_CODES) {
      expect(toTokenErrorCode(code)).toBe(code);
    }
  });

  it('gives other for any value outside the list', () => {
    expect(toTokenErrorCode('server_error')).toBe('other');
    expect(toTokenErrorCode('INVALID_SCOPE')).toBe('other');
    expect(toTokenErrorCode('https://idp.example.com/token?code=abc')).toBe('other');
    expect(toTokenErrorCode(undefined)).toBe('other');
    expect(toTokenErrorCode(42)).toBe('other');
  });
});

describe('refreshFailureCode (SMA-692 D10)', () => {
  it('reads the code of a RefreshFailed', () => {
    expect(refreshFailureCode(new RefreshFailed('invalid_scope', 'ResponseBodyError'))).toBe('invalid_scope');
  });

  it('reads the code of a RefreshFailed from a SECOND copy of core/errors', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    // Precondition: without this, the test passes vacuously if the import returns the same module.
    expect(foreign.RefreshFailed).not.toBe(RefreshFailed);
    expect(refreshFailureCode(new foreign.RefreshFailed('invalid_client', 'ResponseBodyError'))).toBe('invalid_client');
  });

  it('gives other for an Error that only claims the code and holds any string', () => {
    const forged = Object.assign(new Error('x'), { code: 'oidc_refresh_failed', oauthError: 'https://idp.example.com/secret' });
    expect(refreshFailureCode(forged)).toBe('other');
  });

  it('is undefined for every other error and for a non-error', () => {
    expect(refreshFailureCode(new RefreshRejected('invalid_grant'))).toBeUndefined();
    expect(refreshFailureCode(new Error('oidc refresh_token_grant failed: TypeError'))).toBeUndefined();
    expect(refreshFailureCode({ code: 'oidc_refresh_failed', oauthError: 'invalid_scope' })).toBeUndefined();
    expect(refreshFailureCode(undefined)).toBeUndefined();
  });

  it('keeps the message free of the code and of any URL', () => {
    const err = new RefreshFailed('invalid_scope', 'ResponseBodyError');
    expect(err.message).toBe('oidc refresh_token_grant failed: ResponseBodyError');
    expect(err.code).toBe('oidc_refresh_failed');
  });
});
```

- [ ] **Step 2: Write the failing single-flight tests**

In `tests/core/single-flight.test.ts`, replace line 5:

```ts
import { RefreshRejected, SessionStoreTimeout } from '../../src/core/errors.js';
```

with:

```ts
import { RefreshFailed, RefreshRejected, SessionStoreTimeout } from '../../src/core/errors.js';
```

Append at the end of the file (after line 1007):

```ts

// ---------------------------------------------------------------------------------------------
// SMA-692 D10. A transient refresh failure used to log only `reason: 'transient'`. After a scope
// change, an `invalid_scope` then looked the same as a network error. The log line now carries
// the OAuth code, from a closed set. It never carries the error object, its message or a URL.
// ---------------------------------------------------------------------------------------------
describe('the OAuth code in the refresh log (SMA-692 D10)', () => {
  it('logs the OAuth code of a transient failure', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { logger, events } = recordingLogger();
    const failed = () => Promise.reject(new RefreshFailed('invalid_scope', 'ResponseBodyError'));

    await expect(resolveSession({ ...deps(store, failed), logger }, 's')).rejects.toBeInstanceOf(RefreshFailed);
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'transient', degraded: false, oauthError: 'invalid_scope' }]);
  });

  it('logs the code on the degraded path too, and keeps the session', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() + 30_000 }), 60_000, null);
    const { logger, events } = recordingLogger();
    const failed = () => Promise.reject(new RefreshFailed('invalid_client', 'ResponseBodyError'));

    const out = await resolveSession({ ...deps(store, failed), logger, skewMs: 60_000 }, 's');

    expect(out?.refreshState).toBe('failed');
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'transient', degraded: true, oauthError: 'invalid_client' }]);
    expect(await store.get('s')).not.toBeNull();
  });

  // SMA-657 shape: `refresh` runs in the copy that built the runtime, resolveSession in another.
  it('logs the code of a RefreshFailed from a SECOND module copy', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    expect(foreign.RefreshFailed).not.toBe(RefreshFailed);
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { logger, events } = recordingLogger();
    const failed = () => Promise.reject(new foreign.RefreshFailed('invalid_scope', 'ResponseBodyError'));

    await expect(resolveSession({ ...deps(store, failed), logger }, 's')).rejects.toBeInstanceOf(foreign.RefreshFailed);
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'transient', degraded: false, oauthError: 'invalid_scope' }]);
  });

  it('logs other, never the raw string, for an error that only claims the code', async () => {
    const store = new MemorySessionStore();
    await store.set('s', makeRecord({ accessExpiresAt: Date.now() - 1 }), 60_000, null);
    const { logger, events } = recordingLogger();
    const forged = Object.assign(new Error('x'), { code: 'oidc_refresh_failed', oauthError: 'https://idp.example.com/secret' });

    await expect(resolveSession({ ...deps(store, () => Promise.reject(forged)), logger }, 's')).rejects.toBe(forged);
    expect(events).toContainEqual(['session.refresh_failed', { sid: sidTag('s'), reason: 'transient', degraded: false, oauthError: 'other' }]);
  });
});
```

The existing rows at lines 223, 469, 479 and 490 stay unchanged. They use `toContainEqual` with the exact field set, so they also prove that a failure with no OAuth code, and a rejection, get NO `oauthError` field.

- [ ] **Step 3: Run the tests, expect FAIL**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-auth exec vitest run tests/core/errors.test.ts tests/core/single-flight.test.ts
```

Expected: FAIL. The new rows fail with `TypeError` or `is not a function` (the new exports do not exist yet). The old rows pass.

- [ ] **Step 4: Implement the errors half**

In `src/core/errors.ts`, append after the `RefreshRejected` class (after line 129):

```ts

/**
 * RFC 6749 § 5.2: the error codes of a token endpoint response. A CLOSED set (SMA-692 D10). The
 * refresh log line carries one of these codes, or 'other'. The IdP writes the response body, so
 * a value outside this list never reaches a log line as it is. The type and toTokenErrorCode are
 * one fact, like RefreshRejected's `oauthError` above.
 */
export const TOKEN_ERROR_CODES = ['invalid_request', 'invalid_client', 'invalid_grant', 'unauthorized_client', 'unsupported_grant_type', 'invalid_scope'] as const;

export type TokenErrorCode = (typeof TOKEN_ERROR_CODES)[number];

/** Maps an untrusted value onto TOKEN_ERROR_CODES. Any other value, and a non-string, gives 'other'. */
export function toTokenErrorCode(value: unknown): TokenErrorCode | 'other' {
  return (TOKEN_ERROR_CODES as readonly unknown[]).includes(value) ? (value as TokenErrorCode) : 'other';
}

/**
 * The identity provider answered a refresh with an OAuth error that is NOT definitive (SMA-626
 * § 2.2, SMA-692 D11): the session stays, and a later request tries again. `oauthError` is the
 * code from the closed set above (SMA-692 D10). The message holds only the class name of the
 * library error, as adapters/oidc.ts's wrapError does. It holds no code, no URL and no token.
 *
 * NOT exported from src/server.ts, for the reason RefreshRejected records above.
 */
export class RefreshFailed extends AuthError {
  readonly code = 'oidc_refresh_failed';
  constructor(
    readonly oauthError: TokenErrorCode | 'other',
    causeName: string,
  ) {
    super(`oidc refresh_token_grant failed: ${causeName}`);
  }
}

/**
 * The OAuth code of a RefreshFailed from ANY copy of this module, or undefined for every other
 * error (SMA-692 D10). It classifies by `code`, not `instanceof`, for the reason
 * isRefreshRejected records. The value goes through toTokenErrorCode again: an object that only
 * claims the code can hold any string.
 */
export function refreshFailureCode(err: unknown): TokenErrorCode | 'other' | undefined {
  if (!hasAuthErrorCode(err, 'oidc_refresh_failed')) return undefined;
  return toTokenErrorCode((err as { oauthError?: unknown }).oauthError);
}
```

- [ ] **Step 5: Implement the single-flight half**

In `src/core/single-flight.ts`, replace line 2:

```ts
import { isRefreshRejected } from './errors';
```

with:

```ts
import { isRefreshRejected, refreshFailureCode } from './errors';
```

Replace lines 208-212:

```ts
          const rejected = isRefreshRejected(err);
          const liveUntil = Math.min(fresh.accessExpiresAt, fresh.absoluteExpiresAt);
          const degraded = !rejected && Date.now() < liveUntil;

          logger.event('session.refresh_failed', { sid: sidTag(sid), reason: rejected ? 'rejected' : 'transient', degraded });
```

with:

```ts
          const rejected = isRefreshRejected(err);
          const liveUntil = Math.min(fresh.accessExpiresAt, fresh.absoluteExpiresAt);
          const degraded = !rejected && Date.now() < liveUntil;
          // SMA-692 D10. The OAuth code of a transient failure, from a closed set. It is never the
          // error object, its message or a URL. A failure with no OAuth code (a network error, a
          // timeout) gets no field. A rejection is always `invalid_grant`, so `reason` says it.
          const oauthError = rejected ? undefined : refreshFailureCode(err);

          logger.event('session.refresh_failed', { sid: sidTag(sid), reason: rejected ? 'rejected' : 'transient', degraded, ...(oauthError !== undefined ? { oauthError } : {}) });
```

- [ ] **Step 6: Update the logger contract comment and the README**

In `src/ports/logger.ts`, replace lines 11-12:

```ts
// Never pass a caught library error object into `fields`: node-redis embeds the DSN in its own
// connection errors and openid-client may include a URL. Extract `name` and a fixed message.
```

with:

```ts
// Never pass a caught library error object into `fields`: node-redis embeds the DSN in its own
// connection errors and openid-client may include a URL. Extract `name` and a fixed message.
//
// `session.refresh_failed` may carry `oauthError` (SMA-692 D10). Its value is a code of the RFC
// 6749 § 5.2 list or 'other', never the IdP's raw string: core/errors.ts's toTokenErrorCode maps
// it. This type admits any string key, so that function is the control, not this port.
```

In `ts/packages/paigasus-auth/README.md`, replace the paragraph at lines 151-156 (it starts `No event this package logs carries a token`) with:

```markdown
No event this package logs carries a token, a refresh token, an authorization code, the client
secret, the Redis connection string, or a transaction secret. A logged session id (`sid`) is
truncated to 8 characters — enough to correlate a support request, not enough to replay. This
holds even for an unhandled adapter error: this package never logs a caught node-redis or
`openid-client` error object directly (both can embed a URL or a DSN in their own error text) —
only a fixed name and message are extracted for logging.

`session.refresh_failed` can carry `oauthError` (SMA-692). It is the OAuth error code of a
transient refresh failure: one code of the RFC 6749 § 5.2 list, or `other` for any other value.
It shows, for example, an `invalid_scope` after a scope change. A failure with no OAuth code, such
as a network error, has no `oauthError`.
```

- [ ] **Step 7: Run the tests, expect PASS**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-auth exec vitest run tests/core/errors.test.ts tests/core/single-flight.test.ts
```

Expected: PASS, every row.

- [ ] **Step 8: Mutation 1a, delete the log field**

With the Edit tool, in `src/core/single-flight.ts` replace `degraded, ...(oauthError !== undefined ? { oauthError } : {}) });` with `degraded });`. Run the Step 7 command. Expected: FAIL in the four rows of `the OAuth code in the refresh log (SMA-692 D10)`. (The unused `oauthError` constant is a lint finding only; vitest does not report it.) Restore with the inverse Edit. Run again: PASS.

- [ ] **Step 9: Mutation 1b, read the code without the closed-set map**

With the Edit tool, in `src/core/errors.ts` replace `return toTokenErrorCode((err as { oauthError?: unknown }).oauthError);` with `return (err as { oauthError: TokenErrorCode | 'other' }).oauthError;`. Run the Step 7 command. Expected: FAIL in `gives other for an Error that only claims the code and holds any string` and in `logs other, never the raw string, for an error that only claims the code`. Restore with the inverse Edit. Run again: PASS.

- [ ] **Step 10: Typecheck, lint and format**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --write packages/paigasus-auth/src/core/errors.ts packages/paigasus-auth/src/core/single-flight.ts packages/paigasus-auth/src/ports/logger.ts packages/paigasus-auth/tests/core/errors.test.ts packages/paigasus-auth/tests/core/single-flight.test.ts packages/paigasus-auth/README.md
moon run paigasus-auth-ts:typecheck paigasus-auth-ts:test ts:lint ts:fmt
```

Expected: all four targets pass.

- [ ] **Step 11: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
git add ts/packages/paigasus-auth/src/core/errors.ts ts/packages/paigasus-auth/src/core/single-flight.ts ts/packages/paigasus-auth/src/ports/logger.ts ts/packages/paigasus-auth/tests/core/errors.test.ts ts/packages/paigasus-auth/tests/core/single-flight.test.ts ts/packages/paigasus-auth/README.md
git commit -m "feat(ts): log the OAuth code of a transient refresh failure (SMA-692)" \
  -m "A transient refresh failure now can carry its OAuth error code, from the closed RFC 6749
section 5.2 list or other. The session.refresh_failed line shows it. The code is read by
its error code, not by instanceof, so it crosses the two Next module copies." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The OIDC adapter takes the scopes, the audience and the refresh scope (D1, D4, D10)

This task changes the adapter and the smallest wiring that keeps the build green: `runtime.ts` passes `scopes` from the parsed env, and `routes.ts` stops passing `scopes`. `AuthRuntime.scopes` stays until Task 3. The refresh request does not change in this task, because the runtime passes no `refreshScope` yet.

**Files:**
- Modify: `ts/packages/paigasus-auth/src/adapters/oidc.ts:35-36` (import), `:61-66` (`BuildAuthorizationUrlParams`), `:92-105` (`CreateOidcClientOptions`), `:160-172` (classifier), `:229-236` (`buildAuthorizationUrl`), `:286` (`refresh`)
- Modify: `ts/packages/paigasus-auth/src/http/routes.ts:179-185`
- Modify: `ts/packages/paigasus-auth/src/runtime.ts:138-144`
- Modify: `ts/packages/paigasus-auth/tests/fixtures/jwks.ts:57-86` (interface), `:113-120` (state), `:148-149` (`/token` handler), `:236-258` (returned object)
- Test: `ts/packages/paigasus-auth/tests/adapters/oidc.test.ts:14-33`, `:132`, `:202-212`, `:307-315`, append at the end
- Test: `ts/packages/paigasus-auth/tests/http/login.test.ts:39-46`, `tests/http/callback.test.ts:87`, `tests/http/route-handler.test.ts:27` (add `scopes`)

**Interfaces:**
- Consumes: `RefreshFailed`, `toTokenErrorCode` (Task 1).
- Produces:
  - `interface BuildAuthorizationUrlParams { redirectUri: string; state: string }`
  - `interface CreateOidcClientOptions { issuer: string; clientId: string; clientSecret: string; httpTimeoutMs: number; clockToleranceSeconds: number; scopes: string; audience?: string; refreshScope?: string; allowInsecureRequests?: boolean }`
  - `OidcClient.refresh(refreshToken: string): Promise<RefreshedTokens>` is unchanged, so `ResolveDeps.refresh` is unchanged (D4).
  - A refresh that fails with a `ResponseBodyError` other than `invalid_grant` rejects with `RefreshFailed`.
  - Test fixture: `OidcFixture.tokenRequests(): readonly URLSearchParams[]`.

- [ ] **Step 1: Add the token-request recorder to the fixture**

In `tests/fixtures/jwks.ts`, in `interface OidcFixture`, insert before `close(): Promise<void>;` (line 85):

```ts
  /**
   * The body of every /token request this fixture received, in order (SMA-692). A test reads the
   * real `scope` and `audience` of a refresh request here. The bodies hold the client secret, so a
   * test must never print one.
   */
  tokenRequests(): readonly URLSearchParams[];
```

After line 119 (`let nextTokenError: …`), insert:

```ts
  const tokenRequestBodies: URLSearchParams[] = [];
```

Replace line 149:

```ts
        const rawBody = await readBody(req);
```

with:

```ts
        const rawBody = await readBody(req);
        tokenRequestBodies.push(new URLSearchParams(rawBody));
```

In the returned object, insert before `close(): Promise<void> {` (line 253):

```ts
    tokenRequests(): readonly URLSearchParams[] {
      return [...tokenRequestBodies];
    },
```

- [ ] **Step 2: Write the failing adapter tests**

In `tests/adapters/oidc.test.ts`, replace lines 14-16:

```ts
import { createOidcClient, type OidcClient } from '../../src/adapters/oidc.js';
import { startOidcFixture, type OidcFixture } from '../fixtures/jwks.js';
import { RefreshRejected } from '../../src/core/errors.js';
```

with:

```ts
import { createOidcClient, type CreateOidcClientOptions, type OidcClient } from '../../src/adapters/oidc.js';
import { startOidcFixture, type OidcFixture } from '../fixtures/jwks.js';
import { RefreshFailed, RefreshRejected } from '../../src/core/errors.js';
```

Replace lines 18-33 (from `const REDIRECT_URI` to the end of `makeClient`) with:

```ts
const REDIRECT_URI = 'https://rp.example.com/auth/callback';
const STATE = 'txn-state-value';
const NONCE = 'expected-nonce-value';
const SCOPES = 'openid profile email offline_access';
const API_AUDIENCE = 'https://api.example.com';

let fixture: OidcFixture;

function makeClient(clockToleranceSeconds = 30, extra: Partial<Pick<CreateOidcClientOptions, 'audience' | 'refreshScope'>> = {}): OidcClient {
  return createOidcClient({
    issuer: fixture.issuer,
    clientId: fixture.clientId,
    clientSecret: fixture.clientSecret,
    httpTimeoutMs: 5000,
    clockToleranceSeconds,
    scopes: SCOPES,
    ...extra,
    allowInsecureRequests: true, // the fixture is plain http on localhost — never set in production
  });
}

/** The one refresh_token request the fixture received. Throws, and so fails the test, otherwise. */
function onlyRefreshRequest(): URLSearchParams {
  const refreshes = fixture.tokenRequests().filter((p) => p.get('grant_type') === 'refresh_token');
  const [only] = refreshes;
  if (refreshes.length !== 1 || only === undefined) throw new Error(`expected exactly one refresh request, got ${String(refreshes.length)}`);
  return only;
}
```

Replace line 132:

```ts
    const req = await oidc.buildAuthorizationUrl({ redirectUri: REDIRECT_URI, scopes: 'openid profile', state: STATE });
```

with:

```ts
    const req = await oidc.buildAuthorizationUrl({ redirectUri: REDIRECT_URI, state: STATE });
```

In the discovery-failure test, replace lines 208-209:

```ts
      clockToleranceSeconds: 30,
      allowInsecureRequests: true,
```

with:

```ts
      clockToleranceSeconds: 30,
      scopes: SCOPES,
      allowInsecureRequests: true,
```

Append at the end of the file:

```ts

// SMA-692. The tests read the REAL request: the authorization URL the adapter builds, and the
// /token body the fixture records. openid-client sends an `undefined` value as the string
// "undefined" (spec F7), so an absent value must leave its key out.
describe('createOidcClient — the scopes, the audience and the refresh scope (SMA-692)', () => {
  it('sends the injected scopes as scope', async () => {
    const req = await makeClient().buildAuthorizationUrl({ redirectUri: REDIRECT_URI, state: STATE });
    expect(new URL(req.url).searchParams.get('scope')).toBe(SCOPES);
  });

  it('sends no audience parameter, and no "undefined" string, when none is injected', async () => {
    const req = await makeClient().buildAuthorizationUrl({ redirectUri: REDIRECT_URI, state: STATE });
    expect(new URL(req.url).searchParams.has('audience')).toBe(false);
    expect(req.url).not.toContain('undefined');
  });

  it('sends the injected audience as the audience parameter', async () => {
    const req = await makeClient(30, { audience: API_AUDIENCE }).buildAuthorizationUrl({ redirectUri: REDIRECT_URI, state: STATE });
    expect(new URL(req.url).searchParams.get('audience')).toBe(API_AUDIENCE);
  });

  // D3-a: with no refreshScope the refresh body keeps exactly the keys it had before SMA-692.
  it('sends the refresh request of today when no refreshScope is injected, even with an audience', async () => {
    await makeClient(30, { audience: API_AUDIENCE }).refresh('some-refresh-token');
    expect([...onlyRefreshRequest().keys()].sort()).toEqual(['client_id', 'client_secret', 'grant_type', 'refresh_token']);
  });

  it('sends the injected refreshScope as scope on a refresh, and never audience (spec F1)', async () => {
    await makeClient(30, { audience: API_AUDIENCE, refreshScope: 'openid api://paigasus-api/access' }).refresh('some-refresh-token');
    const body = onlyRefreshRequest();
    expect(body.get('scope')).toBe('openid api://paigasus-api/access');
    expect(body.has('audience')).toBe(false);
  });
});

// SMA-692 D10. Every OAuth code except invalid_grant stays transient (D11), and the error now
// carries the code. The codes outside RFC 6749 § 5.2 map to 'other'.
describe('createOidcClient — the OAuth code of a transient refresh failure (SMA-692 D10)', () => {
  it.each(['invalid_client', 'unauthorized_client', 'invalid_scope', 'invalid_request', 'unsupported_grant_type'])('carries %s on the transient error', async (code) => {
    fixture.setNextTokenError(code);
    const err: unknown = await makeClient().refresh('some-refresh-token').catch((e: unknown) => e);
    expect(err).toBeInstanceOf(RefreshFailed);
    expect((err as RefreshFailed).oauthError).toBe(code);
    expect((err as Error).message).toBe('oidc refresh_token_grant failed: ResponseBodyError');
  });

  it.each(['server_error', 'temporarily_unavailable'])('carries other for %s, a code outside RFC 6749 § 5.2', async (code) => {
    fixture.setNextTokenError(code);
    const err: unknown = await makeClient().refresh('some-refresh-token').catch((e: unknown) => e);
    expect((err as RefreshFailed).oauthError).toBe('other');
  });

  it('carries no code when the response has WWW-Authenticate (no .error field to read)', async () => {
    fixture.setNextTokenError('invalid_client', 'Basic realm="idp"');
    const err: unknown = await makeClient().refresh('some-refresh-token').catch((e: unknown) => e);
    expect(err).not.toBeInstanceOf(RefreshFailed);
    expect((err as Error).message).toMatch(/oidc refresh_token_grant failed/);
  });
});
```

In `tests/http/login.test.ts`, replace lines 44-45:

```ts
      clockToleranceSeconds: 30,
      allowInsecureRequests: true, // the fixture is plain http on localhost — never set in production
```

with:

```ts
      clockToleranceSeconds: 30,
      scopes: 'openid profile email',
      allowInsecureRequests: true, // the fixture is plain http on localhost — never set in production
```

In `tests/http/callback.test.ts` (the `createOidcClient({` call at line 87) and `tests/http/route-handler.test.ts` (the call at line 27), add the line `scopes: 'openid profile email',` directly after the `clockToleranceSeconds: …,` line of that call. Read each call first; keep its indentation.

- [ ] **Step 3: Run the tests, expect FAIL**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-auth exec vitest run tests/adapters/oidc.test.ts
```

Expected: FAIL. `sends the injected scopes as scope` gets the string `"undefined"`: the adapter still reads `params.scopes`, which the test no longer passes, and openid-client sends `undefined` as that string (spec F7). For the same reason `sends no audience parameter, and no "undefined" string, when none is injected` fails on its `undefined` check. `sends the injected audience as the audience parameter` gets `null`. `sends the injected refreshScope…` gets `null` for `scope`. The D10 rows fail with `expected Error to be an instance of RefreshFailed`. The four-key row and the WWW-Authenticate row pass already: they pin today's behaviour.

- [ ] **Step 4: Implement the adapter**

In `src/adapters/oidc.ts`, replace line 35:

```ts
import { RefreshRejected } from '../core/errors';
```

with:

```ts
import { RefreshFailed, RefreshRejected, toTokenErrorCode } from '../core/errors';
```

Replace lines 61-66:

```ts
export interface BuildAuthorizationUrlParams {
  redirectUri: string;
  scopes: string;
  /** The caller's CSRF state value — task 8 binds this to the stored login transaction id. */
  state: string;
}
```

with:

```ts
/** The scopes and the audience are not here: createOidcClient receives them once (SMA-692 D4). */
export interface BuildAuthorizationUrlParams {
  redirectUri: string;
  /** The caller's CSRF state value — task 8 binds this to the stored login transaction id. */
  state: string;
}
```

In `CreateOidcClientOptions`, replace line 97:

```ts
  clockToleranceSeconds: number;
```

with:

```ts
  clockToleranceSeconds: number;
  /**
   * The `scope` of every authorization request (SMA-692). Always set: createAuthRuntime passes
   * PAIGASUS_OIDC_SCOPES, or the default list when that variable is absent.
   */
  scopes: string;
  /**
   * The `audience` parameter of the authorization request (SMA-692 D1). Only Auth0 needs it. When it
   * is absent, the request has no `audience` key: openid-client sends an `undefined` value as the
   * string "undefined" (spec F7). The refresh request never sends it: Auth0 keeps the original
   * audience on a refresh (spec F1).
   */
  audience?: string;
  /**
   * The `scope` of every refresh request (SMA-692 D3-a). Set only when the operator set
   * PAIGASUS_OIDC_SCOPES. When it is absent, the refresh request has no `scope`, as before SMA-692.
   */
  refreshScope?: string;
```

Replace lines 168-171:

```ts
  if (cause instanceof client.ResponseBodyError && cause.error === 'invalid_grant') {
    return new RefreshRejected('invalid_grant');
  }
  return wrapError('refresh_token_grant', cause);
```

with:

```ts
  if (cause instanceof client.ResponseBodyError) {
    if (cause.error === 'invalid_grant') return new RefreshRejected('invalid_grant');
    // SMA-692 D10. Every other OAuth code stays transient (D11), but the error now carries the
    // code, so the refresh log line can tell an `invalid_scope` from a network error. The IdP
    // writes `.error`, so toTokenErrorCode maps a value outside RFC 6749 § 5.2 to 'other'.
    return new RefreshFailed(toTokenErrorCode(cause.error), cause.name);
  }
  return wrapError('refresh_token_grant', cause);
```

Replace lines 229-236:

```ts
        const url = client.buildAuthorizationUrl(config, {
          redirect_uri: params.redirectUri,
          scope: params.scopes,
          code_challenge: codeChallenge,
          code_challenge_method: 'S256',
          state: params.state,
          nonce,
        });
```

with:

```ts
        const url = client.buildAuthorizationUrl(config, {
          redirect_uri: params.redirectUri,
          scope: opts.scopes,
          // SMA-692 D1, spec F7: leave the key out when it is absent. Never pass `undefined`.
          ...(opts.audience !== undefined ? { audience: opts.audience } : {}),
          code_challenge: codeChallenge,
          code_challenge_method: 'S256',
          state: params.state,
          nonce,
        });
```

Replace line 286:

```ts
        const tokens = await client.refreshTokenGrant(config, refreshToken);
```

with:

```ts
        // SMA-692 D3-a. With no refreshScope the call is the call of before SMA-692, with no third
        // argument. `audience` is never sent here (spec F1).
        const tokens = opts.refreshScope !== undefined ? await client.refreshTokenGrant(config, refreshToken, { scope: opts.refreshScope }) : await client.refreshTokenGrant(config, refreshToken);
```

- [ ] **Step 5: Implement the wiring**

In `src/http/routes.ts`, replace lines 179-185:

```ts
  const authorization = await runtime.oidc.buildAuthorizationUrl({
    redirectUri: runtime.redirectUri,
    scopes: runtime.scopes,
    // The CSRF `state` IS the transaction id, so the callback can find its cookie and its stored
    // transaction from the same value the IdP hands back.
    state: txnId,
  });
```

with:

```ts
  // The scopes and the audience are not passed here: runtime.oidc holds them (SMA-692 D4).
  const authorization = await runtime.oidc.buildAuthorizationUrl({
    redirectUri: runtime.redirectUri,
    // The CSRF `state` IS the transaction id, so the callback can find its cookie and its stored
    // transaction from the same value the IdP hands back.
    state: txnId,
  });
```

In `src/runtime.ts`, replace lines 138-144:

```ts
  const oidc = createOidcClient({
    issuer: cfg.PAIGASUS_OIDC_ISSUER,
    clientId: cfg.PAIGASUS_OIDC_CLIENT_ID,
    clientSecret: cfg.PAIGASUS_OIDC_CLIENT_SECRET,
    httpTimeoutMs: cfg.PAIGASUS_OIDC_HTTP_TIMEOUT_MS,
    clockToleranceSeconds: cfg.PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS,
  });
```

with:

```ts
  const oidc = createOidcClient({
    issuer: cfg.PAIGASUS_OIDC_ISSUER,
    clientId: cfg.PAIGASUS_OIDC_CLIENT_ID,
    clientSecret: cfg.PAIGASUS_OIDC_CLIENT_SECRET,
    httpTimeoutMs: cfg.PAIGASUS_OIDC_HTTP_TIMEOUT_MS,
    clockToleranceSeconds: cfg.PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS,
    scopes: cfg.PAIGASUS_OIDC_SCOPES,
  });
```

(Task 3 replaces this call. At this point `PAIGASUS_OIDC_SCOPES` still has its zod default, so it is a `string`.)

- [ ] **Step 6: Run the tests, expect PASS**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-auth exec vitest run tests/adapters/oidc.test.ts tests/http
```

Expected: PASS, every row, the old `treats %s as transient, NOT as RefreshRejected` rows included.

- [ ] **Step 7: Mutation 2a, drop the audience spread**

With the Edit tool, in `src/adapters/oidc.ts` delete the line `          ...(opts.audience !== undefined ? { audience: opts.audience } : {}),`. Run the Step 6 command. Expected: FAIL in `sends the injected audience as the audience parameter`. Restore with the inverse Edit (insert the line again after `scope: opts.scopes,` and its comment). Run again: PASS.

- [ ] **Step 8: Mutation 2b, pass audience as undefined**

With the Edit tool, replace `...(opts.audience !== undefined ? { audience: opts.audience } : {}),` with `audience: opts.audience as string,`. Run the Step 6 command. Expected: FAIL in `sends no audience parameter, and no "undefined" string, when none is injected`. Restore with the inverse Edit. Run again: PASS.

- [ ] **Step 9: Mutation 2c, always pass the refresh scope**

With the Edit tool, replace `const tokens = opts.refreshScope !== undefined ? await client.refreshTokenGrant(config, refreshToken, { scope: opts.refreshScope }) : await client.refreshTokenGrant(config, refreshToken);` with `const tokens = await client.refreshTokenGrant(config, refreshToken, { scope: opts.refreshScope as string });`. Run the Step 6 command. Expected: FAIL in `sends the refresh request of today when no refreshScope is injected, even with an audience` (the body gets a `scope` key). Restore with the inverse Edit. Run again: PASS.

- [ ] **Step 10: Mutation 2d, never pass the refresh scope**

With the Edit tool, replace the same line with `const tokens = await client.refreshTokenGrant(config, refreshToken);`. Run the Step 6 command. Expected: FAIL in `sends the injected refreshScope as scope on a refresh, and never audience (spec F1)`. Restore with the inverse Edit. Run again: PASS.

- [ ] **Step 11: Mutation 2e, the classifier drops the code**

With the Edit tool, replace `return new RefreshFailed(toTokenErrorCode(cause.error), cause.name);` with `return wrapError('refresh_token_grant', cause);`. Run the Step 6 command. Expected: FAIL in every `carries %s on the transient error` row and every `carries other for %s` row. Restore with the inverse Edit. Run again: PASS.

- [ ] **Step 12: Typecheck, lint, format and the whole package**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --write packages/paigasus-auth/src/adapters/oidc.ts packages/paigasus-auth/src/http/routes.ts packages/paigasus-auth/src/runtime.ts packages/paigasus-auth/tests/fixtures/jwks.ts packages/paigasus-auth/tests/adapters/oidc.test.ts packages/paigasus-auth/tests/http/login.test.ts packages/paigasus-auth/tests/http/callback.test.ts packages/paigasus-auth/tests/http/route-handler.test.ts
moon run paigasus-auth-ts:typecheck paigasus-auth-ts:test ts:lint ts:fmt
```

Expected: all four targets pass.

- [ ] **Step 13: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
git add ts/packages/paigasus-auth/src/adapters/oidc.ts ts/packages/paigasus-auth/src/http/routes.ts ts/packages/paigasus-auth/src/runtime.ts ts/packages/paigasus-auth/tests/fixtures/jwks.ts ts/packages/paigasus-auth/tests/adapters/oidc.test.ts ts/packages/paigasus-auth/tests/http/login.test.ts ts/packages/paigasus-auth/tests/http/callback.test.ts ts/packages/paigasus-auth/tests/http/route-handler.test.ts
git commit -m "feat(ts): inject the scopes, the audience and the refresh scope into the oidc client (SMA-692)" \
  -m "createOidcClient receives the scopes, an optional audience and an optional refresh scope
once. The authorization request sends audience only when it is set. The refresh request sends
scope only when a refresh scope is set, and never sends audience. A transient refresh failure
now rejects with RefreshFailed, which carries the OAuth code." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: The env shape, the runtime factory and D3-a (D1, D3-a, D4, D5)

**Files:**
- Modify: `ts/packages/paigasus-auth/src/config.ts:17-46`
- Modify: `ts/packages/paigasus-auth/src/runtime.ts:35`, `:56-60`, `:89-90`, `:102` (above), `:138-145` (as left by Task 2), `:178`
- Modify: `ts/packages/paigasus-auth/src/core/errors.ts:120-122` (a stale sentence)
- Modify: `ts/packages/paigasus-auth/README.md:37` (the table), then Prettier
- Modify: `ts/packages/paigasus-auth/tests/e2e/fixture-server.ts:124`
- Test: `ts/packages/paigasus-auth/tests/config.test.ts:22-24`, append rows
- Test: `ts/packages/paigasus-auth/tests/runtime.test.ts:3`, `:82-87`
- Test (drop `scopes` from the `AuthRuntime` literal): `tests/server.test.ts:46`, `tests/support/store-failure.ts:128`, `tests/next/get-session.test.ts:84`, `tests/http/callback.test.ts:108`, `tests/http/logout.test.ts:151`, `tests/http/login.test.ts:59`, `tests/http/route-handler.test.ts:61`, `tests/http/login-returnto-table.test.ts:43`

**Interfaces:**
- Consumes: `createOidcClient`, `CreateOidcClientOptions`, `OidcClient` (Task 2).
- Produces:
  - `authEnvShape.PAIGASUS_OIDC_SCOPES`: optional string, no default, must hold the whitespace-separated token `openid`.
  - `authEnvShape.PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE`: optional string, non-empty, no leading or trailing whitespace.
  - `AuthEnv` gains `PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE?: string | undefined`; `PAIGASUS_OIDC_SCOPES` becomes `?: string | undefined`.
  - `interface CreateAuthRuntimeDeps { store?: SessionStore; resolver?: PrincipalResolver; logger?: AuthLogger; oidcClientFactory?: (opts: CreateOidcClientOptions) => OidcClient }`
  - `AuthRuntime` loses `scopes`.
  - The factory receives `scopes` (the env value, or `'openid profile email offline_access'`), `refreshScope` only when the env is set, and `audience` only when the env is set.

- [ ] **Step 1: Write the failing config tests**

In `tests/config.test.ts`, delete line 24:

```ts
    expect(parsed.PAIGASUS_OIDC_SCOPES).toBe('openid profile email offline_access');
```

Append before the final `});` of the `describe('authEnvShape', …)` block (line 80):

```ts

  // SMA-692 D3-a. No default here: createAuthRuntime applies the default list for the
  // authorization request only, so "absent" must stay visible after the parse.
  it('gives PAIGASUS_OIDC_SCOPES no default', () => {
    expect(schema.parse(VALID).PAIGASUS_OIDC_SCOPES).toBeUndefined();
  });

  // SMA-692 D5. A token-exact match on whitespace-separated tokens.
  it.each(['openid', 'openid profile email offline_access', 'profile email openid', 'profile\topenid', 'openid api://paigasus-api/access'])('accepts PAIGASUS_OIDC_SCOPES %j, which holds the token openid', (value) => {
    expect(schema.parse({ ...VALID, PAIGASUS_OIDC_SCOPES: value }).PAIGASUS_OIDC_SCOPES).toBe(value);
  });

  it.each(['profile email', 'openidx profile', 'profile openid-connect', 'OPENID profile', '', '   '])('refuses PAIGASUS_OIDC_SCOPES %j, which does not hold the token openid', (value) => {
    expect(() => schema.parse({ ...VALID, PAIGASUS_OIDC_SCOPES: value })).toThrow();
  });

  // SMA-692 D1.
  it('leaves PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE undefined when absent', () => {
    expect(schema.parse(VALID).PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE).toBeUndefined();
  });

  it('accepts a set PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE', () => {
    expect(schema.parse({ ...VALID, PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE: 'https://api.example.com' }).PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE).toBe('https://api.example.com');
  });

  it.each(['', ' https://api.example.com', 'https://api.example.com ', '\thttps://api.example.com'])('refuses PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE %j', (value) => {
    expect(() => schema.parse({ ...VALID, PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE: value })).toThrow();
  });
```

- [ ] **Step 2: Write the failing runtime tests**

In `tests/runtime.test.ts`, replace line 3:

```ts
import { createAuthRuntime, getAuthRuntime } from '../src/runtime.js';
```

with:

```ts
import { createAuthRuntime, getAuthRuntime } from '../src/runtime.js';
import { createOidcClient, type CreateOidcClientOptions, type OidcClient } from '../src/adapters/oidc.js';
```

Replace lines 82-87 (the `exposes the configured scopes` row and its comment) with:

```ts
  // SMA-692 D4. The factory dependency is the only way a test sees the options the runtime derives
  // from the env: AuthRuntime has no scopes field any more.
  function recordingFactory(): { factory: (opts: CreateOidcClientOptions) => OidcClient; seen: CreateOidcClientOptions[]; built: OidcClient[] } {
    const seen: CreateOidcClientOptions[] = [];
    const built: OidcClient[] = [];
    const factory = (opts: CreateOidcClientOptions): OidcClient => {
      seen.push(opts);
      const client = createOidcClient(opts);
      built.push(client);
      return client;
    };
    return { factory, seen, built };
  }

  it('passes the set env values to the OIDC client factory, and keeps the client it returns', async () => {
    const { factory, seen, built } = recordingFactory();
    const rt = await createAuthRuntime(
      { ...BASE, PAIGASUS_OIDC_SCOPES: 'openid profile email offline_access api://paigasus-api/access', PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE: 'https://api.example.com' },
      { oidcClientFactory: factory },
    );
    expect(seen).toHaveLength(1);
    expect(seen[0]).toMatchObject({
      issuer: 'https://idp.example.com',
      clientId: 'c',
      scopes: 'openid profile email offline_access api://paigasus-api/access',
      refreshScope: 'openid profile email offline_access api://paigasus-api/access',
      audience: 'https://api.example.com',
    });
    expect(rt.oidc).toBe(built[0]);
  });

  // D3-a: an absent PAIGASUS_OIDC_SCOPES gives the default list to the authorization request, and
  // NO refresh scope, so the refresh request stays the request of before SMA-692.
  it('uses the default scopes for the authorization request only when the env is absent', async () => {
    const { factory, seen } = recordingFactory();
    // eslint-disable-next-line @typescript-eslint/no-unused-vars -- rest-sibling destructuring
    const { PAIGASUS_OIDC_SCOPES: _omitted, ...withoutScopes } = BASE;
    await createAuthRuntime(withoutScopes, { oidcClientFactory: factory });
    expect(seen).toHaveLength(1);
    expect(seen[0]?.scopes).toBe('openid profile email offline_access');
    expect(seen[0]).not.toHaveProperty('refreshScope');
    expect(seen[0]).not.toHaveProperty('audience');
  });
```

- [ ] **Step 3: Run the tests, expect FAIL**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-auth exec vitest run tests/config.test.ts tests/runtime.test.ts
```

Expected: FAIL. `gives PAIGASUS_OIDC_SCOPES no default` gets the default string. The refuse rows for `profile email`, `openidx profile` and the others do not throw. The audience rows fail (the key is stripped: `undefined` for the accept row, no throw for the refuse rows). Both runtime rows fail with `expected [] to have a length of 1` (the runtime ignores the factory).

- [ ] **Step 4: Implement the env shape**

In `src/config.ts`, replace lines 37-38:

```ts
const overrideUrl = z.url();

```

with:

```ts
const overrideUrl = z.url();

/**
 * True when the whitespace-separated scope list holds the token `openid` exactly (SMA-692 D5).
 * Without it the first login fails at once (`idTokenExpected: true` in adapters/oidc.ts), with an
 * unclear error. `openidx` does not count. The chart refuses the same value at render time, with
 * a readable reason (charts/paigasus/templates/_audience.tpl).
 */
const hasOpenidScope = (value: string): boolean => value.split(/\s+/).includes('openid');

```

Replace line 46:

```ts
  PAIGASUS_OIDC_SCOPES: z.string().min(1).default('openid profile email offline_access'),
```

with:

```ts
  // SMA-692 D3-a: NO default here. createAuthRuntime applies the default list to the
  // authorization request only. The refresh request sends `scope` only when this key is set, so
  // the parsed config must keep "absent" distinct from "the default".
  PAIGASUS_OIDC_SCOPES: z.string().refine(hasOpenidScope, { error: 'must contain the scope openid' }).optional(),
  // SMA-692 D1. The `audience` authorization parameter (Auth0). Not the IAM audience.
  PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE: z
    .string()
    .refine((v) => v.length > 0 && v === v.trim(), { error: 'must be non-empty, with no leading or trailing whitespace' })
    .optional(),
```

- [ ] **Step 5: Implement the runtime**

In `src/runtime.ts`, replace line 35:

```ts
import { createOidcClient, type OidcClient } from './adapters/oidc';
```

with:

```ts
import { createOidcClient, type CreateOidcClientOptions, type OidcClient } from './adapters/oidc';
```

Replace lines 56-60:

```ts
export interface CreateAuthRuntimeDeps {
  store?: SessionStore;
  resolver?: PrincipalResolver;
  logger?: AuthLogger;
}
```

with:

```ts
export interface CreateAuthRuntimeDeps {
  store?: SessionStore;
  resolver?: PrincipalResolver;
  logger?: AuthLogger;
  /**
   * Builds the OIDC client (SMA-692 D4). The default is createOidcClient. A test injects a
   * recording factory to see the options that the runtime derives from the env.
   */
  oidcClientFactory?: (opts: CreateOidcClientOptions) => OidcClient;
}
```

Delete lines 89-90:

```ts
  /** Parsed but otherwise unreachable before this — task 8 needs it to build the authorization URL. */
  scopes: string;
```

Insert directly above `export async function createAuthRuntime(` (line 102):

```ts
/**
 * The scopes of the authorization request when PAIGASUS_OIDC_SCOPES is absent (SMA-692 D3-a).
 * The default is here, not in authEnvShape: the refresh request sends `scope` only when the
 * operator set the variable, so the parsed config keeps "absent" distinct from "the default".
 */
const DEFAULT_OIDC_SCOPES = 'openid profile email offline_access';

```

Replace the `createOidcClient` call as Task 2 left it:

```ts
  const oidc = createOidcClient({
    issuer: cfg.PAIGASUS_OIDC_ISSUER,
    clientId: cfg.PAIGASUS_OIDC_CLIENT_ID,
    clientSecret: cfg.PAIGASUS_OIDC_CLIENT_SECRET,
    httpTimeoutMs: cfg.PAIGASUS_OIDC_HTTP_TIMEOUT_MS,
    clockToleranceSeconds: cfg.PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS,
    scopes: cfg.PAIGASUS_OIDC_SCOPES,
  });
```

with:

```ts
  // SMA-692 D3-a, D4. `refreshScope` and `audience` are set only when their variables are set.
  // A key with an `undefined` value would reach openid-client as the string "undefined" (spec F7),
  // and exactOptionalPropertyTypes refuses it too.
  const scopes = cfg.PAIGASUS_OIDC_SCOPES;
  const audience = cfg.PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE;
  const oidc = (deps.oidcClientFactory ?? createOidcClient)({
    issuer: cfg.PAIGASUS_OIDC_ISSUER,
    clientId: cfg.PAIGASUS_OIDC_CLIENT_ID,
    clientSecret: cfg.PAIGASUS_OIDC_CLIENT_SECRET,
    httpTimeoutMs: cfg.PAIGASUS_OIDC_HTTP_TIMEOUT_MS,
    clockToleranceSeconds: cfg.PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS,
    scopes: scopes ?? DEFAULT_OIDC_SCOPES,
    ...(scopes !== undefined ? { refreshScope: scopes } : {}),
    ...(audience !== undefined ? { audience } : {}),
  });
```

Then delete the line `    scopes: cfg.PAIGASUS_OIDC_SCOPES,` from the returned object (line 178 before these edits, directly after `basePath,`). Do this AFTER the call replacement above: before it, the call holds a line with the same text and the same indent.

- [ ] **Step 6: Remove `scopes` from the eight `AuthRuntime` literals and the e2e fixture**

With the Edit tool, delete the one `scopes: '…',` line of the `AuthRuntime` literal in each file. Each line is the only `    scopes:` line in the literal; read the file first:

- `tests/server.test.ts:46` — `    scopes: 'openid',`
- `tests/support/store-failure.ts:128` — `    scopes: 'openid profile email',`
- `tests/next/get-session.test.ts:84` — `    scopes: 'openid profile email offline_access',`
- `tests/http/callback.test.ts:108` — `    scopes: 'openid profile email',` (the `AuthRuntime` literal, NOT the `createOidcClient` option that Task 2 added)
- `tests/http/logout.test.ts:151` — `    scopes: 'openid profile email',`
- `tests/http/login.test.ts:59` — `    scopes: 'openid profile email',` (the literal's line, which follows `basePath: ZONE_BASE_PATH,`; keep the `createOidcClient` option that Task 2 added)
- `tests/http/route-handler.test.ts:61` — `    scopes: 'openid profile email',` (the literal's line, which follows `basePath: '/iam',`)
- `tests/http/login-returnto-table.test.ts:43` — `    scopes: 'openid profile',`

In `tests/e2e/fixture-server.ts`, delete line 124:

```ts
    PAIGASUS_OIDC_SCOPES: 'openid profile email offline_access',
```

With D3-a an explicit value opts the fixture into a refresh `scope`. Without it the e2e tier runs the default path of a chart install that does not set `oidc.scopes`.

- [ ] **Step 7: Fix the stale sentence in `core/errors.ts`**

In `src/core/errors.ts`, replace lines 120-122:

```ts
 * NOT exported from src/server.ts, deliberately: no consumer can produce or observe one.
 * getSession() swallows every failure into `null`, and CreateAuthRuntimeDeps exposes no OIDC
 * override.
```

with:

```ts
 * NOT exported from src/server.ts, deliberately: no consumer can observe one. getSession()
 * swallows every failure into `null`. CreateAuthRuntimeDeps.oidcClientFactory (SMA-692) lets a
 * caller inject an OIDC client, but that client's errors reach the same swallowing path.
```

- [ ] **Step 8: Update the README env table**

In `ts/packages/paigasus-auth/README.md`, replace the `PAIGASUS_OIDC_SCOPES` row (line 37) with these two rows (Prettier re-pads the table in Step 11):

```markdown
| `PAIGASUS_OIDC_SCOPES` | no | `openid profile email offline_access`, for the authorization request only | A space-separated scope string. It must contain the scope `openid`; the parse fails without it, and `openidx` does not count. Keep `offline_access` unless the IdP issues a refresh token without it — removing it can make every access-token expiry log the user out (see "Why `offline_access` is a default scope" below). When the variable is set, each refresh request also sends it as `scope`. When it is absent, a refresh request sends no `scope` (SMA-692). Entra ID needs it: add one scope of the API, for example `api://paigasus-api/access`. |
| `PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE` | no | — (no `audience` parameter) | A non-empty string with no leading or trailing whitespace. The authorization request sends it as the `audience` parameter; Auth0 needs it to issue an access token for an API. A refresh request never sends it. Set it to the audience that IAM accepts (`oidc.audience` in the chart) (SMA-692). |
```

- [ ] **Step 9: Run the tests, expect PASS**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-auth exec vitest run tests/config.test.ts tests/runtime.test.ts
```

Expected: PASS, every row.

- [ ] **Step 10: The mutations**

Run each mutation with the Edit tool, run the Step 9 command, see the named FAIL, restore with the inverse Edit, and run again to see PASS:

- 3a. In `src/config.ts`, replace `.refine(hasOpenidScope, { error: 'must contain the scope openid' }).optional(),` with `.refine(hasOpenidScope, { error: 'must contain the scope openid' }).default('openid profile email offline_access'),`. Expected FAIL: `gives PAIGASUS_OIDC_SCOPES no default`.
- 3b. In `src/runtime.ts`, replace `...(scopes !== undefined ? { refreshScope: scopes } : {}),` with `refreshScope: scopes ?? DEFAULT_OIDC_SCOPES,`. Expected FAIL: `uses the default scopes for the authorization request only when the env is absent`.
- 3c. In `src/runtime.ts`, delete the line `    ...(scopes !== undefined ? { refreshScope: scopes } : {}),`. Expected FAIL: `passes the set env values to the OIDC client factory, and keeps the client it returns`.
- 3d. In `src/runtime.ts`, delete the line `    ...(audience !== undefined ? { audience } : {}),`. Expected FAIL: the same row as 3c.
- 3e. In `src/runtime.ts`, replace `(deps.oidcClientFactory ?? createOidcClient)(` with `createOidcClient(`. Expected FAIL: both factory rows.
- 3f. In `src/config.ts`, replace `.refine(hasOpenidScope, { error: 'must contain the scope openid' })` with `.min(1)`. Expected FAIL: the refuse rows for `profile email`, `openidx profile`, `profile openid-connect`, `OPENID profile` and `   `.
- 3g. In `src/config.ts`, replace `const hasOpenidScope = (value: string): boolean => value.split(/\s+/).includes('openid');` with `const hasOpenidScope = (value: string): boolean => value.includes('openid');`. Expected FAIL: the refuse rows for `openidx profile` and `profile openid-connect`.
- 3h. In `src/config.ts`, replace `.refine((v) => v.length > 0 && v === v.trim(), { error: 'must be non-empty, with no leading or trailing whitespace' })` with `.refine((v) => v.length > 0, { error: 'must be non-empty, with no leading or trailing whitespace' })`. Expected FAIL: the three whitespace refuse rows of `PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE`.

- [ ] **Step 11: Format, typecheck, lint and the whole package**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts exec prettier --write packages/paigasus-auth/src/config.ts packages/paigasus-auth/src/runtime.ts packages/paigasus-auth/src/core/errors.ts packages/paigasus-auth/README.md packages/paigasus-auth/tests/config.test.ts packages/paigasus-auth/tests/runtime.test.ts packages/paigasus-auth/tests/server.test.ts packages/paigasus-auth/tests/support/store-failure.ts packages/paigasus-auth/tests/next/get-session.test.ts packages/paigasus-auth/tests/http/callback.test.ts packages/paigasus-auth/tests/http/logout.test.ts packages/paigasus-auth/tests/http/login.test.ts packages/paigasus-auth/tests/http/route-handler.test.ts packages/paigasus-auth/tests/http/login-returnto-table.test.ts packages/paigasus-auth/tests/e2e/fixture-server.ts
moon run paigasus-auth-ts:typecheck paigasus-auth-ts:test iam-console-ts:typecheck gateway-console-ts:typecheck ts:lint ts:fmt
```

Expected: all six targets pass. `grep -rn "runtime.scopes\|rt.scopes" ts/packages/paigasus-auth` prints nothing.

- [ ] **Step 12: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
git add ts/packages/paigasus-auth/src/config.ts ts/packages/paigasus-auth/src/runtime.ts ts/packages/paigasus-auth/src/core/errors.ts ts/packages/paigasus-auth/README.md ts/packages/paigasus-auth/tests/config.test.ts ts/packages/paigasus-auth/tests/runtime.test.ts ts/packages/paigasus-auth/tests/server.test.ts ts/packages/paigasus-auth/tests/support/store-failure.ts ts/packages/paigasus-auth/tests/next/get-session.test.ts ts/packages/paigasus-auth/tests/http/callback.test.ts ts/packages/paigasus-auth/tests/http/logout.test.ts ts/packages/paigasus-auth/tests/http/login.test.ts ts/packages/paigasus-auth/tests/http/route-handler.test.ts ts/packages/paigasus-auth/tests/http/login-returnto-table.test.ts ts/packages/paigasus-auth/tests/e2e/fixture-server.ts
git commit -m "feat(ts): request a dedicated API audience from the console env (SMA-692)" \
  -m "PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE is a new optional env for the audience parameter.
PAIGASUS_OIDC_SCOPES must contain openid and has no default in the shape any more.
createAuthRuntime applies the default list to the authorization request only, so a refresh
sends scope only when the operator set the variable. An OIDC client factory in
CreateAuthRuntimeDeps replaces AuthRuntime.scopes as the check of the wiring." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The chart values, the two refusing helpers and their rows (D6-D9)

**Files:**
- Modify: `charts/paigasus/values.yaml:106` (insert after)
- Modify: `charts/paigasus/templates/_audience.tpl:58` (append after the last `{{- end -}}`)
- Modify: `charts/paigasus/templates/console-env-configmap.yaml:15` (append after)
- Modify: `ci/helm-render/fixtures/leaked-value/templates/console-env-configmap.yaml` (whole file)
- Modify: `charts/CLAUDE.md:22-25`
- Modify: `charts/paigasus/README.md:150` (a new section before `## The default image tags`)
- Test: `charts/paigasus/tests/env.sh:70` (header), `:431-433` (rows before the final check)
- Test: `charts/paigasus/tests/refusals.sh:107-108` (rows before `expect_render "iam only"`)

**Interfaces:**
- Consumes: `.Values.oidc.scopes`, `.Values.oidc.authorizationAudience`, `.Values.oidc.audience` (all read with `dig`).
- Produces:
  - `paigasus.consoleScopes` (root context): the value of `oidc.scopes` as a string, or `""`. Fails when set and the tokens do not include `openid`. Message contains `oidc.scopes must contain the scope openid`.
  - `paigasus.consoleAuthorizationAudience` (root context): the value as a string, or `""`. Fails when set and `oidc.audience` is empty (message contains `while oidc.audience is empty`), or when set and not equal to `oidc.audience` (message contains `does not equal oidc.audience`).
  - `console-env` ConfigMap keys `PAIGASUS_OIDC_SCOPES` and `PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE`, each only when non-empty, each with `| quote`.

- [ ] **Step 1: Write the failing refusal rows**

In `charts/paigasus/tests/refusals.sh`, insert after line 107 (the `--set oidc.caBundle.existingConfigMap=… --set oidc.caBundle.key=""` line) and before the blank line that precedes `expect_render "iam only"`:

```bash
# SMA-692 D7. The console must not ask for a token that IAM refuses. The chart fails when
# oidc.authorizationAudience is set and oidc.audience is empty (IAM then accepts only the client
# id, and Auth0 refuses a client id as an API audience), or when the two values differ.
expect_fail "authorizationAudience with oidc.audience empty" "while oidc.audience is empty" \
  --set oidc.authorizationAudience=paigasus-console
expect_fail "authorizationAudience differs from oidc.audience" "does not equal oidc.audience" \
  --set oidc.audience=api://paigasus --set oidc.authorizationAudience=api://other
# SMA-692 D9. The scope list must hold the token openid. openidx does not count.
expect_fail "scopes without openid" "oidc.scopes must contain the scope openid" \
  --set "oidc.scopes=profile email offline_access"
expect_fail "scopes with openidx" "oidc.scopes must contain the scope openid" \
  --set "oidc.scopes=openidx profile email"
expect_render "authorizationAudience equal to oidc.audience" \
  --set oidc.audience=api://paigasus --set oidc.authorizationAudience=api://paigasus
expect_render "scopes with openid not first" \
  --set "oidc.scopes=profile email openid offline_access"
```

- [ ] **Step 2: Write the failing env rows**

In `charts/paigasus/tests/env.sh`, replace line 70:

```bash
# A third row counter reds the script when an N row call line is deleted.
```

with:

```bash
# A third row counter reds the script when an N row call line is deleted.
#
# The script also checks the SMA-692 console OIDC values in the console-env ConfigMap
# (check_console_oidc, check_console_oidc_restart):
#   O1 unset        PAIGASUS_OIDC_SCOPES and PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE are absent,
#                   and so are their comment lines.
#   O2 both-set     Both keys hold the exact values.
#   O3 reuse-values-no-key
#                   `--set oidc.scopes=null --set oidc.authorizationAudience=null`. The render
#                   succeeds, both keys are absent, and the D7 check does not fail.
#   O4 number       oidc.audience and oidc.authorizationAudience are both the number 123. The
#                   render succeeds, and the key holds the string "123".
#   O5 restart-scopes
#                   A change of oidc.scopes changes both console pod templates, not the IAM one.
#   O6 restart-audience
#                   A change of oidc.authorizationAudience does the same.
# A fourth row counter reds the script when an O row call line is deleted.
```

Replace lines 431-433:

```bash
fi

if [ "$ec" -eq 0 ]; then echo "== chart env OK =="; fi
```

(the end of the `NOTES_ROWS` check and the final line) with:

```bash
fi

# The console OIDC values (SMA-692). Renders go to a file, as for check_audience.
OIDC_ROWS=0
OIDC_ROWS_WANT=6
API_SCOPES="openid profile email offline_access api://paigasus/access"

# check_console_oidc <label> <scopes|-> <authorization audience|-> [helm args...]
# "-" means the key, and its comment line, must be absent from the whole render.
check_console_oidc() {
  local label="$1" scopes="$2" aud="$3"; shift 3
  local out
  OIDC_ROWS=$((OIDC_ROWS + 1))
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" "$@" >"$TMP/oidc.yaml" 2>"$TMP/oidc.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/oidc.err"; ec=1; return 0
  fi
  if ! out="$(SCOPES="$scopes" AUD="$aud" python3 -c '
import os, sys, yaml
want = {"PAIGASUS_OIDC_SCOPES": os.environ["SCOPES"], "PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE": os.environ["AUD"]}
marker = {"PAIGASUS_OIDC_SCOPES": "oidc.scopes is set", "PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE": "oidc.authorizationAudience is set"}
with open(sys.argv[1]) as fh:
    raw = fh.read()
docs = [d for d in yaml.safe_load_all(raw) if d]
cms = [d for d in docs if d.get("kind") == "ConfigMap" and d["metadata"]["name"].endswith("-console-env")]
problems = []
if len(cms) != 1:
    problems.append(str(len(cms)) + " console-env ConfigMap(s), want 1")
else:
    data = cms[0].get("data") or {}
    for key in sorted(want):
        if want[key] == "-":
            if key in raw or marker[key] in raw:
                problems.append(key + " or its comment is in the render, want it absent")
        elif data.get(key) != want[key]:
            problems.append(key + " is " + repr(data.get(key)) + ", want " + repr(want[key]))
print("|".join(problems) if problems else "OK")' "$TMP/oidc.yaml" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

# check_console_oidc_restart <label> <scopes|audience>
# The second render changes only the named value. Both console pod templates must differ from
# the first render. The IAM pod template must be equal. The gateway zone is on.
check_console_oidc_restart() {
  local label="$1" mode="$2" out
  local first second
  if [ "$mode" = "scopes" ]; then
    first=(--set zones.gateway.enabled=true)
    second=(--set zones.gateway.enabled=true --set "oidc.scopes=$API_SCOPES")
  else
    first=(--set zones.gateway.enabled=true --set oidc.audience=api://paigasus)
    second=(--set zones.gateway.enabled=true --set oidc.audience=api://paigasus --set oidc.authorizationAudience=api://paigasus)
  fi
  OIDC_ROWS=$((OIDC_ROWS + 1))
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" "${first[@]}" >"$TMP/oidc-restart-1.yaml" 2>"$TMP/oidc-restart.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/oidc-restart.err"; ec=1; return 0
  fi
  if ! helm template paigasus "$CHART" "${BASE[@]+"${BASE[@]}"}" "${second[@]}" >"$TMP/oidc-restart-2.yaml" 2>"$TMP/oidc-restart.err"; then
    echo "FAIL [$label]: render failed"; cat "$TMP/oidc-restart.err"; ec=1; return 0
  fi
  if ! out="$(python3 -c '
import sys, yaml
def templates(path):
    with open(path) as fh:
        docs = [d for d in yaml.safe_load_all(fh) if d]
    return {d["spec"]["template"]["metadata"]["labels"]["app.kubernetes.io/name"]: d["spec"]["template"] for d in docs if d.get("kind") == "Deployment"}
a, b = templates(sys.argv[1]), templates(sys.argv[2])
want = ["gateway-console", "iam-backend", "iam-console"]
problems = []
if sorted(a) != want or sorted(b) != want:
    problems.append("Deployments are " + repr(sorted(a)) + " and " + repr(sorted(b)) + ", want " + repr(want))
else:
    problems += [n + ": spec.template is equal; it must differ" for n in ("gateway-console", "iam-console") if a[n] == b[n]]
    if a["iam-backend"] != b["iam-backend"]:
        problems.append("iam-backend: spec.template differs; it must be equal")
print("|".join(problems) if problems else "OK")' "$TMP/oidc-restart-1.yaml" "$TMP/oidc-restart-2.yaml" 2>&1)"; then
    echo "FAIL [$label]: the checker failed"; printf '%s\n' "$out"; ec=1; return 0
  fi
  if [ "$out" = "OK" ]; then echo "  ok [$label]"; else echo "FAIL [$label]: $out"; ec=1; fi
}

check_console_oidc "O1 unset"              - -
check_console_oidc "O2 both-set"           "$API_SCOPES" api://paigasus \
  --set "oidc.scopes=$API_SCOPES" --set oidc.audience=api://paigasus --set oidc.authorizationAudience=api://paigasus
check_console_oidc "O3 reuse-values-no-key" - - --set oidc.scopes=null --set oidc.authorizationAudience=null
check_console_oidc "O4 number"             - 123 --set oidc.audience=123 --set oidc.authorizationAudience=123
check_console_oidc_restart "O5 restart-scopes"   scopes
check_console_oidc_restart "O6 restart-audience" audience

if [ "$OIDC_ROWS" -lt "$OIDC_ROWS_WANT" ]; then
  echo "FAIL [oidc rows]: $OIDC_ROWS oidc row(s) ran, want $OIDC_ROWS_WANT"; ec=1
fi

if [ "$ec" -eq 0 ]; then echo "== chart env OK =="; fi
```

Note: `local first second` and a later `first=(…)` is valid in bash 3.2. Both arrays always have elements, so they need no `+` guard.

- [ ] **Step 3: Run the chart scripts, expect FAIL**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 -c 'import yaml'
/bin/bash charts/paigasus/tests/refusals.sh --set ingress.host=console.example.test; echo "rc=$?"
/bin/bash charts/paigasus/tests/env.sh; echo "rc=$?"
```

Expected: `refusals.sh` prints `FAIL [authorizationAudience with oidc.audience empty]: rendered, expected a refusal`, the same for the other three `expect_fail` rows, the two new `expect_render` rows pass, `rc=1`. `env.sh` prints `ok` for O1 and O3, `FAIL [O2 both-set]` (both keys `None`), `FAIL [O4 number]`, `FAIL [O5 restart-scopes]` and `FAIL [O6 restart-audience]` (`spec.template is equal; it must differ`), `rc=1`.

- [ ] **Step 4: Add the values**

In `charts/paigasus/values.yaml`, insert after line 106 (the last comment line of `acknowledgeClientIdAudience`, which ends `See RUNBOOK-chart.md § 6.`):

```yaml
  scopes: ""             # NOT required. The scopes that both consoles request (SMA-692). Empty:
                         # the console default, openid profile email offline_access. Entra ID
                         # needs it: add exactly one scope of the API app registration, for
                         # example api://paigasus-api/access. The list must contain openid (the
                         # render fails without it). It should contain offline_access: without
                         # it the IdP issues no refresh token, and every user must log in again
                         # at each access-token expiry. When set, the consoles also send it on
                         # each refresh. A change restarts both consoles, not IAM. See
                         # RUNBOOK-chart.md § 6.
  authorizationAudience: ""   # NOT required. The audience parameter that both consoles send in
                              # the authorization request (SMA-692). Empty: no audience
                              # parameter. Auth0 needs it to issue an access token for an API.
                              # It must equal oidc.audience (the render fails otherwise). A change
                              # restarts both consoles, not IAM. Quote the value in a values file.
                              # See RUNBOOK-chart.md § 6.
```

- [ ] **Step 5: Add the two helpers**

In `charts/paigasus/templates/_audience.tpl`, append after the last line (`{{- end -}}` of `paigasus.iamAudienceNotes`):

```
{{/*
paigasus.consoleScopes (SMA-692 D6, D9): oidc.scopes as a string, or "" when it is empty or
absent. dig reads an absent key as "" (helm upgrade --reuse-values from a release made before
the key existed). The render fails when the value is set and its whitespace-separated tokens do
not include openid: without openid the first console login fails, and @paigasus/auth refuses the
value at pod start with only "PAIGASUS_OIDC_SCOPES: custom". openidx does not count.
paigasus.validate in _helpers.tpl holds the other refusals. This one is here so that _helpers.tpl
and its two whole-file fixture copies do not change (spec D8). console-env-configmap.yaml calls
it, and that file renders on every install, with no condition.
*/}}
{{- define "paigasus.consoleScopes" -}}
{{- $scopes := dig "scopes" "" .Values.oidc -}}
{{- if $scopes -}}
{{- $scopes = toString $scopes -}}
{{- if not (has "openid" (regexSplit "\\s+" $scopes -1)) -}}
{{- fail (printf "oidc.scopes must contain the scope openid, got %q. Without it the console login fails. See docs/ops/RUNBOOK-chart.md section 6 (SMA-692)" $scopes) -}}
{{- end -}}
{{- $scopes -}}
{{- end -}}
{{- end -}}

{{/*
paigasus.consoleAuthorizationAudience (SMA-692 D6, D7): oidc.authorizationAudience as a string,
or "" when it is empty or absent. The render fails when the value is set and:
  1. oidc.audience is empty. IAM then accepts only oidc.clientId, and Auth0 refuses a client id
     as an API audience. This also closes authorizationAudience == clientId.
  2. it does not equal oidc.audience. The console would then ask for a token that IAM refuses.
The compare is exact, as strings (toString on both sides, so a number from --set compares by its
digits). A space-separated list of audiences is out of scope. The placement follows
paigasus.consoleScopes above.
*/}}
{{- define "paigasus.consoleAuthorizationAudience" -}}
{{- $want := dig "authorizationAudience" "" .Values.oidc -}}
{{- if $want -}}
{{- $want = toString $want -}}
{{- $iam := dig "audience" "" .Values.oidc -}}
{{- if not $iam -}}
{{- fail (printf "oidc.authorizationAudience is %q while oidc.audience is empty. IAM then accepts only oidc.clientId, and the IdP refuses a client id as an API audience. Set oidc.audience to the same value. See docs/ops/RUNBOOK-chart.md section 6 (SMA-692)" $want) -}}
{{- end -}}
{{- if ne $want (toString $iam) -}}
{{- fail (printf "oidc.authorizationAudience %q does not equal oidc.audience %q. The console would request a token that IAM refuses. See docs/ops/RUNBOOK-chart.md section 6 (SMA-692)" $want (toString $iam)) -}}
{{- end -}}
{{- $want -}}
{{- end -}}
{{- end -}}
```

- [ ] **Step 6: Render the two keys**

In `charts/paigasus/templates/console-env-configmap.yaml`, append after line 15 (`  PAIGASUS_SESSION_STORE: "redis"`). The YAML comments are INSIDE the `with` blocks, so an empty value renders no byte and the goldens stay identical (`charts/CLAUDE.md`):

```
{{- with include "paigasus.consoleScopes" . }}
  # oidc.scopes is set (SMA-692). Both consoles request these scopes, and send them again on
  # each refresh.
  PAIGASUS_OIDC_SCOPES: {{ . | quote }}
{{- end }}
{{- with include "paigasus.consoleAuthorizationAudience" . }}
  # oidc.authorizationAudience is set (SMA-692). Both consoles send it as the audience
  # parameter of the authorization request. It equals oidc.audience.
  PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE: {{ . | quote }}
{{- end }}
```

- [ ] **Step 7: Run the chart scripts and the goldens, expect PASS**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash charts/paigasus/tests/refusals.sh --set ingress.host=console.example.test; echo "rc=$?"
/bin/bash charts/paigasus/tests/env.sh; echo "rc=$?"
/bin/bash charts/paigasus/tests/render.sh; echo "rc=$?"
git diff --stat charts/paigasus/tests/golden
```

Expected: `== chart refusals OK ==` and `rc=0`; every O row `ok`, `== chart env OK ==` and `rc=0`; `== chart render OK ==` and `rc=0`; the `git diff --stat` prints nothing.

- [ ] **Step 8: Re-sync the leaked-value fixture**

The fixture is a whole-file copy of the live ConfigMap plus four mutation lines (`ci/helm-render/README.md`, "Refreshing a stale fixture"). Write `ci/helm-render/fixtures/leaked-value/templates/console-env-configmap.yaml` with the Write tool, with exactly this content (the live file, with the four mutation lines kept after `PAIGASUS_SESSION_STORE`):

```
{{/* SPDX-License-Identifier: Apache-2.0 */}}
{{- include "paigasus.validate" . -}}
apiVersion: v1
kind: ConfigMap
metadata:
  name: {{ include "paigasus.name" (list . "console-env") }}
data:
  PAIGASUS_PUBLIC_ORIGIN: {{ printf "https://%s" .Values.ingress.host | quote }}
  PAIGASUS_OIDC_ISSUER: {{ .Values.oidc.issuer | quote }}
  PAIGASUS_OIDC_CLIENT_ID: {{ .Values.oidc.clientId | quote }}
  PAIGASUS_IAM_GRPC_URL: {{ printf "http://%s:%d" (include "paigasus.name" (list . "iam-backend")) (int .Values.zones.iam.backend.grpcPort) | quote }}
  # Forced, not exposed. createAuthRuntime refuses "memory" once PAIGASUS_ZONES names more than
  # one zone, and console-core's descriptor cache needs Redis regardless. A knob whose only other
  # setting crash-loops the pod is not a knob.
  PAIGASUS_SESSION_STORE: "redis"
  # NEGATIVE-CONTROL FIXTURE (ci/helm-render): written outside any range, so it leaks
  # zones.gateway.backend.url even when the gateway zone is disabled. Check 2 must fail and
  # check 1.1 must stay green.
  PAIGASUS_UPSTREAM_URL: {{ .Values.zones.gateway.backend.url | quote }}
{{- with include "paigasus.consoleScopes" . }}
  # oidc.scopes is set (SMA-692). Both consoles request these scopes, and send them again on
  # each refresh.
  PAIGASUS_OIDC_SCOPES: {{ . | quote }}
{{- end }}
{{- with include "paigasus.consoleAuthorizationAudience" . }}
  # oidc.authorizationAudience is set (SMA-692). Both consoles send it as the audience
  # parameter of the authorization request. It equals oidc.audience.
  PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE: {{ . | quote }}
{{- end }}
```

Check that the only difference to the live file is the four mutation lines:

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
diff charts/paigasus/templates/console-env-configmap.yaml ci/helm-render/fixtures/leaked-value/templates/console-env-configmap.yaml
```

Expected: exactly one hunk, `15a16,19`, with the four `>` lines of the mutation.

- [ ] **Step 9: Update `charts/CLAUDE.md`**

Replace lines 22-25:

```markdown
- **Four negative-control fixtures are whole-file copies** of `templates/_helpers.tpl` (two) and
  `templates/console-deployment.yaml` (two), under `ci/helm-render/fixtures/`. An edit to either
  live file must re-sync them in the SAME commit, keeping each mutation. A stale `_helpers.tpl`
  fixture has none of the new helpers, so its renders fail and the control reports INCONCLUSIVE.
```

with:

```markdown
- **All six negative-control fixtures are whole-file copies** of a live template, under
  `ci/helm-render/fixtures/`: `templates/_helpers.tpl` (`slug-mirror`, `zones-omits-enabled`),
  `templates/console-deployment.yaml` (`security-context`, `template-only-diff`),
  `templates/console-env-configmap.yaml` (`leaked-value`) and `templates/ingress.yaml`
  (`literal-ingress`). An edit to one of these live files must re-sync its copies in the SAME
  commit, keeping each mutation. A stale `_helpers.tpl` fixture has none of the new helpers, so its
  renders fail and the control reports INCONCLUSIVE.
```

- [ ] **Step 10: Add the chart README section**

In `charts/paigasus/README.md`, insert before `## The default image tags` (line 151), after the blank line that ends the audience section:

```markdown
## The console authorization request (`oidc.scopes`, `oidc.authorizationAudience`)

Two values change what both consoles request from the IdP (SMA-692). Both are empty by default,
and an empty value renders no key, so the render is byte-identical to a chart without them.

- `oidc.scopes` renders `PAIGASUS_OIDC_SCOPES` into the `console-env` ConfigMap. Empty: the
  console default, `openid profile email offline_access`. When set, the consoles also send the
  list as the `scope` of each refresh request. Entra ID needs it (one scope of the API).
- `oidc.authorizationAudience` renders `PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE`. The consoles send it
  as the `audience` parameter of the authorization request, never on a refresh. Auth0 needs it.
- The helpers are `paigasus.consoleScopes` and `paigasus.consoleAuthorizationAudience` in
  `templates/_audience.tpl`. They read the values with `dig`, so an absent key under
  `--reuse-values` gives `""`. `console-env-configmap.yaml` calls them. That file renders on every
  install, so their refusals fire on every render. They are not in `paigasus.validate`, so
  `_helpers.tpl` and its two fixture copies do not change.
- The render fails when `oidc.scopes` is set and does not hold the token `openid` (`openidx` does
  not count), when `oidc.authorizationAudience` is set and `oidc.audience` is empty, and when the
  two audience values differ. The compare is exact, as strings.
- A change of either value changes the `console-env` ConfigMap, so `checksum/console-env` restarts
  both consoles. IAM does not restart.

`tests/env.sh` holds the rows `O1 unset`, `O2 both-set`, `O3 reuse-values-no-key`, `O4 number`,
`O5 restart-scopes` and `O6 restart-audience`, with a fourth row counter. `tests/refusals.sh` holds
the four refusals and two valid renders. See `docs/ops/RUNBOOK-chart.md` § 6 for the IdP setup.

```

- [ ] **Step 11: The mutations**

Run each mutation with the Edit tool, run the named script, see the named FAIL, restore with the inverse Edit, run again to see the pass:

- 4a. In `_audience.tpl`, delete the line `{{- fail (printf "oidc.scopes must contain the scope openid, got %q. …" $scopes) -}}` (keep its `if`/`end`). Run `/bin/bash charts/paigasus/tests/refusals.sh --set ingress.host=console.example.test`. Expected FAIL: `scopes without openid` and `scopes with openidx` (`rendered, expected a refusal`).
- 4b. In `_audience.tpl`, replace `(has "openid" (regexSplit "\\s+" $scopes -1))` with `(contains "openid" $scopes)`. Run `refusals.sh` as in 4a. Expected FAIL: `scopes with openidx` only.
- 4c. In `_audience.tpl`, delete the line `{{- fail (printf "oidc.authorizationAudience is %q while oidc.audience is empty. …" $want) -}}`. Run `refusals.sh`. Expected FAIL: `authorizationAudience with oidc.audience empty` (it now fails with the `does not equal` message, which is not its own message).
- 4d. In `_audience.tpl`, delete the line `{{- fail (printf "oidc.authorizationAudience %q does not equal oidc.audience %q. …" $want (toString $iam)) -}}`. Run `refusals.sh`. Expected FAIL: `authorizationAudience differs from oidc.audience`.
- 4e. In `console-env-configmap.yaml`, delete the line `  PAIGASUS_OIDC_SCOPES: {{ . | quote }}`. Run `/bin/bash charts/paigasus/tests/env.sh`. Expected FAIL: `O2 both-set` only. `O5 restart-scopes` stays green, because the comment lines still change the ConfigMap and so its checksum.
- 4f. In `console-env-configmap.yaml`, delete the line `  PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE: {{ . | quote }}`. Run `env.sh`. Expected FAIL: `O2 both-set` and `O4 number`.
- 4g. In `env.sh`, delete the line `check_console_oidc_restart "O6 restart-audience" audience`. Run `env.sh`. Expected FAIL: `[oidc rows]: 5 oidc row(s) ran, want 6`.
- 4h. In `_audience.tpl`, replace `{{- if ne $want (toString $iam) -}}` with `{{- if ne $want $iam -}}`. Run `env.sh`. Expected FAIL: `O4 number` (the render fails: Go's `ne` refuses to compare a string with an int64). Run `refusals.sh`: `authorizationAudience equal to oidc.audience` stays green (two strings).

- [ ] **Step 12: The helm-render gate, all three modes**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash ci/helm-render/run.sh --self-test; echo "rc=$?"
/bin/bash ci/helm-render/run.sh --negative-control; echo "rc=$?"
/bin/bash ci/helm-render/run.sh; echo "rc=$?"
git diff --stat charts/paigasus/tests/golden
```

Expected: `== helm-render self-test passed ==` rc 0; `negative-control OK [leaked-value]` among six OK lines and `== helm-render negative control passed (6 fixtures) ==` rc 0; `PASS  [6 env.sh]`, `PASS  [6 refusals.sh]`, `PASS  [6 render.sh]` and `== helm-render: all checks passed ==` rc 0; no golden diff.

- [ ] **Step 13: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
git add charts/paigasus/values.yaml charts/paigasus/templates/_audience.tpl charts/paigasus/templates/console-env-configmap.yaml ci/helm-render/fixtures/leaked-value/templates/console-env-configmap.yaml charts/CLAUDE.md charts/paigasus/README.md charts/paigasus/tests/env.sh charts/paigasus/tests/refusals.sh
git commit -m "feat(repo): chart values for the console scopes and authorization audience (SMA-692)" \
  -m "oidc.scopes and oidc.authorizationAudience render into the console-env ConfigMap only when
set, so the goldens do not change. The render fails when the scopes do not hold openid, and
when the authorization audience is set with an empty or a different oidc.audience. The
leaked-value fixture is a whole-file copy of the ConfigMap and is re-synced." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: The runbook (§ 5.3 of the spec)

Every edit below replaces a statement that SMA-692 makes false or incomplete. The runbook has no test. Step 7 checks by `grep` that each stale statement is gone and each new one is present.

**Files:**
- Modify: `docs/ops/RUNBOOK-chart.md:26-27` (§ 1 table), `:38-49` (§ 2), `:86-87` (§ 5 table), `:104-106` (§ 6 item 1), `:136` (§ 6 scopes), `:156-157` (§ 6 after the migration order), `:203-209` (§ 6 Auth0 and Entra ID)

**Interfaces:**
- Consumes: the value names and the refusal messages of Task 4, the env behaviour of Task 3.
- Produces: operator text only.

- [ ] **Step 1: § 1, the values table**

After the row `| \`oidc.acknowledgeClientIdAudience\` | no | … |` (line 27), insert:

```markdown
| `oidc.scopes` | no | The scopes that both consoles request. Empty: `openid profile email offline_access`. The list must contain `openid`, or the render fails. Keep `offline_access`, or the IdP issues no refresh token. When set, the consoles also send it on each refresh. Entra ID needs it (§ 6) |
| `oidc.authorizationAudience` | no | The `audience` parameter that both consoles send in the authorization request. Empty: no `audience` parameter. It must equal `oidc.audience`, or the render fails. Auth0 needs it (§ 6) |
```

- [ ] **Step 2: § 2, the refused list**

Replace lines 38-39:

```markdown
`paigasus.validate` in `templates/_helpers.tpl` stops the render with its own message. The chart
refuses:
```

with:

```markdown
`paigasus.validate` in `templates/_helpers.tpl` stops the render with its own message. The last
two items in this list are in `templates/_audience.tpl` instead. `templates/console-env-configmap.yaml`
calls them, and that file renders on every install. The chart refuses:
```

Replace line 49:

```markdown
- `oidc.caBundle.existingConfigMap` set with an empty `oidc.caBundle.key`.
```

with:

```markdown
- `oidc.caBundle.existingConfigMap` set with an empty `oidc.caBundle.key`;
- `oidc.scopes` set without the scope `openid` (`openidx` does not count);
- `oidc.authorizationAudience` set while `oidc.audience` is empty, or set to a value that does not
  equal `oidc.audience`.
```

- [ ] **Step 3: § 5, the restart table**

After the row `| \`oidc.acknowledgeClientIdAudience\` | nothing | … |` (line 87), insert:

```markdown
| `oidc.scopes` or `oidc.authorizationAudience` | both consoles, not IAM | it changes the `console-env` ConfigMap and so `checksum/console-env` (`tests/env.sh` rows O5 and O6) |
```

- [ ] **Step 4: § 6, item 1 and the scope sentence**

Replace lines 104-106:

```markdown
   The value helps only when the IdP issues a JWT access token for the console's scopes
   (`openid profile email offline_access`). The console sends no `audience` or `resource`
   parameter. This value does not work with an opaque token, or a token for a different API.
```

with:

```markdown
   The value helps only when the IdP issues a JWT access token for the console's scopes
   (`oidc.scopes`, default `openid profile email offline_access`). The console sends an
   `audience` parameter only when `oidc.authorizationAudience` is set. It never sends a
   `resource` parameter. This value does not work with an opaque token, or a token for a
   different API.
```

Replace line 136:

```markdown
The console requests the scopes `openid profile email offline_access`.
```

with:

```markdown
By default the console requests the scopes `openid profile email offline_access`. Set
`oidc.scopes` to request a different list. The list must contain `openid`. Keep `offline_access`:
without it the IdP issues no refresh token, and every user must log in again when the access
token expires. When `oidc.scopes` is set, the console also sends the list as the `scope` of each
refresh request. When it is empty, a refresh request has no `scope`, as before SMA-692.
```

- [ ] **Step 5: § 6, the migration cases for Auth0 and Entra ID**

After line 157 (`that replaces \`aud\`, and does not add to it, has the same result.`), insert:

```markdown

**Migration for Auth0 and Entra ID (SMA-692).** Step 1 above adds a second audience next to the
client id. Auth0 has no equal step: an Auth0 access token has one API audience. The chart also
refuses `oidc.authorizationAudience` unless it equals `oidc.audience`, so IAM and the consoles
change in one upgrade. The real cases:

- **Auth0 with a tenant Default Audience D, moving to `oidc.authorizationAudience` = D.** The
  tokens do not change. Nothing breaks.
- **Auth0 moving from no API (or from D) to a new API A.** This is a hard cut. A session that
  logged in before the upgrade keeps a token for the old audience, and an Auth0 refresh keeps
  that audience. IAM refuses those tokens. Every user must log in again. IAM and the consoles
  restart at different times.
- **Entra ID moving to a new scope list.** The refresh of an old session can fail, because the
  refresh now sends the new `oidc.scopes`. The error code is not measured. Entra ID reports many
  conditions as `invalid_grant`, which deletes the session. It reports others as
  `invalid_scope`, which ends the session when the access token expires. In both cases the user
  must log in again. The console log line `session.refresh_failed` shows the code in the field
  `oauthError`.
- **Mixed pods.** During the rollout, old and new console pods share one session store. For a
  short time, a pod with the other scope list can refresh a session.
```

- [ ] **Step 6: § 6, the Auth0 and Entra ID lines**

Replace lines 203-209:

```markdown
- **Auth0.** The console cannot send the `audience` parameter today. It sends only `scope`
  (`ts/packages/paigasus-auth/src/adapters/oidc.ts`). The tenant "Default Audience" setting is a
  possible path. It is not measured.
- **Entra ID.** An access token for an API application ID URI needs a scope of that API. The
  console reads its scopes from `PAIGASUS_OIDC_SCOPES`, but the chart has no value for it. So
  Entra ID cannot use a dedicated audience with this chart today. A follow-up issue tracks a
  configurable scope list.
```

with:

```markdown
- **Auth0.** Auth0 issues an access token for an API only when the authorization request has the
  `audience` parameter, or when the tenant has a "Default Audience".
  - Create an API. Its identifier is the audience. Enable "Allow Offline Access" on the API, or
    the console gets no refresh token.
  - Set `oidc.audience` and `oidc.authorizationAudience` to that identifier. Quote both values in
    a values file.
  - IAM needs `email` in the access token (item 2). Auth0 does not put it into an API access
    token by default. Add it with a post-login Action. Not measured.
  - The console does not send `audience` on a refresh. Auth0 keeps the original audience on a
    refresh.
  - Alternative: the tenant "Default Audience". It applies to every application of the tenant.
    With it, `oidc.authorizationAudience` can stay empty.
- **Entra ID.** An access token for an API needs a scope of that API.
  - Register a SEPARATE app registration for the API. Do not expose the API on the console's own
    registration: the v2 `aud` would then be the console's client id, which is the setup that the
    warning above is about.
  - Set its `accessTokenAcceptedVersion` to 2. IAM refuses a v1.0 access token: its `iss`
    (`https://sts.windows.net/<tenant>/`) is not the v2 issuer of the discovery document. v1.0 is
    not supported.
  - Expose one scope, give the console's registration the delegated permission, and give
    consent.
  - Set `oidc.scopes` to `openid profile email offline_access` plus exactly one scope of that API,
    for example `api://paigasus-api/access`. One request can hold scopes of only one resource.
  - Set `oidc.audience` to the API registration's application (client) id, a GUID. A v2 access
    token has that value as its `aud`.
  - Add `email` as an optional claim of the access token. Not measured.
  - Before the switch, decode a real access token and check its `aud`, `iss` and `email`.
```

- [ ] **Step 7: Check the edits**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
grep -cE 'The console cannot send the `audience` parameter today|the chart has no value for it|The console sends no `audience` or `resource`' docs/ops/RUNBOOK-chart.md
grep -c 'oidc.authorizationAudience' docs/ops/RUNBOOK-chart.md
grep -c 'oidc.scopes' docs/ops/RUNBOOK-chart.md
```

Expected: the first count is `0`. The second and third counts are each `5` or more.

- [ ] **Step 8: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
git add docs/ops/RUNBOOK-chart.md
git commit -m "docs(repo): runbook setup for an Auth0 or Entra ID API audience (SMA-692)" \
  -m "The runbook names oidc.scopes and oidc.authorizationAudience in the values, refusal and
restart tables. Section 6 states the Auth0 and Entra ID setup and the migration cases. The
statements that the console cannot send an audience or a scope list are removed." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Final verification

No new code. This task runs the gates that the changed files select, and records the result in the report to the controller. It makes no commit.

**Files:** none.

**Interfaces:** none.

- [ ] **Step 1: The ts gates**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-auth-ts:test paigasus-auth-ts:typecheck iam-console-ts:typecheck gateway-console-ts:typecheck paigasus-console-core-ts:typecheck ts:lint ts:fmt
```

Expected: every target passes.

- [ ] **Step 2: The auth e2e tier, when Docker runs**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
docker info >/dev/null 2>&1 && moon run paigasus-auth-ts:test-e2e
```

Expected: when Docker runs, the tier passes against Keycloak with the default scope path (Task 3 removed the explicit `PAIGASUS_OIDC_SCOPES`). When Docker does not run, report that the tier did not run locally; CI runs it.

- [ ] **Step 3: The chart gate**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-692
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash ci/helm-render/run.sh --self-test && /bin/bash ci/helm-render/run.sh --negative-control && /bin/bash ci/helm-render/run.sh
git diff --stat origin/main -- charts/paigasus/tests/golden
```

Expected: the three modes pass; no golden diff against `origin/main`.

- [ ] **Step 4: The full graph before the push**

Run the full `moon ci` command that the root `CLAUDE.md` holds under "Before you push: the full gate graph", with `--base origin/main --include-relations`. Read the root `CLAUDE.md` section "This development Mac only" first: `repo:affected-smoke` needs `/bin/bash` 3.2, and `repo:ruff-ci`, `repo:next-public-free` and `repo:publish-metadata` need bash 4+. Re-run a gate that fails only for its bash under the other bash directly (`<bash-binary> ci/<gate>/run.sh`) and report both results. Do not read a bash-version failure as a finding.

Expected: every selected target passes, or each failure is a documented host artifact with its direct re-run green.

## Self-review against the spec

**Coverage.**

| Spec item | Task |
| -- | -- |
| § 5.1 `authEnvShape`: audience env, `openid` refinement, no default (D1, D3-a, D5) | 3 |
| § 5.1 adapter: `scopes`, `audience`, `refreshScope`; `BuildAuthorizationUrlParams`; F7 omission | 2 |
| § 5.1 D10 in the adapter | 1 (class, log), 2 (classifier) |
| § 5.1 runtime factory, `AuthRuntime.scopes` removed | 3 |
| § 5.1 `routes.ts` drops `scopes` | 2 |
| § 5.1 README table | 3 (and the Redaction paragraph in 1) |
| § 5.2 values, helpers, ConfigMap, charts README, fixture, `charts/CLAUDE.md` | 4 |
| § 5.3 runbook, every listed section | 5 |
| § 6 error handling (pod start refusal, D11 unchanged) | 3 (refusal), 2 (transient rows unchanged) |
| § 7 `config.test.ts` rows | 3 |
| § 7 `oidc.test.ts` rows and the recorder | 2 |
| § 7 `runtime.test.ts` through the factory | 3 |
| § 7 eight literal sites | 3 |
| § 7 D3-a refresh body with and without the env | 2 (adapter), 3 (runtime) |
| § 7 chart rows in `env.sh` and `refusals.sh` | 4 |
| § 7 goldens byte-identical | 4, 6 |
| § 7 red when the feature line is deleted | mutations 1a-1b, 2a-2e, 3a-3h, 4a-4h |

**Type consistency.** `CreateOidcClientOptions` (Task 2) is the type the Task 3 factory takes. `RefreshFailed` and `toTokenErrorCode` (Task 1) are what Task 2 imports. `TokenErrorCode | 'other'` is the one type of `RefreshFailed.oauthError`, of `refreshFailureCode` and of the log field. `BuildAuthorizationUrlParams` is `{ redirectUri, state }` in the adapter, in `routes.ts` and in the one test call.

**Facts this plan found in the code, not in the spec.**

- `AuthEventFields` has no per-key allow-list (`src/ports/logger.ts:29`). The closed value set of `oauthError` comes from `toTokenErrorCode`. Task 1 pins it with a forged-object row.
- `tests/e2e/fixture-server.ts:124` sets `PAIGASUS_OIDC_SCOPES` explicitly. Under D3-a that would opt the Keycloak e2e tier into a refresh `scope`. Task 3 removes the line. The tier does not refresh today (`roundtrip.spec.ts:117-120` only checks that a refresh token is issued), so no test depends on it.
- `core/errors.ts:120-122` says `CreateAuthRuntimeDeps` exposes no OIDC override. Task 3 makes that false, so Task 3 corrects it.
- `_helpers.tpl:51-54` says every refusal lives in `paigasus.validate`. Spec D8 puts the two new refusals in `_audience.tpl`. `console-env-configmap.yaml` renders on every install with no condition, so the refusals always fire. The helper comments, the charts README and RUNBOOK § 2 say so.
- `charts/CLAUDE.md:22` says four fixtures. There are six whole-file copies, `literal-ingress` (`ingress.yaml`) included. Task 4 names all six.
- RFC 6749 § 5.2 does not list `server_error` or `temporarily_unavailable`. D10 fixes the § 5.2 list, so those two log `other`. Task 2 pins this.
- The commit scopes `charts` and `ops` do not exist. Tasks 4 and 5 use `repo`.
