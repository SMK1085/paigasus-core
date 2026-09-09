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
//
// PKCE ENFORCEMENT (review round 1, Important 2): the /token handler recomputes the S256
// challenge from the request body's `code_verifier` and compares it against whatever
// `setNextCodeChallenge` last set, failing the exchange on a mismatch or an absent verifier.
// This is what makes it possible for a caller (routes.ts's callback tests) to prove it sends the
// verifier IT STORED, rather than a constant or nothing — before this, dropping PKCE from the
// grant call entirely still passed every test, because this handler drained the body without
// looking at it. When `setNextCodeChallenge` has not been called (undefined), the check is
// skipped, so every OTHER caller of this fixture (oidc.test.ts, single-flight's containers tests)
// is unaffected.
import { createHash, randomUUID } from 'node:crypto';
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
  /**
   * Overrides the NEXT /token response's `expires_in` field, which otherwise defaults to `3600`.
   * Pass `undefined` to OMIT the field entirely — oidc.test.ts's missing-`expires_in` cases. Each
   * fixture instance is fresh per test (see `beforeEach`), so there is no "reset to default"
   * need: a test that wants the default simply never calls this.
   */
  setNextExpiresIn(expiresIn: number | undefined): void;
  /**
   * The S256 challenge the NEXT /token request's `code_verifier` must hash to — see the file
   * header. `undefined` (the default) disables the check entirely.
   */
  setNextCodeChallenge(challenge: string | undefined): void;
  close(): Promise<void>;
}

function b64url(input: Uint8Array | string): string {
  const buf = typeof input === 'string' ? Buffer.from(input, 'utf8') : Buffer.from(input);
  return buf.toString('base64url');
}

/** RFC 7636 S256: `BASE64URL(SHA256(code_verifier))`. Exported so tests can compute the matching
 * challenge for whatever `codeVerifier` they seed into a `LoginTransaction`. */
export function s256CodeChallenge(codeVerifier: string): string {
  return createHash('sha256').update(codeVerifier).digest('base64url');
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
  let nextCodeChallenge: string | undefined;
  // A boxed override, distinct from `undefined`-meaning-"not set": `setNextExpiresIn(undefined)`
  // must be distinguishable from never calling it at all, since the former omits the response
  // field and the latter keeps the normal 3600 default.
  let expiresInOverride: { value: number | undefined } | undefined;
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
        const rawBody = await readBody(req);
        if (nextCodeChallenge !== undefined) {
          const params = new URLSearchParams(rawBody);
          const verifier = params.get('code_verifier');
          const computed = verifier !== null ? s256CodeChallenge(verifier) : undefined;
          if (computed === undefined || computed !== nextCodeChallenge) {
            res.writeHead(400, { 'content-type': 'application/json' });
            res.end(JSON.stringify({ error: 'invalid_grant', error_description: 'PKCE verification failed' }));
            return;
          }
        }
        const idToken = nextIdToken;
        const body: Record<string, unknown> = {
          access_token: `at_${b64url(randomUUID())}`,
          refresh_token: `rt_${b64url(randomUUID())}`,
          token_type: 'bearer',
        };
        const expiresIn = expiresInOverride !== undefined ? expiresInOverride.value : 3600;
        if (expiresIn !== undefined) {
          body['expires_in'] = expiresIn;
        }
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
    setNextExpiresIn(expiresIn: number | undefined) {
      expiresInOverride = { value: expiresIn };
    },
    setNextCodeChallenge(challenge: string | undefined) {
      nextCodeChallenge = challenge;
    },
    close(): Promise<void> {
      return new Promise<void>((resolve, reject) => {
        server.close((err) => (err ? reject(err) : resolve()));
      });
    },
  };
}
