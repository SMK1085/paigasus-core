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
ci/images/run.sh smoke [iam|gateway]...  # smoke-test images built at this HEAD
```

No CI job runs `build` or `all` any more. `.github/workflows/images.yml` runs `build-oci`,
`load-oci`, `smoke` and `rehearse` instead (see "Release tooling" below). So `build` and `all` can
break without turning CI red. Run them yourself before you rely on them.

The build context is `rs/` (the Cargo workspace root). `.github/workflows/images.yml` runs on
`workflow_dispatch`, on every `push` to `main` that touches `rs/**` or `ts/**`, and on pull
requests that touch the build inputs (`rs/Cargo.lock`, `rs/Cargo.toml`, `rs/rust-toolchain.toml`,
`rs/Dockerfile`, `rs/.dockerignore`, `ts/Dockerfile`, `ts/.dockerignore`, `ts/pnpm-lock.yaml`,
`ts/pnpm-workspace.yaml`, `ts/package.json`, `ts/.npmrc`, `ts/apps/*/lib/config.ts`,
`ts/apps/*/next.config.ts`, `ts/apps/*/package.json`, `ts/packages/paigasus-kernel/package.json`,
`rs/crates/bindings/paigasus-node-bindings/index.js`,
`rs/crates/bindings/paigasus-node-bindings/index.d.ts`,
`ci/images/**`,
`.github/workflows/images.yml`, `.prototools`, `.proto/plugins/crane.toml`,
`.proto/plugins/syft.toml`). **The workflow is not a required check**, so a broken image build
reds `main` after merge rather than blocking the PR that broke it.

The `ts/` entries on the `pull_request` filter follow one rule. A file is on the filter when a
change to it can break the image build but not the ordinary build. `moon ci` already runs
`next build` for each affected console, so a source fault or a `tsconfig` fault fails the PR
there. The listed files are the ones that only the Docker path reads, or that it reads
differently. For example, the Docker install uses `--filter "@paigasus/${APP}..."`, so it depends
on the package `name` in each app's `package.json`. `ts/pnpm-lock.yaml` keys its importers by
path, not by name, so a rename does not change the lockfile.

The rule also excludes a file that is already a Moon task `input`, even when `ts/Dockerfile` reads
it too — a bad edit there already reds the ordinary `moon ci` build, so the image workflow adds no
new coverage. The five committed wasm artifacts and `rs/crates/bindings/paigasus-wasm/package.json`
are `inputs` of the console `build`/`test` tasks (`ts/apps/*/moon.yml`,
`ts/packages/paigasus-console-core/moon.yml`) for this reason, and stay off the filter.
`rs/crates/bindings/paigasus-node-bindings/package.json` is an `input` of
`paigasus-kernel-ts:build`/`:test` for the same reason. Its two siblings, `index.js` and
`index.d.ts`, are NOT inputs anywhere — the kernel build task's own `napi build` step regenerates
them fresh every run, so their committed content can drift without reding the ordinary build — and
`ts/Dockerfile` copies both by name, so they are on the filter.

That said, a PR touching any of the filtered inputs above — including `rs/Dockerfile` or
`ts/Dockerfile` — already triggers the workflow automatically via its `pull_request` path filter;
no manual step is needed there. Run `gh workflow run images.yml --ref <branch>` instead on a PR
that touches `rs/**` or `ts/**` but **none** of those filtered inputs (a plain service or console
code change, say) — those are the two cases the narrower `pull_request` filter does not cover, and
either one can still break an image build. (This 404s until `images.yml` itself exists on
`main`.)

## 2. Image names

```text
ghcr.io/smk1085/paigasus-iam:<git-sha>
ghcr.io/smk1085/paigasus-gateway:<git-sha>
```

Publishing to a real registry is deferred — `ci/images/run.sh` builds, smoke-tests, and (through
its `rehearse` command) pushes only to two throwaway local `registry:2` containers. It pushes to
no real registry, and no registry credentials are wired into `images.yml`. The names and the
`:<git-sha>` tag convention are fixed now regardless, so SMA-513's Helm chart has a concrete
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

  **Reading a CA-bundle boot failure.** Every failure attributed to a bundle names the config key —
  the no-bundle case below is the one that does not, because there is no key to name. A bundle whose
  PEM decodes as base64 but is not valid DER is the subtle one — it passes the PEM parse and fails
  only when the TLS client is built, so boot reports it against the config key rather than against
  the platform trust store. If the message instead says the platform trust store contains no
  parseable certificates **and tells you to "fix that first"**, the store is the primary fault:
  repair it, then re-check the bundle, which may also be invalid. The two services word that case
  differently — only the "fix that first" instruction is common to both, so match on that rather
  than on a full sentence. The plain *"this can also mean the platform trust store contains no
  parseable certificates"* wording, carrying no "fix that first", appears only when no bundle is
  configured at all.

  **A self-signed *leaf* works too.** rustls applies no `cA` check to a trust anchor, so an IdP
  presenting a bare self-signed certificate validates once that certificate's own PEM is in the
  bundle — no `accept_invalid_tls` needed. A small private CA is still the tidier posture once
  more than one host is involved (rotation and revocation stay CA-level instead of per-leaf).

## 6. Conventions the console images follow

`ts/Dockerfile` builds one image for both console zones. The `APP` build arg selects the zone. The
image has the same shape as the Rust service images:

- **The runtime stage uses a distroless base.** The base is
  `gcr.io/distroless/nodejs24-debian12:nonroot`, pinned by digest on the runtime `FROM` line of
  `ts/Dockerfile`. That line is the only record of the digest; this runbook does not copy it. The
  base has **no shell**. `smoke_consoles` in `ci/images/run.sh` checks the running container for
  a shell, not only the pin.
- **The builder stage is digest-pinned too.** The builder `FROM` line in `ts/Dockerfile` names a
  `node:X.Y.Z-bookworm` tag and its multi-platform index digest. The index digest resolves on
  both the amd64 and the arm64 leg of `images.yml`. Code that the builder compiles goes into
  `/app`, so this pin controls what the image runs. `assert_console_pins` in `ci/images/run.sh`
  fails if either `FROM` line has no `@sha256:` digest, or if the runtime tag is not `nonroot`.
- **Every image that the build reads is one of the two digest-pinned `FROM` images.**
  `assert_console_pins` reads `ts/Dockerfile` after it drops comment lines and joins continuation
  lines. It fails if the file has a number of `FROM` instructions that is not 2. It also fails if
  a `FROM` line does not match one of the two pin patterns. It fails if a `--from=` value, or a
  `from=` value in a `RUN --mount=` argument, is not `builder` or `bindings`. A `--from=<image>`
  pulls an image that no `FROM` line pins. It also fails if the file starts with a parser
  directive (`# syntax=`, `# escape=` or `# check=`, in any letter case). A `# syntax=` directive
  pulls an unpinned BuildKit frontend image. An `# escape=` directive changes how the check reads
  the file. A heredoc body line that starts with `from` counts as a `FROM` line, so it gives a
  false failure.
- **The image runs as uid:gid `65532:65532`** (`USER 65532:65532`). The Rust service images use
  the same uid (`rs/Dockerfile`'s `USER 65532:65532`). So one Kubernetes `securityContext`
  (`runAsNonRoot: true`, `runAsUser: 65532`) covers all four images. `smoke_consoles` reads the
  uid of the running container with `docker top`. It fails if the uid is not `65532`.
- **The image holds two fixed-path `.mjs` files**, `/app/entrypoint.mjs` and
  `/app/healthcheck.mjs`. The builder writes them. `ENTRYPOINT` and `HEALTHCHECK` name them
  literally. This is necessary because exec-form `ENTRYPOINT` and `HEALTHCHECK` do **not** expand
  `ARG` or `ENV`, so neither instruction can use `${APP}`. The files are `.mjs`, not `.js`, because
  every console `package.json` sets `"type": "module"`. Next copies that `package.json` into the
  standalone tree, so a CJS `require()` shim would need `require(esm)` interop and would fail.
- **The healthcheck file fetches `<BASE_PATH>/healthz` on `127.0.0.1:$PORT`, with a 2500 ms
  signal.** The `fetch` call passes `signal: AbortSignal.timeout(2500)`. Docker kills the probe
  after `--timeout=3s`. Without the signal, a server that accepts the connection and never answers
  holds the probe until Docker kills it. `assert_console_pins` fails if the `printf` line that
  writes the file does not hold exactly one `AbortSignal.timeout(<ms>)`. It also fails if that
  value is not less than the `HEALTHCHECK --timeout`. It reads only a `--timeout=<N>s` value in
  whole seconds.
- **`smoke_consoles` runs the healthcheck file in the running container, with a deadline.** It
  uses `docker exec` and the image's own node, under `with_deadline` with `CONSOLE_HC_DEADLINE`
  (20 s). It fails if the file exits with a code that is not 0. It reports a timeout only when the
  exit code is 143 or 137 and the elapsed time reached the deadline.
- **`assert_console_pins` holds three values equal to the pins in `.prototools`.** The three
  values are the Node major of the runtime base, the exact Node version of the builder, and the
  pnpm version of the builder. `.prototools` is the only record of those versions; this runbook
  does not copy them. The runtime base can hold only the major, because distroless publishes no
  patch-level tags.
- **`smoke_consoles` prints the Node version that the runtime image runs.** The runtime base pins
  only the Node major, so no file records the full version. A different major, a version that the
  row cannot parse, or a missing `node` pin in `.prototools` is an error. A different minor or
  patch version is a `::warning::`, and the row stays green. On 2026-09-25 the runtime image ran
  Node 24.14.0 and `.prototools` pinned 24.16.0. When the runtime is older, a later digest refresh
  of the runtime base closes the gap. When the runtime is newer, change `.prototools` and the
  builder `FROM` line together.
- **The image uses runtime configuration only.** The image bakes no `PAIGASUS_*` environment
  variable. `assert_console_pins` reads `ENV` and `ARG` instructions in `ts/Dockerfile` to enforce
  this. It joins continuation lines first and matches `ENV` and `ARG` in any letter case. After
  that check, no other line of `ts/Dockerfile` can hold the text `PAIGASUS_`. A `RUN`, `COPY`,
  `ADD` or `ONBUILD` step and a heredoc body line are errors. The check does not read comment
  lines. The text check needs the literal text `PAIGASUS_`, so a name that a step builds from
  parts, or a name from `--build-arg`, gets past it. There is one exception to the rule:
  `PAIGASUS_COMPILED_*` (`PAIGASUS_COMPILED_ZONE`, `PAIGASUS_COMPILED_BASE_PATH`).
  `createNextConfig` in `ts/packages/paigasus-next-config` writes these at build time on purpose.
  They record the zone that the artifact was built for, so `runtime.ts` can compare them with the
  `PAIGASUS_ZONE` that a deployment supplies. They are not deployment-varying configuration.
  `ts/Dockerfile` never writes them, so the text check has no exemption for them.
- **`smoke_consoles` reads the built image for baked configuration.** It fails if `Config.Env` of
  the image holds a `PAIGASUS_*` key. The error names the key and never the value. It also walks
  `/app` with the image's own node, and it fails if a file whose name starts with `.env` is there
  outside `node_modules`. The walk must count at least 100 files, or it read the wrong tree. It
  does not see a `PAIGASUS_*` value in a file with a different name, or an `.env*` file under
  `node_modules`. It reports the `.env` scan as not checked if the grep that filters
  `node_modules` paths exits above 1.
- **The build context excludes `.env` files.** `ts/.dockerignore` excludes `**/.env` and
  `**/.env.*`. This is necessary because Next copies an app's `.env` and `.env.production` into
  `.next/standalone`, and the Next server loads them at runtime. Without the exclusion, a local
  `build-console` on a tree that holds real values ships those values in the image. CI is not
  affected, because `.gitignore` ignores `.env*` and CI builds from a clean checkout.
- **Every `pnpm install` and `pnpm i` in `ts/Dockerfile` uses `--frozen-lockfile`.**
  `assert_console_pins` finds each `pnpm` invocation and splits it into words. It treats the
  invocation as an install when a word is `install`, `i`, `install-test` or `it`. So
  `pnpm --filter x install`, `pnpm -C ts i` and `sh -c "pnpm install"` are installs, and
  `pnpm info` and `pnpm exec` are not. It fails if one install has no bare `--frozen-lockfile`
  flag. It also fails on any `--frozen-lockfile=<value>` form and on `--no-frozen-lockfile`. It
  does not check `pnpm add`, `pnpm update` or `npm install`.
- **Dependabot updates the two pinned images of the `/ts` docker block as follows.** This is
  read from the source of `dependabot-core`, not from the published GitHub documentation, which
  does not describe it. A change in `dependabot-core` can change it.
  - **The runtime base:** the tag `nonroot` holds no version, so Dependabot does not propose a
    different tag for it. It refreshes only the digest. The Node major is part of the image name
    (`nodejs24-debian12`), so a Node major bump is a different image, and Dependabot does not
    propose it. The `semver-major` ignore on this entry thus does not apply today. It stays as a
    guard in case the tag changes to one that holds a version.
  - **The builder, `node`:** the `ignore` list blocks all three version update types (major, minor
    and patch), because `assert_console_pins` holds the exact builder version to `.prototools`.
    The builder is digest-pinned. Dependabot-core is expected to refresh its digest on the same
    tag, and the same refresh is expected for `/rs`. But `dependabot-core` has an experiment,
    `docker_digest_only_update_suppression`, that can stop a digest-only refresh on a versioned
    tag. This applies to both the `/ts` builder tag and the `/rs` tag. It is not known if that
    experiment is on for this repository.
  - A version bump of either image is a manual change. It changes `ts/Dockerfile` and
    `.prototools` together.
- **The standalone output has no static assets.** Next writes no `.next/static` and no `public/`
  into `.next/standalone`. The builder stage of the console image copies both, the same as the
  `build` task in `ts/apps/<app>/moon.yml`. An image without that copy answers 200 on
  `<basePath>/healthz` and 404 on every chunk, so a probe-based smoke test does not see the fault.
  So `smoke_consoles` in `ci/images/run.sh` checks for a **served chunk**. A staged-tree parity
  check keeps the two staging sites in agreement.
- **The zone row of `smoke_consoles` proves only that a basePath is in effect.** It checks that
  the zone's chunk returns 404 under the other zone's prefix. The chunk also returns 404 under an
  unknown prefix and under no prefix (measured on `iam-console:dev`). So the row alone does not
  prove that the assets of the two zones do not collide. Acceptance criterion 3 of SMA-513 has two
  halves. The container smoke test (steps 2 to 4 of `smoke_consoles`) proves the per-image half
  (SMA-513 spec D4): each zone emits its asset URLs under its own basePath and serves them there.
  Kind row R2 (`ts/apps/iam-console/tests/cluster/phase-a/cross-zone.spec.ts`) proves the
  one-origin half: both zones hydrate through one Traefik ingress.

Two more facts about the staged-tree parity check are important:

- **The check runs locally only.** It compares the staged `.next/static` of the image with a host
  build at `ts/apps/<app>/.next/standalone/apps/<app>/.next/static`. It runs only when that host
  build exists. The `images` job in `.github/workflows/images.yml` (`ci/images/run.sh
  all-consoles`) does not make a host build. So in CI the check always takes its "not checked"
  path and gates nothing. Use it as a local aid, not as a CI guarantee. A green CI `all-consoles`
  run does not give parity coverage; run `moon run <app>-ts:build` locally before you use it.
- **Parity depends on an assumption: the host build and the image build give the same chunk
  names.** This was true in every measurement, but nothing makes sure of it. Four things can make
  it false. The first is a Next or Turbopack bump that changes how chunk names are hashed. The
  second is a compile-time variable that is different between the two builds. The third is the
  platform difference between a developer's machine and the builder image. The fourth is that the
  filtered `pnpm install` resolves a different optional platform dependency than a full install.
  When the assumption fails, it fails on every run. The failure goes to the parity error that
  already states that the mismatch is not a drift between `ts/Dockerfile` and `moon.yml`.

`ci/images/console-selftest.sh` proves the checks of this section. For each check it applies one
mutation and requires the check's own error text, and the unchanged files must stay green. It runs
the rendered healthcheck file in the pinned runtime image against a server that never answers:
with the signal the file exits 1 with a `TimeoutError`, and without it the file hangs until a
watchdog kills it. `images.yml` runs the self-test directly before the console build. Run it
locally with `/bin/bash ci/images/console-selftest.sh`. `ci/kind/run.sh` reports every
`build-console` failure as an infrastructure error (rc 2), so a failure of a static check in
`assert_console_pins` shows there as rc 2, not as a test failure.

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

`ci/images/run.sh build` makes a `chisel-manifest-<service>.txt` file for each service, for
example `chisel-manifest-iam.txt`. The file lists the exact `chisel cut` package versions the
build used, including the resolved `libc6`. This file stays local: `images.yml` does not run
`build`, so CI uploads nothing from it.

`ci/images/run.sh build-oci` writes a different file name: `chisel-manifest-<service>-<arch>.txt`.
It adds the architecture, because a per-arch build can run on two different runners in the same
workflow. `.github/workflows/images.yml` runs `build-oci` and uploads these files as a CI
artifact named `chisel-manifests-<arch>`, with 90-day retention.

This is the answerable half of a real limit. `chisel cut` uses the **live** Ubuntu archive (see
§ 2.6 of the design document). So two builds one month apart produce different, patched base
layers. The image is **not** bit-reproducible from `rs/Dockerfile` alone. The manifest artifact
for one build is the only record of which packages that build used. Without it, you cannot answer
"which libc is in the image I am running" after the fact.

## Release tooling (SMA-658, PR 1)

### Tools

The release tooling adds three CLIs: crane 0.22.1, cosign 3.1.3, and syft 1.52.0. Each tool is
pinned in `.prototools`. Each tool uses a vendored plugin file in `.proto/plugins/`. Install one
tool with `proto install <tool>`, for example `proto install crane`. `ci.yml`'s bare
`proto install` step now installs all three tools too.

### New `ci/images/run.sh` commands

```bash
ci/images/run.sh build-oci <iam|gateway> <outdir>   # SMA-658: OCI archive, no --load
ci/images/run.sh load-oci <archive> <image-name>     # load + identity check (prints M3)
ci/images/run.sh smoke [iam|gateway]...              # smoke-test images built at this HEAD
ci/images/run.sh rehearse <archive.oci.tar>...       # SMA-658: publish steps vs two local registries
```

`load-oci` prints a line that starts with `M3` followed by a space. This line records the loaded image's ID, the
runner's Docker version, and the runner's image store. The image store decides which ID is
correct: a containerd store and the classic store report different IDs for the same archive.
Read the expected ID against the store named on the same line.

`load-oci` does not call `docker load` on the archive. `docker load` of an OCI-layout archive
needs the containerd image store to read it. GitHub-hosted runners use the classic store, so
`docker load` fails there with `open .../blobs/json: no such file or directory`. It only works on
a Mac because Docker Desktop uses the containerd store. So `load-oci` starts a throwaway local
registry, pushes the archive to it with `crane push`, then pulls the image back by digest and
tags it. This works on both image stores.

`build-oci` runs `docker buildx build`, not plain `docker build`. The OCI exporter needs the
`docker-container` buildx driver. GitHub-hosted runners do not carry a containerd image store, and
a plain `docker build` there does not reliably use the builder that `docker/setup-buildx-action`
selects. `docker buildx build` always uses that builder. This makes no difference on a Mac with
Docker Desktop's containerd store: Docker Desktop already routes `docker build` the same way.

### The rehearsal workflow

Before the first rehearsal run, set up the `images-rehearsal` environment once. In the repository
settings, make an environment named `images-rehearsal`. Give it a `main`-only deployment rule and
no reviewers. Store a non-empty dummy secret named `REHEARSAL_ENV_PROBE` in that environment. The
secret value can be any non-empty string. The rehearsal only checks that the secret reached the
job.

Run the rehearsal once after PR 1 merges. Run it again after any change to the attest or sign
steps.

```bash
gh workflow run images-rehearsal.yml --ref main
```

The `publish` job first prints `M9 environment secret reached the job (length N)`. This line
proves the environment secret reached the job. The job runs under a main-only, no-reviewer
environment. PR 2's real release job will use the same environment shape.

A passing run ends with `REHEARSAL OK: <image>@<digest>`. This line proves the push, the
build-provenance attestation, the SBOM attestation, the cosign signature, and both verify commands
worked against a real registry.

`REHEARSAL OK` does NOT prove the Docker Hub login, the Docker Hub copy, or the git tag. PR 1 adds
none of those steps. PR 2's first real release is their first test.

### The scratch package

The rehearsal pushes to `ghcr.io/smk1085/paigasus-rehearsal`. GitHub makes this package private
after its first push. It holds only rehearsal images. Delete old versions in the package settings
when you do not need them.

## Release a service image

A maintainer sets a service version by hand. release-plz does not process these crates, because
their Cargo manifests set `publish = false` (SMA-658, spec § 3.1).

Before you release a service for the first time, create both registry repositories:
`ghcr.io/smk1085/paigasus-<svc>` and `docker.io/smaschek/paigasus-<svc>`. Set the Docker Hub
repository to public. The `decide` step logs in first, then reads Docker Hub. A missing or private
repository sends a 401 answer. The step treats a 401 answer as a fatal error, not as "no image
yet".

1. Open a pull request with the title `chore(rs): release paigasus-<svc> v<version>`. In it:
   - set `version` in `rs/crates/services/paigasus-<svc>/Cargo.toml`;
   - run `cargo update --manifest-path rs/Cargo.toml -p paigasus-<svc> --offline`;
   - add a `## [<version>] - <date>` section to that crate's `CHANGELOG.md`.
   `repo:actionlint` check 11 fails the pull request when the changelog section is missing.
2. Merge it. The `plan` job selects the service, because its version has no tag.
3. Approve the `approve-images-<svc>` job. Each service chain has its own approval job, but all
   three approval jobs (`approve-release`, `approve-images-iam`, `approve-images-gateway`) use the
   same `release-approval` environment. GitHub approves a pending deployment by environment, not by
   job. So one approval releases every chain that waits for approval in the same run. This was
   measured on the first live release (run 35648073131): one approval released both image
   chains. To release
   only one chain, keep only that chain pending: put only that service's version bump in the
   release, and do not combine it with a kernel release you want to hold back. The security floor
   holds either way: a human must approve before any step that publishes or tags an image runs.
4. The `publish-images-<svc>` job pushes to GHCR, copies the index to Docker Hub, signs both,
   moves `:<major>.<minor>` and `:latest` only forward, and verifies the result. A release still in
   `0.x` does not move `:<major>`. That tag starts once the service reaches `1.0.0` or later. The
   `tag-<svc>` job then makes `paigasus-<svc>-v<version>`.
5. After the first push of a new package, set the GHCR package to public and link it to the
   repository. GitHub makes every new package private.

### Verify a published image

Replace `<svc>` with `iam` or `gateway`, and `<digest>` with the digest from the release job log.

Verify the GHCR image:

```bash
cosign verify \
  --certificate-identity "https://github.com/SMK1085/paigasus-core/.github/workflows/release.yml@refs/heads/main" \
  --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
  ghcr.io/smk1085/paigasus-<svc>@<digest>

gh attestation verify "oci://ghcr.io/smk1085/paigasus-<svc>@<digest>" \
  --repo SMK1085/paigasus-core \
  --signer-workflow SMK1085/paigasus-core/.github/workflows/release.yml \
  --source-ref refs/heads/main
```

Verify the Docker Hub image, with the same identity and the same issuer:

```bash
cosign verify \
  --certificate-identity "https://github.com/SMK1085/paigasus-core/.github/workflows/release.yml@refs/heads/main" \
  --certificate-oidc-issuer "https://token.actions.githubusercontent.com" \
  docker.io/smaschek/paigasus-<svc>@<digest>

gh attestation verify "oci://docker.io/smaschek/paigasus-<svc>@<digest>" \
  --repo SMK1085/paigasus-core \
  --signer-workflow SMK1085/paigasus-core/.github/workflows/release.yml \
  --source-ref refs/heads/main
```

Both registries hold the same index digest. GHCR stores three attestations and a cosign signature.
The build-provenance attestation has the INDEX digest as its subject, which is the digest the two
commands above use. The two SBOM attestations have a PER-PLATFORM digest as their subject, one for
amd64 and one for arm64, because each architecture has its own SBOM. So an SBOM query against the
index digest finds nothing. Get the two per-platform digests from the index itself:

```bash
crane manifest ghcr.io/smk1085/paigasus-<svc>@<digest> \
  | jq -r '.manifests[] | "\(.platform.architecture) \(.digest)"'
```

Docker Hub stores only a cosign signature. The copy step does not copy registry referrers, so
Docker Hub never receives any of the three attestations. `gh attestation verify` still works for a
Docker Hub image, because it queries the GitHub API by digest, not the registry. Do not use
`cosign download attestation` against a Docker Hub image. That command reads registry referrers,
and Docker Hub carries none.

### What a re-run does

The first digest published under `:<version>` is final. A later run reads that digest, discards
its own build and continues with the published one, so a re-run can never replace a released
image. A rebuild never reproduces a digest: the chisel cut resolves the live Ubuntu archive on
every build.

### If the release plan itself cannot be read

`ci/release-plan/run.sh`'s fail-safe branch writes `skip_iam=false` and `skip_gateway=false` (S12,
so both chains RUN — the fail-safe direction), and it writes `version_iam` and `version_gateway` as
explicit EMPTY strings, not as outputs left unwritten. Every chain job that got past its gate then
hard-fails at the label compare (`the archive carries version , but plan says .`), because `plan`'s
version output is empty. Read that specific failure as "the release plan could not be read" — check
the `plan` job's own log — not as a build problem in `images-build-<svc>`.
