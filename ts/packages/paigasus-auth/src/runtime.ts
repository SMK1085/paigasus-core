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
import { createOidcClient, type OidcClient } from './adapters/oidc.js';
import { claimsPrincipalResolver } from './adapters/claims-resolver.js';
import { MemorySessionStore } from './adapters/memory-store.js';
import { noopLogger } from './adapters/noop-logger.js';
import { createRedisSessionStore } from './adapters/redis-store.js';
import type { AuthEnv } from './config.js';
import { AuthConfigError } from './core/errors.js';
import type { AuthLogger } from './ports/logger.js';
import type { PrincipalResolver } from './ports/principal-resolver.js';
import type { SessionStore } from './ports/session-store.js';

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
  // refresh HTTP call completing inside the lock's lifetime. "At or above" — not just "above" —
  // because a refresh that takes exactly the lock's TTL still races a second holder.
  if (cfg.PAIGASUS_OIDC_HTTP_TIMEOUT_MS >= cfg.PAIGASUS_SESSION_LOCK_TTL_MS) {
    throw new AuthConfigError('PAIGASUS_OIDC_HTTP_TIMEOUT_MS must be strictly below PAIGASUS_SESSION_LOCK_TTL_MS');
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
  };
}
