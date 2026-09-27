<!-- moon-diagnosis:ok -->
# SMA-705 Console OIDC Readiness Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A console pod is not ready until its auth runtime is built and one OIDC discovery succeeded. The Helm chart probes a new `<basePath>/readyz` route, and the chart pins console images that serve it.

**Architecture:** The OIDC adapter gets a synchronous `discoveryStatus()` and a `discover()` that shares the one `configPromise`. `@paigasus/auth/server` exports `readinessResponse(getRuntime, logger)`, which reads the status, starts discovery without `await` when it is idle, and answers 200 `ready` or 503 `unready`. Each console mounts it at `app/readyz/route.ts` and lists `/readyz` in `proxy.ts`. The chart's `readinessProbe` moves to `/readyz`, and both console versions move to `0.2.0`.

**Tech Stack:** TypeScript 5 (ESM, `verbatimModuleSyntax`, `exactOptionalPropertyTypes`), Vitest 5, Playwright 1.63, Next 16.3.4, openid-client 6.8.8, Node 24, Helm 3.22.0, Moon 2.5.3, pnpm 11.

**Spec:** `docs/superpowers/specs/2026-09-27-sma-705-console-oidc-readiness-design.md` (revision 2, approved at GATE 1). Read all of it before you start. Decision, acceptance and test numbers (D1–D11, A1–A4, T1–T17, § 5.6) in this plan refer to it. Do not re-open a decision.

## Spec deviations and resolved open points

The spec was checked against the tree at `1ff0f8fe`. These are the differences. Every other claim that was checked is correct: `runtime.ts` lines 19-22, README lines 189-191 and 212-215, `@redis/client` 6.2.1 `dist/lib/client/index.js:176`, the five `OidcClient` fakes (§ 5.2), `login-returnto-table.test.ts` compiles by `as unknown as`, the two whole-file fixture copies and their intended differences, and the `chart.yml` path filters (§ 8).

1. **§ 4.5, `import 'server-only'` in the route file (left open by the spec).** No route file in either app imports it: `app/healthz/route.ts`, `app/auth/[...auth]/route.ts` and `gateway-console/app/api/chat/route.ts`. `lib/auth.ts`, which the route imports, already starts with `import 'server-only'`. The plan leaves the line out.
2. **§ 4.4, `errorName` (left open by the spec).** The plan does not share `libraryErrorName`. That helper is private to `adapters/oidc.ts`, and it names library errors. `http/readiness.ts` gets its own one-line `errorName` with the same rule.
3. **D10 and § 6, "the log shows ... the error name".** `AuthConfigError` (every cross-field rule of `createAuthRuntime`) set no `name`, so its `name` was `Error`. **Decided after GATE 1 by the issue owner: add the name in this PR.** Task 3 Step 0 sets `this.name = 'AuthConfigError'`, as `SessionStoreTimeout` does, with a test. A cross-field refusal then logs `readiness.runtime_failed { error: 'AuthConfigError' }`. Side effect, accepted: Next also prints `AuthConfigError: …` in place of `Error: …` when this error escapes an auth route.
4. **§ 4.6 and § 9, the golden files change twice.** The golden files also pin the console image tags. So Task 9 (the version bump) regenerates them again, for the image lines only.
5. **§ 7, `ci/kind/README.md` "Where to look first".** It is a paragraph label inside "## Reading the evidence", not a heading. Task 10 adds the row there.
6. **§ 5.5 T15, added pin.** The spec asks for `"stage":"readiness"` in the output. The plan also pins `zone` and `reason: 'network'` (port 1 is on the Fetch "bad port" list, measured by SMA-656). A STOP rule covers a different `reason`.
7. **The brief's commit scope `charts` is not allowed.** `ts/packages/commitlint-config/index.cjs` line 42 allows `rs, py, ts, contracts, ci, docs, deps, release, repo, claude, workspace`. Chart commits use `repo` (the SMA-697, SMA-691 and SMA-678 precedent).
8. **§ 5.5 T16, confirmed by reading.** The iam-console e2e harness starts a TLS fake IdP (`startFakeIdp`, `NODE_EXTRA_CA_CERTS`), and `login.spec.ts` rows R2, R3 and R12 complete OIDC logins through the same build. A login needs discovery, so the harness can reach the discovered state. Task 7 still has a STOP rule. The new test title must not start with `R<n>:`, because `tests/unit/e2e-rows.test.ts` pins the rows R1–R20.

## Global Constraints

- Worktree: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness`. Branch: `feature/sma-705-auth-oidc-readiness`. Use only this tree and absolute paths. Before the first commit of each task, run `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness branch --show-current` and confirm the branch name. Do not switch branches.
- `WT` in this plan means the worktree path above. `PKG` means `WT/ts/packages/paigasus-auth`. Write the literal path in every command: the worktree sandbox refuses a git command that holds a shell variable.
- Shell setup for every command: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. A Moon command runs from `WT`: `cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness && moon run …`.
- Every new source file starts with `// SPDX-License-Identifier: Apache-2.0` (`#` for YAML and shell).
- Write every comment, doc line and CHANGELOG line in ASD-STE100 Simplified Technical English: short sentences, active voice, one idea per sentence, no idiom. Keep technical names exactly as they are.
- Relative imports in `PKG/src/` are EXTENSIONLESS (`'./discovery-log'`). Test files import with `.js` (`'../../src/http/readiness.js'`), as the existing tests do.
- Commits: conventional, with an allowed scope (`ts`, `repo`, `release`, `docs`). The subject ends with `(SMA-705)`. Use `git -C <WT> commit -m "<subject>" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"`. Put no `#NNN` line and no `token: value` line in a commit body (commitlint `footer-leading-blank`).
- Never use `git commit --amend`, `git reset`, `--no-verify`, a bare `git stash`, or `git checkout -- <file>`. Undo a mutation by an edit.
- Run every command in the foreground. Start no background job. Do not install host software (no `brew`, no global `npm install`).
- Do not touch Rust (`rs/`). Do not edit the approved spec.
- Do not write the token `ciReport` into any new file. A tracked file that holds it needs a `<!-- moon-diagnosis:ok -->` marker (`repo:actionlint` check 12). This plan file has the marker.
- Run ONE auth test file: `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/packages/paigasus-auth exec vitest run <path>`. The package has NO `test` script, so `pnpm --filter @paigasus/auth test` is a SILENT NO-OP. Read the `Test Files … passed` line: the Moon task passes `--passWithNoTests`, so a wrong path passes with zero files.
- Moon targets: `paigasus-auth-ts:test`, `paigasus-auth-ts:typecheck`, `paigasus-console-core-ts:typecheck`, `iam-console-ts:build|test|typecheck|test-e2e`, `gateway-console-ts:build|test|typecheck|test-e2e`, `ts:lint`, `ts:fmt`. `:typecheck` is a separate target: a green vitest does NOT mean `tsc` passes. Every task that changes a `.ts` file runs the typecheck of each project it touched.
- Lint one file set fast: `pnpm -C <WT>/ts exec eslint <paths relative to ts/>`. Format: `pnpm -C <WT>/ts exec prettier --write <paths relative to ts/>`, then `moon run ts:fmt --force` (`printWidth` is 200). `ts:fmt` does not key on a `README.md`, a `CHANGELOG.md` or a `package.json` edit, so always pass `--force`.
- The chart gates on this Mac: run `ci/helm-render/run.sh` and `charts/paigasus/tests/render.sh` with **system `/bin/bash` (3.2.57)** from `WT`, with `export PROTO_REPORTER=text`. Both scripts use no `mapfile`, no `declare -A` and no here-string (`ci/helm-render/README.md`, "Running it locally"). `render.sh` captures `helm` output through the proto shim, and a shim can leak proto's NDJSON into captured stdout (root `CLAUDE.md`, SMA-609), so `PROTO_REPORTER=text` is mandatory there. Do not run the helm-render gate inside Docker: a worktree's `.git` links to a host directory.

## Review Focus

These five input classes are the most likely to bite a person, and no spec test pins them fully. Each line names the task that adds the test.

1. **The kubelet sends no cookie and follows a same-host redirect.** A probe that the proxy redirects to `/auth/login` still "answers", but it does not test `/readyz` (D9). Expected: no redirect, 503 `unready`, no `location` header. Tests: Task 5 and Task 6 (T14 unit rows, T15 with `redirect: 'manual'`).
2. **A probe and a login at the same moment on a cold pod.** Expected: one discovery request to the IdP. Tests: Task 1 (T3), Task 3 (T13a asserts `discoveryRequests() === 1`).
3. **A login that overlaps a failing readiness attempt.** Expected: one `oidc.discovery_failed` event for each caller, with the stages `login` and `readiness` (D8), and a 503 for both. Test: Task 3 (the D8 overlap row, through the real adapter and the real login route).
4. **The runtime build fails, then the operator fixes the cause.** `getAuthRuntime` clears its slot, so the next probe builds again. Expected: the handler keeps no state, so the second call reaches the discovery status and answers 200. Test: Task 3 (T11b).
5. **A real cross-field refusal from `createAuthRuntime`.** Expected: 503 `unready` and `readiness.runtime_failed { error: 'AuthConfigError' }` (deviation 3). Test: Task 3 (the `AuthConfigError` row).

## File Structure

| File | Change | Task |
|---|---|---|
| `PKG/src/adapters/oidc.ts` | modify: `OidcDiscoveryStatus`, `discoveryStatus()`, `discover()`, the `discovered` flag, the header | 1 |
| `PKG/src/runtime.ts` | modify: the "DISCOVERY STAYS LAZY" comment | 1 |
| `PKG/tests/fixtures/jwks.ts` | modify: `discoveryRequests()` | 1 |
| `PKG/tests/adapters/oidc.test.ts` | modify: T1–T4 | 1 |
| `PKG/tests/support/store-failure.ts` | modify: `FakeOidc` status and `discover()` control | 1 |
| `PKG/tests/next/get-session.test.ts`, `PKG/tests/http/logout.test.ts`, `PKG/tests/http/callback.test.ts`, `PKG/tests/http/route-handler.test.ts` | modify: the two new fake members | 1 |
| `PKG/src/ports/logger.ts` | modify: `'readiness'` stage, `'readiness.runtime_failed'` | 2 |
| `PKG/src/http/discovery-log.ts` | create: `logDiscoveryFailed` | 2 |
| `PKG/src/http/routes.ts` | modify: `discoveryFailedResponse` calls the helper | 2 |
| `PKG/tests/http/discovery-log.test.ts` | create | 2 |
| `PKG/src/core/errors.ts`, `PKG/tests/core/errors.test.ts` | modify: `AuthConfigError` gets a `name` (Step 0) | 3 |
| `PKG/src/http/readiness.ts` | create: `readinessResponse` | 3 |
| `PKG/tests/http/readiness.test.ts` | create: T5–T13b and Review Focus 3–5 | 3 |
| `PKG/src/server.ts`, `PKG/tests/server.test.ts` | modify: the exports | 4 |
| `WT/ts/apps/iam-console/app/readyz/route.ts` | create | 5 |
| `WT/ts/apps/iam-console/proxy.ts`, `tests/unit/proxy.test.ts`, `tests/standalone-runtime.test.ts` | modify: D9, T14, T15 | 5 |
| `WT/ts/apps/gateway-console/app/readyz/route.ts` | create | 6 |
| `WT/ts/apps/gateway-console/proxy.ts`, `tests/unit/proxy.test.ts`, `tests/standalone-runtime.test.ts` | modify: D9, T14, T15 | 6 |
| `WT/ts/apps/iam-console/tests/e2e/harness.spec.ts` | modify: T16 | 7 |
| `WT/charts/paigasus/templates/console-deployment.yaml` + the two fixture copies under `WT/ci/helm-render/fixtures/` | modify: the probe path and comment | 8 |
| `WT/charts/paigasus/tests/golden/iam-only.yaml`, `iam-and-gateway.yaml` | regenerate | 8, 9 |
| `WT/ts/apps/{iam,gateway}-console/package.json`, `CHANGELOG.md`, `WT/charts/paigasus/values.yaml` | modify: `0.2.0` | 9 |
| `PKG/README.md`, `WT/docs/ops/RUNBOOK-containers.md`, `WT/docs/ops/RUNBOOK-chart.md`, `WT/ci/kind/README.md`, three older specs | modify: § 7 | 10 |

---

### Task 1: The adapter reports and starts discovery (T1–T4), and all five fakes compile

**Files:**
- Modify: `PKG/src/adapters/oidc.ts` (header line 12; interface lines 91-99; `createOidcClient` lines 299-337 and 449-466)
- Modify: `PKG/src/runtime.ts` (lines 19-22)
- Modify: `PKG/tests/fixtures/jwks.ts` (lines 85-91, 127, 132, 261-263)
- Modify: `PKG/tests/adapters/oidc.test.ts` (append after line 569)
- Modify: `PKG/tests/support/store-failure.ts` (line 21, lines 80-123)
- Modify: `PKG/tests/next/get-session.test.ts` (lines 61-64), `PKG/tests/http/logout.test.ts` (line 26, lines 101-105), `PKG/tests/http/callback.test.ts` (line 68), `PKG/tests/http/route-handler.test.ts` (line 67)

**Interfaces:**
- Produces, in `src/adapters/oidc.ts`:
  - `export type OidcDiscoveryStatus = 'idle' | 'discovering' | 'discovered';`
  - `OidcClient.discoveryStatus(): OidcDiscoveryStatus` (synchronous, no I/O)
  - `OidcClient.discover(): Promise<void>` (joins or starts discovery; rejects with `OidcDiscoveryFailed`)
- Produces, in `tests/fixtures/jwks.ts`: `OidcFixture.discoveryRequests(): number` (count of `GET /.well-known/openid-configuration`).
- Produces, in `tests/support/store-failure.ts`, on `FakeOidc`: `status: OidcDiscoveryStatus` (default `'discovered'`), `discoverCalls: number`, `resolveDiscover?: () => void`, `rejectDiscover?: (err: Error) => void`. Each `discover()` call returns a NEW pending promise and replaces the two settle functions.

- [ ] **Step 1: Record the baseline**

Run (from `WT`): `moon run paigasus-auth-ts:test paigasus-auth-ts:typecheck --force`
Expected: PASS. Write down the `Tests  N passed` count for `paigasus-auth-ts:test`. If this fails on the unchanged tree, STOP and report: the failure is not from this work.

- [ ] **Step 2: Add the fixture counter**

In `PKG/tests/fixtures/jwks.ts`, replace lines 85-91:

```ts
  /**
   * The body of every /token request this fixture received, in order (SMA-692). A test reads the
   * real `scope` and `audience` of a refresh request here. The bodies hold the client secret, so a
   * test must never print one.
   */
  tokenRequests(): readonly URLSearchParams[];
  close(): Promise<void>;
```

with:

```ts
  /**
   * The body of every /token request this fixture received, in order (SMA-692). A test reads the
   * real `scope` and `audience` of a refresh request here. The bodies hold the client secret, so a
   * test must never print one.
   */
  tokenRequests(): readonly URLSearchParams[];
  /**
   * The number of discovery requests (`GET /.well-known/openid-configuration`) this fixture
   * received (SMA-705). A test uses it to prove that two callers share one discovery.
   */
  discoveryRequests(): number;
  close(): Promise<void>;
```

Replace line 127 (`  const tokenRequestBodies: URLSearchParams[] = [];`) with:

```ts
  const tokenRequestBodies: URLSearchParams[] = [];
  let discoveryRequestCount = 0;
```

Replace the two lines

```ts
      if (req.method === 'GET' && url.pathname === '/.well-known/openid-configuration') {
        const body = {
```

with:

```ts
      if (req.method === 'GET' && url.pathname === '/.well-known/openid-configuration') {
        discoveryRequestCount += 1;
        const body = {
```

Replace

```ts
    tokenRequests(): readonly URLSearchParams[] {
      return [...tokenRequestBodies];
    },
```

with:

```ts
    tokenRequests(): readonly URLSearchParams[] {
      return [...tokenRequestBodies];
    },
    discoveryRequests(): number {
      return discoveryRequestCount;
    },
```

- [ ] **Step 3: Write the failing adapter tests (T1–T4)**

Append at the end of `PKG/tests/adapters/oidc.test.ts` (after line 569):

```ts

// SMA-705 T1-T4. The two members that the readiness route uses, against the real local fixture.
// The top-level beforeEach starts a fresh fixture for each row, so `discoveryRequests()` starts at 0.
describe('createOidcClient — discoveryStatus() and discover() (SMA-705 T1-T4)', () => {
  it('T1: idle, then discovering during discover(), then discovered', async () => {
    const oidc = makeClient();
    expect(oidc.discoveryStatus()).toBe('idle');
    const pending = oidc.discover();
    expect(oidc.discoveryStatus()).toBe('discovering');
    await pending;
    expect(oidc.discoveryStatus()).toBe('discovered');
    expect(fixture.discoveryRequests()).toBe(1);
  });

  it('T2: an unreachable issuer rejects with OidcDiscoveryFailed, and the status is idle again', async () => {
    const oidc = createOidcClient({
      issuer: 'http://127.0.0.1:1',
      clientId: 'test-client',
      clientSecret: 'test-secret',
      httpTimeoutMs: 2000,
      clockToleranceSeconds: 30,
      scopes: 'openid',
      allowInsecureRequests: true,
    });
    const pending = oidc.discover();
    expect(oidc.discoveryStatus()).toBe('discovering');
    const err: unknown = await pending.catch((e: unknown) => e);
    expect(isOidcDiscoveryFailed(err)).toBe(true);
    expect(oidc.discoveryStatus()).toBe('idle');
  });

  it('T3: discover() and buildAuthorizationUrl started together send ONE discovery request', async () => {
    const oidc = makeClient();
    await Promise.all([oidc.discover(), oidc.buildAuthorizationUrl({ redirectUri: REDIRECT_URI, state: STATE })]);
    expect(fixture.discoveryRequests()).toBe(1);
  });

  it('T4: after discovered, a second discover() sends no request and the status stays discovered', async () => {
    const oidc = makeClient();
    await oidc.discover();
    await oidc.discover();
    expect(fixture.discoveryRequests()).toBe(1);
    expect(oidc.discoveryStatus()).toBe('discovered');
  });
});
```

- [ ] **Step 4: Run the tests to see them fail**

Run: `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/packages/paigasus-auth exec vitest run tests/adapters/oidc.test.ts`
Expected: FAIL. T1–T4 fail with `TypeError: oidc.discoveryStatus is not a function` or `oidc.discover is not a function`. Every older row still passes.

- [ ] **Step 5: Implement the two members**

In `PKG/src/adapters/oidc.ts`, replace line 12:

```ts
// cleared, so the NEXT call retries rather than replaying the same rejection forever.
```

with:

```ts
// cleared, so the NEXT call retries rather than replaying the same rejection forever. SMA-705: the
// readiness route (http/readiness.ts) starts discovery through `discover()`, and it is the first
// caller in a normal process. `discoveryStatus()` reads the state with no I/O.
```

Replace lines 91-99 (the `OidcClient` interface):

```ts
export interface OidcClient {
  buildAuthorizationUrl(params: BuildAuthorizationUrlParams): Promise<AuthorizationRequest>;
  authorizationCodeGrant(params: AuthorizationCodeGrantParams): Promise<OidcTokens>;
  /** Matches core/single-flight.ts's `ResolveDeps.refresh` signature exactly. */
  refresh(refreshToken: string): Promise<RefreshedTokens>;
  /** Best-effort (design doc § 9.5) — callers decide whether a rejection blocks logout. */
  revoke(token: string): Promise<void>;
  buildEndSessionUrl(params: BuildEndSessionUrlParams): Promise<string>;
}
```

with:

```ts
/**
 * SMA-705. The state of one client's discovery. `discovered` is final: the adapter keeps the first
 * successful result for the life of the process. A failure goes back to `idle`.
 */
export type OidcDiscoveryStatus = 'idle' | 'discovering' | 'discovered';

export interface OidcClient {
  buildAuthorizationUrl(params: BuildAuthorizationUrlParams): Promise<AuthorizationRequest>;
  authorizationCodeGrant(params: AuthorizationCodeGrantParams): Promise<OidcTokens>;
  /** Matches core/single-flight.ts's `ResolveDeps.refresh` signature exactly. */
  refresh(refreshToken: string): Promise<RefreshedTokens>;
  /** Best-effort (design doc § 9.5) — callers decide whether a rejection blocks logout. */
  revoke(token: string): Promise<void>;
  buildEndSessionUrl(params: BuildEndSessionUrlParams): Promise<string>;
  /** SMA-705 D3. Synchronous, no I/O: the state of this client's discovery. */
  discoveryStatus(): OidcDiscoveryStatus;
  /**
   * SMA-705. Resolves when discovery has succeeded. Starts it when none is in flight; joins the one
   * in flight otherwise. Rejects with OidcDiscoveryFailed exactly as every other method does.
   */
  discover(): Promise<void>;
}
```

Replace line 301:

```ts
  let configPromise: Promise<client.Configuration> | undefined;
```

with:

```ts
  let configPromise: Promise<client.Configuration> | undefined;
  // SMA-705. True after the first successful discovery, for the life of this client. Only the
  // `.then` in getConfig sets it.
  let discovered = false;
```

Replace these lines in `getConfig`:

```ts
      .discovery(new URL(opts.issuer), opts.clientId, { client_secret: secret.reveal(), [client.clockTolerance]: opts.clockToleranceSeconds }, undefined, {
        timeout: opts.httpTimeoutMs / 1000,
        execute,
      })
      .catch((err: unknown) => {
```

with:

```ts
      .discovery(new URL(opts.issuer), opts.clientId, { client_secret: secret.reveal(), [client.clockTolerance]: opts.clockToleranceSeconds }, undefined, {
        timeout: opts.httpTimeoutMs / 1000,
        execute,
      })
      .then((config) => {
        // SMA-705. The flag is set inside the chain, so the status is 'discovered' before any
        // caller of this promise resumes. A failure skips this handler and never sets the flag.
        discovered = true;
        return config;
      })
      .catch((err: unknown) => {
```

Replace the end of the returned object:

```ts
        // Throws (synchronously) when the discovered server metadata has no end_session_endpoint.
        throw wrapError('build_end_session_url', err);
      }
    },
  };
}
```

with:

```ts
        // Throws (synchronously) when the discovered server metadata has no end_session_endpoint.
        throw wrapError('build_end_session_url', err);
      }
    },

    discoveryStatus(): OidcDiscoveryStatus {
      if (discovered) return 'discovered';
      return configPromise !== undefined ? 'discovering' : 'idle';
    },

    async discover(): Promise<void> {
      // Through getConfig(), never client.discovery directly. A probe, a login and a callback that
      // run at the same time then share one configPromise, and the IdP gets one request.
      await getConfig();
    },
  };
}
```

In `PKG/src/runtime.ts`, replace lines 19-22:

```ts
// DISCOVERY STAYS LAZY. createOidcClient (adapters/oidc.ts) performs no I/O by itself; the first
// login/refresh/logout triggers `openid-client`'s discovery call. That is what lets every
// cross-field-rule check below reject synchronously-fast, before any network call, even against
// an issuer URL that resolves to nothing (exactly what tests/runtime.test.ts's BASE fixture is).
```

with:

```ts
// DISCOVERY STAYS LAZY. createOidcClient (adapters/oidc.ts) performs no I/O by itself; the first
// login/refresh/logout triggers `openid-client`'s discovery call. That is what lets every
// cross-field-rule check below reject synchronously-fast, before any network call, even against
// an issuer URL that resolves to nothing (exactly what tests/runtime.test.ts's BASE fixture is).
// SMA-705: the readiness route (http/readiness.ts) starts discovery through `oidc.discover()`, and
// it is the first caller in a normal process.
```

- [ ] **Step 6: Run the adapter tests to see them pass**

Run: `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/packages/paigasus-auth exec vitest run tests/adapters/oidc.test.ts`
Expected: PASS, every row, including the SMA-656 retry row.

- [ ] **Step 7: Give the five fakes the two members**

`PKG/tests/support/store-failure.ts`: replace line 21:

```ts
import type { AuthorizationRequest, BuildEndSessionUrlParams, OidcClient, OidcTokens, RefreshedTokens } from '../../src/adapters/oidc.js';
```

with:

```ts
import type { AuthorizationRequest, BuildEndSessionUrlParams, OidcClient, OidcDiscoveryStatus, OidcTokens, RefreshedTokens } from '../../src/adapters/oidc.js';
```

Replace the `FakeOidc` interface (lines 80-90):

```ts
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
```

with:

```ts
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
  /** SMA-705: what `discoveryStatus()` answers. The default is 'discovered'. */
  status: OidcDiscoveryStatus;
  /** SMA-705: the number of `discover()` calls. */
  discoverCalls: number;
  /** SMA-705: resolves the promise of the latest `discover()` call. That call sets it. */
  resolveDiscover?: () => void;
  /** SMA-705: rejects the promise of the latest `discover()` call. That call sets it. */
  rejectDiscover?: (err: Error) => void;
}
```

Replace the header line of `fakeOidc` and the next three lines:

```ts
/** No network. Unless `codeGrantError` is set, the code exchange succeeds and returns NEW_REFRESH_TOKEN. */
export function fakeOidc(): FakeOidc {
  const oidc: FakeOidc = {
    revokeCalls: [],
    failRevoke: false,
    endSessionCalls: [],
```

with:

```ts
/**
 * No network. Unless `codeGrantError` is set, the code exchange succeeds and returns
 * NEW_REFRESH_TOKEN. SMA-705: each `discover()` call returns a pending promise. The test settles it
 * with `resolveDiscover` or `rejectDiscover`, so it controls when the detached promise settles.
 */
export function fakeOidc(): FakeOidc {
  const oidc: FakeOidc = {
    revokeCalls: [],
    failRevoke: false,
    endSessionCalls: [],
    status: 'discovered',
    discoverCalls: 0,
    discoveryStatus: (): OidcDiscoveryStatus => oidc.status,
    discover: (): Promise<void> => {
      oidc.discoverCalls += 1;
      return new Promise<void>((resolve, reject) => {
        oidc.resolveDiscover = () => resolve();
        oidc.rejectDiscover = (err: Error) => reject(err);
      });
    },
```

`PKG/tests/next/get-session.test.ts`: replace line 63:

```ts
  return { buildAuthorizationUrl: fail, authorizationCodeGrant: fail, refresh: fail, revoke: fail, buildEndSessionUrl: fail };
```

with:

```ts
  return { buildAuthorizationUrl: fail, authorizationCodeGrant: fail, refresh: fail, revoke: fail, buildEndSessionUrl: fail, discoveryStatus: () => 'discovered', discover: fail };
```

`PKG/tests/http/logout.test.ts`: replace line 26:

```ts
import type { AuthorizationRequest, BuildEndSessionUrlParams, OidcClient, OidcTokens, RefreshedTokens } from '../../src/adapters/oidc.js';
```

with:

```ts
import type { AuthorizationRequest, BuildEndSessionUrlParams, OidcClient, OidcDiscoveryStatus, OidcTokens, RefreshedTokens } from '../../src/adapters/oidc.js';
```

Replace lines 101-106:

```ts
    buildEndSessionUrl(params: BuildEndSessionUrlParams): Promise<string> {
      buildEndSessionUrlCalls.push(params);
      return Promise.resolve(opts.endSessionUrl ?? END_SESSION_URL);
    },
  };
}
```

with:

```ts
    buildEndSessionUrl(params: BuildEndSessionUrlParams): Promise<string> {
      buildEndSessionUrlCalls.push(params);
      return Promise.resolve(opts.endSessionUrl ?? END_SESSION_URL);
    },
    discoveryStatus(): OidcDiscoveryStatus {
      return 'discovered';
    },
    discover(): Promise<void> {
      throw new Error('not used in logout tests');
    },
  };
}
```

`PKG/tests/http/callback.test.ts`: replace line 68:

```ts
    buildEndSessionUrl: (params) => inner.buildEndSessionUrl(params),
```

with:

```ts
    buildEndSessionUrl: (params) => inner.buildEndSessionUrl(params),
    discoveryStatus: () => inner.discoveryStatus(),
    discover: () => inner.discover(),
```

`PKG/tests/http/route-handler.test.ts`: replace line 67:

```ts
      buildEndSessionUrl: (params) => inner.buildEndSessionUrl(params),
```

with:

```ts
      buildEndSessionUrl: (params) => inner.buildEndSessionUrl(params),
      discoveryStatus: () => inner.discoveryStatus(),
      discover: () => inner.discover(),
```

- [ ] **Step 8: Typecheck, lint, format, and run the package suite**

Run (from `WT`): `moon run paigasus-auth-ts:typecheck`
Expected: PASS. If `tsc` names another file that builds an `OidcClient`, give it the same two members and report it (the spec lists five).

Run: `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts exec prettier --write packages/paigasus-auth/src/adapters/oidc.ts packages/paigasus-auth/src/runtime.ts packages/paigasus-auth/tests/fixtures/jwks.ts packages/paigasus-auth/tests/adapters/oidc.test.ts packages/paigasus-auth/tests/support/store-failure.ts packages/paigasus-auth/tests/next/get-session.test.ts packages/paigasus-auth/tests/http/logout.test.ts packages/paigasus-auth/tests/http/callback.test.ts packages/paigasus-auth/tests/http/route-handler.test.ts`
Then the same path list with `exec eslint` instead of `exec prettier --write`.
Expected: no lint error. Do not disable a rule; fix the code by the rule's own advice.

Run (from `WT`): `moon run paigasus-auth-ts:test --force`
Expected: PASS, with the baseline count plus 4.

- [ ] **Step 9: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness branch --show-current
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness add ts/packages/paigasus-auth/src/adapters/oidc.ts ts/packages/paigasus-auth/src/runtime.ts ts/packages/paigasus-auth/tests/fixtures/jwks.ts ts/packages/paigasus-auth/tests/adapters/oidc.test.ts ts/packages/paigasus-auth/tests/support/store-failure.ts ts/packages/paigasus-auth/tests/next/get-session.test.ts ts/packages/paigasus-auth/tests/http/logout.test.ts ts/packages/paigasus-auth/tests/http/callback.test.ts ts/packages/paigasus-auth/tests/http/route-handler.test.ts
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness commit -m "feat(ts): report and start OIDC discovery from the adapter (SMA-705)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: One emitter for `oidc.discovery_failed`, with a `readiness` stage (D8)

**Files:**
- Modify: `PKG/src/ports/logger.ts` (lines 14-16, 18-32, 43-49)
- Create: `PKG/src/http/discovery-log.ts`
- Modify: `PKG/src/http/routes.ts` (line 42, after line 46, lines 130-139)
- Create: `PKG/tests/http/discovery-log.test.ts`

**Interfaces:**
- Consumes: `oidcDiscoveryReason(err: unknown): OidcDiscoveryFailureReason` (`src/core/errors.ts`); `harness`, `IDP_SENTINELS`, `SENTINEL_IDP_URL`, `expectEventsClean` (`tests/support/store-failure.ts`).
- Produces:
  - `export type OidcDiscoveryStage = 'login' | 'callback' | 'readiness';`
  - `AuthEventName` gains `'readiness.runtime_failed'`.
  - `export function logDiscoveryFailed(runtime: AuthRuntime, stage: OidcDiscoveryStage, err: unknown): void` in `src/http/discovery-log.ts`.

- [ ] **Step 1: Write the failing test**

Create `PKG/tests/http/discovery-log.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-705 D8. logDiscoveryFailed is the one emitter of `oidc.discovery_failed`. The login and the
// callback route call it (SMA-656, tests/http/discovery-failed.test.ts covers them through the
// routes), and the readiness route calls it with the stage 'readiness'.
import { describe, expect, it } from 'vitest';
import { OidcDiscoveryFailed } from '../../src/core/errors.js';
import { logDiscoveryFailed } from '../../src/http/discovery-log.js';
import { IDP_SENTINELS, SENTINEL_IDP_URL, expectEventsClean, harness } from '../support/store-failure.js';

/** No store call happens in these rows. */
const noStoreFailure = (): Error => new Error('no store call in the SMA-705 rows');

describe('logDiscoveryFailed (SMA-705 D8)', () => {
  it.each(['login', 'callback', 'readiness'] as const)('logs one event with the %s stage and the closed reason', (stage) => {
    const h = harness([], noStoreFailure);
    logDiscoveryFailed(h.runtime, stage, new OidcDiscoveryFailed(`oidc discovery failed at ${SENTINEL_IDP_URL}`, 'tls'));
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage, reason: 'tls' }]]);
    expectEventsClean(h.events, IDP_SENTINELS);
  });

  it('logs the reason other for an error that is not an OidcDiscoveryFailed', () => {
    const h = harness([], noStoreFailure);
    logDiscoveryFailed(h.runtime, 'readiness', new Error(`fetch failed at ${SENTINEL_IDP_URL}`));
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'readiness', reason: 'other' }]]);
    expectEventsClean(h.events, IDP_SENTINELS);
  });
});
```

- [ ] **Step 2: Run it to see it fail**

Run: `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/packages/paigasus-auth exec vitest run tests/http/discovery-log.test.ts`
Expected: FAIL at collection: `Failed to load url ../../src/http/discovery-log.js` (the module does not exist).

- [ ] **Step 3: Widen the logger port**

In `PKG/src/ports/logger.ts`, replace lines 14-16:

```ts
// `session.refresh_failed` may carry `oauthError` (SMA-692 D10). Its value is a code of the RFC
// 6749 § 5.2 list or 'other', never the IdP's raw string: core/errors.ts's toTokenErrorCode maps
// it. This type admits any string key, so that function is the control, not this port.
```

with:

```ts
// `session.refresh_failed` may carry `oauthError` (SMA-692 D10). Its value is a code of the RFC
// 6749 § 5.2 list or 'other', never the IdP's raw string: core/errors.ts's toTokenErrorCode maps
// it. This type admits any string key, so that function is the control, not this port.
//
// `readiness.runtime_failed` carries `error`, the caught error's `name` only (SMA-705 D10). Never
// its message or its `input`: node-redis parses the Redis URL with `new URL()`, and that TypeError
// holds the URL, password included.
```

Replace line 32:

```ts
  | 'oidc.discovery_failed';
```

with:

```ts
  | 'oidc.discovery_failed'
  | 'readiness.runtime_failed';
```

Replace lines 43-49:

```ts
/**
 * The closed set of `stage` values for `oidc.discovery_failed` (SMA-656 D7): the route that needed
 * the discovered configuration. `AuthEventFields` is a free record, so the one emitter,
 * http/routes.ts's `discoveryFailedResponse`, takes this type as a typed parameter. A later refresh
 * or logout stage is a new member here, not a new event name.
 */
export type OidcDiscoveryStage = 'login' | 'callback';
```

with:

```ts
/**
 * The closed set of `stage` values for `oidc.discovery_failed` (SMA-656 D7, SMA-705 D8): the route
 * that needed the discovered configuration. `AuthEventFields` is a free record, so the one emitter,
 * http/discovery-log.ts's `logDiscoveryFailed`, takes this type as a typed parameter. A later
 * refresh or logout stage is a new member here, not a new event name.
 */
export type OidcDiscoveryStage = 'login' | 'callback' | 'readiness';
```

- [ ] **Step 4: Create the helper**

Create `PKG/src/http/discovery-log.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The one emitter of `oidc.discovery_failed` (SMA-656 D7, SMA-705 D8). http/routes.ts calls it for
// the login and the callback stage. http/readiness.ts calls it for the readiness stage.
// `AuthEventFields` is a free record, so the typed `stage` parameter is what checks the literal.
//
// The event holds the zone, the stage and the closed `reason`. It never holds the caught error, its
// message, its name or a URL (ports/logger.ts's redaction contract). `oidcDiscoveryReason` maps any
// value outside the closed list to 'other'.
import { oidcDiscoveryReason } from '../core/errors';
import type { OidcDiscoveryStage } from '../ports/logger';
import type { AuthRuntime } from '../runtime';

export function logDiscoveryFailed(runtime: AuthRuntime, stage: OidcDiscoveryStage, err: unknown): void {
  runtime.logger.event('oidc.discovery_failed', { zone: runtime.zone, stage, reason: oidcDiscoveryReason(err) });
}
```

- [ ] **Step 5: Route the SMA-656 emitter through the helper**

In `PKG/src/http/routes.ts`, replace line 42:

```ts
import { CallbackRejected, isOidcDiscoveryFailed, oidcDiscoveryReason } from '../core/errors';
```

with:

```ts
import { CallbackRejected, isOidcDiscoveryFailed } from '../core/errors';
```

Replace line 46:

```ts
import { SESSION_COOKIE, TXN_COOKIE_PREFIX, clearCookie, readCookies, serializeCookie, txnCookieName } from './cookies';
```

with:

```ts
import { SESSION_COOKIE, TXN_COOKIE_PREFIX, clearCookie, readCookies, serializeCookie, txnCookieName } from './cookies';
import { logDiscoveryFailed } from './discovery-log';
```

Replace lines 130-139:

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
```

with:

```ts
/**
 * The 503 for an OIDC discovery failure (SMA-656 D4, D7), shared by the login and the callback
 * route. Logs one `oidc.discovery_failed` event through http/discovery-log.ts's
 * `logDiscoveryFailed`, the one emitter of that event (SMA-705 D8): the zone, the typed stage and
 * the closed `reason`, never the caught error, its message, its name or a URL (ports/logger.ts's
 * redaction contract; A2).
 */
function discoveryFailedResponse(runtime: AuthRuntime, stage: OidcDiscoveryStage, err: unknown, href: string): Response {
  logDiscoveryFailed(runtime, stage, err);
  return storeUnavailableResponse({ kind: 'link', href, service: 'identity_provider' });
}
```

- [ ] **Step 6: Run the new test and the unchanged SMA-656 rows**

Run: `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/packages/paigasus-auth exec vitest run tests/http`
Expected: PASS, every file, including `discovery-failed.test.ts`. That file must pass with NO edit.

Run: `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness diff --stat 306fe882 -- ts/packages/paigasus-auth/tests/http/discovery-failed.test.ts`
Expected: no output (the SMA-656 test file is unchanged since the branch base).

- [ ] **Step 7: Typecheck, lint, format**

Run (from `WT`): `moon run paigasus-auth-ts:typecheck paigasus-console-core-ts:typecheck`
Expected: PASS. `@paigasus/console-core` imports `AuthEventName`; its typecheck proves that the wider union is accepted.

Run prettier `--write`, then eslint, over: `packages/paigasus-auth/src/ports/logger.ts packages/paigasus-auth/src/http/discovery-log.ts packages/paigasus-auth/src/http/routes.ts packages/paigasus-auth/tests/http/discovery-log.test.ts` (commands as in Task 1 Step 8).
Expected: no lint error.

- [ ] **Step 8: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness branch --show-current
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness add ts/packages/paigasus-auth/src/ports/logger.ts ts/packages/paigasus-auth/src/http/discovery-log.ts ts/packages/paigasus-auth/src/http/routes.ts ts/packages/paigasus-auth/tests/http/discovery-log.test.ts
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness commit -m "refactor(ts): log oidc.discovery_failed through one helper with a readiness stage (SMA-705)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: `readinessResponse` (T5–T13b and Review Focus 3–5)

**Files:**
- Modify: `PKG/src/core/errors.ts` (`AuthConfigError` gets a `name`, Step 0)
- Modify: `PKG/tests/core/errors.test.ts` (Step 0)
- Create: `PKG/src/http/readiness.ts`
- Create: `PKG/tests/http/readiness.test.ts`

**Interfaces:**
- Consumes: `logDiscoveryFailed` (Task 2); `OidcClient.discoveryStatus()`, `OidcClient.discover()`, `FakeOidc.status|discoverCalls|resolveDiscover|rejectDiscover`, `OidcFixture.discoveryRequests()` (Task 1); `createAuthRuntime(cfg, deps)` with `deps.oidcClientFactory` and `deps.logger` (`src/runtime.ts`); `createAuthRoutes` (`src/http/routes.ts`).
- Produces: `export async function readinessResponse(getRuntime: () => Promise<AuthRuntime>, logger: AuthLogger): Promise<Response>` in `src/http/readiness.ts`. Answers exactly 200 `{"status":"ready"}` or 503 `{"status":"unready"}`, always with `cache-control: no-store`.

- [ ] **Step 0: `AuthConfigError` gets a `name` (plan deviation 3)**

First add the failing test. In `PKG/tests/core/errors.test.ts`, add `AuthConfigError` to the import list from `'../../src/core/errors.js'` (keep the list sorted), and add this block at the end of the file:

```ts
describe('AuthConfigError (SMA-705 deviation 3)', () => {
  // readiness.runtime_failed logs only an error's `name`. A production bundle can mangle
  // `constructor.name`, so the class sets its name explicitly, as SessionStoreTimeout does.
  it('has the name AuthConfigError and keeps its code and message', () => {
    const err = new AuthConfigError('PAIGASUS_ZONE has no entry in PAIGASUS_ZONES');
    expect(err.name).toBe('AuthConfigError');
    expect(err.code).toBe('auth_config_invalid');
    expect(err.message).toBe('PAIGASUS_ZONE has no entry in PAIGASUS_ZONES');
    expect(err).toBeInstanceOf(AuthError);
  });
});
```

Run: `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/packages/paigasus-auth exec vitest run tests/core/errors.test.ts`
Expected: FAIL, `expected 'Error' to be 'AuthConfigError'`.

Then, in `PKG/src/core/errors.ts`, replace

```ts
export class AuthConfigError extends AuthError {
  readonly code = 'auth_config_invalid';
}
```

with

```ts
export class AuthConfigError extends AuthError {
  readonly code = 'auth_config_invalid';

  constructor(message: string) {
    super(message);
    // SMA-705: readiness.runtime_failed logs only the name. Set it explicitly, because a
    // production bundle can mangle `constructor.name` (the SessionStoreTimeout reason).
    this.name = 'AuthConfigError';
  }
}
```

Run the same command. Expected: PASS. Then run `moon run paigasus-auth-ts:test` and `moon run paigasus-auth-ts:typecheck`. Expected: PASS. If an existing test fails because it expected the name `Error` or the text `Error: `, STOP and report it. Do not edit that test.

Commit: `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness add ts/packages/paigasus-auth/src/core/errors.ts ts/packages/paigasus-auth/tests/core/errors.test.ts`, then `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness commit -m "fix(ts): give AuthConfigError its own name (SMA-705)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"`.

- [ ] **Step 1: Write the failing tests**

Create `PKG/tests/http/readiness.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-705: readinessResponse, the handler behind each console's `<basePath>/readyz` (spec D2, D3,
// D7, D8, D10, D11; T5-T13b). T5-T12 use the network-free FakeOidc of tests/support/store-failure.ts.
// T13a, T13b and the D8 overlap row use the REAL adapter through createAuthRuntime.
//
// A detached promise settles after the handler returns. A row that checks a log or an unhandled
// rejection settles the fake's promise, then awaits one setImmediate: Node emits
// `unhandledRejection` only after the microtask queue drains. A listener records every unhandled
// rejection, and each such row asserts that the list is empty.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { createOidcClient } from '../../src/adapters/oidc.js';
import { OidcDiscoveryFailed } from '../../src/core/errors.js';
import { readinessResponse } from '../../src/http/readiness.js';
import { createAuthRoutes } from '../../src/http/routes.js';
import type { AuthEventFields, AuthEventName, AuthLogger } from '../../src/ports/logger.js';
import { createAuthRuntime, type AuthRuntime } from '../../src/runtime.js';
import { startOidcFixture, type OidcFixture } from '../fixtures/jwks.js';
import { IDP_SENTINELS, SENTINEL_IDP_URL, expectEventsClean, harness, type Harness } from '../support/store-failure.js';

type Events = Array<[AuthEventName, AuthEventFields]>;

/** The Redis URL of D10's measured leak. No body, header or logged field may hold its password. */
const REDIS_SENTINEL = 'redis://u:sentinel-705@h';
const RUNTIME_SENTINELS: readonly string[] = ['sentinel-705'];

/** Port 1 is on the Fetch "bad port" list, so discovery fails at once with no connect (SMA-656). */
const UNREACHABLE_ISSUER = 'http://127.0.0.1:1';

/** A valid composed configuration: tests/runtime.test.ts's BASE, one zone and the memory store. */
const BASE_CONFIG = {
  PAIGASUS_ZONE: 'iam',
  PAIGASUS_ZONES: { iam: '/iam' },
  PAIGASUS_OIDC_ISSUER: 'https://idp.example.com',
  PAIGASUS_OIDC_CLIENT_ID: 'c',
  PAIGASUS_OIDC_CLIENT_SECRET: 's',
  PAIGASUS_PUBLIC_ORIGIN: 'https://app.example.com',
  PAIGASUS_OIDC_SCOPES: 'openid profile email offline_access',
  PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS: 30,
  PAIGASUS_OIDC_HTTP_TIMEOUT_MS: 3500,
  PAIGASUS_SESSION_STORE: 'memory' as const,
  PAIGASUS_SESSION_REDIS_TIMEOUT_MS: 1000,
  PAIGASUS_SESSION_TTL_SECONDS: 28800,
  PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS: 86400,
  PAIGASUS_SESSION_REFRESH_SKEW_SECONDS: 30,
  PAIGASUS_SESSION_LOCK_TTL_MS: 10000,
  PAIGASUS_SESSION_LOCK_WAIT_MS: 3000,
};

/** No store call happens in these rows. */
const noStoreFailure = (): Error => new Error('no store call in the SMA-705 rows');

let unhandled: unknown[] = [];
const onUnhandled = (reason: unknown): void => {
  unhandled.push(reason);
};

beforeEach(() => {
  unhandled = [];
  process.on('unhandledRejection', onUnhandled);
});

afterEach(() => {
  process.off('unhandledRejection', onUnhandled);
});

function recordingLogger(events: Events): AuthLogger {
  return { event: (name, fields) => void events.push([name, { ...fields }]) };
}

function getterFor(h: Harness): () => Promise<AuthRuntime> {
  return () => Promise.resolve(h.runtime);
}

/** Lets every settled promise run its handlers: all microtasks, then one macrotask turn. */
function settle(): Promise<void> {
  return new Promise((resolve) => setImmediate(resolve));
}

/** The promise's value, or 'timed_out' after `ms`. */
async function within<T>(promise: Promise<T>, ms: number): Promise<T | 'timed_out'> {
  let timer: NodeJS.Timeout | undefined;
  const timeout = new Promise<'timed_out'>((resolve) => {
    timer = setTimeout(() => resolve('timed_out'), ms);
  });
  try {
    return await Promise.race([promise, timeout]);
  } finally {
    clearTimeout(timer);
  }
}

/** D7: the exact body, `cache-control: no-store`, and no `forbidden` string in the body or a header. */
async function expectProbe(res: Response, code: 200 | 503, status: 'ready' | 'unready', forbidden: readonly string[] = []): Promise<void> {
  expect(res.status).toBe(code);
  expect(res.headers.get('cache-control')).toBe('no-store');
  const body = await res.text();
  expect(body).toBe(JSON.stringify({ status }));
  for (const text of [body, ...[...res.headers].map(([name, value]) => `${name}: ${value}`)]) {
    for (const sentinel of forbidden) expect(text).not.toContain(sentinel);
  }
}

/** D10's measured shape: node-redis's `new URL()` TypeError holds the URL in its message and `input`. */
function redisUrlError(): TypeError {
  return Object.assign(new TypeError(`Invalid URL: ${REDIS_SENTINEL}`), { code: 'ERR_INVALID_URL', input: REDIS_SENTINEL });
}

describe('readinessResponse — the discovery states (SMA-705 D2, D3, D7)', () => {
  it('T5: discovered -> 200 ready, no discover() call, no event', async () => {
    const h = harness([], noStoreFailure);
    h.oidc.status = 'discovered';
    await expectProbe(await readinessResponse(getterFor(h), h.runtime.logger), 200, 'ready');
    expect(h.oidc.discoverCalls).toBe(0);
    expect(h.events).toEqual([]);
  });

  it('T6: discovering -> 503 unready, no discover() call', async () => {
    const h = harness([], noStoreFailure);
    h.oidc.status = 'discovering';
    await expectProbe(await readinessResponse(getterFor(h), h.runtime.logger), 503, 'unready');
    expect(h.oidc.discoverCalls).toBe(0);
    expect(h.events).toEqual([]);
  });

  it('T7: idle -> 503 unready at once, with exactly one discover() call that is still pending', async () => {
    const h = harness([], noStoreFailure);
    h.oidc.status = 'idle';
    const res = await within(readinessResponse(getterFor(h), h.runtime.logger), 1_000);
    expect(res, 'the handler waited for discovery (D3)').not.toBe('timed_out');
    await expectProbe(res as Response, 503, 'unready');
    expect(h.oidc.discoverCalls).toBe(1);
    h.oidc.resolveDiscover?.();
    await settle();
  });

  it('T10: idle, and discover() resolves -> no event', async () => {
    const h = harness([], noStoreFailure);
    h.oidc.status = 'idle';
    await expectProbe(await readinessResponse(getterFor(h), h.runtime.logger), 503, 'unready');
    h.oidc.resolveDiscover?.();
    await settle();
    expect(h.events).toEqual([]);
    expect(unhandled).toEqual([]);
  });
});

describe('readinessResponse — a discovery failure it started (SMA-705 D8, D11)', () => {
  it('T8: OidcDiscoveryFailed with a URL in its message -> one readiness event, nothing leaks', async () => {
    const h = harness([], noStoreFailure);
    h.oidc.status = 'idle';
    const res = await readinessResponse(getterFor(h), h.runtime.logger);
    h.oidc.rejectDiscover?.(new OidcDiscoveryFailed(`oidc discovery failed at ${SENTINEL_IDP_URL}`, 'dns'));
    await settle();
    await expectProbe(res, 503, 'unready', IDP_SENTINELS);
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'readiness', reason: 'dns' }]]);
    expectEventsClean(h.events, IDP_SENTINELS);
    expect(unhandled).toEqual([]);
  });

  it('T9: an OidcDiscoveryFailed from a second module copy -> the event has its reason', async () => {
    vi.resetModules();
    const foreign = await import('../../src/core/errors.js');
    expect(foreign.OidcDiscoveryFailed).not.toBe(OidcDiscoveryFailed);
    const h = harness([], noStoreFailure);
    h.oidc.status = 'idle';
    await expectProbe(await readinessResponse(getterFor(h), h.runtime.logger), 503, 'unready');
    h.oidc.rejectDiscover?.(new foreign.OidcDiscoveryFailed(`oidc discovery failed at ${SENTINEL_IDP_URL}`, 'tls'));
    await settle();
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'readiness', reason: 'tls' }]]);
    expectEventsClean(h.events, IDP_SENTINELS);
    expect(unhandled).toEqual([]);
  });

  it('T9: a plain Error -> one event with the reason other', async () => {
    const h = harness([], noStoreFailure);
    h.oidc.status = 'idle';
    await expectProbe(await readinessResponse(getterFor(h), h.runtime.logger), 503, 'unready');
    h.oidc.rejectDiscover?.(new Error(`fetch failed at ${SENTINEL_IDP_URL}`));
    await settle();
    expect(h.events).toEqual([['oidc.discovery_failed', { zone: 'iam', stage: 'readiness', reason: 'other' }]]);
    expectEventsClean(h.events, IDP_SENTINELS);
    expect(unhandled).toEqual([]);
  });

  it('T12: a logger that throws inside the discovery catch -> no unhandled rejection', async () => {
    const h = harness([], noStoreFailure);
    h.runtime.logger = {
      event: () => {
        throw new Error('the logger failed');
      },
    };
    h.oidc.status = 'idle';
    const res = await readinessResponse(getterFor(h), { event: () => undefined });
    h.oidc.rejectDiscover?.(new OidcDiscoveryFailed('oidc discovery failed: TypeError', 'network'));
    await settle();
    expect(unhandled).toEqual([]);
    await expectProbe(res, 503, 'unready');
  });
});

describe('readinessResponse — the runtime build fails (SMA-705 D10)', () => {
  it.each([
    ['rejects', (): Promise<AuthRuntime> => Promise.reject(redisUrlError())],
    [
      'throws synchronously',
      (): Promise<AuthRuntime> => {
        throw redisUrlError();
      },
    ],
  ])('T11: a runtime getter that %s -> 503 unready and one readiness.runtime_failed with the name only', async (_label, getter) => {
    const events: Events = [];
    await expectProbe(await readinessResponse(getter, recordingLogger(events)), 503, 'unready', RUNTIME_SENTINELS);
    expect(events).toEqual([['readiness.runtime_failed', { error: 'TypeError' }]]);
    expectEventsClean(events, RUNTIME_SENTINELS);
  });

  // Review Focus 4. getAuthRuntime clears its slot on a failure, so the next probe builds again.
  // The handler must keep no state of its own.
  it('T11b: a getter that fails once, then works -> the second call answers 200', async () => {
    const h = harness([], noStoreFailure);
    h.oidc.status = 'discovered';
    const events: Events = [];
    let calls = 0;
    const getter = (): Promise<AuthRuntime> => {
      calls += 1;
      return calls === 1 ? Promise.reject(redisUrlError()) : Promise.resolve(h.runtime);
    };
    await expectProbe(await readinessResponse(getter, recordingLogger(events)), 503, 'unready', RUNTIME_SENTINELS);
    await expectProbe(await readinessResponse(getter, recordingLogger(events)), 200, 'ready');
    expect(calls).toBe(2);
    expect(events).toEqual([['readiness.runtime_failed', { error: 'TypeError' }]]);
  });

  // Review Focus 5. AuthConfigError sets its own `name` (plan deviation 3, Step 0).
  it('a real cross-field refusal from createAuthRuntime -> 503 unready, error AuthConfigError', async () => {
    const events: Events = [];
    const getter = (): Promise<AuthRuntime> => createAuthRuntime({ ...BASE_CONFIG, PAIGASUS_ZONES: { iam: '/iam', gateway: '/gateway' } });
    await expectProbe(await readinessResponse(getter, recordingLogger(events)), 503, 'unready');
    expect(events).toEqual([['readiness.runtime_failed', { error: 'AuthConfigError' }]]);
  });
});

describe('readinessResponse through the real adapter (SMA-705 T13a, T13b, D8)', () => {
  let fixture: OidcFixture;

  beforeEach(async () => {
    fixture = await startOidcFixture();
  });

  afterEach(async () => {
    await fixture.close();
  });

  function realRuntime(issuer: string, events: Events): Promise<AuthRuntime> {
    return createAuthRuntime(
      { ...BASE_CONFIG, PAIGASUS_OIDC_ISSUER: issuer },
      {
        logger: recordingLogger(events),
        // The fixture is plain http on localhost. createAuthRuntime does not pass
        // allowInsecureRequests, so the factory adds it. Never in production.
        oidcClientFactory: (o) => createOidcClient({ ...o, allowInsecureRequests: true }),
      },
    );
  }

  it('T13a: a healthy IdP -> 503 first, then 200 after the attempt settles, with one discovery request', async () => {
    const events: Events = [];
    const runtime = await realRuntime(fixture.issuer, events);
    const getter = (): Promise<AuthRuntime> => Promise.resolve(runtime);
    const first = await readinessResponse(getter, runtime.logger);
    await runtime.oidc.discover();
    await settle();
    await expectProbe(first, 503, 'unready');
    await expectProbe(await readinessResponse(getter, runtime.logger), 200, 'ready');
    expect(fixture.discoveryRequests()).toBe(1);
    expect(events).toEqual([]);
  });

  it('T13b: an unreachable IdP -> 503 on every call, and one readiness event per settled attempt', async () => {
    const events: Events = [];
    const runtime = await realRuntime(UNREACHABLE_ISSUER, events);
    const getter = (): Promise<AuthRuntime> => Promise.resolve(runtime);
    for (let attempt = 1; attempt <= 3; attempt += 1) {
      const res = await readinessResponse(getter, runtime.logger);
      await runtime.oidc.discover().catch(() => undefined);
      await settle();
      await expectProbe(res, 503, 'unready');
      expect(events).toHaveLength(attempt);
    }
    expect(events).toEqual([
      ['oidc.discovery_failed', { zone: 'iam', stage: 'readiness', reason: 'network' }],
      ['oidc.discovery_failed', { zone: 'iam', stage: 'readiness', reason: 'network' }],
      ['oidc.discovery_failed', { zone: 'iam', stage: 'readiness', reason: 'network' }],
    ]);
    expect(unhandled).toEqual([]);
  });

  // Review Focus 3. D8: each caller that needed a configuration logs its own event.
  it('D8: a login next to a failing readiness attempt -> two 503s and one event for each stage', async () => {
    const events: Events = [];
    const runtime = await realRuntime(UNREACHABLE_ISSUER, events);
    const probe = await readinessResponse(() => Promise.resolve(runtime), runtime.logger);
    const login = await createAuthRoutes(runtime).handle(new Request('https://app.example.com/iam/auth/login'));
    await settle();
    await expectProbe(probe, 503, 'unready');
    expect(login.status).toBe(503);
    expect(events.map(([name, fields]) => `${name}:${String(fields['stage'])}:${String(fields['reason'])}`).sort()).toEqual([
      'oidc.discovery_failed:login:network',
      'oidc.discovery_failed:readiness:network',
    ]);
    expect(unhandled).toEqual([]);
  });
});
```

- [ ] **Step 2: Run the tests to see them fail**

Run: `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/packages/paigasus-auth exec vitest run tests/http/readiness.test.ts`
Expected: FAIL at collection: `Failed to load url ../../src/http/readiness.js`.

- [ ] **Step 3: Implement the handler**

Create `PKG/src/http/readiness.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The readiness probe of a console zone (SMA-705): `GET <basePath>/readyz`. Each app mounts it with
// a route file of three lines (D5). The chart's readinessProbe calls it.
//
// THE STATES (D7). 200 {"status":"ready"} when the auth runtime exists and its OIDC client has
// discovered the IdP. 503 {"status":"unready"} in every other case. Both carry
// `cache-control: no-store`. No other status or body occurs. The body never says why: the route is
// public, so an anonymous caller gets no reason. The operator reads the log.
//
// READY IS STICKY (D2). The adapter keeps its first successful discovery for the life of the
// process and never discovers again, so this route stays ready after that. An IdP outage after
// the first success affects every pod. Removing every pod would also remove the pages of signed-in
// users, and it would not make a sign-in work.
//
// THE ROUTE NEVER WAITS FOR DISCOVERY (D3). It reads a synchronous status. When discovery is idle,
// it starts one without `await` and answers at once. The next probe sees the result. One wait
// stays: the first call of a process builds the runtime, and that includes the Redis connect.
//
// THE LOG (D8, D10). A discovery attempt that this route started and that fails logs one
// `oidc.discovery_failed { zone, stage: 'readiness', reason }` through logDiscoveryFailed. A runtime
// build that fails logs `readiness.runtime_failed { error }`, where `error` is the error's `name`
// only. It never logs the error object, its message or its `input`: node-redis parses the Redis URL
// with `new URL()`, and that TypeError holds the URL, password included (measured, spec D10).
//
// NO THROW OUT OF A DETACHED PROMISE (D11). The `.catch` of the started attempt wraps its log call
// in try/catch. A logger that throws would otherwise make a new unhandled rejection.
import type { AuthLogger } from '../ports/logger';
import type { AuthRuntime } from '../runtime';
import { logDiscoveryFailed } from './discovery-log';

/** The two probe bodies (D7). */
type ReadinessStatus = 'ready' | 'unready';

function probeResponse(code: 200 | 503, status: ReadinessStatus): Response {
  return Response.json({ status }, { status: code, headers: { 'cache-control': 'no-store' } });
}

/**
 * The error's `name`, or 'unknown_error'. This is the rule of adapters/oidc.ts's `libraryErrorName`.
 * That helper is private to the adapter and names library errors, so this file has its own copy.
 */
function errorName(err: unknown): string {
  return err instanceof Error ? err.name : 'unknown_error';
}

/**
 * The readiness answer (SMA-705). `getRuntime` is the app's runtime getter (for example
 * `authRuntime`). It can throw synchronously or reject. `logger` is for the runtime-failure path
 * only, where no runtime exists: pass the logger that the app gives getAuthRuntime.
 */
export async function readinessResponse(getRuntime: () => Promise<AuthRuntime>, logger: AuthLogger): Promise<Response> {
  let runtime: AuthRuntime;
  try {
    // The call is inside the try, so a synchronous throw from the getter lands here too.
    runtime = await getRuntime();
  } catch (err) {
    logger.event('readiness.runtime_failed', { error: errorName(err) });
    return probeResponse(503, 'unready');
  }
  const status = runtime.oidc.discoveryStatus();
  if (status === 'discovered') return probeResponse(200, 'ready');
  if (status === 'idle') {
    // Not awaited (D3). The `.catch` logs every rejection and never rethrows. A rejection that is
    // not an OidcDiscoveryFailed gets the reason 'other' from oidcDiscoveryReason (D8).
    runtime.oidc.discover().catch((err: unknown) => {
      try {
        logDiscoveryFailed(runtime, 'readiness', err);
      } catch {
        // D11. A logger that throws must not make an unhandled rejection. Nothing can log it.
      }
    });
  }
  return probeResponse(503, 'unready');
}
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/packages/paigasus-auth exec vitest run tests/http/readiness.test.ts`
Expected: PASS, every row. **If T13b or the D8 row fails only because `reason` is not `network`, STOP.** Report the measured reason. Do not edit the test or the adapter to make it pass. **If the `AuthConfigError` row logs a value other than `AuthConfigError`, STOP** and report the value.

- [ ] **Step 5: Typecheck, lint, format, suite**

Run (from `WT`): `moon run paigasus-auth-ts:typecheck`
Expected: PASS.

Run prettier `--write`, then eslint, over `packages/paigasus-auth/src/http/readiness.ts packages/paigasus-auth/tests/http/readiness.test.ts`.
Expected: no lint error. If `@typescript-eslint/no-floating-promises` reports the `.catch` chain, report it; the rule accepts a `.catch` with a handler, so a report means a code change broke the chain.

Run (from `WT`): `moon run paigasus-auth-ts:test --force`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness branch --show-current
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness add ts/packages/paigasus-auth/src/http/readiness.ts ts/packages/paigasus-auth/tests/http/readiness.test.ts
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness commit -m "feat(ts): add readinessResponse for the console readiness probe (SMA-705)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: Export `readinessResponse` and `OidcDiscoveryStatus` from `@paigasus/auth/server`

**Files:**
- Modify: `PKG/src/server.ts` (lines 11-17, after line 34, lines 164-173)
- Modify: `PKG/tests/server.test.ts` (line 24; append at the end)

**Interfaces:**
- Consumes: `readinessResponse` (Task 3), `OidcDiscoveryStatus` (Task 1).
- Produces: `import { readinessResponse, type OidcDiscoveryStatus } from '@paigasus/auth/server'` works for the apps (Tasks 5, 6).

- [ ] **Step 1: Write the failing test**

In `PKG/tests/server.test.ts`, replace line 24:

```ts
import { createAuthRouteHandler } from '../src/server.js';
```

with:

```ts
import { createAuthRouteHandler, readinessResponse, type OidcDiscoveryStatus } from '../src/server.js';
import { readinessResponse as readinessFromModule } from '../src/http/readiness.js';
```

Also replace line 9, `import { describe, expect, it, vi } from 'vitest';`, with `import { describe, expect, expectTypeOf, it, vi } from 'vitest';`.

Append at the end of the file:

```ts

// SMA-705 D5. The apps import the readiness handler and the status type from the server entry.
describe('the readiness exports (SMA-705)', () => {
  it('re-exports readinessResponse from http/readiness.ts', () => {
    expect(readinessResponse).toBe(readinessFromModule);
  });

  it('exports the OidcDiscoveryStatus type with its three states', () => {
    // A type-level check: `moon run paigasus-auth-ts:typecheck` fails if the type is not exported
    // or if its union changes.
    expectTypeOf<OidcDiscoveryStatus>().toEqualTypeOf<'idle' | 'discovering' | 'discovered'>();
  });
});
```

- [ ] **Step 2: Run it to see it fail**

Run: `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/packages/paigasus-auth exec vitest run tests/server.test.ts`
Expected: FAIL in `re-exports readinessResponse`: `expected undefined to be [Function readinessResponse]`. The other rows pass.

- [ ] **Step 3: Add the exports**

In `PKG/src/server.ts`, replace lines 16-17:

```ts
// `authEnvShape` (the env shape an app composes into `@paigasus/next-config`'s
// `defineRuntimeConfig`), and every port and adapter.
```

with:

```ts
// `authEnvShape` (the env shape an app composes into `@paigasus/next-config`'s
// `defineRuntimeConfig`), `readinessResponse` (the console readiness probe, SMA-705), and every
// port and adapter.
```

Replace line 34:

```ts
export { SESSION_COOKIE } from './http/cookies';
```

with:

```ts
export { SESSION_COOKIE } from './http/cookies';

/** The console readiness probe, `<basePath>/readyz` (SMA-705). See http/readiness.ts. */
export { readinessResponse } from './http/readiness';
```

Replace lines 164-173:

```ts
export type {
  AuthorizationCodeGrantParams,
  AuthorizationRequest,
  BuildAuthorizationUrlParams,
  BuildEndSessionUrlParams,
  CreateOidcClientOptions,
  OidcClient,
  OidcTokens,
  RefreshedTokens as OidcRefreshedTokens,
} from './adapters/oidc';
```

with:

```ts
export type {
  AuthorizationCodeGrantParams,
  AuthorizationRequest,
  BuildAuthorizationUrlParams,
  BuildEndSessionUrlParams,
  CreateOidcClientOptions,
  OidcClient,
  OidcDiscoveryStatus,
  OidcTokens,
  RefreshedTokens as OidcRefreshedTokens,
} from './adapters/oidc';
```

- [ ] **Step 4: Run it to see it pass, then typecheck, lint, format**

Run: `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/packages/paigasus-auth exec vitest run tests/server.test.ts tests/structure`
Expected: PASS (the structure tests prove the import graph and the exports map did not change).

Run (from `WT`): `moon run paigasus-auth-ts:typecheck paigasus-console-core-ts:typecheck iam-console-ts:typecheck gateway-console-ts:typecheck`
Expected: PASS.

Run prettier `--write`, then eslint, over `packages/paigasus-auth/src/server.ts packages/paigasus-auth/tests/server.test.ts`.
Expected: no lint error.

- [ ] **Step 5: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness branch --show-current
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness add ts/packages/paigasus-auth/src/server.ts ts/packages/paigasus-auth/tests/server.test.ts
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness commit -m "feat(ts): export readinessResponse from @paigasus/auth/server (SMA-705)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: The IAM console serves `/iam/readyz` (D9, T14, T15)

**Files:**
- Create: `WT/ts/apps/iam-console/app/readyz/route.ts`
- Modify: `WT/ts/apps/iam-console/proxy.ts` (lines 27-30)
- Modify: `WT/ts/apps/iam-console/tests/unit/proxy.test.ts` (lines 52, 121)
- Modify (whole file): `WT/ts/apps/iam-console/tests/standalone-runtime.test.ts`

**Interfaces:**
- Consumes: `readinessResponse` from `@paigasus/auth/server` (Task 4); `logger` from `@paigasus/console-core`; `authRuntime(): Promise<AuthRuntime>` from `lib/auth.ts`.
- Produces: `GET /iam/readyz`. The console-core logger writes one JSON line per event: `{"time":…,"event":"<name>","fields":{…}}`.

- [ ] **Step 1: Write the failing unit rows (T14)**

In `WT/ts/apps/iam-console/tests/unit/proxy.test.ts`, replace line 52:

```ts
  it.each(['/iam', '/iam/healthz', '/iam/auth/login', '/iam/auth/callback', '/iam/auth/logout', '/iam/auth/logout/callback'])('lets %s through with no cookie', (path) => {
```

with:

```ts
  it.each(['/iam', '/iam/healthz', '/iam/readyz', '/iam/auth/login', '/iam/auth/callback', '/iam/auth/logout', '/iam/auth/logout/callback'])('lets %s through with no cookie', (path) => {
```

Replace line 121:

```ts
  it.each(['/iam/', '/iam/orgs', '/iam/orgs?offset=50', '/iam/healthz', '/iam/auth/login'])('runs the proxy for the page %s', (url) => {
```

with:

```ts
  it.each(['/iam/', '/iam/orgs', '/iam/orgs?offset=50', '/iam/healthz', '/iam/readyz', '/iam/auth/login'])('runs the proxy for the page %s', (url) => {
```

Run: `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/apps/iam-console exec vitest run tests/unit/proxy.test.ts`
Expected: FAIL in `lets /iam/readyz through with no cookie` (the `location` header is the login redirect). The matcher row `/iam/readyz` PASSES already: the matcher runs the proxy for every page.

- [ ] **Step 2: Write the failing built-app row (T15)**

Replace the whole content of `WT/ts/apps/iam-console/tests/standalone-runtime.test.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// AC 3's runtime half (spec § 9.5). The build asserts the standalone ARTIFACT exists; this asserts
// the SERVER reads deployment configuration at runtime. Two boots of the SAME binary with different
// PAIGASUS_ZONES must answer different zone maps on `GET /iam/healthz`, which is public and runs the
// FULL configuration parse. If Next had inlined a value, or the standalone trace had dropped a
// package, both boots would agree — and every unit test in this repo would still pass.
//
// Each boot gets a COMPLETE, valid environment, so a failure can only come from what a case
// changes. The mismatch case asserts the mismatch MESSAGE in the server output, not only a 500: a
// missing unrelated variable also answers 500, and must not pass it.
//
// SMA-705 T15. One more boot proves that the BUILT app serves `GET /iam/readyz`. It uses ONE zone,
// because createAuthRuntime refuses the memory store for two zones. Its issuer is on a port that
// fetch refuses at once. The request does not follow a redirect, so a proxy that sends the probe to
// the login route fails the row.
import { spawn, type ChildProcess } from 'node:child_process';
import { createServer } from 'node:net';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const SERVER_ENTRY = fileURLToPath(new URL('../.next/standalone/apps/iam-console/server.js', import.meta.url));

const COMPLETE_ENV: Readonly<Record<string, string>> = {
  PAIGASUS_ZONE: 'iam',
  PAIGASUS_OIDC_ISSUER: 'https://idp.example.test',
  PAIGASUS_OIDC_CLIENT_ID: 'console',
  PAIGASUS_OIDC_CLIENT_SECRET: 'console-secret',
  PAIGASUS_PUBLIC_ORIGIN: 'https://console.example.test',
  PAIGASUS_SESSION_STORE: 'memory',
  PAIGASUS_SERVICES: '{"iam":"http://127.0.0.1:9"}',
  PAIGASUS_IAM_GRPC_URL: 'http://127.0.0.1:9',
};

/** Port 1 is on the Fetch "bad port" list, so discovery fails at once with no connect (SMA-656). */
const UNREACHABLE_ISSUER = 'https://127.0.0.1:1';

async function freePort(): Promise<number> {
  return await new Promise((resolve, reject) => {
    const srv = createServer();
    srv.once('error', reject);
    srv.listen(0, '127.0.0.1', () => {
      const address = srv.address();
      if (address === null || typeof address === 'string') {
        reject(new Error('could not acquire a port'));
        return;
      }
      const { port } = address;
      srv.close(() => resolve(port));
    });
  });
}

type Answer = { status: number; body: string; headers: Headers; output: string };

type BootOptions = {
  /** Keep the server alive until its output holds this text, for at most 5 s. */
  waitForOutput?: string;
  /** Values that replace COMPLETE_ENV entries for this boot. */
  env?: Readonly<Record<string, string>>;
};

// The child is OBSERVED, not merely polled (the reasons are recorded in this file's history): an
// early death is reported as a death, both streams are drained so a chatty child cannot block, and
// a deadline still covers a process that starts and never listens. `waitForOutput` keeps the server
// alive until its log contains that text, because Next writes a route error to stderr around the
// time it sends the 500.
async function getWith(path: string, zones: Record<string, string>, options: BootOptions = {}): Promise<Answer> {
  const { waitForOutput } = options;
  const port = await freePort();
  const child: ChildProcess = spawn(process.execPath, [SERVER_ENTRY], {
    env: { ...process.env, ...COMPLETE_ENV, ...options.env, PORT: String(port), HOSTNAME: '127.0.0.1', PAIGASUS_ZONES: JSON.stringify(zones) },
    stdio: 'pipe',
  });

  let output = '';
  const capture = (chunk: Buffer | string): void => {
    output += String(chunk);
  };
  child.stdout?.on('data', capture);
  child.stderr?.on('data', capture);

  let died: string | undefined;
  child.once('error', (err: Error) => {
    died = `the server process failed to start: ${err.message}`;
  });
  child.once('exit', (code, signal) => {
    died = `the server process exited before answering (code ${String(code)}, signal ${String(signal)})`;
  });

  const withOutput = (message: string): Error => new Error(`${message}\n--- server output ---\n${output.trim() === '' ? '(none captured)' : output}`);

  try {
    const deadline = Date.now() + 60_000;
    for (;;) {
      if (died !== undefined) throw withOutput(died);
      if (Date.now() > deadline) throw withOutput('standalone server did not become ready within 60s');
      let res: Response | undefined;
      try {
        // `redirect: 'manual'`: a redirect must reach the assertions, not be followed (SMA-705 T15).
        res = await fetch(`http://127.0.0.1:${String(port)}${path}`, { redirect: 'manual', signal: AbortSignal.timeout(5_000) });
      } catch {
        // not listening yet, or this attempt hung and timed out
      }
      if (res !== undefined) {
        const answer = { status: res.status, body: await res.text(), headers: res.headers };
        const logDeadline = Date.now() + 5_000;
        while (waitForOutput !== undefined && !output.includes(waitForOutput) && Date.now() < logDeadline) {
          await new Promise((r) => setTimeout(r, 100));
        }
        return { ...answer, output };
      }
      await new Promise((r) => setTimeout(r, 250));
    }
  } finally {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
  }
}

describe('the standalone server reads configuration at runtime', () => {
  it('serves different zone maps from the SAME binary', async () => {
    const first = await getWith('/iam/healthz', { iam: '/iam', gateway: '/gateway' });
    const second = await getWith('/iam/healthz', { iam: '/iam', reports: '/reports' });

    expect(first.status).toBe(200);
    expect(second.status).toBe(200);
    expect(JSON.parse(first.body)).toEqual({ zone: 'iam', zones: { iam: '/iam', gateway: '/gateway' } });
    expect(JSON.parse(second.body)).toEqual({ zone: 'iam', zones: { iam: '/iam', reports: '/reports' } });
  });

  it('fails loudly, with the mismatch message, when the deployed prefix disagrees with the compiled one', async () => {
    const message = 'Base path mismatch for zone "iam"';
    const result = await getWith('/iam/healthz', { iam: '/admin/iam' }, { waitForOutput: message });
    expect(result.status).toBe(500);
    expect(result.output).toContain(message);
  });
});

describe('GET /iam/readyz on the built server (SMA-705 T15)', () => {
  it('answers 503 unready with no redirect, and logs the discovery failure that it started', async () => {
    const result = await getWith('/iam/readyz', { iam: '/iam' }, { env: { PAIGASUS_OIDC_ISSUER: UNREACHABLE_ISSUER }, waitForOutput: '"stage":"readiness"' });
    expect(result.status).toBe(503);
    expect(result.body).toBe('{"status":"unready"}');
    expect(result.headers.get('cache-control')).toBe('no-store');
    expect(result.headers.get('location')).toBeNull();
    const line = result.output.split('\n').find((text) => text.includes('"event":"oidc.discovery_failed"'));
    expect(line, `no oidc.discovery_failed line\n--- server output ---\n${result.output}`).toBeDefined();
    const logged = JSON.parse(line ?? '{}') as { fields?: unknown };
    expect(logged.fields).toEqual({ zone: 'iam', stage: 'readiness', reason: 'network' });
    expect(result.output).not.toContain('readiness.runtime_failed');
  });
});
```

Run (from `WT`): `moon run iam-console-ts:build --force`
Expected: PASS.
Run: `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/apps/iam-console exec vitest run tests/standalone-runtime.test.ts`
Expected: FAIL in the T15 row only: `status` is a redirect (307 to the login route) or 404, not 503. The two `/iam/healthz` rows pass.

- [ ] **Step 3: Add the route and the public path**

Create `WT/ts/apps/iam-console/app/readyz/route.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// `GET /iam/readyz`: the chart's readiness probe (SMA-705). Public (proxy.ts lists it). It answers
// 200 {"status":"ready"} after the auth runtime is built and one OIDC discovery succeeded, and 503
// {"status":"unready"} before that. It never waits for discovery. @paigasus/auth holds the logic
// (readinessResponse, SMA-705 D5). `/healthz` stays the check that touches no dependency.
import { connection } from 'next/server';
import { readinessResponse } from '@paigasus/auth/server';
import { logger } from '@paigasus/console-core';
import { authRuntime } from '../../lib/auth';

export async function GET(): Promise<Response> {
  await connection();
  return readinessResponse(authRuntime, logger);
}
```

In `WT/ts/apps/iam-console/proxy.ts`, replace lines 27-30:

```ts
const authMiddleware = createAuthMiddleware({
  publicPaths: [...authRoutePaths(), '/', '/healthz'],
  loginPath: '/auth/login',
});
```

with:

```ts
const authMiddleware = createAuthMiddleware({
  // '/readyz' (SMA-705 D9): the kubelet's readiness probe sends no session cookie. Without this
  // entry the proxy redirects the probe to the login route, and the probe never reaches /readyz.
  publicPaths: [...authRoutePaths(), '/', '/healthz', '/readyz'],
  loginPath: '/auth/login',
});
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/apps/iam-console exec vitest run tests/unit/proxy.test.ts`
Expected: PASS.

Run (from `WT`): `moon run iam-console-ts:build --force`, then `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/apps/iam-console exec vitest run tests/standalone-runtime.test.ts`
Expected: PASS, three rows. **If T15 fails only on `reason` (not `network`), STOP** and report the logged line. Do not change the assertion.

- [ ] **Step 5: Typecheck, lint, format, app suite**

Run prettier `--write`, then eslint, over `apps/iam-console/app/readyz/route.ts apps/iam-console/proxy.ts apps/iam-console/tests/unit/proxy.test.ts apps/iam-console/tests/standalone-runtime.test.ts`. Prettier may wrap the long `it.each` line; that is expected.
Expected: no lint error.

Run (from `WT`): `moon run iam-console-ts:typecheck iam-console-ts:test`
Expected: PASS. `iam-console-ts:test` runs the whole vitest suite and the three tailwind-source guard modes, after a build.

- [ ] **Step 6: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness branch --show-current
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness add ts/apps/iam-console/app/readyz/route.ts ts/apps/iam-console/proxy.ts ts/apps/iam-console/tests/unit/proxy.test.ts ts/apps/iam-console/tests/standalone-runtime.test.ts
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness commit -m "feat(ts): serve /iam/readyz from the IAM console (SMA-705)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: The gateway console serves `/gateway/readyz` (D9, T14, T15)

**Files:**
- Create: `WT/ts/apps/gateway-console/app/readyz/route.ts`
- Modify: `WT/ts/apps/gateway-console/proxy.ts` (lines 27-32)
- Modify: `WT/ts/apps/gateway-console/tests/unit/proxy.test.ts` (lines 51, 127)
- Modify (whole file): `WT/ts/apps/gateway-console/tests/standalone-runtime.test.ts`

**Interfaces:**
- Consumes: the same as Task 5. `ts/apps/gateway-console/lib/auth.ts` exports the same `authRuntime()`.
- Produces: `GET /gateway/readyz`.

- [ ] **Step 1: Write the failing unit rows (T14)**

In `WT/ts/apps/gateway-console/tests/unit/proxy.test.ts`, replace line 51:

```ts
  it.each(['/gateway', '/gateway/healthz', '/gateway/auth/login', '/gateway/auth/callback', '/gateway/auth/logout', '/gateway/auth/logout/callback', '/gateway/api/chat'])(
```

with:

```ts
  it.each(['/gateway', '/gateway/healthz', '/gateway/readyz', '/gateway/auth/login', '/gateway/auth/callback', '/gateway/auth/logout', '/gateway/auth/logout/callback', '/gateway/api/chat'])(
```

Replace line 127:

```ts
  it.each(['/gateway/', '/gateway/overview', '/gateway/overview?offset=50', '/gateway/healthz', '/gateway/auth/login'])('runs the proxy for the page %s', (url) => {
```

with:

```ts
  it.each(['/gateway/', '/gateway/overview', '/gateway/overview?offset=50', '/gateway/healthz', '/gateway/readyz', '/gateway/auth/login'])('runs the proxy for the page %s', (url) => {
```

Run: `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/apps/gateway-console exec vitest run tests/unit/proxy.test.ts`
Expected: FAIL in `lets /gateway/readyz through with no cookie` only.

- [ ] **Step 2: Write the failing built-app row (T15)**

Replace the whole content of `WT/ts/apps/gateway-console/tests/standalone-runtime.test.ts` with:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// AC 3's runtime half (spec § 9.5). The build asserts the standalone ARTIFACT exists; this asserts
// the SERVER reads deployment configuration at runtime. Two boots of the SAME binary with different
// PAIGASUS_ZONES must answer different zone maps on `GET /gateway/healthz`, which is public and runs
// the FULL configuration parse. If Next had inlined a value, or the standalone trace had dropped a
// package, both boots would agree — and every unit test in this repo would still pass.
//
// Each boot gets a COMPLETE, valid environment, so a failure can only come from what a case
// changes. The mismatch case asserts the mismatch MESSAGE in the server output, not only a 500: a
// missing unrelated variable also answers 500, and must not pass it.
//
// SMA-705 T15. One more boot proves that the BUILT app serves `GET /gateway/readyz`. It uses ONE
// zone, because createAuthRuntime refuses the memory store for two zones. Its issuer is on a port
// that fetch refuses at once. The request does not follow a redirect, so a proxy that sends the
// probe to the login route fails the row.
import { spawn, type ChildProcess } from 'node:child_process';
import { createServer } from 'node:net';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const SERVER_ENTRY = fileURLToPath(new URL('../.next/standalone/apps/gateway-console/server.js', import.meta.url));

const COMPLETE_ENV: Readonly<Record<string, string>> = {
  PAIGASUS_ZONE: 'gateway',
  PAIGASUS_OIDC_ISSUER: 'https://idp.example.test',
  PAIGASUS_OIDC_CLIENT_ID: 'console',
  PAIGASUS_OIDC_CLIENT_SECRET: 'console-secret',
  PAIGASUS_PUBLIC_ORIGIN: 'https://console.example.test',
  PAIGASUS_SESSION_STORE: 'memory',
  PAIGASUS_SERVICES: '{"iam":"http://127.0.0.1:9","gateway":"http://127.0.0.1:9"}',
  PAIGASUS_IAM_GRPC_URL: 'http://127.0.0.1:9',
};

/** Port 1 is on the Fetch "bad port" list, so discovery fails at once with no connect (SMA-656). */
const UNREACHABLE_ISSUER = 'https://127.0.0.1:1';

async function freePort(): Promise<number> {
  return await new Promise((resolve, reject) => {
    const srv = createServer();
    srv.once('error', reject);
    srv.listen(0, '127.0.0.1', () => {
      const address = srv.address();
      if (address === null || typeof address === 'string') {
        reject(new Error('could not acquire a port'));
        return;
      }
      const { port } = address;
      srv.close(() => resolve(port));
    });
  });
}

type Answer = { status: number; body: string; headers: Headers; output: string };

type BootOptions = {
  /** Keep the server alive until its output holds this text, for at most 5 s. */
  waitForOutput?: string;
  /** Values that replace COMPLETE_ENV entries for this boot. */
  env?: Readonly<Record<string, string>>;
};

// The child is OBSERVED, not merely polled: an early death is reported as a death, both streams are
// drained so a chatty child cannot block, and a deadline still covers a process that starts and
// never listens. `waitForOutput` keeps the server alive until its log contains that text, because
// Next writes a route error to stderr around the time it sends the 500.
async function getWith(path: string, zones: Record<string, string>, options: BootOptions = {}): Promise<Answer> {
  const { waitForOutput } = options;
  const port = await freePort();
  const child: ChildProcess = spawn(process.execPath, [SERVER_ENTRY], {
    env: { ...process.env, ...COMPLETE_ENV, ...options.env, PORT: String(port), HOSTNAME: '127.0.0.1', PAIGASUS_ZONES: JSON.stringify(zones) },
    stdio: 'pipe',
  });

  let output = '';
  const capture = (chunk: Buffer | string): void => {
    output += String(chunk);
  };
  child.stdout?.on('data', capture);
  child.stderr?.on('data', capture);

  let died: string | undefined;
  child.once('error', (err: Error) => {
    died = `the server process failed to start: ${err.message}`;
  });
  child.once('exit', (code, signal) => {
    died = `the server process exited before answering (code ${String(code)}, signal ${String(signal)})`;
  });

  const withOutput = (message: string): Error => new Error(`${message}\n--- server output ---\n${output.trim() === '' ? '(none captured)' : output}`);

  try {
    const deadline = Date.now() + 60_000;
    for (;;) {
      if (died !== undefined) throw withOutput(died);
      if (Date.now() > deadline) throw withOutput('standalone server did not become ready within 60s');
      let res: Response | undefined;
      try {
        // `redirect: 'manual'`: a redirect must reach the assertions, not be followed (SMA-705 T15).
        res = await fetch(`http://127.0.0.1:${String(port)}${path}`, { redirect: 'manual', signal: AbortSignal.timeout(5_000) });
      } catch {
        // not listening yet, or this attempt hung and timed out
      }
      if (res !== undefined) {
        const answer = { status: res.status, body: await res.text(), headers: res.headers };
        const logDeadline = Date.now() + 5_000;
        while (waitForOutput !== undefined && !output.includes(waitForOutput) && Date.now() < logDeadline) {
          await new Promise((r) => setTimeout(r, 100));
        }
        return { ...answer, output };
      }
      await new Promise((r) => setTimeout(r, 250));
    }
  } finally {
    if (child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
  }
}

describe('the standalone server reads configuration at runtime', () => {
  it('serves different zone maps from the SAME binary', async () => {
    const first = await getWith('/gateway/healthz', { gateway: '/gateway', iam: '/iam' });
    const second = await getWith('/gateway/healthz', { gateway: '/gateway', reports: '/reports' });

    expect(first.status).toBe(200);
    expect(second.status).toBe(200);
    expect(JSON.parse(first.body)).toEqual({ zone: 'gateway', zones: { gateway: '/gateway', iam: '/iam' } });
    expect(JSON.parse(second.body)).toEqual({ zone: 'gateway', zones: { gateway: '/gateway', reports: '/reports' } });
  });

  it('fails loudly, with the mismatch message, when the deployed prefix disagrees with the compiled one', async () => {
    const message = 'Base path mismatch for zone "gateway"';
    const result = await getWith('/gateway/healthz', { gateway: '/admin/gateway' }, { waitForOutput: message });
    expect(result.status).toBe(500);
    expect(result.output).toContain(message);
  });
});

describe('GET /gateway/readyz on the built server (SMA-705 T15)', () => {
  it('answers 503 unready with no redirect, and logs the discovery failure that it started', async () => {
    const result = await getWith('/gateway/readyz', { gateway: '/gateway' }, { env: { PAIGASUS_OIDC_ISSUER: UNREACHABLE_ISSUER }, waitForOutput: '"stage":"readiness"' });
    expect(result.status).toBe(503);
    expect(result.body).toBe('{"status":"unready"}');
    expect(result.headers.get('cache-control')).toBe('no-store');
    expect(result.headers.get('location')).toBeNull();
    const line = result.output.split('\n').find((text) => text.includes('"event":"oidc.discovery_failed"'));
    expect(line, `no oidc.discovery_failed line\n--- server output ---\n${result.output}`).toBeDefined();
    const logged = JSON.parse(line ?? '{}') as { fields?: unknown };
    expect(logged.fields).toEqual({ zone: 'gateway', stage: 'readiness', reason: 'network' });
    expect(result.output).not.toContain('readiness.runtime_failed');
  });
});
```

Run (from `WT`): `moon run gateway-console-ts:build --force`, then `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/apps/gateway-console exec vitest run tests/standalone-runtime.test.ts`
Expected: FAIL in the T15 row only (a redirect or 404, not 503).

- [ ] **Step 3: Add the route and the public path**

Create `WT/ts/apps/gateway-console/app/readyz/route.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// `GET /gateway/readyz`: the chart's readiness probe (SMA-705). Public (proxy.ts lists it). It
// answers 200 {"status":"ready"} after the auth runtime is built and one OIDC discovery succeeded,
// and 503 {"status":"unready"} before that. It never waits for discovery. @paigasus/auth holds the
// logic (readinessResponse, SMA-705 D5). `/healthz` stays the check that touches no dependency.
import { connection } from 'next/server';
import { readinessResponse } from '@paigasus/auth/server';
import { logger } from '@paigasus/console-core';
import { authRuntime } from '../../lib/auth';

export async function GET(): Promise<Response> {
  await connection();
  return readinessResponse(authRuntime, logger);
}
```

In `WT/ts/apps/gateway-console/proxy.ts`, replace:

```ts
  // '/api/chat' (SMA-635): the playground route answers its own 401 JSON. An exact match, so it
  // opens no other path.
  publicPaths: [...authRoutePaths(), '/', '/healthz', '/api/chat'],
```

with:

```ts
  // '/api/chat' (SMA-635): the playground route answers its own 401 JSON. An exact match, so it
  // opens no other path. '/readyz' (SMA-705 D9): the kubelet's readiness probe sends no session
  // cookie. Without this entry the proxy redirects the probe to the login route.
  publicPaths: [...authRoutePaths(), '/', '/healthz', '/readyz', '/api/chat'],
```

- [ ] **Step 4: Run the tests to see them pass**

Run: `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/apps/gateway-console exec vitest run tests/unit/proxy.test.ts`
Expected: PASS.

Run (from `WT`): `moon run gateway-console-ts:build --force`, then the standalone command of Step 2.
Expected: PASS, three rows. The same STOP rule as Task 5 Step 4 applies to `reason`.

- [ ] **Step 5: Typecheck, lint, format, app suite**

Run prettier `--write`, then eslint, over `apps/gateway-console/app/readyz/route.ts apps/gateway-console/proxy.ts apps/gateway-console/tests/unit/proxy.test.ts apps/gateway-console/tests/standalone-runtime.test.ts`.
Expected: no lint error.

Run (from `WT`): `moon run gateway-console-ts:typecheck gateway-console-ts:test`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness branch --show-current
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness add ts/apps/gateway-console/app/readyz/route.ts ts/apps/gateway-console/proxy.ts ts/apps/gateway-console/tests/unit/proxy.test.ts ts/apps/gateway-console/tests/standalone-runtime.test.ts
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness commit -m "feat(ts): serve /gateway/readyz from the gateway console (SMA-705)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 7: `/iam/readyz` reaches ready through the e2e build (T16)

**Files:**
- Modify: `WT/ts/apps/iam-console/tests/e2e/harness.spec.ts` (append after line 12)

**Interfaces:**
- Consumes: the worker-scoped `harness` fixture (`tests/e2e/support/harness.ts`): `harness.url(path)`, `harness.serverOutput()`. The harness starts a TLS fake IdP and passes its issuer with `NODE_EXTRA_CA_CERTS`.
- Produces: nothing for later tasks. Task 11's mutation M1 uses this test.

The test title must NOT start with `R<n>:`. `tests/unit/e2e-rows.test.ts` allows exactly the rows R1–R20.

- [ ] **Step 1: Write the test**

Append to `WT/ts/apps/iam-console/tests/e2e/harness.spec.ts`:

```ts

// SMA-705 T16. The positive half of /iam/readyz through the real build. The TLS fake IdP answers
// discovery, so the probe must reach 200 within the bound. An earlier spec of this run can have
// logged in, which discovers too. Either way the route must answer 200.
test('harness: /iam/readyz reaches 200 ready once discovery succeeded (SMA-705 T16)', async ({ request, harness }) => {
  const url = harness.url('/iam/readyz');
  let last = 0;
  await expect
    .poll(
      async () => {
        last = (await request.get(url, { maxRedirects: 0 })).status();
        return last;
      },
      { timeout: 15_000, intervals: [250], message: 'GET /iam/readyz never answered 200' },
    )
    .toBe(200)
    .catch((error: unknown) => {
      throw new Error(`the last status was ${String(last)}\n--- server output ---\n${harness.serverOutput()}`, { cause: error });
    });
  const response = await request.get(url, { maxRedirects: 0 });
  expect(response.status()).toBe(200);
  expect(response.headers()['cache-control']).toBe('no-store');
  expect(await response.json()).toEqual({ status: 'ready' });
  expect(harness.serverOutput()).not.toContain('"stage":"readiness"');
  expect(harness.serverOutput()).not.toContain('readiness.runtime_failed');
});
```

- [ ] **Step 2: Confirm that the harness reaches the discovered state**

Run (from `WT`): `moon run iam-console-ts:build --force`
Then: `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/apps/iam-console exec playwright test tests/e2e/harness.spec.ts`
Expected: PASS, 2 tests. The route exists since Task 5, so this row is green at once. Task 11 mutation M1 proves that it can fail.

**If the new test fails with a last status of 503, STOP.** Do not change the harness, the bound or the IdP. Report the thrown message: it holds the server output. Look for `oidc.discovery_failed` (the fake IdP did not answer discovery) or `readiness.runtime_failed`. This means the harness cannot reach the discovered state, and the spec requires a report, not a workaround. **If Playwright cannot start** (a browser is not installed), STOP and report. Do not install anything.

- [ ] **Step 3: Lint, format, app suite**

Run prettier `--write`, then eslint, over `apps/iam-console/tests/e2e/harness.spec.ts`.
Expected: no lint error.

Run (from `WT`): `moon run iam-console-ts:test iam-console-ts:typecheck`
Expected: PASS. This runs `e2e-rows.test.ts` (no new `R<n>` row), `e2e-read-only.test.ts` (no file write in `tests/e2e/**`) and `hydration.test.ts`.

- [ ] **Step 4: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness branch --show-current
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness add ts/apps/iam-console/tests/e2e/harness.spec.ts
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness commit -m "test(ts): prove /iam/readyz reaches ready through the e2e build (SMA-705)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 8: The chart probes `/readyz` (A2, T17)

**Files:**
- Modify: `WT/charts/paigasus/templates/console-deployment.yaml` (lines 88-102)
- Modify: `WT/ci/helm-render/fixtures/template-only-diff/templates/console-deployment.yaml` (the same block, lines 89-103)
- Modify: `WT/ci/helm-render/fixtures/security-context/templates/console-deployment.yaml` (the same block, lines 88-102)
- Regenerate: `WT/charts/paigasus/tests/golden/iam-only.yaml`, `WT/charts/paigasus/tests/golden/iam-and-gateway.yaml`

**Interfaces:**
- Consumes: the routes of Tasks 5 and 6.
- Produces: every console Deployment has `readinessProbe.httpGet.path: <basePath>/readyz`.

Nothing in `ci/helm-render/helm_render.py`, `ci/affected-graph/*.py`, the other chart scripts or `ci/kind/` pins the probe path (checked with `grep -rn "healthz\|readinessProbe\|readyz"`). `CHART_SCRIPT_FLOOR` and `HELM_RENDER_SH_CALL_SITES` do not change: no chart script is added.

- [ ] **Step 1: Confirm that the two fixtures differ from the live file only by their mutation**

Run (from `WT`): `diff charts/paigasus/templates/console-deployment.yaml ci/helm-render/fixtures/template-only-diff/templates/console-deployment.yaml`
Expected: exactly one hunk at line 47: the `NEGATIVE-CONTROL FIXTURE` comment and the image line that reads `$root.Values.zones.iam.console.image.tag`.

Run: `diff charts/paigasus/templates/console-deployment.yaml ci/helm-render/fixtures/security-context/templates/console-deployment.yaml`
Expected: exactly one hunk at line 42: `runAsNonRoot: true` replaced by the `NEGATIVE-CONTROL FIXTURE` comment.
If either diff shows more, STOP and report: a fixture already drifted.

- [ ] **Step 2: Edit the live template**

In `WT/charts/paigasus/templates/console-deployment.yaml`, replace:

```yaml
          # Liveness is a TCP check, NOT httpGet on /healthz. That route runs the FULL config
          # parse and its failure is deliberately not memoized, so using it for liveness puts a
          # misconfigured pod into CrashLoopBackOff instead of leaving it running and NotReady
          # with a readable log — and RUNBOOK-containers.md § 4 requires liveness never to touch
          # a dependency. The consoles have no /readyz; that is a recorded gap (spec § 7.8).
          livenessProbe:
            tcpSocket:
              port: http
            periodSeconds: 20
          readinessProbe:
            httpGet:
              path: {{ $z.basePath }}/healthz
              port: http
            periodSeconds: 10
            failureThreshold: 3
```

with:

```yaml
          # Liveness is a TCP check, NOT httpGet on /healthz. That route runs the FULL config
          # parse and its failure is deliberately not memoized, so using it for liveness puts a
          # misconfigured pod into CrashLoopBackOff instead of leaving it running and NotReady
          # with a readable log — and RUNBOOK-containers.md § 4 requires liveness never to touch
          # a dependency.
          # Readiness is /readyz (SMA-705). The pod is not ready until its auth runtime is built
          # and one OIDC discovery succeeded. After that it stays ready for the life of the
          # process. Readiness does not check Redis (SMA-705 D1). No timeoutSeconds: the route
          # never waits for discovery (SMA-705 D3).
          livenessProbe:
            tcpSocket:
              port: http
            periodSeconds: 20
          readinessProbe:
            httpGet:
              path: {{ $z.basePath }}/readyz
              port: http
            periodSeconds: 10
            failureThreshold: 3
```

- [ ] **Step 3: See the golden test fail**

Run (from `WT`): `export PROTO_REPORTER=text && /bin/bash charts/paigasus/tests/render.sh`
Expected: exit 1. Two `FAIL [...]: rendered output differs from the golden file` lines. The diff shows only the comment lines and `path: /iam/readyz` (and `path: /gateway/readyz` in `iam-and-gateway`). This is T17 red.

- [ ] **Step 4: Re-sync both fixtures in the same commit**

Apply the Step 2 replacement, character for character, to `WT/ci/helm-render/fixtures/template-only-diff/templates/console-deployment.yaml` and to `WT/ci/helm-render/fixtures/security-context/templates/console-deployment.yaml`. The block text is the same in both fixtures. Keep each fixture's own mutation.

Run the two `diff` commands of Step 1 again.
Expected: the same single hunk each, as before.

- [ ] **Step 5: Regenerate the golden files and review them**

Run (from `WT`): `helm version --short`
Expected: `v3.22.0+g…` (the `.prototools` pin). If the version differs, STOP: the golden files are a byte pin of the pinned helm.

Run: `export PROTO_REPORTER=text && /bin/bash charts/paigasus/tests/render.sh --update`
Expected: `  updated iam-only.yaml` and `  updated iam-and-gateway.yaml`.

Run: `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness diff -U0 -- charts/paigasus/tests/golden`
Expected: only these changes, once per console Deployment (one in `iam-only.yaml`, two in `iam-and-gateway.yaml`): the line `a dependency. The consoles have no /readyz; that is a recorded gap (spec § 7.8).` becomes `a dependency.`, four `# Readiness is /readyz (SMA-705)…` comment lines are added, and `path: /iam/healthz` or `path: /gateway/healthz` under `readinessProbe` becomes `…/readyz`. The two backend `path: /healthz` lines do NOT change. If any other line changed (for example a `{"type":"message"` line from proto), STOP and report.

Run: `export PROTO_REPORTER=text && /bin/bash charts/paigasus/tests/render.sh`
Expected: `  ok [iam-only]`, `  ok [iam-and-gateway]`, `== chart render OK ==`, exit 0.

- [ ] **Step 6: Run the helm-render gate**

Run (from `WT`), each with `export PROTO_REPORTER=text &&` first:
- `/bin/bash ci/helm-render/run.sh --self-test`
- `/bin/bash ci/helm-render/run.sh --negative-control`
- `/bin/bash ci/helm-render/run.sh`

Expected: exit 0 for each, and no `FAIL` row. The negative control must still report each fixture's own named row (`template-only-diff` and `security-context` included).
If a run exits 2 and names `maps.sh` with a SIGPIPE (141), that is the 512-byte pipe host state (`ci/helm-render/README.md`, residual 5), not a finding: record it and rely on CI. If row `8c chart-app-version` exits 2 for a missing tag, run `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness fetch --tags` and run it again.

- [ ] **Step 7: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness branch --show-current
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness add charts/paigasus/templates/console-deployment.yaml ci/helm-render/fixtures/template-only-diff/templates/console-deployment.yaml ci/helm-render/fixtures/security-context/templates/console-deployment.yaml charts/paigasus/tests/golden/iam-only.yaml charts/paigasus/tests/golden/iam-and-gateway.yaml
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness commit -m "feat(repo): probe console readiness on /readyz in the chart (SMA-705)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 9: Both console versions move to `0.2.0` (§ 9, SMA-688)

**Files:**
- Modify: `WT/ts/apps/iam-console/package.json` (line 3), `WT/ts/apps/gateway-console/package.json` (line 3)
- Modify: `WT/ts/apps/iam-console/CHANGELOG.md`, `WT/ts/apps/gateway-console/CHANGELOG.md` (after `## [Unreleased]`)
- Modify: `WT/charts/paigasus/values.yaml` (lines 20, 75)
- Regenerate: both golden files (the image lines only)

**Interfaces:**
- Consumes: the chain registry `WT/ci/images/chains.toml` (version files and changelogs of `iam-console` and `gateway-console`).
- Produces: `repo:helm-render` row 8a green with both tags at `0.2.0`; `release_plan.py --assert` green.

Checked, and NOT changed here: `Chart.yaml` `appVersion` stays `0.1.0` (`.github/CLAUDE.md`, SMA-696; row 8c needs the release tags of that version). `Chart.yaml` `version` stays `0.1.0` (no chart release process moves it). `ts/pnpm-lock.yaml` records no version for a workspace importer. `ci/kind/values/a.yaml` uses the `dev` tags. The `0.1.0` literals in `ci/images/run.sh`, `helm_render.py` and `release_plan.py` are synthetic test values.

- [ ] **Step 1: Bump the two package versions, and see row 8a fail**

In both `WT/ts/apps/iam-console/package.json` and `WT/ts/apps/gateway-console/package.json`, replace line 3:

```json
  "version": "0.1.0",
```

with:

```json
  "version": "0.2.0",
```

Run (from `WT`): `export PROTO_REPORTER=text && /bin/bash ci/helm-render/run.sh`
Expected: exit 1, with row `8a default-image-tags` red for `iam-console` and `gateway-console` (the tag `0.1.0` differs from the version `0.2.0`).

Run: `export PROTO_REPORTER=text && bash ci/release-plan/run.sh --assert`
Expected: non-zero exit. A `release-plan:` line says `paigasus-iam-console is at 0.2.0 but <path of its CHANGELOG.md> has no `## [0.2.0]` heading`, and a second line says the same for `paigasus-gateway-console`.

- [ ] **Step 2: Add the CHANGELOG sections**

In `WT/ts/apps/iam-console/CHANGELOG.md`, replace:

```md
## [Unreleased]

## [0.1.0] - 2026-09-26
```

with:

```md
## [Unreleased]

## [0.2.0] - 2026-09-27

### Added

- A readiness route, `GET /iam/readyz`. It answers 503 `{"status":"unready"}` until the auth
  runtime is built and one OIDC discovery succeeded. After that it answers 200
  `{"status":"ready"}` for the life of the process. It never waits for discovery. The proxy lets
  it through with no session cookie. The Helm chart's readiness probe uses it (SMA-705).
- Two log events. `oidc.discovery_failed` has the stage `readiness` for a discovery that the
  route started. `readiness.runtime_failed` holds only the name of the error that stopped the
  runtime build (SMA-705).

## [0.1.0] - 2026-09-26
```

In `WT/ts/apps/gateway-console/CHANGELOG.md`, make the same replacement with `GET /gateway/readyz` in place of `GET /iam/readyz`.

Run: `export PROTO_REPORTER=text && bash ci/release-plan/run.sh --assert`
Expected: exit 0.

- [ ] **Step 3: Move the two chart tags**

In `WT/charts/paigasus/values.yaml`, replace:

```yaml
        # Pinned to the image version in ts/apps/iam-console/package.json (SMA-688). A version
        # bump updates this tag in the same PR; repo:helm-render row 8a fails otherwise. An empty
        # tag falls back to .Chart.AppVersion, a version every image released (row 8c).
        tag: "0.1.0"
```

with:

```yaml
        # Pinned to the image version in ts/apps/iam-console/package.json (SMA-688). A version
        # bump updates this tag in the same PR; repo:helm-render row 8a fails otherwise. An empty
        # tag falls back to .Chart.AppVersion, a version every image released (row 8c).
        tag: "0.2.0"
```

Replace:

```yaml
        # Pinned to the image version in ts/apps/gateway-console/package.json (SMA-688).
        # repo:helm-render row 8a fails when the two differ.
        tag: "0.1.0"
```

with:

```yaml
        # Pinned to the image version in ts/apps/gateway-console/package.json (SMA-688).
        # repo:helm-render row 8a fails when the two differ.
        tag: "0.2.0"
```

The two backend blocks (`paigasus-iam`, `paigasus-gateway`) keep `tag: "0.1.0"`.

- [ ] **Step 4: Regenerate and review the golden files**

Run (from `WT`): `export PROTO_REPORTER=text && /bin/bash charts/paigasus/tests/render.sh --update`
Run: `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness diff -U0 -- charts/paigasus/tests/golden`
Expected: only the console image lines change: `ghcr.io/smk1085/paigasus-iam-console:0.1.0` → `:0.2.0` (both files) and `ghcr.io/smk1085/paigasus-gateway-console:0.1.0` → `:0.2.0` (`iam-and-gateway.yaml`). Any other change: STOP and report.

Run: `export PROTO_REPORTER=text && /bin/bash charts/paigasus/tests/render.sh`
Expected: `== chart render OK ==`, exit 0.

Run, each with `export PROTO_REPORTER=text &&` first: `/bin/bash ci/helm-render/run.sh --self-test`, `/bin/bash ci/helm-render/run.sh --negative-control`, `/bin/bash ci/helm-render/run.sh`
Expected: exit 0 for each. Row 8a and row 8b are green.

- [ ] **Step 5: Format and check the lockfile**

Run: `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts exec prettier --write apps/iam-console/package.json apps/gateway-console/package.json apps/iam-console/CHANGELOG.md apps/gateway-console/CHANGELOG.md`
Then (from `WT`): `moon run ts:fmt --force`
Expected: PASS.

Run: `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness diff --stat -- ts/pnpm-lock.yaml`
Expected: no output.

- [ ] **Step 6: Commit**

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness branch --show-current
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness add ts/apps/iam-console/package.json ts/apps/gateway-console/package.json ts/apps/iam-console/CHANGELOG.md ts/apps/gateway-console/CHANGELOG.md charts/paigasus/values.yaml charts/paigasus/tests/golden/iam-only.yaml charts/paigasus/tests/golden/iam-and-gateway.yaml
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness commit -m "chore(release): bump both console versions to 0.2.0 (SMA-705)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 10: Documentation (spec § 7)

**Files:**
- Modify: `PKG/README.md` (lines 189-191, 212-215, 236-238; a new section before `## Cookies` at line 243)
- Modify: `WT/docs/ops/RUNBOOK-containers.md` (§ 6, before line 347)
- Modify: `WT/docs/ops/RUNBOOK-chart.md` (append a § 10 after line 486)
- Modify: `WT/ci/kind/README.md` ("Where to look first", after line 74)
- Modify: `WT/docs/superpowers/specs/2026-09-26-sma-656-auth-login-oidc-503-design.md` (§ 6, lines 455-457)
- Modify: `WT/docs/superpowers/specs/2026-09-19-sma-513-multi-zone-ingress-helm-design.md` (§ 7.8, lines 476-478)
- Modify: `WT/docs/superpowers/specs/2026-09-09-sma-506-auth-design.md` (§ 12, lines 1043-1052)

The source comments of § 7 ("the comments listed in § 4") landed in Tasks 1–3, 5, 6 and 8.

- [ ] **Step 1: README, the SMA-656 section**

In `PKG/README.md`, replace lines 189-191:

```md
The OIDC client gets the IdP's discovery document on the first OIDC call (login, callback,
refresh, revocation or logout) of a process, and keeps it for the life of the process. A failed discovery is not kept: the next
call tries again, and it can wait up to `PAIGASUS_OIDC_HTTP_TIMEOUT_MS`.
```

with:

```md
The OIDC client gets the IdP's discovery document on the first call that needs it, and keeps it
for the life of the process. In a console behind the chart, the first caller is the readiness
route (`/readyz`, SMA-705, below). A login, a callback, a refresh, a revocation or a logout is the
first caller only where nothing probes `/readyz`, for example a local `next start`. A failed
discovery is not kept: the next call tries again, and it can wait up to
`PAIGASUS_OIDC_HTTP_TIMEOUT_MS`.
```

Replace lines 212-215:

```md
**The `oidc.discovery_failed` event.** Each such 503 logs one event, `{ zone, stage, reason }`.
`stage` is `login` or `callback`. The event never holds the caught error, its message, its name or
a URL. It means "this process has no discovered configuration, and a login or a callback needed
one". Its absence does NOT mean that the IdP is healthy. `reason` is one of:
```

with:

```md
**The `oidc.discovery_failed` event.** Each such 503 logs one event, `{ zone, stage, reason }`.
`stage` is `login`, `callback` or `readiness`. The readiness route logs `readiness` for an attempt
that it started (SMA-705). The event never holds the caught error, its message, its name or a URL.
It means "this process has no discovered configuration, and a login, a callback or the readiness
route needed one". When a login or a callback joins an attempt that the readiness route started,
one failure logs one event for each of them. Its absence does NOT mean that the IdP is healthy.
`reason` is one of:
```

Replace lines 236-238:

```md
- One pod that cannot discover, behind a round-robin balancer, fails the callbacks of logins that
  other pods started. Each retry then costs a full sign-in at the IdP. SMA-705 tracks a readiness
  gate or an eager discovery at start.
```

with:

```md
- A pod that cannot discover is not ready, so the Service sends it no login or callback traffic
  (SMA-705, "The readiness route" below). Before SMA-705 such a pod stayed ready. Behind a
  round-robin balancer it then failed the callbacks of logins that other pods started.
```

- [ ] **Step 2: README, the new readiness section**

In `PKG/README.md`, directly before the line `## Cookies`, insert:

```md
### The readiness route (SMA-705)

`@paigasus/auth/server` exports `readinessResponse(getRuntime, logger)`. Each console mounts it as
`app/readyz/route.ts`, so the route is `<basePath>/readyz`. The route is public: each app's
`proxy.ts` lists `/readyz` in `publicPaths`. The Helm chart's readiness probe calls it. `/healthz`
does not change: it parses the configuration and touches no dependency.

| State                        | Answer                                                                          |
| ---------------------------- | ------------------------------------------------------------------------------- |
| The runtime build failed     | 503 `{"status":"unready"}`, and one `readiness.runtime_failed` event            |
| Discovery has not started    | 503 `{"status":"unready"}`. The route starts discovery and does not wait for it |
| Discovery is in flight       | 503 `{"status":"unready"}`                                                      |
| Discovery succeeded one time | 200 `{"status":"ready"}`                                                        |

Every answer has `Cache-Control: no-store`. The body never says why the pod is not ready, because
the route is public. Read the log.

- **Ready is sticky.** After one successful discovery, the pod stays ready for the life of the
  process. The adapter never discovers again. An IdP outage after that affects every pod.
  Removing every pod would also remove the pages of signed-in users.
- **The route never waits for discovery.** The next probe sees the result. The first call of a
  process builds the runtime, and that includes the Redis connect (bounded by
  `PAIGASUS_SESSION_REDIS_TIMEOUT_MS`). So the first probe can pass the kubelet's default 1 s
  timeout. The kubelet then counts one failure, and the next probe finds the runtime.
- **Readiness does not check Redis.** Every console pod shares one Redis, so a Redis fault would
  take every pod out of rotation. The store 503 above answers a store fault.
- **`readiness.runtime_failed { error }`.** The runtime build failed: a configuration parse error,
  a cross-field rule, or the Redis client build. `error` is the error's `name` only, for example
  `TypeError`. A cross-field rule logs `AuthConfigError`. The event
  never holds the message: a malformed Redis URL puts the password into the message of
  node-redis's `TypeError`. Each probe tries the build again.

**Known limits.**

- A fresh install or a full restart during an IdP outage leaves no ready console pod. The ingress
  then answers 503 for every console page, also for a signed-in user. A rolling update keeps the
  old ready pods.
- A configuration defect (a wrong issuer, a wrong CA, a malformed Redis URL) keeps the pod not
  ready for ever. The log shows the `reason` or the error name.
- A failing pod logs about one `oidc.discovery_failed` line each 10 s: each probe after a settled
  failure starts a new attempt. A login or a callback that joins the attempt adds its own event.
- Ready proves discovery only. It does not prove that the pod reaches `token_endpoint` or
  `jwks_uri`. A pod-local fault after the first success (DNS or egress on one node) does not make
  the pod not ready.
- The route is not rate-limited. The adapter joins an attempt in flight, so the IdP gets at most
  one discovery request at a time per pod. A caller that loops `/readyz` against a fast failure
  gets back-to-back IdP requests and one log line per attempt.
- Nothing makes a third console app ship `app/readyz/route.ts`.

```

Run: `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts exec prettier --write packages/paigasus-auth/README.md`
Expected: Prettier may align the table again; that is expected.

- [ ] **Step 3: The container runbook**

In `WT/docs/ops/RUNBOOK-containers.md`, directly before the line that starts with `- **The healthcheck file fetches `<BASE_PATH>/healthz` on `127.0.0.1:$PORT`, with a 2500 ms`, insert:

```md
- **The consoles have two probe routes (SMA-705).** `<BASE_PATH>/healthz` parses the
  configuration and touches no dependency. The image `HEALTHCHECK`, `ci/images/run.sh` and the
  test harnesses use it. `<BASE_PATH>/readyz` answers 200 `{"status":"ready"}` only after the
  auth runtime is built and one OIDC discovery succeeded, and 503 `{"status":"unready"}` before
  that. It does not check Redis. The Helm chart's `readinessProbe` uses `/readyz`, and its liveness
  probe is a TCP check. See `ts/packages/paigasus-auth/README.md`, "The readiness route".
```

- [ ] **Step 4: The chart runbook**

Append at the end of `WT/docs/ops/RUNBOOK-chart.md`:

```md

## 10. Console pods stay NotReady (SMA-705)

A console pod is ready only after its auth runtime is built and one OIDC discovery succeeded. Its
readiness probe is `<basePath>/readyz`. So when a console cannot reach the IdP, or cannot build
its runtime, the pod stays `0/1 Ready` and `helm install --wait` does not finish. The probe body
does not say why. Read the pod log:

- `oidc.discovery_failed` with `"stage":"readiness"`: discovery failed. The `reason` field names
  the class of the fault. `ts/packages/paigasus-auth/README.md` has the table of reasons. A `tls`
  reason usually means a missing or wrong `oidc.caBundle` (§ 7). A `dns` reason means that the pod
  cannot resolve the issuer host.
- `readiness.runtime_failed`: the runtime build failed. The `error` field holds only the error's
  name. Check the console env (`PAIGASUS_*`), and check `PAIGASUS_SESSION_REDIS_URL` for a
  malformed URL. A cross-field rule logs `AuthConfigError`.

A pod that was ready one time stays ready for the life of the process. An IdP outage after that
does not make it not ready. Readiness does not check Redis (SMA-705 D1).
```

- [ ] **Step 5: The kind README**

In `WT/ci/kind/README.md`, after the line that starts with `- The preflight fails: read `idp-preflight.json` and `keycloak.log`.`, insert:

```md
- `helm install --wait` times out and a console pod is not Ready: the pod failed its `/readyz` probe (SMA-705). Read `logs/paigasus-*-iam-console-*.log` or `logs/paigasus-*-gateway-console-*.log`. Look for `oidc.discovery_failed` with `"stage":"readiness"`: its `reason` names the fault, and a `tls` reason comes from the CA or the CoreDNS block, as for the preflight. Or look for `readiness.runtime_failed`: its `error` names the error class.
```

- [ ] **Step 6: The three older specs**

In `WT/docs/superpowers/specs/2026-09-26-sma-656-auth-login-oidc-503-design.md`, replace:

```md
- One pod that cannot discover, behind a round-robin balancer, fails the callbacks of logins that
  other pods started. Each retry then costs a full sign-in at the IdP. A readiness gate or an eager
  discovery at start would prevent this. SMA-705 tracks it.
```

with:

```md
- One pod that cannot discover, behind a round-robin balancer, fails the callbacks of logins that
  other pods started. Each retry then costs a full sign-in at the IdP. A readiness gate or an eager
  discovery at start would prevent this. SMA-705 tracks it. **Closed by SMA-705:** the chart's
  readiness probe is now `<basePath>/readyz`, so such a pod is not ready and gets no traffic.
```

In `WT/docs/superpowers/specs/2026-09-19-sma-513-multi-zone-ingress-helm-design.md`, replace:

```md
**Known gap:** `getRuntimeConfig()` parses environment only and never connects, so a console whose
Redis is down still reports Ready while every session read fails. A real `/readyz` is the fix and
is out of scope here (§ 11).
```

with:

```md
**Known gap:** `getRuntimeConfig()` parses environment only and never connects, so a console whose
Redis is down still reports Ready while every session read fails. A real `/readyz` is the fix and
is out of scope here (§ 11).

**Superseded by SMA-705.** The consoles now have `<basePath>/readyz`, and the readiness probe uses
it. It gates on the auth runtime and one OIDC discovery. It does NOT check Redis, by decision
(SMA-705 D1): every console pod shares one Redis, so a Redis fault would take every pod out of
rotation. SMA-653's store 503 page answers a store fault.
```

In `WT/docs/superpowers/specs/2026-09-09-sma-506-auth-design.md`, replace:

```md
`session.resolve_failed`, `logout.completed`, `store.unavailable`, `oidc.discovery_failed`.
```

with:

```md
`session.resolve_failed`, `logout.completed`, `store.unavailable`, `oidc.discovery_failed`,
`readiness.runtime_failed`.
```

and replace:

```md
- `oidc.discovery_failed` — `{ zone, stage, reason }`: a login or a callback needed the OIDC
  discovery document, and this process could not get it (SMA-656). `stage` is `login` or
  `callback`; `reason` is a closed list. Its absence does not mean that the IdP is healthy.
```

with:

```md
- `oidc.discovery_failed` — `{ zone, stage, reason }`: a login, a callback or the readiness route
  needed the OIDC discovery document, and this process could not get it (SMA-656, SMA-705).
  `stage` is `login`, `callback` or `readiness`; `reason` is a closed list. Its absence does not
  mean that the IdP is healthy.
- `readiness.runtime_failed` — `{ error }`: the readiness route could not build the auth runtime
  (SMA-705). `error` is the caught error's `name` only, never its message.
```

- [ ] **Step 7: Format and commit**

Run (from `WT`): `moon run ts:fmt --force`
Expected: PASS. The files outside `ts/` have no Prettier gate.

```bash
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness branch --show-current
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness add ts/packages/paigasus-auth/README.md docs/ops/RUNBOOK-containers.md docs/ops/RUNBOOK-chart.md ci/kind/README.md docs/superpowers/specs/2026-09-26-sma-656-auth-login-oidc-503-design.md docs/superpowers/specs/2026-09-19-sma-513-multi-zone-ingress-helm-design.md docs/superpowers/specs/2026-09-09-sma-506-auth-design.md
git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness commit -m "docs(repo): document the console readiness route (SMA-705)" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 11: The mutation battery (spec § 5.6)

**Files:** none change at the end of this task. Every mutation is restored by an edit.

For each mutation: make the edit with the Edit tool, run the named command, record which tests failed, then restore the exact original text with the Edit tool. Do NOT use `git checkout --`, `git stash` or `git reset`: they also discard work. A mutation that leaves every listed test green is a FINDING: stop and report it. Do not weaken a test. If you fix anything, run the WHOLE battery again from Step 1, because a fix can make an earlier row inert. Record each result for the PR description.

Commands:
- **AUTH** (a vitest battery): `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/packages/paigasus-auth exec vitest run tests/adapters/oidc.test.ts tests/http`
- **IAM-T15**: from `WT`, `moon run iam-console-ts:build --force`, then `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/apps/iam-console exec vitest run tests/standalone-runtime.test.ts`
- **GW-T15**: from `WT`, `moon run gateway-console-ts:build --force`, then `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/apps/gateway-console exec vitest run tests/standalone-runtime.test.ts`
- **T16**: from `WT`, `moon run iam-console-ts:build --force`, then `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/apps/iam-console exec playwright test tests/e2e/harness.spec.ts`
- **T17**: from `WT`, `export PROTO_REPORTER=text && /bin/bash charts/paigasus/tests/render.sh`

- [ ] **Step 1: Run the clean battery once**

Run AUTH, IAM-T15, GW-T15, T16 and T17.
Expected: PASS for each.

- [ ] **Step 2: M1 — `discoveryStatus()` never returns `'discovered'`**

Edit `PKG/src/adapters/oidc.ts`: delete the line `      if (discovered) return 'discovered';`.
Run AUTH. Must fail: T1, T4, T13a. Run T16. Must fail: T16 (the last status is 503).
Restore the line by an edit.

- [ ] **Step 3: M2 — the `.catch` in `getConfig` does not clear `configPromise`**

Edit: delete the line `        configPromise = undefined;` in `getConfig`.
Run AUTH. Must fail: T2 (the status stays `discovering`). T13b and the SMA-656 retry row fail too.
Restore the line by an edit.

- [ ] **Step 4: M3 — `discover()` calls `client.discovery` directly**

Edit: in `discover()`, replace the line `      await getConfig();` with:

```ts
      await client.discovery(new URL(opts.issuer), opts.clientId, secret.reveal(), undefined, { execute: opts.allowInsecureRequests === true ? [client.allowInsecureRequests] : [] });
```

Run AUTH. Must fail: T3 (2 requests), T4 (2 requests).
Restore the line `      await getConfig();` by an edit.

- [ ] **Step 5: M4 — the handler answers 200 for `'discovering'`**

Edit `PKG/src/http/readiness.ts`: replace `  if (status === 'discovered') return probeResponse(200, 'ready');` with `  if (status !== 'idle') return probeResponse(200, 'ready');`.
Run AUTH. Must fail: T6.
Restore the line by an edit.

- [ ] **Step 6: M5 — the handler does not start `discover()` for `'idle'`**

Edit: delete the whole block from `  if (status === 'idle') {` to its closing `  }` (the block that holds `runtime.oidc.discover().catch(`).
Run AUTH. Must fail: T7 (0 calls), T13b (no event), the D8 overlap row.
Run IAM-T15 and GW-T15. Must fail: T15 in both apps (no `oidc.discovery_failed` line).
Restore the block by an edit, exactly as Task 3 Step 3 wrote it.

- [ ] **Step 7: M6 — the handler starts `discover()` for `'discovering'` too**

Edit: replace `  if (status === 'idle') {` with `  if (status === 'idle' || status === 'discovering') {`.
Run AUTH. Must fail: T6 (1 call).
Restore the line by an edit.

- [ ] **Step 8: M7 — the handler awaits `discover()`**

Edit: replace `    runtime.oidc.discover().catch((err: unknown) => {` with `    await runtime.oidc.discover().catch((err: unknown) => {`.
Run AUTH. Must fail: T7 (`timed_out`). T8, T9, T10 and T12 can also fail by the vitest timeout, because the fake settles only after the handler returns. That is expected.
Restore the line by an edit.

- [ ] **Step 9: M8 — the discovery event holds `String(err)`**

Edit `PKG/src/http/discovery-log.ts`: replace `{ zone: runtime.zone, stage, reason: oidcDiscoveryReason(err) }` with `{ zone: runtime.zone, stage, reason: oidcDiscoveryReason(err), error: String(err) }`.
Run AUTH. Must fail: T8 (the exact fields and `expectEventsClean`), and the SMA-656 T4 and T13 rows.
Restore by an edit.

- [ ] **Step 10: M9 — the event logs `err.reason` without `oidcDiscoveryReason`**

Edit: replace `reason: oidcDiscoveryReason(err)` in `discovery-log.ts` with `reason: (err as { reason: string }).reason`.
Run AUTH. Must fail: T9 (the plain `Error` row: the reason is `undefined`, not `other`), and the second `discovery-log.test.ts` row.
Restore by an edit.

- [ ] **Step 11: M10 — the handler logs only for `instanceof OidcDiscoveryFailed`**

Edit `PKG/src/http/readiness.ts`: add the line `import { OidcDiscoveryFailed } from '../core/errors';` after `import { logDiscoveryFailed } from './discovery-log';`, and replace `        logDiscoveryFailed(runtime, 'readiness', err);` with `        if (err instanceof OidcDiscoveryFailed) logDiscoveryFailed(runtime, 'readiness', err);`.
Run AUTH. Must fail: T9 (the second-module-copy row and the plain `Error` row: no event).
Restore both lines by an edit (delete the added import).

- [ ] **Step 12: M11 — the handler rethrows in its `.catch`**

Edit: directly after the closing `      }` of the inner `catch { … }` block in the discovery `.catch`, insert the line `      throw err;`.
Run AUTH. Must fail: T8, T9 and T13b at `expect(unhandled).toEqual([])`. Vitest also reports the unhandled rejection.
Restore by deleting the inserted line.

- [ ] **Step 13: M12 — the handler does not wrap the log call in `try`**

Edit: replace

```ts
      try {
        logDiscoveryFailed(runtime, 'readiness', err);
      } catch {
        // D11. A logger that throws must not make an unhandled rejection. Nothing can log it.
      }
```

with

```ts
      logDiscoveryFailed(runtime, 'readiness', err);
```

Run AUTH. Must fail: T12.
Restore the five lines by an edit.

- [ ] **Step 13b: M18 — `AuthConfigError` sets no `name`**

Edit: in `PKG/src/core/errors.ts`, delete the line `    this.name = 'AuthConfigError';`.
Run: `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/packages/paigasus-auth exec vitest run tests/core/errors.test.ts tests/http/readiness.test.ts` (NOT AUTH: AUTH does not hold `tests/core/`). Must fail: the `AuthConfigError (SMA-705 deviation 3)` row in `tests/core/errors.test.ts`, and the `AuthConfigError` row in `tests/http/readiness.test.ts`.
Restore the line by an edit.

- [ ] **Step 14: M13 — the handler does not catch a runtime-getter failure**

Edit: insert the line `    throw err;` as the first line inside `  } catch (err) {` of the `getRuntime` try.
Run AUTH. Must fail: T11 (both rows), T11b, the `AuthConfigError` row.
Restore by deleting the inserted line.

- [ ] **Step 15: M14 — the runtime-failure event holds `String(err)`**

Edit: replace `{ error: errorName(err) }` with `{ error: String(err) }`.
Run AUTH. Must fail: T11 (the fields and `sentinel-705`), T11b.
Restore by an edit.

- [ ] **Step 16: M15 — `'/readyz'` removed from `publicPaths` in the IAM console**

Edit `WT/ts/apps/iam-console/proxy.ts`: replace `  publicPaths: [...authRoutePaths(), '/', '/healthz', '/readyz'],` with `  publicPaths: [...authRoutePaths(), '/', '/healthz'],`.
Run `pnpm -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness/ts/apps/iam-console exec vitest run tests/unit/proxy.test.ts`. Must fail: T14 (`lets /iam/readyz through with no cookie`).
Run IAM-T15. Must fail: T15 (a redirect status and a `location` header).
Restore the line by an edit.

- [ ] **Step 17: M16 — delete `app/readyz/route.ts` in the IAM console**

Delete `WT/ts/apps/iam-console/app/readyz/route.ts` with `rm`.
Run IAM-T15. Must fail: T15 (404).
Restore the file with the Write tool, with exactly the content of Task 5 Step 3.
Run: `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness diff --stat -- ts/apps/iam-console/app/readyz/route.ts`
Expected: no output (the restored file equals the committed one).

- [ ] **Step 18: M17 — the chart keeps `/healthz` as the readiness path**

Edit `WT/charts/paigasus/templates/console-deployment.yaml`: replace `              path: {{ $z.basePath }}/readyz` with `              path: {{ $z.basePath }}/healthz`.
Run T17. Must fail: both golden rows.
Restore the line by an edit.

- [ ] **Step 19: Confirm that every mutation is restored, and rebuild clean**

Run: `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-705-oidc-readiness status --short`
Expected: no output.

Run AUTH, IAM-T15, GW-T15, T16 and T17 again (the IAM-T15, GW-T15 and T16 commands rebuild with `--force`, so no mutated build stays in `.next`).
Expected: PASS for each.

- [ ] **Step 20: Report**

Report the table M1–M18 with the tests that failed for each row. There is nothing to commit in this task.

---

### Task 12: Full local verification

**Files:** none.

- [ ] **Step 1: The touched Moon projects**

Run (from `WT`): `moon run paigasus-auth-ts:test paigasus-auth-ts:typecheck paigasus-console-core-ts:typecheck iam-console-ts:typecheck iam-console-ts:test gateway-console-ts:typecheck gateway-console-ts:test ts:lint ts:fmt --force`
Expected: PASS. If a task fails, capture `.moon/cache/ciReport.json` and `.moon/cache/states/<project>/<task>/` outside the repository BEFORE any re-run (root `CLAUDE.md`, "Diagnosing an unattributed `moon ci` failure", step 0). Then report the failure.

- [ ] **Step 2: The e2e tiers**

Run (from `WT`): `moon run iam-console-ts:test-e2e --force`
Expected: PASS (every row, T16 included). This tier needs no Docker.

If Docker is running: `moon run gateway-console-ts:test-e2e paigasus-auth-ts:test-e2e --force`. Expected: PASS. If Docker is not running, record that these two tiers ran only in CI. Do not start or install Docker.

- [ ] **Step 3: The chart and release gates**

Run (from `WT`), each with `export PROTO_REPORTER=text &&` first:
- `/bin/bash charts/paigasus/tests/render.sh`
- `/bin/bash ci/helm-render/run.sh --self-test`
- `/bin/bash ci/helm-render/run.sh --negative-control`
- `/bin/bash ci/helm-render/run.sh`
- `bash ci/release-plan/run.sh --assert`

Expected: exit 0 for each.

- [ ] **Step 4: Before a push**

Run the full gate graph that the root `CLAUDE.md` names between its `ci-targets` markers, with `--base origin/main --include-relations`. Read the root `CLAUDE.md` "This development Mac only" section first: no single local bash runs every gate. Re-run a gate that needs the other bash directly, as that section says, and read its result in place of the `moon ci` verdict.

- [ ] **Step 5: What only CI can check**

Record these for the PR description:
- **The `chart.yml` kind job.** It installs the chart with `helm install --wait` against Keycloak, with the locally built `:dev` images. It is the only proof that a real pod reaches ready through `/readyz` with a real IdP and a real CA. It is not a required check: the PR must cite a green run of it before merge (spec § 8).
- **The `0.2.0` images.** They exist only after the release (SMA-688). Between the merge and the release, the chart on `main` pins a tag that does not exist yet (spec § 9).
- **`repo:actionlint` check 11** runs `release_plan.py --assert` on the PR. Step 3 ran the same assertion locally.

---

## Self-Review

**Spec coverage.**

| Spec item | Where |
|---|---|
| A1 (no traffic before runtime and discovery) | Task 3 (T5–T13b), Tasks 5–6 (T15), Task 7 (T16), Task 8 (probe path) |
| A2 (the chart uses the route; the pinned tag serves it) | Task 8 (T17), Task 9 (`0.2.0`, row 8a) |
| A3 (nothing leaks into a probe response; the log has only the reason or the name) | Task 3 (T8, T9, T11 with sentinels), Task 11 M8, M14 |
| A4 (tests through the built app; a mutation table) | Tasks 5–7 (T15, T16), Task 11 (M1–M18) |
| D1 (no Redis) | By construction in Task 3; comments in Tasks 3 and 8; docs in Task 10 |
| D2 (sticky) | Task 1 (T4), Task 3 (T5), header in Task 3 |
| D3 (never waits) | Task 3 (T7), Task 11 M7 |
| D4 (no eager discovery) | Nothing added, by design |
| D5 (logic once, in `@paigasus/auth`) | Tasks 3, 4; route files in Tasks 5, 6 |
| D6 (`/readyz`; `/healthz` unchanged) | Tasks 5, 6 (the `/healthz` rows still pass), Task 10 |
| D7 (two bodies, `no-store`) | Task 3 (`expectProbe` exact body), Tasks 5–7 |
| D8 (`readiness` stage; one helper; one event per caller) | Task 2, Task 3 (T8, D8 overlap row) |
| D9 (public path) | Tasks 5, 6 (T14, T15), Task 11 M15 |
| D10 (runtime failure: 503 and the name only) | Task 3 (T11, T11b, `AuthConfigError` row), Task 11 M13, M14 |
| D11 (no throw from the detached promise) | Task 3 (T12), Task 11 M11, M12 |
| § 4.1 adapter and comments | Task 1 |
| § 4.2 logger | Task 2 |
| § 4.3 helper, SMA-656 tests unchanged | Task 2 (Step 6 diff check) |
| § 4.4 handler, server exports | Tasks 3, 4 |
| § 4.5 routes and `proxy.ts` | Tasks 5, 6 |
| § 4.6 chart, fixtures, goldens | Task 8 |
| § 5.1 T1–T4 and the fixture counter | Task 1 |
| § 5.2 the five fakes | Task 1 Step 7 |
| § 5.3 T5–T12 | Task 3 |
| § 5.4 T13a, T13b | Task 3 |
| § 5.5 T14, T15, T16, T17 | Tasks 5, 6, 7, 8 |
| § 5.6 mutation table (17 spec rows plus M18 (a plan addition for deviation 3)) | Task 11 (M1–M18, one per row) |
| § 7 documentation | Task 10, plus the code comments in Tasks 1–3, 5, 6, 8 |
| § 8 kind | Task 12 Step 5 (cite the green run) |
| § 9 rollout | Task 9 |
| § 10 out of scope | Not implemented, by design |

**Placeholder scan.** No step says "TBD", "similar to Task N" or "add error handling". Each code step shows the full code. The conditional instructions are STOP rules: the `reason` rows (Tasks 3, 5, 6), the `AuthConfigError` value (Task 3), the T16 harness state (Task 7), fixture drift and helm version (Task 8), and an unexpected golden diff (Tasks 8, 9).

**Type and name consistency.**
- `OidcDiscoveryStatus`, `discoveryStatus()`, `discover()`: defined in Task 1; used in Tasks 3, 4, 11.
- `OidcFixture.discoveryRequests()`: defined in Task 1; used in Tasks 1 and 3.
- `FakeOidc.status`, `discoverCalls`, `resolveDiscover`, `rejectDiscover(err: Error)`: defined in Task 1; used in Task 3.
- `OidcDiscoveryStage` with `'readiness'`, `'readiness.runtime_failed'`, `logDiscoveryFailed(runtime, stage, err)`: defined in Task 2; used in Tasks 3 and 11.
- `readinessResponse(getRuntime, logger)`: defined in Task 3; exported in Task 4; used in Tasks 5, 6.
- The log line fields `{ zone, stage: 'readiness', reason }` and `{ error }` are the same in Tasks 2, 3, 5, 6, 9 and 10.

**Review Focus check.** Each of the five lines has its test in the owning task: 1 in Tasks 5 and 6, 2 in Tasks 1 and 3, 3–5 in Task 3.
