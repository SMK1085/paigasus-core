// SPDX-License-Identifier: Apache-2.0
import './server-guard.js';

import { createContextKey } from '@connectrpc/connect';
import type { Interceptor, Transport } from '@connectrpc/connect';
import { createGrpcTransport, Http2SessionManager } from '@connectrpc/connect-node';

/**
 * The identity of a transport.
 *
 * ONE field today, and the whole object is the cache key anyway. That is the point: spec § 7.1's
 * rule exists for the SECOND field. `createGrpcTransport` also accepts `nodeOptions` — the TLS
 * trust material — and this repo already carries four CA-bundle knobs with divergent semantics, so
 * a second option is a question of when, not whether. Keying on the base URL alone would let two
 * different trust configurations share one transport.
 *
 * When `nodeOptions` is added it must go to the `Http2SessionManager` CONSTRUCTOR (its third
 * argument), not to `createGrpcTransport`: connect-node documents that supplying `sessionManager`
 * makes a `nodeOptions` passed to the transport ineffective.
 */
export type TransportOptions = {
  readonly baseUrl: string;
};

/**
 * A union rather than an optional, so that an unauthenticated call — the health check is the real
 * case — is a written decision rather than an omission (spec § 7.4).
 */
export type Auth = { readonly bearer: string } | { readonly anonymous: true };

/**
 * An unset deadline lets a gRPC call in a Next server component hang the request forever. That is a
 * production hazard, not a default worth inheriting (spec § 7.2). Callers override per call via
 * `CallOptions.timeoutMs`.
 */
export const DEFAULT_TIMEOUT_MS = 10_000;

/**
 * The per-call authorization channel.
 *
 * The interceptor lives on the transport and the transport is cached, so the interceptor cannot
 * close over a token — it has to read one per call. `createContextKey` requires a default; the
 * default is `anonymous` so that a code path which somehow reaches the interceptor without a bound
 * Auth sends NO credential rather than a stale one. In practice the default is unreachable:
 * `createIamClient` takes `Auth` as a required parameter (spec § 7.4).
 */
export const authContextKey = createContextKey<Auth>(
  { anonymous: true },
  { description: '@paigasus/sdk per-call authorization' },
);

export const authInterceptor: Interceptor = (next) => async (req) => {
  const auth = req.contextValues.get(authContextKey);
  if ('bearer' in auth) {
    req.header.set('authorization', `Bearer ${auth.bearer}`);
  }
  return next(req);
};

/**
 * A stable serialization of the whole options object: keys sorted at every depth, `undefined`
 * dropped so an explicitly-absent option keys the same as an omitted one.
 *
 * `TransportOptions` is deliberately restricted to JSON-serializable data. A function-valued option
 * would serialize identically for two different functions and silently alias two transports, so a
 * future option that is not plain data needs its own identity contribution rather than this
 * function's default handling.
 */
export function stableTransportKey(options: TransportOptions): string {
  return serialize(options);
}

function serialize(value: unknown): string {
  if (value === undefined) return 'undefined';
  if (value === null || typeof value !== 'object') return JSON.stringify(value) ?? 'undefined';
  if (Array.isArray(value)) return `[${value.map(serialize).join(',')}]`;
  const entries = Object.entries(value as Record<string, unknown>)
    .filter(([, v]) => v !== undefined)
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0));
  return `{${entries.map(([k, v]) => `${JSON.stringify(k)}:${serialize(v)}`).join(',')}}`;
}

type CacheEntry = {
  readonly transport: Transport;
  readonly sessionManager: Http2SessionManager;
};

/**
 * Nothing evicts, and that is deliberate (spec § 7.3). Under a Next server a handful of long-lived
 * transports to a fixed set of in-cluster services is the intended shape.
 */
const cache = new Map<string, CacheEntry>();

export function getTransport(options: TransportOptions): Transport {
  const key = stableTransportKey(options);
  const hit = cache.get(key);
  if (hit !== undefined) return hit.transport;

  // The session manager is held so disposeTransports() can close the connection. Without a handle
  // there is no way to close an HTTP/2 session, and an open session keeps the Node process alive.
  const sessionManager = new Http2SessionManager(options.baseUrl);
  const transport = createGrpcTransport({
    baseUrl: options.baseUrl,
    sessionManager,
    defaultTimeoutMs: DEFAULT_TIMEOUT_MS,
    interceptors: [authInterceptor],
  });

  cache.set(key, { transport, sessionManager });
  return transport;
}

/**
 * Close every cached transport's HTTP/2 session and empty the cache.
 *
 * An open HTTP/2 session keeps a Node process alive, so `vitest run` can hang after the assertions
 * pass without this. Call it from `afterAll`, and from a server's shutdown path.
 */
export function disposeTransports(): void {
  for (const { sessionManager } of cache.values()) {
    sessionManager.abort();
  }
  cache.clear();
}
