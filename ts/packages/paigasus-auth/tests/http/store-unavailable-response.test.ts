// SPDX-License-Identifier: Apache-2.0
//
// SMA-653 § 3: the 503 page and the store-call wrapper, tested directly. The routes that use them
// are tested in store-unavailable.test.ts.
import { describe, expect, it } from 'vitest';
import { SessionStoreTimeout, SessionStoreUnavailable } from '../../src/core/errors.js';
import { STORE_DOWN, STORE_UNAVAILABLE_CSP, escapeHtmlAttribute, loginRetryHref, storeStep, storeUnavailableResponse } from '../../src/http/store-unavailable.js';
import type { AuthEventFields, AuthEventName } from '../../src/ports/logger.js';

const HOSTILE = `/iam/a"b<c>d&e'f#g h`;

// SMA-656 T0. The full page bodies, written on the code BEFORE SMA-656 changed the builder. They
// must pass with no edit after that change: that is the proof that every SMA-653 call site's page
// stays byte-identical (SMA-656 D4).
const LINK_GOLDEN = [
  '<!doctype html>',
  '<html lang="en">',
  '<head><meta charset="utf-8"><title>Sign-in is temporarily unavailable</title></head>',
  '<body>',
  '<h1>Sign-in is temporarily unavailable</h1>',
  '<p>The session service did not answer. Try again in a few seconds.</p>',
  '<p><a href="/iam/auth/login">Try again</a></p>',
  '</body>',
  '</html>',
  '',
].join('\n');

const POST_GOLDEN = [
  '<!doctype html>',
  '<html lang="en">',
  '<head><meta charset="utf-8"><title>Sign-out did not complete</title></head>',
  '<body>',
  '<h1>Sign-out did not complete</h1>',
  '<p>The session service did not answer, so your session may still be active. Try again in a few seconds.</p>',
  '<form method="post" action="/iam/auth/logout"><button type="submit">Sign out again</button></form>',
  '</body>',
  '</html>',
  '',
].join('\n');

// SMA-656 D4, T8. The IdP page: the same heading, the same link, one different sentence.
const IDP_LINK_GOLDEN = [
  '<!doctype html>',
  '<html lang="en">',
  '<head><meta charset="utf-8"><title>Sign-in is temporarily unavailable</title></head>',
  '<body>',
  '<h1>Sign-in is temporarily unavailable</h1>',
  '<p>The identity provider is not available. Try again in a few seconds.</p>',
  '<p><a href="/iam/auth/login">Try again</a></p>',
  '</body>',
  '</html>',
  '',
].join('\n');

function recorder(): { ctx: { zone: string; logger: { event(name: AuthEventName, fields: AuthEventFields): void } }; events: Array<[AuthEventName, AuthEventFields]> } {
  const events: Array<[AuthEventName, AuthEventFields]> = [];
  return { ctx: { zone: 'iam', logger: { event: (name, fields) => void events.push([name, { ...fields }]) } }, events };
}

describe('storeUnavailableResponse', () => {
  it('is a 503 with the § 3 headers and no Set-Cookie', async () => {
    const res = storeUnavailableResponse({ kind: 'link', href: '/iam/auth/login' });
    expect(res.status).toBe(503);
    expect(res.headers.get('retry-after')).toBe('5');
    expect(res.headers.get('cache-control')).toBe('no-store');
    expect(res.headers.get('content-type')).toBe('text/html; charset=utf-8');
    expect(res.headers.get('referrer-policy')).toBe('no-referrer');
    expect(res.headers.get('content-security-policy')).toBe(STORE_UNAVAILABLE_CSP);
    expect(STORE_UNAVAILABLE_CSP).toBe("default-src 'none'; frame-ancestors 'none'; base-uri 'none'");
    expect(res.headers.getSetCookie()).toEqual([]);
    const body = await res.text();
    expect(body).toContain('<a href="/iam/auth/login">');
    expect(body).toContain('Sign-in is temporarily unavailable');
    expect(body).not.toMatch(/<script|<style|style=/i);
  });

  // The logout retry form's successful response is a 302 to the IdP's cross-origin end-session
  // endpoint. Chromium applies `form-action` to the redirects after a form submission, and it
  // blocked that redirect (measured on Playwright 1.63.0 Chromium, SMA-653 final review).
  it('has no form-action, so the logout retry reaches the cross-origin IdP redirect', () => {
    expect(STORE_UNAVAILABLE_CSP).not.toContain('form-action');
  });

  it('renders a POST form for the logout affordance', async () => {
    const body = await storeUnavailableResponse({ kind: 'post', action: '/iam/auth/logout' }).text();
    expect(body).toContain('<form method="post" action="/iam/auth/logout">');
    expect(body).toContain('Sign-out did not complete');
    expect(body).toContain('your session may still be active');
    expect(body).not.toContain('You are still signed in');
    expect(body).not.toContain('<a href=');
  });

  it('HTML-escapes the attribute, in a double-quoted attribute, with exact bytes', async () => {
    const body = await storeUnavailableResponse({ kind: 'link', href: HOSTILE }).text();
    expect(body).toContain('href="/iam/a&quot;b&lt;c&gt;d&amp;e&#39;f#g h"');
    expect(body).not.toContain('b<c');
  });
});

describe('storeUnavailableResponse — golden bodies (SMA-656 T0)', () => {
  it('the link page is byte-identical', async () => {
    expect(await storeUnavailableResponse({ kind: 'link', href: '/iam/auth/login' }).text()).toBe(LINK_GOLDEN);
  });

  it('the post page is byte-identical', async () => {
    expect(await storeUnavailableResponse({ kind: 'post', action: '/iam/auth/logout' }).text()).toBe(POST_GOLDEN);
  });
});

describe('storeUnavailableResponse — the identity provider (SMA-656 D4, T8)', () => {
  it('names the identity provider, with the same headers and no Set-Cookie', async () => {
    const res = storeUnavailableResponse({ kind: 'link', href: '/iam/auth/login', service: 'identity_provider' });
    expect(res.status).toBe(503);
    expect(res.headers.get('retry-after')).toBe('5');
    expect(res.headers.get('cache-control')).toBe('no-store');
    expect(res.headers.get('content-type')).toBe('text/html; charset=utf-8');
    expect(res.headers.get('referrer-policy')).toBe('no-referrer');
    expect(res.headers.get('content-security-policy')).toBe(STORE_UNAVAILABLE_CSP);
    expect(res.headers.getSetCookie()).toEqual([]);
    const body = await res.text();
    expect(body).toBe(IDP_LINK_GOLDEN);
    expect(body).not.toContain('The session service did not answer');
  });

  it('an explicit session_store service gives the T0 page', async () => {
    expect(await storeUnavailableResponse({ kind: 'link', href: '/iam/auth/login', service: 'session_store' }).text()).toBe(LINK_GOLDEN);
  });

  // Review Focus 1: the IdP variant escapes the attribute exactly as the store variant does.
  it('HTML-escapes a hostile href in the IdP page', async () => {
    const body = await storeUnavailableResponse({ kind: 'link', href: HOSTILE, service: 'identity_provider' }).text();
    expect(body).toContain('href="/iam/a&quot;b&lt;c&gt;d&amp;e&#39;f#g h"');
    expect(body).not.toContain('b<c');
  });
});

describe('escapeHtmlAttribute', () => {
  it('escapes the five characters, & first', () => {
    expect(escapeHtmlAttribute(`&<>"'`)).toBe('&amp;&lt;&gt;&quot;&#39;');
    expect(escapeHtmlAttribute('&lt;')).toBe('&amp;lt;');
  });
});

describe('loginRetryHref', () => {
  it('is the bare login route with no returnTo', () => {
    expect(loginRetryHref('/iam')).toBe('/iam/auth/login');
    expect(loginRetryHref('')).toBe('/auth/login');
  });

  it('URL-encodes returnTo so that & and # survive a round trip', () => {
    const href = loginRetryHref('/iam', HOSTILE);
    expect(href).toBe(`/iam/auth/login?returnTo=${encodeURIComponent(HOSTILE)}`);
    expect(new URL(href, 'https://rp.example.com').searchParams.get('returnTo')).toBe(HOSTILE);
  });
});

describe('storeStep', () => {
  it('returns the value when the call succeeds, and logs nothing', async () => {
    const { ctx, events } = recorder();
    await expect(storeStep(ctx, 'logout_get', 'sid-0123456789', () => Promise.resolve(42))).resolves.toBe(42);
    expect(events).toEqual([]);
  });

  it.each([
    ['SessionStoreUnavailable', () => new SessionStoreUnavailable('down (redis://u:sentinel-pw@redis.invalid)')],
    ['SessionStoreTimeout', () => new SessionStoreTimeout('redis://u:sentinel-pw@redis.invalid', 4000, 'deadline')],
  ])('maps a %s to STORE_DOWN and logs one redacted event', async (_name, makeError) => {
    const { ctx, events } = recorder();
    await expect(storeStep(ctx, 'logout_delete', 'sid-0123456789', () => Promise.reject(makeError()))).resolves.toBe(STORE_DOWN);
    expect(events).toEqual([['store.unavailable', { zone: 'iam', stage: 'logout_delete', sid: 'sid-0123' }]]);
    expect(JSON.stringify(events)).not.toContain('sentinel');
  });

  it('omits sid when the step has none', async () => {
    const { ctx, events } = recorder();
    await storeStep(ctx, 'callback_take_transaction', undefined, () => Promise.reject(new SessionStoreUnavailable('down')));
    expect(events).toEqual([['store.unavailable', { zone: 'iam', stage: 'callback_take_transaction' }]]);
  });

  it('re-throws any other error unchanged', async () => {
    const { ctx, events } = recorder();
    const boom = new Error('boom');
    await expect(storeStep(ctx, 'login_delete', undefined, () => Promise.reject(boom))).rejects.toBe(boom);
    expect(events).toEqual([]);
  });
});
