// SPDX-License-Identifier: Apache-2.0
//
// The 503 that an auth route returns when the session store is unavailable (SMA-653, SMA-506
// design § 7.2: "503 with a retry affordance; no partial state").
//
// PRIVATE to this package: routes.ts is the only caller, and server.ts does not export it.
//
// THE PAGE. A fixed HTML document with one retry control: a link for login and callback, and a
// POST form for logout (a link cannot send a POST). It carries NO error text: the error message of
// a SessionStoreUnavailable holds the redacted DSN (adapters/redis-store.ts), and the response is
// not a place for it in any form.
//
// THE HEADERS (spec § 3).
//   - Retry-After: 5. With the shipped 1000 ms store timeout, the SMA-651 circuit cooldown is
//     4000 ms, so a retry after 5 s reaches a closed or re-probing circuit. Advisory only.
//   - Cache-Control: no-store. An error page must never be served from a cache.
//   - Referrer-Policy: no-referrer. A callback 503 is served at /auth/callback?code=…&state=…, and
//     the code may not be spent yet. The retry link must not send that URL as a Referer.
//   - The CSP makes a future escaping defect inert, forbids framing, and `base-uri 'none'` stops an
//     injected <base> from moving the absolute-path links. It carries NO `form-action`. The logout
//     retry form's successful response is a 302 to the IdP's cross-origin end-session endpoint.
//     Chromium applies `form-action` to the redirects after a form submission, and it blocked that
//     redirect (measured on Playwright 1.63.0 Chromium, SMA-653 final review).
//   - NO Set-Cookie. The browser keeps the state it had before the request (spec D4).
//
// ESCAPING. The retry target can hold an attacker-influenced `returnTo` (validated to a same-origin
// path, but still hostile bytes). Two layers: loginRetryHref URL-encodes it as a query value, and
// storeUnavailableResponse HTML-escapes the whole attribute into a DOUBLE-quoted attribute.
import { isSessionStoreUnavailable } from '../core/errors';
import { sidTag, type AuthLogger, type StoreUnavailableStage } from '../ports/logger';

export const RETRY_AFTER_SECONDS = 5;

export const STORE_UNAVAILABLE_CSP = "default-src 'none'; frame-ancestors 'none'; base-uri 'none'";

export type RetryAffordance = { kind: 'link'; href: string } | { kind: 'post'; action: string };

/** HTML-escapes a value for a double-quoted attribute. `&` goes first, so no escape is doubled. */
export function escapeHtmlAttribute(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

/** The login route, with `returnTo` URL-encoded as its query value when one is given. */
export function loginRetryHref(basePath: string, returnTo?: string): string {
  const login = `${basePath}/auth/login`;
  return returnTo === undefined ? login : `${login}?returnTo=${encodeURIComponent(returnTo)}`;
}

export function storeUnavailableResponse(retry: RetryAffordance): Response {
  const signOut = retry.kind === 'post';
  const heading = signOut ? 'Sign-out did not complete' : 'Sign-in is temporarily unavailable';
  const sentence = signOut ? 'The session service did not answer, so your session may still be active. Try again in a few seconds.' : 'The session service did not answer. Try again in a few seconds.';
  const control =
    retry.kind === 'post'
      ? `<form method="post" action="${escapeHtmlAttribute(retry.action)}"><button type="submit">Sign out again</button></form>`
      : `<p><a href="${escapeHtmlAttribute(retry.href)}">Try again</a></p>`;
  const body = [
    '<!doctype html>',
    '<html lang="en">',
    `<head><meta charset="utf-8"><title>${heading}</title></head>`,
    '<body>',
    `<h1>${heading}</h1>`,
    `<p>${sentence}</p>`,
    control,
    '</body>',
    '</html>',
    '',
  ].join('\n');

  return new Response(body, {
    status: 503,
    headers: {
      'Retry-After': String(RETRY_AFTER_SECONDS),
      'Cache-Control': 'no-store',
      'Content-Type': 'text/html; charset=utf-8',
      'Referrer-Policy': 'no-referrer',
      'Content-Security-Policy': STORE_UNAVAILABLE_CSP,
    },
  });
}

/** What `storeStep` needs from the runtime. An `AuthRuntime` satisfies it. */
export interface StoreFailureContext {
  logger: AuthLogger;
  zone: string;
}

/** The value `storeStep` returns in place of a result when the store is unavailable. */
export const STORE_DOWN = Symbol('paigasus.auth.store-down');

/**
 * Runs ONE store call. A store-unavailable error (classified by `code`, spec D2) becomes
 * `STORE_DOWN` plus one `store.unavailable` event. Any other error propagates unchanged.
 *
 * The event holds only the zone, the fixed stage literal and a truncated sid. Never the caught
 * error: its message holds the redacted DSN (ports/logger.ts's redaction contract).
 */
export async function storeStep<T>(runtime: StoreFailureContext, stage: StoreUnavailableStage, sid: string | undefined, op: () => Promise<T>): Promise<T | typeof STORE_DOWN> {
  try {
    return await op();
  } catch (err) {
    if (!isSessionStoreUnavailable(err)) throw err;
    runtime.logger.event('store.unavailable', { zone: runtime.zone, stage, ...(sid !== undefined ? { sid: sidTag(sid) } : {}) });
    return STORE_DOWN;
  }
}
