// SPDX-License-Identifier: Apache-2.0
import { createHash, randomBytes } from 'node:crypto';
import { afterAll, beforeAll, describe, expect, it } from 'vitest';
import { createLocalJWKSet, jwtVerify, type JSONWebKeySet } from 'jose';
import { startFakeIdp, type FakeIdp } from '../../support/fake-idp';
import { httpsRequest } from '../../support/https-client';
import { testTls, type TlsMaterial } from '../../support/tls';

const REDIRECT = 'https://127.0.0.1:9/iam/auth/callback';

describe('the fake IdP', () => {
  let tls: TlsMaterial;
  let idp: FakeIdp;

  beforeAll(async () => {
    tls = testTls();
    idp = await startFakeIdp({ cert: tls });
  });
  afterAll(() => idp.close());

  async function authorize(redirectUri: string, verifier: string): Promise<string> {
    const url = new URL(`${idp.issuer}/authorize`);
    url.search = new URLSearchParams({
      response_type: 'code',
      client_id: idp.clientId,
      redirect_uri: redirectUri,
      scope: 'openid',
      state: 'state-1',
      nonce: 'nonce-1',
      code_challenge: createHash('sha256').update(verifier).digest('base64url'),
      code_challenge_method: 'S256',
    }).toString();
    const res = await httpsRequest(url.toString(), tls);
    expect(res.status).toBe(302);
    const location = new URL(String(res.headers.location));
    expect(location.searchParams.get('state')).toBe('state-1');
    return location.searchParams.get('code') ?? '';
  }

  function exchange(code: string, redirectUri: string, verifier: string) {
    return httpsRequest(`${idp.issuer}/token`, tls, {
      method: 'POST',
      headers: { 'content-type': 'application/x-www-form-urlencoded' },
      body: new URLSearchParams({ grant_type: 'authorization_code', code, redirect_uri: redirectUri, code_verifier: verifier, client_id: idp.clientId, client_secret: idp.clientSecret }).toString(),
    });
  }

  it('serves a discovery document whose issuer is its own https origin', async () => {
    const res = await httpsRequest(`${idp.issuer}/.well-known/openid-configuration`, tls);
    expect(res.status).toBe(200);
    expect((JSON.parse(res.body) as { issuer: string }).issuer).toBe(idp.issuer);
    expect(idp.issuer.startsWith('https://')).toBe(true);
  });

  it('issues a signed id_token for a PKCE exchange with the same redirect_uri', async () => {
    const verifier = randomBytes(32).toString('base64url');
    const code = await authorize(REDIRECT, verifier);
    const res = await exchange(code, REDIRECT, verifier);
    expect(res.status).toBe(200);
    const body = JSON.parse(res.body) as { access_token: string; refresh_token: string; id_token: string; expires_in: number };
    const jwks = JSON.parse((await httpsRequest(`${idp.issuer}/jwks`, tls)).body) as JSONWebKeySet;
    const { payload } = await jwtVerify(body.id_token, createLocalJWKSet(jwks), { issuer: idp.issuer, audience: idp.clientId });
    expect(payload.nonce).toBe('nonce-1');
    expect(body.expires_in).toBe(3600);
    expect(idp.issued.at(-1)).toEqual({ accessToken: body.access_token, refreshToken: body.refresh_token });
    expect(idp.authorizeRedirectUris.at(-1)).toBe(REDIRECT);
    expect(idp.tokenRedirectUris.at(-1)).toBe(REDIRECT);
  });

  it('rejects an exchange whose redirect_uri differs from the authorization request', async () => {
    const verifier = randomBytes(32).toString('base64url');
    const code = await authorize(REDIRECT, verifier);
    const res = await exchange(code, 'https://127.0.0.1:9/auth/callback', verifier);
    expect(res.status).toBe(400);
    expect(JSON.parse(res.body)).toMatchObject({ error: 'invalid_grant' });
  });

  it('rejects a wrong PKCE verifier', async () => {
    const code = await authorize(REDIRECT, randomBytes(32).toString('base64url'));
    const res = await exchange(code, REDIRECT, randomBytes(32).toString('base64url'));
    expect(res.status).toBe(400);
  });

  it('rotates a refresh token and refuses the old one', async () => {
    const verifier = randomBytes(32).toString('base64url');
    const first = JSON.parse((await exchange(await authorize(REDIRECT, verifier), REDIRECT, verifier)).body) as { refresh_token: string };
    const refresh = (token: string) =>
      httpsRequest(`${idp.issuer}/token`, tls, {
        method: 'POST',
        headers: { 'content-type': 'application/x-www-form-urlencoded' },
        body: new URLSearchParams({ grant_type: 'refresh_token', refresh_token: token, client_id: idp.clientId, client_secret: idp.clientSecret }).toString(),
      });
    expect((await refresh(first.refresh_token)).status).toBe(200);
    expect((await refresh(first.refresh_token)).status).toBe(400);
  });
});
