// SPDX-License-Identifier: Apache-2.0
//
// Written against the measured `openid-client@6.8.8` API — see
// docs/superpowers/specs/2026-09-09-sma-506-measurements.md § M1 — not v5-era examples.
// v6 exports FUNCTIONS taking a `Configuration`, not a `Client` class.
//
// DISCOVERY IS LAZY. `createOidcClient` performs no I/O; the first call to any method below
// triggers `client.discovery(...)` once and caches the resulting `Configuration` for every later
// call. This is what lets `createAuthRuntime` validate configuration and fail fast on a bad
// cross-field rule (§ runtime.ts) WITHOUT making a network call — discovery only happens when a
// login, refresh, or logout actually occurs. On a discovery failure the cached promise is
// cleared, so the NEXT call retries rather than replaying the same rejection forever.
//
// CLOCK TOLERANCE IS SYMBOL-KEYED (M1). `[client.clockTolerance]` on the client metadata object,
// not a string option and not a `Configuration` property — `Configuration` only exposes a plain
// `timeout` accessor (also M1), which is what carries PAIGASUS_OIDC_HTTP_TIMEOUT_MS.
//
// NEVER LOG A CAUGHT LIBRARY ERROR OBJECT. `openid-client` errors may embed a URL (the discovery
// document location, a token endpoint). Every method here catches and rethrows through
// `wrapError`, which keeps only the error's `name`.
//
// NON-REPUDIATION CHECKS ARE ALWAYS ENABLED, AND THAT IS NOT FREE. `authorizationCodeGrant` does
// NOT verify the id_token's JWS signature by default (M1 addendum) — enabling it is the right
// call and stays unconditional, but it changes JWKS from "a cache openid-client refreshes
// opportunistically" into a dependency every login AND every refresh now has, with real edge
// cases (M1 records the four measured costs in full: an unreachable jwks_uri fails every login
// AND refresh, not just login as before this change; a rotated, unknown `kid` can 60-second-stall
// a legitimate login before oauth4webapi refetches; an HS256-signed id_token is now fatal — ruled
// acceptable here since ADR-0015 pins IAM to RS256/ES256 for access tokens). JWKS rotation itself
// is handled by oauth4webapi's own keystore/cache, NOT by re-running discovery — but "handled"
// means "cached for 300s and retried at 60s", not "invisible", which is the caveat this paragraph
// exists to state.
import * as client from 'openid-client';
import type { IdTokenClaims } from '../ports/principal-resolver.js';

export interface RefreshedTokens {
  accessToken: string;
  refreshToken?: string;
  expiresIn: number; // seconds
}

export interface OidcTokens extends RefreshedTokens {
  idTokenClaims: IdTokenClaims;
}

export interface AuthorizationRequest {
  url: string;
  codeVerifier: string;
  nonce: string;
}

export interface BuildAuthorizationUrlParams {
  redirectUri: string;
  scopes: string;
  /** The caller's CSRF state value — task 8 binds this to the stored login transaction id. */
  state: string;
}

export interface AuthorizationCodeGrantParams {
  /** The full callback URL the IdP redirected to, including `code` and `state`. */
  currentUrl: URL;
  codeVerifier: string;
  expectedState: string;
  expectedNonce: string;
}

export interface BuildEndSessionUrlParams {
  idTokenHint?: string;
  postLogoutRedirectUri: string;
  state?: string;
}

export interface OidcClient {
  buildAuthorizationUrl(params: BuildAuthorizationUrlParams): Promise<AuthorizationRequest>;
  authorizationCodeGrant(params: AuthorizationCodeGrantParams): Promise<OidcTokens>;
  /** Matches core/single-flight.ts's `ResolveDeps.refresh` signature exactly. */
  refresh(refreshToken: string): Promise<RefreshedTokens>;
  /** Best-effort (design doc § 9.5) — callers decide whether a rejection blocks logout. */
  revoke(token: string): Promise<void>;
  buildEndSessionUrl(params: BuildEndSessionUrlParams): Promise<string>;
}

export interface CreateOidcClientOptions {
  issuer: string;
  clientId: string;
  clientSecret: string;
  httpTimeoutMs: number;
  clockToleranceSeconds: number;
  /**
   * Lifts the HTTPS-only restriction for discovery and every later call on the resulting
   * Configuration (M1). Defaults to false. `authEnvShape.PAIGASUS_OIDC_ISSUER` is a strict
   * `httpsUrl`, so no production configuration can set this — it exists only so
   * tests/adapters/oidc.test.ts can point the adapter at the local plain-http JWKS fixture.
   */
  allowInsecureRequests?: boolean;
}

/**
 * Holds the client secret without ever printing it. `toString`/`toJSON` both return a fixed
 * redacted form, matching redis-store.ts's `RedactedDsn` pattern for the same reason: an
 * accidental `console.log`/`JSON.stringify` on anything holding this must not leak the secret.
 */
class RedactedSecret {
  readonly #value: string;
  constructor(value: string) {
    this.#value = value;
  }
  reveal(): string {
    return this.#value;
  }
  toString(): string {
    return '<redacted>';
  }
  toJSON(): string {
    return '<redacted>';
  }
}

/**
 * Never rethrow a caught openid-client/oauth4webapi error object — several of its error classes
 * (`ResponseBodyError`, `OperationProcessingError`, ...) can carry the request URL. Keep only the
 * error's `name`, which identifies the failure class without any request or response content.
 */
function wrapError(stage: string, cause: unknown): Error {
  const name = cause instanceof Error ? cause.name : 'unknown_error';
  return new Error(`oidc ${stage} failed: ${name}`);
}

export function createOidcClient(opts: CreateOidcClientOptions): OidcClient {
  const secret = new RedactedSecret(opts.clientSecret);
  let configPromise: Promise<client.Configuration> | undefined;

  function getConfig(): Promise<client.Configuration> {
    // M1: openid-client does NOT verify the id_token's JWS signature by default for a plain
    // authorizationCodeGrant — it follows OIDC Core's allowance that TLS to the token endpoint
    // already authenticates the issuer, and leaves signature verification an opt-in
    // ("non-repudiation") extra. Measured directly: without this, a token signed by a key never
    // published in the JWKS document is ACCEPTED. Always enabled, unconditionally — this is
    // exactly what makes the "signed by a different key" rejection in tests/adapters/oidc.test.ts
    // (and the design doc's own requirement) true. It is NOT free — see this file's header
    // comment and the M1 addendum for the four measured costs.
    //
    // ONE ARRAY, BUILT ONCE — deliberately, not `allowInsecureRequests === true ? [a, b] : [b]`.
    // Review round 1 found that shape puts `enableNonRepudiationChecks` in TWO separate literals,
    // one per ternary arm, and every test sets `allowInsecureRequests: true` — so only the test
    // arm ever ran, and deleting the term from the PRODUCTION arm alone would red nothing. A
    // single array with a conditional `unshift` has exactly one occurrence of the term, so there
    // is no unexercised arm left for a future edit to silently break.
    const execute: Array<(config: client.Configuration) => void> = [client.enableNonRepudiationChecks];
    if (opts.allowInsecureRequests === true) {
      execute.unshift(client.allowInsecureRequests);
    }

    configPromise ??= client
      .discovery(new URL(opts.issuer), opts.clientId, { client_secret: secret.reveal(), [client.clockTolerance]: opts.clockToleranceSeconds }, undefined, {
        timeout: opts.httpTimeoutMs / 1000,
        execute,
      })
      .catch((err: unknown) => {
        // Let the NEXT call retry discovery instead of replaying this rejection forever.
        configPromise = undefined;
        throw wrapError('discovery', err);
      });
    return configPromise;
  }

  function toIdTokenClaims(claims: client.IDToken): IdTokenClaims {
    const { iss, sub, email, name } = claims;
    return {
      iss,
      sub,
      ...(typeof email === 'string' ? { email } : {}),
      ...(typeof name === 'string' ? { name } : {}),
    };
  }

  return {
    async buildAuthorizationUrl(params): Promise<AuthorizationRequest> {
      const config = await getConfig();
      try {
        const codeVerifier = client.randomPKCECodeVerifier();
        const codeChallenge = await client.calculatePKCECodeChallenge(codeVerifier);
        const nonce = client.randomNonce();
        const url = client.buildAuthorizationUrl(config, {
          redirect_uri: params.redirectUri,
          scope: params.scopes,
          code_challenge: codeChallenge,
          code_challenge_method: 'S256',
          state: params.state,
          nonce,
        });
        return { url: url.toString(), codeVerifier, nonce };
      } catch (err) {
        // client.buildAuthorizationUrl throws (synchronously, inside this try) when the
        // discovered server metadata has no authorization_endpoint — the no-raw-library-error
        // rule is unconditional, so this path is wrapped exactly like every network-calling one.
        throw wrapError('build_authorization_url', err);
      }
    },

    async authorizationCodeGrant(params): Promise<OidcTokens> {
      const config = await getConfig();
      try {
        const tokens = await client.authorizationCodeGrant(config, params.currentUrl, {
          pkceCodeVerifier: params.codeVerifier,
          expectedState: params.expectedState,
          expectedNonce: params.expectedNonce,
          idTokenExpected: true,
        });
        const claims = tokens.claims();
        if (claims === undefined) {
          throw new Error('no id_token in the token response');
        }
        // RFC 6749 § 5.1 marks `expires_in` RECOMMENDED, not REQUIRED, but this package requires
        // it deliberately: `tokens.expiresIn()` returning `undefined` used to fall back to `0`,
        // which the login path (routes.ts) turns straight into `accessExpiresAt: now`. Every
        // later read would then see the token as already due for refresh — an immediate refresh
        // on read, or an outright session delete when no refresh token exists — a silent login
        // loop. Failing loudly here beats minting a session that expires the instant it is
        // created.
        const expiresIn = tokens.expiresIn();
        if (expiresIn === undefined) {
          throw new Error('token response is missing expires_in');
        }
        return {
          accessToken: tokens.access_token,
          ...(tokens.refresh_token !== undefined ? { refreshToken: tokens.refresh_token } : {}),
          expiresIn,
          idTokenClaims: toIdTokenClaims(claims),
        };
      } catch (err) {
        throw wrapError('authorization_code_grant', err);
      }
    },

    async refresh(refreshToken): Promise<RefreshedTokens> {
      const config = await getConfig();
      try {
        const tokens = await client.refreshTokenGrant(config, refreshToken);
        // See the identical check and comment in authorizationCodeGrant above — the same silent
        // login-loop risk applies to a refresh response missing `expires_in`.
        const expiresIn = tokens.expiresIn();
        if (expiresIn === undefined) {
          throw new Error('token response is missing expires_in');
        }
        return {
          accessToken: tokens.access_token,
          ...(tokens.refresh_token !== undefined ? { refreshToken: tokens.refresh_token } : {}),
          expiresIn,
        };
      } catch (err) {
        throw wrapError('refresh_token_grant', err);
      }
    },

    async revoke(token): Promise<void> {
      const config = await getConfig();
      try {
        await client.tokenRevocation(config, token);
      } catch (err) {
        throw wrapError('token_revocation', err);
      }
    },

    async buildEndSessionUrl(params): Promise<string> {
      const config = await getConfig();
      try {
        const url = client.buildEndSessionUrl(config, {
          post_logout_redirect_uri: params.postLogoutRedirectUri,
          ...(params.idTokenHint !== undefined ? { id_token_hint: params.idTokenHint } : {}),
          ...(params.state !== undefined ? { state: params.state } : {}),
        });
        return url.toString();
      } catch (err) {
        // Throws (synchronously) when the discovered server metadata has no end_session_endpoint.
        throw wrapError('build_end_session_url', err);
      }
    },
  };
}
