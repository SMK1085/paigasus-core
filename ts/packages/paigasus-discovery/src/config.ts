// SPDX-License-Identifier: Apache-2.0
import { z } from 'zod';
import { SERVICE_SLUGS } from './core/state.js';
import { DEFAULT_TIMINGS } from './core/record.js';

// PAIGASUS_ZONES' transform is NOT reusable here: `zoneMapFromJson` validates each value with
// `canonicalBasePath`, which is a PATH validator, not a URL validator.

const SLUG_RE = /^[a-z][a-z0-9]*$/;

/**
 * Parse and validate the service address map.
 *
 * Keys are checked against the closed set derived from the capability registry. An unknown key
 * FAILS CONSTRUCTION rather than being ignored: ignoring it produces a silently empty console,
 * which the design calls the worse failure because a wrongly-hidden item is invisible.
 */
export function parseServiceMap(raw: string): Record<string, string> {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    throw new Error('PAIGASUS_SERVICES must be a JSON object mapping service name to base URL');
  }
  if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) {
    throw new Error('PAIGASUS_SERVICES must be a JSON object mapping service name to base URL');
  }

  const out: Record<string, string> = Object.create(null) as Record<string, string>;
  for (const [key, value] of Object.entries(parsed)) {
    if (!SLUG_RE.test(key)) {
      throw new Error(`PAIGASUS_SERVICES: "${key}" is not a valid service name (expected ^[a-z][a-z0-9]*$)`);
    }
    if (!SERVICE_SLUGS.includes(key)) {
      throw new Error(
        `PAIGASUS_SERVICES: "${key}" is not a known service (expected one of ${SERVICE_SLUGS.join(', ')})`,
      );
    }
    if (typeof value !== 'string') {
      throw new Error(`PAIGASUS_SERVICES: the address for "${key}" must be a string`);
    }
    out[key] = canonicalBaseUrl(key, value);
  }
  return out;
}

function canonicalBaseUrl(key: string, value: string): string {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    throw new Error(`PAIGASUS_SERVICES: the address for "${key}" is not an absolute URL`);
  }
  if (url.protocol !== 'http:' && url.protocol !== 'https:') {
    throw new Error(`PAIGASUS_SERVICES: the address for "${key}" must use http: or https:`);
  }
  if (url.username !== '' || url.password !== '') {
    throw new Error(`PAIGASUS_SERVICES: the address for "${key}" must not carry credentials`);
  }
  if (url.search !== '' || url.hash !== '') {
    throw new Error(`PAIGASUS_SERVICES: the address for "${key}" must not carry a query or fragment`);
  }
  const path = url.pathname.replace(/\/+$/, '');
  return `${url.origin}${path}`;
}

const positiveInt = (fallback: number) =>
  z
    .string()
    .optional()
    .transform((v) => (v === undefined || v === '' ? fallback : Number(v)))
    .pipe(z.number().int().positive());

/**
 * The variables this package owns, composed into an app's `defineRuntimeConfig()` call.
 *
 * NOTE: `describeIssues` in @paigasus/next-config renders `issue.message` only for the keys it
 * owns itself, so a failure here surfaces as `PAIGASUS_SERVICES: custom`. The README therefore
 * carries the expected form of every variable.
 */
export const discoveryEnvShape = {
  PAIGASUS_SERVICES: z.string().transform((raw, ctx) => {
    try {
      return parseServiceMap(raw);
    } catch (err) {
      ctx.addIssue({ code: 'custom', message: err instanceof Error ? err.message : 'invalid' });
      return z.NEVER;
    }
  }),
  PAIGASUS_DISCOVERY_NEGATIVE_MS: positiveInt(DEFAULT_TIMINGS.negativeMs),
  PAIGASUS_DISCOVERY_FRESH_MS: positiveInt(DEFAULT_TIMINGS.freshMs),
  PAIGASUS_DISCOVERY_STALE_MS: positiveInt(DEFAULT_TIMINGS.staleMs),
  PAIGASUS_DISCOVERY_PROBE_TIMEOUT_MS: positiveInt(DEFAULT_TIMINGS.probeTimeoutMs),
  PAIGASUS_DISCOVERY_LOCK_WAIT_MS: positiveInt(DEFAULT_TIMINGS.lockWaitMs),
  PAIGASUS_DISCOVERY_LOCK_TTL_MS: positiveInt(DEFAULT_TIMINGS.lockTtlMs),
};
