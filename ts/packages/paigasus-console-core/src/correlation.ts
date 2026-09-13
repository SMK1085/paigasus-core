// SPDX-License-Identifier: Apache-2.0
//
// The per-request correlation id (spec § 6.2). proxy.ts mints a UUID into the REQUEST headers of
// every request it lets through; lib/iam.ts sends it to IAM as `paigasus-correlation-id`; IAM adopts
// it (rs/crates/libs/paigasus-observability/src/correlation.rs:103-119, applied to the gRPC server
// at rs/crates/services/paigasus-iam/src/adapters/grpc/mod.rs:139) and puts it into every error's
// ErrorInfo metadata. So the id a user reads on the 403 view is the id in IAM's logs.
//
// `forbidden()` takes no argument and a React cache() holder does not reach forbidden.tsx
// (measured, spec § 13 #2). A request header does: forbidden.tsx reads it with headers().
import 'server-only';
import { headers } from 'next/headers';
import { unstable_rethrow } from 'next/navigation';
import { CORRELATION_HEADER, REQUEST_PATH_HEADER } from './correlation-header';

export { CORRELATION_HEADER, REQUEST_PATH_HEADER };

/**
 * How the 403 view gets its correlation id (spec § 6.2). DECIDED BY MEASUREMENT, not by taste: the
 * SMA-511 plan's Task 12 builds the app and requests a page that calls forbidden(), and reads the
 * 403 body.
 *
 *   'header'   — the view read the id proxy.ts set, with headers(), and shows it.
 *   'fallback' — it could not; the view shows no id, and callIam's `iam.call_failed` log line
 *                (lib/errors.ts) carries IAM's correlation id and the request path instead.
 *
 * The e2e tier (Task 22) reads this constant to choose its assertion.
 */
export const FORBIDDEN_VIEW_CORRELATION: 'header' | 'fallback' = 'header';

const UUID_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

/**
 * One request header, or null. Null outside a request scope too: headers() throws there, and a
 * caller such as callIam must still work in a test or a background task. unstable_rethrow first,
 * so Next's own control-flow errors (dynamic-usage bailouts during a build) are never swallowed.
 */
async function requestHeader(name: string): Promise<string | null> {
  try {
    const value = (await headers()).get(name);
    return value === null || value === '' ? null : value;
  } catch (err) {
    unstable_rethrow(err);
    return null;
  }
}

/** The id proxy.ts set for this request, or null. A value that is not a UUID is treated as absent. */
export async function requestCorrelationId(): Promise<string | null> {
  const value = await requestHeader(CORRELATION_HEADER);
  return value !== null && UUID_RE.test(value) ? value : null;
}

/** The public path of this request (for example `/iam/orgs/<uuid>`), or null. */
export async function requestPath(): Promise<string | null> {
  const value = await requestHeader(REQUEST_PATH_HEADER);
  return value !== null && value.startsWith('/') ? value : null;
}
