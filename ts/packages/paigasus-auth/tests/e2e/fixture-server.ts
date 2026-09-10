// SPDX-License-Identifier: Apache-2.0
//
// A plain `node:http` server exercising @paigasus/auth the way a real Next app would — but
// through the Web-standard `(Request) => Promise<Response>` contract src/server.ts's
// `createAuthRouteHandler` exposes, since this package targets that contract, not Next's App
// Router request lifecycle specifically. Started by playwright.config.ts's `webServer` via
// `node --experimental-transform-types --import tests/fixtures/ts-esm-loader.mjs --import
// tests/e2e/e2e-loader.mjs tests/e2e/fixture-server.ts` — see e2e-loader.mjs's own header for M8
// (why NOT `NODE_OPTIONS=--conditions=react-server`, the brief's original plan).
//
// WHY THIS DOES NOT CALL `getSession`/`requireSession` (src/next/get-session.ts), even though
// src/server.ts re-exports both and the brief's Step 3 describes a "requireSession()-guarded
// page". Those two functions are structurally coupled to Next's App Router request lifecycle:
// `getSession` calls `next/headers`'s `cookies()`, which reads a store Next's OWN server
// populates via AsyncLocalStorage for the lifetime of one Next-handled request, and
// `requireSession` calls `next/navigation`'s `redirect()`, which throws a sentinel Next's own
// rendering pipeline catches — see tests/next/get-session.test.ts's header, which mocks BOTH for
// exactly this reason ("this suite runs in plain Node, with no Next request context to read
// cookies from or to redirect within"). A bare `node:http` server IS plain Node with no Next
// request context, so calling either for real here would throw the same way, not exercise
// anything AC 1/AC 3/§10.1 need proven.
//
// The guarded page below therefore reimplements exactly what `getSession` does INTERNALLY —
// read the session cookie via `readCookies` (the same low-level parser `http/routes.ts` uses) and
// resolve it via `resolveSession` (`core/single-flight.js`, the function `getSession` itself
// calls) — and redirects on `null` exactly as `requireSession` does. This is not a weakened
// stand-in: it is the same session-resolution logic under test, minus the Next-specific plumbing
// that cannot run outside a Next server and that none of task 13's specs are about.
import http from 'node:http';
import { Readable } from 'node:stream';
import { createAuthRouteHandler, getAuthRuntime } from '../../src/server.js';
import type { ComposedConfig } from '../../src/runtime.js';
import { resolveSession } from '../../src/core/single-flight.js';
import { validateReturnTo } from '../../src/core/return-to.js';
import { readCookies, SESSION_COOKIE } from '../../src/http/cookies.js';
import { FIXTURE_SERVER_ORIGIN, FIXTURE_SERVER_PORT, KEYCLOAK_CLIENT_ID, KEYCLOAK_CLIENT_SECRET, ZONE, ZONE_BASE_PATH } from './constants.js';
import { tryReadRuntimeEnv, type RuntimeEnv } from './runtime-env.js';

const RUNTIME_ENV_WAIT_MS = 120_000;

/**
 * Polls for the file tests/e2e/global-setup.ts writes. This is what makes the relative order of
 * Playwright's `globalSetup` and `webServer` NOT MATTER: whichever starts first, this server
 * cannot open its listening port (and therefore cannot pass `webServer.url`'s readiness check)
 * until global-setup.ts has started Keycloak and Redis and written their connection details.
 */
async function waitForRuntimeEnv(timeoutMs: number): Promise<RuntimeEnv> {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const env = tryReadRuntimeEnv();
    if (env !== undefined) return env;
    if (Date.now() >= deadline) {
      throw new Error(`tests/e2e/global-setup.ts never published its runtime env within ${String(timeoutMs)}ms`);
    }
    await new Promise((resolvePromise) => setTimeout(resolvePromise, 250));
  }
}

interface RequestInitWithDuplex extends RequestInit {
  duplex?: 'half';
}

function toWebRequest(req: http.IncomingMessage, origin: string): Request {
  const url = new URL(req.url ?? '/', origin);
  const headers = new Headers();
  for (const [key, value] of Object.entries(req.headers)) {
    if (value === undefined) continue;
    if (Array.isArray(value)) {
      for (const v of value) headers.append(key, v);
    } else {
      headers.append(key, value);
    }
  }
  const method = req.method ?? 'GET';
  const init: RequestInitWithDuplex = { method, headers };
  if (method !== 'GET' && method !== 'HEAD') {
    init.body = Readable.toWeb(req) as ReadableStream<Uint8Array>;
    init.duplex = 'half';
  }
  return new Request(url, init);
}

async function writeWebResponse(res: http.ServerResponse, response: Response): Promise<void> {
  res.statusCode = response.status;
  response.headers.forEach((value, key) => {
    if (key.toLowerCase() === 'set-cookie') return; // handled below — see getSetCookie's own doc.
    res.setHeader(key, value);
  });
  const setCookies = response.headers.getSetCookie();
  if (setCookies.length > 0) res.setHeader('Set-Cookie', setCookies);
  const body = Buffer.from(await response.arrayBuffer());
  res.end(body);
}

function htmlResponse(html: string, status = 200): Response {
  return new Response(html, { status, headers: { 'content-type': 'text/html; charset=utf-8' } });
}

function escapeHtml(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

function publicPage(): Response {
  return htmlResponse(`<!doctype html>
<html><body>
<h1 data-testid="public-heading">paigasus-auth E2E fixture</h1>
<a href="${ZONE_BASE_PATH}/guarded" data-testid="guarded-link">Guarded page</a>
</body></html>`);
}

async function main(): Promise<void> {
  const env = await waitForRuntimeEnv(RUNTIME_ENV_WAIT_MS);

  // A hand-built ComposedConfig, exactly like tests/runtime.test.ts's BASE fixture — this fixture
  // never calls @paigasus/next-config's `defineRuntimeConfig`/`authEnvShape.parse`, so
  // PAIGASUS_PUBLIC_ORIGIN being `http://` rather than `https://` is not zod-checked here (see
  // constants.ts's own comment on FIXTURE_SERVER_ORIGIN — this is the M11 subject, not an
  // oversight).
  const cfg: ComposedConfig = {
    PAIGASUS_OIDC_ISSUER: env.issuer,
    PAIGASUS_OIDC_CLIENT_ID: KEYCLOAK_CLIENT_ID,
    PAIGASUS_OIDC_CLIENT_SECRET: KEYCLOAK_CLIENT_SECRET,
    PAIGASUS_PUBLIC_ORIGIN: FIXTURE_SERVER_ORIGIN,
    PAIGASUS_OIDC_SCOPES: 'openid profile email offline_access',
    PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS: 30,
    PAIGASUS_OIDC_HTTP_TIMEOUT_MS: 3500,
    PAIGASUS_SESSION_STORE: 'redis',
    PAIGASUS_SESSION_REDIS_URL: env.redisUrl,
    PAIGASUS_SESSION_REDIS_TIMEOUT_MS: 1000,
    PAIGASUS_SESSION_TTL_SECONDS: 28800,
    PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS: 86400,
    PAIGASUS_SESSION_REFRESH_SKEW_SECONDS: 30,
    PAIGASUS_SESSION_LOCK_TTL_MS: 10000,
    PAIGASUS_SESSION_LOCK_WAIT_MS: 3000,
    PAIGASUS_ZONE: ZONE,
    PAIGASUS_ZONES: { [ZONE]: ZONE_BASE_PATH },
  };

  const runtime = await getAuthRuntime(cfg);
  const authHandler = createAuthRouteHandler(runtime);
  const authPrefix = `${runtime.basePath}/auth/`;
  const guardedPath = `${runtime.basePath}/guarded`;
  const rootPath = `${runtime.basePath}/`;
  const debugSessionPath = `${runtime.basePath}/__test__/session`;

  async function handleGuardedPage(request: Request): Promise<Response> {
    const cookies = readCookies(request.headers.get('cookie'));
    const sid = cookies.get(SESSION_COOKIE);
    const resolved =
      sid === undefined
        ? null
        : await resolveSession(
            {
              store: runtime.store,
              refresh: (refreshToken) => runtime.oidc.refresh(refreshToken),
              logger: runtime.logger,
              skewMs: runtime.skewMs,
              lockTtlMs: runtime.lockTtlMs,
              lockWaitMs: runtime.lockWaitMs,
              ttlMs: runtime.ttlMs,
            },
            sid,
          ).catch((err: unknown) => {
            // A redacted diagnostic, never the raw error: node-redis's own connection errors
            // embed the DSN, and openid-client's can embed a URL. Without this line a store or
            // IdP failure here is indistinguishable from "no session" — the guarded page just
            // redirects to login and hides which system actually failed.
            const kind = err instanceof Error ? err.name : typeof err;
            console.error(`fixture guarded page: session resolution failed (${kind})`);
            return null;
          });

    if (resolved === null) {
      const returnTo = validateReturnTo(guardedPath, rootPath);
      const loginUrl = `${runtime.basePath}/auth/login?returnTo=${encodeURIComponent(returnTo)}`;
      return new Response(null, { status: 302, headers: { Location: loginUrl } });
    }

    return htmlResponse(`<!doctype html>
<html><body>
<h1 data-testid="guarded-heading">Guarded page</h1>
<p data-testid="principal-subject">${escapeHtml(resolved.principal.subject)}</p>
<form method="POST" action="${runtime.basePath}/auth/logout">
  <button type="submit" data-testid="logout-button">Log out</button>
</form>
</body></html>`);
  }

  /**
   * TEST-ONLY. Never shipped outside this fixture: it exposes the raw session record (including
   * the access token) so roundtrip.spec.ts can assert the __Host-pgs_sid cookie's VALUE never
   * contains the access token, against ground truth rather than a heuristic. No auth of its own
   * beyond requiring the same session cookie every other route requires.
   */
  async function handleDebugSession(request: Request): Promise<Response> {
    const cookies = readCookies(request.headers.get('cookie'));
    const sid = cookies.get(SESSION_COOKIE);
    if (sid === undefined) return new Response(null, { status: 404 });
    const record = await runtime.store.get(sid);
    if (record === null) return new Response(null, { status: 404 });
    return new Response(JSON.stringify({ sid, accessToken: record.accessToken, refreshToken: record.refreshToken ?? null }), {
      status: 200,
      headers: { 'content-type': 'application/json' },
    });
  }

  const server = http.createServer((req, res) => {
    void (async () => {
      try {
        const request = toWebRequest(req, FIXTURE_SERVER_ORIGIN);
        const { pathname } = new URL(request.url);

        if (pathname.startsWith(authPrefix)) {
          await writeWebResponse(res, await authHandler(request));
          return;
        }
        if (pathname === guardedPath) {
          await writeWebResponse(res, await handleGuardedPage(request));
          return;
        }
        if (pathname === debugSessionPath) {
          await writeWebResponse(res, await handleDebugSession(request));
          return;
        }
        if (pathname === rootPath) {
          await writeWebResponse(res, publicPage());
          return;
        }
        res.writeHead(404);
        res.end();
      } catch (err) {
        console.error('fixture server request failed:', err);
        res.writeHead(500);
        res.end('fixture server error');
      }
    })();
  });

  await new Promise<void>((resolvePromise) => {
    server.listen(FIXTURE_SERVER_PORT, '127.0.0.1', resolvePromise);
  });
  console.log(`paigasus-auth E2E fixture server listening on ${FIXTURE_SERVER_ORIGIN}`);
}

main().catch((err: unknown) => {
  console.error(err);
  process.exitCode = 1;
});
