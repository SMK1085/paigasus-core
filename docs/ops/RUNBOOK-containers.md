# Container images RUNBOOK — `paigasus-iam` / `paigasus-gateway` (SMA-500)

Operator-facing reference for the two service container images: how to build them locally, what
they are named, how they take configuration, the liveness/readiness/startup probe contract, and
the operational rules that follow from how the images are built. This is the contract SMA-513's
Helm chart reads — nothing here may be vague or left implicit.

For the design rationale behind these choices (why a chiseled base over Chainguard/distroless
images, why one parameterized `Dockerfile` instead of two, why publishing is deferred but naming
is not), see
[`docs/superpowers/specs/2026-08-19-sma-500-service-container-images-design.md`](../superpowers/specs/2026-08-19-sma-500-service-container-images-design.md).
This runbook does not repeat it.

---

## 1. Build locally

The supported entry point is `ci/images/run.sh`, not a raw `docker build` — it also asserts the
toolchain/glibc pins (§6) and stamps OCI labels:

```bash
ci/images/run.sh all              # build + smoke-test both images
ci/images/run.sh build            # build both images, no smoke test
ci/images/run.sh build iam        # build only paigasus-iam
ci/images/run.sh build gateway    # build only paigasus-gateway
ci/images/run.sh smoke            # smoke-test whatever images are already built
```

The build context is `rs/` (the Cargo workspace root). This is also what
`.github/workflows/images.yml` runs in CI — `workflow_dispatch`, on every `push` to `main` that
touches `rs/**`, and on pull requests that touch the build inputs (`rs/Cargo.lock`,
`rs/Cargo.toml`, `rs/rust-toolchain.toml`, `rs/Dockerfile`, `rs/.dockerignore`,
`ci/images/**`). **The workflow is not a required check**, so a broken image build reds `main`
after merge rather than blocking the PR that broke it.

That said, a PR touching any of the filtered inputs above — including `rs/Dockerfile` — already
triggers the workflow automatically via its `pull_request` path filter; no manual step is needed
there. Run `gh workflow run images.yml --ref <branch>` instead on a PR that touches `rs/**` but
**none** of those filtered inputs (a plain service code change, say) — that is the one case the
narrower `pull_request` filter does not cover, and it can still break an image build. (This 404s
until `images.yml` itself exists on `main`.)

## 2. Image names

```text
ghcr.io/smk1085/paigasus-iam:<git-sha>
ghcr.io/smk1085/paigasus-gateway:<git-sha>
```

Publishing to the registry is deferred — `ci/images/run.sh` only builds and smoke-tests, it never
pushes, and no registry credentials are wired into `images.yml`. The names and the `:<git-sha>`
tag convention are fixed now regardless, so SMA-513's Helm chart has a concrete
`image.repository`/`image.tag` to inherit rather than inventing its own image story.

## 3. Runtime configuration

Both services take configuration from runtime environment only. Nothing is baked into the image.
Precedence, lowest to highest:

```text
defaults  <  optional TOML file  <  IAM_* / GATEWAY_* environment variables
```

A double underscore (`__`) maps an environment variable onto nested config, e.g.
`IAM_API_KEYS__PEPPER` sets `api_keys.pepper`. `GATEWAY_*` follows the identical scheme for
`paigasus-gateway`.

Mounting an `iam.toml` / `gateway.toml` into the container still works and still layers beneath
the environment — that is figment's ordinary merge behaviour, not a container-specific mechanism,
and the image does not need to change to support it. The image itself ships neither file.

## 4. Probe contract

### Listening ports

| Service | HTTP | gRPC |
| --- | --- | --- |
| `paigasus-iam` | `8080` | `9090` |
| `paigasus-gateway` | `8088` | — |

Both probes below ride the HTTP port. These are config defaults, not fixed values —
`IAM_HTTP_ADDR`, `IAM_GRPC_ADDR`, and `GATEWAY_HTTP_ADDR` each override the **full** `host:port`,
not just the port number. IAM also accepts an optional, separate `[metrics].addr`
(`IAM_METRICS__ADDR`; the gateway has the identical `GATEWAY_METRICS__ADDR`) that moves
`/metrics` onto its own port instead of merging it onto `http_addr` — when unset (the default),
`/metrics` shares the HTTP port with everything else.

| Probe | Endpoint | Notes |
| --- | --- | --- |
| liveness | `GET /healthz` | Never touches a dependency, by construction in both services |
| readiness | `GET /readyz` | IAM pings Postgres; gateway issues a real gRPC introspect to IAM |
| startup | `GET /healthz` | Since SMA-571 IAM binds before it migrates, so this only covers process start: config load, `Database::connect`, and the binds. A migrating replica is *unready*, not absent — `/readyz` carries the distinction |

The image has no shell, so every probe command must use an **absolute path** and the **exec
form** — no `sh -c`, no shell pipelines. Docker's own `HEALTHCHECK` only ever calls `/healthz`
(Docker has no separate readiness concept); `/readyz` is reachable through the same binary for
anyone wiring a Kubernetes `exec` readiness probe:

```bash
/usr/local/bin/paigasus-service healthcheck --path /readyz
```

Exit codes: `0` healthy, `1` unhealthy, `2` usage error (unrecognized arguments — this never falls
through to starting the service).

## 5. Operational rules that are NOT image properties

These follow from how the services behave once containerized, not from anything in the image
build itself — they bite the first operator who deploys without reading this section.

- **IAM serialises its boot migration with a Postgres advisory lock (SMA-559), but that covers
  migrations against *each other* and nothing else.** Two replicas starting together now converge:
  the loser waits `migration.lock_wait_secs` (`IAM_MIGRATION__LOCK_WAIT_SECS`, default 120,
  validated 1–3600), then finds nothing to do. **Two exceptions keep `replicas: 1` /
  `strategy.rollingUpdate.maxSurge: 0` a requirement rather than a recommendation:**
  1. **The release that introduces the lock.** Old replicas still migrate unguarded, so the
     upgrade *to* the locking version is the one rollout the lock cannot protect. Relax only from
     the release after it.
  2. **A migration doing DDL on a table a background maintainer also touches** — the m0008 class.
     An old replica's `PgPartitionMaintainer` holds `AUDIT_PARTITION_LOCK_KEY`, which m0008 waits
     for under a 5s `lock_timeout`; hold it longer and the entire migration transaction aborts,
     even though that replica won the migration lock.

  A long migration also still warrants a maintenance window: the whole run is **one transaction**,
  so m0008-class DDL holds `ACCESS EXCLUSIVE` on `audit_log` for its full duration and **every
  running replica's audit writes block** for that window. Sizing `lock_wait_secs` for a large
  table is simultaneously sizing an audit-write stall.

  **Probe budgets.** A migrating or lock-waiting replica now has its HTTP and gRPC sockets bound
  (the metrics socket too, but only when `metrics.enabled` AND a separate `metrics.addr` are both
  set — otherwise `/metrics` is merged onto the HTTP socket, and with metrics disabled there is no
  third socket at all) and answers `/healthz` 200 within a second of process start, so
  `startupProbe` no longer has to be
  sized against `migration.lock_wait_secs` at all — budget it for config load plus
  `Database::connect`. What a long migration now costs is readiness, not existence: `/readyz`
  answers `503 {"status":"migrating"}` for as long as it takes, and the replica stays out of the
  Service's endpoint list until it flips. **A failing `readinessProbe` never restarts a pod — it
  only removes it from the Service's endpoints** — and liveness is unconditional from process
  start (`/healthz` never depends on migration state), so nothing in this design restarts a
  slow-migrating pod at all. Note there is also nothing to *size* for the migration window: a
  replica that has never been ready is simply absent from the endpoint list until `/readyz`
  first succeeds, however long that takes, and `readinessProbe.failureThreshold ×
  periodSeconds` does not extend or shorten it. That threshold governs only how much
  consecutive failure an *already-ready* replica tolerates before it is withdrawn — size it for
  a database blip in steady state, not for the boot migration.

  `/readyz` has three bodies and they are not interchangeable: `migrating` means the schema is not
  yet applied, `unready` means the schema is there but the database ping failed, `ready` means
  serving. Alert on sustained `migrating`, page on `unready`.

  **App routes answer differently from the probes, deliberately.** While a replica is migrating,
  every path except the probes (`/healthz`, `/readyz`, and `/metrics` when mounted) returns `503`
  with the service's standard error envelope, `{"error":{"code":"service-migrating","message":…}}`,
  plus `paigasus-retryable: true` and the usual correlation headers. That is the same shape every
  other error on `/v1/*` routes takes (SMA-587), so a client can branch on `error.code` without
  special-casing the boot window; `service-migrating` is a registered reason in
  `contracts/proto/paigasus/common/v1/error.proto`. This is deliberately a **catch-all**, not a
  `/v1/*`-scoped fallback: the deferred router has no routing table for app routes yet, so it
  cannot distinguish a real `/v1/*` route from a path that will never exist — `GET /unknown` gets
  the same `503 service-migrating` envelope as `GET /v1/organizations` while the slot is empty.
  Answering `404` for an unrecognized path during this window would assert "this route does not
  exist", which is a stronger and less true claim than "not ready yet"; scoping the fallback to
  `/v1/*` would also duplicate route-prefix knowledge into the boot router that only the real
  router should own. Once `install` swaps in the real router, an unknown path goes back to the
  normal `404` an unmatched route always returns. The probes keep their `{"status":…}` bodies
  because they are not part of the API surface and because `/readyz`'s three values are the
  distinction above. Do not "unify" the two.

  **Chart defaults (handoff to SMA-513).** `strategy.rollingUpdate.maxSurge` need no longer be
  pinned to `0`, subject to the two exceptions above. `startupProbe` no longer needs sizing
  against `IAM_MIGRATION__LOCK_WAIT_SECS` (SMA-571 removed the `start-period` coupling entirely —
  see the probe budgets above), but still expose the env var so a slow migration can be given more
  room. **Precondition to confirm before relaxing `maxSurge`:**
  `AppState::new`'s `reconcile_starter` (`src/adapters/http/mod.rs` around :396) writes system
  policies and roles on every boot with no advisory lock of its own and has never been tested
  under concurrency — pre-existing and out of scope for SMA-559, but SMA-513 should confirm it is
  safe under a surging rollout rather than discover it isn't.

  **Recovering a stranded lock.** A pod SIGKILL'd on a partitioned node leaves its backend holding
  the lock until TCP-level timeouts fire — by default, hours — and every later replica then waits
  and fails to boot. Find it (scoped to this database, since one cluster may host several):

  ```sql
  SELECT pid, granted, query_start
  FROM pg_locks l JOIN pg_stat_activity a USING (pid)
  WHERE l.locktype = 'advisory'
    AND l.database = (SELECT oid FROM pg_database WHERE datname = current_database())
    AND ((l.classid::bigint << 32) | l.objid::bigint) = 5580559;
  ```

  The parentheses are load-bearing — Postgres gives `<<` and `|` equal precedence. Then
  `SELECT pg_terminate_backend(<pid>)`. **This needs privileges the IAM application role usually
  lacks**: `query_start` reads as NULL for other users' backends without `pg_read_all_stats`, and
  `pg_terminate_backend` needs `pg_signal_backend` or superuser — run it as an admin role. The
  stranded backend is *idle in transaction* only once its DDL statement has finished; a pod killed
  **mid-DDL** leaves an `active` backend instead, which `idle_in_transaction_session_timeout` can
  never reap (and `idle_session_timeout` does not apply to either case — setting that one
  aggressively would instead kill a healthy replica between poll attempts).

  What reaps both cases is **TCP keepalives**, because both leave the connection silent on the
  wire: set `tcp_keepalives_idle`, `tcp_keepalives_interval` and `tcp_keepalives_count` to bounded
  values so the server actively probes a client that has vanished and drops the connection when
  the probes go unanswered. `tcp_user_timeout` **complements** these rather than replacing them —
  it bounds how long *transmitted but unacknowledged* data may linger, which on an otherwise
  silent connection means the keepalive probes themselves; with no keepalives configured there is
  nothing for it to act on. Set `idle_in_transaction_session_timeout` as well: it is server-side
  and independent of TCP, so it still covers the post-DDL window if the keepalive settings are
  wrong.

  Behind a transaction-mode pooler the lock is safe by construction — it is acquired and released
  within one transaction — but PgBouncer's `idle_transaction_timeout` can kill a long migration.
- **Gateway `/readyz` issues a real gRPC introspect call to IAM on every poll**, and both of the
  gateway's health routes sit inside its metrics and correlation layers (deliberately — SMA-504
  D10), so probe traffic is metered and load-bearing. Keep `readinessProbe.periodSeconds` at 30s
  or above, and filter `route!~"/healthz|/readyz"` on dashboards. IAM's health routes sit
  **outside** its equivalent layers, so this asymmetry between the two services is real, not an
  oversight to normalize away.
- **A migrating IAM now answers on a live socket rather than refusing the connection** (SMA-571).
  HTTP app routes return `503 {"error":{"code":"service-migrating",…}}` — the standard error
  envelope, so an existing client decoder handles it unchanged — while `/readyz` returns
  `503 {"status":"migrating"}` (see the probe budgets above for why the two differ); gRPC returns a
  well-formed `UNAVAILABLE` (HTTP 200 with
  `grpc-status: 14`), and gRPC health reports `NOT_SERVING`. The gateway needs no change: its
  readiness classification already treats `Unavailable` as not-ready, and its channel is built with
  `connect_lazy`, so a dead IAM has always surfaced as `Rpc(Status::Unavailable)` rather than a
  connect error. One caveat for a future topology: if IAM is ever fronted by a headless Service with
  client-side load balancing, a subchannel to a migrating replica stays READY and returns per-RPC
  `UNAVAILABLE` instead of being evicted on TRANSIENT_FAILURE — correct, but worth knowing before
  adopting that shape.
- **gRPC health is not equivalent to `/readyz` after startup.** `grpc.health.v1.Health` reports
  `NOT_SERVING` during the migration and `SERVING` once installed, and then stays `SERVING`
  regardless of later database health, while `/readyz` can go 503 `unready` on a failed ping.
  A `grpc_health_probe` readiness probe therefore catches the boot case but not a later database
  outage; use the HTTP probe for readiness. Making gRPC health track `/readyz` is a deferred
  follow-up.
- **Neither service terminates TLS.** Both require a TLS-terminating ingress in front of them.
- **A private-CA identity provider is supported (SMA-558), and the routes are not equivalent.**
  Both services' `reqwest` clients now trust the compiled-in Mozilla roots, the image's own store,
  **and** any bundle you name — unioned, so no route costs you the public roots. Prefer them in
  this order:

  1. **`authn.extra_ca_bundle_path`** (`IAM_AUTHN__EXTRA_CA_BUNDLE_PATH`), and
     `upstream.openai.extra_ca_bundle_path` for the gateway's upstream. **Recommended** — the only
     route that fails loudly at boot when it is wrong, and the only one with an auditable record
     in config. A rotated bundle needs a restart.
  2. **Bind-mount your CA as an additional file into `/etc/ssl/certs/`** (e.g.
     `/etc/ssl/certs/corp-ca.crt`), leaving `ca-certificates.crt` itself untouched. No environment
     variable needed — the platform trust-store reader loads every regular file in that directory
     at boot; it does not require OpenSSL's `c_rehash` hashed-symlink naming. Genuinely additive,
     so this is the natural second choice after the config knob above.
  3. **`SSL_CERT_FILE` / `SSL_CERT_DIR` — last resort.** Setting either short-circuits the
     platform probe: the process then reads *only* the path(s) you name and **ignores the image's
     own store**, so it replaces rather than adds. `SSL_CERT_FILE` alone also loses the directory
     scan entirely. A path that does not exist, or a file that is not PEM, is silently ignored —
     no boot error, no request error against public hosts, and a still-broken private IdP.

  **Put roots in the bundle, never intermediates.** Every certificate in it becomes an
  unconstrained trust anchor for every request the client that loaded it makes, to any host it
  reaches — TLS performs no `cA` check on an anchor, so an intermediate is silently promoted to a
  root. The bundle is scoped to one client, not the whole process: IAM's is the JWKS fetcher's and
  the gateway's is the OpenAI egress client's, while the gRPC, NATS and Redis links each build
  their own TLS config and never consult it.

  **Reading a CA-bundle boot failure.** All four failure modes name the config key. A bundle whose
  PEM decodes as base64 but is not valid DER is the subtle one — it passes the PEM parse and fails
  only when the TLS client is built, so boot reports it against the config key rather than against
  the platform trust store. If instead you see *"a control client built WITHOUT ... also failed,
  so the platform trust store contains no parseable certificates"*, the store is the primary
  fault: fix it first, then re-check the bundle, which may also be invalid. The plain *"this can
  also mean the platform trust store contains no parseable certificates"* wording appears only
  when no bundle is configured at all.

  **A self-signed *leaf* works too.** rustls applies no `cA` check to a trust anchor, so an IdP
  presenting a bare self-signed certificate validates once that certificate's own PEM is in the
  bundle — no `accept_invalid_tls` needed. A small private CA is still the tidier posture once
  more than one host is involved (rotation and revocation stay CA-level instead of per-leaf).

## 6. Conventions the console images must follow

Any future console-facing image in this repo should follow the same shape:

- A chiseled or otherwise distroless base with **no shell**.
- A numeric, non-root `USER`.
- A self-contained probe entrypoint (the binary probes itself) plus a `HEALTHCHECK` instruction
  that uses it.
- Runtime environment configuration only — nothing deployment-varying baked into the image.
- Digest-pinned base images, covered by Dependabot's `docker` ecosystem updater.

## 7. What the first Deployment needs

The values below are what SMA-513's chart reads off these two images specifically — spelled out
because "a numeric, non-root `USER`" (§ 6) is a convention, not an operable value.

### Identity and `securityContext`

Both images run as uid:gid **`65532:65532`** — `USER 65532:65532` in `rs/Dockerfile`; the
chiseled rootfs has no `/etc/passwd`, so it cannot be a name (§ 2.6 of the design doc). The
Deployment's pod or container `securityContext` should set:

```yaml
securityContext:
  runAsUser: 65532
  runAsGroup: 65532
  runAsNonRoot: true
```

**Do not** additionally set `readOnlyRootFilesystem: true` on the strength of this runbook. That
posture is **untested** — nothing in SMA-500's smoke suite or elsewhere exercises either image
under a read-only root, and the rootfs does ship a writable `/tmp` (mode `1777`) whose need, if
any, is unverified. Treat it as a follow-up to test, not a default to ship.

### `terminationGracePeriodSeconds`

The image sets `STOPSIGNAL SIGTERM`, and both services install a SIGTERM handler: IAM drains a
`JoinSet` of relays and maintainers before exiting. Set `terminationGracePeriodSeconds`
generously enough to cover that drain. Neither the image nor this issue measures a worst-case
drain time, so treat Kubernetes' 30s default as a floor to widen from, not a value already
validated against IAM's actual outbox relay workload — and revisit it if a rolling update is
observed truncating a drain.

### Which libc is in the image I am running?

`ci/images/run.sh build` produces (and `.github/workflows/images.yml` uploads as a CI artifact,
`chisel-manifests`, 90-day retention) a `chisel-manifest-<service>.txt` per service — e.g.
`chisel-manifest-iam.txt` — listing the exact `chisel cut` package versions, including the
resolved `libc6`, that build actually resolved. This is the answerable half of a real
limitation: `chisel cut` resolves against the **live** Ubuntu archive (§ 2.6 of the design doc),
so two builds a month apart produce different, patched base layers — the image is **not**
bit-reproducible from `rs/Dockerfile` alone. The manifest artifact for the specific build that
produced a given `:<git-sha>` tag is the only record of which packages actually shipped in it;
without it, "which libc is in the image I am running" has no answer after the fact.
