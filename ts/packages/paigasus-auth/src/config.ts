// SPDX-License-Identifier: Apache-2.0
//
// The environment variables @paigasus/auth owns, as a zod raw shape an app composes into
// @paigasus/next-config's defineRuntimeConfig(extraShape).
//
// THIS SHAPE IS FLAT AND PER-KEY, DELIBERATELY. defineRuntimeConfig builds z.object({...core,
// ...extra}) with no refinement hook (next-config/src/runtime.ts:201), and throws if an extra
// shape declares PAIGASUS_ZONE or PAIGASUS_ZONES (:195-199). So two things CANNOT live here and
// live in createAuthRuntime instead (src/runtime.ts):
//   - "REDIS_URL is required when STORE=redis" — a cross-field rule.
//   - the derived redirect URIs — they need the zone keys this shape may not declare.
//
// Note also that next-config's describeIssues renders a message only for `custom` issues on ITS
// OWN keys; every issue raised here surfaces as `<key>: <code>`. That filter is what stops a
// package-authored message leaking a secret, so it is accepted rather than widened. The names
// below are therefore self-describing, and README.md carries the expected form of each.
import { z } from 'zod';

const seconds = (fallback: number) => z.coerce.number().int().positive().default(fallback);
const millis = (fallback: number) => z.coerce.number().int().positive().default(fallback);

/** An absolute https URL with no trailing slash and no surrounding whitespace. */
const httpsUrl = z
  .string()
  .refine((v) => v === v.trim() && v.length > 0, { error: 'must not have leading or trailing whitespace' })
  .refine((v) => URL.canParse(v) && new URL(v).protocol === 'https:', { error: 'must be an absolute https URL' })
  .refine((v) => !v.endsWith('/'), { error: 'must not end with a slash' });

/**
 * An overridden redirect URI. A well-formed absolute URL, but NOT scheme-restricted to https like
 * `httpsUrl` above — an operator overriding this is typically pointing at a reverse proxy or a
 * non-default hostname, and the scheme is that deployment's call, not this package's. Without
 * this check the override was a bare `z.string()`, so a malformed value (a stray space, a path
 * with no scheme) was accepted in silence and only surfaced later as an opaque failure inside
 * `openid-client`.
 */
const overrideUrl = z.url();

/**
 * The one separator class both sides agree on (SMA-692 final fix I1). RFC 6749 § 3.3 allows only
 * a single space (%x20) between scope tokens, but JS `\s` and RE2 `\s` (the chart's `regexSplit`)
 * do not agree on their own: JS `\s` also matches NBSP and `\v`, RE2 `\s` does not. This explicit
 * class removes that disagreement. `charts/paigasus/templates/_audience.tpl`'s
 * `paigasus.consoleScopes` uses the same class, as a Go regex literal.
 */
const OIDC_SCOPES_SEPARATOR = /[\t\n\f\r ]+/;

/**
 * Splits on OIDC_SCOPES_SEPARATOR, drops empty tokens (a leading, trailing or repeated
 * separator), and joins the rest with one space. `openid\u00a0profile` (an NBSP, not in the
 * class) stays ONE token, so it does not equal `openid` — an operator whose value carries an NBSP
 * still gets refused, but by the openid check below, not silently accepted.
 */
const normalizeOidcScopes = (value: string): string =>
  value
    .split(OIDC_SCOPES_SEPARATOR)
    .filter((token) => token.length > 0)
    .join(' ');

/**
 * True when the normalized, space-separated scope list holds the token `openid` exactly (SMA-692
 * D5). Without it the first login fails at once (`idTokenExpected: true` in adapters/oidc.ts),
 * with an unclear error. `openidx` does not count. The chart refuses the same value at render
 * time, with a readable reason (charts/paigasus/templates/_audience.tpl).
 */
const hasOpenidScope = (value: string): boolean => value.split(' ').includes('openid');

export const authEnvShape = {
  PAIGASUS_OIDC_ISSUER: httpsUrl,
  PAIGASUS_OIDC_CLIENT_ID: z.string().min(1),
  PAIGASUS_OIDC_CLIENT_SECRET: z.string().min(1),
  PAIGASUS_PUBLIC_ORIGIN: httpsUrl,
  PAIGASUS_OIDC_REDIRECT_URI: overrideUrl.optional(),
  PAIGASUS_OIDC_POST_LOGOUT_REDIRECT_URI: overrideUrl.optional(),
  // SMA-692 D3-a: NO default here. createAuthRuntime applies the default list to the
  // authorization request only. The refresh request sends `scope` only when this key is set, so
  // the parsed config must keep "absent" distinct from "the default".
  //
  // Final fix I1: the transform runs first and normalizes whitespace. Both refines then read the
  // NORMALIZED value, so a whitespace-only input refuses as empty, and the parsed config (so both
  // the authorization `scope` and the refresh `scope`) carries the normalized string, never the
  // raw one. `.optional()` still wraps the whole chain, so an absent key skips it and stays
  // `undefined` — the inferred type is unchanged: `string | undefined`.
  PAIGASUS_OIDC_SCOPES: z
    .string()
    .transform(normalizeOidcScopes)
    .refine((v) => v.length > 0, { error: 'must not be empty after whitespace normalization' })
    .refine(hasOpenidScope, { error: 'must contain the scope openid' })
    .optional(),
  // SMA-692 D1. The `audience` authorization parameter (Auth0). Not the IAM audience.
  PAIGASUS_OIDC_AUTHORIZATION_AUDIENCE: z
    .string()
    .refine((v) => v.length > 0 && v === v.trim(), { error: 'must be non-empty, with no leading or trailing whitespace' })
    .optional(),
  PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS: seconds(30),
  PAIGASUS_OIDC_HTTP_TIMEOUT_MS: millis(3500),
  PAIGASUS_SESSION_STORE: z.enum(['redis', 'memory']),
  PAIGASUS_SESSION_REDIS_URL: z.string().optional(),
  // SMA-651 D9: at most 536870911 ms. Node turns a timer delay above 2^31 - 1 ms into 1 ms, and the
  // session store's largest timer is 4 x this value (its per-operation deadline).
  PAIGASUS_SESSION_REDIS_TIMEOUT_MS: z.coerce.number().int().positive().max(536_870_911).default(1000),
  PAIGASUS_SESSION_TTL_SECONDS: seconds(28800),
  PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS: seconds(86400),
  PAIGASUS_SESSION_REFRESH_SKEW_SECONDS: seconds(30),
  // Default raised 5000 -> 10000 (SMA-506 task-7 review, Important 2): a refresh now makes TWO
  // sequential bounded calls under the lock — the token endpoint, then (with non-repudiation
  // checks enabled, see adapters/oidc.ts) a JWKS fetch — so createAuthRuntime asserts
  // `2 * PAIGASUS_OIDC_HTTP_TIMEOUT_MS < PAIGASUS_SESSION_LOCK_TTL_MS`. The shipped defaults must
  // themselves satisfy that: 3500 * 2 = 7000 < 10000.
  PAIGASUS_SESSION_LOCK_TTL_MS: millis(10000),
  PAIGASUS_SESSION_LOCK_WAIT_MS: millis(3000),
} as const;

export type AuthEnv = z.infer<z.ZodObject<typeof authEnvShape>>;
