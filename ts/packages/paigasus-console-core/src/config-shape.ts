// SPDX-License-Identifier: Apache-2.0
import 'server-only';

/**
 * The config slice this package reads. Declared structurally rather than importing an app's
 * ConsoleConfig, so both zones satisfy it without the package knowing either app exists.
 *
 * MEASURED against the two call sites that actually read it (SMA-512 PR 2, task 4, controller
 * ruling C): `iam-clients.ts`'s `iamClientsForToken()` reads `PAIGASUS_IAM_GRPC_URL`;
 * `discovery.ts`'s `descriptorCacheFor()` reads the three `PAIGASUS_SESSION_*` keys, and
 * `createAppDiscovery()` reads `PAIGASUS_SERVICES` directly plus, through
 * `@paigasus/discovery/server`'s `timingsFromEnv()`, the six `PAIGASUS_DISCOVERY_*_MS` keys. A
 * missing key here is a silent behaviour change: discovery would fall back to `DEFAULT_TIMINGS`
 * instead of the operator's configured values, with no error at all.
 *
 * `PAIGASUS_SERVICES` stays a plain (non-`Readonly`) `Record`: `@paigasus/discovery/server`'s
 * `timingsFromEnv(config: DiscoveryEnv)` infers that field as a mutable `Record<string, string>`,
 * and a `Readonly<Record<string, string>>` here is NOT assignable to it — TypeScript treats a
 * `Record<string, V>`'s implicit index signature as readonly-checked, unlike a plain named
 * property. `createAppDiscovery`'s own `services` field wants `Readonly<Record<string, string>>`,
 * but a mutable source is always assignable to a readonly target, so the plain form here satisfies
 * both callers.
 */
export type ConsoleCoreConfig = {
  readonly PAIGASUS_IAM_GRPC_URL: string;
  readonly PAIGASUS_SESSION_STORE: 'redis' | 'memory';
  readonly PAIGASUS_SESSION_REDIS_URL?: string | undefined;
  readonly PAIGASUS_SESSION_REDIS_TIMEOUT_MS: number;
  readonly PAIGASUS_SERVICES: Record<string, string>;
  readonly PAIGASUS_DISCOVERY_NEGATIVE_MS: number;
  readonly PAIGASUS_DISCOVERY_FRESH_MS: number;
  readonly PAIGASUS_DISCOVERY_STALE_MS: number;
  readonly PAIGASUS_DISCOVERY_PROBE_TIMEOUT_MS: number;
  readonly PAIGASUS_DISCOVERY_LOCK_WAIT_MS: number;
  readonly PAIGASUS_DISCOVERY_LOCK_TTL_MS: number;
};
