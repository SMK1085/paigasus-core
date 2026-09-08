// SPDX-License-Identifier: Apache-2.0
//
// Deployment configuration, read and validated at FIRST REQUEST — never at module scope.
//
// `server-only` is the structural guard. In the edge runtime Next does not provide a dynamic
// `process.env`: values must be known at build time, so an edge route or middleware importing
// this module would read inlined or undefined values rather than deployment values. That is the
// same silent-wrong-value class the compiled-versus-deployed cross-check below exists to catch,
// arriving through a different door (spec § 5). This module is Node-runtime only.
import 'server-only';
import { z, type ZodRawShape } from 'zod';
import { canonicalBasePath } from './base-path.js';

/** Keys this package owns. An extra shape declaring one of them is a hard error. */
const OWNED_KEYS = ['PAIGASUS_ZONE', 'PAIGASUS_ZONES'] as const;

/** The client-safe projection's key set, pinned by a strict-equality test. */
export const PUBLIC_CONFIG_KEYS = ['zone', 'zones'] as const;

export interface PublicConfig {
  zone: string;
  zones: Record<string, string>;
}

/**
 * `PAIGASUS_ZONES` arrives as a JSON string. Each value is validated as a canonical base path,
 * which is what closes the one hole in "zod strips unknown keys": zod constrains an OBJECT's key
 * set, never a RECORD's, so without this a secret pasted into the operator's JSON would ride
 * through the public projection inside an allowed key (spec § 5.4).
 *
 * Issue messages name the zone id but never the value. A zone id is a public routing label; a
 * value might be a mis-pasted secret.
 */
const zoneMapFromJson = z.string().transform((raw, ctx): Record<string, string> => {
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    ctx.addIssue({ code: 'custom', message: 'is not valid JSON' });
    return z.NEVER;
  }
  if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) {
    ctx.addIssue({ code: 'custom', message: 'must be a JSON object mapping zone id to base path' });
    return z.NEVER;
  }
  const out: Record<string, string> = {};
  for (const [id, value] of Object.entries(parsed as Record<string, unknown>)) {
    if (typeof value !== 'string') {
      ctx.addIssue({ code: 'custom', message: `entry ${JSON.stringify(id)} must be a string` });
      return z.NEVER;
    }
    try {
      out[id] = canonicalBasePath(value, `entry ${JSON.stringify(id)}`);
    } catch {
      ctx.addIssue({ code: 'custom', message: `entry ${JSON.stringify(id)} is not a valid base path` });
      return z.NEVER;
    }
  }
  return out;
});

/** The variables this package owns. Neither has a default — a default hides a misconfiguration. */
export const coreEnvShape = {
  PAIGASUS_ZONE: z.string().min(1),
  PAIGASUS_ZONES: zoneMapFromJson,
};

/**
 * Format a validation failure without ever rendering an input value.
 *
 * Custom issues carry messages authored in this file, which are provably value-free. Built-in
 * issues are rendered as their `code` alone rather than their message, because a future zod
 * version could start echoing the input into a built-in message and nothing would notice.
 */
function describeIssues(error: z.ZodError): string {
  const parts = error.issues.map((issue) => {
    const where = issue.path.length > 0 ? issue.path.join('.') : '(root)';
    return issue.code === 'custom' ? `${where}: ${issue.message}` : `${where}: ${issue.code}`;
  });
  return [...new Set(parts)].join('; ');
}

/**
 * Compare what the IMAGE was built as against what the DEPLOYMENT declares.
 *
 * Fail-closed by construction: an absent compiled value throws rather than skipping the check. A
 * check that silently does nothing is worse than no check, because the design document then
 * records that the condition is detected.
 *
 * Zone ids and base paths ARE the public slice, so naming them in an error leaks nothing.
 */
function assertCompiledAgreement(zone: string, zones: Record<string, string>): void {
  const compiledZone = process.env.PAIGASUS_COMPILED_ZONE;
  const compiledBasePath = process.env.PAIGASUS_COMPILED_BASE_PATH;

  if (compiledZone === undefined || compiledZone === '') {
    throw new Error('PAIGASUS_COMPILED_ZONE is absent. createNextConfig() writes it, so this image was not built with the factory. ' + 'Failing closed rather than skipping the zone cross-check.');
  }
  if (compiledBasePath === undefined) {
    throw new Error(
      'PAIGASUS_COMPILED_BASE_PATH is absent. createNextConfig() writes it, so this image was not built with the factory. ' + 'Failing closed rather than skipping the base-path cross-check.',
    );
  }
  if (zone !== compiledZone) {
    throw new Error(`Zone mismatch: this image was built as zone "${compiledZone}" but PAIGASUS_ZONE is "${zone}". Deploy the right image, or fix PAIGASUS_ZONE.`);
  }

  const declared = zones[zone];
  if (declared === undefined) {
    throw new Error(`PAIGASUS_ZONES has no entry for this app's own zone "${zone}". Every deployed zone must appear in the map.`);
  }

  const compiledCanonical = canonicalBasePath(compiledBasePath, 'PAIGASUS_COMPILED_BASE_PATH');
  if (declared !== compiledCanonical) {
    throw new Error(
      `Base path mismatch for zone "${zone}": the image serves "${compiledCanonical}" but PAIGASUS_ZONES declares "${declared}". ` +
        'Next has no runtime basePath, so rebuild the image at the new prefix or fix the ingress.',
    );
  }
}

/**
 * Build this app's single runtime-configuration accessor.
 *
 * Call it ONCE per app. `@paigasus/auth` and `@paigasus/sdk` each export a zod shape for the
 * variables they own; the app composes them here. One schema, one parse, one place — a registry
 * would let two packages parse the same environment twice and disagree.
 */
export function defineRuntimeConfig<T extends ZodRawShape = Record<never, never>>(extraShape?: T) {
  const extra = (extraShape ?? {}) as T;
  for (const key of OWNED_KEYS) {
    if (Object.prototype.hasOwnProperty.call(extra, key)) {
      throw new Error(`defineRuntimeConfig: extraShape must not declare ${key} — @paigasus/next-config owns it.`);
    }
  }

  const schema = z.object({ ...coreEnvShape, ...extra });
  type Parsed = z.infer<typeof schema>;
  let cached: Parsed | undefined;

  function getRuntimeConfig(): Parsed {
    // A module-scope read from a prerendered page would bake the BUILDER's values into the image
    // and hand them to every self-hoster. Failing the build with an actionable message is the
    // only outcome that cannot ship silently.
    if (process.env.NEXT_PHASE === 'phase-production-build') {
      throw new Error(
        'getRuntimeConfig() was called during `next build`. Deployment configuration must be read at request time, not module scope — ' +
          "a build-time read bakes the builder's values into the image. Move the call inside a request-scoped function, or `await connection()` from `next/server` first.",
      );
    }
    // Success is memoized; failure is not. A misconfigured container must fail EVERY request,
    // not just the first.
    if (cached !== undefined) {
      return cached;
    }
    const result = schema.safeParse(process.env);
    if (!result.success) {
      throw new Error(`Invalid runtime configuration — ${describeIssues(result.error)}`);
    }
    const parsed = result.data as Parsed & { PAIGASUS_ZONE: string; PAIGASUS_ZONES: Record<string, string> };
    assertCompiledAgreement(parsed.PAIGASUS_ZONE, parsed.PAIGASUS_ZONES);
    cached = parsed;
    return cached;
  }

  /**
   * The client-safe slice. An ALLOWLIST BY CONSTRUCTION: the projection names two fields, so a
   * secret added to an extra shape cannot reach the browser unless someone edits this function
   * and `PUBLIC_CONFIG_KEYS` together — and a strict-equality test on that constant makes the
   * edit deliberate.
   *
   * Internal service URLs are cluster-internal DNS and must never reach the browser.
   *
   * TRANSPORT: pass the result as a PROP from a server component. This package ships no inline
   * `<script>` serialization; that path is a `</script>`-injection hazard when the JSON is not
   * escaped, and whichever issue first needs it owns the escaping rule (spec § 5.4).
   */
  function getPublicConfig(): PublicConfig {
    const full = getRuntimeConfig() as Parsed & { PAIGASUS_ZONE: string; PAIGASUS_ZONES: Record<string, string> };
    return { zone: full.PAIGASUS_ZONE, zones: full.PAIGASUS_ZONES };
  }

  return { getRuntimeConfig, getPublicConfig };
}
