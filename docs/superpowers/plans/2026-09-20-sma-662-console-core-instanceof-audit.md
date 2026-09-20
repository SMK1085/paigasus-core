# SMA-662 console-core cross-copy instanceof audit — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Record the audit's findings in the code — one new test row pinning the third-party
contract the audit's headline rests on, and a comment at every classification site saying why it
cannot fail across Next's two module copies.

**Architecture:** No behaviour changes. One test row is added to an existing file. Four source
files gain comments — `discovery.ts`, `errors.ts`, `iam-clients.ts` and `principal-resolver.ts` —
two of which also correct a claim that is now known to be wrong, plus one test file gains a row.

**Tech Stack:** TypeScript, vitest 5, `@connectrpc/connect` 2.2.0, Moon.

**Spec:** `docs/superpowers/specs/2026-09-20-sma-662-console-core-instanceof-audit-design.md`

## Global Constraints

- Every source file opens with `// SPDX-License-Identifier: Apache-2.0`. All files here already
  have it; do not add a second one.
- Branch: `feature/sma-662-ts-audit-console-core-instanceof`. Commits are conventional with a
  workspace scope, e.g. `test(ts): …`, `docs(ts): …`.
- **commitlint traps, both hit on this branch already.** A body line may not exceed 100
  characters, and no body line may begin with `word:` — commitlint reads it as a footer token and
  fails `footer-leading-blank`. Wrap the body and start no line with a word followed by a colon.
- Prefix every shell command with
  `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so Moon and pnpm resolve to the
  repo-pinned versions.
- Run all commands from the worktree root,
  `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-662`.
- **Do not change behaviour.** If a step seems to require an `if`, a new export, or a signature
  change, stop and report — the spec's D1 says nothing changes behaviour.
- **Do not add `server.deps.inline` to `vitest.config.ts`.** The spec measured it and rejected it
  (§ 10).

---

### Task 1: Pin `ConnectError`'s cross-copy brand

The audit's headline is that `errors.ts:23` is safe because `ConnectError` declares a static
`Symbol.hasInstance` that falls back to a duck-type brand. Nothing in the repository would notice
a connect-es release dropping it. This row is that control.

**Files:**
- Modify: `ts/packages/paigasus-console-core/tests/unit/call-iam.test.ts` (append at end of file)

**Interfaces:**
- Consumes: `callIam` from `../../src/errors` and `Code`, `ConnectError` from
  `@connectrpc/connect` — both are ALREADY imported at the top of this file. Do not add duplicate
  imports.
- Produces: nothing other tasks rely on.

- [ ] **Step 1: Append the fixture class and the row**

Append this to the very end of `ts/packages/paigasus-console-core/tests/unit/call-iam.test.ts`,
after the closing `});` of the existing `describe('callIam', ...)` block:

```ts

// SMA-662. `callIam` classifies with `instanceof ConnectError`, and this package exists TWICE in
// one process: Next gives a route handler and a page separate module graphs. What makes that safe
// is not this package — it is `ConnectError`'s static Symbol.hasInstance, which falls back to a
// duck-type BRAND (`name === 'ConnectError'` plus code/metadata/details/rawMessage/cause) when the
// prototype does not match (@connectrpc/connect 2.2.0, dist/esm/connect-error.js:82-98). This row
// pins that third-party contract, because nothing else in the repository would notice a connect-es
// release dropping it.
//
// The fixture is a LOCAL class, not a second module copy. `vi.resetModules()` plus a dynamic
// import does NOT duplicate a node_modules package under this config (MEASURED, spec § 10: vitest
// externalizes node_modules, and resetModules clears Vite's registry, not Node's ESM cache), so a
// local class is what reproduces "an object the other copy built" — a foreign prototype carrying
// the right brand.
class ForeignConnectError extends Error {
  readonly code: number;
  readonly metadata = new Headers();
  readonly details: unknown[] = [];
  readonly rawMessage: string;

  constructor(rawMessage: string, code: number) {
    super(`[permission_denied] ${rawMessage}`);
    this.name = 'ConnectError';
    this.code = code;
    this.rawMessage = rawMessage;
    this.cause = undefined;
  }

  /** The brand does not require this; `mapError` calls it. */
  findDetails(): never[] {
    return [];
  }
}

describe('callIam across two module copies (SMA-662)', () => {
  it('maps a ConnectError whose prototype belongs to another copy, instead of rethrowing it', async () => {
    const err = new ForeignConnectError('denied', Code.PermissionDenied);

    // The precondition comes FIRST. Without it the row passes vacuously against a real
    // ConnectError, which is the whole thing it is trying not to be.
    expect(Object.getPrototypeOf(err)).not.toBe(ConnectError.prototype);
    // The BRAND admits it, not the prototype. This is the assertion that reds if connect-es
    // removes Symbol.hasInstance.
    expect(err instanceof ConnectError).toBe(true);

    const result = await callIam(() => Promise.reject(err));

    expect(result.ok).toBe(false);
    if (!result.ok) {
      expect(result.error.presentation).toBe('forbidden');
      expect(result.error.transport).toEqual({ kind: 'grpc', code: 7, codeName: 'PermissionDenied' });
    }
  });
});
```

- [ ] **Step 2: Run the row and confirm it PASSES**

Run:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-console-core && pnpm exec vitest run tests/unit/call-iam.test.ts
```

Expected: all tests pass, including the new one. **Red-first does not apply here** — the row
describes behaviour that already works, and it exists to pin a dependency's contract. Step 3 is
the evidence that it bites. If the row FAILS, stop and report: the measurement in the spec says it
should pass, so a failure means something changed.

- [ ] **Step 3: Mutation 1 — prove the row tests the BRAND, not just `callIam`**

Edit `ts/packages/paigasus-console-core/src/errors.ts` line 23. Replace:

```ts
    if (!(err instanceof ConnectError)) throw err;
```

with a prototype test, which is what `instanceof` would mean without the brand:

```ts
    if (Object.getPrototypeOf(err) !== ConnectError.prototype) throw err;
```

Run the same command as Step 2.

Expected: the new row FAILS — `callIam` now rethrows, so the promise rejects instead of resolving
to `{ ok: false }`. Record the failure text for the pull request.

**Now revert it.** Restore the exact original line above. Do NOT use `git checkout --` on the
file: that also reverts anything else uncommitted in it. Edit the one line back by hand and
confirm with:

```bash
grep -n "instanceof ConnectError" ts/packages/paigasus-console-core/src/errors.ts
```

Expected: one line, `23:    if (!(err instanceof ConnectError)) throw err;`

- [ ] **Step 4: Mutation 2 — prove the fixture is load-bearing**

In the test file, break ONE brand field. Change the constructor's

```ts
    this.rawMessage = rawMessage;
```

to

```ts
    this.rawMessage = 42 as unknown as string;
```

Do not delete the field declaration instead: whether an unassigned declared field exists as an own
property depends on `useDefineForClassFields`, so that mutation would fail on a different brand
clause depending on the tsconfig. Assigning a number fails deterministically on
`typeof v.rawMessage == "string"`.

Run the same command as Step 2.

Expected: the new row FAILS at the `expect(err instanceof ConnectError).toBe(true)` assertion.
This proves the fixture is a genuine brand-shaped object, satisfying the brand clause by clause,
and not accidentally a real `ConnectError`. Record the failure text.

**Now revert both edits by hand** and re-run Step 2 to confirm everything passes again.

- [ ] **Step 5: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-662
git add ts/packages/paigasus-console-core/tests/unit/call-iam.test.ts
git commit -F - <<'EOF'
test(ts): pin ConnectError's cross-copy Symbol.hasInstance brand (SMA-662)

callIam classifies with `instanceof ConnectError`, and Next gives a route handler and a
page separate copies of this package. That is safe only because ConnectError declares a
static Symbol.hasInstance with a duck-type brand fallback. Nothing in the repository
would have noticed a connect-es release dropping it. This row does.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 2: State the sharing model in `src/errors.ts`

**Files:**
- Modify: `ts/packages/paigasus-console-core/src/errors.ts` (header block, and line 23)

**Interfaces:**
- Consumes: nothing. Comments only.
- Produces: nothing.

- [ ] **Step 1: Add the header paragraphs**

The file currently opens:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The ONE wrapper around an SDK call (spec § 6.1). It turns a ConnectError into the SDK's
// PaigasusError and returns it as data. It RETHROWS EVERY OTHER ERROR UNCHANGED: Next's redirect(),
// notFound() and forbidden() are thrown errors, and so is a real bug; swallowing either would turn
// a navigation into an error page or hide the bug. The app branches on presentation, domain and
// reason only, never on message text (ADR-0019 E8).
import 'server-only';
```

Insert these two paragraphs between the `(ADR-0019 E8).` line and `import 'server-only';`:

```ts
//
// MODULE COPIES, AND WHY THE CHECK BELOW IS SAFE (SMA-662). Next gives a route handler and a page SEPARATE
// module graphs, so this package exists twice in one process and a class has two identities. The
// `instanceof` below survives that, and the reason lives in the DEPENDENCY, not here:
// `ConnectError` declares a static Symbol.hasInstance that falls back to a duck-type BRAND —
// `name === 'ConnectError'` plus `code`, `metadata`, `details`, `rawMessage` and `cause` —
// whenever the prototype does not match (@connectrpc/connect 2.2.0,
// dist/esm/connect-error.js:82-98). An instance the other copy built passes every clause.
// tests/unit/call-iam.test.ts pins that contract, because nothing else here would notice a
// connect-es release dropping it. Note the brand is WIDER than a prototype test: any Error of that
// shape is admitted, and that widening IS the mechanism.
//
// WHAT CROSSES AND WHAT DOES NOT. createConsoleRuntime builds every product fresh per call and
// reads no globalThis. Three things are shared: the descriptor cache and its Redis client, one per
// process (discovery.ts's globalThis symbol); the AuthRuntime, one per process, owned by
// @paigasus/auth; and @paigasus/sdk's transport cache, one per module COPY. The IAM clients
// themselves never cross. The full audit, and the rule it applies (SMA-657 D7), are in
// docs/superpowers/specs/2026-09-20-sma-662-console-core-instanceof-audit-design.md.
```

(The header text says "the check below" rather than "line 23" because inserting this header block
pushes that check about twenty lines down, so "line 23" would have been false on arrival.)

- [ ] **Step 2: Add the one-line reason at the site**

Find this line inside `callIam`:

```ts
    if (!(err instanceof ConnectError)) throw err;
```

Insert this comment directly above it, at the same indentation:

```ts
    // Safe across the two module copies: ConnectError's Symbol.hasInstance brand admits an
    // instance the other copy built, so this is not a prototype test (see the header).
```

- [ ] **Step 3: Verify nothing broke**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-console-core && pnpm exec vitest run tests/unit/call-iam.test.ts && pnpm exec tsc -p tsconfig.json --noEmit
```

Expected: tests pass, typecheck clean.

- [ ] **Step 4: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-662
git add ts/packages/paigasus-console-core/src/errors.ts
git commit -F - <<'EOF'
docs(ts): state console-core's module-copy sharing model in errors.ts (SMA-662)

Records which createConsoleRuntime products cross Next's two module graphs and which do
not, and why the ConnectError check survives the split. The full audit lives in the
SMA-662 spec.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 3: Comment the four remaining sites, and correct two wrong claims

**Files:**
- Modify: `ts/packages/paigasus-console-core/src/discovery.ts` (doc block at `:95-100`, and
  above the `if` at `:274`)
- Modify: `ts/packages/paigasus-console-core/src/principal-resolver.ts` (header; and the stale
  citation on line 4)
- Modify: `ts/packages/paigasus-console-core/src/iam-clients.ts` (lines 8-9)

**Interfaces:**
- Consumes: nothing. Comments only.
- Produces: nothing.

- [ ] **Step 1: `discovery.ts` — the node-redis sites**

The doc block above `connectionLossReason` currently reads:

```ts
/**
 * A fixed value from `instanceof` checks, never the message or `constructor.name` (SMA-648 D7). A
 * node-redis error message can embed the DSN, the error classes do not set `name`, and a
 * production bundle can mangle `constructor.name`. SocketTimeoutDuringMaintenanceError is not a
 * subclass of SocketTimeoutError, so it maps to `other`.
 */
```

Add this paragraph before the closing ` */`:

```ts
 *
 * Safe across the two module copies (SMA-662). watchConnectionLoss registers this listener below,
 * inside the same redisDescriptorCache call that created the client, so the copy whose `redis`
 * import built the error object is always the copy whose `redis` import these two checks resolve.
 * That pairing is the whole argument, and it holds however many clients exist: it does NOT rest on
 * descriptorCacheFor running once per process, which the exported resetDiscoveryForTest can
 * defeat.
```

- [ ] **Step 2: `discovery.ts` — the `DescriptorCacheTimeoutError` site**

Find this line inside `withOperationDeadline`'s `bounded` closure:

```ts
      if (error instanceof DescriptorCacheTimeoutError && error.phase === 'deadline') {
```

Insert this comment directly above it, at the same indentation:

```ts
      // Safe across the two module copies (SMA-662): this class is thrown just above, at the
      // circuit-open guard and in the deadline timer, and caught here — all inside the one
      // `bounded` closure, which the other copy reaches through globalThis and runs verbatim. The
      // error that escapes below DOES cross, and nothing out there classifies it:
      // @paigasus/discovery bare-catches every cache failure and names no class.
```

- [ ] **Step 3: `principal-resolver.ts` — the stale citation**

Line 4 currently reads:

```ts
// calls it once (ts/packages/paigasus-auth/src/http/routes.ts:236), with the new access token.
```

Replace `:236` with `:293` (verified: `routes.ts:293` is
`const principal = await runtime.resolver.resolve({ ... })`):

```ts
// calls it once (ts/packages/paigasus-auth/src/http/routes.ts:293), with the new access token.
```

- [ ] **Step 4: `principal-resolver.ts` — the shared-state paragraph**

The header's last paragraph currently ends:

```ts
// instead, so the two causes stay distinguishable in the log. The pages do not read this snapshot
// (they call currentPrincipal()), so a degraded login costs nothing after the first render.
import 'server-only';
```

Insert this paragraph between that line and `import 'server-only';`:

```ts
//
// THIS IS THE ONE PRODUCT OF THIS PACKAGE THAT LIVES ON SHARED STATE (SMA-662). The app's
// lib/auth.ts hands this resolver to getAuthRuntime, a process singleton whose FIRST call fixes it
// for the life of the process, so one copy's resolver serves requests the other copy handles. It
// is safe because the closure supplies BOTH sides: deps.clientsForToken builds the client and
// callIam below classifies its errors, and both belong to the copy that built this resolver. The
// two `err instanceof Error` reads in the catch test a BUILTIN, which both copies share.
```

- [ ] **Step 5: `iam-clients.ts` — correct "process-scoped"**

Lines 8-9 currently read:

```ts
// No client and no token lives past the request (ts/packages/paigasus-sdk/src/iam.ts:23-28). Only
// the SDK's transport is process-scoped.
```

Replace both lines with:

```ts
// No client and no token lives past the request (ts/packages/paigasus-sdk/src/iam.ts:23-28). The
// SDK's transport outlives the request, but it is cached per module COPY, not per process
// (@paigasus/sdk/src/transport.ts:215): Next gives a route handler and a page separate module
// graphs, so each graph opens its own HTTP/2 session pool to IAM, and disposeTransports() reaches
// only the copy that calls it (SMA-662).
```

- [ ] **Step 6: Verify**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd ts/packages/paigasus-console-core && pnpm exec tsc -p tsconfig.json --noEmit && pnpm exec vitest run
```

Expected: typecheck clean, the whole Docker-free suite passes.

- [ ] **Step 7: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-662
git add ts/packages/paigasus-console-core/src/discovery.ts ts/packages/paigasus-console-core/src/principal-resolver.ts ts/packages/paigasus-console-core/src/iam-clients.ts
git commit -F - <<'EOF'
docs(ts): say why each console-core instanceof survives two module copies (SMA-662)

Adds the reason at the node-redis sites, the descriptor-cache timeout site and the
principal resolver. Corrects two claims the audit disproved. The SDK transport cache is
per module copy, not process-scoped, and the resolver's call site moved to routes.ts:293.

Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
EOF
```

---

### Task 4: Full verification

**Files:** none modified. This task runs gates and reports.

**Interfaces:**
- Consumes: the three commits above.
- Produces: the evidence the pull request cites.

- [ ] **Step 1: The package's own tasks**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-662
moon run paigasus-console-core-ts:test paigasus-console-core-ts:typecheck
```

Expected: both pass.

- [ ] **Step 2: The repo-wide TypeScript gates**

`ts:fmt` is a separate whole-tree Prettier gate, decoupled from `ts:lint`. Both must run because
this change touched `.ts` files.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-662
moon run ts:lint ts:fmt
```

Expected: both pass. If `ts:fmt` reports a diff, run the repo's formatter and amend the commit the
diff belongs to — do not add a fourth "formatting" commit.

- [ ] **Step 3: Confirm no behaviour changed**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-662
git diff origin/main...HEAD -- ts/packages/paigasus-console-core/src/ | grep -E "^[+-]" | grep -vE "^[+-][+-]" | grep -vE "^[+-]\s*(//|\*|/\*)" | grep -vE "^[+-]\s*$"
```

Expected: **no output.** Every line added or removed under `src/` is a comment. If any code line
appears, stop and report — the spec's D1 says nothing changes behaviour.

- [ ] **Step 4: Report**

Report, for the pull request body: the two mutation failures recorded in Task 1 Steps 3 and 4
(verbatim), the result of each command above, and confirmation that Step 3 produced no output.

---

## Notes for the executor

- **`rs/` is not touched.** Do not run the Rust gates.
- **Do not run `ci/actionlint/run.sh` locally.** It has no working local bash on this machine
  (CLAUDE.md). This change touches no workflow file.
- **This change adds no `repo:*` gate**, so none of the seven registration obligations apply.
- If the worktree is missing `node_modules`, run `pnpm -C ts install` first.
