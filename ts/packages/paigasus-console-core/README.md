# @paigasus/console-core

Server-only console composition shared by the Paigasus console zones: the IAM
clients, provisioning and the principal resolver, the authorization
self-queries, scopes, discovery, the error wrapper and the JSON-lines logger.

The root export is server-only. The `./testing` subpath holds in-process fakes
for test harnesses and the `dev:stack` command.

## Redaction

A malformed, empty or whitespace-only `PAIGASUS_SESSION_REDIS_URL` makes the console descriptor
cache throw a plain `Error` with the fixed message
`PAIGASUS_SESSION_REDIS_URL is not a valid Redis URL` (SMA-715). The node-redis parse error is
dropped: its `input` property holds the whole URL, password included. The message gives no detail,
so check the value for these usual causes:

- an empty or whitespace-only value, for example an empty `session-redis-url` Secret key (without
  this check, node-redis would connect to `localhost:6379`);
- a password that holds `@`, `:`, `/`, `?`, `#` or `%` and is not percent-encoded;
- a port above 65535;
- a scheme other than `redis:`, `rediss:` or `unix:`;
- a database path or a `db` parameter that is not a number.

The next request tries to build the cache again.

## Tests

- `moon run paigasus-console-core-ts:test` runs the unit and integration tests.
  It needs no Docker.
- `moon run paigasus-console-core-ts:test-e2e` runs the Docker-backed tier in
  `tests/containers/`. It starts a real Redis with testcontainers and drives
  the descriptor cache's Redis client through idle gaps, a server pause and a
  server-side close (SMA-648).

The `test-e2e` task **needs Docker** and has **no skip hatch**. It fails when
Docker is unreachable. Its inputs include this package's `src/**/*`, so an edit
to any console-core source file now selects a task that needs Docker. It also
selects on `@paigasus/discovery`'s `src/**/*`.
