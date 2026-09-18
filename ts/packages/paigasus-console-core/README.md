# @paigasus/console-core

Server-only console composition shared by the Paigasus console zones: the IAM
clients, provisioning and the principal resolver, the authorization
self-queries, scopes, discovery, the error wrapper and the JSON-lines logger.

The root export is server-only. The `./testing` subpath holds in-process fakes
for test harnesses and the `dev:stack` command.

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
