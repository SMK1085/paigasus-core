// SPDX-License-Identifier: Apache-2.0
//
// The composition root — design doc § 6.8
// (docs/superpowers/specs/2026-09-09-sma-506-auth-design.md).
//
// createAuthRuntime OWNS every cross-field rule and derivation that src/config.ts's
// `authEnvShape` cannot express. `defineRuntimeConfig` (from @paigasus/next-config) builds a flat
// `z.object({...core, ...extra})` with NO refinement hook, and it THROWS if an extra shape
// declares `PAIGASUS_ZONE` or `PAIGASUS_ZONES` — see config.ts's own header comment. So two things
// live here instead of in the shape:
//
//   - cross-field rules ("REDIS_URL is required when STORE=redis", § below);
//   - derivations that need the zone keys this package may not declare (the redirect URIs).
//
// This function therefore receives the ALREADY-PARSED composed config — this package's keys plus
// the two zone keys @paigasus/next-config owns — rather than raw env. A caller passes the result
// of its app's `getRuntimeConfig()`.
//
// DISCOVERY STAYS LAZY. createOidcClient (adapters/oidc.ts) performs no I/O by itself; the first
// login/refresh/logout triggers `openid-client`'s discovery call. That is what lets every
// cross-field-rule check below reject synchronously-fast, before any network call, even against
// an issuer URL that resolves to nothing (exactly what tests/runtime.test.ts's BASE fixture is).
//
// ONE RUNTIME PER PROCESS — THE CALLER'S CONTRACT. `createAuthRuntime` itself builds a FRESH
// OidcClient, a fresh store, and (for the redis backend) a fresh Redis connection on every
// invocation — that is deliberate, since tests call it repeatedly with different configs and
// expect independent validation each time. A real app must NOT call it per request: doing so
// would re-run `openid-client` discovery and open a new Redis connection on every request, never
// closing the old one. `getAuthRuntime` below is the process-wide singleton every other caller
// (task 8's routes, the Next binding) uses instead — it memoises the FIRST successful call on
// `globalThis` (see the comment on RUNTIME_KEY: Next gives a route handler and a page separate
// copies of this module), keyed on nothing (there is exactly one configuration per process), and
// resets on failure so a misconfigured-at-boot process can recover once the config is fixed and
// the container is asked to try again.
import { createOidcClient, type OidcClient } from './adapters/oidc';
import { claimsPrincipalResolver } from './adapters/claims-resolver';
import { MemorySessionStore } from './adapters/memory-store';
import { noopLogger } from './adapters/noop-logger';
import { createRedisSessionStore } from './adapters/redis-store';
import type { AuthEnv } from './config';
import { AuthConfigError } from './core/errors';
import type { AuthLogger } from './ports/logger';
import type { PrincipalResolver } from './ports/principal-resolver';
import type { SessionStore } from './ports/session-store';

/**
 * What createAuthRuntime actually receives: `authEnvShape`'s keys, parsed, PLUS the two zone keys
 * @paigasus/next-config's `defineRuntimeConfig` owns and this package may not declare. A caller
 * composes `authEnvShape` into `defineRuntimeConfig` and passes `getRuntimeConfig()`'s result.
 */
export type ComposedConfig = AuthEnv & {
  PAIGASUS_ZONE: string;
  PAIGASUS_ZONES: Record<string, string>;
};

export interface CreateAuthRuntimeDeps {
  store?: SessionStore;
  resolver?: PrincipalResolver;
  logger?: AuthLogger;
}

export interface AuthRuntime {
  store: SessionStore;
  resolver: PrincipalResolver;
  logger: AuthLogger;
  oidc: OidcClient;
  /**
   * `PAIGASUS_PUBLIC_ORIGIN` as parsed: an https origin with no trailing slash (config.ts
   * `httpsUrl`). `createAuthRouteHandler` (server.ts) rebuilds a route handler's request URL on it,
   * because Next gives a route handler the server's BIND address instead (SMA-511 spec § 7.1).
   */
  publicOrigin: string;
  redirectUri: string;
  postLogoutRedirectUri: string;
  /** All zones share one cookie on one origin (design doc § 6.7) — never per-zone, never configurable. */
  cookieDomainless: true;
  skewMs: number;
  lockTtlMs: number;
  lockWaitMs: number;
  ttlMs: number;
  absoluteTtlMs: number;
  zone: string;
  basePath: string;
  /** Parsed but otherwise unreachable before this — task 8 needs it to build the authorization URL. */
  scopes: string;
}

/** Invariant 1. Called once for validation and again (cheaply) when the store is actually built. */
function requireRedisUrl(cfg: ComposedConfig): string {
  const url = cfg.PAIGASUS_SESSION_REDIS_URL;
  if (url === undefined) {
    throw new AuthConfigError('PAIGASUS_SESSION_REDIS_URL is required when PAIGASUS_SESSION_STORE is "redis"');
  }
  return url;
}

export async function createAuthRuntime(cfg: ComposedConfig, deps: CreateAuthRuntimeDeps = {}): Promise<AuthRuntime> {
  // Invariant 4 first: the zone map is read for basePath below, so membership is a precondition
  // for every derivation that follows, not just its own rule.
  const basePath = cfg.PAIGASUS_ZONES[cfg.PAIGASUS_ZONE];
  if (basePath === undefined) {
    throw new AuthConfigError('PAIGASUS_ZONE has no entry in PAIGASUS_ZONES');
  }

  if (cfg.PAIGASUS_SESSION_STORE === 'redis') {
    requireRedisUrl(cfg); // Invariant 1 — validated here regardless of an injected store.
  } else if (Object.keys(cfg.PAIGASUS_ZONES).length > 1) {
    // Invariant 2: the memory store (adapters/memory-store.ts) is single-process. Under
    // multi-zone the zones share nothing — a user who logs in on zone A is anonymous on zone B —
    // and the single-flight lock is per-process, so AC 2's "exactly one refresh" guarantee does
    // not hold across processes. A startup error beats that silent wrong answer.
    throw new AuthConfigError('PAIGASUS_SESSION_STORE cannot be "memory" when PAIGASUS_ZONES declares more than one zone');
  }

  // Invariant 3: core/single-flight.ts's write-fencing (its own invariant 5) depends on the
  // refresh completing inside the lock's lifetime. The bound is 2x, not 1x, because
  // adapters/oidc.ts's `refresh()` makes TWO sequential calls under the lock, each individually
  // bounded by PAIGASUS_OIDC_HTTP_TIMEOUT_MS: the token endpoint (refreshTokenGrant), then —
  // because non-repudiation checks are always enabled (M1 addendum) and most IdPs return an
  // id_token on refresh — a JWKS fetch to verify its signature. A 1x bound (the review-round-1
  // shape) let a cold-cache refresh take up to 2x the timeout while holding a 1x-sized lock,
  // which is exactly the "a refresh outlives its lock" case invariant 5 exists to prevent.
  // "At or above" — not just "above" — because a refresh taking exactly the bound still races a
  // second holder.
  if (2 * cfg.PAIGASUS_OIDC_HTTP_TIMEOUT_MS >= cfg.PAIGASUS_SESSION_LOCK_TTL_MS) {
    throw new AuthConfigError('2x PAIGASUS_OIDC_HTTP_TIMEOUT_MS must be strictly below PAIGASUS_SESSION_LOCK_TTL_MS');
  }

  const redirectUri = cfg.PAIGASUS_OIDC_REDIRECT_URI ?? `${cfg.PAIGASUS_PUBLIC_ORIGIN}${basePath}/auth/callback`;
  const postLogoutRedirectUri = cfg.PAIGASUS_OIDC_POST_LOGOUT_REDIRECT_URI ?? `${cfg.PAIGASUS_PUBLIC_ORIGIN}${basePath}/`;

  const oidc = createOidcClient({
    issuer: cfg.PAIGASUS_OIDC_ISSUER,
    clientId: cfg.PAIGASUS_OIDC_CLIENT_ID,
    clientSecret: cfg.PAIGASUS_OIDC_CLIENT_SECRET,
    httpTimeoutMs: cfg.PAIGASUS_OIDC_HTTP_TIMEOUT_MS,
    clockToleranceSeconds: cfg.PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS,
  });

  const store =
    deps.store ??
    (cfg.PAIGASUS_SESSION_STORE === 'redis'
      ? await createRedisSessionStore({
          url: requireRedisUrl(cfg),
          commandTimeoutMs: cfg.PAIGASUS_SESSION_REDIS_TIMEOUT_MS,
          // All zones share ONE store (design doc § 6.7) — no per-zone key prefix to configure.
          keyPrefix: '',
        })
      : new MemorySessionStore());

  return {
    store,
    resolver: deps.resolver ?? claimsPrincipalResolver,
    logger: deps.logger ?? noopLogger,
    oidc,
    publicOrigin: cfg.PAIGASUS_PUBLIC_ORIGIN,
    redirectUri,
    postLogoutRedirectUri,
    cookieDomainless: true,
    skewMs: cfg.PAIGASUS_SESSION_REFRESH_SKEW_SECONDS * 1000,
    lockTtlMs: cfg.PAIGASUS_SESSION_LOCK_TTL_MS,
    lockWaitMs: cfg.PAIGASUS_SESSION_LOCK_WAIT_MS,
    ttlMs: cfg.PAIGASUS_SESSION_TTL_SECONDS * 1000,
    absoluteTtlMs: cfg.PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS * 1000,
    zone: cfg.PAIGASUS_ZONE,
    basePath,
    scopes: cfg.PAIGASUS_OIDC_SCOPES,
  };
}

/**
 * The cache lives on `globalThis`, NOT in a module-level variable, and that placement is
 * load-bearing (SMA-511 Task 22).
 *
 * MEASURED on Next 16.3.4 with Turbopack, on this repository's own standalone build: a route
 * handler and a server component get SEPARATE module graphs.
 * `.next/server/app/auth/[...auth]/route.js` loads `chunks/[turbopack]_runtime.js`, and
 * `.next/server/app/(console)/orgs/page.js` loads `chunks/ssr/[turbopack]_runtime.js` — two
 * registries, each with its own copy of this module. A module-level variable therefore gives each
 * layer its OWN runtime, and with the memory store each layer also gets its own session records:
 * `/iam/auth/callback` writes the session, the page that follows it finds none, and the browser
 * loops between the console and the identity provider until it stops (measured:
 * `net::ERR_TOO_MANY_REDIRECTS`). Redis hides the fault, because the records are outside the
 * process; the memory adapter — which this package documents as single-PROCESS — does not.
 *
 * `Symbol.for` keys the slot in the global symbol registry, so every copy of this module resolves
 * the same key with no export to import.
 */
const RUNTIME_KEY: unique symbol = Symbol.for('paigasus.auth.runtime');

type RuntimeHolder = { [RUNTIME_KEY]?: Promise<AuthRuntime> };

/**
 * The process-wide singleton. Every real caller — task 8's routes, the Next binding — uses this,
 * never `createAuthRuntime` directly, so discovery, the store adapter, and (for redis) the Redis
 * connection are built exactly ONCE per process. The promise is cached after the first call and
 * every later call's arguments are ignored, matching "one runtime per process" — this is
 * deliberate, not an oversight: a second, differently-configured call in the same process would
 * indicate a bug upstream (there is exactly one deployment configuration per running container),
 * not a legitimate need for a second runtime. A failed first call clears the cache, so a
 * misconfigured-at-boot process can recover once its config is fixed and it is asked to try again.
 *
 * `createAuthRuntime` itself is NOT memoised and stays directly callable — this package's own
 * tests (tests/runtime.test.ts) rely on that to validate many independent configurations.
 */
export function getAuthRuntime(cfg: ComposedConfig, deps?: CreateAuthRuntimeDeps): Promise<AuthRuntime> {
  const holder = globalThis as typeof globalThis & RuntimeHolder;
  let shared = holder[RUNTIME_KEY];
  if (shared === undefined) {
    shared = createAuthRuntime(cfg, deps).catch((err: unknown) => {
      delete holder[RUNTIME_KEY];
      throw err;
    });
    holder[RUNTIME_KEY] = shared;
  }
  return shared;
}
