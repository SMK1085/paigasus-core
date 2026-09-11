# @paigasus/auth

OIDC relying party, Redis-backed session store, and auth route factory for Paigasus console
zones. Design doc:
[`docs/superpowers/specs/2026-09-09-sma-506-auth-design.md`](../../../docs/superpowers/specs/2026-09-09-sma-506-auth-design.md).
Measurements record:
[`docs/superpowers/specs/2026-09-09-sma-506-measurements.md`](../../../docs/superpowers/specs/2026-09-09-sma-506-measurements.md).

## Why this README exists

`@paigasus/auth` composes its environment shape (`authEnvShape`, `src/config.ts`) into
`@paigasus/next-config`'s `defineRuntimeConfig`. That package's `describeIssues` renders a
guidance message only for `custom` zod issues on **its own** keys (`PAIGASUS_ZONE`,
`PAIGASUS_ZONES`). Every validation failure raised by one of this package's variables therefore
surfaces as a bare `<VARIABLE_NAME>: <zod-error-code>`, with no further explanation.

That is deliberate, not a bug to route around: widening the trusted-message allowlist to cover
this package's own `.refine()` messages would let a package-authored string reach the error path
unfiltered — which is exactly the secret-leak vector that filter exists to close. So the remedy
here is documentation, not a mechanism. The table below is the guidance a misconfigured deployment
does not get from the error itself.

## Environment variables

All variables are read once, at first request, through the composed runtime config. A parse
failure at that point is fail-closed: the process serves no request until every required variable
is present and well-formed.

| Variable                                 | Required                                 | Default                                                                         | Expected form                                                                                                                                                                                                                                                                                                                                                       |
| ---------------------------------------- | ---------------------------------------- | ------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `PAIGASUS_OIDC_ISSUER`                   | yes                                      | —                                                                               | An absolute `https://` URL, no leading/trailing whitespace, no trailing slash.                                                                                                                                                                                                                                                                                      |
| `PAIGASUS_OIDC_CLIENT_ID`                | yes                                      | —                                                                               | A non-empty string.                                                                                                                                                                                                                                                                                                                                                 |
| `PAIGASUS_OIDC_CLIENT_SECRET`            | yes                                      | —                                                                               | A non-empty string. Never optional — see "The client secret is required" below.                                                                                                                                                                                                                                                                                     |
| `PAIGASUS_PUBLIC_ORIGIN`                 | yes                                      | —                                                                               | An absolute `https://` URL, no leading/trailing whitespace, no trailing slash.                                                                                                                                                                                                                                                                                      |
| `PAIGASUS_OIDC_REDIRECT_URI`             | no                                       | derived from `PAIGASUS_PUBLIC_ORIGIN` + the zone's base path + `/auth/callback` | A well-formed absolute URL (any scheme — an override is typically a reverse-proxy address, not this package's concern).                                                                                                                                                                                                                                             |
| `PAIGASUS_OIDC_POST_LOGOUT_REDIRECT_URI` | no                                       | derived from `PAIGASUS_PUBLIC_ORIGIN` + the zone's base path + `/`              | A well-formed absolute URL.                                                                                                                                                                                                                                                                                                                                         |
| `PAIGASUS_OIDC_SCOPES`                   | no                                       | `openid profile email offline_access`                                           | A space-separated scope string. Keep `offline_access` unless the IdP issues a refresh token without it — removing it can make every access-token expiry log the user out (see "Why `offline_access` is a default scope" below).                                                                                                                                     |
| `PAIGASUS_OIDC_CLOCK_TOLERANCE_SECONDS`  | no                                       | `30`                                                                            | A positive integer.                                                                                                                                                                                                                                                                                                                                                 |
| `PAIGASUS_OIDC_HTTP_TIMEOUT_MS`          | no                                       | `3500`                                                                          | A positive integer. Must satisfy `2 * PAIGASUS_OIDC_HTTP_TIMEOUT_MS < PAIGASUS_SESSION_LOCK_TTL_MS` — see that variable.                                                                                                                                                                                                                                            |
| `PAIGASUS_SESSION_STORE`                 | yes                                      | —                                                                               | `redis` or `memory`. There is no third value and no presence-based fallback — a typo fails startup rather than silently downgrading to `memory`.                                                                                                                                                                                                                    |
| `PAIGASUS_SESSION_REDIS_URL`             | when `PAIGASUS_SESSION_STORE` is `redis` | —                                                                               | A node-redis connection string, e.g. `redis://[[user][:password]@]host[:port][/db-number]`. Required at startup when the store is `redis`; the value is never logged (see "Redaction" below).                                                                                                                                                                       |
| `PAIGASUS_SESSION_REDIS_TIMEOUT_MS`      | no                                       | `1000`                                                                          | A positive integer.                                                                                                                                                                                                                                                                                                                                                 |
| `PAIGASUS_SESSION_TTL_SECONDS`           | no                                       | `28800` (8 hours)                                                               | A positive integer. Idle timeout for a session record.                                                                                                                                                                                                                                                                                                              |
| `PAIGASUS_SESSION_ABSOLUTE_TTL_SECONDS`  | no                                       | `86400` (24 hours)                                                              | A positive integer. Hard ceiling on a session's lifetime regardless of activity.                                                                                                                                                                                                                                                                                    |
| `PAIGASUS_SESSION_REFRESH_SKEW_SECONDS`  | no                                       | `30`                                                                            | A positive integer. How early, before actual access-token expiry, a refresh is attempted.                                                                                                                                                                                                                                                                           |
| `PAIGASUS_SESSION_LOCK_TTL_MS`           | no                                       | `10000`                                                                         | A positive integer. Must stay strictly above `2 * PAIGASUS_OIDC_HTTP_TIMEOUT_MS` — a refresh makes two sequential bounded calls (token exchange, then a JWKS fetch) under this lock, and a refresh that outlives its lock is exactly what the single-flight guarantee (AC 2) depends on not happening. The shipped defaults (`3500`, `10000`) already satisfy this. |
| `PAIGASUS_SESSION_LOCK_WAIT_MS`          | no                                       | `3000`                                                                          | A positive integer. How long a concurrent request waits for another request's in-flight refresh before giving up.                                                                                                                                                                                                                                                   |

`createAuthRuntime` also reads two variables this package does **not** own —
`PAIGASUS_ZONE` and `PAIGASUS_ZONES` — from `@paigasus/next-config`'s own shape, to derive
each zone's base path and its redirect URIs. See that package's own README/docs for their form.

### The client secret is required

A BFF (backend-for-frontend) is a confidential OIDC client by definition. Something already
trusted to hold refresh tokens in Redis can hold a client secret; making it optional would only
admit a strictly weaker deployment, for no gain. PKCE is used in addition, always — the client
secret does not replace it.

### Why `offline_access` is a default scope

Without `offline_access`, some identity providers (Keycloak, for example) never issue a refresh
token for the code flow. This package deletes the session and logs the user out the moment the
access token expires if no refresh token is available — which, on some providers, is as often as
every five minutes. Keep `offline_access` in `PAIGASUS_OIDC_SCOPES` unless you have verified your
IdP issues refresh tokens without it.

**`createAuthRuntime` does not check this at startup, and there is no first-login assertion.**
`handleCallback` (`src/http/routes.ts`) writes the session record with `refreshToken` simply
absent when the token response carries none — nothing fails loudly at that point. The actual
failure surfaces later and silently: `resolveSession` (`src/core/single-flight.ts`) deletes the
record the first time it needs to refresh and finds no `refreshToken`, logging
`session.deleted` with `reason: 'no_refresh_token'` and signing the user out. Dropping
`offline_access` against an IdP that then issues no refresh token therefore produces **mass
logouts at first access-token expiry**, not a startup error — every active session on that
deployment signs out together, at whatever interval the IdP's access-token lifetime is.

### The session store: `redis` vs `memory`

`PAIGASUS_SESSION_STORE=memory` is for **local development and single-process tests only.** It
does not satisfy AC 2 (exactly one refresh, system-wide) across processes, and a real deployment
is not single-process:

- Each zone in a multi-zone deployment (`PAIGASUS_ZONES` naming more than one zone) is a
  **separate Next process.** Under `memory`, a user who signs in on zone A is anonymous on zone B
  — the zones share nothing, because there is nothing outside the one process holding the state.
- The single-flight refresh lock (design doc § 8) is also per-process under `memory`, so two
  processes can each start their own refresh for the same session at the same time. AC 2's
  "exactly one refresh" guarantee holds only within a single process.

Because a silently-downgraded production deployment is worse than a loud failure,
**`createAuthRuntime` refuses to start with `PAIGASUS_SESSION_STORE=memory` when `PAIGASUS_ZONES`
declares more than one zone.** Use `redis` for anything beyond a single-process
development or test deployment.

### Redaction

No event this package logs carries a token, a refresh token, an authorization code, the client
secret, the Redis connection string, or a transaction secret. A logged session id (`sid`) is
truncated to 8 characters — enough to correlate a support request, not enough to replay. This
holds even for an unhandled adapter error: this package never logs a caught node-redis or
`openid-client` error object directly (both can embed a URL or a DSN in their own error text) —
only a fixed name and message are extracted for logging.

## Routes

`createAuthRoutes` / `createAuthRoutes(...).handle` (see `@paigasus/auth/server`) serve four
routes under a zone's base path:

| Route                             | Purpose                                                                                                  |
| --------------------------------- | -------------------------------------------------------------------------------------------------------- |
| `<basePath>/auth/login`           | Starts the authorization-code + PKCE flow, redirects to the IdP.                                         |
| `<basePath>/auth/callback`        | The IdP's redirect target. Exchanges the code, creates the session.                                      |
| `<basePath>/auth/logout`          | Deletes the session record first, then redirects to the IdP's end-session endpoint (see "Logout" below). |
| `<basePath>/auth/logout/callback` | The IdP's post-logout redirect target.                                                                   |

Under Next, a zone mounts these routes at `app/auth/[...auth]/route.ts` and exports
`createAuthRouteHandler(runtime)` as `GET` and `POST`. Next removes the basePath from a route
handler's `req.url` and puts the server's bind address in it. So `createAuthRouteHandler` rebuilds
the URL as `PAIGASUS_PUBLIC_ORIGIN` + basePath + path + query before it dispatches. A plain server
that passes the full path (this package's e2e fixture) works too. The callback sends `redirect_uri`
from `runtime.redirectUri`, never from the request URL, so it always equals the value sent to the
IdP's `/authorize`. `/auth/login` resolves the dot segments of a `returnTo` path first. It then
replaces a path under the zone's own `/auth/` routes with the zone root, so a crafted link cannot
loop.

## Cookies

Every cookie this package writes carries the `__Host-` prefix: `__Host-pgs_sid` (the session
cookie, a browser-session cookie with no `Max-Age`) and one `__Host-pgs_txn_<id>` per in-flight
login attempt. `__Host-` forces `Secure`, `Path=/`, and no `Domain` attribute — there is no
configuration knob for the cookie names, and none is planned: all zones share one cookie on one
origin, and a knob would invite exactly one drift.

## Logout revokes, not just clears (AC 3)

`/auth/logout` deletes the session record from the store **before** redirecting to the IdP —
delete-first, not delete-after. A copy of the session cookie captured before logout is therefore
dead immediately: the store lookup that `getSession`/`requireSession` performs on every request
fails, regardless of whether the browser that presents the stolen cookie ever reaches the IdP's
own end-session endpoint.

## Middleware does no authorization (AC 4)

`createAuthMiddleware` (`@paigasus/auth/middleware`) is its own package entry point, structurally
unable to import a session store, a principal resolver, or `openid-client`. It checks only whether
the session cookie is **present** — never whether it is valid. A forged or expired cookie passes
the middleware and is rejected downstream, by the real store lookup inside
`getSession()`/`requireSession()`. This split is what keeps a future edit from turning the
middleware into an authorizer (the CVE-2025-29927 middleware-auth-bypass shape); see
`src/middleware.ts`'s own header and `tests/middleware.test.ts`'s import-graph assertion.

### `publicPaths` must list every route this package serves

The middleware compares `req.nextUrl.pathname`, which Next gives WITHOUT the zone's basePath. So
`publicPaths` and `loginPath` are basePath-relative: `/auth/login`, not `/iam/auth/login`. The
middleware puts the basePath back when it builds the login redirect and its `returnTo` value.

`createAuthMiddleware({ publicPaths, loginPath })` requires `publicPaths` to name every route
`createAuthRoutes` dispatches on: the login, callback, logout, and logout-callback paths. **Do not
hand-copy this list.** Build it with `authRoutePaths()` (`@paigasus/auth/middleware`), which
returns the four basePath-relative paths from the shared route table:

```ts
// proxy.ts
import { authRoutePaths, createAuthMiddleware } from '@paigasus/auth/middleware';

export default createAuthMiddleware({ publicPaths: [...authRoutePaths(), '/'], loginPath: '/auth/login' });
```

`requireSession()` redirects to the basePath-relative `/auth/login` for the same reason: Next's
`redirect()` adds the basePath itself.

If a path is missing — the callback path is the easy one to miss — the failure has no error
anywhere: `/auth/login` clears the session cookie, the identity provider's redirect back to
`/auth/callback` arrives with no cookie, middleware sees a non-public path with no cookie and
bounces it back to `/auth/login`, which clears the cookie again and redirects again. The visitor
loops forever between the two paths. This is the default outcome of copying a middleware example
without reading this section, which is why `authRoutePaths` exists instead of a documentation
comment alone.

## `/client` never sees a token (AC 5)

`@paigasus/auth/client` imports nothing beyond `react` and the `SessionView` type. A client
bundle built from that entry point cannot embed a store, an OIDC client, or a secret — there is no
import path from it to anything that holds one.

## `can()` fails open while grants are deferred (§ 13 of the design doc)

`can()` (`@paigasus/auth/client`) is a **cosmetic** navigation-visibility check, never the
authorization decision — every mutating action is re-checked authoritatively by IAM, server-side,
regardless of what `can()` returns. Until `SMA-508` wires a real grants resolver,
`SessionView.grantsAvailable` is `false` and `can()` returns `true` for every check. This is
deliberate: failing closed here would render a console with no navigation at all, before a
downstream consumer (`@paigasus/app-shell`) ever gets the chance to let IAM answer.

## Not covered by this package

- Metrics/observability beyond structured log events — there is no TypeScript observability crate
  yet (design doc § 12).
- A real grants/memberships snapshot — the session carries an empty, typed slot for it; wiring a
  live resolver is `SMA-508`.
- Mounting these routes and this middleware inside a zone app — this package is tested as a
  library, not inside a real Next application (see `tests/e2e/` for the closest approximation, a
  standalone fixture server).
