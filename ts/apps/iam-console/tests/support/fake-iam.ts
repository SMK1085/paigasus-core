// SPDX-License-Identifier: Apache-2.0
//
// An in-process fake of IAM for the integration and e2e tiers (spec § 9.1): a real gRPC server over
// h2c for the five services the console calls, and a plain HTTP server for `GET /v1/service-info`,
// which @paigasus/discovery probes.
//
// It copies these IAM behaviours, which the console depends on. Each one names its source:
//   - Introspect is exempt from bearer enforcement and never provisions. It answers
//     `identity-not-provisioned` until the token has made one bearer-enforced call
//     (rs/crates/services/paigasus-iam/src/adapters/grpc/authn.rs:139-141, :178;
//     application/authenticate_token.rs:104-105).
//   - EVERY other RPC is bearer-enforced, and provisions the token (authn.rs:182-190). So does the
//     HTTP route (adapters/http/service_info.rs:10, auth_middleware.rs:54).
//   - Introspect always returns an empty `role_grants` (authenticate_token.rs:161-165).
//   - On a gRPC call, an incoming `paigasus-correlation-id` in the HYPHENATED UUID form (8-4-4-4-12
//     hex digits, any case) is adopted and echoed in lower case. Any other value, and a missing
//     header, gets a minted id. Every gRPC error carries the id in
//     `ErrorInfo.metadata["correlation_id"]` (adapters/grpc/convert.rs:59-74).
//   - A denial is `PermissionDenied` + `ErrorInfo(domain "iam.paigasus.io", reason "forbidden",
//     metadata { retryable: "false" })` (convert.rs:111-131).
//
// Where the fake does LESS than IAM. No current test depends on these differences. A new test that
// needs one of them must extend the fake first, or it tests the fake and not IAM:
//   - IAM adopts every form that `Uuid::parse_str` accepts: hyphenated, simple (32 hex digits),
//     braced and `urn:uuid:` (rs/crates/libs/paigasus-observability/src/correlation.rs:103-119). It
//     echoes the hyphenated lower-case form. The fake adopts only the hyphenated form, so it mints a
//     new id where IAM keeps a simple, braced or `urn:uuid:` id.
//   - IAM mints a UUIDv7 (correlation.rs:94-101). The fake mints a UUIDv4 (`randomUUID()`).
//   - IAM also puts `request_id` in `ErrorInfo.metadata` (convert.rs:69-72) and sets a
//     `paigasus-request-id` response header (correlation.rs:174). The fake sets no `request_id`.
//   - IAM's HTTP routes run the same correlation layer. The fake's HTTP route only records the
//     incoming header; it adopts, mints and echoes no id.
//   - IAM's authorization is Cedar default-DENY: a request allowed by no policy is denied. The
//     fake's UNSCRIPTED `authz.isAuthorized` answers `{ allowed: true }` (see `defaults()`), so it
//     default-ALLOWS. This is the divergence most likely to make a test lie, because every user
//     action in the console calls IAM: a test that reads an unscripted allow as proof of an
//     authorization decision proves nothing. A test about authorization must SCRIPT
//     `authz.isAuthorized` — with `denial()` for the deny arm — rather than rely on this default,
//     which exists only so that a test about something else need not script it.
//
// It imports NO `server-only` module. The Playwright e2e harness (a worker-scoped fixture) loads it
// under plain Node, where `server-only` resolves to its throwing default export. That is why the
// service descriptors come from @paigasus/proto (allowed under tests/support/ by the `apps`
// boundary rule's ignore) and not from @paigasus/sdk.
import { randomUUID } from 'node:crypto';
import { createServer as createHttpServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { createServer as createHttp2Server, type ServerHttp2Session } from 'node:http2';
import type { AddressInfo } from 'node:net';
import type { DescMessage, DescService, MessageInitShape, MessageShape } from '@bufbuild/protobuf';
import { Code, ConnectError, type ConnectRouter, type HandlerContext } from '@connectrpc/connect';
import { connectNodeAdapter } from '@connectrpc/connect-node';
import { ErrorInfoSchema, ServiceInfoService } from '@paigasus/proto';
import { AuditService, AuthnService, AuthorizationService, TenancyService } from '@paigasus/proto/iam';

/** IAM's error domain on the wire (ErrorDomain.IAM through the registry's mapping rule). */
export const IAM_ERROR_DOMAIN = 'iam.paigasus.io';
/** The issuer the default Introspect answer reports. */
export const FAKE_IAM_ISSUER = 'https://idp.fake-iam.test';

const CORRELATION_HEADER = 'paigasus-correlation-id';
const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

const SERVICES = {
  tenancy: TenancyService,
  authn: AuthnService,
  authz: AuthorizationService,
  audit: AuditService,
  serviceInfo: ServiceInfoService,
} as const;

type ServiceMap = typeof SERVICES;
type ServiceKey = keyof ServiceMap;
type MethodsOf<S extends ServiceKey> = ServiceMap[S]['method'];

type Entry = {
  [S in ServiceKey]: {
    [M in Extract<keyof MethodsOf<S>, string>]: { key: `${S}.${M}`; desc: MethodsOf<S>[M] };
  }[Extract<keyof MethodsOf<S>, string>];
}[ServiceKey];

/** Every RPC the fake serves, as `<client key>.<method localName>` — the same keys as `IamClients`. */
export type FakeIamMethod = Entry['key'];

/** What a scripted handler receives besides the request. */
export type FakeIamContext = {
  /** The bearer token of the call, or null when the call carried none. */
  readonly token: string | null;
  /** The correlation id IAM adopted or minted for this call. */
  readonly correlationId: string;
};

type HandlerFor<D> = D extends { input: infer I extends DescMessage; output: infer O extends DescMessage }
  ? (request: MessageShape<I>, context: FakeIamContext) => MessageInitShape<O> | Promise<MessageInitShape<O>>
  : never;

/** Scripted answers, keyed by method. A handler may throw — use `denial()` for an IAM denial. */
export type FakeIamHandlers = { [E in Entry as E['key']]?: HandlerFor<E['desc']> };

export type FakeIamCall = {
  /** A `FakeIamMethod`, or `http.getServiceInfo` for the HTTP route. */
  readonly method: string;
  readonly token: string | null;
  /** The `paigasus-correlation-id` request header as it ARRIVED, or null. */
  readonly correlationId: string | null;
  readonly request: unknown;
};

export type ServiceDescriptorBody = { service: string; version: string; capabilities: string[] };

export type FakeIam = {
  readonly grpcUrl: string;
  readonly httpUrl: string;
  readonly calls: FakeIamCall[];
  callsTo(method: string): FakeIamCall[];
  /** Tokens that have made a bearer-enforced call, gRPC or HTTP. */
  readonly provisioned: Set<string>;
  /** The principal PRN the default Introspect answer reports for this token. Stable per token. */
  principalPrnFor(token: string): string;
  /**
   * REPLACES the whole handler map; it does not merge. A method missing from the new map falls back
   * to the built-in behaviour. Each fake is its own instance with no module state, so a test worker
   * can start, script and close one per test or per worker.
   */
  setHandlers(handlers: FakeIamHandlers): void;
  /**
   * A descriptor changes both the gRPC and the HTTP answer. `{ status }` changes ONLY the HTTP
   * route (the discovery probe): the gRPC GetServiceInfo keeps the last descriptor, because it is
   * also the provisioning call and a degraded-discovery scenario must not fail every first login.
   */
  setServiceInfo(descriptor: ServiceDescriptorBody | { status: number }): void;
  close(): Promise<void>;
};

const DEFAULT_DESCRIPTOR: ServiceDescriptorBody = { service: 'iam', version: '0.0.0-fake', capabilities: ['iam.authz.cedar', 'iam.audit'] };

/** Calls IAM serves WITHOUT bearer enforcement (authn.rs:139-141). */
const UNENFORCED: ReadonlySet<string> = new Set(['authn.introspect', 'authn.introspectApiKey']);

/** A ConnectError shaped the way IAM's `iam_status` shapes one (convert.rs:76-80). */
function iamError(code: Code, reason: string, message: string, correlationId?: string): ConnectError {
  const metadata: Record<string, string> = { retryable: 'false' };
  if (correlationId !== undefined) metadata['correlation_id'] = correlationId;
  return new ConnectError(message, code, undefined, [{ desc: ErrorInfoSchema, value: { reason, domain: IAM_ERROR_DOMAIN, metadata } }]);
}

/**
 * An IAM denial: `PermissionDenied` + ErrorInfo(domain `iam.paigasus.io`, reason `forbidden`,
 * metadata `{ retryable: "false" }`). When `correlationId` is omitted, the fake fills in the id of
 * the call that throws it, exactly as IAM does inside a request scope.
 */
export function denial(opts: { reason?: string; correlationId?: string; code?: Code } = {}): ConnectError {
  return iamError(opts.code ?? Code.PermissionDenied, opts.reason ?? 'forbidden', 'the principal is not allowed to perform this action', opts.correlationId);
}

/**
 * The `google.rpc.ErrorInfo` detail a failed call carried, or null when it carried none.
 *
 * A test cannot read this for itself: `findDetails` needs `ErrorInfoSchema`, and only
 * `tests/support/**` may import @paigasus/proto (the `apps` boundary rule's ignore). It is also
 * the ONLY way to assert the detail's `correlation_id`. `mapError` reads that key first and falls
 * back to the `paigasus-correlation-id` RESPONSE HEADER, which the fake sets to the same value —
 * so through `mapError` alone the two are indistinguishable, and a fake that stopped writing the
 * key would keep every assertion green (MEASURED, by removing the stamping).
 */
export function errorInfoOf(error: unknown): { reason: string; domain: string; metadata: Record<string, string> } | null {
  if (!(error instanceof ConnectError)) return null;
  const detail = error.findDetails(ErrorInfoSchema)[0];
  return detail === undefined ? null : { reason: detail.reason, domain: detail.domain, metadata: detail.metadata };
}

function bearerOf(value: string | null | undefined): string | null {
  if (value === null || value === undefined) return null;
  const match = /^Bearer (.+)$/.exec(value);
  return match?.[1] ?? null;
}

/** Adds `correlation_id` to every ErrorInfo detail that lacks one. */
function stampCorrelation(err: unknown, correlationId: string): unknown {
  if (!(err instanceof ConnectError)) return err;
  for (const detail of err.details) {
    if (!('desc' in detail) || detail.desc.typeName !== ErrorInfoSchema.typeName) continue;
    const value = detail.value as { metadata?: Record<string, string> };
    const metadata = { ...(value.metadata ?? {}) };
    metadata['correlation_id'] ??= correlationId;
    detail.value = { ...value, metadata };
  }
  return err;
}

function listen(server: { listen(port: number, host: string, cb: () => void): unknown; address(): AddressInfo | string | null }): Promise<string> {
  return new Promise((resolve, reject) => {
    server.listen(0, '127.0.0.1', () => {
      const address = server.address();
      if (address === null || typeof address === 'string') {
        reject(new Error('fake IAM: could not read the listening port'));
        return;
      }
      resolve(`http://127.0.0.1:${String(address.port)}`);
    });
  });
}

export async function startFakeIam(opts: { handlers?: FakeIamHandlers } = {}): Promise<FakeIam> {
  let handlers: FakeIamHandlers = opts.handlers ?? {};
  let grpcDescriptor: ServiceDescriptorBody = DEFAULT_DESCRIPTOR;
  let httpAnswer: ServiceDescriptorBody | { status: number } = DEFAULT_DESCRIPTOR;
  const calls: FakeIamCall[] = [];
  const provisioned = new Set<string>();
  const principals = new Map<string, string>();

  const principalPrnFor = (token: string): string => {
    let prn = principals.get(token);
    if (prn === undefined) {
      prn = `prn:pgs:iam:::principal/${randomUUID()}`;
      principals.set(token, prn);
    }
    return prn;
  };

  const scripted = (method: string): ((request: unknown, context: FakeIamContext) => unknown) | undefined =>
    (handlers as Record<string, ((request: unknown, context: FakeIamContext) => unknown) | undefined>)[method];

  async function introspect(request: { token: string }, context: FakeIamContext): Promise<unknown> {
    if (!provisioned.has(request.token)) {
      throw iamError(Code.PermissionDenied, 'identity-not-provisioned', 'the identity is not provisioned');
    }
    const handler = scripted('authn.introspect');
    const extra = handler === undefined ? {} : ((await handler(request, context)) as object);
    return {
      principalPrn: principalPrnFor(request.token),
      status: 'active',
      issuer: FAKE_IAM_ISSUER,
      subject: `subject-of-${principalPrnFor(request.token).slice(-12)}`,
      memberships: [],
      ...extra,
      roleGrants: [],
    };
  }

  function defaults(method: string): unknown {
    switch (method) {
      case 'serviceInfo.getServiceInfo':
        return { serviceInfo: { ...grpcDescriptor } };
      // DEFAULT-ALLOW, where IAM's Cedar is default-DENY. See the divergence list in the header:
      // a test about authorization must script this method rather than rely on this answer.
      case 'authz.isAuthorized':
        return { allowed: true, determiningPolicies: [], reason: '' };
      case 'authz.listRoleGrants':
        return { grants: [] };
      default:
        throw new ConnectError(`fake IAM: no handler is scripted for ${method}`, Code.Unimplemented);
    }
  }

  async function dispatch(method: string, request: unknown, ctx: HandlerContext): Promise<unknown> {
    const token = bearerOf(ctx.requestHeader.get('authorization'));
    const incoming = ctx.requestHeader.get(CORRELATION_HEADER);
    const correlationId = incoming !== null && UUID_RE.test(incoming) ? incoming.toLowerCase() : randomUUID();
    ctx.responseHeader.set(CORRELATION_HEADER, correlationId);
    calls.push({ method, token, correlationId: incoming, request });
    const context: FakeIamContext = { token, correlationId };
    try {
      if (!UNENFORCED.has(method)) {
        if (token === null) throw iamError(Code.Unauthenticated, 'missing-authorization', 'a bearer token is required');
        provisioned.add(token);
      }
      if (method === 'authn.introspect') return await introspect(request as { token: string }, context);
      const handler = scripted(method);
      return handler === undefined ? defaults(method) : await handler(request, context);
    } catch (err) {
      throw stampCorrelation(err, correlationId);
    }
  }

  function routes(router: ConnectRouter): void {
    for (const [serviceKey, service] of Object.entries(SERVICES) as [ServiceKey, DescService][]) {
      const impl: Record<string, (request: unknown, ctx: HandlerContext) => Promise<unknown>> = {};
      for (const method of service.methods) {
        impl[method.localName] = (request, ctx) => dispatch(`${serviceKey}.${method.localName}`, request, ctx);
      }
      // The map is built from the descriptor itself, so every key is a real method of `service`;
      // TypeScript cannot follow that through Object.entries, hence the cast.
      router.service(service, impl as never);
    }
  }

  const sessions = new Set<ServerHttp2Session>();
  const grpcServer = createHttp2Server(connectNodeAdapter({ routes }));
  grpcServer.on('session', (session) => {
    sessions.add(session);
    session.once('close', () => sessions.delete(session));
  });

  function handleHttp(req: IncomingMessage, res: ServerResponse): void {
    const url = new URL(req.url ?? '/', 'http://fake-iam.invalid');
    const json = (status: number, body: unknown): void => {
      res.writeHead(status, { 'content-type': 'application/json' });
      res.end(JSON.stringify(body));
    };
    if (req.method !== 'GET' || url.pathname !== '/v1/service-info') {
      json(404, { error: { code: 'not-found', message: 'no such route' } });
      return;
    }
    const token = bearerOf(req.headers.authorization);
    const header = req.headers[CORRELATION_HEADER];
    calls.push({ method: 'http.getServiceInfo', token, correlationId: typeof header === 'string' ? header : null, request: null });
    if (token === null) {
      json(401, { error: { code: 'missing-authorization', message: 'a bearer token is required' } });
      return;
    }
    provisioned.add(token);
    if ('status' in httpAnswer) {
      json(httpAnswer.status, { error: { code: 'internal', message: 'the fake IAM is configured to fail' } });
      return;
    }
    json(200, httpAnswer);
  }
  const httpServer = createHttpServer(handleHttp);

  const [grpcUrl, httpUrl] = await Promise.all([listen(grpcServer), listen(httpServer)]);

  return {
    grpcUrl,
    httpUrl,
    calls,
    callsTo: (method) => calls.filter((call) => call.method === method),
    provisioned,
    principalPrnFor,
    setHandlers(next) {
      handlers = next;
    },
    setServiceInfo(next) {
      httpAnswer = next;
      if (!('status' in next)) grpcDescriptor = next;
    },
    async close() {
      for (const session of sessions) session.destroy();
      await Promise.all([new Promise<void>((resolve) => grpcServer.close(() => resolve())), new Promise<void>((resolve) => httpServer.close(() => resolve()))]);
    },
  };
}
