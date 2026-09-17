// SPDX-License-Identifier: Apache-2.0
//
// FIX 1 (final whole-branch review, SMA-508) — `createIamClient` was unguarded by any test.
// MEASURED: replacing its body with `return createClient(service, getTransport(options));` —
// discarding `auth` entirely — left the suite at 23/23 passing. Every token-isolation proof in
// tests/iam.test.ts drives the exported-for-test `bindAuth` directly; the only test that touches
// the production factory asserted `typeof client.getOrganization === 'function'`, which bare
// `createClient` also satisfies. This file drives `createIamClient` itself, end to end, by
// mocking `@connectrpc/connect-node` the way tests/transport-wiring.test.ts:27-32 already does,
// so `getTransport`'s real `createGrpcTransport` call is redirected to a recording Transport.
//
// A separate file from tests/iam.test.ts, not an addition to it: `vi.mock` applies to the WHOLE
// file it is declared in, and iam.test.ts's existing tests deliberately exercise `bindAuth`
// against a hand-built Transport with no module mocking involved — mixing the two here would mock
// `@connectrpc/connect-node` for tests that neither need nor expect it.
import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import ts from 'typescript';
import { afterEach, describe, expect, expectTypeOf, it, vi } from 'vitest';
import type { ContextValues, StreamResponse, Transport, UnaryResponse } from '@connectrpc/connect';
import type { DescMessage, DescMethodStreaming, DescMethodUnary, DescService, MessageInitShape, MessageShape } from '@bufbuild/protobuf';
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

// `HeadersInit` is declared by connect's own d.ts against the DOM lib, which this package's
// tsconfig deliberately excludes (spec § 6.2 layer 4), so the name itself does not resolve here.
// `unknown` is a supertype of every member of that union, so it still satisfies the interface's
// contravariant parameter position (type-only fix, forced by tsc).
type HeaderParam = unknown;

// SMA-575 AC 1. `RecordedCall` is a type alias, so the hoisting rule below does not apply to it —
// types are erased before `vi.hoisted`'s factory ever runs.
type RecordedCall = { readonly method: DescMethodUnary | DescMethodStreaming; readonly input: unknown; readonly contextValues: ContextValues | undefined };

// `vi.hoisted`, not a plain `const` — see tests/transport-wiring.test.ts:9-22 for why: vi.mock's
// factory is hoisted above every top-level statement in the file, so a factory closing over a
// plain `const` throws a TDZ ReferenceError at mock time.
const { calls, createGrpcTransport } = vi.hoisted(() => {
  const calls: RecordedCall[] = [];
  // Typed with Transport's own generics, exactly as tests/iam.test.ts's local recordingTransport
  // is — an object-literal method shorthand otherwise infers a narrower, non-generic signature.
  // Not `async`: neither method awaits anything, and @typescript-eslint/require-await treats an
  // async function with no await expression as an error (measured: `moon run ts:lint` reds on
  // exactly that rule).
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
    // Explicit call-signature type argument, not inferred from the implementation — see
    // tests/transport-wiring.test.ts:13-22 for why a zero-arg inference makes `.mock.calls[0]` a
    // zero-length tuple under vitest@5's Mock<T> typing.
    createGrpcTransport: vi.fn<(options: unknown) => Transport>(() => transport),
  };
});

vi.mock('@connectrpc/connect-node', async (importOriginal) => {
  // Spread the real module: transport.ts also imports Http2SessionManager from it, and a factory
  // returning only the spy would break the constructor call rather than the assertion.
  const actual = await importOriginal<typeof import('@connectrpc/connect-node')>();
  return { ...actual, createGrpcTransport };
});

const { authContextKey, disposeTransports } = await import('../src/transport.js');
const { createIamClient } = await import('../src/iam.js');

afterEach(() => {
  disposeTransports();
  createGrpcTransport.mockClear();
  calls.length = 0;
});

describe('createIamClient wires the bound Auth through the real production path (FIX 1, final review)', () => {
  it('binds the given Auth so a real call carries it as ContextValues', async () => {
    const client = createIamClient(TenancyService, { baseUrl: 'https://iam.invalid' }, { bearer: 'alice-token' });

    await client.getOrganization({});

    expect(calls).toHaveLength(1);
    expect(calls[0]?.contextValues?.get(authContextKey)).toEqual({ bearer: 'alice-token' });
  });
});

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
const REQUIRED_PAIRS = [
  'paigasus.iam.v1.UserService/CreateUser',
  'paigasus.iam.v1.AuthorizationService/RetireSystemPolicy',
  ...OutboxService.methods.map((m) => `${OutboxService.typeName}/${m.name}`),
];

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
    // MEASURED in Task 2 Step 2: SAME_OBJECT= true — Connect passes the init object through by
    // identity, so this asserts identity rather than deep equality.
    expect(call?.input).toBe(row.init);
    expect(call?.contextValues?.get(authContextKey)).toEqual({ bearer: 'ops-token' });
  });
});

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
