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
export const authContextKey = createContextKey<Auth>({ anonymous: true }, { description: '@paigasus/sdk per-call authorization' });

export const authInterceptor: Interceptor = (next) => async (req) => {
  const auth = req.contextValues.get(authContextKey);
  if ('bearer' in auth) {
    // An empty or whitespace-only bearer is refused here, at the point it would be BOUND into the
    // outgoing header, rather than sent as `Authorization: Bearer `. A caller that reads an unset
    // environment variable into `bearer` gets a clear local error naming the cause, instead of a
    // confusing server-side parse failure on the other end of the call.
    if (auth.bearer.trim() === '') {
      throw new Error(
        '@paigasus/sdk: refusing to send an empty or whitespace-only bearer token. This usually means an unset environment variable; use { anonymous: true } for an intentionally unauthenticated call.',
      );
    }
    req.header.set('authorization', `Bearer ${auth.bearer}`);
  }
  return next(req);
};

/**
 * The complete set of keys `TransportOptions` is allowed to carry. `serialize` below walks
 * whatever OWN keys an options object happens to have, and TypeScript's excess-property check
 * only fires against an object LITERAL — a variable typed wider than `TransportOptions` (or built
 * up with `Object.assign`, or read from `JSON.parse`) passes `getTransport` with no cast at all.
 * MEASURED: `const wider = { baseUrl: '...', bearer: 'SECRET' }; getTransport(wider);` typechecks
 * at rc 0. Without this list, `stableTransportKey` would then silently fold `bearer` into the
 * cache key — forking a brand-new `Http2SessionManager` and HTTP/2 session per distinct token,
 * with no eviction (the cache is deliberately unbounded, spec § 7.3), and retaining every bearer
 * string as a Map key for the process's lifetime. Extend this set, in lockstep with
 * `TransportOptions`, the day a real second field (`nodeOptions`) is added.
 */
const TRANSPORT_OPTION_KEYS: ReadonlySet<string> = new Set(['baseUrl']);

/**
 * A stable serialization of the whole options object: keys sorted at every depth, `undefined`
 * dropped so an explicitly-absent option keys the same as an omitted one.
 *
 * `TransportOptions` is deliberately restricted to JSON-serializable data — not merely "not a
 * function". MEASURED: `serialize` collapses ANY object with no own enumerable keys to the
 * literal string `{}`, regardless of its actual identity or content — a `Date`, a `Map`, a `Set`,
 * or a class instance all alias to that same `{}`. The concrete future risk is a `nodeOptions`
 * with a `secureContext` (a `tls.SecureContext` object) or a `checkServerIdentity` (a callback
 * function): two different values there would serialize identically and silently alias two
 * different trust configurations onto one cached transport — precisely the failure this
 * whole-object key exists to prevent. A future option that is not plain JSON data needs its own
 * identity contribution rather than this function's default handling.
 *
 * A THIRD, separate risk — an option TypeScript never rejects at all, because excess-property
 * checking only applies to an object literal — is closed by `TRANSPORT_OPTION_KEYS` above: an own
 * key outside that set throws here rather than silently joining the cache key. This also catches
 * the opposite mistake, a future field added to `TransportOptions` but forgotten in that list.
 */
export function stableTransportKey(options: TransportOptions): string {
  for (const key of Object.keys(options)) {
    if (!TRANSPORT_OPTION_KEYS.has(key)) {
      throw new Error(`@paigasus/sdk: TransportOptions carries an undeclared key "${key}". Add it to TRANSPORT_OPTION_KEYS in transport.ts if it is meant to be part of the transport's identity.`);
    }
  }
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
