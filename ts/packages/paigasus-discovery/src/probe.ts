// SPDX-License-Identifier: Apache-2.0
import { reasonForStatus, reasonForThrown } from './core/reasons.js';
import type { DegradedReason, ServiceDescriptor } from './types.js';

export type ProbeOutcome =
  | { readonly ok: true; readonly descriptor: ServiceDescriptor }
  | { readonly ok: false; readonly reason: DegradedReason };

export type ProbeOptions = {
  readonly baseUrl: string;
  readonly token: string;
  readonly timeoutMs: number;
  readonly maxBytes: number;
  readonly fetch: typeof globalThis.fetch;
};

/** The path both services serve. IAM also has a gRPC RPC; the gateway has no gRPC server at all. */
export const SERVICE_INFO_PATH = '/v1/service-info';

function isJsonContentType(value: string | null): boolean {
  if (value === null) return false;
  // Case-insensitive by contract: this header is supplied by another system.
  const essence = value.split(';', 1)[0]?.trim().toLowerCase();
  return essence === 'application/json';
}

function toDescriptor(body: unknown): ServiceDescriptor | null {
  if (typeof body !== 'object' || body === null) return null;
  const b = body as Record<string, unknown>;
  if (typeof b['service'] !== 'string' || typeof b['version'] !== 'string') return null;
  const caps = b['capabilities'];
  // Both services always send `capabilities`, as [] when empty. Tolerate its absence anyway:
  // an older build is exactly what this whole mechanism exists to cope with.
  if (caps !== undefined && (!Array.isArray(caps) || !caps.every((c) => typeof c === 'string'))) {
    return null;
  }
  // Unknown fields are dropped HERE. That is decision 6's "unknown key -> ignore", enforced at
  // the boundary rather than by every downstream reader.
  return {
    service: b['service'],
    version: b['version'],
    capabilities: caps === undefined ? [] : caps,
  };
}

async function readCapped(response: Response, maxBytes: number): Promise<string | null> {
  const declared = response.headers.get('content-length');
  if (declared !== null && Number(declared) > maxBytes) return null;
  const body = response.body;
  if (body === null) return '';
  const reader = body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    // Cap enforced WHILE reading, so a server that lies about content-length cannot make us
    // buffer an arbitrary body inside the timeout window on a fast internal link.
    if (total > maxBytes) {
      await reader.cancel();
      return null;
    }
    chunks.push(value);
  }
  const joined = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    joined.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return new TextDecoder().decode(joined);
}

/**
 * One probe of one service. Never throws.
 *
 * A 401/403 is returned to the caller and — critically — is NEVER cached by the resolver: the
 * descriptor is caller-independent but the AUTH OUTCOME is not, so writing it into the shared
 * entry would let one user's expired cookie disable navigation for the whole deployment.
 */
export async function probeService(opts: ProbeOptions): Promise<ProbeOutcome> {
  if (opts.token.trim() === '') return { ok: false, reason: 'unauthorized' };

  let response: Response;
  try {
    response = await opts.fetch(`${opts.baseUrl}${SERVICE_INFO_PATH}`, {
      method: 'GET',
      headers: { authorization: `Bearer ${opts.token}`, accept: 'application/json' },
      // fetch follows redirects by default. Following one would forward the user's bearer token
      // to wherever a compromised or misconfigured service points.
      redirect: 'error',
      signal: AbortSignal.timeout(opts.timeoutMs),
    });
  } catch (err) {
    return { ok: false, reason: reasonForThrown(err) };
  }

  if (!response.ok) return { ok: false, reason: reasonForStatus(response.status) };
  if (!isJsonContentType(response.headers.get('content-type'))) {
    return { ok: false, reason: 'bad-response' };
  }

  let raw: string | null;
  try {
    raw = await readCapped(response, opts.maxBytes);
  } catch {
    return { ok: false, reason: 'network' };
  }
  if (raw === null) return { ok: false, reason: 'bad-response' };

  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return { ok: false, reason: 'bad-response' };
  }

  const descriptor = toDescriptor(parsed);
  return descriptor === null ? { ok: false, reason: 'bad-response' } : { ok: true, descriptor };
}
