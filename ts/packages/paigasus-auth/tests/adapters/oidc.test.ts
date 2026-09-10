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
import { createOidcClient, type OidcClient } from '../../src/adapters/oidc.js';
import { startOidcFixture, type OidcFixture } from '../fixtures/jwks.js';
import { RefreshRejected } from '../../src/core/errors.js';

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
  });

  // WITNESS TEST (task 9 review): openid-client@6.8.8 appends `client_id` to the end-session
  // parameters UNCONDITIONALLY whenever the caller does not supply one
  // (build/index.js:1129-1141 — `if (!parameters.has('client_id')) parameters.set('client_id',
  // c.client_id);`). This is load-bearing: task 9's logout route never sets `idTokenHint`, and
  // `client_id` plus a registered `post_logout_redirect_uri` is what lets Keycloak and Entra ID
  // skip the confirmation interstitial in its absence (routes.ts's handleLogout doc comment).
  // Nothing else in this package would notice a future major version silently dropping this
  // default, so it is asserted here directly.
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

// ---------------------------------------------------------------------------------------------
// MEASUREMENT (SMA-626 § 2.3). The refresh classifier keys on `ResponseBodyError.error`, and that
// only works if a refused refresh actually arrives as that class. oauth4webapi@3.8.8's
// checkOAuthBodyError calls checkAuthenticationChallenges(response) BEFORE parsing the body
// (build/index.js:917-937), so a response carrying WWW-Authenticate throws
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
