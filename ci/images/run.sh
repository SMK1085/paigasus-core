#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# SMA-500 — build and smoke-test the service container images.
#
# Deliberately NOT a Moon task: a `repo:*` task would have to join ci.yml's `T=(…)` array (a
# --release build on every affected PR, against a 30-minute timeout and the ~14 GB disk that
# cedar-policy has already overflowed once) or become a T_EXEMPT entry. It runs from
# .github/workflows/images.yml instead.
#
# usage: ci/images/run.sh {build|smoke|all} [iam|gateway]
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
REGISTRY="${PAIGASUS_IMAGE_REGISTRY:-ghcr.io/paigasus}"
REVISION="$(git -C "$ROOT" rev-parse HEAD)"

crate_for() {
  case "$1" in
    iam)     echo "paigasus-iam" ;;
    gateway) echo "paigasus-gateway" ;;
    *) echo "unknown service: $1" >&2; return 1 ;;
  esac
}

# The pins in rs/Dockerfile are only as good as their agreement with the repo's own toolchain
# pin. `FROM rust:X.Y.Z` does NOT decide which compiler runs — rust-toolchain.toml is inside the
# build context and rustup honours it — so a channel bump would leave the Dockerfile looking
# pinned and being nothing of the sort (SMA-500 D3).
assert_pins() {
  local dockerfile="$ROOT/rs/Dockerfile"
  local channel from_version
  channel="$(grep -E '^channel[[:space:]]*=' "$ROOT/rs/rust-toolchain.toml" | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
  from_version="$(grep -oE '^FROM rust:[0-9]+\.[0-9]+\.[0-9]+' "$dockerfile" | head -1 | sed 's/^FROM rust://')"
  if [ "$channel" != "$from_version" ]; then
    echo "::error::rs/Dockerfile builds on rust:${from_version} but rs/rust-toolchain.toml pins ${channel}." >&2
    echo "  Bump the FROM line (and its digest) together with the toolchain, or the image ships a different compiler." >&2
    return 1
  fi
  # Builder glibc must be <= runtime glibc. bookworm is 2.36, noble (ubuntu:24.04) is 2.39.
  # Inverting it fails at CONTAINER START with `GLIBC_2.4x not found`, not at build time.
  if ! grep -qE '^FROM rust:[0-9.]+-bookworm@sha256:' "$dockerfile"; then
    echo "::error::the builder base must stay a digest-pinned -bookworm tag (glibc 2.36 <= the runtime's 2.39)." >&2
    return 1
  fi
  # AC-2: nothing deployment-varying may be baked. Config reaches the container through
  # IAM_*/GATEWAY_* env at RUNTIME only.
  if grep -nE '^[[:space:]]*ENV[[:space:]]+(IAM_|GATEWAY_)' "$dockerfile"; then
    echo "::error::rs/Dockerfile bakes service config into the image; configure at runtime via env instead." >&2
    return 1
  fi
  echo "  pins OK: rustc ${channel}, bookworm builder, no baked service config"
}

build_one() {
  local service="$1" crate tag
  crate="$(crate_for "$service")"
  tag="${REGISTRY}/${crate}:${REVISION}"
  echo "== build ${crate} =="
  # --progress=plain so chisel's `Fetching pool/...` lines are capturable below: they name the
  # exact archive package versions this image resolved, which `chisel cut` re-resolves against
  # the LIVE archive on every build (SMA-500 limitation 2).
  docker build \
    --progress=plain \
    -f "$ROOT/rs/Dockerfile" \
    --build-arg "BIN=${crate}" \
    --label "org.opencontainers.image.title=${crate}" \
    --label "org.opencontainers.image.description=Paigasus ${service} service" \
    --label "org.opencontainers.image.source=https://github.com/paigasus/paigasus-core" \
    --label "org.opencontainers.image.revision=${REVISION}" \
    --label "org.opencontainers.image.licenses=Apache-2.0" \
    -t "$tag" -t "${crate}:dev" \
    "$ROOT/rs" 2>&1 | tee "/tmp/paigasus-build-${service}.log"
  grep -oE 'Fetching pool/[^ ]+\.deb' "/tmp/paigasus-build-${service}.log" | sort -u > "$ROOT/chisel-manifest-${service}.txt" || true
  echo "  built ${tag}"
}

cmd="${1:?usage: ci/images/run.sh \{build\|smoke\|all\} [iam|gateway]}"
target="${2:-all}"
services=("iam" "gateway")
[ "$target" != "all" ] && services=("$target")

case "$cmd" in
  build) assert_pins; for s in "${services[@]}"; do build_one "$s"; done ;;
  *) echo "unknown command: $cmd" >&2; exit 1 ;;
esac
