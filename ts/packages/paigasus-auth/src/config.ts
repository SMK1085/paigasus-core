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

export const authEnvShape = {
  PAIGASUS_OIDC_ISSUER: httpsUrl,
  PAIGASUS_OIDC_CLIENT_ID: z.string().min(1),
  PAIGASUS_OIDC_CLIENT_SECRET: z.string().min(1),
  PAIGASUS_PUBLIC_ORIGIN: httpsUrl,
  PAIGASUS_OIDC_REDIRECT_URI: z.string().optional(),
  PAIGASUS_OIDC_POST_LOGOUT_REDIRECT_URI: z.string().optional(),
  PAIGASUS_OIDC_SCOPES: z.string().min(1).default('openid profile email offline_access'),
  PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS: seconds(30),
  PAIGASUS_OIDC_HTTP_TIMEOUT_MS: millis(3500),
  PAIGASUS_SESSION_STORE: z.enum(['redis', 'memory']),
  PAIGASUS_SESSION_REDIS_URL: z.string().optional(),
  PAIGASUS_SESSION_REDIS_TIMEOUT_MS: millis(1000),
  PAIGASUS_SESSION_TTL_SECONDS: seconds(28800),
  PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS: seconds(86400),
  PAIGASUS_SESSION_REFRESH_SKEW_SECONDS: seconds(30),
  PAIGASUS_SESSION_LOCK_TTL_MS: millis(5000),
  PAIGASUS_SESSION_LOCK_WAIT_MS: millis(3000),
} as const;

export type AuthEnv = z.infer<z.ZodObject<typeof authEnvShape>>;
