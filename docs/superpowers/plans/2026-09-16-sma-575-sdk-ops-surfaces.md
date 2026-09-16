# SMA-575 sdk ops-surface proof and HTTP guard — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Prove that `@paigasus/sdk` reaches the six IAM ops RPCs through generated Connect-ES descriptors, enforce that `src/chat.ts` is the only hand-written HTTP client, and correct ADR-0018.

**Architecture:** Test-only change in `ts/packages/paigasus-sdk`, plus two Moon `inputs` lines, one affected-graph control case, one `package.json` comment, and a Notion ADR amendment. No file under `src/` changes.

**Tech Stack:** TypeScript 5, vitest 5 (`expectTypeOf`), `@connectrpc/connect` 2.2, `@bufbuild/protobuf` 2.14, the TypeScript compiler API, Moon 2.5.3.

**Spec:** `docs/superpowers/specs/2026-09-16-sma-575-sdk-ops-surfaces-design.md` (Revision 2, approved at GATE 1 on 2026-09-16). Read it before you start.

## Global Constraints

- Work ONLY in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-575`. Before the first command, run `git -C /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-575 branch --show-current`. It must print `feature/sma-575-sdk-connect-es-ops-surfaces`. If it does not, STOP and report.
- Prefix every shell command with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- Run foreground commands only. Do not start a background job and end your turn.
- **Add commits, do not amend.** Never `git commit --amend`, never `--no-verify`.
- Commit subject: conventional commit with scope `ts` or `ci`, lower-case start, issue key at the END, for example `test(ts): pin the sdk ops-surface descriptors (SMA-575)`. A subject that starts with `SMA-575` fails commitlint. End every commit message with a blank line and `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`. Do not put a `#NNN` or `token: value` line in the body.
- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0`.
- **No test writes into `src/`.** No step of this plan changes a file under `ts/packages/paigasus-sdk/src/`, except a marked mutation that you restore in the same step.
- Restore a mutation with the Edit tool (remove the exact inserted text). NEVER use `git checkout --` or `git restore`: they also revert uncommitted work.
- Test files import source with the `.js` suffix (`'../src/iam.js'`), as the existing tests do. `src/` files use extensionless imports, but no task edits `src/`.
- Package commands: `pnpm -C ts/packages/paigasus-sdk exec vitest run <file>` and `pnpm -C ts/packages/paigasus-sdk exec tsc -p tsconfig.json --noEmit`. Moon commands: `moon run paigasus-sdk-ts:test --force`. Use `--force` whenever you measure a mutation, because a Moon cache hit replays an old PASS.
- Lint and format before each commit: `moon run ts:lint ts:fmt`. If `ts:fmt` fails, run `pnpm -C ts exec prettier --write <files>` and check the diff.

---

### Task 1: Stop `server-guard.test.ts` writing probe files into `src/`

Spec § 2.

**Files:**
- Modify: `ts/packages/paigasus-sdk/tests/server-guard.test.ts` (lines 6 and 112-235)

**Interfaces:**
- Consumes: the existing local function `importsServerOnly(source: string, fileName: string): boolean` (same file, about line 96).
- Produces: nothing for later tasks. Task 3 relies on the fact that no test writes into `src/`.

- [ ] **Step 1: Read the file**

Read `ts/packages/paigasus-sdk/tests/server-guard.test.ts` in full. The six tests to change are the six `it(...)` blocks after `it('no other file in src/ imports it', ...)` in the describe block `"AC 1 — 'server-only' is imported at exactly one site"`. Each one writes a `src/__*-probe.ts` file.

- [ ] **Step 2: Replace the six probe tests**

Replace all six `it(...)` blocks (from `it('detects double-quoted server-only import as offender'` to the end of `it('does not flag unrelated import with server-only in comment as offender'`, inclusive) with this block:

```ts
  // In-memory probes. They used to write `src/__*-probe.ts` files and walk the tree. Vitest runs
  // test files in parallel, so another file that walks `src/` (tests/http-surface.test.ts) could
  // read a probe mid-flight and report a false violation, or hit ENOENT when the probe was deleted
  // between its readdir and its read. A killed run also left the probe in `src/`. RULE: no test in
  // this package writes into `src/` (SMA-575 spec § 2). The probes test `importsServerOnly`, which
  // is the part that decides; the walk itself is covered by 'no other file in src/ imports it'.
  const PROBE = 'src/__probe.ts';

  it('detects double-quoted server-only import as offender', () => {
    const content = `// SPDX-License-Identifier: Apache-2.0
import "server-only";
`;
    expect(importsServerOnly(content, PROBE)).toBe(true);
  });

  it('does not flag server-only inside a comment as offender', () => {
    const content = `// SPDX-License-Identifier: Apache-2.0
// This mentions server-only in prose, not as an import.
export const guard = true;
`;
    expect(importsServerOnly(content, PROBE)).toBe(false);
  });

  it('detects server-only import with trailing comment as offender', () => {
    const content = `// SPDX-License-Identifier: Apache-2.0
import "server-only"; // boundary
`;
    expect(importsServerOnly(content, PROBE)).toBe(true);
  });

  it('detects server-only import with block comment noise as offender', () => {
    const content = `// SPDX-License-Identifier: Apache-2.0
import /* boundary */ "server-only";
`;
    expect(importsServerOnly(content, PROBE)).toBe(true);
  });

  it('detects server-only import with a multiline block comment as offender', () => {
    // The block comment carries a REAL line terminator (not an escaped "\\n" inside one
    // source line) — this is the case a regex anchored on `[^'"\n]*` cannot see, since the
    // comment crosses a newline before the specifier is reached.
    const content = `// SPDX-License-Identifier: Apache-2.0
import /* boundary
*/ "server-only";
`;
    expect(importsServerOnly(content, PROBE)).toBe(true);
  });

  it('does not flag unrelated import with server-only in comment as offender', () => {
    const content = `// SPDX-License-Identifier: Apache-2.0
import { readFileSync } from 'node:fs'; // not a server-only import
`;
    expect(importsServerOnly(content, PROBE)).toBe(false);
  });
```

- [ ] **Step 3: Remove the unused imports**

In line 6, change `import { readdirSync, readFileSync, writeFileSync, unlinkSync } from 'node:fs';` to `import { readdirSync, readFileSync } from 'node:fs';`. Then run `grep -n "writeFileSync\|unlinkSync\|probeFile" ts/packages/paigasus-sdk/tests/server-guard.test.ts`. Expected: no output.

- [ ] **Step 4: Prove the probes still bite**

Mutation: in `importsServerOnly`, change `statement.moduleSpecifier.text === 'server-only'` to `statement.moduleSpecifier.text === 'server-onlyX'`.
Run: `pnpm -C ts/packages/paigasus-sdk exec vitest run tests/server-guard.test.ts`
Expected: FAIL. The four "detects …" probes fail (expected `true`, received `false`). Record the count of failed tests.
Restore the exact text with Edit. Run the same command. Expected: PASS.

- [ ] **Step 5: Lint, format, test, commit**

Run: `moon run paigasus-sdk-ts:test ts:lint ts:fmt`
Expected: all pass.

```bash
git add ts/packages/paigasus-sdk/tests/server-guard.test.ts
git commit -m "test(ts): keep sdk server-guard probes out of src (SMA-575)

The six probes wrote files into src/ while other test files walk it.
They now check strings in memory.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

---

### Task 2: Prove the six ops RPCs are reachable through `createIamClient`

Spec § 3.

**Files:**
- Modify: `ts/packages/paigasus-sdk/tests/iam-factory.test.ts`
- Modify: `ts/packages/paigasus-sdk/moon.yml` (the `build` and `typecheck` tasks)
- Modify: `ci/affected-graph/run.sh` (add one `run_task_case_ci` control next to the `sdk->iam-console` cases, about line 568)

**Interfaces:**
- Consumes: `createIamClient<S extends DescService>(service: S, options: TransportOptions, auth: Auth): Client<S>` and `authContextKey` from `src/iam.ts` / `src/transport.ts`. `Auth` is `{ bearer: string }` (see the existing FIX 1 test).
- Produces: nothing for later tasks.

- [ ] **Step 1: Extend the recorder**

In `vi.hoisted(...)`, replace `seen` with a `calls` array that records the method, the input and the context values. The new hoisted block:

```ts
type RecordedCall = { readonly method: DescMethodUnary | DescMethodStreaming; readonly input: unknown; readonly contextValues: ContextValues | undefined };

const { calls, createGrpcTransport } = vi.hoisted(() => {
  const calls: RecordedCall[] = [];
  const transport: Transport = {
    unary<I extends DescMessage, O extends DescMessage>(
      method: DescMethodUnary<I, O>,
      _signal: AbortSignal | undefined,
      _timeoutMs: number | undefined,
      _header: HeaderParam,
      input: MessageInitShape<I>,
      contextValues: ContextValues | undefined,
    ): Promise<UnaryResponse<I, O>> {
      calls.push({ method, input, contextValues });
      return Promise.resolve({
        stream: false,
        message: {},
        header: new Headers(),
        trailer: new Headers(),
      } as unknown as UnaryResponse<I, O>);
    },
    stream<I extends DescMessage, O extends DescMessage>(
      method: DescMethodStreaming<I, O>,
      _signal: AbortSignal | undefined,
      _timeoutMs: number | undefined,
      _header: HeaderParam,
      input: AsyncIterable<MessageInitShape<I>>,
      contextValues: ContextValues | undefined,
    ): Promise<StreamResponse<I, O>> {
      calls.push({ method, input, contextValues });
      throw new Error('not used');
    },
  };
  return {
    calls,
    createGrpcTransport: vi.fn<(options: unknown) => Transport>(() => transport),
  };
});
```

Keep the existing explanatory comments inside the block. `RecordedCall` is a type alias; declare it above `vi.hoisted` (types are erased, so the hoisting rule does not apply to it). If `DescMethodUnary` without type arguments does not compile, use `DescMethodUnary<DescMessage, DescMessage>` and the same for `DescMethodStreaming`.

Update `afterEach`: replace `seen.length = 0;` with `calls.length = 0;`.

Update the FIX 1 test: replace its two assertions with

```ts
    expect(calls).toHaveLength(1);
    expect(calls[0]?.contextValues?.get(authContextKey)).toEqual({ bearer: 'alice-token' });
```

Run: `pnpm -C ts/packages/paigasus-sdk exec vitest run tests/iam-factory.test.ts`
Expected: PASS (1 test).

- [ ] **Step 2: Measure how Connect passes the input**

Add this temporary test at the end of the file:

```ts
it('MEASURE: input identity', async () => {
  const init = { email: 'probe@example.invalid' };
  await createIamClient(UserService, { baseUrl: 'https://iam.invalid' }, { bearer: 't' }).createUser(init);
  console.log('SAME_OBJECT=', calls[0]?.input === init, 'VALUE=', JSON.stringify(calls[0]?.input));
});
```

Add `UserService` to the `@paigasus/proto/iam` import. If `email` is not a field of `CreateUserRequest`, read `CreateUserRequest` in `ts/packages/paigasus-proto/src/generated/paigasus/iam/v1/iam_pb.ts` (about line 2130) and use a real string field.
Run: `pnpm -C ts/packages/paigasus-sdk exec vitest run tests/iam-factory.test.ts`
Record the printed `SAME_OBJECT` value. Delete the temporary test. In Step 3, use `toBe(row.init)` if `SAME_OBJECT= true`, else `toEqual(row.init)`, and write the measured result in the comment above that assertion.

- [ ] **Step 3: Write the row table and the reachability tests**

Add these imports (merge with the existing ones; keep `import type` for types):

```ts
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import type { MessageShape } from '@bufbuild/protobuf';
import type { Client } from '@connectrpc/connect';
import {
  AuthorizationService,
  BulkReplayDeadLettersRequestSchema,
  BulkReplayDeadLettersResponseSchema,
  CreateUserRequestSchema,
  CreateUserResponseSchema,
  DiscardDeadLetterRequestSchema,
  DiscardDeadLetterResponseSchema,
  ListDeadLettersRequestSchema,
  ListDeadLettersResponseSchema,
  OutboxService,
  ReplayDeadLetterRequestSchema,
  ReplayDeadLetterResponseSchema,
  RetireSystemPolicyRequestSchema,
  RetireSystemPolicyResponseSchema,
  TenancyService,
  UserService,
} from '@paigasus/proto/iam';
```

Then add, after the FIX 1 describe block:

```ts
// SMA-575 AC 1. The three IAM ops surfaces (spec § 3). Each row calls the PRODUCTION factory and
// asserts what reached the transport: the generated service, the RPC, the generated request and
// response messages, the init object, and the bound Auth. Every expected value is a STRING
// literal, never read back from the descriptor under test, so a wrong descriptor cannot agree
// with itself.
const OPTIONS = { baseUrl: 'https://iam.invalid' };

type OpsRow = {
  readonly service: string;
  readonly rpc: string;
  readonly request: string;
  readonly response: string;
  readonly init: object;
  readonly call: (auth: { bearer: string }, init: object) => Promise<unknown>;
};

// Each `call` is a closure with its own static type, so no row needs a cast. The init objects are
// empty: the transport is a recorder, so no field is validated.
const OPS_ROWS: readonly OpsRow[] = [
  {
    service: 'paigasus.iam.v1.UserService',
    rpc: 'CreateUser',
    request: 'paigasus.iam.v1.CreateUserRequest',
    response: 'paigasus.iam.v1.CreateUserResponse',
    init: {},
    call: (auth, init) => createIamClient(UserService, OPTIONS, auth).createUser(init),
  },
  {
    service: 'paigasus.iam.v1.AuthorizationService',
    rpc: 'RetireSystemPolicy',
    request: 'paigasus.iam.v1.RetireSystemPolicyRequest',
    response: 'paigasus.iam.v1.RetireSystemPolicyResponse',
    init: {},
    call: (auth, init) => createIamClient(AuthorizationService, OPTIONS, auth).retireSystemPolicy(init),
  },
  {
    service: 'paigasus.iam.v1.OutboxService',
    rpc: 'ListDeadLetters',
    request: 'paigasus.iam.v1.ListDeadLettersRequest',
    response: 'paigasus.iam.v1.ListDeadLettersResponse',
    init: {},
    call: (auth, init) => createIamClient(OutboxService, OPTIONS, auth).listDeadLetters(init),
  },
  {
    service: 'paigasus.iam.v1.OutboxService',
    rpc: 'ReplayDeadLetter',
    request: 'paigasus.iam.v1.ReplayDeadLetterRequest',
    response: 'paigasus.iam.v1.ReplayDeadLetterResponse',
    init: {},
    call: (auth, init) => createIamClient(OutboxService, OPTIONS, auth).replayDeadLetter(init),
  },
  {
    service: 'paigasus.iam.v1.OutboxService',
    rpc: 'BulkReplayDeadLetters',
    request: 'paigasus.iam.v1.BulkReplayDeadLettersRequest',
    response: 'paigasus.iam.v1.BulkReplayDeadLettersResponse',
    init: {},
    call: (auth, init) => createIamClient(OutboxService, OPTIONS, auth).bulkReplayDeadLetters(init),
  },
  {
    service: 'paigasus.iam.v1.OutboxService',
    rpc: 'DiscardDeadLetter',
    request: 'paigasus.iam.v1.DiscardDeadLetterRequest',
    response: 'paigasus.iam.v1.DiscardDeadLetterResponse',
    init: {},
    call: (auth, init) => createIamClient(OutboxService, OPTIONS, auth).discardDeadLetter(init),
  },
];

// The Outbox RPC names, pinned by strict equality. OutboxService exists ONLY for the ops
// surfaces, so a change here is a change to SMA-575's scope and must be deliberate.
const OUTBOX_RPCS = ['BulkReplayDeadLetters', 'DiscardDeadLetter', 'ListDeadLetters', 'ReplayDeadLetter'];

// The six (service, rpc) pairs the table must cover. The Outbox half is DERIVED from the
// descriptor, so a new Outbox RPC is a missing row, not a silent gap.
const REQUIRED_PAIRS = ['paigasus.iam.v1.UserService/CreateUser', 'paigasus.iam.v1.AuthorizationService/RetireSystemPolicy', ...OutboxService.methods.map((m) => `${OutboxService.typeName}/${m.name}`)];

describe('SMA-575 AC 1 — the IAM ops surfaces are reachable through createIamClient', () => {
  it('the row table has exactly six rows', () => {
    expect(OPS_ROWS).toHaveLength(6);
  });

  it('the row table covers exactly the required (service, rpc) pairs', () => {
    expect(OPS_ROWS.map((r) => `${r.service}/${r.rpc}`).sort()).toEqual([...REQUIRED_PAIRS].sort());
  });

  it('OutboxService carries exactly the four ops RPCs', () => {
    expect(OutboxService.methods.map((m) => m.name).sort()).toEqual(OUTBOX_RPCS);
  });

  it.each(OPS_ROWS)('$service/$rpc reaches the transport with generated descriptors and the bound Auth', async (row) => {
    await row.call({ bearer: 'ops-token' }, row.init);

    expect(calls).toHaveLength(1);
    const [call] = calls;
    expect(call?.method.parent.typeName).toBe(row.service);
    expect(call?.method.name).toBe(row.rpc);
    expect(call?.method.input.typeName).toBe(row.request);
    expect(call?.method.output.typeName).toBe(row.response);
    // MEASURED in Task 2 Step 2: <write the SAME_OBJECT result here>.
    expect(call?.input).toBe(row.init);
    expect(call?.contextValues?.get(authContextKey)).toEqual({ bearer: 'ops-token' });
  });
});
```

Notes for this step:
- If `row.call`'s `init: object` parameter does not type-check against `MessageInitShape<…>`, change the closures to ignore the `init` parameter type by declaring the table rows with `init` typed per row, for example `call: (auth, init) => createIamClient(UserService, OPTIONS, auth).createUser(init as MessageInitShape<typeof CreateUserRequestSchema>)`. Do NOT use `any`.
- Replace `toBe(row.init)` with `toEqual(row.init)` if Step 2 measured `SAME_OBJECT= false`, and fill in the comment.
- `TenancyService` stays imported for the FIX 1 test.

Run: `pnpm -C ts/packages/paigasus-sdk exec vitest run tests/iam-factory.test.ts`
Expected: PASS (1 + 3 + 6 = 10 tests).

- [ ] **Step 4: Add the type-level proof**

Add after the describe block of Step 3:

```ts
// SMA-575 AC 1, type level. `expectTypeOf(...).toEqualTypeOf` fails plain `tsc` on a mismatch
// (expect-type's MISMATCH rest parameter), so `paigasus-sdk-ts:build` and `:typecheck` are the
// gate; vitest itself does not type-check. Both tasks key on tests/**/* (moon.yml, SMA-575).
// The client type is taken from the FACTORY via an instantiation expression, not from `Client<S>`
// directly, so a factory that returned a hand-written wrapper type would fail here.
type Ops<S extends DescService> = ReturnType<typeof createIamClient<S>>;

describe('SMA-575 AC 1 — the factory types are the generated contract types', () => {
  it('uses generated request and response types for every ops RPC', () => {
    expectTypeOf<Parameters<Ops<typeof UserService>['createUser']>[0]>().toEqualTypeOf<MessageInitShape<typeof CreateUserRequestSchema>>();
    expectTypeOf<Awaited<ReturnType<Ops<typeof UserService>['createUser']>>>().toEqualTypeOf<MessageShape<typeof CreateUserResponseSchema>>();

    expectTypeOf<Parameters<Ops<typeof AuthorizationService>['retireSystemPolicy']>[0]>().toEqualTypeOf<MessageInitShape<typeof RetireSystemPolicyRequestSchema>>();
    expectTypeOf<Awaited<ReturnType<Ops<typeof AuthorizationService>['retireSystemPolicy']>>>().toEqualTypeOf<MessageShape<typeof RetireSystemPolicyResponseSchema>>();

    expectTypeOf<Parameters<Ops<typeof OutboxService>['listDeadLetters']>[0]>().toEqualTypeOf<MessageInitShape<typeof ListDeadLettersRequestSchema>>();
    expectTypeOf<Awaited<ReturnType<Ops<typeof OutboxService>['listDeadLetters']>>>().toEqualTypeOf<MessageShape<typeof ListDeadLettersResponseSchema>>();

    expectTypeOf<Parameters<Ops<typeof OutboxService>['replayDeadLetter']>[0]>().toEqualTypeOf<MessageInitShape<typeof ReplayDeadLetterRequestSchema>>();
    expectTypeOf<Awaited<ReturnType<Ops<typeof OutboxService>['replayDeadLetter']>>>().toEqualTypeOf<MessageShape<typeof ReplayDeadLetterResponseSchema>>();

    expectTypeOf<Parameters<Ops<typeof OutboxService>['bulkReplayDeadLetters']>[0]>().toEqualTypeOf<MessageInitShape<typeof BulkReplayDeadLettersRequestSchema>>();
    expectTypeOf<Awaited<ReturnType<Ops<typeof OutboxService>['bulkReplayDeadLetters']>>>().toEqualTypeOf<MessageShape<typeof BulkReplayDeadLettersResponseSchema>>();

    expectTypeOf<Parameters<Ops<typeof OutboxService>['discardDeadLetter']>[0]>().toEqualTypeOf<MessageInitShape<typeof DiscardDeadLetterRequestSchema>>();
    expectTypeOf<Awaited<ReturnType<Ops<typeof OutboxService>['discardDeadLetter']>>>().toEqualTypeOf<MessageShape<typeof DiscardDeadLetterResponseSchema>>();
  });
});
```

Import `expectTypeOf` from `vitest`, and add `DescService` to the existing `import type … from '@bufbuild/protobuf'`. `createIamClient` is a `const` from a top-level `await import(...)`, so the instantiation expression `typeof createIamClient<S>` is valid TypeScript. If `tsc` still rejects it, STOP and report the exact error; do not replace the factory type with `Client<S>`, because that would stop testing the factory.

Run: `pnpm -C ts/packages/paigasus-sdk exec tsc -p tsconfig.json --noEmit`
Expected: exit 0.
Run: `pnpm -C ts/packages/paigasus-sdk exec vitest run tests/iam-factory.test.ts`
Expected: PASS (11 tests).

- [ ] **Step 5: Pin the exports of `src/iam.ts`**

Add after the type-level describe block:

```ts
// SMA-575 AC 1, "no hand-written DTOs" at the ENTRY level. A new exported wrapper with its own
// request shape (for example `createUser(dto: { email: string })`) would pass every row above,
// because the rows only exercise `createIamClient`. So the entry's export set is pinned by strict
// equality: runtime keys from the module namespace, type-only exports from the AST. A new export
// is a deliberate edit to one of these two lists.
const IAM_RUNTIME_EXPORTS = [
  'AuditService',
  'AuthnService',
  'AuthorizationService',
  'OutboxService',
  'ServiceAccountService',
  'ServiceInfoService',
  'TenancyService',
  'UserService',
  'bindAuth',
  'createIamClient',
  'disposeTransports',
];
const IAM_TYPE_EXPORTS = ['Auth', 'TransportOptions'];

const PKG_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');

function typeOnlyExportNames(source: string): string[] {
  const file = ts.createSourceFile('iam.ts', source, ts.ScriptTarget.ESNext, true);
  const names: string[] = [];
  for (const statement of file.statements) {
    const exported = ts.canHaveModifiers(statement) && (ts.getModifiers(statement) ?? []).some((m) => m.kind === ts.SyntaxKind.ExportKeyword);
    if (ts.isExportDeclaration(statement) && statement.exportClause && ts.isNamedExports(statement.exportClause)) {
      for (const element of statement.exportClause.elements) {
        if (statement.isTypeOnly || element.isTypeOnly) names.push(element.name.text);
      }
    } else if (exported && (ts.isTypeAliasDeclaration(statement) || ts.isInterfaceDeclaration(statement))) {
      names.push(statement.name.text);
    }
  }
  return names.sort();
}

describe('SMA-575 AC 1 — the ./iam entry exports nothing beyond the generated services and the factory', () => {
  it('pins the runtime exports', async () => {
    const iam = await import('../src/iam.js');
    expect(Object.keys(iam).sort()).toEqual([...IAM_RUNTIME_EXPORTS].sort());
  });

  it('pins the type-only exports', () => {
    expect(typeOnlyExportNames(readFileSync(resolve(PKG_ROOT, 'src/iam.ts'), 'utf8'))).toEqual([...IAM_TYPE_EXPORTS].sort());
  });
});
```

Run: `pnpm -C ts/packages/paigasus-sdk exec vitest run tests/iam-factory.test.ts`
Expected: PASS (13 tests). If the runtime key list differs from the pinned list, STOP and compare with `src/iam.ts:16-18,30,70`: the pinned list must match the file as it is today, and the file must not change.

- [ ] **Step 6: Make `build` and `typecheck` key on the tests**

In `ts/packages/paigasus-sdk/moon.yml`, change the `build` and `typecheck` tasks to:

```yaml
  build:
    deps: ['contracts:generate']
    inputs:
      - '/ts/packages/paigasus-proto/src/**/*'
      # SMA-575 spec § 3.3. tsconfig.json `include` names tests/ and vitest.config.ts, so tsc
      # type-checks them — but the inherited inputs are @group(sources) plus config only. Without
      # these two lines a PR that edits only a test file gets a cached PASS, and the
      # `expectTypeOf` proof in tests/iam-factory.test.ts is never checked in CI. The control is
      # `run_task_case_ci "sdk-tests->sdk"` in ci/affected-graph/run.sh.
      - 'tests/**/*'
      - 'vitest.config.ts'
  typecheck:
    deps: ['contracts:generate']
    inputs:
      - '/ts/packages/paigasus-proto/src/**/*'
      # Same reason as `build` above (SMA-575 spec § 3.3).
      - 'tests/**/*'
      - 'vitest.config.ts'
```

Do not change the `test` task.

- [ ] **Step 7: Measure what a test-file edit selects, and add the control case**

Run (the file edit is temporary; it is restored in this step):

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-575
printf '\n// SMA575-PROBE\n' >> ts/packages/paigasus-sdk/tests/iam-factory.test.ts
moon query tasks --affected --upstream none --downstream none 2>/dev/null > /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/6498f3bb-e773-4e0b-a03b-8b76d20a1859/scratchpad/affected.json; echo rc=$?
```

Parse the JSON and take ONE target per `tasks[project][task]` (do NOT grep `"target"`: every `deps[]` entry carries its own `"target"` key). Record the selected `paigasus-sdk-ts:*` tasks and `ts:*` tasks. Expected: `paigasus-sdk-ts:build`, `paigasus-sdk-ts:typecheck` (if CI-eligible) and `paigasus-sdk-ts:test`, plus `ts:lint` and possibly `ts:fmt`.

If `moon query tasks --affected` does not accept those flags on 2.5.3, read how `run_task_case_ci` builds its query in `ci/affected-graph/run.sh` and use the same command.

Remove the `// SMA575-PROBE` lines with Edit (they are the last two lines of the file).

Now read `run_task_case_ci` and the `sdk->iam-console` cases in `ci/affected-graph/run.sh` (about lines 540-575). The existing cases list `paigasus-sdk-ts:build` and `:test` but not `:typecheck`; the function filters to CI-eligible tasks. Add, directly after the `sdk-errors->iam-console` case, a new case whose expected set is EXACTLY what the function reports for a test-file edit. The expected shape, to be confirmed by the run in Step 8:

```bash
  # SMA-575 — an sdk TEST-file edit must select the sdk's own `build` (tsc over tests/, which holds
  # the expectTypeOf proof of AC 1) and `test`, plus `ts:lint`, and nothing downstream: a test file
  # is not a source of any consumer. This is the ONLY control on the `tests/**/*` lines in
  # ts/packages/paigasus-sdk/moon.yml. Without them the edit selects `test` alone and tsc never
  # re-checks the type-level proof.
  run_task_case_ci "sdk-tests->sdk" "ts/packages/paigasus-sdk/tests/iam-factory.test.ts" \
    "paigasus-sdk-ts:build,paigasus-sdk-ts:test,ts:lint"
```

- [ ] **Step 8: Run the affected-graph suite with system bash**

Run: `/bin/bash ci/affected-graph/run.sh 2>&1 | tail -40`
Use `/bin/bash` (3.2). Homebrew bash 5.3.15 deadlocks on this suite on this machine (CLAUDE.md).
Expected: every case passes. If `sdk-tests->sdk` fails, read the reported actual set. Correct the expected string to the actual set ONLY if every extra or missing id is explained by Step 7; otherwise STOP and report. If any OTHER case fails, STOP and report (the moon.yml change must not change other cases).

Then prove the control bites: remove the two `tests/**/*` lines from `build` in `moon.yml` (Edit), run `/bin/bash ci/affected-graph/run.sh 2>&1 | grep -A3 "sdk-tests"`. Expected: the case FAILS and reports `paigasus-sdk-ts:build` missing. Restore the two lines with Edit and run the suite again. Expected: PASS.

- [ ] **Step 9: Mutation battery (spec § 3.4)**

For each mutation: apply with Edit, run the stated command, record the failing assertion message, restore with Edit, confirm PASS. Run all seven even if one surprises you; after any fix, run all seven again.

| # | Mutation | Command | Expected failure |
|---|---|---|---|
| 1 | In the `CreateUser` row, `service: 'paigasus.iam.v1.UserService'` → `'paigasus.iam.v1.TenancyService'` | `pnpm -C ts/packages/paigasus-sdk exec vitest run tests/iam-factory.test.ts` | the `CreateUser` row fails on `parent.typeName` (and the pair-coverage test fails too; record both) |
| 2 | In the `ListDeadLetters` row, `rpc: 'ListDeadLetters'` → `'ListDeadLetter'` | same | the row fails on `method.name` (the pair test also fails; record both) |
| 3 | In `src/iam.ts:71`, `return bindAuth(createClient(service, getTransport(options)), auth);` → `return createClient(service, getTransport(options));` | same | every row fails on the context value; FIX 1 fails too |
| 4 | Delete the whole `DiscardDeadLetter` row | same | 'has exactly six rows' and 'covers exactly the required pairs' fail |
| 5 | Add `'PurgeDeadLetters'` to `OUTBOX_RPCS` | same | 'OutboxService carries exactly the four ops RPCs' fails |
| 6 | In the `listDeadLetters` request line, `ListDeadLettersRequestSchema` → `ReplayDeadLetterRequestSchema` | `pnpm -C ts/packages/paigasus-sdk exec tsc -p tsconfig.json --noEmit` | tsc error on that line. Then run `moon run paigasus-sdk-ts:build` WITHOUT `--force` and confirm it FAILS too — this proves the new `tests/**/*` input reaches the Moon cache key |
| 7 | Append `export const createUser = (): void => undefined;` to `src/iam.ts` | vitest command above | 'pins the runtime exports' fails |

For mutation 6, first run `moon run paigasus-sdk-ts:build` once on the clean tree so the cache holds a PASS, then apply the mutation.

After the battery: `git status --short` must show only `tests/iam-factory.test.ts`, `moon.yml` and `ci/affected-graph/run.sh` as modified. `git diff -- ts/packages/paigasus-sdk/src` must be empty.

- [ ] **Step 10: Lint, format, test, commit**

Run: `moon run paigasus-sdk-ts:test paigasus-sdk-ts:build paigasus-sdk-ts:typecheck ts:lint ts:fmt`
Expected: all pass.

```bash
git add ts/packages/paigasus-sdk/tests/iam-factory.test.ts ts/packages/paigasus-sdk/moon.yml ci/affected-graph/run.sh
git commit -m "test(ts): prove the sdk reaches the IAM ops RPCs (SMA-575)

Six rows drive createIamClient for CreateUser, RetireSystemPolicy and
the four OutboxService RPCs. Type-level checks and an export pin cover
the no-hand-written-DTO rule. build and typecheck now key on tests/.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

Report the mutation table with the recorded messages, and the Step 7 measurement.

---

### Task 3: Guard that hand-written HTTP lives only in `chat.ts`

Spec § 4 and § 6.

**Files:**
- Create: `ts/packages/paigasus-sdk/tests/http-surface.test.ts`
- Modify: `ts/packages/paigasus-sdk/package.json` (the `_comment_entries` value only)

**Interfaces:**
- Consumes: nothing from earlier tasks. Relies on Task 1 (no test writes into `src/`).
- Produces: the file name `tests/http-surface.test.ts`, which the ADR amendment (Task 5) names.

- [ ] **Step 1: Write the checker and the negative controls**

Create `ts/packages/paigasus-sdk/tests/http-surface.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// SMA-575 AC 2 (spec § 4). The ONLY hand-written HTTP client in @paigasus/sdk is src/chat.ts, and
// it serves only the gateway's POST /v1/chat/completions. Everything else reaches a service through
// a generated Connect-ES client (ADR-0018 decision 5, Amendments A1 and A2).
//
// A vitest test, not an ESLint rule: nobody can `eslint-disable` an assertion, and the negative
// controls below are plain calls of a pure function. It PARSES with the TypeScript compiler API
// and visits with `ts.forEachChild`, which skips comments and JSDoc, so prose that names `fetch`
// does not count.
//
// RULE: no test in this package writes into `src/`. This file walks `src/`, and vitest runs test
// files in parallel (SMA-575 spec § 2).
import { readdirSync, readFileSync, statSync } from 'node:fs';
import { dirname, posix, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { describe, expect, it } from 'vitest';

const PKG_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const SRC_DIR = 'src';
const CHAT_FILE = 'src/chat.ts';

// Rule 1. Bare specifiers any src/ file may import. Strict: a new dependency is a deliberate edit.
const ALLOWED_SPECIFIERS = new Set(['server-only', '@connectrpc/connect', '@bufbuild/protobuf', '@paigasus/proto', '@paigasus/proto/iam']);

// Rule 1, narrowed. @connectrpc/connect-node also exports a generic HTTP client
// (`createNodeHttpClient`) and the Connect and gRPC-web transports, so it is allowed in ONE file
// and for the two gRPC names only.
const CONNECT_NODE = '@connectrpc/connect-node';
const CONNECT_NODE_FILE = 'src/transport.ts';
const CONNECT_NODE_IMPORTS = new Set(['createGrpcTransport', 'Http2SessionManager']);

// Rule 2. Banned outside src/chat.ts. `globalThis`, `global` and `process` close the obvious
// escapes (`globalThis['fe' + 'tch']`, `process.getBuiltinModule('node:https')`). `Headers` is
// NOT here: src/errors/map-error.ts uses it to read response metadata.
const BANNED_IDENTIFIERS = new Set(['fetch', 'XMLHttpRequest', 'WebSocket', 'EventSource', 'Request', 'Response', 'RequestInit', 'globalThis', 'global', 'process']);

type Violation = { readonly relPath: string; readonly line: number; readonly reason: string };

/** `relPath` is package-relative with `/` separators, e.g. `src/errors/map-error.ts`. */
function findViolations(relPath: string, source: string): Violation[] {
  const file = ts.createSourceFile(relPath, source, ts.ScriptTarget.ESNext, true, ts.ScriptKind.TS);
  const violations: Violation[] = [];
  const report = (node: ts.Node, reason: string): void => {
    violations.push({ relPath, line: file.getLineAndCharacterOfPosition(node.getStart(file)).line + 1, reason });
  };

  const checkSpecifier = (specifier: ts.Node, declaration?: ts.ImportDeclaration | ts.ExportDeclaration): void => {
    // A template literal or a variable hides the module from review: always a violation.
    if (!ts.isStringLiteral(specifier)) {
      report(specifier, 'module specifier is not a plain string literal');
      return;
    }
    const text = specifier.text;
    if (text.startsWith('./') || text.startsWith('../')) {
      const target = posix.normalize(posix.join(posix.dirname(relPath), text));
      if (!target.startsWith(`${SRC_DIR}/`)) report(specifier, `relative import leaves src/: ${text}`);
      return;
    }
    if (ALLOWED_SPECIFIERS.has(text)) return;
    if (text === CONNECT_NODE && relPath === CONNECT_NODE_FILE && declaration !== undefined && ts.isImportDeclaration(declaration)) {
      const clause = declaration.importClause;
      const bindings = clause?.namedBindings;
      if (clause !== undefined && clause.name === undefined && bindings !== undefined && ts.isNamedImports(bindings)) {
        const names = bindings.elements.map((e) => (e.propertyName ?? e.name).text);
        if (names.length > 0 && names.every((n) => CONNECT_NODE_IMPORTS.has(n))) return;
      }
    }
    report(specifier, `module not on the allowlist: ${text}`);
  };

  const visit = (node: ts.Node): void => {
    if ((ts.isImportDeclaration(node) || ts.isExportDeclaration(node)) && node.moduleSpecifier !== undefined) {
      checkSpecifier(node.moduleSpecifier, node);
    } else if (ts.isCallExpression(node) && (node.expression.kind === ts.SyntaxKind.ImportKeyword || (ts.isIdentifier(node.expression) && node.expression.text === 'require'))) {
      const [first] = node.arguments;
      if (first === undefined) report(node, 'import()/require() without an argument');
      else checkSpecifier(first);
    }
    if (ts.isIdentifier(node) && relPath !== CHAT_FILE && BANNED_IDENTIFIERS.has(node.text)) {
      report(node, `network identifier outside ${CHAT_FILE}: ${node.text}`);
    }
    ts.forEachChild(node, visit);
  };
  visit(file);
  return violations;
}

describe('SMA-575 AC 2 — findViolations negative controls', () => {
  // [fixture, relPath, expected violation count]. Code strings in an ARRAY, never in comments, so
  // stripping comments cannot make a control inert.
  const CASES: readonly (readonly [string, string, number])[] = [
    ['const r = fetch(url);', 'src/x.ts', 1],
    ['const f = globalThis.fetch;', 'src/x.ts', 2],
    ['type F = typeof fetch;', 'src/x.ts', 1],
    ["const f = globalThis['fe' + 'tch'];", 'src/x.ts', 1],
    ["const h = process.getBuiltinModule('node:https');", 'src/x.ts', 1],
    ['const s = new WebSocket(u);', 'src/x.ts', 1],
    ['export const read = (r: Response) => r;', 'src/x.ts', 1],
    ["import 'node:https';", 'src/x.ts', 1],
    ["import type { Dispatcher } from 'undici';", 'src/x.ts', 1],
    ["export * from 'undici';", 'src/x.ts', 1],
    ["export type { Dispatcher } from 'undici';", 'src/x.ts', 1],
    ["const m = await import('node:http');", 'src/x.ts', 1],
    ['const m = await import(`undici`);', 'src/x.ts', 1],
    ['const m = await import(name);', 'src/x.ts', 1],
    // An ALLOWED module behind a template literal: only the non-literal rule can catch this one.
    ['const m = await import(`server-only`);', 'src/x.ts', 1],
    ["const a = require('axios');", 'src/x.ts', 1],
    ["import { createNodeHttpClient } from '@connectrpc/connect-node';", 'src/transport.ts', 1],
    ["import { createGrpcTransport } from '@connectrpc/connect-node';", 'src/x.ts', 1],
    ["export { createGrpcTransport } from '@connectrpc/connect-node';", 'src/transport.ts', 1],
    ["import { x } from '../../paigasus-auth/src/x';", 'src/x.ts', 1],
    ['// fetch is mentioned here\n/** See {@link fetch}. @param fetch unused */\nexport const a = 1;', 'src/x.ts', 0],
    ['const r = await fetch(u);', 'src/chat.ts', 0],
    ["import 'node:https';", 'src/chat.ts', 1],
    ["import { fetch } from './x';", 'src/errors/chat.ts', 1],
    ["import { createGrpcTransport, Http2SessionManager } from '@connectrpc/connect-node';", 'src/transport.ts', 0],
    [
      [
        "import './server-guard';",
        "import { createClient } from '@connectrpc/connect';",
        "import type { DescService } from '@bufbuild/protobuf';",
        "import { ErrorReason } from '@paigasus/proto';",
        "import { OutboxService } from '@paigasus/proto/iam';",
        "import { mapError } from './errors/map-error';",
        "export { y } from '../src/y';",
        "import 'server-only';",
      ].join('\n'),
      'src/x.ts',
      0,
    ],
  ];

  it.each(CASES)('fixture %j at %s yields %i violation(s)', (source, relPath, expected) => {
    expect(findViolations(relPath, source)).toHaveLength(expected);
  });
});
```

Run: `pnpm -C ts/packages/paigasus-sdk exec vitest run tests/http-surface.test.ts`
Expected: PASS (26 tests). If a count differs, print `findViolations(relPath, source)` for that fixture and fix the CHECKER, not the expected count, unless the spec § 4.4 table itself says otherwise. The spec table is authoritative.

- [ ] **Step 2: Add the real-tree assertions**

Append to the same file:

```ts
function listSourceFiles(): string[] {
  // readdirSync with recursive: true — no glob dependency; Node 24 supports it natively.
  return readdirSync(resolve(PKG_ROOT, SRC_DIR), { recursive: true, encoding: 'utf8' })
    .map((entry) => `${SRC_DIR}/${entry.split(sep).join('/')}`)
    .filter((relPath) => statSync(resolve(PKG_ROOT, relPath)).isFile())
    .sort();
}

function literalTexts(node: ts.Node, out: string[] = []): string[] {
  if (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node)) out.push(node.text);
  else if (ts.isTemplateExpression(node)) {
    out.push(node.head.text);
    for (const span of node.templateSpans) out.push(span.literal.text);
  }
  ts.forEachChild(node, (child) => {
    literalTexts(child, out);
  });
  return out;
}

describe('SMA-575 AC 2 — the real src/ tree', () => {
  const files = listSourceFiles();

  it('walks at least the ten files that exist today', () => {
    expect(files.length).toBeGreaterThanOrEqual(10);
  });

  it('holds only .ts files, so nothing hides from the walk', () => {
    expect(files.filter((f) => !f.endsWith('.ts'))).toEqual([]);
  });

  it('has every package entry point under ./src/', () => {
    const pkg = JSON.parse(readFileSync(resolve(PKG_ROOT, 'package.json'), 'utf8')) as { exports: Record<string, string> };
    expect(Object.values(pkg.exports).filter((target) => !target.startsWith(`./${SRC_DIR}/`))).toEqual([]);
  });

  it('has no violation in any file', () => {
    const violations = files.filter((f) => f.endsWith('.ts')).flatMap((f) => findViolations(f, readFileSync(resolve(PKG_ROOT, f), 'utf8')));
    expect(violations).toEqual([]);
  });

  describe('the chat exception is live and narrow', () => {
    const chat = ts.createSourceFile(CHAT_FILE, readFileSync(resolve(PKG_ROOT, CHAT_FILE), 'utf8'), ts.ScriptTarget.ESNext, true, ts.ScriptKind.TS);

    it('makes exactly one call through the fetch seam', () => {
      let seamCalls = 0;
      const visit = (node: ts.Node): void => {
        if (ts.isCallExpression(node) && ts.isIdentifier(node.expression) && node.expression.text === 'fetchImpl') seamCalls += 1;
        ts.forEachChild(node, visit);
      };
      visit(chat);
      expect(seamCalls).toBe(1);
    });

    it('names no gateway path other than /v1/chat/completions', () => {
      expect(literalTexts(chat).filter((text) => text.includes('/v1/'))).toEqual(['/v1/chat/completions']);
    });
  });
});
```

Run: `pnpm -C ts/packages/paigasus-sdk exec vitest run tests/http-surface.test.ts`
Expected: PASS (32 tests).

If 'has no violation in any file' fails on today's tree, do NOT change `src/` and do NOT widen the lists on your own. Print the violations, STOP, and report them: the spec (§ 4.2) states that no such identifier exists outside `chat.ts` today, so a failure means the spec is wrong and needs a decision.

- [ ] **Step 3: Mutation battery (spec § 4.4)**

Clean-tree baseline first: the Step 2 command passes. Then, for each row, apply with Edit, run the Step 2 command, record the failing test name, restore with Edit, confirm PASS.

| # | Mutation | Expected failure |
|---|---|---|
| 1 | Append `export const probe = typeof fetch;` to `src/iam.ts` | 'has no violation in any file' (rule 2) |
| 2 | Add `import 'node:https';` as the last import line of `src/transport.ts` | 'has no violation in any file' (rule 1) |
| 3 | In `src/transport.ts`, add `, createConnectTransport` inside the `@connectrpc/connect-node` import braces (the file will not type-check; vitest does not type-check, so the guard is what fails) | 'has no violation in any file' |
| 4 | In `src/chat.ts:239`, change `fetchImpl(url, {` to `(fetchImpl)(url, {` | 'makes exactly one call through the fetch seam' |
| 5 | In `src/chat.ts:206`, change `/v1/chat/completions` to `/v1/embeddings` | 'names no gateway path other than /v1/chat/completions' |
| 6 | In the test file, change `const SRC_DIR = 'src';` to `const SRC_DIR = 'src/errors';` | 'walks at least the ten files' |
| 7 | In `findViolations`, delete `relPath !== CHAT_FILE && ` | the `src/chat.ts` fixture row expecting 0 and the real-tree test both fail |
| 8 | In `checkSpecifier`, change `if (!ts.isStringLiteral(specifier))` to `if (!ts.isStringLiteral(specifier) && !ts.isNoSubstitutionTemplateLiteral(specifier))` | the ``import(`server-only`)`` fixture row fails (0 violations instead of 1). The ``import(`undici`)`` row still passes, because `undici` is off the allowlist either way |

Run all eight even if one surprises you. After any fix, run all eight again. When done: `git diff -- ts/packages/paigasus-sdk/src` must be empty.

- [ ] **Step 4: Correct the stale `package.json` comment**

In `ts/packages/paigasus-sdk/package.json`, replace ONLY the value of `_comment_entries` with:

```json
"_comment_entries": "Five entry points: `.` (the root barrel), `./iam` (the generated Connect-ES clients and createIamClient), `./chat` (the gateway's OpenAI-compatible chat client — the ONLY hand-written HTTP surface, enforced by tests/http-surface.test.ts, SMA-575), `./errors`, and `./errors/types` (types only, deliberately unguarded). tests/server-guard.test.ts is driven off THIS map, so a new entry is covered the day it is added. tests/http-surface.test.ts asserts every target stays under ./src/.",
```

Run: `node -e "JSON.parse(require('fs').readFileSync('ts/packages/paigasus-sdk/package.json','utf8'))" && echo ok`
Expected: `ok`.

- [ ] **Step 5: Lint, format, test, commit**

Run: `moon run paigasus-sdk-ts:test paigasus-sdk-ts:build ts:lint ts:fmt`
Expected: all pass. `ts:lint` uses type-aware rules; fix any finding in the TEST file only.

```bash
git add ts/packages/paigasus-sdk/tests/http-surface.test.ts ts/packages/paigasus-sdk/package.json
git commit -m "test(ts): allow hand-written HTTP only in the sdk chat client (SMA-575)

An AST guard over src/ enforces an import allowlist and bans network
identifiers outside chat.ts. chat.ts may call only the chat endpoint.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>"
```

Report the mutation table with the recorded results.

---

### Task 4: Full verification (controller)

Spec § 9. The controller runs this task, not an implementer subagent.

- [ ] **Step 1:** `git log --oneline origin/main..HEAD` shows the two spec commits, the plan commit and the three task commits. `git diff origin/main --stat` touches only: the spec, the plan, `tests/server-guard.test.ts`, `tests/iam-factory.test.ts`, `tests/http-surface.test.ts`, `package.json`, `moon.yml` (sdk), and `ci/affected-graph/run.sh`.
- [ ] **Step 2:** Run the full CI target list from CLAUDE.md (the command between the `ci-targets` markers), with `--base origin/main`. Record the pass/fail list from moon's own summary.
- [ ] **Step 3:** CLAUDE.md records that no single local bash passes every gate. Re-run separately: `/bin/bash ci/affected-graph/run.sh` (needs 3.2), `/opt/homebrew/bin/bash ci/ruff/run.sh` and `/opt/homebrew/bin/bash ci/next-public/run.sh` (need 4+). `repo:actionlint` has no working local bash; its verdict comes from CI. Report each result separately and say which ones the local `moon ci` verdict does not cover.
- [ ] **Step 4:** If a failure has no obvious cause, follow the "Diagnosing an unattributed `moon ci` failure" procedure in CLAUDE.md, starting with step 0 (copy the cache artifacts before any re-run). <!-- moon-diagnosis:ok -->

### Task 5: ADR-0018 Amendment A2 and the Linear comment (controller, after the PR is open)

Spec § 5 and § 1.1. The controller runs this task after Stage 6 opens the PR, and before GATE 2.

- [ ] **Step 1:** Fetch ADR-0018 (`https://app.notion.com/p/3bb830e8fbaa81579b5cc146e505173a`) again. Confirm that Amendment A1 is unchanged since 2026-09-16 and that no A2 exists.
- [ ] **Step 2:** Under the A1 heading, add one line: "Partly corrected by A2 (2026-09-16)."
- [ ] **Step 3:** Append "Amendment A2 — 2026-09-16 (SMA-575)" after A1, in the A1 format and in Simplified Technical English: a status line (amendment accepted; ADR status unchanged), A2.1 (erratum: `@paigasus/sdk/chat`, not `/http`; the SMA-508 spec already used `/chat`; membership unchanged), A2.2 (the downstream clause is live: `iam-console` and `@paigasus/console-core` via SMA-511 and SMA-512, `gateway-console` via SMA-512; refer to the `contracts->proto` case in `ci/affected-graph/run.sh` by name, and do not copy the id list), A2.3 (the two controls: `tests/iam-factory.test.ts` and `tests/http-surface.test.ts`, with the PR link; re-check the file names against the PR head first). Record that the A1.1 line references were checked on 2026-09-16 and still match.
- [ ] **Step 4:** Check the ADR index table in Notion. A2 changes no status, so expect no edit. Record what the table shows.
- [ ] **Step 5:** Add a Linear comment on SMA-575: what earlier issues already delivered (SMA-508, SMA-624, SMA-625), the two errors in the issue text (spec § 1.1), the PR link, and the ADR amendment. Do not edit the issue description.
