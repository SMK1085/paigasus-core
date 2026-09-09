// SPDX-License-Identifier: Apache-2.0
//
// A tiny OIDC provider for tests/adapters/oidc.test.ts. It serves a real
// /.well-known/openid-configuration, a real JWKS document, and a /token
// endpoint that returns an ID token minted with fully overridable claims — so
// `openid-client`'s real discovery and grant paths run end to end, and the
// negative test cases in oidc.test.ts exercise the library's real ID Token
// validation rather than a stub of it.
//
// Signing uses jose's low-level CompactSign, not the JWT-claim SignJWT
// builder: the whole point of this fixture is to mint tokens with
// DELIBERATELY wrong iss/aud/nonce/exp/iat, and SignJWT's claim setters exist
// to make that hard to get wrong by accident. CompactSign signs whatever byte
// payload it is given.
import { randomUUID } from 'node:crypto';
import { createServer, type IncomingMessage, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { CompactSign, exportJWK, generateKeyPair } from 'jose';

export const FIXTURE_CLIENT_ID = 'test-client';
export const FIXTURE_CLIENT_SECRET = 'test-secret';
const PRIMARY_KID = 'primary';

export interface MintIdTokenOptions {
  iss?: string;
  aud?: string;
  sub?: string;
  nonce?: string;
  /** Seconds since epoch. Defaults to now. */
  iat?: number;
  /** Seconds since epoch. Defaults to iat + 300. */
  exp?: number;
  /**
   * Seconds since epoch. Omitted by default — most real ID tokens carry no `nbf`. Included only
   * by the clock-tolerance tests: `nbf` is the one claim oauth4webapi validates in the FUTURE
   * direction against `[client.clockTolerance]` (`nbf > now + clockTolerance` rejects) — see M1.
   * `exp`'s clock-tolerance check runs only in the PAST direction, so it cannot produce a
   * "further into the future is rejected" case on its own.
   */
  nbf?: number;
  /** 'none' mints an unsigned token (empty signature segment). Default 'RS256'. */
  alg?: 'RS256' | 'none';
  /** Sign with a key never published in the JWKS document, under the PRIMARY kid. */
  wrongKey?: boolean;
}

export interface OidcFixture {
  issuer: string;
  clientId: string;
  clientSecret: string;
  /** Mint an ID token with the given overrides, defaulting to a well-formed one. */
  mintIdToken(overrides?: MintIdTokenOptions): Promise<string>;
  /** The next authorization_code or refresh_token grant's /token response uses this ID token. */
  setNextIdToken(idToken: string | undefined): void;
  close(): Promise<void>;
}

function b64url(input: Uint8Array | string): string {
  const buf = typeof input === 'string' ? Buffer.from(input, 'utf8') : Buffer.from(input);
  return buf.toString('base64url');
}

async function readBody(req: IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of req) {
    chunks.push(chunk as Buffer);
  }
  return Buffer.concat(chunks).toString('utf8');
}

/** Starts the fixture server on an ephemeral localhost port and returns its handle. */
export async function startOidcFixture(): Promise<OidcFixture> {
  const primary = await generateKeyPair('RS256', { extractable: true });
  const other = await generateKeyPair('RS256', { extractable: true });
  const primaryJwk = { ...(await exportJWK(primary.publicKey)), kid: PRIMARY_KID, use: 'sig', alg: 'RS256' };

  let nextIdToken: string | undefined;
  let issuer = '';

  const server: Server = createServer((req, res) => {
    void (async () => {
      const url = new URL(req.url ?? '/', 'http://placeholder');
      if (req.method === 'GET' && url.pathname === '/.well-known/openid-configuration') {
        const body = {
          issuer,
          authorization_endpoint: `${issuer}/authorize`,
          token_endpoint: `${issuer}/token`,
          jwks_uri: `${issuer}/jwks`,
          revocation_endpoint: `${issuer}/revoke`,
          end_session_endpoint: `${issuer}/logout`,
          response_types_supported: ['code'],
          subject_types_supported: ['public'],
          id_token_signing_alg_values_supported: ['RS256'],
          token_endpoint_auth_methods_supported: ['client_secret_post'],
          grant_types_supported: ['authorization_code', 'refresh_token'],
        };
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify(body));
        return;
      }
      if (req.method === 'GET' && url.pathname === '/jwks') {
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify({ keys: [primaryJwk] }));
        return;
      }
      if (req.method === 'POST' && url.pathname === '/token') {
        await readBody(req); // drain — the fixture does not need to inspect the grant params
        const idToken = nextIdToken;
        const body: Record<string, unknown> = {
          access_token: `at_${b64url(randomUUID())}`,
          refresh_token: `rt_${b64url(randomUUID())}`,
          token_type: 'bearer',
          expires_in: 3600,
        };
        if (idToken !== undefined) {
          body['id_token'] = idToken;
        }
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end(JSON.stringify(body));
        return;
      }
      if (req.method === 'POST' && url.pathname === '/revoke') {
        await readBody(req);
        res.writeHead(200, { 'content-type': 'application/json' });
        res.end('{}');
        return;
      }
      res.writeHead(404);
      res.end();
    })().catch(() => {
      res.writeHead(500);
      res.end();
    });
  });

  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const address = server.address() as AddressInfo;
  issuer = `http://127.0.0.1:${String(address.port)}`;

  async function mintIdToken(overrides: MintIdTokenOptions = {}): Promise<string> {
    const now = Math.floor(Date.now() / 1000);
    const iat = overrides.iat ?? now;
    const claims: Record<string, unknown> = {
      iss: overrides.iss ?? issuer,
      aud: overrides.aud ?? FIXTURE_CLIENT_ID,
      sub: overrides.sub ?? 'user-1',
      iat,
      exp: overrides.exp ?? iat + 300,
    };
    if (overrides.nonce !== undefined) {
      claims['nonce'] = overrides.nonce;
    }
    if (overrides.nbf !== undefined) {
      claims['nbf'] = overrides.nbf;
    }

    if (overrides.alg === 'none') {
      const header = { alg: 'none', typ: 'JWT' };
      return `${b64url(JSON.stringify(header))}.${b64url(JSON.stringify(claims))}.`;
    }

    const signingKey = overrides.wrongKey === true ? other.privateKey : primary.privateKey;
    return new CompactSign(new TextEncoder().encode(JSON.stringify(claims))).setProtectedHeader({ alg: 'RS256', kid: PRIMARY_KID }).sign(signingKey);
  }

  return {
    issuer,
    clientId: FIXTURE_CLIENT_ID,
    clientSecret: FIXTURE_CLIENT_SECRET,
    mintIdToken,
    setNextIdToken(idToken: string | undefined) {
      nextIdToken = idToken;
    },
    close(): Promise<void> {
      return new Promise<void>((resolve, reject) => {
        server.close((err) => (err ? reject(err) : resolve()));
      });
    },
  };
}
