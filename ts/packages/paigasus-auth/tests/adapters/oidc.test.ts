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
import { createOidcClient, type OidcClient } from '../../src/adapters/oidc.js';
import { startOidcFixture, type OidcFixture } from '../fixtures/jwks.js';

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
