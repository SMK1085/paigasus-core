// SPDX-License-Identifier: Apache-2.0
//
// Deployment configuration, read and validated at FIRST REQUEST — never at module scope.
//
// TWO GUARDS, COVERING TWO DIFFERENT DOORS. This module is Node-runtime only, because the edge
// runtime has no dynamic `process.env` — values are inlined at build time there, so an edge
// consumer would read the BUILDER's values rather than the deployment's. That is the same
// silent-wrong-value class the compiled-versus-deployed cross-check below exists to catch.
//
// 1. `server-only` is a CLIENT-BUNDLE guard, and only that. It throws when the `react-server`
//    export condition is ABSENT, i.e. in a client component. It does NOT stop a middleware or
//    edge route: Next sets `react-server` for the middleware layer too
//    (`next/dist/build/webpack-config.js` — `reactServerConditionNames` at :557, applied to
//    `issuerLayer: WEBPACK_LAYERS.middleware` at :1404-1408), so the import resolves to
//    server-only's own `empty.js` there and is a no-op. MEASURED on Next 16.3.4: a middleware.ts
//    importing this module builds at exit 0 and this file lands in `.next/server/edge/chunks/`.
//    See docs/superpowers/specs/2026-09-08-sma-502-measurements.md M6.
// 2. The NEXT_RUNTIME check in getRuntimeConfig() is what covers the edge door. Next defines
//    `process.env.NEXT_RUNTIME` as the literal `'edge'` for the edge compilation
//    (`next/dist/build/define-env.js:80`), so the branch is statically true in an edge bundle.
import 'server-only';
import { z, type ZodRawShape } from 'zod';
import { canonicalBasePath } from './base-path';

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
 * which NARROWS the one hole in "zod strips unknown keys": zod constrains an OBJECT's key set,
 * never a RECORD's, so without this any secret pasted into the operator's JSON would ride through
 * the public projection inside an allowed key (spec § 5.4).
 *
 * It does not CLOSE that hole. A path-shaped secret — `{"x": "/sk-live-abc123"}` — is a valid
 * canonical base path and still reaches the browser. What is removed is every non-path value
 * (a URL, a token with a `.` or a `/`-free string); what remains is the narrower "secret that
 * looks like a path" class, whose real control is that Helm GENERATES this map from the same
 * values block as the ingress rules rather than anyone hand-writing it.
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
  // `Object.create(null)`, NOT `{}`. Two distinct defects came from the object literal, and both
  // are silent.
  //
  // WRITING. `JSON.parse` makes `__proto__` an OWN property, so `Object.entries` yields it and the
  // loop below reaches `out['__proto__'] = …`. On an object literal that assignment hits the
  // INHERITED setter on Object.prototype, which ignores a string value — the entry is dropped with
  // no error, and getPublicConfig() then hands the browser a map that is quietly missing a zone.
  //
  // READING. `assertCompiledAgreement` decides a zone is undeclared with `zones[zone] === undefined`.
  // On an object literal `zones['toString']` returns an inherited FUNCTION, so a missing zone id
  // that happens to name an Object.prototype member reads as present and skips the base-path
  // cross-check entirely. A null-prototype object has nothing to inherit, so both go away.
  const out: Record<string, string> = Object.create(null) as Record<string, string>;
  for (const [id, value] of Object.entries(parsed as Record<string, unknown>)) {
    // The same rule createNextConfig applies to `zone`, applied to this map's KEYS. A padded key
    // would otherwise fail the lookup in assertCompiledAgreement and report "no entry for zone
    // \"iam\"" while the operator is looking at a map that visibly contains iam.
    if (id !== id.trim() || id === '') {
      ctx.addIssue({ code: 'custom', message: `entry ${JSON.stringify(id)} must be a zone id with no leading or trailing whitespace` });
      return z.NEVER;
    }
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

/**
 * The variables this package owns. Neither has a default — a default hides a misconfiguration.
 *
 * `PAIGASUS_ZONE` refuses surrounding whitespace, matching `createNextConfig`'s rule for the same
 * id at build time. Without it, `PAIGASUS_ZONE=' iam '` reaches `assertCompiledAgreement` and is
 * reported as a zone MISMATCH — an error whose two sides render identically in a container log,
 * and which points the operator at the image rather than at the variable. Refusing it here names
 * the actual fault. The message is authored in this file and value-free, so `describeIssues`
 * renders it (it is a `custom` issue on an owned key).
 */
export const coreEnvShape = {
  PAIGASUS_ZONE: z
    .string()
    .min(1)
    .refine((value) => value === value.trim(), { error: 'must have no leading or trailing whitespace' }),
  PAIGASUS_ZONES: zoneMapFromJson,
};

/**
 * Format a validation failure without ever rendering an input value.
 *
 * A `message` is rendered ONLY for an issue whose path root is a key this package owns, because
 * those messages are authored in this file and are provably value-free. Everything else — a
 * built-in issue, and every issue raised by an `extraShape` — is rendered as its `code` alone.
 *
 * The extra-shape half is the load-bearing part. `extraShape` exists so `@paigasus/auth` and
 * `@paigasus/sdk` can supply their own zod shapes, and a `.refine()` there may interpolate the
 * input into its message. That produces `code: 'custom'` and would otherwise put a secret
 * straight into the thrown error and the container log. An owner check is the only filter that
 * holds once this file stops authoring every custom issue.
 *
 * MEASURED on the pinned zod 4.5.4, because the two `.refine()` message forms do NOT behave the
 * same and only one leaks. zod 3's second-argument FUNCTION form,
 * `.refine(fn, (v) => ({ message: `${v} …` }))`, is ignored by zod 4 — the issue message comes
 * back as the generic `Invalid input`. The zod 4 form, `.refine(fn, { error: (iss) => `${iss.input} …` })`,
 * renders the input verbatim. The test pins the second form; pinning the first would assert
 * nothing, since it cannot leak on this zod version.
 */
function describeIssues(error: z.ZodError): string {
  const owned: readonly string[] = OWNED_KEYS;
  const parts = error.issues.map((issue) => {
    const root = issue.path.length > 0 ? issue.path[0] : undefined;
    const where = issue.path.length > 0 ? issue.path.join('.') : '(root)';
    const trusted = issue.code === 'custom' && typeof root === 'string' && owned.includes(root);
    return trusted ? `${where}: ${issue.message}` : `${where}: ${issue.code}`;
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
    // The EDGE door. `server-only` does not close it — Next sets the `react-server` condition for
    // the middleware layer too, so that import is a no-op there (MEASURED; see this file's header
    // and § M6 of the measurements document). Next defines `process.env.NEXT_RUNTIME` as the
    // literal 'edge' for the edge compilation, so this branch is statically true in an edge
    // bundle and the failure is loud rather than silent.
    if (process.env.NEXT_RUNTIME === 'edge') {
      throw new Error(
        'getRuntimeConfig() was called in the EDGE runtime. The edge runtime has no dynamic `process.env` — values are inlined at build time there, so config read here would be the ' +
          "builder's values or undefined, never the deployment's. Read configuration in a Node-runtime server component or route handler instead; ADR-0017 keeps middleware to cookie-presence checks for exactly this reason.",
      );
    }
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
