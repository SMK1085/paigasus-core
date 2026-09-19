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
#        ci/images/run.sh smoke [iam|gateway]...   # no argument: both images; else exactly those
#        ci/images/run.sh all                       # build both + smoke; takes no service arg
#        ci/images/run.sh build-oci <iam|gateway> <outdir>   # SMA-658: OCI archive, no --load
#        ci/images/run.sh load-oci <archive> <image-name>     # load + identity check (prints M3)
#        ci/images/run.sh rehearse <archive.oci.tar>...      # SMA-658: publish steps vs two local registries
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"
REGISTRY="${PAIGASUS_IMAGE_REGISTRY:-ghcr.io/smk1085}"
REVISION="$(git -C "$ROOT" rev-parse HEAD)"

# proto prints an NDJSON preamble on STDOUT inside an agent session, and that poisons every
# `$(...)` capture of a shimmed tool such as `uv` (CLAUDE.md, SMA-609). Exported once here so every
# capture below inherits it.
export PROTO_REPORTER=text

# The release decisions live in release_decision.py so a self-test can prove them (SMA-658).
# Standard library only: no project, no lock, any Python >= 3.12 that uv can find.
decide() {
  uv run --no-project --python '>=3.12' python3 "$HERE/release_decision.py" "$@"
}

# kv "<key=value lines>" <key> — the value of one key, or an empty string.
kv() {
  printf '%s\n' "$1" | sed -n "s/^$2=//p"
}

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

# SMA-658: the two throwaway registries that `rehearse` pushes to. Refresh with:
#   docker buildx imagetools inspect registry:2 --format '{{.Manifest.Digest}}'
REGISTRY_2_DIGEST="registry:2@sha256:a3d8aaa63ed8681a604f1dea0aa03f100d5895b6a58ace528858a7b332415373"

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
  channel="$(grep -E '^channel[[:space:]]*=' "$ROOT/rs/rust-toolchain.toml" | sed -n 1p | sed -E 's/.*"([^"]+)".*/\1/')"
  from_version="$(grep -oE '^FROM rust:[0-9]+\.[0-9]+\.[0-9]+' "$dockerfile" | sed -n 1p | sed 's/^FROM rust://')"
  if [ "$channel" != "$from_version" ]; then
    echo "::error::rs/Dockerfile's FROM tag (rust:${from_version}) disagrees with rs/rust-toolchain.toml's channel (${channel})." >&2
    echo "  Bump the FROM line (and its digest) together with the toolchain, or the image ships a different compiler." >&2
    return 1
  fi
  # The FROM tag alone does not pin the compiler rustup actually runs — rust-toolchain.toml is
  # inside the build context and rustup honours it over the image. ENV RUSTUP_TOOLCHAIN is what
  # closes that gap, so it must agree with channel/FROM too, or a bump that forgets this one line
  # reintroduces the exact drift the FROM/channel check above exists to prevent.
  rustup_toolchain="$(grep -oE '^ENV RUSTUP_TOOLCHAIN=[0-9]+\.[0-9]+\.[0-9]+' "$dockerfile" | sed -n 1p | sed 's/^ENV RUSTUP_TOOLCHAIN=//')"
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
  ubuntu_from="$(grep -oE '^FROM ubuntu:[0-9]+\.[0-9]+' "$dockerfile" | sed -n 1p | sed 's/^FROM ubuntu://')"
  ubuntu_chisel="$(grep -oE 'chisel cut --release ubuntu-[0-9]+\.[0-9]+' "$dockerfile" | sed -n 1p | sed 's/.*ubuntu-//')"
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
  local start_period
  start_period="$(grep -oE '\-\-start-period=[0-9]+s' "$dockerfile" | sed -n 1p | grep -oE '[0-9]+' || true)"
  if [ -z "$start_period" ]; then
    echo "::error::could not read the HEALTHCHECK --start-period; the grep anchor moved, or the HEALTHCHECK was removed." >&2
    return 1
  fi
  # SMA-571 removed the start-period <-> lock_wait_secs coupling: IAM binds before migrating, so
  # this only has to cover config load + Database::connect + the binds. A floor is kept so that
  # deleting the HEALTHCHECK, or setting --start-period=0s, is still caught — after the coupling
  # was removed, nothing else reads this value at all.
  if [ "$start_period" -lt 30 ]; then
    echo "::error::rs/Dockerfile's HEALTHCHECK --start-period=${start_period}s is below the 30s floor." >&2
    return 1
  fi
  echo "  pins OK: rustc ${channel}, bookworm builder, ubuntu ${ubuntu_from} == chisel release, no baked service config, start-period ${start_period}s >= 30s"
}

# Writes the chisel package list that a build log names into $2, and fails when it is empty
# (SMA-500 fix-round 1: an empty manifest answers nothing when someone asks which libc shipped).
extract_chisel_manifest() {
  local build_log="$1" out="$2"
  grep -oE 'Fetching pool/[^ ]+\.deb' "$build_log" | sort -u > "$out" || true
  if [ ! -s "$out" ]; then
    echo "::error::$(basename "$out") is empty; the package-fetch log format may have changed — update the grep pattern in ci/images/run.sh." >&2
    return 1
  fi
}

# The version line of a service crate's own Cargo.toml. Both services carry a literal version
# (not `version.workspace = true`), which is what the image's version label must equal.
version_for() {
  local crate="$1" v
  v="$(sed -n 's/^version = "\([0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*\)"$/\1/p' "$ROOT/rs/crates/services/${crate}/Cargo.toml" | sed -n 1p)"
  if [ -z "$v" ]; then
    echo "::error::no literal version line in rs/crates/services/${crate}/Cargo.toml" >&2
    return 1
  fi
  echo "$v"
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
  extract_chisel_manifest "$build_log" "$ROOT/chisel-manifest-${service}.txt"
  echo "  built ${tag}"
}

# SMA-658: the release build. The same image as build_one, exported as an OCI ARCHIVE instead of
# being loaded, so its bytes (and so its digest) are fixed before anything is pushed.
# --provenance=false --sbom=false: buildx would otherwise wrap the image in an index with its own
# attestation manifests; the release path attests through GitHub instead (spec D4).
# name=<crate>:dev: `docker load` of the archive then restores that name for load_oci and smoke.
build_oci() {
  local service="$1" outdir="$2" crate version arch archive build_log
  crate="$(crate_for "$service")"
  version="$(version_for "$crate")"
  arch="$(docker version --format '{{.Server.Arch}}')"
  mkdir -p "$outdir"
  archive="${outdir}/${crate}-${arch}.oci.tar"
  build_log="$(mktemp "${TMPDIR:-/tmp}/paigasus-build-${service}.XXXXXX")"
  trap 'rm -f "$build_log"' RETURN
  echo "== build-oci ${crate} ${version} (${arch}) =="
  docker build \
    --progress=plain \
    --no-cache-filter=rootfs \
    --provenance=false --sbom=false \
    --output "type=oci,dest=${archive},name=${crate}:dev" \
    -f "$ROOT/rs/Dockerfile" \
    --build-arg "BIN=${crate}" \
    --label "org.opencontainers.image.title=${crate}" \
    --label "org.opencontainers.image.description=Paigasus ${service} service" \
    --label "org.opencontainers.image.source=https://github.com/SMK1085/paigasus-core" \
    --label "org.opencontainers.image.revision=${REVISION}" \
    --label "org.opencontainers.image.version=${version}" \
    --label "org.opencontainers.image.licenses=Apache-2.0" \
    "$ROOT/rs" 2>&1 | tee "$build_log"
  extract_chisel_manifest "$build_log" "$ROOT/chisel-manifest-${service}-${arch}.txt"
  echo "  built ${archive}"
}

# SMA-658 spec § 4.2: the image the smoke suite tests must be the image in the archive. What
# `docker load` reports as the image ID depends on the daemon's image store (measured M3, local
# Docker 29.8): the containerd store gives the MANIFEST digest, the classic store gives the CONFIG
# digest. So the check accepts the one value that matches this daemon's store, and prints every
# value once per runner so CI records M3 for each runner label.
load_oci() {
  local archive="$1" name="$2" digests manifest config store driver loaded size expected
  digests="$(decide oci-digests "$archive")"
  manifest="$(kv "$digests" manifest)"
  config="$(kv "$digests" config)"
  docker load -i "$archive" >/dev/null
  loaded="$(docker image inspect --format '{{.Id}}' "$name")"
  size="$(docker image inspect --format '{{.Size}}' "$name")"
  driver="$(docker info --format '{{json .DriverStatus}}')"
  store="classic"
  case "$driver" in *io.containerd.snapshotter*) store="containerd" ;; esac
  expected="$config"
  [ "$store" = "containerd" ] && expected="$manifest"
  echo "M3 arch=$(docker version --format '{{.Server.Arch}}') docker=$(docker version --format '{{.Server.Version}}') store=${store} loaded_id=${loaded} manifest=${manifest} config=${config} size=${size}"
  if [ "$loaded" != "$expected" ]; then
    echo "::error::${name} loaded as ${loaded}, but the ${store} store should report ${expected}: the loaded image is not the archive's image." >&2
    return 1
  fi
  echo "  ${name} is the archive's image (${store} store)"
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

# Bounds a single Docker CLI invocation to at most `$1` seconds, so a hung `docker inspect`/
# `docker run`/`docker logs` (daemon wedged, control-plane stall) cannot itself carry `wait_ready`
# past its own advertised deadline the way an unbounded call would.
#
# GNU `timeout` would do this in one word and IS present on the `ubuntu-latest` runner this
# script's CI caller uses — but this script is not CI-only: `ci/images/run.sh all` run locally is
# the ONLY pre-merge exercise of this file (`images.yml` is not a required check), and macOS ships
# no `timeout` binary at all (a documented constraint in this repo's CLAUDE.md). Reaching for it
# here would pass on CI and break every local run silently until someone tried one on a Mac. Do
# NOT "simplify" this back to `timeout "$@"` for that reason.
#
# Implemented as background job + a watchdog that escalates TERM then KILL, which needs nothing
# beyond POSIX job control (`&`, `wait`, `kill`) and behaves identically under bash on macOS and
# Linux.
with_deadline() {
  local secs="$1"
  shift
  [ "$secs" -lt 1 ] && secs=1
  "$@" &
  local cmd_pid=$!
  (
    sleep "$secs"
    kill -TERM "$cmd_pid" 2>/dev/null
    sleep 1
    kill -KILL "$cmd_pid" 2>/dev/null
  ) &
  local watchdog_pid=$!
  local rc=0
  wait "$cmd_pid" 2>/dev/null || rc=$?
  # The watchdog either already fired (harmless double-kill of a dead pid) or is still sleeping —
  # either way it must be reaped so it cannot fire a stray kill at a LATER, unrelated pid that the
  # OS has since recycled onto `$cmd_pid`'s old number.
  kill "$watchdog_pid" 2>/dev/null || true
  wait "$watchdog_pid" 2>/dev/null || true
  return "$rc"
}

# Caps `$1` at `$2`, floored at 1 — the shared shape every per-call Docker timeout below uses so
# no single invocation is ever handed more of the budget than `wait_ready` has left.
cap_timeout() {
  local v="$1" ceiling="$2"
  [ "$v" -gt "$ceiling" ] && v="$ceiling"
  [ "$v" -lt 1 ] && v=1
  echo "$v"
}

# SMA-571: `healthy` no longer implies migrated — IAM binds before it migrates, so /healthz
# answers 200 while /readyz is still 503 "migrating". Without this the very next assertion races
# a fresh database's full migration set. Its own budget, deliberately separate from wait_healthy's.
#
# Mirrors wait_healthy's "container is gone" early-out (SMA-571 final review): the most likely
# NEW way this loop fails is a migration failure AFTER `healthy` — the boot-phase drain now runs
# and the container exits 1 — and without this check that burns the full budget (each iteration
# spawning a `docker run`) before ever reporting anything more useful than "last /readyz status:
# 000".
#
# Driven off an ABSOLUTE deadline (CodeRabbit, PR 167), not an iteration count: the old
# `seq 1 120` + `sleep 1` shape claimed a "never became ready" 120s budget in its error message,
# but each iteration also spends up to `--max-time 5` on the request itself — so a run where every
# request times out takes up to 120 * (5 + 1) =~ 12 minutes wall-clock, six times what the message
# says. `$SECONDS` (reset at function entry) tracks elapsed wall-clock directly; each request's
# `--max-time` and the trailing `sleep` are both capped at whatever of the budget remains, so
# neither can itself carry the loop past the deadline it is supposed to enforce.
#
# CodeRabbit round 2 (PR 167): that deadline bounded the LOOP but not the individual Docker calls
# inside it — `docker inspect`, the `docker run` issuing the request, and the diagnostic
# `docker logs` were all unbounded, so any ONE hung Docker call could overshoot the advertised
# budget on its own regardless of how carefully the loop re-checked `remaining` around it. Every
# Docker invocation below is now re-checked against the CURRENT `remaining` immediately before it
# runs (so a call that cannot fit in what is left is skipped rather than started) and wrapped in
# `with_deadline` so a call that DOES start cannot itself outlive the budget.
wait_ready() {
  local name="$1" url="$2" budget="${3:-120}" code=000 status start elapsed remaining req_timeout sleep_for insp_timeout run_timeout
  start=$SECONDS
  while :; do
    elapsed=$((SECONDS - start))
    remaining=$((budget - elapsed))
    [ "$remaining" -le 0 ] && break
    # Diagnostic control-plane call — normally instant, so capped at a small fixed ceiling
    # (never more than 5s of the budget) rather than however much of `remaining` happens to be
    # left, so a wedged daemon still yields several retries instead of burning the whole budget
    # on one inspect.
    insp_timeout="$(cap_timeout "$remaining" 5)"
    status="$(with_deadline "$insp_timeout" docker inspect --format '{{.State.Status}}' "$name" 2>/dev/null || echo missing)"
    if [ "$status" != "running" ]; then
      echo "::error::$name is gone (status: $status); logs follow" >&2
      remaining=$((budget - (SECONDS - start)))
      with_deadline "$(cap_timeout "$remaining" 10)" docker logs "$name" 2>&1 | tail -30 >&2
      return 1
    fi
    remaining=$((budget - (SECONDS - start)))
    [ "$remaining" -le 0 ] && break
    # `--connect-timeout`/`--max-time`: this is a POLLING loop, so a single hung request would
    # otherwise stall past the deadline rather than just costing one iteration. Capped at whatever
    # of the budget remains (and never below 1s), not a fixed 5s.
    req_timeout=$((remaining < 5 ? remaining : 5))
    [ "$req_timeout" -lt 1 ] && req_timeout=1
    # `docker run`'s OWN startup (image already pulled, but the daemon still has to create and
    # start a container) is outside curl's `--max-time`, so the `docker run` invocation itself is
    # bounded too — a few seconds' allowance over `req_timeout` for that overhead, still capped at
    # whatever of the budget remains.
    run_timeout="$(cap_timeout $((req_timeout + 3)) "$remaining")"
    code="$(with_deadline "$run_timeout" docker run --rm --network "$NET" "$CURL_8_11_1_DIGEST" -s --connect-timeout 2 --max-time "$req_timeout" -o /dev/null -w '%{http_code}' "$url" || echo 000)"
    [ "$code" = "200" ] && { echo "  $name is ready (${elapsed}s)"; return 0; }
    remaining=$((budget - (SECONDS - start)))
    [ "$remaining" -le 0 ] && break
    sleep_for=$((remaining < 1 ? remaining : 1))
    sleep "$sleep_for"
  done
  echo "::error::$name never became ready within ${budget}s (last /readyz status: $code)" >&2
  with_deadline 10 docker logs "$name" 2>&1 | tail -30 >&2
  return 1
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

# The image under test must be the one THIS checkout built. This replaces the old rule that
# `smoke` took no service argument (SMA-500): the danger was that `smoke` would test a stale or
# absent image of the OTHER service and still report SMOKE OK. A per-service smoke never touches
# the other service's image, and this check refuses a stale image of the service it does test.
assert_fresh() {
  local image="$1" rev
  rev="$(docker image inspect --format '{{index .Config.Labels "org.opencontainers.image.revision"}}' "$image" 2>/dev/null || true)"
  if [ "$rev" != "$REVISION" ]; then
    echo "::error::${image} carries revision '${rev:-<none>}', expected ${REVISION}: a stale or absent image would be smoke-tested." >&2
    return 1
  fi
}

smoke_gateway() {
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
}

smoke_iam() {
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
  wait_ready "$IAM_NAME" "http://${IAM_NAME}:8080/readyz"
  expect_status "iam /healthz" "http://${IAM_NAME}:8080/healthz" 200
  expect_status "iam /readyz"  "http://${IAM_NAME}:8080/readyz"  200
  assert_base_intact paigasus-iam:dev
}

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
assert_uid() {
  local c="$1" uid
  uid="$(docker top "$c" -o pid,uid 2>/dev/null | tail -1 | awk '{print $NF}')"
  [ "$uid" = "65532" ] || { echo "::error::$c runs as ${uid}, expected 65532" >&2; return 1; }
  echo "  $c runs as uid ${uid}"
}

smoke() {
  local s
  trap cleanup EXIT
  cleanup
  docker network create "$NET" >/dev/null
  for s in "$@"; do
    case "$s" in
      gateway) assert_fresh paigasus-gateway:dev; smoke_gateway; assert_uid "$GW_NAME" ;;
      iam)     assert_fresh paigasus-iam:dev;     smoke_iam;     assert_uid "$IAM_NAME" ;;
      *) echo "unknown service: $s" >&2; return 1 ;;
    esac
  done
  echo "SMOKE OK ($*)"
}

# --- rehearse (SMA-658 spec § 8) -----------------------------------------------------------------
# Runs the publish sequence of the future release path against two LOCAL registries: A stands in
# for GHCR and B for Docker Hub. It needs no credential, so images.yml runs it on a pull request.
# It proves the parts that do not need OIDC or a secret: push by digest, the index, the
# digest-preserving copy, the D10 adoption rule, the conflict refusal and the floating-tag rule.
# The decisions come from release_decision.py, so PR 2 runs the SAME decision code; the registry
# commands here are a copy of PR 2's sequence, which is the residual (spec § 7.1 wants them
# literal in release.yml).
REH_A_NAME="rehearse-a-${RUN_ID}"
REH_B_NAME="rehearse-b-${RUN_ID}"
REH_TMP=""

rehearse_cleanup() {
  docker rm -f "$REH_A_NAME" "$REH_B_NAME" >/dev/null 2>&1 || true
  if [ -n "$REH_TMP" ]; then rm -rf "$REH_TMP"; fi
}

rh_fail() {
  echo "::error::rehearse: $*" >&2
  return 1
}

# Starts a registry:2 on a free localhost port and prints `localhost:<port>`. `localhost`, not
# 127.0.0.1, because crane and buildx both treat a `localhost` registry as plain HTTP.
start_registry() {
  local name="$1" hostport port i
  docker run -d --name "$name" -p 127.0.0.1::5000 "$REGISTRY_2_DIGEST" >/dev/null
  hostport="$(docker port "$name" 5000/tcp | sed -n 1p)"
  port="${hostport##*:}"
  for i in $(seq 1 30); do
    if crane catalog --insecure "localhost:${port}" >/dev/null 2>&1; then
      echo "localhost:${port}"
      return 0
    fi
    sleep 1
  done
  rh_fail "registry ${name} never answered on localhost:${port} (${i}s)"
}

# The digest that `$1:$2` resolves to, or `none` when the tag does not exist. Any OTHER failure
# (a network error, a registry 5xx, auth) is fatal: reading it as "absent" would let a real
# release push a second digest under a published version (spec D10). PR 2 keeps this rule.
tag_digest() {
  local ref="$1:$2" out rc=0
  out="$(crane digest --insecure "$ref" 2>&1)" || rc=$?
  if [ "$rc" -eq 0 ]; then
    printf '%s\n' "$out"
    return 0
  fi
  case "$out" in
    *MANIFEST_UNKNOWN*|*NAME_UNKNOWN*) echo "none" ;;
    *) rh_fail "cannot read ${ref}: ${out}" ;;
  esac
}

expect_kv() {
  local got
  got="$(kv "$1" "$2")"
  [ "$got" = "$3" ] || rh_fail "expected $2=$3, got $2=${got:-<empty>}"
}

expect_tag() {
  local got
  got="$(tag_digest "$1" "$2")"
  [ "$got" = "$3" ] || rh_fail "$1:$2 is ${got}, expected $3"
}

rehearse() {
  if [ "$#" -lt 1 ]; then
    echo "usage: ci/images/run.sh rehearse <archive.oci.tar>..." >&2
    return 1
  fi
  if ! command -v crane >/dev/null 2>&1; then
    echo "::error::crane is not on PATH; run 'proto install crane'" >&2
    return 2
  fi
  trap rehearse_cleanup EXIT
  REH_TMP="$(mktemp -d "${TMPDIR:-/tmp}/paigasus-rehearse.XXXXXX")"
  local a b repo_a repo_b archive digests manifest platform arch refs first index index2 rebuilt ga gb out rc r t
  refs=()
  a="$(start_registry "$REH_A_NAME")"
  b="$(start_registry "$REH_B_NAME")"
  repo_a="${a}/paigasus-rehearse"
  repo_b="${b}/paigasus-rehearse"

  echo "== rehearse: push each platform to A and keep its digest =="
  for archive in "$@"; do
    digests="$(decide oci-digests "$archive")"
    manifest="$(kv "$digests" manifest)"
    platform="$(kv "$digests" platform)"
    arch="${platform#*/}"
    mkdir -p "$REH_TMP/$arch"
    tar -xf "$archive" -C "$REH_TMP/$arch"
    crane push --insecure "$REH_TMP/$arch" "${repo_a}:${REVISION}-${arch}" >/dev/null
    expect_tag "$repo_a" "${REVISION}-${arch}" "$manifest"
    refs+=("${repo_a}@${manifest}")
    echo "  ${arch}: ${manifest}"
  done
  first="${refs[0]#*@}"
  docker buildx imagetools create --tag "${repo_a}:${REVISION}" "${refs[@]}"
  index="$(tag_digest "$repo_a" "$REVISION")"
  echo "  index: ${index}"

  echo "== case 1: nothing published -> push-new, copy to B, tag both =="
  ga="$(tag_digest "$repo_a" 0.1.0)"
  gb="$(tag_digest "$repo_b" 0.1.0)"
  out="$(decide adopt --new-digest "$index" --ghcr "$ga" --dockerhub "$gb" --git-tag absent)"
  expect_kv "$out" action push-new
  docker buildx imagetools create --tag "${repo_b}:${REVISION}" "${repo_a}@${index}"
  expect_tag "$repo_b" "$REVISION" "$index"
  : > "$REH_TMP/tags"
  out="$(decide floating --service gateway --version 0.1.0 --tags-file "$REH_TMP/tags")"
  expect_kv "$out" move true
  for r in "$repo_a" "$repo_b"; do
    crane tag --insecure "${r}@${index}" 0.1.0
    crane tag --insecure "${r}@${index}" "$(kv "$out" minor_tag)"
    crane tag --insecure "${r}@${index}" latest
    for t in 0.1.0 0.1 latest; do expect_tag "$r" "$t" "$index"; done
  done

  echo "== case 2: a rebuild of a published version -> adopt the first digest, overwrite nothing =="
  crane mutate --insecure "${repo_a}@${first}" --label org.opencontainers.image.description=rehearsal-rebuild -t "${repo_a}:rebuild" >/dev/null
  rebuilt="$(tag_digest "$repo_a" rebuild)"
  [ "$rebuilt" != "$first" ] || rh_fail "the rebuild kept the first digest; the case proves nothing"
  docker buildx imagetools create --tag "${repo_a}:rebuild-index" "${repo_a}@${rebuilt}"
  index2="$(tag_digest "$repo_a" rebuild-index)"
  ga="$(tag_digest "$repo_a" 0.1.0)"
  gb="$(tag_digest "$repo_b" 0.1.0)"
  out="$(decide adopt --new-digest "$index2" --ghcr "$ga" --dockerhub "$gb" --git-tag absent)"
  expect_kv "$out" action adopt
  expect_kv "$out" digest "$index"
  expect_kv "$out" copy_to none
  # A real "overwrite nothing" claim needs a write that COULD have happened. Gate a tag move to
  # the rebuild's index behind the same condition PR 2 will use, so a branch that pushed
  # unconditionally would move the tag to $index2 and the assertion below would catch it.
  if [ "$(kv "$out" action)" = push-new ]; then
    for r in "$repo_a" "$repo_b"; do crane tag --insecure "${r}@${index2}" 0.1.0; done
  fi
  expect_tag "$repo_a" 0.1.0 "$index"
  expect_tag "$repo_b" 0.1.0 "$index"

  echo "== case 3: only A holds the version -> adopt A's digest and copy it to B =="
  crane tag --insecure "${repo_a}@${index2}" 0.1.1
  ga="$(tag_digest "$repo_a" 0.1.1)"
  gb="$(tag_digest "$repo_b" 0.1.1)"
  out="$(decide adopt --new-digest "$index" --ghcr "$ga" --dockerhub "$gb" --git-tag absent)"
  expect_kv "$out" action adopt
  expect_kv "$out" digest "$index2"
  expect_kv "$out" copy_to dockerhub
  docker buildx imagetools create --tag "${repo_b}:0.1.1" "${repo_a}@${index2}"
  expect_tag "$repo_b" 0.1.1 "$index2"

  echo "== case 4: the registries disagree -> refuse (exit 3) =="
  crane tag --insecure "${repo_a}@${index}" 0.1.2
  docker buildx imagetools create --tag "${repo_b}:0.1.2" "${repo_a}@${index2}"
  ga="$(tag_digest "$repo_a" 0.1.2)"
  gb="$(tag_digest "$repo_b" 0.1.2)"
  rc=0
  out="$(decide adopt --new-digest "$index" --ghcr "$ga" --dockerhub "$gb" --git-tag absent)" || rc=$?
  [ "$rc" -eq 3 ] || rh_fail "a digest conflict must exit 3, got ${rc}"
  expect_kv "$out" action conflict

  echo "== case 5: an older version than a released one -> the floating tags hold =="
  printf 'abc123\trefs/tags/paigasus-gateway-v0.2.0\n' > "$REH_TMP/tags"
  out="$(decide floating --service gateway --version 0.1.1 --tags-file "$REH_TMP/tags")"
  expect_kv "$out" move false
  # Same reasoning as case 2: gate the case-1 tag-move loop behind the real condition, using
  # $index2 (already known to differ from $index) as the would-be new value, so a branch that
  # moved the tags unconditionally would be caught below.
  if [ "$(kv "$out" move)" = true ]; then
    for r in "$repo_a" "$repo_b"; do
      crane tag --insecure "${r}@${index2}" 0.1.1
      crane tag --insecure "${r}@${index2}" "$(kv "$out" minor_tag)"
      crane tag --insecure "${r}@${index2}" latest
    done
  fi
  expect_tag "$repo_a" latest "$index"
  expect_tag "$repo_b" latest "$index"

  echo "REHEARSE OK"
}

# One usage string for both the missing-command case and the unknown-command case below, so the
# two never drift apart. Lists every command the case block accepts, in the order it accepts them.
USAGE="usage: ci/images/run.sh build [iam|gateway] | ci/images/run.sh build-oci <iam|gateway> <outdir> | ci/images/run.sh load-oci <archive> <image-name> | ci/images/run.sh smoke [iam|gateway]... | ci/images/run.sh all | ci/images/run.sh rehearse <archive.oci.tar>..."
cmd="${1:?$USAGE}"
target="${2:-}"
services=("iam" "gateway")
[ -n "$target" ] && services=("$target")

# `smoke` with no argument smokes both images, gateway first (the old behaviour). With service
# arguments it smokes exactly those; assert_fresh (above) is what stops a stale image from being
# tested. `all` still takes no argument: it builds with --load and smokes both.
case "$cmd" in
  build) assert_pins; for s in "${services[@]}"; do build_one "$s"; done ;;
  smoke)
    shift
    if [ "$#" -eq 0 ]; then smoke gateway iam; else smoke "$@"; fi
    ;;
  all)
    if [ -n "$target" ]; then
      echo "usage: ci/images/run.sh all takes no service argument — it builds and smokes both images; use 'build [iam|gateway]' to build one" >&2
      exit 1
    fi
    assert_pins
    for s in "${services[@]}"; do build_one "$s"; done
    smoke gateway iam
    ;;
  build-oci)
    if [ -z "$target" ] || [ -z "${3:-}" ]; then
      echo "usage: ci/images/run.sh build-oci <iam|gateway> <outdir>" >&2
      exit 1
    fi
    assert_pins
    build_oci "$target" "$3"
    ;;
  load-oci)
    if [ -z "$target" ] || [ -z "${3:-}" ]; then
      echo "usage: ci/images/run.sh load-oci <archive> <image-name>" >&2
      exit 1
    fi
    load_oci "$target" "$3"
    ;;
  rehearse) shift; rehearse "$@" ;;
  *)
    echo "unknown command: $cmd" >&2
    echo "$USAGE" >&2
    exit 1
    ;;
esac
