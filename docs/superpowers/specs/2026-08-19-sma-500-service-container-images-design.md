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

### 2.2 The CA bundle is load-bearing — but for two crates only, not four

An earlier draft of this section asserted that IAM's reqwest-based OIDC/JWKS fetches read the image
trust store. **That is wrong**, and the correction matters both for the slice choice and for what
the runbook tells a self-hoster. Per-crate truth, read off `rs/Cargo.toml` and `rs/Cargo.lock`:

| Consumer | Trust source | Reads `/etc/ssl/certs`? |
| --- | --- | --- |
| `tonic` (gateway -> IAM) | `tls-native-roots` -> `rustls-native-certs 0.8.4` | **Yes** |
| `async-nats` (IAM outbox) | native roots, unless `root_ca_bundle` is configured | **Yes** |
| `reqwest` (IAM -> IdP discovery/JWKS) | `rustls-tls` -> `webpki-roots 1.0.8`, compiled in | No |
| `sqlx`/`sea-orm` (Postgres) | `webpki-roots 0.26.11`, compiled in | No |
| `redis` | `tls-rustls-webpki-roots`, compiled in | No |

So the bundle is still required — the gateway would not survive without it — but the failure mode
is the *opposite* of what the earlier draft claimed. `build_channel` calls
`ClientTlsConfig::new().with_native_roots()` eagerly (`adapters/iam/client.rs:175-192`, under
`connect_lazy`, which defers only the TCP handshake), so a missing bundle is a hard **boot**
failure for the gateway, not a late silent one. That is the better failure mode, and the smoke
test's gateway-boot assertion is therefore already a partial proof that the bundle is usable
(§ 5.2).

**The consequence worth recording is one this issue does not fix.** Because reqwest carries
compiled-in Mozilla roots, a self-hoster whose IdP is behind a private or corporate CA **cannot**
make IAM trust it by mounting a CA into the image — `SSL_CERT_FILE` does not apply either. The
only in-code escape today is `authn.accept_invalid_tls`, which `config.rs:118-124` documents as a
full authentication bypass. Containerizing does not cause this, but it is the first time it becomes
an operator-visible constraint, so § 7 records it as a limitation and the runbook states it plainly.
Switching reqwest to `rustls-tls-native-roots` (or adding an `authn.ca_bundle_path`) is a
contained follow-up that belongs in its own issue: it changes the authentication TLS posture, which
is well outside "add Dockerfiles".

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

### 2.5 `/healthz` and `/readyz` are correct — but the two services layer them differently

IAM: `/healthz` returns `200 {"status":"ok"}` unconditionally; `/readyz` executes `SELECT 1`
against the pool and returns `503 {"status":"unready"}` on error. Gateway: `/healthz` is
dependency-free by construction (its test asserts a never-invoked IAM fake); `/readyz` probes IAM
with a deliberately-invalid sentinel token and classifies reachable/unreachable.

An earlier draft claimed both services mount health outside the tracing/metrics layers. **That is
true only of IAM.** `paigasus-iam/src/adapters/http/mod.rs:864,901-904` merges `health_router()` and
`readyz_router()` above `app_routes`, so probe traffic emits no request span and counts toward no
RED metrics. The gateway does the opposite, deliberately and with a comment saying so
(`paigasus-gateway/src/adapters/http/mod.rs:98-109`): health routes are declared on the same
`Router` that then takes `http_metrics_layer("gateway")` and `CorrelationLayer`, described there as
*"harmless and not worth diverging the two services' composition for"*.

Two consequences follow, and both belong in the probe contract SMA-513 inherits rather than in a
code change here:

1. **Gateway probe traffic is counted.** Every liveness and readiness poll increments
   `gateway_http_requests_total{route="/healthz"|"/readyz"}` and mints correlation ids. With a
   Docker `HEALTHCHECK` at 30 s plus a k8s `readinessProbe` at the 10 s default across N replicas,
   this is a permanent synthetic floor under the SMA-466 RED dashboards.
2. **Gateway `/readyz` costs a real RPC.** It issues an `IntrospectApiKey` gRPC call to IAM on every
   poll (`mod.rs:140-151`), so readiness probe frequency is load on IAM, not just on the gateway.

Changing the gateway's composition is explicitly **not** in scope: the placement is a documented
SMA-504 D10 decision, and rewriting it to match IAM is a behavioural change to a shipped service
hiding inside a containerization issue. The design instead recommends probe periods that keep the
synthetic floor small (§ 4.6) and hands SMA-513 a `route!~"/healthz|/readyz"` note for the
dashboards.

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

A fifth question — the one that most often breaks a `FROM scratch` image — was resolved by running
the real thing rather than reasoning about it. glibc `dlopen`s NSS modules at the first
`getaddrinfo`, and a chiseled rootfs is a classic place for that to fail on the first DNS lookup,
long after every file-presence check has passed. Measured, by building a scratch image from this
exact rootfs and executing a binary in it as uid 65532:

```
/etc/nsswitch.conf : ABSENT
NSS modules        : libnss_dns.so.2, libnss_files.so.2, libnss_compat.so.2  (via libc6_libs)
/etc/hosts         : absent from the rootfs; Docker injects it at runtime

DNS OK: pgdb       -> 172.18.0.2:5432    (container hostname, user-defined bridge)
DNS OK: github.com -> 140.82.121.4:443   (public resolution)
uid=65532 euid=65532
```

**Name resolution works, and the absent `nsswitch.conf` is not a defect**: with the file missing
glibc falls back to a compiled-in default that covers `files dns` for the `hosts` database, and the
modules themselves ship inside `libc6_libs`. This is recorded as measured rather than assumed
because the same reasoning applied to a *different* base could give the opposite answer, and
because the smoke test is designed to keep it proven (§ 5.2 resolves Postgres by container
hostname, never `127.0.0.1`, so the assertion cannot go vacuous).

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

Design choices in § 3 are made to require **no change** to any of these. That is not the same as
"no gate re-runs", and an earlier draft blurred the two. Several existing gates legitimately re-key
and re-run on this change (note D9's single `rs/Dockerfile` sits OUTSIDE `rs/crates/`, so
`repo:publish-metadata` is not among them) — all expected to pass, but the implementer should know which:

| Gate | Why it re-keys |
| --- | --- |
| `repo:observability-drift` | `inputs: rs/crates/libs/paigasus-observability/**/*` — the new `health.rs` |
| `repo:error-code-single-site` | `inputs: rs/crates/**/src/**/*.rs` — the two `main.rs` edits |
| `repo:machete` | `inputs: rs/**/*.rs` |
| `repo:redis-connect-single-site` | the IAM `main.rs` edit |
| `repo:actionlint` | `inputs: ['**/*']`, and it will additionally **lint the new workflow** |

That last row carries a real constraint rather than just a cost. SMA-525's `pattern_verdict`
requires every **wildcard-free** `paths:` entry to be an exactly-tracked file, so each literal path
in `images.yml` (`rs/Cargo.lock`, `rs/Cargo.toml`, `rs/rust-toolchain.toml`, the Dockerfile, the
`.dockerignore`, the workflow itself) must be committed **in the same change** that introduces the
filter, or the gate reds. Glob entries such as `ci/images/**` are exempt from that rule.

## 3. Decisions

| # | Decision | Choice |
| --- | --- | --- |
| D1 | CI scope | **Build + verify, no publish.** No registry, credentials or retention policy until there is a consumer (SMA-513) |
| D2 | Runtime base | **Ubuntu 24.04 chiseled** — a `scratch` image carrying a 5.3 MB chisel-cut rootfs |
| D3 | Builder | **`rust:1.95.0-bookworm`**, digest-pinned, with the toolchain agreement *asserted*, not assumed (D3 note) |
| D4 | Probe | A **`healthcheck [--path P]` subcommand on each service binary**, one shared implementation |
| D5 | Build entry point | **`ci/images/run.sh {build,smoke,all}`**, the house `ci/<gate>/run.sh` convention. **No Moon task** |
| D6 | Trigger | Dedicated `images.yml`; **push-to-`main`** on `rs/**`, **PR** on build-inputs only |
| D7 | Config | **Runtime env only.** No config file in the image, no `ENV IAM_*`/`ENV GATEWAY_*` |
| D8 | Architecture | **amd64 verified in CI**; the Dockerfile stays arch-agnostic so an arm64 host builds arm64 |
| D9 | One Dockerfile | **A single parameterized Dockerfile**, `ARG BIN`, built twice — not two near-identical copies |
| D10 | Image naming | **Decided now, published later:** `ghcr.io/<org>/paigasus-{iam,gateway}:<git-sha>` |

### D2 — why chiseled Ubuntu over the alternatives

**Alpine** was the first choice and was reversed. It forces `x86_64-unknown-linux-musl`, adding a
second Rust target to build and cache, and imports musl's `mallocng` — measurably slower than glibc
under exactly the thread contention these services generate (Cedar evaluation, blake3 hashing, JWKS
validation, a pooled Postgres). Its one real advantage was that busybox `wget` makes `HEALTHCHECK`
free; D4 removes that advantage at a cost of ~40 lines.

**Chainguard** was evaluated and rejected on tier mechanics, not merit. The free tier is
`:latest`/`:latest-dev` only — versioned tags moved to the paid catalogs in Aug/Nov 2024 — with no
CVE-remediation SLA. In a repo where GitHub Actions are SHA-pinned, rustc is pinned to `1.95.0` and
every proto CLI is version-pinned, a floating `:latest` would become the single least-pinned input
in the build. Digest-pinning does not rescue it, because free-tier images are not retained across
versions, so a pinned digest can begin 404-ing and red CI on a day nothing changed.

**`scratch` + static musl** is the true floor, but re-imports musl for no gain over chiseled once
the shell is gone either way.

### D3 — the builder pin must be asserted, because `FROM` alone does not pin the compiler

`rust-toolchain.toml` (`channel = "1.95.0"`) sits inside the `rs/` build context and is therefore
copied into the builder. rustup honours it, so the tag on the `FROM` line does **not** determine
which compiler runs: after a routine channel bump, `FROM rust:1.95.0-bookworm` would still be
pinned while rustup silently downloads and uses the new channel inside the old image. The image
would look pinned and be nothing of the sort — the same silent-omission class
`repo:affected-smoke` and `repo:error-code-single-site` exist to prevent.

Two mitigations, both cheap:

- `ci/images/run.sh` greps the Dockerfile's `FROM rust:<X.Y.Z>` and compares it to
  `rust-toolchain.toml`'s `channel`, failing loudly on disagreement.
- The builder stage sets `ENV RUSTUP_TOOLCHAIN=1.95.0` so the pinned image toolchain is used
  deliberately, and the `components = ["rustfmt", "clippy"]` fetch that would otherwise happen
  implicitly on first `cargo` invocation (the official images install `--profile minimal`) never
  becomes an unpinned network input of the image build.

A related invariant, stated because violating it fails at *container start* rather than build time
and would therefore land on `main` invisibly (limitation 1): **builder glibc must be ≤ runtime
glibc.** `rust:1.95.0-bookworm` is Debian 12 (glibc 2.36); Ubuntu 24.04 is 2.39. A future
"modernise the builder" bump to a noble/trixie-based image inverts it and produces
`GLIBC_2.4x not found` at run time. Both the Dockerfile comment and `run.sh` record it.

### D4 — why a subcommand, not a separate probe binary

A shell-less image cannot run `curl`. Three options were considered:

- **A separate `paigasus-probe` crate** would be the most explicitly reusable convention, but it
  adds a Rust crate, which re-baselines the `lockfile->all-lint` strict-equality set, and the target
  URL becomes an argument that can drift from the port the service actually binds.
- **No in-image probe** would be defensible — Kubernetes `httpGet` probes are executed by the
  kubelet — but it leaves `docker run` and compose users with no health signal, and reads as a dodge
  of AC-3.
- **A subcommand** costs no new crate, no new bytes in the image, and no gate re-baseline, and it
  reads `http_addr` from the *same* figment config the server binds, so the probe target cannot
  drift from reality.

The subcommand takes an optional `--path`, defaulting to `/healthz`. Without it, AC-3 would be met
only in a weakened restatement: `/readyz` would have no executable artifact anywhere in this issue,
and a shell-less image makes a Kubernetes `exec` readiness probe impossible for anyone who needs
one. Ten extra lines close that.

The convention the console images inherit is *"every image ships a self-contained probe entrypoint
and a `HEALTHCHECK` that uses it"* — which a Node image satisfies with `node -e`, without this
binary.

### D9 — one Dockerfile, not two

The two services' Dockerfiles differ only in crate name, binary name and exposed ports. Two copies
would mean ~60 duplicated lines carrying two base-image digests and two chisel checksums, in a repo
that ships `repo:redis-connect-single-site`, `repo:iam-docker-policy-single-site` and
`repo:error-code-single-site` precisely to stop hand-duplicated policy from drifting. A single
`rs/Dockerfile` with `ARG BIN` / `ARG PORTS`, invoked twice by `run.sh`, keeps the pins in one place.

### D10 — decide the image names now, publish later

D1 defers publishing, but deferring the *naming* would re-create the failure this whole workstream
exists to prevent — SMA-513's chart needs `image.repository`/`image.tag` to inherit something, and
"the console must not invent a bespoke image story" applies just as much to the chart. The names and
the `:<git-sha>` tag convention are therefore fixed here and recorded in the runbook; only the
push, credentials, semver tags, retention and signing are deferred.

## 4. Architecture

### 4.1 Files

```
rs/.dockerignore                                     new
rs/Dockerfile                                        new  (parameterized, ARG BIN)
rs/crates/libs/paigasus-observability/src/health.rs  new  (the probe)
rs/crates/services/{paigasus-iam,paigasus-gateway}/src/main.rs   modified (arg dispatch)
ci/images/run.sh                                     new  ({build,smoke,all})
.github/workflows/images.yml                         new
.github/dependabot.yml                               modified (docker ecosystem)
docs/ops/RUNBOOK-containers.md                       new
CLAUDE.md                                            modified (gotchas)
```

Every new source file opens with an SPDX header per repo convention — `//` in `health.rs`, `#` in
`run.sh` and the Dockerfile.

The build **context is `rs/`** (the cargo workspace root). `rs/.dockerignore` must exclude `target/`
above all else — without it the context upload is gigabytes on any machine that has ever run
`cargo build`.

`.github/dependabot.yml` gains a `package-ecosystem: docker` entry. D2's whole argument against
Chainguard was that a floating tag would be the least-pinned input; the mirror-image risk — a
pinned-and-never-updated `ubuntu:24.04` and `rust:1.95.0-bookworm` on a security-audited product —
has to be closed in the same change, or the pins rot. Dependabot's docker updater handles digest
pins. The chisel version and its checksums are **not** covered by any updater and are owned by
whoever bumps `CHISEL_VERSION`; the runbook says so.

### 4.2 Dockerfile shape

Three stages, parameterized by `ARG BIN`:

```dockerfile
# syntax=docker/dockerfile:1
# SPDX-License-Identifier: Apache-2.0
ARG BIN

# Builder glibc MUST be <= runtime glibc (bookworm 2.36 <= noble 2.39). Inverting this
# produces `GLIBC_2.4x not found` at CONTAINER START, not at build time. See spec D3.
FROM rust:1.95.0-bookworm@sha256:... AS builder
# Use the image's own pinned toolchain rather than letting rust-toolchain.toml pull a
# different channel over the network at build time (spec D3).
ENV RUSTUP_TOOLCHAIN=1.95.0
ARG BIN
WORKDIR /src
COPY . .
# The binary is copied OUT of the cache mount inside the same RUN: a cache mount is not part
# of the resulting layer, so a later `COPY --from=builder /src/target/...` finds nothing.
# Distinct cache `id`s per binary: the default sharing mode is `shared`, so two concurrent
# builds would otherwise contend on cargo's lock.
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=target-${BIN},target=/src/target \
    cargo build --release --locked -p ${BIN} --bin ${BIN} \
 && mkdir -p /out && cp /src/target/release/${BIN} /out/${BIN}

FROM ubuntu:24.04@sha256:... AS rootfs
ARG CHISEL_VERSION=v1.4.2
# One pinned checksum PER ARCHITECTURE: the release publishes a distinct tarball and a
# distinct sha384 per arch, so a lone checksum would either break arm64 or, if skipped,
# silently drop the integrity check.
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
ARG BIN
COPY --from=rootfs /rootfs /
# Installed at a FIXED path, not /usr/local/bin/${BIN}: ENTRYPOINT/HEALTHCHECK in exec form do
# NOT expand ARG or ENV, so a parameterized Dockerfile cannot name the binary there. See below.
COPY --from=builder /out/${BIN} /usr/local/bin/paigasus-service
# `FROM scratch` leaves Config.Env empty. Docker injects a default PATH; containerd/CRI does
# not reliably, so an SMA-513 exec probe written as ["paigasus-service","healthcheck"] would
# fail there without this.
ENV PATH=/usr/local/bin
USER 65532:65532
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/paigasus-service"]
HEALTHCHECK --interval=30s --timeout=3s --start-period=60s --retries=3 \
  CMD ["/usr/local/bin/paigasus-service", "healthcheck"]
```

Notes on choices that are not obvious:

- **Full `rust:1.95.0-bookworm`, not `-slim`:** `ring` and `blake3` compile C, so the builder needs a
  C toolchain. Builder size is irrelevant — exactly one file crosses into the final stage.
- **`--locked`:** the image ships the resolution the repo has committed. `lint` and
  `repo:publish-metadata` already take this posture; a Dependabot Cargo PR has previously shipped a
  lockfile resolved from 3 of 11 workspace members.
- **Checksum verification is `sha384sum`**, because that is what Canonical publishes alongside the
  chisel release assets.
- **`USER 65532:65532` is numeric** because the rootfs has no `/etc/passwd` (§ 2.6).
- **`STOPSIGNAL SIGTERM`** makes explicit what both binaries already rely on: each installs a
  SIGTERM handler and drains (IAM a `JoinSet` of relays and maintainers). It also documents for
  SMA-513 what `terminationGracePeriodSeconds` must cover.
- **`ENTRYPOINT`/`HEALTHCHECK` use absolute paths and the exec form.** A shell form would need a
  shell that does not exist.
- **The binary is installed as `/usr/local/bin/paigasus-service`, a fixed name.** This is forced,
  not stylistic: exec-form `ENTRYPOINT`/`HEALTHCHECK` perform no `ARG`/`ENV` expansion, so
  `["/usr/local/bin/${BIN}"]` would look for a literal `${BIN}`. Naming it once is what makes D9's
  single Dockerfile possible; copying it twice (real name plus a fixed alias) would double the
  largest layer, and `scratch` has no shell with which to symlink. The cost is that `argv[0]` and
  `docker top` show `paigasus-service` rather than the service name — which costs nothing
  operationally, because logs and metrics take their service name from the binary itself
  (`paigasus_logging::init("paigasus-iam")`), not from `argv[0]`. `run.sh` additionally sets
  `org.opencontainers.image.title` to the real service name, so the image is self-describing.
- **`--start-period=60s`, not 15s**, because IAM runs `Migrator::up` before binding (§ 2.4). A short
  start period would have the daemon marking a migrating container unhealthy.
- **`EXPOSE` is documentation only.** IAM declares 8080/9090, the gateway 8088; a deployment that
  configures a separate `metrics.addr` exposes that port itself, since it is optional config.
- **No `WORKDIR` carrying a config file.** With no `iam.toml` present, figment resolves defaults +
  env. A self-hoster who *mounts* a TOML still gets file layering, because that is figment's
  documented behaviour — AC-2 forbids baking config at build time, not supporting file config.

**Caching.** BuildKit cache mounts make local rebuilds fast but are **not** persisted by the
`type=gha` backend, so they are empty on every CI run. `type=gha` layer caching is deliberately
**not** used either: the repository-wide Actions cache is a 10 GB LRU pool that `ci.yml` already
fills with `~/.cargo` + `rs/target`, and evicting *that* would cold-start the **required** check to
speed up a non-required one. CI therefore compiles cold, which D6's narrow trigger is what makes
acceptable. `cargo-chef` is likewise not adopted — it exists to solve the cold-build cost that D6
already bounds. § 7 records both as revisitable.

### 4.3 The probe

`paigasus_observability::health` — one implementation, consumed by both services, both of which
already depend on this crate. Placing it here rather than duplicating ~40 lines into two `main.rs`
files is the same single-site discipline that `repo:redis-connect-single-site` and
`repo:iam-docker-policy-single-site` already enforce elsewhere.

```rust
/// Probe `path` on `addr` over plaintext HTTP/1.1, within a TOTAL deadline.
/// `Ok(true)` iff the response status is 2xx.
pub fn probe(addr: SocketAddr, path: &str, deadline: Duration) -> io::Result<bool>
```

Standard library only — no new dependency in a crate both services link.

Three behaviours are load-bearing and are specified rather than left to the implementer:

1. **`deadline` is a total budget, not a connect timeout.** `TcpStream::connect_timeout` bounds only
   the connect; a server that has accepted but wedged (a saturated axum, a blocked handler) would
   otherwise block the probe forever. The remaining budget is applied via `set_read_timeout` /
   `set_write_timeout`, re-armed after each read.
2. **An unspecified address is mapped to loopback before connecting.** Both services default to
   `0.0.0.0`, which is not a destination.
3. **Plaintext HTTP/1.1 is assumed, and that is safe today** — neither service terminates TLS
   (`axum-server` is a dev-dependency only), so both images require a TLS-terminating ingress. If an
   `http_addr` TLS option is ever added, this assumption breaks silently, so the runbook states it.

### 4.4 The subcommand

`main` becomes a plain `fn` that dispatches *before* `#[tokio::main]` builds a multi-threaded
runtime for a process that will make one blocking request and exit:

```rust
fn main() -> ExitCode {
    match dispatch(std::env::args().skip(1)) {
        Mode::Healthcheck { path } => healthcheck(&path),
        Mode::Serve => run(),          // #[tokio::main]-equivalent async entry
    }
}
```

`dispatch` is a pure, unit-testable function. Three rules:

- `healthcheck [--path P]`, default `/healthz`, runs the probe.
- No arguments runs the server.
- **Any other argv[1] exits 2 with a one-line usage.** A typo'd `healthchek` in a `HEALTHCHECK`, a
  compose file or an SMA-513 exec probe must not silently fall through to a full server boot —
  for IAM that would mean `Database::connect` + `Migrator::up` on every probe interval, each attempt
  SIGKILLed at the 3 s timeout.

The healthcheck path calls `Config::load()` but **not** `validate()`: probe mode needs only
`http_addr`, and IAM's validator rejects a config with no configured issuers, which would fail the
healthcheck for a reason unrelated to health.

Exit-code contract: **0 = healthy, 1 = unhealthy (including connection refused, timeout, non-2xx),
2 = usage error.** An `Err` from `probe` is collapsed to unhealthy, never propagated — and the error
text is **not** printed. Docker retains the last five health-check outputs in `State.Health.Log`, and
a propagated `figment::Error` names config keys and can carry values from the `IAM_*` env layer. Probe
mode emits a fixed string (`healthcheck: config load failed`) instead.

### 4.5 Build and smoke script

`ci/images/run.sh {build,smoke,all} [iam|gateway]` — one file, house convention. `build` applies OCI
labels (`org.opencontainers.image.source`, `.revision`, `.licenses`, `.title`, `.description`;
revision is build-identifying, not deployment-varying, so AC-2 holds) and enforces three mechanical
guards that are cheaper here than as prose:

- no `ENV IAM_`/`ENV GATEWAY_` line in the Dockerfile (D7);
- `FROM rust:<X.Y.Z>` equals `rust-toolchain.toml`'s `channel` (D3);
- the builder base is a `bookworm` tag, i.e. the glibc ordering invariant (D3).

`build` also captures the resolved package versions that `chisel cut` selected into a
`chisel-manifest.txt` build artifact, so "which libc did this image ship?" is answerable after the
fact — the cheap half of limitation 2. It reads them from `chisel cut`'s own build output
(`--progress=plain`, whose `Fetching pool/main/g/glibc/libc6_2.39-0ubuntu8.8_amd64.deb` lines carry
the exact versions) rather than writing a manifest file into `/rootfs`, which would ship the
manifest inside every image.

### 4.6 The probe contract the console must inherit

Documented in `docs/ops/RUNBOOK-containers.md` and consumed by SMA-513:

| Probe | Endpoint | Notes |
| --- | --- | --- |
| liveness | `GET /healthz` | Never touches a dependency, by construction in both services |
| readiness | `GET /readyz` | IAM pings Postgres; gateway issues a real gRPC introspect to IAM |
| startup | `GET /healthz` | IAM migrates at boot — a `startupProbe` with a generous `failureThreshold` is required, or the kubelet kills it mid-migration |

Rules that go with it:

- **Probe commands use absolute paths and the exec form**; the image has no shell.
- **Gateway probe periods are load.** Because gateway `/readyz` costs an IAM RPC and both its health
  routes are metered (§ 2.5), keep `readinessProbe.periodSeconds` at 30 s or above and give SMA-513
  dashboards a `route!~"/healthz|/readyz"` filter.
- **IAM migrates on every process start, with no advisory lock around `Migrator::up`.** A rolling
  update or a scale-out therefore risks concurrent migration. Until that is fixed in the service, the
  documented contract for SMA-513 is: migrate with a single replica — `replicas: 1` with
  `strategy.rollingUpdate.maxSurge: 0`, or a pre-install migration Job. This is a service property,
  not an image property, but containerization is what makes it reachable, so it is stated here rather
  than discovered by the first operator who scales IAM to 2.
- **Images are `ghcr.io/<org>/paigasus-{iam,gateway}:<git-sha>`** (D10), and both require a
  TLS-terminating ingress (§ 4.3).

Docker has no readiness concept, so the in-image `HEALTHCHECK` maps to `/healthz` only. `/readyz` is
reachable through the same binary via `/usr/local/bin/paigasus-service healthcheck --path /readyz` for anyone who needs an
`exec` readiness probe.

## 5. Verification

### 5.1 Rust

Unit tests for `probe()` against an in-process `TcpListener`: 200, 503, connection refused, the
`0.0.0.0` -> loopback mapping, and — the one that pins behaviour 1 in § 4.3 — a listener that
**accepts and never writes**, which must hit the total deadline rather than block.

Unit tests for `dispatch()` per service: `healthcheck`, `healthcheck --path /readyz`, no args, and an
unknown argument (exit 2, no server boot).

These run under the existing `moon ci :test` graph with no task or gate changes.

### 5.2 The smoke test

Designed so it cannot pass vacuously — the failure this must never have is an image that starts,
answers `/healthz`, and is declared good.

- **gateway, standalone.** With only `GATEWAY_UPSTREAM__OPENAI__API_KEY` set (a literal dummy,
  `sk-smoke-not-a-real-key`): `/healthz` -> **200**, and `/readyz` -> **503**, because no IAM is
  reachable. The negative case is the point; asserting only the positive one would not catch a
  `/readyz` that always returns 200. A successful gateway boot is also a partial CA-bundle proof,
  since `with_native_roots()` runs eagerly (§ 2.2).
- **iam, with Postgres, reached by container hostname.** On a user-defined bridge network with
  `IAM_DATABASE_URL` pointing at `pgdb`, **never** `127.0.0.1` — so glibc name resolution inside the
  chiseled rootfs is genuinely exercised (§ 2.6) rather than bypassed by an IP literal. Then
  `/healthz` -> 200 and `/readyz` -> **200** after migrations.
- **The in-image probe.** `docker inspect --format '{{.State.Health.Status}}'` polled until
  `healthy` — the only assertion that exercises the probe binary *inside* the shell-less image.
  Plus `paigasus-service healthcheck --path /readyz` invoked via `docker exec` to prove the
  readiness path.
- **Runtime-only config (AC-2).** Both containers start with env vars and nothing else — no mounted
  file, no `--env-file`. Success *is* the proof of D7.
- **The base is still the base.** Three assertions that would otherwise let a future
  `FROM ubuntu:24.04` "just to debug something" pass the whole suite: `docker run --entrypoint
  /bin/sh <img> -c true` must **fail**; the extracted `/etc/ssl/certs/ca-certificates.crt` must
  carry >= 100 certificates; and `docker image inspect --format '{{.Size}}'` must be under a stated
  ceiling.
- **Non-root, as actually run.** `docker top <c> -o user` rather than
  `docker inspect '{{.Config.User}}'` — the latter asserts image config, so a `--user 0` invocation
  would still pass it.

`run.sh smoke` must never dump full `docker inspect` output into the CI log; `.Config.Env` would
print the dummy key and set a bad precedent for anyone who later runs it with a real one.

### 5.3 CI

`.github/workflows/images.yml`, modelled on `prebuild.yml`:

- **Job shape:** a two-leg matrix (`iam`, `gateway`) with `fail-fast: false`, so the two cold
  `--release` builds do not serialize into one job against one timeout, and a gateway failure still
  reports the IAM result. `timeout-minutes: 45`.
- **Concurrency:** `group: images-${{ github.workflow }}-${{ github.ref }}-${{ github.event_name }}`
  with `cancel-in-progress: ${{ github.event_name == 'pull_request' }}` — the event name is in the
  group so a manual dispatch cannot cancel a running push job, mirroring `prebuild.yml`'s reasoning.
- **Triggers:** `push` to `main` filtered on `rs/**`, the Dockerfile, `ci/images/**`, the workflow;
  `pull_request` filtered on the inputs that can actually break an image build (`rs/Cargo.lock`,
  `rs/Cargo.toml`, `rs/rust-toolchain.toml`, `rs/Dockerfile`, `rs/.dockerignore`, `ci/images/**`, the
  workflow); `workflow_dispatch`.
- All `branches:`/`paths:` as **block sequences**, and every wildcard-free `paths:` entry committed in
  the same change (§ 2.7). `permissions: contents: read` only. Actions SHA-pinned. ci.yml's
  reclaim-runner-disk step included, since two cold release builds of a tree that has already
  overflowed the runner disk once is the sharpest cost risk here.
- Not a required check, exactly as `prebuild.yml` is not.

## 6. Rollout and rollback

Additive. No existing Moon task, gate definition, `T` array or workflow trigger changes, so no gate
needs re-baselining. Several existing gates do **re-key and re-run** (§ 2.7) — all expected to pass.

The only changes to shipped code are the two `main.rs` dispatch edits and the new `health.rs`; the
dispatch is inert unless argv[1] is present. Rollback is deleting the new files and reverting the
dispatch; nothing depends on any of it until SMA-513.

## 7. Limitations

1. **The image build is not a required check.** A broken image build reds `main`, not the PR that
   caused it — the `prebuild.yml` trade-off, accepted knowingly to avoid a `--release` build on every
   PR against a 30-minute timeout and a ~14 GB disk that cedar-policy has already overflowed once
   (SMA-444). Compounding it: `ci/images/run.sh` is shell that no Moon task and no `T` entry runs, so
   until the first push to `main`, the *only* verification of the repo's first container build is the
   PR-path filter firing. Reviewers should trigger `workflow_dispatch` on the PR branch.
2. **Not bit-reproducible over time.** `chisel cut` resolves against the live Ubuntu archive (§ 2.6),
   so rebuilding an old commit produces a newer, patched base. Mitigated, not solved, by archiving the
   resolved package manifest (§ 4.5). Pinning to a `snapshot.ubuntu.com` pocket via a custom chisel
   release directory would solve it properly and is deliberately deferred: it means maintaining a
   forked release dir, which is a larger commitment than this issue should make unilaterally.
3. **A private-CA IdP is not deployable.** IAM's reqwest path carries compiled-in webpki roots, so
   mounting a CA into the image does not make IAM trust a corporate IdP (§ 2.2). Needs its own issue.
4. **Gateway probe traffic pollutes gateway RED metrics** and costs an IAM RPC per readiness poll
   (§ 2.5). Mitigated by probe-period guidance, not by a code change.
5. **amd64 only in CI.** The Dockerfile is arch-agnostic and an arm64 host builds arm64, but no arm64
   leg is verified. Multi-arch belongs with publishing.
6. **No `cargo-chef`, no `type=gha` cache** (§ 4.2), so CI compiles cold whenever the trigger fires.
7. **The slice list is hand-maintained.** A future dependency needing a system library fails at
   container start with a loader error, not at build time. The smoke test catches it only for code
   paths it reaches.
8. **`apt-get install ca-certificates curl` in the rootfs stage is unpinned** — the one unpinned
   input in a Dockerfile whose whole argument is pinning. It affects only the throwaway stage that
   fetches chisel, never the final image.
9. **`chisel` is fetched from GitHub at build time**, pinned by version and sha384. A GitHub outage
   fails the build, same class as any base-image pull.

## 8. Non-goals

- Pushing images, credentials, semver tags, retention policy, cosign signing, SBOM/provenance. The
  *names* are decided (D10); only publication is deferred.
- Helm charts and Kubernetes manifests — SMA-513 owns them; this spec hands them a probe contract
  (§ 4.6).
- Console/Next.js images. This spec establishes the conventions they inherit; it does not build them.
- Any change to `/healthz` or `/readyz` **semantics**, or to the gateway's router composition
  (§ 2.5) — both are deliberate shipped decisions, and rewriting them inside a containerization
  issue would be a behavioural change in disguise.
- Switching reqwest to native roots (limitation 3) and adding an advisory lock around
  `Migrator::up` (§ 4.6) — both are real, both are service changes, both need their own issue.
- Multi-arch builds and arm64 CI verification.
