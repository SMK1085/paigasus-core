// SPDX-License-Identifier: Apache-2.0
//
// The 503 that an auth route returns when the session store is unavailable (SMA-653, SMA-506
// design § 7.2: "503 with a retry affordance; no partial state"), and, since SMA-656, when OIDC
// discovery fails on /auth/login or /auth/callback (SMA-506 design § 7.1: "login fails, 503 page").
// The names `storeUnavailableResponse` and `STORE_UNAVAILABLE_CSP` are historical: they predate the
// identity-provider case, and a rename is out of scope (SMA-656 § 8).
//
// PRIVATE to this package: routes.ts is the only caller, and server.ts does not export it.
//
// THE PAGE. A fixed HTML document with one retry control: a link for login and callback, and a
// POST form for logout (a link cannot send a POST). It carries NO error text: the error message of
// a SessionStoreUnavailable holds the redacted DSN (adapters/redis-store.ts), an openid-client error
// can hold a URL, and the response is not a place for either in any form. The link variant's
// optional `service` picks the sentence: the session service (the default, so every SMA-653 call
// site stays byte-identical) or the identity provider (SMA-656 D4). The IdP sentence does not say
// WHY discovery failed: a 404 or an issuer mismatch is not "did not answer".
//
// THE HEADERS (spec § 3).
//   - Retry-After: 5. With the shipped 1000 ms store timeout, the SMA-651 circuit cooldown is
//     4000 ms, so a retry after 5 s reaches a closed or re-probing circuit. Advisory only. For the
//     identity-provider case it is advisory too: a retry runs discovery again, because the adapter
//     clears its cached discovery after a failure, and that retry can wait up to
//     PAIGASUS_OIDC_HTTP_TIMEOUT_MS for its answer (SMA-656 D5).
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

/** The link variant. `service` picks the sentence; a missing field means 'session_store' (SMA-656 D4). */
export type RetryLink = { kind: 'link'; href: string; service?: 'session_store' | 'identity_provider' };

/** The post variant (logout) has no `service`, so the type cannot express a state the builder ignores. */
export type RetryAffordance = RetryLink | { kind: 'post'; action: string };

const SESSION_STORE_SENTENCE = 'The session service did not answer. Try again in a few seconds.';
const IDENTITY_PROVIDER_SENTENCE = 'The identity provider is not available. Try again in a few seconds.';
const SIGN_OUT_SENTENCE = 'The session service did not answer, so your session may still be active. Try again in a few seconds.';

function sentenceFor(retry: RetryAffordance): string {
  if (retry.kind === 'post') return SIGN_OUT_SENTENCE;
  const service = retry.service ?? 'session_store';
  return service === 'identity_provider' ? IDENTITY_PROVIDER_SENTENCE : SESSION_STORE_SENTENCE;
}

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
  const sentence = sentenceFor(retry);
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
