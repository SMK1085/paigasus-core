<!-- SPDX-License-Identifier: Apache-2.0 -->

# SMA-627 — refuse a caller-supplied `authorization` header — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `@paigasus/sdk` refuse a caller-supplied `authorization` header on both `Auth` arms, with a `ConnectError` this package's own error map renders as `invalid-input`.

**Architecture:** One check at the top of `authInterceptor` in `ts/packages/paigasus-sdk/src/transport.ts`, before it reads the bound `Auth`. The interceptor sits on the cached transport, so the rule reaches every client and every call, unary and streaming alike. No public type changes; no new files.

**Tech Stack:** TypeScript (ESM, Node ≥ 24), `@connectrpc/connect` 2.2.0, vitest 5, pnpm, Moon.

**Spec:** `docs/superpowers/specs/2026-09-10-sma-627-anonymous-authorization-header-design.md` (revision 2). Read it before Task 1 — every task cites its sections.

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`. Both files this plan touches already have one; do not add a second.
- Prettier: `printWidth: 200`, `singleQuote: true`, `semi: true`, `trailingComma: 'all'`, `arrowParens: 'always'` (`ts/.prettierrc.js`). `ts:fmt` is a separate whole-tree CI gate — run it after touching any `.ts` file.
- eslint enforces `@typescript-eslint/require-await`: an `async` function with no `await` is an error. Test stand-ins return `Promise.resolve(...)` instead.
- **Do not use the name `HeadersInit` in this package.** Its tsconfig deliberately excludes the DOM lib, so the name does not resolve — `tests/iam.test.ts:18-23` records this. Build a `Headers` and mutate it instead.
- Branch: `feature/sma-627-anonymous-client-authorization-header` (already checked out, in the worktree `.claude/worktrees/sma-627`).
- Conventional commits with a workspace scope: `feat(ts):` / `fix(ts):` / `docs(ts):`.
- commitlint enforces `body-max-line-length: 100`. Wrap commit-message bodies by hand.
- Run all commands from the worktree root. Prefix with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so the pinned toolchain resolves.

**Baseline:** `pnpm exec vitest run` in `ts/packages/paigasus-sdk` reports **236 tests in 12 files, all pass**. Every task re-runs the whole package suite, not only its own file.

**One deliberate deviation from the spec.** Spec § 5 says `fakeUnaryRequest()` "gains an optional `headers?: HeadersInit` parameter". This plan does **not** do that, because `HeadersInit` does not resolve in this package (see Global Constraints). Tests set the header after construction — `req.header.set('authorization', …)` — which needs no helper change and models the same thing: M1 establishes that `CallOptions.headers` *becomes* `req.header`, so writing on `req.header` is writing what a caller sent. `fakeStreamRequest()` in Task 3 is still added, since no existing helper builds a stream request.

---

### Task 1: Give the existing empty-bearer refusal a `ConnectError`

Spec § 3.1. This lands first so Task 2 writes its new throw in the final form rather than converting it a task later. It changes no behaviour a caller can observe except the error's type and code.

**Files:**
- Modify: `ts/packages/paigasus-sdk/src/transport.ts:1-8` (imports), `:56-60` (the throw)
- Test: `ts/packages/paigasus-sdk/tests/transport.test.ts` (add a helper + one test)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `rejection(promise: Promise<unknown>): Promise<unknown>` — a test helper that returns the rejection reason and fails loudly if the promise resolves. Tasks 2 and 3 use it. `ConnectError` and `Code` become value imports in both files.

- [ ] **Step 1: Write the failing test**

In `ts/packages/paigasus-sdk/tests/transport.test.ts`, extend the two existing imports at the top of the file:

```ts
import { Code, ConnectError, createContextValues } from '@connectrpc/connect';
import { authContextKey, authInterceptor, disposeTransports, getTransport, stableTransportKey } from '../src/transport.js';
import { presentationForGrpcCode } from '../src/errors.js';
```

Add this helper directly above the `describe('authInterceptor (spec § 7.4)', …)` block (below `noopNext`):

```ts
/**
 * The rejection reason of a promise, or a hard failure if it resolved.
 *
 * `expect(...).rejects.toThrow(/re/)` cannot assert on a thrown value's TYPE or its `code`, and a
 * bare `.catch(e => e)` cannot tell a rejection from a resolution. This does both.
 */
async function rejection(promise: Promise<unknown>): Promise<unknown> {
  try {
    await promise;
  } catch (error: unknown) {
    return error;
  }
  throw new Error('expected the call to reject, but it resolved');
}
```

Add this test inside the existing `describe('authInterceptor (spec § 7.4)', …)` block, after the whitespace-only bearer test:

```ts
  it('refuses an empty bearer with Code.InvalidArgument, so the error map reports invalid-input', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { bearer: '' });

    // A plain Error reaches the caller as ConnectError(Code.Unknown), and
    // src/errors/transport-status.ts has NO Unknown row — presentationForGrpcCode falls through to
    // `generic`, so the SDK would render a CALLER error as an unclassified SERVICE failure
    // (spec § 3.1). The presentation assertion is what pins the reason, not just the code.
    const error = await rejection(authInterceptor(noopNext)(req));
    expect(error).toBeInstanceOf(ConnectError);
    expect((error as ConnectError).code).toBe(Code.InvalidArgument);
    expect(presentationForGrpcCode((error as ConnectError).code)).toBe('invalid-input');
  });
```

- [ ] **Step 2: Run the test and verify it fails**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; cd ts/packages/paigasus-sdk && pnpm exec vitest run tests/transport.test.ts`

Expected: FAIL on the new test with `expected Error … to be an instance of ConnectError`. The five pre-existing `authInterceptor` tests still pass.

- [ ] **Step 3: Change the throw**

In `ts/packages/paigasus-sdk/src/transport.ts`, change the first import line from

```ts
import { createContextKey } from '@connectrpc/connect';
```

to

```ts
import { Code, ConnectError, createContextKey } from '@connectrpc/connect';
```

Then replace the empty-bearer throw (currently `throw new Error(` at `:57`) with:

```ts
      // A ConnectError with Code.InvalidArgument, not a plain Error. A plain Error reaches the
      // caller as ConnectError(Code.Unknown), and src/errors/transport-status.ts has no Unknown
      // row, so presentationForGrpcCode falls through to `generic` — the SDK would blame the
      // service for the caller's own input, the defect src/chat.ts:218-220 records and fixed for
      // an unserializable request body (spec § 3.1).
      throw new ConnectError(
        '@paigasus/sdk: refusing to send an empty or whitespace-only bearer token. This usually means an unset environment variable; use { anonymous: true } for an intentionally unauthenticated call.',
        Code.InvalidArgument,
      );
```

The message is unchanged, so the two pre-existing `.rejects.toThrow(/empty|whitespace/)` tests keep passing — `ConnectError` preserves the message, prefixing it with `[invalid_argument] `.

- [ ] **Step 4: Run the whole package suite and verify it passes**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; cd ts/packages/paigasus-sdk && pnpm exec vitest run`

Expected: PASS, **237 tests in 12 files**. If any pre-existing test now fails, stop — the message changed when it should not have.

- [ ] **Step 5: Format and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && pnpm exec prettier --write packages/paigasus-sdk/src/transport.ts packages/paigasus-sdk/tests/transport.test.ts && cd ..
git add ts/packages/paigasus-sdk/src/transport.ts ts/packages/paigasus-sdk/tests/transport.test.ts
git commit -m "fix(ts): report an empty @paigasus/sdk bearer as invalid-input, not a service fault (SMA-627)" -m "A plain Error reaches the caller as ConnectError(Code.Unknown), for which
src/errors/transport-status.ts has no row, so the presentation fell through to
generic — blaming the service for the caller's own input." -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Refuse a caller-supplied `authorization` header

Spec § 3, § 3.3, § 3.4, § 3.5, § 3.6. The core of the issue.

**Files:**
- Modify: `ts/packages/paigasus-sdk/src/transport.ts:49` (the top of `authInterceptor`)
- Test: `ts/packages/paigasus-sdk/tests/transport.test.ts` (one new `describe` block)

**Interfaces:**
- Consumes: `rejection()` and the `Code` / `ConnectError` / `presentationForGrpcCode` imports from Task 1.
- Produces: no new exported names. `authInterceptor` keeps its `Interceptor` type.

- [ ] **Step 1: Write the failing tests**

Add this whole `describe` block to `ts/packages/paigasus-sdk/tests/transport.test.ts`, after the existing `describe('authInterceptor (spec § 7.4)', …)` block:

```ts
describe('a caller-supplied authorization header is refused (SMA-627 spec § 3)', () => {
  it('refuses it on the anonymous arm, rather than forwarding a credential from a client declared unauthenticated', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { anonymous: true });
    req.header.set('authorization', 'Bearer caller-token');
    await expect(authInterceptor(noopNext)(req)).rejects.toThrow(/authorization/);
  });

  it('refuses it on the bearer arm, rather than silently overwriting it', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { bearer: 'sdk-token' });
    req.header.set('authorization', 'Bearer caller-token');
    await expect(authInterceptor(noopNext)(req)).rejects.toThrow(/authorization/);
  });

  // Headers.has is case-insensitive by construction, so these two cannot fail against any
  // plausible implementation. Kept as defence in depth, NOT counted as coverage (spec § 5.1).
  it.each(['Authorization', 'AUTHORIZATION'])('refuses it spelled %s', async (name) => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { anonymous: true });
    req.header.set(name, 'Bearer caller-token');
    await expect(authInterceptor(noopNext)(req)).rejects.toThrow(/authorization/);
  });

  // A present-but-empty header is still the caller reaching around the binding, and Headers.has
  // reports it as present (spec § 3.4). The bearer row additionally proves the check runs BEFORE
  // req.header.set — after it, the header would be non-empty and the case would be meaningless.
  it.each([
    ['anonymous', { anonymous: true }],
    ['bearer', { bearer: 'sdk-token' }],
  ] as const)('refuses a present-but-empty header on the %s arm', async (_label, auth) => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, auth);
    req.header.set('authorization', '');
    await expect(authInterceptor(noopNext)(req)).rejects.toThrow(/authorization/);
  });

  it('reports the HEADER, not the empty bearer, when a request is wrong in both ways', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { bearer: '' });
    req.header.set('authorization', 'Bearer caller-token');

    // The mutation this detects: a check placed AFTER the contextValues.get branch reports the
    // empty bearer instead, and the ordering spec § 3.6 fixes becomes accidental.
    const message = ((await rejection(authInterceptor(noopNext)(req))) as Error).message;
    expect(message).toMatch(/authorization/);
    expect(message).not.toMatch(/empty|whitespace/);
  });

  it('refuses with Code.InvalidArgument, so the error map reports invalid-input', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { anonymous: true });
    req.header.set('authorization', 'Bearer caller-token');

    const error = await rejection(authInterceptor(noopNext)(req));
    expect(error).toBeInstanceOf(ConnectError);
    expect((error as ConnectError).code).toBe(Code.InvalidArgument);
    expect(presentationForGrpcCode((error as ConnectError).code)).toBe('invalid-input');
  });

  it('names the cause and both remedies, and NEVER echoes the credential', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { anonymous: true });
    req.header.set('authorization', 'Bearer super-secret-value');

    // The mutation this detects: `...: ${req.header.get('authorization')}`, the obvious
    // debugging-friendly form, which writes a live credential into an exception message, a
    // container log and any error reporter (spec § 3.5).
    const message = ((await rejection(authInterceptor(noopNext)(req))) as Error).message;
    expect(message).toMatch(/authorization/);
    expect(message).toMatch(/bearer/i);
    expect(message).toMatch(/anonymous/);
    expect(message).toMatch(/proxy-authorization/);
    expect(message).not.toContain('super-secret-value');
  });

  it('leaves proxy-authorization untouched on the anonymous arm, and adds no authorization', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { anonymous: true });
    req.header.set('proxy-authorization', 'Basic Zm9vOmJhcg==');

    // A RECORDING next, not the shared noopNext: noopNext returns the SAME Headers object it was
    // given (tests/transport.test.ts:120), so asserting on the returned header would prove nothing
    // about `next` having been called at all.
    const seen: (string | null)[] = [];
    const recordingNext: Parameters<typeof authInterceptor>[0] = (r) => {
      seen.push(r.header.get('proxy-authorization'));
      return Promise.resolve({ stream: false, header: r.header } as unknown as UnaryResponse);
    };

    await authInterceptor(recordingNext)(req);
    expect(seen).toEqual(['Basic Zm9vOmJhcg==']);
    expect(req.header.get('authorization')).toBeNull();
  });

  it('claims exactly one header: proxy-authorization, cookie and a custom header survive a bearer call', async () => {
    const req = fakeUnaryRequest();
    req.contextValues.set(authContextKey, { bearer: 'sdk-token' });
    req.header.set('proxy-authorization', 'Basic Zm9vOmJhcg==');
    req.header.set('cookie', 'sid=abc');
    req.header.set('x-paigasus-probe', 'kept');

    // The mutation these detect: a check written on a substring or a regex rather than an exact
    // field name, which would refuse proxy-authorization too (spec § 3.3).
    await authInterceptor(noopNext)(req);
    expect(req.header.get('authorization')).toBe('Bearer sdk-token');
    expect(req.header.get('proxy-authorization')).toBe('Basic Zm9vOmJhcg==');
    expect(req.header.get('cookie')).toBe('sid=abc');
    expect(req.header.get('x-paigasus-probe')).toBe('kept');
  });
});
```

- [ ] **Step 2: Run the tests and verify they fail**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; cd ts/packages/paigasus-sdk && pnpm exec vitest run tests/transport.test.ts`

Expected: FAIL. The seven refusal cases fail with `expected … to reject, but it resolved` or an unmet `.rejects` assertion. The two pass-through cases (`proxy-authorization` … and `claims exactly one header` …) **already pass** — they assert today's behaviour and must keep passing after Step 3. If either of those two fails here, stop and fix the test before implementing.

- [ ] **Step 3: Add the check**

In `ts/packages/paigasus-sdk/src/transport.ts`, replace the first line of the interceptor body — currently `const auth = req.contextValues.get(authContextKey);` at `:50` — with the block below, keeping the rest of the function unchanged:

```ts
  // The SDK owns `authorization`. A request that reaches here already carrying one is refused on
  // BOTH Auth arms (spec § 3): on the anonymous arm the header would otherwise be forwarded from a
  // client explicitly declared unauthenticated, and on the bearer arm the `set` below would
  // silently overwrite it without a word. One check closes both.
  //
  // Checked BEFORE contextValues.get, deliberately (spec § 3.6). A request that is wrong in both
  // ways then reports the header deterministically rather than depending on an evaluation order
  // nobody wrote down, and the check can never trip over the header the bearer branch itself
  // writes three lines below.
  //
  // ConnectError with Code.InvalidArgument, not a plain Error, for the reason given on the
  // empty-bearer throw below: Code.Unknown has no row in src/errors/transport-status.ts and
  // presents as `generic`, blaming the service for a caller error (spec § 3.1).
  //
  // The message must NEVER carry the header's VALUE — that would put a live credential into an
  // exception message, a container log and any error reporter (spec § 3.5). It names
  // `proxy-authorization` because that field stays untouched and is the way out for a credential
  // aimed at an intermediary; note that `Bearer` is consequently the only Authorization scheme
  // this client can send at all (spec § 3.3).
  //
  // This reasoning holds only while `authInterceptor` is the WHOLE interceptor array. Connect
  // applies the interceptor at the END of the array first, so one appended after this that set an
  // `authorization` header would run first and trip this refusal against the SDK's own writing.
  // tests/transport-wiring.test.ts:49 pins `interceptors` to exactly [authInterceptor]; that pin
  // is this invariant's guard.
  if (req.header.has('authorization')) {
    throw new ConnectError(
      "@paigasus/sdk: refusing a caller-supplied `authorization` header — this client owns it. Pass the credential as { bearer } to the client factory, or use { anonymous: true } for an unauthenticated call. Do not forward an incoming request's headers wholesale; send the session-bound token instead. A credential for an intermediary belongs in `proxy-authorization`, which this client does not touch.",
      Code.InvalidArgument,
    );
  }

  const auth = req.contextValues.get(authContextKey);
```

- [ ] **Step 4: Run the whole package suite and verify it passes**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; cd ts/packages/paigasus-sdk && pnpm exec vitest run`

Expected: PASS, **248 tests in 12 files** (237 after Task 1, plus 11 — nine test blocks: seven plain `it`, and two `it.each` blocks contributing 2 runs each).

- [ ] **Step 5: Format, typecheck and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && pnpm exec prettier --write packages/paigasus-sdk/src/transport.ts packages/paigasus-sdk/tests/transport.test.ts && cd ..
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-sdk && pnpm exec tsc -p tsconfig.json --noEmit && cd ../../..
git add ts/packages/paigasus-sdk/src/transport.ts ts/packages/paigasus-sdk/tests/transport.test.ts
git commit -m "feat(ts): refuse a caller-supplied authorization header in @paigasus/sdk (SMA-627)" -m "The client owns the header. A request reaching authInterceptor already carrying
one is refused on both Auth arms — closing a silent forward on the anonymous arm
and a silent overwrite on the bearer arm. proxy-authorization stays untouched." -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 3: Cover the streaming path

Spec M5, § 5 case 10. Forward cover: no IAM service declares a streaming RPC today. It exists because an implementer worried about streaming could write `if (!req.stream && req.header.has(…))` and every case in Task 2 would still pass.

**Files:**
- Test: `ts/packages/paigasus-sdk/tests/transport.test.ts` (a helper and two tests)

**Interfaces:**
- Consumes: `authContextKey`, `authInterceptor`, `noopNext` from the existing file.
- Produces: `fakeStreamRequest(): StreamRequest` — a stream-shaped request stand-in.

- [ ] **Step 1: Write the failing tests**

Extend the type-only import at the top of `ts/packages/paigasus-sdk/tests/transport.test.ts`:

```ts
import type { StreamRequest, UnaryRequest, UnaryResponse } from '@connectrpc/connect';
```

Add this helper directly below `fakeUnaryRequest`:

```ts
/**
 * The same stand-in with `stream: true`. `authInterceptor` is an `Interceptor`, whose `next` takes
 * `UnaryRequest | StreamRequest`, and createGrpcTransport installs it on both arms — so a rule that
 * checked `req.stream` would apply to half the surface (spec M5).
 */
function fakeStreamRequest(): StreamRequest {
  return {
    stream: true,
    header: new Headers(),
    contextValues: createContextValues(),
  } as unknown as StreamRequest;
}
```

Add these two tests at the end of the `describe('a caller-supplied authorization header is refused (SMA-627 spec § 3)', …)` block from Task 2:

```ts
  it('refuses it on the STREAMING path too', async () => {
    const req = fakeStreamRequest();
    req.contextValues.set(authContextKey, { anonymous: true });
    req.header.set('authorization', 'Bearer caller-token');

    // The mutation this detects: `if (!req.stream && req.header.has(...))`, which every unary case
    // above still passes.
    await expect(authInterceptor(noopNext)(req)).rejects.toThrow(/authorization/);
  });

  it('still binds the bearer on the STREAMING path when no caller header is present', async () => {
    const req = fakeStreamRequest();
    req.contextValues.set(authContextKey, { bearer: 'sdk-token' });
    await authInterceptor(noopNext)(req);
    expect(req.header.get('authorization')).toBe('Bearer sdk-token');
  });
```

- [ ] **Step 2: Run the tests and verify they behave as expected**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; cd ts/packages/paigasus-sdk && pnpm exec vitest run tests/transport.test.ts`

Expected: **both PASS immediately.** Task 2's implementation has no `req.stream` guard, so it already covers streaming. That is the correct outcome — these are regression tests, not a red-green cycle, and the plan says so rather than inventing a failure.

- [ ] **Step 3: Prove the tests are not vacuous**

Temporarily change the check in `ts/packages/paigasus-sdk/src/transport.ts` from

```ts
  if (req.header.has('authorization')) {
```

to

```ts
  if (!req.stream && req.header.has('authorization')) {
```

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; cd ts/packages/paigasus-sdk && pnpm exec vitest run tests/transport.test.ts`

Expected: FAIL, on `refuses it on the STREAMING path too` and on **that test alone** — every unary case still passes, which is the whole reason this task exists.

**Then revert the mutation.** Restore the line by editing it back to `if (req.header.has('authorization')) {`. Do **not** use `git checkout --` on the file: it would also discard Task 2's committed-but-unformatted state if anything is uncommitted, and this repo has been bitten by that before. Re-run the suite and confirm both streaming tests pass again before Step 4.

- [ ] **Step 4: Run the whole package suite and verify it passes**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; cd ts/packages/paigasus-sdk && pnpm exec vitest run`

Expected: PASS, **250 tests in 12 files**.

- [ ] **Step 5: Format and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && pnpm exec prettier --write packages/paigasus-sdk/tests/transport.test.ts && cd ..
git status --short   # must show ONLY tests/transport.test.ts — if src/transport.ts appears, the Step 3 mutation was not reverted
git add ts/packages/paigasus-sdk/tests/transport.test.ts
git commit -m "test(ts): cover the streaming path for the @paigasus/sdk authorization refusal (SMA-627)" -m "Every case so far used a stream: false stand-in, so a req.stream guard on the
check would have passed all of them. Verified by mutation: the guard reds this
test and no other." -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 4: Put the rule where a consumer will read it

Spec § 4, § 6. A consumer reads the source and the type, not `docs/superpowers/specs/`.

**Files:**
- Modify: `ts/packages/paigasus-sdk/src/transport.ts:26-32` (the `Auth` doc comment)
- Modify: `docs/superpowers/specs/2026-09-09-sma-508-sdk-design.md` (end of § 7.4)

**Interfaces:**
- Consumes: nothing. Comments and prose only; no code changes.
- Produces: nothing.

- [ ] **Step 1: Extend the `Auth` doc comment**

In `ts/packages/paigasus-sdk/src/transport.ts`, replace the doc comment immediately above `export type Auth = …` with:

```ts
/**
 * A union rather than an optional, so that an unauthenticated call — the health check is the real
 * case — is a written decision rather than an omission (spec § 7.4).
 *
 * The client OWNS the `authorization` header. A caller-supplied one — via `CallOptions.headers` —
 * is refused on BOTH arms rather than forwarded or overwritten, so `{ anonymous: true }` means what
 * it says (SMA-627). Two consequences worth knowing before you hit them: `Bearer` is the only
 * Authorization scheme this client can send, and a credential aimed at an intermediary belongs in
 * `proxy-authorization`, which this client does not touch.
 */
```

- [ ] **Step 2: Add the forward reference to the SMA-508 spec**

In `docs/superpowers/specs/2026-09-09-sma-508-sdk-design.md`, append this paragraph at the end of § 7.4 — immediately before the `### 7.5` heading:

```markdown
**Superseded in part by SMA-627.** § 7.4 describes what the SDK *sends*; it does not describe what
it does with a header the CALLER sends. `{ anonymous: true }` originally left a caller-supplied
`authorization` header in place, and the bearer arm silently overwrote one. Both are now refused —
see `docs/superpowers/specs/2026-09-10-sma-627-anonymous-authorization-header-design.md`.
```

- [ ] **Step 3: Verify nothing broke**

Run: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"; cd ts/packages/paigasus-sdk && pnpm exec vitest run`

Expected: PASS, **250 tests in 12 files** — unchanged from Task 3. This step only proves a comment edit did not break a parse.

- [ ] **Step 4: Format and commit**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts && pnpm exec prettier --write packages/paigasus-sdk/src/transport.ts && cd ..
git add ts/packages/paigasus-sdk/src/transport.ts docs/superpowers/specs/2026-09-09-sma-508-sdk-design.md
git commit -m "docs(ts): state the authorization-header rule on the Auth type itself (SMA-627)" -m "A consumer reads the source and the type, not the spec directory. Also marks
SMA-508 section 7.4 as superseded in part, so its description of the anonymous
arm is not read as the whole contract." -m "Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 5: Full-graph verification

Spec AC 8. Per-project tasks do not run the repo-level gates, so a green `vitest` proves nothing about `ts:fmt`, `ts:lint`, the boundary lint rules, or the affected-graph gates.

**Files:** none — verification only.

**Interfaces:**
- Consumes: everything from Tasks 1–4.
- Produces: the evidence for AC 8.

- [ ] **Step 1: Run the TypeScript gates directly first**

They are the ones this change can plausibly red, and they fail faster than the full graph.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run ts:fmt ts:lint paigasus-sdk-ts:typecheck paigasus-sdk-ts:test
```

Expected: all four pass. `ts:fmt` is a whole-tree `prettier --check .`, so it reds on any file this branch touched under `ts/` that Steps above failed to format.

- [ ] **Step 2: Run the full CI target list**

Use the exact command from CLAUDE.md's `ci-targets` markers — it is pinned against `ci.yml`'s `T=(…)` array by `repo:affected-smoke`, so an improvised target list is not equivalent.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :test-e2e \
  --base origin/main \
  --include-relations
```

Expected: green.

**Two failure modes that are NOT this branch's fault**, both recorded in CLAUDE.md and in memory — recognise them rather than debugging the diff:

1. A sub-3s `repo:affected-smoke` abort under a concurrent `moon ci`. **Capture the full task output before re-running**, because a passing re-run overwrites `stdout.log`, truncates `stderr.log` and flips the `ciReport.json` row. Grep the captured output for `proto-shim`; if that line is present the failure is infrastructure, not the affected graph.
2. A hang in `affected-smoke` or `actionlint` on this machine. That is the bash 5.3.15 here-string deadlock, not a gate failure. Re-run the affected-graph suite under the system bash (`/bin/bash`), which completes it.

If a gate genuinely reds, diagnose it with the procedure between CLAUDE.md's `moon-diagnosis` markers — Step 0 (copy `.moon/cache/ciReport.json` and `.moon/cache/states/<project>/<task>/` out of the repo) comes **before** any re-run.

<!-- moon-diagnosis:ok -->
<!-- This file mentions ciReport.json only to DEFER to CLAUDE.md's moon-diagnosis block, which is
     the single source of the procedure. It reproduces none of the advice that block corrects: it
     makes no claim about an action-level exitCode key, and it does not suggest the file carries
     stdout or stderr. `repo:actionlint` check 12 (SMA-597) requires this marker on any file
     carrying the token. -->


- [ ] **Step 3: Confirm the acceptance criteria**

Walk spec § 8 and check each of the eight against what now exists. Report any that are not met rather than declaring completion. AC 6 in particular is checkable as a number: the suite must be **250 tests in 12 files**, up from the baseline 236 (14 added: 1 in Task 1, 11 in Task 2, 2 in Task 3), with no pre-existing test modified except the two empty-bearer ones, whose assertions are unchanged.

No commit — this task produces evidence, not a diff.

---

## Self-review

**Spec coverage.** § 3 → Task 2. § 3.1 (`ConnectError`, both refusals) → Tasks 1 and 2. § 3.3 (exactly one header; `cookie` and a custom header) → Task 2's last test. § 3.4 (empty value) → Task 2's `it.each` empty-header block. § 3.5 (message contents and the no-echo rule) → Task 2's "names the cause" test. § 3.6 (ordering) → Task 2's "reports the HEADER" test. § 4 (the check's comment, the interceptor-order invariant, the `Auth` doc comment) → Tasks 2 and 4. M5 / § 5 case 10 (streaming) → Task 3. § 6 (SMA-508 forward reference) → Task 4. § 8 AC 8 → Task 5.

§ 5.2's untested seam stays untested **by design** and correctly has no task; the spec explains why.

**Placeholders.** None. Every code step carries the literal text to write, and every run step carries the command and the expected outcome, including the two expected-to-pass-immediately cases in Task 3 — stated as such rather than dressed up as a red-green cycle.

**Type consistency.** `rejection()` is defined once in Task 1 and used in Tasks 1 and 2 with the same signature. `fakeUnaryRequest()` keeps its existing zero-argument signature throughout — the spec's suggested `headers?` parameter is deliberately not adopted, and the header note at the top of this plan explains why. `fakeStreamRequest()` is defined in Task 3 and used only there. `ConnectError`, `Code` and `presentationForGrpcCode` are imported in Task 1 and reused unchanged in Task 2.
