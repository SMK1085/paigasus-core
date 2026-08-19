# SMA-500 — Container images for `paigasus-iam` and `paigasus-gateway`

**Status:** draft for adversarial review (2026-08-19)
**Linear:** [SMA-500](https://linear.app/smaschek/issue/SMA-500/ops-dockerfiles-and-container-build-for-paigasus-iam-and-paigasus)
**Design input:** Frontend Architecture Scoping § 7.1 (the red-callout prerequisite) and decision F8
**Blocks:** SMA-513 (multi-zone ingress + Helm chart for the console zones)

## 1. Problem

There are no Dockerfiles anywhere in this repo, yet self-hosted OCI containers are the stated
deployment target for the entire platform (Frontend Architecture Scoping decision F8, "no
build-time config anywhere"). § 7.1 of that document raises it as a red callout: *"Nothing in the
repo is containerized. 'Self-hosted containers' is a stated target with no implementation behind
it. The console must not invent a bespoke image story; this is a prerequisite workstream covering
all three."*

Two consequences follow. The product cannot be deployed by anyone, and — more urgently for the
roadmap — the console images that SMA-513 needs have no convention to inherit. Whatever shape the
first two images take *is* the convention, so it is worth getting right once.

This spec covers the two Rust services. The console images are explicitly downstream of it.

## 2. Evidence

Everything in this section was measured on 2026-08-19 against the tree at `origin/main`
(`1e6257f`), Docker 29.6.2 / buildx v0.35.0, and chisel v1.4.2.

### 2.1 The dependency tree is pure Rust — no native runtime deps

`rs/Cargo.lock` contains `ring` and `rustls` and **no** `openssl-sys`, `native-tls`, `aws-lc-sys`,
`libz-sys` or `zstd-sys`. The workspace posture is deliberate and documented in `rs/Cargo.toml`:
reqwest is `default-features = false, features = ["rustls-tls", "json"]`, sea-orm and
sea-orm-migration are `runtime-tokio-rustls`, redis is `tokio-rustls-comp` +
`tls-rustls-webpki-roots`, and `jsonwebtoken` selects `rust_crypto` explicitly to avoid a cmake
build dependency.

So the runtime image needs a C runtime and a CA bundle, and nothing else. It does **not** need
OpenSSL, and there is no native library whose absence would only surface under load.

Two crates do compile C at *build* time (`ring`, `blake3`), so the builder stage needs a C
toolchain even though the runtime does not.

### 2.2 The CA bundle is load-bearing, not decorative

`paigasus-gateway` enables `tonic = { features = ["tls-ring", "tls-native-roots"] }`.
`tls-native-roots` reads the platform trust store at runtime. `paigasus-iam` fetches OIDC
discovery documents and JWKS over HTTPS via reqwest. An image without
`/etc/ssl/certs/ca-certificates.crt` would build fine, start fine, pass `/healthz`, and then fail
every outbound TLS handshake — a failure mode that only appears against a real IdP.

### 2.3 Both services already load config the way containers need

`IamConfig::figment()` and `GatewayConfig::figment()` are, respectively:

```
Figment::from(Serialized::defaults(Defaults::default()))
    .merge(Toml::file("iam.toml"))          // gateway.toml
    .merge(Env::prefixed("IAM_").split("__"))   // GATEWAY_
```

Defaults `<` optional TOML `<` env, with `__` mapping to struct nesting so secrets inject
without a file (`IAM_API_KEYS__PEPPER` -> `api_keys.pepper`, which has its own regression test).
Default bind addresses are already container-correct: IAM `0.0.0.0:8080` (HTTP) and `0.0.0.0:9090`
(gRPC), gateway `0.0.0.0:8088`.

AC-2 therefore requires no new mechanism at all. It requires that the image **not** ship a
`iam.toml`/`gateway.toml` and **not** bake `ENV IAM_*`/`ENV GATEWAY_*` lines.

### 2.4 The two services have materially different boot requirements

`paigasus-iam`'s `main` connects to Postgres and runs `Migrator::up` before serving. It cannot
start without a reachable database. `paigasus-gateway`'s IAM channel is `connect_lazy`, it has no
database, and it starts standalone — but `GatewayConfig::validate()` rejects an empty
`upstream.openai.api_key`, so it needs one env var to boot at all.

This asymmetry decides the shape of the smoke test (§ 5.2) and the k8s probe guidance (§ 4.6).

### 2.5 `/healthz` and `/readyz` already have the right semantics

IAM: `/healthz` returns `200 {"status":"ok"}` unconditionally; `/readyz` executes `SELECT 1`
against the pool and returns `503 {"status":"unready"}` on error. Gateway: `/healthz` is
dependency-free by construction (its test asserts a never-invoked IAM fake); `/readyz` probes IAM
with a deliberately-invalid sentinel token and classifies reachable/unreachable.

Both are mounted outside the tracing/timeout/auth layers, so probe traffic emits no request spans
and counts toward no RED metrics. Nothing about the endpoints needs to change.

### 2.6 Chisel, measured

`chisel find --release ubuntu-24.04` confirms all five slices this design uses exist. Cutting
them into a rootfs yields:

| Property | Measured |
| --- | --- |
| Total rootfs size | **5.3 MB** |
| `/etc/ssl/certs/ca-certificates.crt` | present via `ca-certificates_data`, **121 certificates** |
| `libgcc_s.so.1` | present via `libgcc-s1_libs` |
| `/etc/passwd` | **absent** |
| `/tmp` | present, mode `1777` |

Four traps came out of this, all of which would otherwise have been discovered during
implementation or, worse, at runtime:

1. **`libgcc-s1_libs` is required and easy to miss.** Rust links `libgcc_s.so.1` for panic
   unwinding. Omitting the slice produces a dynamic-loader error at container start, not a build
   error.
2. **`ca-certificates_data` is the right variant.** It ships the concatenated bundle that
   rustls-native-certs actually reads. `ca-certificates_data-with-certs` additionally installs
   ~120 individual PEMs under `/usr/share/ca-certificates/mozilla/` (+0.5 MB) that nothing in this
   stack opens.
3. **No `/etc/passwd` means a named user is impossible.** `USER 65532:65532` must be numeric.
   A `USER nonroot` would fail to resolve.
4. **`chisel cut --root DIR` does not create `DIR`.** It exits
   `error: cannot extract from package "base-files": target directory does not exist`. The
   Dockerfile must `mkdir -p` first.

And one property that is a caveat rather than a trap: `chisel cut` fetches from the **live Ubuntu
archive** (observed: `libc6_2.39-0ubuntu8.8`). Two builds a month apart therefore produce
different — patched — base layers. This is the correct behaviour for security, but it means AC-4
is satisfied as *a repeatable process*, not as bit-identical output. § 7 states this as a
limitation rather than pretending otherwise.

### 2.7 What the repo's gate machinery will and will not demand

- `ci/affected-graph/ci_targets.py` scopes its coverage check to the **`repo` project**. Tasks on
  `paigasus-iam-rs`/`paigasus-gateway-rs` are outside it. A new `repo:*` task, by contrast, must
  appear in `ci.yml`'s `T=(…)` array **and** CLAUDE.md's marker-delimited command, or become the
  first-ever `T_EXEMPT` entry.
- `run_task_case "lockfile->all-lint"` is strict equality over task names. Any new Rust-project
  task keying on `/rs/Cargo.lock` re-baselines that list.
- `cargo_moon_parity.py` asserts `dependsOn` + `^:build` per crate. **No new crate** means no
  change here; adding one would also re-baseline `lockfile->all-lint`.
- `repo:actionlint` (SMA-525/SMA-540) requires `branches:`, `paths:` and their `-ignore` variants
  as **block sequences**; the inline `[main]` form makes the extractor fail all four keys loudly.

Design choices in § 3 are made to touch none of these.

## 3. Decisions

| # | Decision | Choice |
| --- | --- | --- |
| D1 | CI scope | **Build + verify, no publish.** No registry, credentials, tag scheme or retention policy until there is a consumer (SMA-513) |
| D2 | Runtime base | **Ubuntu 24.04 chiseled** — a `scratch` image carrying a 5.3 MB chisel-cut rootfs |
| D3 | Builder | **`rust:1.95.0-bookworm`**, digest-pinned, matching `rust-toolchain.toml` exactly |
| D4 | Probe | A **`healthcheck` subcommand on each service binary**, backed by one shared implementation |
| D5 | Build entry point | **`ci/images/build.sh`** + `ci/images/smoke.sh`, the house `ci/<gate>/run.sh` convention. **No Moon task** |
| D6 | Trigger | Dedicated `images.yml`; **push-to-`main`** on `rs/**`, **PR** on build-inputs only |
| D7 | Config | **Runtime env only.** No config file in the image, no `ENV IAM_*`/`ENV GATEWAY_*` |
| D8 | Architecture | **amd64 verified in CI**; Dockerfiles stay arch-agnostic so an arm64 host builds arm64 |

### D2 — why chiseled Ubuntu over the alternatives

**Alpine** was the first choice and was reversed. It forces `x86_64-unknown-linux-musl`, which
adds a second Rust target to build and cache, and imports musl's `mallocng` — measurably slower
than glibc under exactly the thread contention these services generate (Cedar evaluation, blake3
hashing, JWKS validation, a pooled Postgres). Its one real advantage was that busybox `wget` makes
`HEALTHCHECK` free; D4 removes that advantage at a cost of ~40 lines.

**Chainguard** was evaluated and rejected on tier mechanics, not merit. The free tier is
`:latest`/`:latest-dev` only — versioned tags moved to the paid catalogs in Aug/Nov 2024 — with no
CVE-remediation SLA. In a repo where GitHub Actions are SHA-pinned, rustc is pinned to `1.95.0`,
and every proto CLI is version-pinned, a floating `:latest` would become the single least-pinned
input in the build. Digest-pinning does not rescue it, because free-tier images are not retained
across versions, so a pinned digest can begin 404-ing and red CI on a day nothing changed. The
OSS-maintainer programme could lift this, but it is qualification-based and therefore on someone
else's timeline.

**`scratch` + static musl** is the true floor (~5 MB of OS, and in fact no OS), but re-imports
musl for no gain over chiseled once the shell is gone either way.

Chiseled Ubuntu gives glibc, an Ubuntu LTS CVE feed, a pinnable release identifier, no shell and
no package manager, at 5.3 MB — and the tooling is open source and free with no tier games.

### D4 — why a subcommand, not a separate probe binary

A shell-less image cannot run `curl`. Three options were considered:

- **A separate `paigasus-probe` crate** would be the most explicitly reusable convention, but it
  adds a Rust crate, which re-baselines the `lockfile->all-lint` strict-equality set, and the
  target URL becomes an argument that can drift from the port the service actually binds.
- **No in-image probe** would be defensible — Kubernetes `httpGet` probes are executed by the
  kubelet, so nothing needs to exist inside the container — but it leaves `docker run` and compose
  users with no health signal, and reads as a dodge of AC-3.
- **A subcommand** costs no new crate, no new bytes in the image, and no gate re-baseline, and it
  reads `http_addr` from the *same* figment config the server binds, so the probe target cannot
  drift from reality.

The subcommand wins. The convention the console images inherit is stated as *"every image ships a
self-contained probe entrypoint and a `HEALTHCHECK` that uses it"* — which a Node image satisfies
with `node -e` and does not need this Rust binary for.

## 4. Architecture

### 4.1 Files

```
rs/.dockerignore                                     new
rs/crates/services/paigasus-iam/Dockerfile           new
rs/crates/services/paigasus-gateway/Dockerfile       new
rs/crates/libs/paigasus-observability/src/health.rs  new  (the probe)
rs/crates/services/{paigasus-iam,paigasus-gateway}/src/main.rs   modified (arg branch)
ci/images/build.sh                                   new
ci/images/smoke.sh                                   new
.github/workflows/images.yml                         new
docs/ops/RUNBOOK-containers.md                       new
CLAUDE.md                                            modified (gotchas)
```

Dockerfiles are co-located with their crates, matching the FFI-binding layout precedent. The build
**context is `rs/`** (the cargo workspace root), so `-f rs/crates/services/paigasus-iam/Dockerfile rs/`.

`rs/.dockerignore` must exclude `target/` above all else — without it the context upload is
gigabytes on any developer machine that has ever run `cargo build`.

### 4.2 Dockerfile shape

Three stages, identical between the two services apart from the crate/binary name and ports:

```dockerfile
FROM rust:1.95.0-bookworm@sha256:... AS builder
WORKDIR /src
COPY . .
# The binary is copied OUT of the cache mount inside the same RUN: a cache mount is not
# part of the resulting layer, so a later `COPY --from=builder /src/target/...` would
# find nothing. See the note below.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked -p paigasus-iam --bin paigasus-iam \
 && mkdir -p /out && cp /src/target/release/paigasus-iam /out/paigasus-iam

FROM ubuntu:24.04@sha256:... AS rootfs
ARG CHISEL_VERSION=v1.4.2
# One pinned checksum PER ARCHITECTURE. A single ARG cannot work here: the release
# publishes a distinct tarball and a distinct sha384 per arch, so a lone checksum would
# either break the arm64 build or, if skipped, silently drop the integrity check.
ARG CHISEL_SHA384_amd64=<pin>
ARG CHISEL_SHA384_arm64=<pin>
RUN set -eux; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
      amd64) sha="${CHISEL_SHA384_amd64}" ;; \
      arm64) sha="${CHISEL_SHA384_arm64}" ;; \
      *) echo "no pinned chisel checksum for $arch" >&2; exit 1 ;; \
    esac; \
    apt-get update && apt-get install -y --no-install-recommends ca-certificates curl; \
    curl -sSL -o /tmp/chisel.tar.gz \
      "https://github.com/canonical/chisel/releases/download/${CHISEL_VERSION}/chisel_${CHISEL_VERSION}_linux_${arch}.tar.gz"; \
    echo "${sha}  /tmp/chisel.tar.gz" | sha384sum -c -; \
    tar -xzf /tmp/chisel.tar.gz -C /usr/local/bin; \
    mkdir -p /rootfs; \
    chisel cut --release ubuntu-24.04 --root /rootfs \
      base-files_base base-files_release-info \
      libc6_libs libgcc-s1_libs ca-certificates_data

FROM scratch
COPY --from=rootfs /rootfs /
COPY --from=builder /out/paigasus-iam /usr/local/bin/paigasus-iam
USER 65532:65532
EXPOSE 8080 9090
HEALTHCHECK --interval=30s --timeout=3s --start-period=15s --retries=3 \
  CMD ["/usr/local/bin/paigasus-iam", "healthcheck"]
ENTRYPOINT ["/usr/local/bin/paigasus-iam"]
```

Notes on choices that are not obvious:

- **Full `rust:1.95.0-bookworm`, not `-slim`:** `ring` and `blake3` compile C, so the builder needs
  a C toolchain. Builder size is irrelevant — exactly one file crosses into the final stage.
- **`--locked`:** the image ships the resolution the repo has committed. `lint` and
  `repo:publish-metadata` already take this posture; a Dependabot Cargo PR has previously shipped a
  lockfile resolved from 3 of 11 workspace members.
- **Checksum verification is `sha384sum`**, because that is what Canonical publishes alongside the
  chisel release assets.
- **`USER 65532:65532` is numeric** because the rootfs has no `/etc/passwd` (§ 2.6).
- **`EXPOSE` is documentation only** and does not publish anything.
- **No `WORKDIR` carrying a config file.** With no `iam.toml` present, figment resolves defaults +
  env. A self-hoster who *mounts* a TOML still gets file layering, because that is figment's
  documented behaviour — AC-2 forbids baking config at build time, not supporting file config.

Two caching mechanisms, with different reach, and it is worth being precise about which helps
where. **BuildKit cache mounts** on the cargo registry and `target/` make local rebuilds fast, but
they are *not* persisted by the `type=gha` cache backend — a CI run starts with them empty, so in
CI only the **layer** cache (`type=gha`) applies, and it hits only when the `COPY . .` context is
unchanged. In practice this means CI compiles from cold on almost every run the trigger fires,
which D6's narrow trigger is what makes acceptable.

Cache mounts also carry the trap the Dockerfile comment above marks: because a cache mount is not
part of the layer, the built binary must be copied out of it inside the same `RUN`. A
`COPY --from=builder /src/target/release/...` in a later stage finds an empty directory.

`cargo-chef` is deliberately **not** adopted: it is a real dependency and a real maintenance
surface, and it exists to solve precisely the cold-build cost that D6's trigger already bounds.
§ 7 records this as revisitable if the workflow becomes a bottleneck.

### 4.3 The probe

`paigasus_observability::health` — one implementation, consumed by both services, both of which
already depend on this crate. Placing it here rather than duplicating ~40 lines into two `main.rs`
files is the same single-site discipline that `repo:redis-connect-single-site` and
`repo:iam-docker-policy-single-site` already enforce elsewhere.

```rust
/// Probe `path` on `addr` over HTTP/1.1. `Ok(true)` iff the status is 2xx.
pub fn probe(addr: SocketAddr, path: &str, timeout: Duration) -> io::Result<bool>
```

Standard library only — no new dependency in a crate that both services link. A blocking
`TcpStream::connect_timeout`, a literal `GET {path} HTTP/1.1\r\nHost: …\r\nConnection: close\r\n\r\n`,
and a parse of the status line.

One behaviour is load-bearing and would otherwise be a latent bug: both services default to
`0.0.0.0`, which is an *unspecified* address, not a destination. `probe` maps an unspecified
address to loopback (`127.0.0.1` / `::1`) before connecting.

### 4.4 The subcommand

Each `main.rs` gains an early branch, before logging is initialised and before any listener is
bound:

```rust
if std::env::args().nth(1).as_deref() == Some("healthcheck") {
    let config = IamConfig::load()?;
    let ok = paigasus_observability::health::probe(config.http_addr, "/healthz", Duration::from_secs(2))
        .unwrap_or(false);
    std::process::exit(if ok { 0 } else { 1 });
}
```

`load()` but **not** `validate()`: probe mode needs only `http_addr`, and IAM's validator rejects a
config with no configured issuers, which would make the healthcheck fail for a reason that has
nothing to do with health.

An `Err` from `probe` (connection refused, timeout) is collapsed to *unhealthy*, not propagated.
A probe that panicked or returned a non-0/1 exit code on a refused connection would still be
reported unhealthy by Docker, but the distinction matters for the exit-code contract the runbook
documents: **0 = healthy, 1 = everything else**.

The dispatch itself is factored into a testable pure function rather than being inline argv
matching, so it can be asserted without spawning a process.

### 4.5 Build and smoke scripts

`ci/images/build.sh [iam|gateway|all]` builds with the digest-pinned bases and applies OCI labels
(`org.opencontainers.image.source`, `.revision`, `.licenses`, `.title`, `.description`). Revision is
build-identifying, not deployment-varying, so it does not violate AC-2. It also greps each
Dockerfile for `ENV IAM_`/`ENV GATEWAY_` and fails if one appears — a cheap mechanical guard on D7.

`ci/images/smoke.sh` is where AC-1 and AC-3 are actually proved; § 5.2 describes it.

### 4.6 The probe contract the console must inherit

Documented in `docs/ops/RUNBOOK-containers.md` and consumed by SMA-513:

| Probe | Endpoint | Notes |
| --- | --- | --- |
| liveness | `GET /healthz` | Never touches a dependency, by construction in both services |
| readiness | `GET /readyz` | IAM pings Postgres; gateway probes IAM |
| startup | `GET /healthz` | IAM runs migrations at boot, so a `startupProbe` with a generous failure threshold is required or the kubelet will kill it mid-migration |

Docker has no readiness concept, so the in-image `HEALTHCHECK` maps to `/healthz` only. `/readyz` is
a Kubernetes `readinessProbe` and is exercised by the smoke test from outside the container.

## 5. Verification

### 5.1 Rust

Unit tests for `probe()` against an in-process `TcpListener`: a 200 response, a 503 response, a
connection refused, a timeout, and the `0.0.0.0` -> loopback mapping. Plus a per-service test of the
arg-dispatch function (`healthcheck` recognised; no args and unknown args fall through to normal
boot).

These run under the existing `moon ci :test` graph with no task or gate changes.

### 5.2 The smoke test

Designed so it cannot pass vacuously — the failure this must never have is an image that starts,
answers `/healthz` from a stub, and is declared good.

- **gateway, standalone.** With only `GATEWAY_UPSTREAM__OPENAI__API_KEY` set: `/healthz` -> **200**,
  and `/readyz` -> **503**, because no IAM is reachable. The negative case is the point; a
  `/readyz` returning 200 here would be lying, and asserting only the positive case would not
  catch it.
- **iam, with Postgres.** Against a Postgres container: `/healthz` -> 200 and `/readyz` -> **200**
  once migrations complete. IAM genuinely cannot boot without a database, so this simultaneously
  proves the image runs, migrations execute, and readiness reflects a real dependency.
- **The in-image probe.** `docker inspect --format '{{.State.Health.Status}}'` is polled until
  `healthy`. This is the only assertion that exercises the probe binary *inside* the shell-less
  image; an outside-the-container `curl` would pass even if `HEALTHCHECK` were broken.
- **Runtime-only config (AC-2).** Both containers are started with env vars and nothing else — no
  mounted file, no `--env-file`. Success *is* the proof of D7.
- **Non-root (convention).** `docker inspect --format '{{.Config.User}}'` is asserted to be
  `65532:65532`.

### 5.3 CI

`.github/workflows/images.yml`, modelled on `prebuild.yml`:

- `push` to `main` filtered on `rs/**`, the Dockerfiles, `ci/images/**`, the workflow.
- `pull_request` filtered on the inputs that can actually break an image build: `rs/Cargo.lock`,
  `rs/Cargo.toml`, `rs/rust-toolchain.toml`, the Dockerfiles, `rs/.dockerignore`, `ci/images/**`,
  the workflow itself. This is what catches "a new dependency does not build in the image" before
  merge, without putting a release build on every source-line PR.
- `workflow_dispatch`.
- All `branches:`/`paths:` as **block sequences** (§ 2.7), `permissions: contents: read` only,
  actions SHA-pinned, and ci.yml's reclaim-runner-disk step included.
- Not a required check, exactly as `prebuild.yml` is not.

## 6. Rollout and rollback

Additive. No existing task, gate, workflow trigger or `T` array changes, so `moon ci` behaves
identically before and after. The two `main.rs` edits are the only changes to shipped code, and
they are inert unless argv[1] is exactly `healthcheck`.

Rollback is deleting the new files and reverting the two `main.rs` branches; nothing else depends
on them until SMA-513.

## 7. Limitations

1. **The image build is not a required check.** A broken image build reds `main`, not the PR that
   caused it. This is the `prebuild.yml` trade-off, accepted knowingly to avoid a `--release` build
   on every PR against a 30-minute timeout and a ~14 GB disk that cedar-policy has already
   overflowed once (SMA-444). The PR path filter narrows the window to changes that plausibly
   break it.
2. **Not bit-reproducible over time.** `chisel cut` resolves against the live Ubuntu archive
   (§ 2.6). Rebuilding an old commit produces a newer, patched base.
3. **amd64 only in CI.** The Dockerfiles are arch-agnostic and an arm64 host builds arm64, but no
   arm64 leg is verified. Multi-arch belongs with publishing.
4. **No `cargo-chef` layer caching.** A cold build compiles the whole dependency tree. Acceptable
   given D6's trigger; revisit if the workflow becomes a bottleneck.
5. **The slice list is hand-maintained.** A future dependency that needs a system library will fail
   at container start with a loader error, not at build time. The smoke test catches it in CI, but
   only for code paths the smoke test reaches.
6. **`chisel` is pinned by version and checksum but is a network fetch at build time.** A GitHub
   outage fails the build. Same class as any base-image pull.

## 8. Non-goals

- Registry publishing, image tagging/retention policy, cosign signing, SBOM/provenance attestations.
- Helm charts, Kubernetes manifests, docker-compose for the service stack — SMA-513 owns deployment
  manifests, and the Frontend Architecture Scoping document explicitly lists containerization
  manifests as out of its own scope.
- Console/Next.js images. This spec establishes the conventions they inherit (§ 4.6); it does not
  build them.
- Any change to `/healthz` or `/readyz` semantics. They are already correct (§ 2.5).
- Multi-arch builds and arm64 CI verification.
