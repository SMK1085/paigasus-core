// SPDX-License-Identifier: Apache-2.0
//
// Cookie names are constants (design doc § 6.7) — all zones share one cookie on one origin, so a
// configuration knob here would invite exactly one drift.
//
// EVERY COOKIE THIS PACKAGE WRITES CARRIES THE __Host- PREFIX (design doc § 9.3). Host-only (no
// `Domain` attribute) constrains what THIS server writes; it does nothing about a sibling host
// under the shared registrable domain writing its own `pgs_sid=...; Domain=example.com` — the
// browser then sends two values in one `Cookie` header and most parsers take the first, which is
// session fixation by another route. `__Host-` forces `Secure`, `Path=/` and no `Domain`, and
// browsers refuse a `Domain`-scoped write of a `__Host-`-prefixed name outright. That is why
// `serializeCookie` below never accepts a `Domain` option — there is nothing to configure.
//
// `__Host-pgs_sid` is a browser-session cookie: no `Max-Age`, no `Expires`. A refresh slides the
// STORE record's TTL, but a Next server component cannot set a cookie, so nothing could slide the
// cookie to match — two independently-expiring lifetimes would drift apart and log an active user
// out mid-session. Lifetime is bounded server-side instead, by the record's idle TTL and its
// `absoluteExpiresAt`.

/** The session cookie. A browser-session cookie — see the file header for why it carries no TTL. */
export const SESSION_COOKIE = '__Host-pgs_sid';

/** Every per-transaction cookie shares this prefix, so routes.ts can enumerate them by name. */
export const TXN_COOKIE_PREFIX = '__Host-pgs_txn_';

/**
 * Per-transaction cookie name (design doc § 9.2). NOT a single fixed name: two tabs starting a
 * login concurrently would otherwise overwrite each other's secret, and the first tab's callback
 * would then fail its own security check.
 */
export function txnCookieName(txnId: string): string {
  return `${TXN_COOKIE_PREFIX}${txnId}`;
}

export interface SerializeCookieOptions {
  /** Omit for a browser-session cookie (the session cookie — see the file header). */
  maxAgeSeconds?: number;
}

/**
 * Builds one `Set-Cookie` header value. Always `HttpOnly`, `Secure`, `SameSite=Lax`, `Path=/`, and
 * never `Domain` — see the file header. `SameSite=Lax`, never `Strict`: both cookies this package
 * writes must survive the cross-site top-level redirect back from the identity provider, which
 * `Strict` would withhold.
 */
export function serializeCookie(name: string, value: string, options: SerializeCookieOptions = {}): string {
  const attributes = [`${name}=${value}`, 'HttpOnly', 'Secure', 'SameSite=Lax', 'Path=/'];
  if (options.maxAgeSeconds !== undefined) {
    attributes.push(`Max-Age=${String(options.maxAgeSeconds)}`);
  }
  return attributes.join('; ');
}

/** Emits an immediately-expiring cookie of the given name, to remove it from the browser. */
export function clearCookie(name: string): string {
  return serializeCookie(name, '', { maxAgeSeconds: 0 });
}

/**
 * Parses a `Cookie` request header into a name -> value map, in the order the header lists them.
 * That order matters to routes.ts's txn-cookie eviction: user agents list same-path cookies
 * earliest-created first, so the first `__Host-pgs_txn_*` entry here is the oldest outstanding one.
 *
 * A DUPLICATE NAME IS TREATED AS ABSENT (review round 1, M6) — both occurrences are dropped,
 * rather than keeping either one. This is exactly the cookie-tossing hazard the file header
 * describes: a sibling host under the shared registrable domain can write a second, competing
 * value under the same name, and the two arrive in one `Cookie` header with no reliable way for
 * this server to tell which one is legitimate. Guessing (by keeping the first OR the last) would
 * silently pick a value that may not be this server's own; refusing to use either is the safe
 * choice, and it degrades a legitimate cookie the same way an attacker's toss does — a missing
 * cookie the caller already has to handle (`txn_missing`, or a `__Host-pgs_sid` treated as absent).
 */
export function readCookies(cookieHeader: string | null | undefined): Map<string, string> {
  const cookies = new Map<string, string>();
  if (cookieHeader === null || cookieHeader === undefined || cookieHeader.length === 0) {
    return cookies;
  }
  const duplicates = new Set<string>();
  for (const pair of cookieHeader.split(';')) {
    const separator = pair.indexOf('=');
    if (separator === -1) continue;
    const name = pair.slice(0, separator).trim();
    const value = pair.slice(separator + 1).trim();
    if (name.length === 0 || duplicates.has(name)) continue;
    if (cookies.has(name)) {
      cookies.delete(name);
      duplicates.add(name);
      continue;
    }
    cookies.set(name, value);
  }
  return cookies;
}
