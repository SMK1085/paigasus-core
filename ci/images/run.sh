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
  # IAM_*/GATEWAY_* env at RUNTIME only. Join `\`-continued lines first: an ENV instruction can
  # spread its assignments across multiple physical lines, and IAM_/GATEWAY_ can appear as the
  # 2nd+ token on either the first or a continuation line, not only as the token right after
  # `ENV` — a naive single-line "starts with ENV IAM_/GATEWAY_" match misses both.
  local joined_env
  joined_env="$(awk '/\\[[:space:]]*$/ { sub(/\\[[:space:]]*$/, " "); printf "%s", $0; next } { print }' "$dockerfile")"
  if grep -nE '^[[:space:]]*ENV[[:space:]]+.*(IAM_|GATEWAY_)' <<<"$joined_env"; then
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
  # --no-cache-filter=rootfs: the `rootfs` stage never references ARG BIN, so it is
  # byte-identical between the iam and gateway builds — BuildKit would otherwise cache-hit it
  # for whichever service builds SECOND in a `build all` run, leaving that service's manifest
  # silently empty even on a stone-cold runner (SMA-500 fix-round 1). Forcing this one stage to
  # always re-execute is what the comment above already assumed ("re-resolves ... on every
  # build") and costs one small apt/chisel fetch, not a rebuild of the (cache-mounted) Rust
  # compile.
  docker build \
    --progress=plain \
    --no-cache-filter=rootfs \
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
  # Defense in depth on top of --no-cache-filter=rootfs above: if the manifest is EVER empty
  # (a future chisel version changing its log wording, buildkit changing --progress=plain
  # formatting, etc.), fail loudly instead of shipping a 0-byte file that silently answers
  # nothing when someone asks "which libc did this image ship?" (SMA-500 fix-round 1).
  if [ ! -s "$ROOT/chisel-manifest-${service}.txt" ]; then
    echo "::error::chisel-manifest-${service}.txt is empty; the package-fetch log format may have changed — update the grep pattern in ci/images/run.sh." >&2
    return 1
  fi
  echo "  built ${tag}"
}

# Every container/network name carries the same $$ suffix so two concurrent
# `run.sh smoke` invocations against one daemon (this repo genuinely runs concurrent sessions
# against one checkout) never collide on a fixed literal name.
RUN_ID="$$"
NET="paigasus-smoke-${RUN_ID}"
GW_NAME="smoke-gw-${RUN_ID}"
IAM_NAME="smoke-iam-${RUN_ID}"
PG_NAME="smoke-pg-${RUN_ID}"
CERTPROBE_NAME="certprobe-${RUN_ID}"
cleanup() {
  docker rm -f "$IAM_NAME" "$GW_NAME" "$PG_NAME" "$CERTPROBE_NAME" >/dev/null 2>&1 || true
  docker network rm "$NET" >/dev/null 2>&1 || true
}

# Poll until the container's own HEALTHCHECK reports healthy. This is the ONLY assertion that
# exercises the probe binary INSIDE the shell-less image — an outside-the-container curl passes
# even when HEALTHCHECK is broken.
wait_healthy() {
  local name="$1" i status
  for i in $(seq 1 60); do
    status="$(docker inspect --format '{{.State.Health.Status}}' "$name" 2>/dev/null || echo missing)"
    [ "$status" = "healthy" ] && { echo "  $name is healthy (in-image probe, ${i}s)"; return 0; }
    [ "$status" = "missing" ] && { echo "::error::$name is gone; logs follow" >&2; docker logs "$name" 2>&1 | tail -30 >&2; return 1; }
    sleep 1
  done
  echo "::error::$name never became healthy (last status: $status)" >&2
  docker logs "$name" 2>&1 | tail -30 >&2
  return 1
}

expect_status() {
  local label="$1" url="$2" want="$3" got
  got="$(docker run --rm --network "$NET" curlimages/curl:8.11.1 -s -o /dev/null -w '%{http_code}' "$url" || echo 000)"
  if [ "$got" != "$want" ]; then
    echo "::error::${label}: expected HTTP ${want}, got ${got}" >&2
    return 1
  fi
  echo "  ${label}: HTTP ${got}"
}

# The base must stay the base. Without these a future `FROM ubuntu:24.04` "just to debug
# something" would pass every other assertion in this suite.
assert_base_intact() {
  local image="$1" certs size
  if docker run --rm --entrypoint /bin/sh "$image" -c true >/dev/null 2>&1; then
    echo "::error::${image} has a shell; the runtime base must stay chiseled/scratch." >&2
    return 1
  fi
  docker create --name "$CERTPROBE_NAME" "$image" >/dev/null
  certs="$(docker cp "$CERTPROBE_NAME":/etc/ssl/certs/ca-certificates.crt - 2>/dev/null | tar -xO 2>/dev/null | grep -c 'BEGIN CERTIFICATE' || true)"
  docker rm -f "$CERTPROBE_NAME" >/dev/null
  if [ "${certs:-0}" -lt 100 ]; then
    echo "::error::${image} carries ${certs} CA certificates; the trust bundle is missing or truncated." >&2
    return 1
  fi
  size="$(docker image inspect --format '{{.Size}}' "$image")"
  if [ "$size" -gt 209715200 ]; then
    echo "::error::${image} is ${size} bytes, over the 200 MB ceiling — the runtime base has probably grown." >&2
    return 1
  fi
  echo "  ${image}: no shell, ${certs} CA certs, $((size / 1024 / 1024)) MB"
}

smoke() {
  trap cleanup EXIT
  cleanup
  docker network create "$NET" >/dev/null

  echo "== gateway: standalone =="
  # Runtime-only config (AC-2): env vars ONLY, no mounted file, no --env-file. Success IS the
  # proof. The key is a literal dummy and must never be a real one.
  docker run -d --name "$GW_NAME" --network "$NET" \
    -e GATEWAY_UPSTREAM__OPENAI__API_KEY=sk-smoke-not-a-real-key \
    paigasus-gateway:dev >/dev/null
  wait_healthy "$GW_NAME"
  expect_status "gateway /healthz" "http://${GW_NAME}:8088/healthz" 200
  # The NEGATIVE case is the point: no IAM is reachable, so a /readyz returning 200 is lying.
  expect_status "gateway /readyz (no IAM)" "http://${GW_NAME}:8088/readyz" 503
  # `if !` rather than `cmd; [ $? -eq 1 ]`: under `set -e` a bare non-zero command aborts the
  # script, and exiting 1 here is the EXPECTED result (the gateway is unready without IAM).
  if docker exec "$GW_NAME" /usr/local/bin/paigasus-service healthcheck --path /readyz; then
    echo "::error::gateway readyz probe reported healthy with no IAM reachable" >&2
    return 1
  fi
  echo "  gateway readyz probe exits non-zero while unready (in-image, --path works)"
  assert_base_intact paigasus-gateway:dev

  echo "== iam: with postgres, reached BY HOSTNAME =="
  docker run -d --name "$PG_NAME" --network "$NET" \
    -e POSTGRES_PASSWORD=smoke -e POSTGRES_DB=iam postgres:16-alpine >/dev/null
  sleep 8
  # `$PG_NAME`, never 127.0.0.1: this is what exercises glibc name resolution inside the
  # chiseled rootfs. An IP literal would bypass NSS entirely and the assertion would go vacuous.
  # IAM_API_KEYS__PEPPER: IamConfig::validate requires a base64 pepper decoding to >=32 bytes
  # (ApiKeyConfig::pepper / Pepper::from_config) — boot fails without it. A literal dummy, never
  # a real secret; decodes to 43 bytes.
  docker run -d --name "$IAM_NAME" --network "$NET" \
    -e IAM_DATABASE_URL="postgres://postgres:smoke@${PG_NAME}:5432/iam" \
    -e IAM_AUTHN__ISSUERS='[{issuer="https://idp.example.com",audiences=["paigasus"]}]' \
    -e IAM_API_KEYS__PEPPER="cGFpZ2FzdXMtc21va2UtcGVwcGVyLW5vdC1hLXJlYWwtc2VjcmV0LTAwMA==" \
    paigasus-iam:dev >/dev/null
  wait_healthy "$IAM_NAME"
  expect_status "iam /healthz" "http://${IAM_NAME}:8080/healthz" 200
  expect_status "iam /readyz"  "http://${IAM_NAME}:8080/readyz"  200
  assert_base_intact paigasus-iam:dev

  echo "== runs as the non-root uid it claims =="
  # `docker top`, not `docker inspect .Config.User`: the latter reads IMAGE config, so a
  # `--user 0` invocation would still pass it.
  # `-o pid,uid`, not `-o pid,user` or `-o user` alone: `pid` stays required — some docker
  # engines (observed on Docker Desktop 29.6.2) need it present in the ps format to correlate
  # host processes back to the container and error `Couldn't find PID field in ps output`
  # otherwise — but `user` is resolved through NSS, so on a Linux runner where uid 65532
  # resolves to a synthesized name (e.g. nss-systemd on GitHub-hosted ubuntu-latest) this would
  # print a username instead of "65532" and false-negative CI on a correct image. `uid` is the
  # raw numeric column and is never name-resolved. Do NOT "simplify" this back to `-o user`.
  # `awk '{print $NF}'` takes the last column so the field order doesn't matter.
  for c in "$GW_NAME" "$IAM_NAME"; do
    uid="$(docker top "$c" -o pid,uid 2>/dev/null | tail -1 | awk '{print $NF}')"
    [ "$uid" = "65532" ] || { echo "::error::$c runs as ${uid}, expected 65532" >&2; return 1; }
    echo "  $c runs as uid ${uid}"
  done
  echo "SMOKE OK"
}

cmd="${1:?usage: ci/images/run.sh \{build\|smoke\|all\} [iam|gateway]}"
target="${2:-all}"
services=("iam" "gateway")
[ "$target" != "all" ] && services=("$target")

case "$cmd" in
  build) assert_pins; for s in "${services[@]}"; do build_one "$s"; done ;;
  smoke) smoke ;;
  all)   assert_pins; for s in "${services[@]}"; do build_one "$s"; done; smoke ;;
  *) echo "unknown command: $cmd" >&2; exit 1 ;;
esac
