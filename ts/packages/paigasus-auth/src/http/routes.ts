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
//
// A STORE FAILURE IS THE EXCEPTION, and it is mapped HERE (SMA-653 D1). A SessionStoreUnavailable
// from any store call on these routes becomes a 503 with a retry control (http/store-unavailable.ts;
// SMA-506 design § 7.2). Unlike CallbackRejected, the 503 is a fixed rule of the design, not a
// caller choice, and only this file knows WHICH store call failed — the logout route needs that to
// still attempt its delete when its read fails.
//
// AN OIDC DISCOVERY FAILURE IS THE SECOND EXCEPTION (SMA-656). An `OidcDiscoveryFailed` from
// `buildAuthorizationUrl` (login) or `authorizationCodeGrant` (callback) becomes a 503 with a retry
// link that names the identity provider, and one `oidc.discovery_failed` event
// (`discoveryFailedResponse`). It is classified by its `code` (`isOidcDiscoveryFailed`), because the
// shared `runtime.oidc` builds it in whichever module copy created the runtime (SMA-657 D7). Every
// other error still propagates.
import type { AuthorizationRequest, OidcTokens } from '../adapters/oidc';
import { hashSecret, newSessionId, newTransactionId, newTransactionSecret, secretMatchesHash } from '../core/ids';
import { validateReturnTo } from '../core/return-to';
import { CallbackRejected, isOidcDiscoveryFailed, oidcDiscoveryReason } from '../core/errors';
import type { SessionRecord } from '../core/session';
import { sidTag, type OidcDiscoveryStage } from '../ports/logger';
import type { AuthRuntime } from '../runtime';
import { SESSION_COOKIE, TXN_COOKIE_PREFIX, clearCookie, readCookies, serializeCookie, txnCookieName } from './cookies';
import { AUTH_ROUTE_SUFFIXES, type AuthRouteSuffix } from './route-table';
import { STORE_DOWN, loginRetryHref, storeStep, storeUnavailableResponse, type RetryLink } from './store-unavailable';

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

/**
 * True when `path` — basePath-INCLUSIVE, and already normalised by the caller — is one of THIS
 * zone's own auth routes, or lives under one.
 *
 * DERIVED FROM THE ROUTE TABLE (review, defect 3). The `returnTo` guard in `handleLogin` used to
 * test a hardcoded `${basePath}/auth/` prefix while `AUTH_ROUTE_SUFFIXES` is the package's single
 * source of truth for what this file serves. The two agree today only because all four suffixes
 * happen to start with `/auth/`: a fifth route outside that prefix would be served here, be public
 * in `authRoutePaths()`, and escape the guard — so a crafted `returnTo` could send the browser
 * straight back into it after a successful login, one loop per click. Reading the table closes
 * that by construction rather than by coincidence.
 */
function isAuthRoutePath(basePath: string, path: string): boolean {
  return AUTH_ROUTE_SUFFIXES.some((suffix) => {
    const route = `${basePath}${suffix}`;
    // The subtree test keeps the old prefix guard's reach for anything BELOW a route.
    return path === route || path.startsWith(`${route}/`);
  });
}

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

/**
 * The 503 for an OIDC discovery failure (SMA-656 D4, D7), shared by the login and the callback
 * route. Logs one `oidc.discovery_failed` event with the zone, the typed stage and the closed
 * `reason` — never the caught error, its message, its name or a URL (ports/logger.ts's redaction
 * contract; A2). `oidcDiscoveryReason` maps any value outside the closed list to 'other'.
 */
function discoveryFailedResponse(runtime: AuthRuntime, stage: OidcDiscoveryStage, err: unknown, href: string): Response {
  runtime.logger.event('oidc.discovery_failed', { zone: runtime.zone, stage, reason: oidcDiscoveryReason(err) });
  return storeUnavailableResponse({ kind: 'link', href, service: 'identity_provider' });
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
  // The check reads the path with its dot segments resolved AND its empty segments collapsed,
  // because a browser resolves the dot segments in the callback's Location and a server may
  // normalise the duplicate slashes. Both steps are needed, and MEASURED separately: `new URL`
  // resolves `/iam/./auth/login` and `/iam/x/../auth/login` to `/iam/auth/login`, but it does NOT
  // collapse a duplicate slash, so `/iam//auth/login` and `/iam/x/..//auth/login` survive it.
  // validateReturnTo passes those two as well (one leading slash, no backslash, and its `%2f` test
  // reads only the first three characters), so collapsing here is what closes the class without
  // depending on a normalization step this package does not control.
  //
  // The placeholder origin only lets `new URL` parse a path. validateReturnTo has already refused
  // every value that is not a same-origin path (`//`, a backslash), so the parse cannot move to
  // another host. The stored value stays `requested`, so its query string is kept.
  //
  // KNOWN LIMIT, stated rather than closed: the comparison is byte-exact on the collapsed path, so
  // an encoded or case-shifted spelling of the same route (`/iam/%61uth/login`, `/iam/AUTH/login`)
  // is not refused. Neither is decoded or case-folded here on purpose — a path is case-sensitive and
  // `%61` is not the same path segment as `a`, so the route table does not serve either value, and
  // folding them would make this guard reject paths the zone legitimately serves.
  const resolvedPath = new URL(requested, 'http://placeholder').pathname.replace(/\/{2,}/g, '/');
  const returnTo = isAuthRoutePath(runtime.basePath, resolvedPath) ? fallback : requested;

  const txnId = newTransactionId();
  const secret = newTransactionSecret();

  // SMA-653 D9: read the presented sid BEFORE the IdP call and the first store call, because it
  // decides where the 503's retry link points. When this browser holds a session, the link goes to
  // `returnTo`, NOT to /auth/login: during a wedge `requireSession` sends a signed-in user here, the
  // store call below fails first, and the session record and cookie both survive. A retry link to
  // /auth/login would delete that still-valid session once Redis recovers (SMA-651 § 5). The same
  // holds while the IdP cannot be discovered (SMA-656 D6). `returnTo` has already passed
  // validateReturnTo and the auth-route guard above, so it cannot loop back into this route.
  // `readCookies` is pure and cannot throw, so this read before the IdP call changes nothing else.
  const presentedSid = readCookies(req.headers.get('cookie')).get(SESSION_COOKIE);
  const retry: RetryLink = { kind: 'link', href: presentedSid !== undefined ? returnTo : loginRetryHref(runtime.basePath, returnTo) };

  // SMA-656 D1, D2: a discovery failure is the one IdP error that this route answers itself, with a
  // 503. It happens before putTransaction and before the session delete, so no store call runs and
  // nothing changes (D9). Any other error — for example `oidc build_authorization_url failed` when
  // the discovered metadata has no authorization_endpoint — still propagates.
  let authorization: AuthorizationRequest;
  try {
    authorization = await runtime.oidc.buildAuthorizationUrl({
      redirectUri: runtime.redirectUri,
      // The scopes and the audience are not passed here: runtime.oidc holds them (SMA-692 D4).
      // The CSRF `state` IS the transaction id, so the callback can find its cookie and its stored
      // transaction from the same value the IdP hands back.
      state: txnId,
    });
  } catch (err) {
    if (!isOidcDiscoveryFailed(err)) throw err;
    return discoveryFailedResponse(runtime, 'login', err, retry.href);
  }

  const put = await storeStep(runtime, 'login_put_transaction', undefined, () =>
    runtime.store.putTransaction(txnId, { codeVerifier: authorization.codeVerifier, nonce: authorization.nonce, returnTo, secretHash: hashSecret(secret), createdAt: Date.now() }, TXN_TTL_MS),
  );
  if (put === STORE_DOWN) return storeUnavailableResponse(retry);

  // I6 (final fix wave): delete the OLD session record here, not only clear its cookie. A
  // single-tab re-login browser-clears __Host-pgs_sid in THIS same 302, so the browser never
  // sends it again — the callback's own `presentedSid` delete (below) therefore never runs for
  // the common case, and a copied cookie stayed live for the full session TTL after the user
  // signed in again. Reading it from the REQUEST (before it is cleared in the response) is what
  // makes this reachable; clearing the browser cookie alone was never enough.
  //
  // If this delete fails, the stored transaction has no cookie and expires unused (spec § 4 row 2).
  if (presentedSid !== undefined) {
    const deleted = await storeStep(runtime, 'login_delete', presentedSid, () => runtime.store.delete(presentedSid));
    if (deleted === STORE_DOWN) return storeUnavailableResponse(retry);
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
  const tx = await storeStep(runtime, 'callback_take_transaction', undefined, () => runtime.store.takeTransaction(state));
  // No exchange has happened, so nothing is orphaned. The code is not spent, but the retry starts a
  // new login rather than replaying this URL (spec § 11: the page must not carry the code).
  if (tx === STORE_DOWN) return storeUnavailableResponse({ kind: 'link', href: loginRetryHref(runtime.basePath) });
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

  // Read ONCE, before the exchange (SMA-656 § 4.6). The discovery 503 below picks its retry target
  // from it, and the session-fixation delete after the exchange reuses it.
  const presentedSid = cookies.get(SESSION_COOKIE);

  let tokens: OidcTokens;
  try {
    tokens = await runtime.oidc.authorizationCodeGrant({
      currentUrl,
      codeVerifier: tx.codeVerifier,
      expectedState: state,
      expectedNonce: tx.nonce,
    });
  } catch (err) {
    // SMA-656 D10. A discovery failure is NOT a failed code exchange, so it does not reject:
    //   - takeTransaction above has consumed the transaction, so a retry of this URL gives
    //     `state_unknown`. The retry link starts a new login instead (the SMA-653 row 3 rule).
    //   - The code is NOT spent, and no tokens exist: OidcDiscoveryFailed comes only from the
    //     adapter's getConfig(), which runs before any token request (the no-revoke invariant in
    //     adapters/oidc.ts's header). So nothing is revoked, unlike failAfterExchange below.
    //   - The retry target follows SMA-656 D6 (the SMA-653 D9 rule): with a session cookie, the
    //     link goes to tx.returnTo, so a valid session that a second tab created is not deleted by
    //     a retry through /auth/login. tx.returnTo passed validateReturnTo and the auth-route guard
    //     at login.
    //   - No `login.callback_rejected`: the callback was not rejected. `code_exchange_failed` stays
    //     for a real token-endpoint failure, so the two are different in the log.
    // Classified by `code`, not `instanceof`: the shared runtime.oidc builds the error in whichever
    // module copy created the runtime (SMA-657 D7).
    if (isOidcDiscoveryFailed(err)) {
      const href = presentedSid !== undefined ? tx.returnTo : loginRetryHref(runtime.basePath, tx.returnTo);
      return discoveryFailedResponse(runtime, 'callback', err, href);
    }
    return reject('code_exchange_failed');
  }

  const principal = await runtime.resolver.resolve({ accessToken: tokens.accessToken, idTokenClaims: tokens.idTokenClaims });

  // SMA-653 D10. From here on the code exchange has SUCCEEDED, so the IdP holds a session (with
  // `offline_access`, an offline one) and a live refresh token. A store failure below must not
  // orphan it: revoke it, best effort, then answer 503. The retry starts a new login, because the
  // code is spent.
  const failAfterExchange = async (): Promise<Response> => {
    if (tokens.refreshToken !== undefined) await bestEffortRevoke(runtime, tokens.refreshToken);
    return storeUnavailableResponse({ kind: 'link', href: loginRetryHref(runtime.basePath, tx.returnTo) });
  };

  // Session fixation guard (design doc § 9.4): whatever the browser presented as its CURRENT
  // session is discarded before a new one is minted, regardless of whether the sid the browser
  // holds still resolves to a live record.
  //
  // I6 (final fix wave): `handleLogin` above ALSO deletes the old record, from the REQUEST cookie
  // it sees before clearing it in its own response — that is what covers the common, single-tab
  // re-login, where the browser never sends the old cookie again once `handleLogin`'s 302 clears
  // it. This delete stays for the case that leaves reachable: two tabs sharing one cookie jar,
  // where a second tab's callback can still present the old sid if its request raced ahead of the
  // first tab's clearing response. `presentedSid` was read before the exchange (SMA-656 § 4.6).
  if (presentedSid !== undefined) {
    const deleted = await storeStep(runtime, 'callback_delete', presentedSid, () => runtime.store.delete(presentedSid));
    if (deleted === STORE_DOWN) return failAfterExchange();
  }

  const sid = newSessionId();
  const now = Date.now();
  const record: SessionRecord = {
    version: 2,
    rev: 0,
    accessToken: tokens.accessToken,
    ...(tokens.refreshToken !== undefined ? { refreshToken: tokens.refreshToken } : {}),
    accessExpiresAt: now + tokens.expiresIn * 1000,
    absoluteExpiresAt: now + runtime.absoluteTtlMs,
    idToken: tokens.idToken,
    idTokenClaims: tokens.idTokenClaims,
    principal,
  };
  // expectedRev: null means "insert only if absent" — a `false` here means a record already
  // exists at this freshly-minted, 256-bit random sid (review round 1, M2). Astronomically
  // unlikely, but an unchecked write on the session-creation path is still an unchecked write.
  const stored = await storeStep(runtime, 'callback_set', sid, () => runtime.store.set(sid, record, runtime.ttlMs, null));
  if (stored === STORE_DOWN) return failAfterExchange();
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

/**
 * RFC 7009 revocation, BEST EFFORT: `true` when the IdP accepted it, `false` on any failure. Never
 * rethrown and never logged as a raw caught error object — it may embed a URL, matching
 * adapters/oidc.ts's own rule. Used by logout step 3, and by the store-failure paths that would
 * otherwise orphan a live refresh token (SMA-653 D6, D10).
 */
async function bestEffortRevoke(runtime: AuthRuntime, refreshToken: string): Promise<boolean> {
  try {
    await runtime.oidc.revoke(refreshToken);
    return true;
  } catch {
    return false;
  }
}

/**
 * SMA-681. True when the stored ID token's payload `aud` (a string or an array) contains
 * `clientId`. Any decode failure gives false, so logout sends no hint. There is no signature
 * check: the result only selects the logout request, and our own login stored the token.
 */
function hintAudienceMatches(idToken: string, clientId: string): boolean {
  const segments = idToken.split('.');
  if (segments.length !== 3 || segments[1] === undefined) return false;
  let payload: unknown;
  try {
    payload = JSON.parse(Buffer.from(segments[1], 'base64url').toString('utf8'));
  } catch {
    return false;
  }
  if (typeof payload !== 'object' || payload === null) return false;
  const aud: unknown = (payload as { aud?: unknown }).aud;
  return aud === clientId || (Array.isArray(aud) && aud.includes(clientId));
}

// POST /auth/logout — AC 3: "a stolen cookie is dead immediately after". Design doc § 9.5.
//
// ORDER IS THE ACCEPTANCE CRITERION. Step 1 (delete) happens before ANY network call, so a slow
// or unreachable identity provider can never leave a live session behind: whatever happens to
// steps 3-4 afterwards, the record this cookie pointed at is already gone. The `store.get` read
// below is a local store lookup, not a call to the identity provider — the ordering this protects
// is against the IdP specifically, and step 1's own record-lookup finds the refresh token for
// step 3 and the ID token for step 4.
//
// `id_token_hint` IS SENT WHEN THE RECORD HOLDS AN ID TOKEN WHOSE `aud` CONTAINS THIS ZONE'S
// CLIENT ID (SMA-681). This reverses SMA-506.
// SMA-506 (design doc § 9.5) did not store the raw ID token. It argued that `client_id` plus a
// registered `post_logout_redirect_uri` is sufficient for Keycloak, and that the raw token is a
// third bearer credential in Redis. The SMA-506 measurement saw no confirmation page only because
// its realm grants `offline_access` as a default client scope: no online SSO session existed, so
// Keycloak had no session to ask about (SMA-682).
//
// OpenID Connect RP-Initiated Logout 1.0, section 2: the OP MUST ask the End-User whether to log
// out if an `id_token_hint` was not provided. Measured on Keycloak 26.4.7 (SMA-681 spec § 3): with a
// live SSO session and no hint, it answers 200 "Do you want to log out?" (row M-b). With the hint it
// redirects to `post_logout_redirect_uri` and ends the session (row M-c), also with a hint 15 s past
// its `exp` (row M-d) and with a refreshed hint (row M-e). With the shipped `offline_access` scope
// it also redirects at once (§ 3.1, rows M-i1, M-i2, M-i4). So this file sends the stored token
// unchanged and does no `exp` check (spec D4).
//
// `openid-client@6.8.8` still appends `client_id` whenever the caller supplies none
// (`build/index.js:1129-1141`), asserted in tests/adapters/oidc.test.ts. Keycloak rejects a hint
// whose `aud` is a different client with 400 (§ 3 row M-g). Two zones share one session cookie and
// one store (design doc § 6.7), so a zone with another client id can read a record that another zone
// wrote. So the hint is sent only when the token's `aud` contains this runtime's own client id (see
// `hintAudienceMatches`). Otherwise the request is the pre-SMA-681 request, with no hint.
//
// RESIDUAL RISK (spec § 5). The ID token is now in the redirect URL, so it goes into the browser
// history, the IdP's access log and the log of any TLS-terminating proxy. It carries the user's
// email, name and username in base64url, which is not encryption. A person who has it can end the
// user's Keycloak SSO session with no cookies (§ 3 row M-h). It gives no access to an API: its `aud`
// is the console client. Logout by POST would remove this exposure, but it needs an auto-submitting
// form and a CSP `form-action` change, and it is out of scope. An IdP with large ID tokens can
// exceed a request-line limit; that is not measured.
//
// NAMED RESIDUAL: an identity provider that REQUIRES `id_token_hint` still fails when there is no
// token to send: no session cookie, no record (absolute expiry, or the `version: 2` deploy removed
// it), a failed store read, or a stored token whose `aud` does not contain this zone's client id.
// The server-side logout still succeeds then (step 1 already ran); only the end-session redirect
// does not complete, or (in the `aud` case) completes with Keycloak's confirmation page instead.
//
// A STORE FAILURE (SMA-653). A failed read does not stop the delete (D5). A failed delete answers
// 503 with a POST retry form and keeps the session cookie (D6): the user must see that logout did
// not finish, never a false "signed out".
async function handleLogout(runtime: AuthRuntime, req: Request): Promise<Response> {
  const cookies = readCookies(req.headers.get('cookie'));
  const sid = cookies.get(SESSION_COOKIE);

  // STEP 1: delete first, before any network call.
  //
  // The read finds the refresh token for step 3 and the ID token for step 4.
  // Its failure must NEVER cost the delete (SMA-653 D5): a failed read leaves `refreshToken`
  // undefined, and the delete still runs.
  // That rescues a TRANSIENT failure. During a real wedge the read opens the SMA-651 circuit, the
  // circuit then refuses the delete at once, and the delete branch below answers 503 — the usual
  // outcome of a wedge (spec § 4 row 8).
  let refreshToken: string | undefined;
  // SMA-681: the same read gives the hint for step 4. A failed read, no record, or a token whose
  // `aud` does not contain this runtime's client id gives none.
  let idToken: string | undefined;
  if (sid !== undefined) {
    const rec = await storeStep(runtime, 'logout_get', sid, () => runtime.store.get(sid));
    refreshToken = rec === STORE_DOWN ? undefined : rec?.refreshToken;
    const storedIdToken = rec === STORE_DOWN ? undefined : rec?.idToken;
    idToken = storedIdToken !== undefined && hintAudienceMatches(storedIdToken, runtime.clientId) ? storedIdToken : undefined;

    const deleted = await storeStep(runtime, 'logout_delete', sid, () => runtime.store.delete(sid));
    if (deleted === STORE_DOWN) {
      // SMA-653 D6: the record may still be live, so this is NOT a logout. No cookie is cleared (a
      // cleared cookie would show a false "signed out" while a copied cookie stays live) and there
      // is no IdP redirect. The refresh token that the read found is revoked, best effort, so a
      // copied cookie works only until its access token expires. This keeps the § 9.5 order rule:
      // the delete was attempted first, and it failed. The retry form sends the cookie again.
      if (refreshToken !== undefined) await bestEffortRevoke(runtime, refreshToken);
      return storeUnavailableResponse({ kind: 'post', action: `${runtime.basePath}/auth/logout` });
    }
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
  // Best-effort: a failure is swallowed, and `revoked: false` in the event below is its record.
  const revoked = refreshToken !== undefined ? await bestEffortRevoke(runtime, refreshToken) : false;

  // STEP 4: redirect to end_session_endpoint with post_logout_redirect_uri and a state bound to
  // this logout. `newTransactionId` is reused as a generic opaque-random-id generator (the same
  // function already backs the login flow's own `state`) — logout's `state` carries no secret and
  // needs no separate generator. If the identity provider advertises no end_session_endpoint, or
  // discovery itself fails, the user is still logged out server-side (step 1 already ran), so this
  // degrades to the zone root rather than surfacing a 500 for what is inherently a courtesy step.
  // The degradation is recorded below via `endSessionRedirected: false` — an operator otherwise
  // gets no signal that this happened, unlike step 3's revocation outcome.
  // The hint is added only when step 1's read found a token (SMA-681).
  //
  // `idTokenHintSent` (SMA-681) is true only when the Location is an end-session URL that carries
  // the hint. It is NOT named `idTokenHint`, so that no reader and no redaction rule takes it for the
  // token. It never contains the token.
  const state = newTransactionId();
  let endSessionRedirected = true;
  let idTokenHintSent = false;
  try {
    const endSessionUrl = await runtime.oidc.buildEndSessionUrl({
      postLogoutRedirectUri: runtime.postLogoutRedirectUri,
      state,
      ...(idToken !== undefined ? { idTokenHint: idToken } : {}),
    });
    headers.set('Location', endSessionUrl);
    idTokenHintSent = idToken !== undefined;
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
    idTokenHintSent,
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
