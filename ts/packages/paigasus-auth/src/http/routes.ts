// SPDX-License-Identifier: Apache-2.0
//
// /auth/login and /auth/callback — the security core of this package (design doc § 9.2).
//
// THE ATTACK THIS FILE DEFENDS AGAINST. Storing the login transaction server-side keyed by
// `state` does NOT bind the flow to the browser that started it:
//
//   1. An attacker starts a login and authenticates at the identity provider AS THEMSELVES.
//   2. The IdP redirects to /auth/callback?code=...&state=..., and the attacker captures that URL
//      without following it.
//   3. The attacker sends the URL to a victim.
//   4. The victim's browser hits the callback. The server exchanges a perfectly valid code and
//      issues the victim a session AS THE ATTACKER.
//   5. Everything the victim then does lands in the attacker's account.
//
// The defence is a browser-bound secret: /auth/login mints a transaction id and a 32-byte secret,
// stores the transaction under the id with only the secret's SHA-256, and sets a cookie carrying
// the secret. /auth/callback requires that cookie and requires it to hash to the stored value,
// compared with `secretMatchesHash` (constant-time), and rejects BEFORE any token exchange — a
// rejected callback must never spend the authorization code at the IdP.
//
// CallbackRejected is allowed to propagate as a rejected promise from `handle()`. Presenting it as
// an HTTP response (an error page, § 10 of the design doc) is a caller concern — this package does
// not decide that here, matching how core/single-flight.ts lets its own AuthError subclasses
// propagate rather than swallowing them into a "safe" return value.
import type { OidcTokens } from '../adapters/oidc.js';
import { hashSecret, newSessionId, newTransactionId, newTransactionSecret, secretMatchesHash } from '../core/ids.js';
import { validateReturnTo } from '../core/return-to.js';
import { CallbackRejected } from '../core/errors.js';
import type { SessionRecord } from '../core/session.js';
import { sidTag } from '../ports/logger.js';
import type { AuthRuntime } from '../runtime.js';
import { SESSION_COOKIE, TXN_COOKIE_PREFIX, clearCookie, readCookies, serializeCookie, txnCookieName } from './cookies.js';

/** Design doc § 9.3: 10 minutes. */
const TXN_TTL_MS = 10 * 60 * 1000;

/** Design doc § 9.2: at most 4 outstanding transaction cookies; the 5th evicts the oldest. */
const MAX_OUTSTANDING_TXN_COOKIES = 4;

export interface AuthRoutes {
  handle(req: Request): Promise<Response>;
}

export function createAuthRoutes(runtime: AuthRuntime): AuthRoutes {
  return {
    async handle(req: Request): Promise<Response> {
      const url = new URL(req.url);
      const { pathname } = url;

      if (pathname.endsWith('/auth/login')) {
        if (req.method !== 'GET') return new Response(null, { status: 405 });
        return handleLogin(runtime, req, url);
      }

      if (pathname.endsWith('/auth/callback')) {
        if (req.method !== 'GET') return new Response(null, { status: 405 });
        return handleCallback(runtime, req, url);
      }

      return new Response(null, { status: 404 });
    },
  };
}

async function handleLogin(runtime: AuthRuntime, req: Request, url: URL): Promise<Response> {
  const returnTo = validateReturnTo(url.searchParams.get('returnTo'), runtime.basePath);

  const txnId = newTransactionId();
  const secret = newTransactionSecret();

  const authorization = await runtime.oidc.buildAuthorizationUrl({
    redirectUri: runtime.redirectUri,
    scopes: runtime.scopes,
    // The CSRF `state` IS the transaction id, so the callback can find its cookie and its stored
    // transaction from the same value the IdP hands back.
    state: txnId,
  });

  await runtime.store.putTransaction(txnId, { codeVerifier: authorization.codeVerifier, nonce: authorization.nonce, returnTo, secretHash: hashSecret(secret), createdAt: Date.now() }, TXN_TTL_MS);

  const headers = new Headers({ Location: authorization.url });
  // Kills a stale __Host-pgs_sid cookie pointing at a deleted record — this is the one place
  // allowed to write cookies, since a server component cannot (design doc § 10.1).
  headers.append('Set-Cookie', clearCookie(SESSION_COOKIE));

  const outstanding = [...readCookies(req.headers.get('cookie')).keys()].filter((name) => name.startsWith(TXN_COOKIE_PREFIX));
  // Oldest first: user agents list same-path cookies earliest-created first (see cookies.ts).
  while (outstanding.length >= MAX_OUTSTANDING_TXN_COOKIES) {
    const oldest = outstanding.shift();
    if (oldest !== undefined) headers.append('Set-Cookie', clearCookie(oldest));
  }
  headers.append('Set-Cookie', serializeCookie(txnCookieName(txnId), secret, { maxAgeSeconds: TXN_TTL_MS / 1000 }));

  runtime.logger.event('login.started', { zone: runtime.zone });

  return new Response(null, { status: 302, headers });
}

async function handleCallback(runtime: AuthRuntime, req: Request, url: URL): Promise<Response> {
  const state = url.searchParams.get('state');
  const cookies = readCookies(req.headers.get('cookie'));

  const reject = (reason: CallbackRejected['reason']): never => {
    runtime.logger.event('login.callback_rejected', { reason, zone: runtime.zone });
    throw new CallbackRejected(reason);
  };

  if (state === null) return reject('state_unknown');

  const cookieSecret = cookies.get(txnCookieName(state));
  if (cookieSecret === undefined) return reject('txn_missing');

  // Atomic get-and-delete: this is what makes a transaction single-use, so a replayed callback
  // (the same state twice) fails here on its second attempt regardless of the cookie it presents.
  const tx = await runtime.store.takeTransaction(state);
  if (tx === null) return reject('state_unknown');

  if (!secretMatchesHash(cookieSecret, tx.secretHash)) return reject('txn_mismatch');

  let tokens: OidcTokens;
  try {
    tokens = await runtime.oidc.authorizationCodeGrant({
      currentUrl: url,
      codeVerifier: tx.codeVerifier,
      expectedState: state,
      expectedNonce: tx.nonce,
    });
  } catch {
    return reject('code_exchange_failed');
  }

  const principal = await runtime.resolver.resolve({ accessToken: tokens.accessToken, idTokenClaims: tokens.idTokenClaims });

  // Session fixation guard (design doc § 9.4): whatever the browser presented as its CURRENT
  // session is discarded before a new one is minted, regardless of whether the sid the browser
  // holds still resolves to a live record.
  const presentedSid = cookies.get(SESSION_COOKIE);
  if (presentedSid !== undefined) {
    await runtime.store.delete(presentedSid);
  }

  const sid = newSessionId();
  const now = Date.now();
  const record: SessionRecord = {
    version: 1,
    rev: 0,
    accessToken: tokens.accessToken,
    ...(tokens.refreshToken !== undefined ? { refreshToken: tokens.refreshToken } : {}),
    accessExpiresAt: now + tokens.expiresIn * 1000,
    absoluteExpiresAt: now + runtime.absoluteTtlMs,
    idTokenClaims: tokens.idTokenClaims,
    principal,
  };
  await runtime.store.set(sid, record, runtime.ttlMs, null);

  runtime.logger.event('session.created', { sid: sidTag(sid), zone: runtime.zone });

  const headers = new Headers({ Location: tx.returnTo });
  headers.append('Set-Cookie', serializeCookie(SESSION_COOKIE, sid));
  // All outstanding txn cookies are cleared on a successful callback (design doc § 9.2), not just
  // the one just consumed — the flow they belonged to is now either complete or moot.
  for (const name of cookies.keys()) {
    if (name.startsWith(TXN_COOKIE_PREFIX)) headers.append('Set-Cookie', clearCookie(name));
  }

  return new Response(null, { status: 302, headers });
}
