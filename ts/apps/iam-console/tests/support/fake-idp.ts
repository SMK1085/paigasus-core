// SPDX-License-Identifier: Apache-2.0
//
// An in-process OIDC provider over HTTPS for the e2e tier (spec § 9.1). HTTPS because
// `authEnvShape` requires an `https:` PAIGASUS_OIDC_ISSUER (ts/packages/paigasus-auth/src/config.ts:22-27).
// Based on ts/packages/paigasus-auth/tests/fixtures/jwks.ts, copied rather than imported.
//
// It approves every authorization request at once, and it CHECKS what a real provider checks and
// what this app must get right under a basePath:
//   - /token recomputes the S256 PKCE challenge from `code_verifier`;
//   - /token requires `redirect_uri` to equal the value sent to /authorize (RFC 6749 § 4.1.3).
//     spec § 7.1 fixes the app side of exactly this, so a mismatch fails the exchange here.
import { createHash, randomBytes, randomUUID } from 'node:crypto';
import { createServer, type Server } from 'node:https';
import type { IncomingMessage, ServerResponse } from 'node:http';
import type { AddressInfo } from 'node:net';
import { exportJWK, generateKeyPair, SignJWT } from 'jose';
import type { TlsMaterial } from './tls';

export type FakeIdp = {
  readonly issuer: string;
  readonly clientId: string;
  readonly clientSecret: string;
  /** Every `redirect_uri` /authorize received, in order. */
  readonly authorizeRedirectUris: string[];
  /** Every `redirect_uri` an authorization_code grant at /token received, in order. */
  readonly tokenRedirectUris: string[];
  /** Every token pair /token issued, in order (authorization_code and refresh_token grants). */
  readonly issued: { accessToken: string; refreshToken: string }[];
  close(): Promise<void>;
};

const CLIENT_ID = 'iam-console-e2e';
const CLIENT_SECRET = 'iam-console-e2e-secret';
const KID = 'fake-idp-key';

type PendingCode = { redirectUri: string; codeChallenge: string; nonce: string | null };

function readBody(req: IncomingMessage): Promise<string> {
  return new Promise((resolve, reject) => {
    const chunks: Buffer[] = [];
    req.on('data', (chunk: Buffer) => chunks.push(chunk));
    req.on('end', () => resolve(Buffer.concat(chunks).toString('utf8')));
    req.on('error', reject);
  });
}

function json(res: ServerResponse, status: number, body: unknown): void {
  res.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' });
  res.end(JSON.stringify(body));
}

function s256(verifier: string): string {
  return createHash('sha256').update(verifier).digest('base64url');
}

/** client_secret_post or client_secret_basic — openid-client picks one from the discovery document. */
function clientAuthenticated(req: IncomingMessage, form: URLSearchParams): boolean {
  const basic = req.headers.authorization;
  if (typeof basic === 'string' && basic.startsWith('Basic ')) {
    const [id, secret] = Buffer.from(basic.slice('Basic '.length), 'base64').toString('utf8').split(':');
    return decodeURIComponent(id ?? '') === CLIENT_ID && decodeURIComponent(secret ?? '') === CLIENT_SECRET;
  }
  return form.get('client_id') === CLIENT_ID && form.get('client_secret') === CLIENT_SECRET;
}

export async function startFakeIdp(opts: { cert: TlsMaterial; subject?: string }): Promise<FakeIdp> {
  const subject = opts.subject ?? 'fake-user-1';
  const keys = await generateKeyPair('RS256', { extractable: true });
  const publicJwk = { ...(await exportJWK(keys.publicKey)), kid: KID, use: 'sig', alg: 'RS256' };

  const codes = new Map<string, PendingCode>();
  const refreshTokens = new Set<string>();
  const authorizeRedirectUris: string[] = [];
  const tokenRedirectUris: string[] = [];
  const issued: { accessToken: string; refreshToken: string }[] = [];
  let issuer = '';

  const mint = (): { accessToken: string; refreshToken: string } => {
    const pair = { accessToken: `fake-at-${randomBytes(16).toString('hex')}`, refreshToken: `fake-rt-${randomBytes(16).toString('hex')}` };
    refreshTokens.add(pair.refreshToken);
    issued.push(pair);
    return pair;
  };

  const idToken = (nonce: string | null): Promise<string> => {
    const jwt = new SignJWT({ email: 'ada@example.test', name: 'Ada Lovelace', ...(nonce === null ? {} : { nonce }) })
      .setProtectedHeader({ alg: 'RS256', kid: KID })
      .setIssuer(issuer)
      .setAudience(CLIENT_ID)
      .setSubject(subject)
      .setIssuedAt()
      .setExpirationTime('10m');
    return jwt.sign(keys.privateKey);
  };

  async function token(req: IncomingMessage, res: ServerResponse): Promise<void> {
    const form = new URLSearchParams(await readBody(req));
    if (!clientAuthenticated(req, form)) {
      json(res, 401, { error: 'invalid_client' });
      return;
    }
    const grant = form.get('grant_type');
    if (grant === 'authorization_code') {
      const code = form.get('code') ?? '';
      const pending = codes.get(code);
      codes.delete(code); // single use
      const redirectUri = form.get('redirect_uri') ?? '';
      tokenRedirectUris.push(redirectUri);
      if (pending === undefined) {
        json(res, 400, { error: 'invalid_grant', error_description: 'unknown or used code' });
        return;
      }
      if (redirectUri !== pending.redirectUri) {
        json(res, 400, { error: 'invalid_grant', error_description: 'redirect_uri does not match the authorization request' });
        return;
      }
      const verifier = form.get('code_verifier');
      if (verifier === null || s256(verifier) !== pending.codeChallenge) {
        json(res, 400, { error: 'invalid_grant', error_description: 'PKCE verification failed' });
        return;
      }
      const pair = mint();
      json(res, 200, { access_token: pair.accessToken, refresh_token: pair.refreshToken, id_token: await idToken(pending.nonce), token_type: 'Bearer', expires_in: 3600 });
      return;
    }
    if (grant === 'refresh_token') {
      const presented = form.get('refresh_token') ?? '';
      if (!refreshTokens.has(presented)) {
        json(res, 400, { error: 'invalid_grant', error_description: 'unknown refresh token' });
        return;
      }
      refreshTokens.delete(presented); // rotation, as most providers do
      const pair = mint();
      // No id_token on refresh: openid-client then has nothing to cross-check against the first one.
      json(res, 200, { access_token: pair.accessToken, refresh_token: pair.refreshToken, token_type: 'Bearer', expires_in: 3600 });
      return;
    }
    json(res, 400, { error: 'unsupported_grant_type' });
  }

  async function handle(req: IncomingMessage, res: ServerResponse): Promise<void> {
    const url = new URL(req.url ?? '/', issuer);
    if (req.method === 'GET' && url.pathname === '/.well-known/openid-configuration') {
      json(res, 200, {
        issuer,
        authorization_endpoint: `${issuer}/authorize`,
        token_endpoint: `${issuer}/token`,
        jwks_uri: `${issuer}/jwks`,
        revocation_endpoint: `${issuer}/revoke`,
        end_session_endpoint: `${issuer}/logout`,
        response_types_supported: ['code'],
        subject_types_supported: ['public'],
        id_token_signing_alg_values_supported: ['RS256'],
        code_challenge_methods_supported: ['S256'],
        token_endpoint_auth_methods_supported: ['client_secret_post', 'client_secret_basic'],
        grant_types_supported: ['authorization_code', 'refresh_token'],
      });
      return;
    }
    if (req.method === 'GET' && url.pathname === '/jwks') {
      json(res, 200, { keys: [publicJwk] });
      return;
    }
    if (req.method === 'GET' && url.pathname === '/authorize') {
      const redirectUri = url.searchParams.get('redirect_uri');
      const state = url.searchParams.get('state');
      const challenge = url.searchParams.get('code_challenge');
      if (url.searchParams.get('client_id') !== CLIENT_ID || redirectUri === null || state === null || challenge === null || url.searchParams.get('code_challenge_method') !== 'S256') {
        json(res, 400, { error: 'invalid_request' });
        return;
      }
      authorizeRedirectUris.push(redirectUri);
      const code = randomUUID();
      codes.set(code, { redirectUri, codeChallenge: challenge, nonce: url.searchParams.get('nonce') });
      const back = new URL(redirectUri);
      back.searchParams.set('code', code);
      back.searchParams.set('state', state);
      res.writeHead(302, { location: back.toString() });
      res.end();
      return;
    }
    if (req.method === 'POST' && url.pathname === '/token') {
      await token(req, res);
      return;
    }
    if (req.method === 'POST' && url.pathname === '/revoke') {
      const form = new URLSearchParams(await readBody(req));
      refreshTokens.delete(form.get('token') ?? '');
      res.writeHead(200);
      res.end();
      return;
    }
    if (req.method === 'GET' && url.pathname === '/logout') {
      const target = url.searchParams.get('post_logout_redirect_uri');
      if (target === null) {
        res.writeHead(200, { 'content-type': 'text/plain' });
        res.end('signed out');
        return;
      }
      const back = new URL(target);
      const state = url.searchParams.get('state');
      if (state !== null) back.searchParams.set('state', state);
      res.writeHead(302, { location: back.toString() });
      res.end();
      return;
    }
    json(res, 404, { error: 'not_found' });
  }

  const server: Server = createServer({ cert: opts.cert.cert, key: opts.cert.key }, (req, res) => {
    handle(req, res).catch(() => {
      if (!res.headersSent) res.writeHead(500);
      res.end();
    });
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const { port } = server.address() as AddressInfo;
  // 127.0.0.1, not `localhost`: the certificate names both, the server listens on IPv4 only, and
  // `localhost` can resolve to ::1 first. The issuer string must equal the `iss` claim byte for
  // byte, so this one spelling is used everywhere.
  issuer = `https://127.0.0.1:${String(port)}`;

  return {
    issuer,
    clientId: CLIENT_ID,
    clientSecret: CLIENT_SECRET,
    authorizeRedirectUris,
    tokenRedirectUris,
    issued,
    close: () =>
      new Promise<void>((resolve, reject) => {
        server.closeAllConnections();
        server.close((err) => (err ? reject(err) : resolve()));
      }),
  };
}
