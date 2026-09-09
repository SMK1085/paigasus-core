// SPDX-License-Identifier: Apache-2.0

/** Matches one or more `/`-separated path segments of URL-safe unreserved characters. */
const PATH_PREFIX = /^(?:\/[A-Za-z0-9._~-]+)+$/;

/**
 * One canonical form for a zone prefix: a leading `/`, no trailing `/`, and `''` for a zone
 * mounted at the origin root — Next rejects `basePath: '/'`.
 *
 * Both sides of the compiled-versus-deployed cross-check run through this, so `/iam`, `/iam/`
 * and `iam` compare equal. Without it, a trailing slash in an operator's `PAIGASUS_ZONES` JSON
 * hard-fails a correctly configured deployment (spec § 4.2).
 *
 * The rejected value is NEVER included in the error. This validator also reads operator-supplied
 * JSON, so echoing it could put a mis-pasted secret into a log. `label` carries the location,
 * which is what a reader actually needs.
 */
export function canonicalBasePath(value: string, label: string): string {
  if (typeof value !== 'string') {
    throw new Error(`${label}: expected a string base path`);
  }
  const trimmed = value.trim();
  if (trimmed === '' || trimmed === '/') {
    return '';
  }
  const withLeading = trimmed.startsWith('/') ? trimmed : `/${trimmed}`;
  const withoutTrailing = withLeading.replace(/\/+$/, '');
  if (!PATH_PREFIX.test(withoutTrailing)) {
    throw new Error(`${label}: not a valid base path prefix (value withheld — it may be operator input)`);
  }
  return withoutTrailing;
}
