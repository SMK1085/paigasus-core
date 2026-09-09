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
IdP issues refresh tokens without it. `createAuthRuntime` also fails loudly on the very first
login if no refresh token comes back, rather than waiting for the first silent logout to reveal
the misconfiguration.

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
