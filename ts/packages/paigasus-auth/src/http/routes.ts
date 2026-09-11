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
import type { OidcTokens } from '../adapters/oidc';
import { hashSecret, newSessionId, newTransactionId, newTransactionSecret, secretMatchesHash } from '../core/ids';
import { validateReturnTo } from '../core/return-to';
import { CallbackRejected } from '../core/errors';
import type { SessionRecord } from '../core/session';
import { sidTag } from '../ports/logger';
import type { AuthRuntime } from '../runtime';
import { SESSION_COOKIE, TXN_COOKIE_PREFIX, clearCookie, readCookies, serializeCookie, txnCookieName } from './cookies';
import { AUTH_ROUTE_SUFFIXES, type AuthRouteSuffix } from './route-table';

/** Design doc § 9.3: 10 minutes. */
const TXN_TTL_MS = 10 * 60 * 1000;

/** Design doc § 9.2: at most 4 outstanding transaction cookies; the 5th evicts the oldest. */
const MAX_OUTSTANDING_TXN_COOKIES = 4;

export interface AuthRoutes {
  handle(req: Request): Promise<Response>;
}

interface RouteEntry {
  method: 'GET' | 'POST';
  run(runtime: AuthRuntime, req: Request, url: URL): Promise<Response>;
}

/**
 * The route table, keyed by the shared suffix tuple (SMA-626 § 3). `Record<AuthRouteSuffix, …>`
 * closes the drift in BOTH directions at typecheck time: a suffix added to
 * `AUTH_ROUTE_SUFFIXES` with no entry here is a missing key, and an entry here for a suffix not in
 * the tuple is an excess key. That is what binds `middleware.ts`'s `authRoutePaths` to what this
 * file actually serves, rather than to a second hand-written list that could drift from it.
 *
 * Logout is POST, not GET (design doc § 9.5): a GET route that mutates server-side state is
 * triggerable by an `<img src>` from any page on the internet. No CSRF token is added on top —
 * `SameSite=Lax` already withholds __Host-pgs_sid from a cross-site form POST, so a forged POST
 * arrives with no session and does nothing. Recorded here so a later reader does not "fix" it.
 *
 * MEASURED 2026-09-10: adding a fifth suffix (`/auth/probe`) to AUTH_ROUTE_SUFFIXES with no entry
 * here fails typecheck — TS2741, `Property '"/auth/probe"' is missing in type '{ ... }' but
 * required in type 'Record<"/auth/login" | "/auth/callback" | "/auth/logout" |
 * "/auth/logout/callback" | "/auth/probe", RouteEntry>'.` Reverting that and instead adding a
 * spurious `'/auth/probe'` entry here (with the tuple back at four) fails typecheck the other way
 * — TS2353, `Object literal may only specify known properties, and ''/auth/probe'' does not exist
 * in type 'Record<"/auth/login" | "/auth/callback" | "/auth/logout" | "/auth/logout/callback",
 * RouteEntry>'.` Both directions, at build time.
 */
const ROUTES: Record<AuthRouteSuffix, RouteEntry> = {
  '/auth/login': { method: 'GET', run: (runtime, req, url) => handleLogin(runtime, req, url) },
  '/auth/callback': { method: 'GET', run: (runtime, req, url) => handleCallback(runtime, req, url) },
  '/auth/logout': { method: 'POST', run: (runtime, req) => handleLogout(runtime, req) },
  '/auth/logout/callback': { method: 'GET', run: (runtime) => handleLogoutCallback(runtime) },
};

export function createAuthRoutes(runtime: AuthRuntime): AuthRoutes {
  // Built ONCE per runtime, and matched EXACTLY against this zone's own base path — an `endsWith`
  // test previously matched `/anything/auth/login` too, harmless only by accident (review round 1,
  // M5).
  const table = new Map<string, RouteEntry>(AUTH_ROUTE_SUFFIXES.map((suffix) => [`${runtime.basePath}${suffix}`, ROUTES[suffix]]));

  return {
    async handle(req: Request): Promise<Response> {
      const url = new URL(req.url);
      const entry = table.get(url.pathname);
      if (entry === undefined) return new Response(null, { status: 404 });
      if (req.method !== entry.method) return new Response(null, { status: 405 });
      return entry.run(runtime, req, url);
    },
  };
}

async function handleLogin(runtime: AuthRuntime, req: Request, url: URL): Promise<Response> {
  // /auth/login unconditionally clears __Host-pgs_sid (design doc § 10.1's stale-cookie recovery
  // depends on that staying unconditional), which is exactly what makes a cross-site
  // `<img src=".../auth/login">` able to force a logout wherever third-party cookie writes are
  // still permitted — the same class of hazard that made logout a POST. A GET can't be made a
  // POST here (the browser must be redirected, and only a top-level navigation can carry that),
  // so the control instead requires the request itself to BE a top-level navigation:
  // `Sec-Fetch-Mode` is `navigate` for exactly that, and browsers set it on every request since
  // support shipped (an <img>, fetch(), or XHR sends `no-cors`/`cors`, never `navigate`). Absence
  // is allowed rather than rejected: a browser that predates Fetch Metadata support, or a
  // non-browser caller (curl, a test harness), sends no such header at all, and this route has no
  // other way to authenticate that class of caller — failing them closed would break login there
  // entirely, not just the forced-logout attack this exists to close.
  const fetchMode = req.headers.get('sec-fetch-mode');
  if (fetchMode !== null && fetchMode !== 'navigate') {
    return new Response(null, { status: 403 });
  }

  // The fallback must be `${basePath}/`, never bare `basePath` (final fix wave, I3). A
  // root-mounted zone (`PAIGASUS_ZONES={"console":"/"}`) has `basePath === ''`
  // (`@paigasus/next-config`'s `canonicalBasePath` collapses `"/"` to `''`), so a bare `basePath`
  // fallback stores `returnTo: ''`. The callback then redirects to `Location: ''`, the browser
  // resolves that as the CURRENT url and re-requests the callback, the txn cookies are already
  // gone, and the retry loops through `txn_missing` back to `/auth/login` forever — login never
  // completes on a root-mounted zone. `src/next/get-session.ts:110` and `src/runtime.ts:129` both
  // already use the trailing-slash form; this call is the one place that had drifted from it.
  const fallback = `${runtime.basePath}/`;
  const requested = validateReturnTo(url.searchParams.get('returnTo'), fallback);
  // SMA-511 spec § 6.4: a returnTo under this zone's own auth routes would send the browser back into
  // /auth/login (or /auth/callback) after a successful login, so a crafted link loops, one click per
  // round. validateReturnTo accepts such a path, because it is same-origin; it is refused here.
  //
  // The check reads the path with its dot segments resolved, because the browser resolves them in
  // the callback's Location: `/iam/./auth/login` and `/iam/x/../auth/login` both land on
  // `/iam/auth/login`. The placeholder origin only lets `new URL` parse a path. validateReturnTo has
  // already refused every value that is not a same-origin path (`//`, a backslash), so the parse
  // cannot move to another host. The stored value stays `requested`, so its query string is kept.
  const resolvedPath = new URL(requested, 'http://placeholder').pathname;
  const returnTo = resolvedPath.startsWith(`${runtime.basePath}/auth/`) ? fallback : requested;

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

  // I6 (final fix wave): delete the OLD session record here, not only clear its cookie. A
  // single-tab re-login browser-clears __Host-pgs_sid in THIS same 302, so the browser never
  // sends it again — the callback's own `presentedSid` delete (below) therefore never runs for
  // the common case, and a copied cookie stayed live for the full session TTL after the user
  // signed in again. Reading it from the REQUEST (before it is cleared in the response) is what
  // makes this reachable; clearing the browser cookie alone was never enough.
  const presentedSid = readCookies(req.headers.get('cookie')).get(SESSION_COOKIE);
  if (presentedSid !== undefined) {
    await runtime.store.delete(presentedSid);
  }

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
  //
  // TRADE-OFF (review round 1, M1): this consumes the transaction BEFORE the secret check below,
  // so anyone who merely learns a victim's `state` (it is not a secret — it travels in the URL)
  // can burn that transaction with a curl request carrying any cookie value, and the victim's own
  // genuine callback then gets `state_unknown`. That is a one-shot denial of ONE login attempt; it
  // grants the attacker nothing (they still don't have the secret, so they cannot complete the
  // flow themselves) and breaks no security property this file exists to hold. It is not fixed
  // because the `SessionStore` port deliberately offers no peek — `takeTransaction` is atomic
  // get-and-delete by design (ports/session-store.ts), and adding one back would reopen exactly
  // the race it exists to prevent.
  const tx = await runtime.store.takeTransaction(state);
  if (tx === null) return reject('state_unknown');

  if (!secretMatchesHash(cookieSecret, tx.secretHash)) return reject('txn_mismatch');

  // The identity provider itself declining (e.g. the user clicked Cancel) is distinct from a
  // genuine exchange failure (review round 1, Important 1): it carries no `code`, burns nothing
  // at the token endpoint, and is an expected, benign outcome an operator must be able to tell
  // apart from an outage. Checked here, after the CSRF checks above (this must still be a
  // request this browser's own flow produced) but before any token-endpoint call.
  if (url.searchParams.get('error') !== null) return reject('idp_error');

  // SMA-511 spec § 7.1: `currentUrl` is runtime.redirectUri plus the incoming query string, NEVER
  // the request URL. openid-client derives the token request's `redirect_uri` from `currentUrl`
  // (`stripParams(currentUrl)`), and a Next route handler's `req.url` carries the server's bind
  // address, so the value would not equal the one sent to /authorize. Built from the same
  // redirectUri `handleLogin` sends, the two are equal with or without an override.
  const currentUrl = new URL(runtime.redirectUri);
  currentUrl.search = url.search;

  let tokens: OidcTokens;
  try {
    tokens = await runtime.oidc.authorizationCodeGrant({
      currentUrl,
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
  //
  // I6 (final fix wave): `handleLogin` above ALSO deletes the old record, from the REQUEST cookie
  // it sees before clearing it in its own response — that is what covers the common, single-tab
  // re-login, where the browser never sends the old cookie again once `handleLogin`'s 302 clears
  // it. This delete stays for the case that leaves reachable: two tabs sharing one cookie jar,
  // where a second tab's callback can still present the old sid if its request raced ahead of the
  // first tab's clearing response.
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
  // expectedRev: null means "insert only if absent" — a `false` here means a record already
  // exists at this freshly-minted, 256-bit random sid (review round 1, M2). Astronomically
  // unlikely, but an unchecked write on the session-creation path is still an unchecked write.
  const stored = await runtime.store.set(sid, record, runtime.ttlMs, null);
  if (!stored) {
    throw new Error('failed to persist a newly minted session: a record already exists at this sid');
  }

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

// POST /auth/logout — AC 3: "a stolen cookie is dead immediately after". Design doc § 9.5.
//
// ORDER IS THE ACCEPTANCE CRITERION. Step 1 (delete) happens before ANY network call, so a slow
// or unreachable identity provider can never leave a live session behind: whatever happens to
// steps 3-4 afterwards, the record this cookie pointed at is already gone. The `store.get` read
// below is a local store lookup, not a call to the identity provider — the ordering this protects
// is against the IdP specifically, and it is what step 1's own record-lookup needs to find a
// refresh token worth revoking in step 3.
//
// `id_token_hint` IS DELIBERATELY OMITTED, and the redirect is NOT identity-free without it.
// `openid-client@6.8.8` appends `client_id` to the end-session parameters UNCONDITIONALLY
// whenever the caller does not supply one (`build/index.js:1129-1141` — `if
// (!parameters.has('client_id')) parameters.set('client_id', c.client_id);`), so the redirect
// built below already carries `client_id` today, asserted in
// tests/adapters/oidc.test.ts's "buildEndSessionUrl carries client_id ..." test. `client_id` plus
// a registered `post_logout_redirect_uri` is enough for both providers this design names —
// Keycloak (this package's own e2e fixture) and Entra ID — to skip the confirmation interstitial
// and honour the redirect, with no `id_token_hint` needed.
//
// The reason `id_token_hint` itself is never sent: it needs the raw, signed ID TOKEN JWT, not
// decoded claims. `SessionRecord` (core/session.ts) stores only the DECODED `idTokenClaims`, and
// adapters/oidc.ts's `OidcTokens` (produced by `authorizationCodeGrant`) never surfaces the raw
// token string either. Storing it would add a THIRD bearer credential to `SessionRecord` beside
// the access and refresh tokens, widening the blast radius of a Redis compromise, for no benefit
// to either provider this design targets — a deliberate trade-off, not an oversight (see the task
// 9 report and design doc § 9.5).
//
// NAMED RESIDUAL: this is insufficient for an identity provider that MANDATES `id_token_hint` and
// does not accept `client_id` as a substitute — Okta documents it as required. Logging out against
// such a provider still succeeds server-side (step 1 already deleted the record), but the
// end-session redirect will not complete: a UX failure there, not a security one.
async function handleLogout(runtime: AuthRuntime, req: Request): Promise<Response> {
  const cookies = readCookies(req.headers.get('cookie'));
  const sid = cookies.get(SESSION_COOKIE);

  // STEP 1: delete first, before any network call.
  let refreshToken: string | undefined;
  if (sid !== undefined) {
    const rec = await runtime.store.get(sid);
    refreshToken = rec?.refreshToken;
    await runtime.store.delete(sid);
  }

  // STEP 2: clear the session cookie and every outstanding transaction cookie. Unconditional,
  // matching handleLogin's own unconditional clear — a logout with no session cookie at all is a
  // no-op here, not an error, and clearing an already-absent cookie is harmless.
  const headers = new Headers();
  headers.append('Set-Cookie', clearCookie(SESSION_COOKIE));
  for (const name of cookies.keys()) {
    if (name.startsWith(TXN_COOKIE_PREFIX)) headers.append('Set-Cookie', clearCookie(name));
  }

  // STEP 3: revoke the refresh token at the IdP — RFC 7009, BEST EFFORT (design doc § 7.1:
  // "revocation | same | logged, logout continues"). A rejection here must never fail the logout:
  // the record is already gone (step 1), so the session is dead server-side either way. Skipped
  // entirely when there was nothing to revoke (no session, or a session with no refresh token).
  // Never rethrown and never logged as a raw caught error object — it may embed a URL, matching
  // adapters/oidc.ts's own rule — only the boolean outcome below is recorded.
  let revoked = false;
  if (refreshToken !== undefined) {
    try {
      await runtime.oidc.revoke(refreshToken);
      revoked = true;
    } catch {
      // Best-effort: swallowed. `revoked: false` in the event below is the record of this.
    }
  }

  // STEP 4: redirect to end_session_endpoint with post_logout_redirect_uri and a state bound to
  // this logout. `newTransactionId` is reused as a generic opaque-random-id generator (the same
  // function already backs the login flow's own `state`) — logout's `state` carries no secret and
  // needs no separate generator. If the identity provider advertises no end_session_endpoint, or
  // discovery itself fails, the user is still logged out server-side (step 1 already ran), so this
  // degrades to the zone root rather than surfacing a 500 for what is inherently a courtesy step.
  // The degradation is recorded below via `endSessionRedirected: false` — an operator otherwise
  // gets no signal that this happened, unlike step 3's revocation outcome.
  const state = newTransactionId();
  let endSessionRedirected = true;
  try {
    const endSessionUrl = await runtime.oidc.buildEndSessionUrl({ postLogoutRedirectUri: runtime.postLogoutRedirectUri, state });
    headers.set('Location', endSessionUrl);
  } catch {
    // Never rethrown and never logged as a raw caught error object — same rule as step 3's catch.
    endSessionRedirected = false;
    headers.set('Location', runtime.postLogoutRedirectUri);
  }

  runtime.logger.event('logout.completed', {
    zone: runtime.zone,
    ...(sid !== undefined ? { sid: sidTag(sid) } : {}),
    revoked,
    endSessionRedirected,
  });

  return new Response(null, { status: 302, headers });
}

// GET /auth/logout/callback — design doc § 9.5, "unspecified in revision 1".
//
// COSMETIC BY DESIGN. Step 1 of /auth/logout already deleted the session record before this
// request could exist, so by the time the identity provider (or anyone else) reaches this route
// the user is already logged out. Nothing here makes a decision based on the request — not even
// the IdP's `state` query parameter — because there is no stored logout state to compare it
// against and no security property left to protect: a genuine `state`, a forged one, or a request
// with no `state` at all land on the exact same response. Treating an unknown state as an error
// would make this the one place in the package that turns a cosmetic mismatch into a security
// event, which design doc § 9.5 explicitly rules out. If the identity provider never redirects
// back here at all, nothing is lost either — the same reasoning applies.
function handleLogoutCallback(runtime: AuthRuntime): Promise<Response> {
  const headers = new Headers({ Location: runtime.postLogoutRedirectUri });
  headers.append('Set-Cookie', clearCookie(SESSION_COOKIE));
  return Promise.resolve(new Response(null, { status: 302, headers }));
}
