# syntax=docker/dockerfile:1
# SPDX-License-Identifier: Apache-2.0
#
# One parameterized image for both service binaries (SMA-500 D9). Build with:
#   docker build -f rs/Dockerfile --build-arg BIN=paigasus-iam rs/
# `ci/images/run.sh build` is the supported entry point and adds the pins and labels.

ARG BIN

# Builder glibc MUST be <= runtime glibc (bookworm 2.36 <= noble 2.39). Inverting this produces
# `GLIBC_2.4x not found` at CONTAINER START, not at build time — invisible on a PR, since the
# image workflow is not a required check.
FROM rust:1.95.0-bookworm@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1 AS builder
# `rust-toolchain.toml` is inside the build context, so rustup would honour it over this image's
# own toolchain and could fetch a DIFFERENT channel over the network — the `FROM` pin above
# would be decorative. Pin explicitly (SMA-500 D3); ci/images/run.sh asserts the two agree.
ENV RUSTUP_TOOLCHAIN=1.95.0
ARG BIN
WORKDIR /src
# `cargo auditable` embeds the crate list in the binary, which is what makes the image SBOM list
# the Rust dependencies. The OS half of the SBOM comes from the `rootfs` stage below: the
# `base-files_chisel` slice and docker/chisel-dpkg-status.sh (SMA-665).
# MEASURED on 2026-09-20 (spec § 4.5): a plain `cargo build` gives
# `cargo=0` in the SBOM, and `cargo auditable build` gives real purls. syft's
# cargo-auditable-binary-cataloger is in its default set, so no syft flag changes.
# Version-pinned: cargo-auditable has no proto plugin, so this is the only pin it can carry.
# Runs BEFORE `COPY . .` (PR 2 review, minor): it needs no source, so keeping it above the copy
# keeps this layer cacheable across every source-only change instead of reinstalling on each one.
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry \
    cargo install cargo-auditable --locked --version 0.7.6
COPY . .
# The binary is copied OUT of the cache mount inside this same RUN: a cache mount is not part of
# the resulting layer, so `COPY --from=builder /src/target/...` in a later stage finds nothing.
# Distinct cache ids per binary — the default sharing mode is `shared`, so two concurrent builds
# would otherwise contend on cargo's lock.
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=target-${BIN},target=/src/target \
    cargo auditable build --release --locked -p "${BIN}" --bin "${BIN}" \
 && mkdir -p /out && cp "/src/target/release/${BIN}" /out/service

FROM ubuntu:24.04@sha256:008173c23f95b170204355c12626cb5a965d779a7e1283b09e9cffbb1bf33ca3 AS rootfs
ARG CHISEL_VERSION=v1.4.2
# One pinned checksum PER ARCHITECTURE: the release publishes a distinct tarball and a distinct
# sha384 per arch, so a single checksum would either break arm64 or silently drop the check.
ARG CHISEL_SHA384_amd64=8e5e8df4dc783dcfa827ca9990ba871af350738de67c51706b3c06bfd4725ab0edbddd9ad4110d1047ecfdc586f7dac6
ARG CHISEL_SHA384_arm64=216f10d4cc461411558fa4ac03fc24e104589126f87657457877389ae8015e1eac4299fcd8557c0dfea3d33342aa3297
# SMA-665. The `base-files_chisel` slice writes /var/lib/chisel/manifest.wall, chisel's own record
# of the cut. The image keeps it on purpose: this reverses SMA-500 § 4.5 (SMA-665 spec D1, Q4,
# Q5). syft 1.52.0 cannot read that file, so docker/chisel-dpkg-status.sh below turns it into
# dpkg status.d data, which syft reads. jq and zstd are for that script. They stay in this stage
# and never reach the final image. The cut writes its output to a file, not into a pipe (dash has
# no pipefail), and then prints it: extract_chisel_manifest in ci/images/run.sh reads the
# `Fetching pool/...` lines from the build log, and the script reads them from /tmp/cut.log.
RUN set -eux; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
      amd64) sha="${CHISEL_SHA384_amd64}" ;; \
      arm64) sha="${CHISEL_SHA384_arm64}" ;; \
      *) echo "no pinned chisel checksum for architecture ${arch}" >&2; exit 1 ;; \
    esac; \
    apt-get update; \
    apt-get install -y --no-install-recommends ca-certificates curl jq zstd; \
    curl -sSLf -o /tmp/chisel.tar.gz \
      "https://github.com/canonical/chisel/releases/download/${CHISEL_VERSION}/chisel_${CHISEL_VERSION}_linux_${arch}.tar.gz"; \
    echo "${sha}  /tmp/chisel.tar.gz" | sha384sum -c -; \
    tar -xzf /tmp/chisel.tar.gz -C /usr/local/bin; \
    mkdir -p /rootfs; \
    chisel cut --release ubuntu-24.04 --root /rootfs \
      base-files_base base-files_release-info base-files_chisel \
      libc6_libs libgcc-s1_libs ca-certificates_data \
      >/tmp/cut.log 2>&1 || { cat /tmp/cut.log; exit 1; }; \
    cat /tmp/cut.log
COPY --chmod=0755 docker/chisel-dpkg-status.sh /usr/local/bin/chisel-dpkg-status.sh
RUN chisel-dpkg-status.sh /rootfs /tmp/cut.log

# SMA-665 T2 (spec D10): the self-test of docker/chisel-dpkg-status.sh. images.yml builds it with
# `--target chisel-dpkg-status-test`, and BuildKit then skips the Rust builder. A normal build
# never builds it. This stage MUST stay above the final FROM scratch: a build with no --target
# builds the LAST stage, and assert_pins in ci/images/run.sh reads the last FROM as the final stage.
FROM rootfs AS chisel-dpkg-status-test
COPY docker/fixtures/ /fixtures/
COPY --chmod=0755 docker/chisel-dpkg-status-test.sh /usr/local/bin/chisel-dpkg-status-test.sh
RUN chisel-dpkg-status-test.sh /usr/local/bin/chisel-dpkg-status.sh /fixtures

FROM scratch
ARG BIN
COPY --from=rootfs /rootfs /
# Installed at a FIXED path: exec-form ENTRYPOINT/HEALTHCHECK do NOT expand ARG or ENV, so
# ["/usr/local/bin/${BIN}"] would look for a literal ${BIN}. Naming it once is what makes one
# parameterized Dockerfile possible; logs and metrics take the service name from the binary
# itself (paigasus_logging::init), not from argv[0].
COPY --from=builder /out/service /usr/local/bin/paigasus-service
# `FROM scratch` leaves Config.Env empty. Docker injects a default PATH; containerd/CRI does not
# reliably, so a Kubernetes exec probe using a bare command name would fail there without this.
ENV PATH=/usr/local/bin
# Numeric: the chiseled rootfs has no /etc/passwd, so a named user cannot resolve.
USER 65532:65532
# Both binaries install SIGTERM handlers and drain (IAM a JoinSet of relays and maintainers).
STOPSIGNAL SIGTERM
ENTRYPOINT ["/usr/local/bin/paigasus-service"]
# 30s start period: config load, a successful Database::connect, and the three binds. Since
# SMA-571 IAM binds BEFORE it migrates, so /healthz answers within a second of start and the
# start period no longer has to cover migration.lock_wait_secs or the migration itself.
# ci/images/run.sh's assert_pins keeps a floor on this. Note --interval=30s means the first
# probe fires ~30s in regardless, so an interval is the effective floor either way.
# This governs docker run / Compose / Swarm and ci/images/run.sh only — the kubelet ignores a
# HEALTHCHECK entirely, so Kubernetes sizes startupProbe instead (docs/ops/RUNBOOK-containers.md).
HEALTHCHECK --interval=30s --timeout=3s --start-period=30s --retries=3 \
  CMD ["/usr/local/bin/paigasus-service", "healthcheck"]
