// SPDX-License-Identifier: Apache-2.0
import { CapabilitySchema, capabilityWireKey } from '@paigasus/proto';

/**
 * Every advertised capability key, derived from the generated registry rather than tabulated.
 *
 * A second copy of the registry would drift against the proto. `capabilityWireKey` reads the
 * descriptor's own value names, so this list moves the day the proto does.
 */
export const CAPABILITY_KEYS: readonly string[] = Object.keys(CapabilitySchema.value)
  .map((n) => capabilityWireKey(Number(n)))
  .filter((k): k is string => k !== undefined);

/**
 * The closed set of service slugs, derived as the first dot-segment of every capability key.
 *
 * The proto guarantees this shape: `ServiceInfo.service` is "a bare slug matching the prefix of
 * its own capability keys". This is what PAIGASUS_SERVICES keys are validated against, so an
 * operator typo fails construction instead of silently emptying the console.
 */
export const SERVICE_SLUGS: readonly string[] = [...new Set(CAPABILITY_KEYS.map((k) => k.slice(0, k.indexOf('.'))))];

// NOTE: `SERVICE_STATES` and `CapabilityKey` live in src/types.ts, NOT here. This file imports
// `@paigasus/proto` as a value, and types.ts is the client-safe entry — re-exporting from here
// would pull protobuf-es into a client bundle.

/** Whether a key is in the registry this build knows about. */
export function isKnownCapability(key: string): boolean {
  return CAPABILITY_KEYS.includes(key);
}

// serviceOf lives in the import-free leaf ./service-of.ts (SMA-510): core/outcome.ts needs it and is
// reachable from the client-safe ./client entry, while THIS file imports @paigasus/proto as a value.
// Re-exported here, so every existing importer is unchanged.
export { serviceOf } from './service-of';
