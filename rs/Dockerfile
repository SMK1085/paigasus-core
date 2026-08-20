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
COPY . .
# The binary is copied OUT of the cache mount inside this same RUN: a cache mount is not part of
# the resulting layer, so `COPY --from=builder /src/target/...` in a later stage finds nothing.
# Distinct cache ids per binary — the default sharing mode is `shared`, so two concurrent builds
# would otherwise contend on cargo's lock.
RUN --mount=type=cache,id=cargo-registry,target=/usr/local/cargo/registry \
    --mount=type=cache,id=target-${BIN},target=/src/target \
    cargo build --release --locked -p "${BIN}" --bin "${BIN}" \
 && mkdir -p /out && cp "/src/target/release/${BIN}" /out/service

FROM ubuntu:24.04@sha256:d78ab76437b1afc5f01e223d6bf0172763f404bb166441328845adbef44518cb AS rootfs
ARG CHISEL_VERSION=v1.4.2
# One pinned checksum PER ARCHITECTURE: the release publishes a distinct tarball and a distinct
# sha384 per arch, so a single checksum would either break arm64 or silently drop the check.
ARG CHISEL_SHA384_amd64=8e5e8df4dc783dcfa827ca9990ba871af350738de67c51706b3c06bfd4725ab0edbddd9ad4110d1047ecfdc586f7dac6
ARG CHISEL_SHA384_arm64=216f10d4cc461411558fa4ac03fc24e104589126f87657457877389ae8015e1eac4299fcd8557c0dfea3d33342aa3297
RUN set -eux; \
    arch="$(dpkg --print-architecture)"; \
    case "$arch" in \
      amd64) sha="${CHISEL_SHA384_amd64}" ;; \
      arm64) sha="${CHISEL_SHA384_arm64}" ;; \
      *) echo "no pinned chisel checksum for architecture ${arch}" >&2; exit 1 ;; \
    esac; \
    apt-get update; \
    apt-get install -y --no-install-recommends ca-certificates curl; \
    curl -sSLf -o /tmp/chisel.tar.gz \
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
# 60s start period because IAM runs Migrator::up before it binds.
HEALTHCHECK --interval=30s --timeout=3s --start-period=60s --retries=3 \
  CMD ["/usr/local/bin/paigasus-service", "healthcheck"]
