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
# usage: ci/images/run.sh build [iam|gateway]     # [iam|gateway] scopes the build
#        ci/images/run.sh smoke                    # always smokes BOTH images; takes no service arg
#        ci/images/run.sh all                       # build both + smoke; takes no service arg
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
REGISTRY="${PAIGASUS_IMAGE_REGISTRY:-ghcr.io/smk1085}"
REVISION="$(git -C "$ROOT" rev-parse HEAD)"

# Digest-pinned smoke-test dependencies. This branch's whole design argument is that a floating
# tag is the least-pinned input in a repo that pins everything else, so the smoke path pins its
# own images too rather than trusting `postgres:16-alpine` (which genuinely floats) or
# `curlimages/curl:8.11.1` (tag-immutable in practice, but pin it anyway for consistency). Both
# are the multi-platform manifest-list digest, so the pin resolves on amd64 and arm64 alike.
# Refresh with:
#   docker buildx imagetools inspect postgres:16-alpine --format '{{.Manifest.Digest}}'
#   docker buildx imagetools inspect curlimages/curl:8.11.1 --format '{{.Manifest.Digest}}'
POSTGRES_16_ALPINE_DIGEST="postgres:16-alpine@sha256:cf78e76683b9ca8c5733cbbdce6c9262b45b6767934dd0a95e671f9a0fc20685"
CURL_8_11_1_DIGEST="curlimages/curl:8.11.1@sha256:c1fe1679c34d9784c1b0d1e5f62ac0a79fca01fb6377cdd33e90473c6f9f9a69"

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
# pinned and being nothing of the sort (SMA-500 D3). Three sites must all agree: the toolchain's
# own `channel`, the Dockerfile's `FROM rust:X.Y.Z` tag, and its `ENV RUSTUP_TOOLCHAIN=X.Y.Z` —
# the FROM tag alone is decorative without the ENV line actually pinning what rustup resolves.
assert_pins() {
  local dockerfile="$ROOT/rs/Dockerfile"
  local channel from_version rustup_toolchain
  channel="$(grep -E '^channel[[:space:]]*=' "$ROOT/rs/rust-toolchain.toml" | head -1 | sed -E 's/.*"([^"]+)".*/\1/')"
  from_version="$(grep -oE '^FROM rust:[0-9]+\.[0-9]+\.[0-9]+' "$dockerfile" | head -1 | sed 's/^FROM rust://')"
  if [ "$channel" != "$from_version" ]; then
    echo "::error::rs/Dockerfile's FROM tag (rust:${from_version}) disagrees with rs/rust-toolchain.toml's channel (${channel})." >&2
    echo "  Bump the FROM line (and its digest) together with the toolchain, or the image ships a different compiler." >&2
    return 1
  fi
  # The FROM tag alone does not pin the compiler rustup actually runs — rust-toolchain.toml is
  # inside the build context and rustup honours it over the image. ENV RUSTUP_TOOLCHAIN is what
  # closes that gap, so it must agree with channel/FROM too, or a bump that forgets this one line
  # reintroduces the exact drift the FROM/channel check above exists to prevent.
  rustup_toolchain="$(grep -oE '^ENV RUSTUP_TOOLCHAIN=[0-9]+\.[0-9]+\.[0-9]+' "$dockerfile" | head -1 | sed 's/^ENV RUSTUP_TOOLCHAIN=//')"
  if [ "$channel" != "$rustup_toolchain" ]; then
    echo "::error::rs/Dockerfile's ENV RUSTUP_TOOLCHAIN (${rustup_toolchain:-<missing>}) disagrees with rs/rust-toolchain.toml's channel (${channel})." >&2
    echo "  Bump ENV RUSTUP_TOOLCHAIN together with the FROM line and the toolchain, or the builder can resolve a different compiler than the FROM tag implies." >&2
    return 1
  fi
  # Builder glibc must be <= runtime glibc. bookworm is 2.36, noble (ubuntu:24.04) is 2.39.
  # Inverting it fails at CONTAINER START with `GLIBC_2.4x not found`, not at build time.
  if ! grep -qE '^FROM rust:[0-9.]+-bookworm@sha256:' "$dockerfile"; then
    echo "::error::the builder base must stay a digest-pinned -bookworm tag (glibc 2.36 <= the runtime's 2.39)." >&2
    return 1
  fi
  # The rootfs stage's FROM tag and its `chisel cut --release ubuntu-X.Y` must name the SAME
  # release: chisel cuts package slices out of a specific Ubuntu release manifest, so an
  # ubuntu:25.04 bump that forgot to also bump `--release ubuntu-24.04` would cut 24.04 slices
  # into a 25.04-labelled rootfs — nothing else here or in the smoke suite would notice.
  local ubuntu_from ubuntu_chisel
  ubuntu_from="$(grep -oE '^FROM ubuntu:[0-9]+\.[0-9]+' "$dockerfile" | head -1 | sed 's/^FROM ubuntu://')"
  ubuntu_chisel="$(grep -oE 'chisel cut --release ubuntu-[0-9]+\.[0-9]+' "$dockerfile" | head -1 | sed 's/.*ubuntu-//')"
  if [ "$ubuntu_from" != "$ubuntu_chisel" ]; then
    echo "::error::rs/Dockerfile's FROM tag (ubuntu:${ubuntu_from}) disagrees with its chisel cut --release (ubuntu-${ubuntu_chisel})." >&2
    echo "  Bump both together, or chisel cuts the wrong release's package slices into the rootfs." >&2
    return 1
  fi
  # AC-2: nothing deployment-varying may be baked. Config reaches the container through
  # IAM_*/GATEWAY_* env at RUNTIME only. Join `\`-continued lines first: an ENV instruction can
  # spread its assignments across multiple physical lines, and IAM_/GATEWAY_ can appear as the
  # 2nd+ token on either the first or a continuation line, not only as the token right after
  # `ENV` — a naive single-line "starts with ENV IAM_/GATEWAY_" match misses both. The
  # instruction match (`[Ee][Nn][Vv]`) is case-insensitive because Docker parses instructions
  # case-insensitively (`env IAM_DATABASE_URL=...` is a valid, equivalent ENV instruction); the
  # variable-name alternation stays case-SENSITIVE on purpose — only IAM_/GATEWAY_ are the
  # repo's actual env prefixes, and lower-casing that half would just as easily hide unrelated
  # matches.
  local joined_env
  joined_env="$(awk '/\\[[:space:]]*$/ { sub(/\\[[:space:]]*$/, " "); printf "%s", $0; next } { print }' "$dockerfile")"
  if grep -nE '^[[:space:]]*[Ee][Nn][Vv][[:space:]]+.*(IAM_|GATEWAY_)' <<<"$joined_env"; then
    echo "::error::rs/Dockerfile bakes service config into the image; configure at runtime via env instead." >&2
    return 1
  fi
  # AC-2 continued, the bigger hole: the ENV guard above only sees baked *env* config. A
  # `COPY iam.toml /iam.toml` into the final stage would bake config just the same, pass the ENV
  # guard, AND pass the smoke suite — a baked TOML layers *beneath* runtime env (figment's
  # documented merge order), so nothing observable changes. The final (last, unnamed
  # `FROM scratch`) stage may therefore COPY exactly two things: the chiseled rootfs and the
  # compiled service binary. Anything else fails closed here rather than shipping silently.
  local final_from_line final_stage copy_lines bad_copy
  final_from_line="$(grep -niE '^FROM[[:space:]]' "$dockerfile" | tail -1 | cut -d: -f1)"
  final_stage="$(sed -n "${final_from_line},\$p" "$dockerfile")"
  copy_lines="$(grep -iE '^[[:space:]]*COPY[[:space:]]+' <<<"$final_stage")"
  bad_copy="$(grep -vxE 'COPY --from=rootfs /rootfs /|COPY --from=builder /out/service /usr/local/bin/paigasus-service' <<<"$copy_lines" || true)"
  if [ -n "$bad_copy" ]; then
    echo "::error::rs/Dockerfile's final stage COPYs something beyond the rootfs and the service binary — this can bake deployment config beneath runtime env, invisible to both the ENV check above and the smoke suite:" >&2
    echo "$bad_copy" >&2
    return 1
  fi
  # SMA-559: a replica that loses the migration-lock race waits with NO listener bound, so the
  # image's start period must cover that wait plus the migration itself. A config default raised
  # without touching the Dockerfile would silently re-arm the restart-while-waiting bug.
  local start_period lock_wait budget required
  start_period="$(grep -oE '\-\-start-period=[0-9]+s' "$dockerfile" | head -1 | grep -oE '[0-9]+')"
  lock_wait="$(grep -oE 'lock_wait_secs: [0-9]+' "$ROOT/rs/crates/services/paigasus-iam/src/config.rs" | head -1 | grep -oE '[0-9]+')"
  budget="$(grep -oE 'MIGRATION_BUDGET_SECS: u64 = [0-9]+' "$ROOT/rs/crates/services/paigasus-iam/src/adapters/persistence/migration_lock.rs" | head -1 | grep -oE '[0-9]+$')"
  if [ -z "$start_period" ] || [ -z "$lock_wait" ] || [ -z "$budget" ]; then
    echo "::error::could not read the start-period/lock-wait/migration-budget triple (start_period=${start_period:-<missing>} lock_wait=${lock_wait:-<missing>} budget=${budget:-<missing>}); one of the grep anchors moved." >&2
    return 1
  fi
  required=$((lock_wait + budget))
  if [ "$start_period" -lt "$required" ]; then
    echo "::error::rs/Dockerfile's HEALTHCHECK --start-period=${start_period}s is below migration.lock_wait_secs (${lock_wait}) + the migration budget (${budget}) = ${required}s." >&2
    echo "  A replica waiting on the SMA-559 migration lock binds no listener, so it would be reported unhealthy while correctly waiting. Raise the start period or lower the default wait." >&2
    return 1
  fi
  echo "  pins OK: rustc ${channel}, bookworm builder, ubuntu ${ubuntu_from} == chisel release, no baked service config, start-period ${start_period}s >= ${required}s"
}

build_one() {
  local service="$1" crate tag build_log
  crate="$(crate_for "$service")"
  tag="${REGISTRY}/${crate}:${REVISION}"
  # mktemp, not a fixed /tmp/paigasus-build-${service}.log: a predictable path in a
  # world-writable directory, written with `tee`, is a symlink-attack target. Reused for both
  # the `tee` below and the chisel-manifest grep, then removed once both are done. The X's stay
  # at the very end of the template (no trailing suffix after them): BSD/macOS mktemp only
  # substitutes a run of trailing X's, unlike GNU mktemp which also accepts one after them.
  build_log="$(mktemp "${TMPDIR:-/tmp}/paigasus-build-${service}.XXXXXX")"
  trap 'rm -f "$build_log"' RETURN
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
  # --load: docker/setup-buildx-action makes a `docker-container` builder CURRENT, and that
  # driver does not reliably auto-load its output into the local `docker images` store on every
  # Docker version (it happens to on 29.6.2, but the CI runner's version is not guaranteed to
  # match). Without --load the failure mode is silent here and loud at the first `docker run`
  # below ("No such image"). Under the plain `docker` driver (no buildx container) --load is a
  # no-op-safe `--output=type=docker`, so it costs nothing locally.
  docker build \
    --progress=plain \
    --no-cache-filter=rootfs \
    --load \
    -f "$ROOT/rs/Dockerfile" \
    --build-arg "BIN=${crate}" \
    --label "org.opencontainers.image.title=${crate}" \
    --label "org.opencontainers.image.description=Paigasus ${service} service" \
    --label "org.opencontainers.image.source=https://github.com/SMK1085/paigasus-core" \
    --label "org.opencontainers.image.revision=${REVISION}" \
    --label "org.opencontainers.image.licenses=Apache-2.0" \
    -t "$tag" -t "${crate}:dev" \
    "$ROOT/rs" 2>&1 | tee "$build_log"
  grep -oE 'Fetching pool/[^ ]+\.deb' "$build_log" | sort -u > "$ROOT/chisel-manifest-${service}.txt" || true
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
  got="$(docker run --rm --network "$NET" "$CURL_8_11_1_DIGEST" -s -o /dev/null -w '%{http_code}' "$url" || echo 000)"
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
  # Capture the ACTUAL exit code rather than treating any non-zero as proof: the healthcheck
  # subcommand's contract is 0=healthy, 1=unhealthy, 2=usage error (§ D4 in the design doc). If
  # `--path` were ever renamed, the binary would exit 2 (usage error) and a bare `if !` would
  # read that as "correctly reported unready" — the only assertion proving `--path` works would
  # have silently stopped proving it. Require exactly 1.
  local readyz_rc=0
  docker exec "$GW_NAME" /usr/local/bin/paigasus-service healthcheck --path /readyz || readyz_rc=$?
  case "$readyz_rc" in
    1) echo "  gateway readyz probe exits 1 (unhealthy) while unready (in-image, --path works)" ;;
    0) echo "::error::gateway readyz probe reported healthy (exit 0) with no IAM reachable" >&2; return 1 ;;
    2) echo "::error::gateway readyz probe exited 2 (usage error) — --path was rejected, so this no longer proves --path works" >&2; return 1 ;;
    *) echo "::error::gateway readyz probe exited ${readyz_rc}, expected exactly 1 (unhealthy)" >&2; return 1 ;;
  esac
  assert_base_intact paigasus-gateway:dev

  echo "== iam: with postgres, reached BY HOSTNAME =="
  # --health-cmd/--health-interval + wait_healthy, not a fixed `sleep`: sea-orm's
  # Database::connect does not retry, so IAM's own boot attempt must land AFTER postgres is
  # actually accepting connections, not after a guessed wait. A fixed `sleep 8` either wastes
  # time on a fast runner or, worse, is too short on a slow one — IAM would exit before postgres
  # is ready, and wait_healthy on IAM would then burn its own 60s budget reporting a probe
  # failure that was really "postgres wasn't up yet".
  docker run -d --name "$PG_NAME" --network "$NET" \
    --health-cmd 'pg_isready -U postgres' --health-interval=1s \
    -e POSTGRES_PASSWORD=smoke -e POSTGRES_DB=iam "$POSTGRES_16_ALPINE_DIGEST" >/dev/null
  wait_healthy "$PG_NAME"
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

cmd="${1:?usage: ci/images/run.sh build [iam|gateway] | ci/images/run.sh \{smoke\|all\}}"
target="${2:-}"
services=("iam" "gateway")
[ -n "$target" ] && services=("$target")

# `smoke` and `all` always exercise BOTH images (§ 5.2: the negative case on the gateway needs
# no IAM reachable, and the positive case needs IAM's own postgres) — a service argument on
# either of them is silently ignored by `build_one`'s scoping but NOT by `smoke`, which has no
# way to honour it. `run.sh all iam` would then build only iam while still smoke-testing
# whatever `paigasus-gateway:dev` happens to already be on the daemon (stale or absent), and
# report SMOKE OK regardless. Reject the argument outright rather than let it lie.
case "$cmd" in
  build) assert_pins; for s in "${services[@]}"; do build_one "$s"; done ;;
  smoke)
    if [ -n "$target" ]; then
      echo "usage: ci/images/run.sh smoke takes no service argument — it always smokes both images" >&2
      exit 1
    fi
    smoke
    ;;
  all)
    if [ -n "$target" ]; then
      echo "usage: ci/images/run.sh all takes no service argument — it builds and smokes both images; use 'build [iam|gateway]' to build one" >&2
      exit 1
    fi
    assert_pins
    for s in "${services[@]}"; do build_one "$s"; done
    smoke
    ;;
  *) echo "unknown command: $cmd" >&2; exit 1 ;;
esac
