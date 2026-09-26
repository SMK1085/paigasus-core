// SPDX-License-Identifier: Apache-2.0
//
// `openid-client` owns the cryptography — re-testing its signature validation would be testing
// someone else's library. These tests prove OUR WIRING passes the right expectations
// (expectedNonce, expectedState, the audience, the issuer, the clock tolerance) into it, so a
// token that should be rejected is. The negative cases are the point (design doc § 11.3).
//
// The clock-tolerance pair uses `nbf`, not the brief's literal 45 s/120 s "minted in the future"
// figures — see docs/superpowers/specs/2026-09-09-sma-506-measurements.md's M1 addendum for why
// that exact pairing is not reachable against the measured oauth4webapi@3.8.8 behaviour, and why
// `nbf` is the real mechanism this test exercises instead.
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import * as client from 'openid-client';
import { classifyDiscoveryError, createOidcClient, type OidcClient } from '../../src/adapters/oidc.js';
import { startOidcFixture, type OidcFixture } from '../fixtures/jwks.js';
import { closedPortIssuer, startDiscoveryFailureFixture, type DiscoveryFailureFixture } from '../fixtures/discovery-failures.js';
import { OidcDiscoveryFailed, RefreshRejected, isOidcDiscoveryFailed, type OidcDiscoveryFailureReason } from '../../src/core/errors.js';

const REDIRECT_URI = 'https://rp.example.com/auth/callback';
const STATE = 'txn-state-value';
const NONCE = 'expected-nonce-value';

let fixture: OidcFixture;

function makeClient(clockToleranceSeconds = 30): OidcClient {
  return createOidcClient({
    issuer: fixture.issuer,
    clientId: fixture.clientId,
    clientSecret: fixture.clientSecret,
    httpTimeoutMs: 5000,
    clockToleranceSeconds,
    allowInsecureRequests: true, // the fixture is plain http on localhost — never set in production
  });
}

function callbackUrl(): URL {
  return new URL(`${REDIRECT_URI}?code=abc123&state=${STATE}`);
}

async function grant(oidc: OidcClient) {
  return oidc.authorizationCodeGrant({
    currentUrl: callbackUrl(),
    codeVerifier: 'verifier-value',
    expectedState: STATE,
    expectedNonce: NONCE,
  });
}

beforeEach(async () => {
  fixture = await startOidcFixture();
});

afterEach(async () => {
  await fixture.close();
});

describe('createOidcClient — authorizationCodeGrant ID Token validation', () => {
  it('accepts a well-formed token', async () => {
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: NONCE }));
    const tokens = await grant(makeClient());
    expect(tokens.idTokenClaims.sub).toBe('user-1');
    expect(tokens.idTokenClaims.iss).toBe(fixture.issuer);
    expect(tokens.accessToken.length).toBeGreaterThan(0);
  });

  // SMA-681 § 4.2: logout sends this exact string as `id_token_hint`, so the adapter must hand back
  // the raw JWT the IdP issued, not a re-encoding of its claims.
  it('returns the raw id_token string, equal to the minted token', async () => {
    const minted = await fixture.mintIdToken({ nonce: NONCE });
    fixture.setNextIdToken(minted);
    const tokens = await grant(makeClient());
    expect(tokens.idToken).toBe(minted);
  });

  it('rejects a token with the wrong iss', async () => {
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: NONCE, iss: 'https://evil.example.com' }));
    await expect(grant(makeClient())).rejects.toThrow();
  });

  it('rejects a token with the wrong aud', async () => {
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: NONCE, aud: 'someone-elses-client' }));
    await expect(grant(makeClient())).rejects.toThrow();
  });

  it('rejects a token whose nonce does not match the expected nonce', async () => {
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: 'a-different-nonce' }));
    await expect(grant(makeClient())).rejects.toThrow();
  });

  it('rejects a token signed by a different key', async () => {
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: NONCE, wrongKey: true }));
    await expect(grant(makeClient())).rejects.toThrow();
  });

  it('rejects a token with alg: none', async () => {
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: NONCE, alg: 'none' }));
    await expect(grant(makeClient())).rejects.toThrow();
  });

  it('rejects an expired token', async () => {
    const now = Math.floor(Date.now() / 1000);
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: NONCE, iat: now - 600, exp: now - 300 }));
    await expect(grant(makeClient())).rejects.toThrow();
  });

  // The clock-skew pair. See the file header and the M1 addendum: `nbf` is the one claim
  // oauth4webapi validates against [client.clockTolerance] in the FUTURE direction.
  //
  // Review round 1 (Important 3) found the original pair — both at tolerance 30, the library's
  // OWN default (oauth4webapi's getClockTolerance returns 30 when [clockTolerance] is absent) —
  // proved nothing about OUR wiring: it passes identically whether or not
  // `[client.clockTolerance]: opts.clockToleranceSeconds` is ever set. This version uses the
  // SAME minted token (same nbf value) against TWO clients configured with DIFFERENT, both
  // non-default tolerances, so the verdict flips only if the configured value actually reaches
  // the library. Mutation-tested: deleting the `[client.clockTolerance]` line from oidc.ts made
  // the "accepts" half of this test fail (both clients then fall back to the library's default
  // 30, and nbf +45 exceeds 30 under both) — see the M1 addendum for the exact failure output.
  it('the configured clock tolerance — not the library default — decides accept vs reject for the same token', async () => {
    const now = Math.floor(Date.now() / 1000);
    const token = await fixture.mintIdToken({ nonce: NONCE, nbf: now + 45 });
    fixture.setNextIdToken(token);

    // 45 <= 60: accepted under the LARGER, explicitly-configured tolerance.
    await expect(grant(makeClient(60))).resolves.toBeDefined();
    // The identical token, 45 > 30: rejected under the SMALLER tolerance.
    await expect(grant(makeClient(30))).rejects.toThrow();
  });
});

describe('createOidcClient — the rest of the surface', () => {
  it('buildAuthorizationUrl carries PKCE, state and nonce', async () => {
    const oidc = makeClient();
    const req = await oidc.buildAuthorizationUrl({ redirectUri: REDIRECT_URI, scopes: 'openid profile', state: STATE });
    const url = new URL(req.url);
    expect(url.searchParams.get('state')).toBe(STATE);
    expect(url.searchParams.get('nonce')).toBe(req.nonce);
    expect(url.searchParams.get('code_challenge_method')).toBe('S256');
    expect(url.searchParams.get('code_challenge')).toBeTruthy();
    expect(req.codeVerifier.length).toBeGreaterThan(0);
  });

  it('refresh exchanges a refresh token for new tokens', async () => {
    const oidc = makeClient();
    const refreshed = await oidc.refresh('some-refresh-token');
    expect(refreshed.accessToken.length).toBeGreaterThan(0);
    expect(refreshed.expiresIn).toBeGreaterThan(0);
  });

  // CodeRabbit finding (PR 230): `expires_in` is RFC 6749 § 5.1 RECOMMENDED, not required. A
  // response omitting it used to fall back to `expiresIn: 0`, which the login/refresh paths turn
  // into an access token that is already due for refresh the instant it is stored — a silent
  // login loop. Both grant paths must reject instead, with an error carrying no token material.
  it('rejects a code-grant response missing expires_in', async () => {
    fixture.setNextIdToken(await fixture.mintIdToken({ nonce: NONCE }));
    fixture.setNextExpiresIn(undefined);
    const err: unknown = await grant(makeClient()).catch((e: unknown) => e);
    expect(err).toBeInstanceOf(Error);
    expect((err as Error).message).toMatch(/oidc authorization_code_grant failed/);
    // No access/refresh/id token leaked into the error message.
    expect((err as Error).message).not.toMatch(/at_|rt_|ey[A-Za-z0-9_-]{10}/);
  });

  it('rejects a refresh response missing expires_in', async () => {
    const oidc = makeClient();
    fixture.setNextExpiresIn(undefined);
    const err: unknown = await oidc.refresh('some-refresh-token').catch((e: unknown) => e);
    expect(err).toBeInstanceOf(Error);
    expect((err as Error).message).toMatch(/oidc refresh_token_grant failed/);
    expect((err as Error).message).not.toMatch(/at_|rt_|ey[A-Za-z0-9_-]{10}/);
  });

  it('revoke resolves without throwing against a reachable revocation endpoint', async () => {
    const oidc = makeClient();
    await expect(oidc.revoke('some-token')).resolves.toBeUndefined();
  });

  it('buildEndSessionUrl carries the post-logout redirect and the id token hint', async () => {
    const oidc = makeClient();
    const url = await oidc.buildEndSessionUrl({ postLogoutRedirectUri: 'https://rp.example.com/', idTokenHint: 'idtok' });
    const parsed = new URL(url);
    expect(parsed.searchParams.get('post_logout_redirect_uri')).toBe('https://rp.example.com/');
    expect(parsed.searchParams.get('id_token_hint')).toBe('idtok');
    // § 3 row M-g: Keycloak rejects a hint whose `aud` is not the request's client_id. openid-client
    // still appends client_id when a hint is present, and it is the client the hint was issued to.
    expect(parsed.searchParams.get('client_id')).toBe(fixture.clientId);
  });

  // WITNESS TEST (task 9 review): openid-client@6.8.8 appends `client_id` to the end-session
  // parameters whenever the caller does not supply one (build/index.js:1129-1141 — `if
  // (!parameters.has('client_id')) parameters.set('client_id', c.client_id);`). Since SMA-681 the
  // logout route also sends `id_token_hint` when the session record holds one. With no record, the
  // request has `client_id` and `post_logout_redirect_uri` only, and Keycloak then shows its
  // confirmation page when an SSO session is live (SMA-681 spec § 3 row M-b). With a hint,
  // Keycloak needs the `client_id` to equal the hint's `aud` (§ 3 row M-g). Nothing else in this
  // package would notice a future major version dropping this default, so it is asserted here.
  it('buildEndSessionUrl carries client_id even when the caller does not supply one', async () => {
    const oidc = makeClient();
    const url = await oidc.buildEndSessionUrl({ postLogoutRedirectUri: 'https://rp.example.com/' });
    const parsed = new URL(url);
    expect(parsed.searchParams.get('client_id')).toBe(fixture.clientId);
  });

  it('wraps a discovery failure without leaking the underlying error object', async () => {
    const oidc = createOidcClient({
      issuer: 'http://127.0.0.1:1', // nothing listens here
      clientId: fixture.clientId,
      clientSecret: fixture.clientSecret,
      httpTimeoutMs: 500,
      clockToleranceSeconds: 30,
      allowInsecureRequests: true,
    });
    await expect(oidc.refresh('rt')).rejects.toThrow(/oidc discovery failed/);
  });
});

// SMA-681 § 4.2. Keycloak returns a new ID token on a refresh (spec § 3 row M-e). The adapter hands
// it back only when the response carries one. It does not compare `sub` with the login token:
// core/single-flight.ts does that. `setNextIdToken` stays set across requests
// (tests/fixtures/jwks.ts:171), so each case uses a fresh fixture (beforeEach) and sets it itself.
describe('createOidcClient — refresh and the ID token (SMA-681)', () => {
  it('returns the raw refreshed id_token and its claims', async () => {
    // A non-default sub: the fixture default is 'user-1' (tests/fixtures/jwks.ts:216), so a
    // hard-coded claim in the adapter could not pass this.
    const minted = await fixture.mintIdToken({ sub: 'refreshed-subject' });
    fixture.setNextIdToken(minted);
    const refreshed = await makeClient().refresh('some-refresh-token');
    expect(refreshed.rotatedIdToken?.token).toBe(minted);
    expect(refreshed.rotatedIdToken?.claims.sub).toBe('refreshed-subject');
    expect(refreshed.rotatedIdToken?.claims.iss).toBe(fixture.issuer);
  });

  it('returns no rotatedIdToken when the refresh response carries no id_token', async () => {
    const refreshed = await makeClient().refresh('some-refresh-token');
    expect(refreshed).not.toHaveProperty('rotatedIdToken');
    expect(refreshed).not.toHaveProperty('idToken');
  });

  // A GUARD, not red-first: openid-client's non-repudiation hook runs on every refresh response
  // that carries an id_token (openid-client/build/index.js:1029), so this passes before and after
  // the change. It fails if a later change disables the hook, or if it stops covering a refresh.
  it('rejects a refreshed id_token signed by a key the JWKS does not publish', async () => {
    fixture.setNextIdToken(await fixture.mintIdToken({ sub: 'refreshed-subject', wrongKey: true }));
    await expect(makeClient().refresh('some-refresh-token')).rejects.toThrow(/oidc refresh_token_grant failed/);
  });
});

// ---------------------------------------------------------------------------------------------
// MEASUREMENT (SMA-626 § 2.3). The refresh classifier keys on `ResponseBodyError.error`, and that
// only works if a refused refresh actually arrives as that class. oauth4webapi@3.8.8's
// checkOAuthBodyError calls checkAuthenticationChallenges(response) BEFORE parsing the body
// (build/index.js:925-937), so a response carrying WWW-Authenticate throws
// WWWAuthenticateChallengeError instead — which has no `.error` field, and would make a
// ResponseBodyError-keyed classifier silently never fire.
//
// These two tests pin the measured answer. They call openid-client DIRECTLY, because
// createOidcClient.refresh() wraps its error and would hide the class.
// ---------------------------------------------------------------------------------------------
describe('MEASUREMENT: the error class a refused refresh produces', () => {
  async function rawConfig(): Promise<client.Configuration> {
    return client.discovery(new URL(fixture.issuer), fixture.clientId, { client_secret: fixture.clientSecret }, undefined, {
      execute: [client.allowInsecureRequests],
    });
  }

  it('invalid_grant with no WWW-Authenticate arrives as ResponseBodyError carrying .error', async () => {
    const config = await rawConfig();
    fixture.setNextTokenError('invalid_grant');

    const err: unknown = await client.refreshTokenGrant(config, 'rt').catch((e: unknown) => e);

    expect(err).toBeInstanceOf(client.ResponseBodyError);
    expect((err as InstanceType<typeof client.ResponseBodyError>).error).toBe('invalid_grant');
  });

  // The counter-example, and the reason the DEFINITIVE set is `invalid_grant` alone. RFC 6749
  // § 5.2 says a token endpoint SHOULD send WWW-Authenticate with invalid_client, and when it
  // does, the error never reaches the body parser at all.
  it('a WWW-Authenticate response arrives as WWWAuthenticateChallengeError, with no .error field', async () => {
    const config = await rawConfig();
    fixture.setNextTokenError('invalid_client', 'Basic realm="idp"');

    const err: unknown = await client.refreshTokenGrant(config, 'rt').catch((e: unknown) => e);

    expect(err).toBeInstanceOf(client.WWWAuthenticateChallengeError);
    expect(err).not.toBeInstanceOf(client.ResponseBodyError);
  });
});

describe('createOidcClient — refresh failure classification (SMA-626 § 2.2)', () => {
  it('maps invalid_grant to RefreshRejected, carrying only the OAuth code', async () => {
    const oidc = makeClient();
    fixture.setNextTokenError('invalid_grant');

    const err: unknown = await oidc.refresh('some-refresh-token').catch((e: unknown) => e);

    expect(err).toBeInstanceOf(RefreshRejected);
    expect((err as RefreshRejected).oauthError).toBe('invalid_grant');
    expect((err as RefreshRejected).code).toBe('oidc_refresh_rejected');
    // Redaction: the refresh token, the client secret and the issuer URL must not appear.
    expect((err as Error).message).not.toContain('some-refresh-token');
    expect((err as Error).message).not.toContain(fixture.clientSecret);
    expect((err as Error).message).not.toContain(fixture.issuer);
  });

  // The whole point of § 2.2's narrowing: these are per-DEPLOYMENT faults, not session
  // revocations. Classifying them as definitive would sign out every session at once when
  // someone rotates the client secret without updating PAIGASUS_OIDC_CLIENT_SECRET.
  it.each(['invalid_client', 'unauthorized_client', 'invalid_scope', 'invalid_request', 'server_error', 'temporarily_unavailable'])('treats %s as transient, NOT as RefreshRejected', async (code) => {
    const oidc = makeClient();
    fixture.setNextTokenError(code);

    const err: unknown = await oidc.refresh('some-refresh-token').catch((e: unknown) => e);

    expect(err).not.toBeInstanceOf(RefreshRejected);
    expect((err as Error).message).toMatch(/oidc refresh_token_grant failed/);
  });

  // A non-OAuth failure raised INSIDE the try block must not be classified either.
  it('treats a response missing expires_in as transient', async () => {
    const oidc = makeClient();
    fixture.setNextExpiresIn(undefined);

    const err: unknown = await oidc.refresh('some-refresh-token').catch((e: unknown) => e);

    expect(err).not.toBeInstanceOf(RefreshRejected);
  });
});

// ---------------------------------------------------------------------------------------------
// SMA-656 T9. Each row makes discovery fail in one way, through the REAL adapter, and measures the
// `reason`. The D8 table was reasoned from the library code; these rows are its measurement. If a
// row measures another reason, the D8 table is corrected, not this test.
// ---------------------------------------------------------------------------------------------
describe('createOidcClient — a discovery failure is an OidcDiscoveryFailed with a closed reason (SMA-656 T9)', () => {
  let failing: DiscoveryFailureFixture | undefined;

  afterEach(async () => {
    await failing?.close();
    failing = undefined;
  });

  function clientFor(issuer: string, httpTimeoutMs = 2000): OidcClient {
    return createOidcClient({ issuer, clientId: 'test-client', clientSecret: 'test-secret', httpTimeoutMs, clockToleranceSeconds: 30, allowInsecureRequests: true });
  }

  function buildUrl(oidc: OidcClient): Promise<unknown> {
    return oidc.buildAuthorizationUrl({ redirectUri: REDIRECT_URI, scopes: 'openid', state: STATE });
  }

  function expectDiscoveryFailed(err: unknown, reason: OidcDiscoveryFailureReason): void {
    expect(isOidcDiscoveryFailed(err)).toBe(true);
    expect(err).toBeInstanceOf(OidcDiscoveryFailed);
    const failed = err as OidcDiscoveryFailed;
    expect(failed.name).toBe('OidcDiscoveryFailed');
    // D3: the name of the library error, nothing else.
    expect(failed.message).toMatch(/^oidc discovery failed: \w+$/);
    expect(failed.message).not.toContain('127.0.0.1');
    expect(failed.cause).toBeUndefined();
    expect(failed.reason).toBe(reason);
  }

  // Review Focus 5: port 1 is on the Fetch "bad port" list, so no connect happens at all.
  it('an issuer on a port that fetch blocks (http://127.0.0.1:1) -> network', async () => {
    expectDiscoveryFailed(await buildUrl(clientFor('http://127.0.0.1:1')).catch((e: unknown) => e), 'network');
  });

  it('a refused connection (a closed port) -> network', async () => {
    expectDiscoveryFailed(await buildUrl(clientFor(await closedPortIssuer())).catch((e: unknown) => e), 'network');
  });

  it.each([
    ['status-404', 'http_client_error'],
    ['status-503', 'http_server_error'],
    ['not-json', 'invalid_metadata'],
    ['wrong-issuer', 'issuer_mismatch'],
    ['issuer-not-a-url', 'invalid_metadata'],
  ] as const)('%s -> %s', async (mode, reason) => {
    failing = await startDiscoveryFailureFixture(mode);
    const err: unknown = await buildUrl(clientFor(failing.issuer)).catch((e: unknown) => e);
    expect(failing.requests).toBeGreaterThan(0);
    expectDiscoveryFailed(err, reason);
  });

  it('hang, with httpTimeoutMs 200 -> timeout', async () => {
    failing = await startDiscoveryFailureFixture('hang');
    const err: unknown = await buildUrl(clientFor(failing.issuer, 200)).catch((e: unknown) => e);
    expect(failing.requests).toBe(1);
    expectDiscoveryFailed(err, 'timeout');
  });

  // D10: the callback calls authorizationCodeGrant, which runs getConfig() first too.
  it('authorizationCodeGrant throws the same class', async () => {
    const err: unknown = await clientFor('http://127.0.0.1:1')
      .authorizationCodeGrant({ currentUrl: callbackUrl(), codeVerifier: 'verifier-value', expectedState: STATE, expectedNonce: NONCE })
      .catch((e: unknown) => e);
    expectDiscoveryFailed(err, 'network');
  });

  // Review Focus 3, D5: a failed discovery is not cached, so a retry runs discovery again.
  it('the next call after a failed discovery runs discovery again', async () => {
    failing = await startDiscoveryFailureFixture('status-503');
    const oidc = clientFor(failing.issuer);
    expectDiscoveryFailed(await buildUrl(oidc).catch((e: unknown) => e), 'http_server_error');
    expectDiscoveryFailed(await buildUrl(oidc).catch((e: unknown) => e), 'http_server_error');
    expect(failing.requests).toBe(2);
  });
});

// SMA-656 T9b. The D8 rows that a fixture cannot produce cheaply, with constructed errors. The
// shapes copy openid-client 6.8.8's errorHandler (build/index.js:117-165): a ClientError carries
// `code`, and for OAUTH_RESPONSE_IS_NOT_CONFORM its `cause` is the Response; for OAUTH_PARSE_ERROR
// its `cause` is oauth4webapi's error, whose own `cause` is what failed while the body was read.
describe('classifyDiscoveryError — constructed errors (SMA-656 T9b)', () => {
  function clientError(code: string | undefined, cause?: unknown): client.ClientError {
    const err = new client.ClientError('a constructed ClientError', cause === undefined ? undefined : { cause });
    if (code !== undefined) err.code = code;
    return err;
  }

  function parseError(nested: unknown): client.ClientError {
    return clientError('OAUTH_PARSE_ERROR', new Error('failed to parse "response" body as JSON', { cause: nested }));
  }

  function fetchFailed(causeCode?: string): TypeError {
    const cause = causeCode === undefined ? new Error('connect failed') : Object.assign(new Error('connect failed'), { code: causeCode });
    return new TypeError('fetch failed', { cause });
  }

  const status = (value: number): Response => new Response(null, { status: value });

  const ROWS: ReadonlyArray<readonly [string, unknown, OidcDiscoveryFailureReason]> = [
    ['row 1: OAUTH_TIMEOUT', clientError('OAUTH_TIMEOUT', new DOMException('timed out', 'TimeoutError')), 'timeout'],
    ['row 1: OAUTH_ABORT (defensive)', clientError('OAUTH_ABORT', new DOMException('aborted', 'AbortError')), 'timeout'],
    ['row 2: NOT_CONFORM, status 500', clientError('OAUTH_RESPONSE_IS_NOT_CONFORM', status(500)), 'http_server_error'],
    ['row 2: NOT_CONFORM, status 503', clientError('OAUTH_RESPONSE_IS_NOT_CONFORM', status(503)), 'http_server_error'],
    ['row 2: NOT_CONFORM, status 599', clientError('OAUTH_RESPONSE_IS_NOT_CONFORM', status(599)), 'http_server_error'],
    ['row 3: NOT_CONFORM, status 499', clientError('OAUTH_RESPONSE_IS_NOT_CONFORM', status(499)), 'http_client_error'],
    ['row 3: NOT_CONFORM, status 404', clientError('OAUTH_RESPONSE_IS_NOT_CONFORM', status(404)), 'http_client_error'],
    ['row 3: NOT_CONFORM, no cause', clientError('OAUTH_RESPONSE_IS_NOT_CONFORM'), 'http_client_error'],
    ['row 3: NOT_CONFORM, a cause that is not a Response', clientError('OAUTH_RESPONSE_IS_NOT_CONFORM', { status: 503 }), 'http_client_error'],
    ['row 4: PARSE_ERROR, the body timed out', parseError(new DOMException('timed out', 'TimeoutError')), 'timeout'],
    ['row 4: PARSE_ERROR, the body was aborted', parseError(new DOMException('aborted', 'AbortError')), 'timeout'],
    ['row 5: PARSE_ERROR, the connection reset during the body', parseError(new TypeError('terminated')), 'network'],
    ['row 6: PARSE_ERROR, bad JSON', parseError(new SyntaxError('Unexpected token')), 'invalid_metadata'],
    ['row 6: PARSE_ERROR, no nested cause', clientError('OAUTH_PARSE_ERROR'), 'invalid_metadata'],
    ['row 6: RESPONSE_IS_NOT_JSON', clientError('OAUTH_RESPONSE_IS_NOT_JSON', status(200)), 'invalid_metadata'],
    ['row 6: RESPONSE_IS_NOT_JSON, a 503 cause is not read', clientError('OAUTH_RESPONSE_IS_NOT_JSON', status(503)), 'invalid_metadata'],
    ['row 6: INVALID_RESPONSE', clientError('OAUTH_INVALID_RESPONSE'), 'invalid_metadata'],
    ['row 6: INVALID_SERVER_METADATA (defensive)', clientError('OAUTH_INVALID_SERVER_METADATA'), 'invalid_metadata'],
    ['row 6: MISSING_SERVER_METADATA (defensive)', clientError('OAUTH_MISSING_SERVER_METADATA'), 'invalid_metadata'],
    ['row 7: JSON_ATTRIBUTE_COMPARISON_FAILED', clientError('OAUTH_JSON_ATTRIBUTE_COMPARISON_FAILED', { expected: 'https://idp.invalid/', attribute: 'issuer' }), 'issuer_mismatch'],
    ['row 8: TypeError with own code ERR_INVALID_URL', Object.assign(new TypeError('Invalid URL'), { code: 'ERR_INVALID_URL' }), 'invalid_metadata'],
    ['row 9: fetch failed, ENOTFOUND', fetchFailed('ENOTFOUND'), 'dns'],
    ['row 11: fetch failed, ECONNREFUSED', fetchFailed('ECONNREFUSED'), 'network'],
    ['row 11: fetch failed, ECONNRESET', fetchFailed('ECONNRESET'), 'network'],
    ['row 11: fetch failed, EAI_AGAIN', fetchFailed('EAI_AGAIN'), 'network'],
    ['row 11: fetch failed, a cause with no code', fetchFailed(), 'network'],
    ['row 11: fetch failed, a cause code that is a URL', fetchFailed('https://idp.invalid/x'), 'network'],
    ['row 11: a TypeError with no cause', new TypeError('fetch failed'), 'network'],
    ['row 12: an unknown ClientError code', clientError('OAUTH_SOMETHING_NEW'), 'other'],
    ['row 12: a ClientError with no code', clientError(undefined), 'other'],
    ['row 12: HTTP_REQUEST_FORBIDDEN', clientError('OAUTH_HTTP_REQUEST_FORBIDDEN'), 'other'],
    ['row 12: a TypeError with another own code', Object.assign(new TypeError('bad argument'), { code: 'ERR_INVALID_ARG_TYPE' }), 'other'],
    ['row 12: a plain Error', new Error('boom'), 'other'],
    ['row 12: a string', 'https://idp.invalid/x', 'other'],
    ['row 12: undefined', undefined, 'other'],
  ];

  it.each(ROWS)('%s', (_label, err, reason) => {
    expect(classifyDiscoveryError(err)).toBe(reason);
  });

  it.each(['CERT_HAS_EXPIRED', 'DEPTH_ZERO_SELF_SIGNED_CERT', 'SELF_SIGNED_CERT_IN_CHAIN', 'UNABLE_TO_VERIFY_LEAF_SIGNATURE', 'UNABLE_TO_GET_ISSUER_CERT_LOCALLY', 'ERR_TLS_CERT_ALTNAME_INVALID'])(
    'row 10: fetch failed, %s -> tls',
    (code) => {
      expect(classifyDiscoveryError(fetchFailed(code))).toBe('tls');
    },
  );
});
