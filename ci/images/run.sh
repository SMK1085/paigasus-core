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
#        ci/images/run.sh build-console [iam|gateway]   # SMA-513: console image; [iam|gateway] scopes the build
#        ci/images/run.sh all-consoles                    # SMA-513: build both consoles + smoke; takes no service arg
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

# The console image's Node major is a SECOND pin beside .prototools. distroless publishes no
# patch-level tags, so only the major can be held equal — that is the ceiling of this check, not
# an oversight. A distroless bump that crosses a major reds here rather than shipping a runtime
# the repo does not pin.
assert_console_pins() {
  local df="$ROOT/ts/Dockerfile" want_major base_major proto_node proto_pnpm builder_node builder_pnpm
  proto_node="$(sed -n 's/^node = "\([0-9.]*\)"$/\1/p' "$ROOT/.prototools")"
  proto_pnpm="$(sed -n 's/^pnpm = "\([0-9.]*\)"$/\1/p' "$ROOT/.prototools")"
  want_major="${proto_node%%.*}"
  base_major="$(sed -n 's#^FROM gcr\.io/distroless/nodejs\([0-9]*\)-debian12.*#\1#p' "$df")"
  if [ -z "$base_major" ]; then
    echo "::error::ts/Dockerfile: no gcr.io/distroless/nodejsNN-debian12 FROM line found." >&2
    return 1
  fi
  if [ "$base_major" != "$want_major" ]; then
    echo "::error::ts/Dockerfile pins Node ${base_major} but .prototools pins ${proto_node}." >&2
    return 1
  fi
  # The runtime stage's FROM line above only pins Node's MAJOR (distroless publishes no
  # patch-level tags). The builder stage is a full node:X.Y.Z-bookworm tag and CAN be held to the
  # exact .prototools version, so it is — a builder bump that drifts from .prototools would
  # otherwise compile the console with a Node the repo does not pin, unnoticed by the check above.
  builder_node="$(sed -n 's/^FROM node:\([0-9.]*\)-bookworm.*/\1/p' "$df" | sed -n 1p)"
  if [ -z "$builder_node" ]; then
    echo "::error::ts/Dockerfile: no FROM node:X.Y.Z-bookworm builder-stage line found." >&2
    return 1
  fi
  if [ "$builder_node" != "$proto_node" ]; then
    echo "::error::ts/Dockerfile builder pins Node ${builder_node} but .prototools pins ${proto_node}." >&2
    return 1
  fi
  # pnpm is a THIRD pin, alongside Node: .prototools names the exact version corepack must
  # activate, and a drift here would build the lockfile-frozen install with a pnpm the repo does
  # not pin, even though the install itself still succeeds.
  builder_pnpm="$(sed -n 's/.*corepack prepare pnpm@\([0-9.]*\).*/\1/p' "$df" | sed -n 1p)"
  if [ -z "$builder_pnpm" ]; then
    echo "::error::ts/Dockerfile: no 'corepack prepare pnpm@X.Y.Z' line found." >&2
    return 1
  fi
  if [ "$builder_pnpm" != "$proto_pnpm" ]; then
    echo "::error::ts/Dockerfile pins pnpm ${builder_pnpm} but .prototools pins ${proto_pnpm}." >&2
    return 1
  fi
  # SMA-513 final review I3: both base images are digest-pinned, and the digest is asserted, the
  # way assert_pins asserts it on the rs builder. Without this, deleting `@sha256:…` or swapping
  # `:nonroot@sha256:…` for `:latest` left every row above green. Code compiled in the builder
  # stage goes into /app, so the builder's digest matters as much as the runtime's.
  # `grep -c` over the FILE, not `grep -q` from a pipe: rc 1 (no match) prints 0 and is folded
  # into the count, and rc 2 (unreadable file) also leaves 0, so both fail closed below.
  # The two pin regexes are locals because S-FROM below reads them a second time, on the
  # normalised file, so the count of FROM lines and the pins read the same view (SMA-670 D6).
  local n_runtime_pin n_builder_pin rt_pin_re bd_pin_re
  rt_pin_re='^FROM gcr\.io/distroless/nodejs[0-9]+-debian12:nonroot@sha256:[0-9a-f]{64}([[:space:]]|$)'
  bd_pin_re='^FROM node:[0-9]+\.[0-9]+\.[0-9]+-bookworm@sha256:[0-9a-f]{64}[[:space:]]+AS[[:space:]]+builder([[:space:]]|$)'
  n_runtime_pin="$(grep -cE "$rt_pin_re" "$df")" || n_runtime_pin=0
  if [ "${n_runtime_pin:-0}" -ne 1 ]; then
    echo "::error::ts/Dockerfile: the runtime FROM line must be exactly one gcr.io/distroless/nodejsNN-debian12:nonroot@sha256:<64 hex> — a missing digest or a different tag leaves the runtime base unpinned." >&2
    return 1
  fi
  n_builder_pin="$(grep -cE "$bd_pin_re" "$df")" || n_builder_pin=0
  if [ "${n_builder_pin:-0}" -ne 1 ]; then
    echo "::error::ts/Dockerfile: the builder FROM line must be exactly one node:X.Y.Z-bookworm@sha256:<64 hex> AS builder — without the digest, the code compiled into /app comes from an unpinned image." >&2
    return 1
  fi

  # S-DIRECTIVE (SMA-670 D6). On the RAW file, because a parser directive decides how the
  # normaliser below must read the file. Only the comment lines before the first line that is not a
  # comment can be directives. A `# syntax=` makes BuildKit pull an unpinned frontend image, and an
  # `# escape=` changes the continuation character that the normaliser joins on. The match is
  # case-insensitive, because Docker reads directives case-insensitively. awk reads the FILE, not a
  # pipe, and it has no `exit`, so it reads its whole input.
  local directive directive_rc=0
  directive="$(awk 'done { next } !/^[[:space:]]*#/ { done = 1; next } { l = tolower($0) } l ~ /^#[[:space:]]*(syntax|escape|check)[[:space:]]*=/ { print; done = 1 }' "$df")" || directive_rc=$?
  if [ "$directive_rc" -ne 0 ]; then
    echo "::error::assert_console_pins: awk exited ${directive_rc} while reading ts/Dockerfile's parser directives; the directive check could not run." >&2
    return 1
  fi
  if [ -n "$directive" ]; then
    echo "::error::ts/Dockerfile starts with a parser directive (${directive}); a '# syntax=' pulls an unpinned BuildKit frontend and '# escape=' changes how this check reads the file — remove it." >&2
    return 1
  fi

  # The checks below read a NORMALISED copy of ts/Dockerfile, written to a temp file, never a
  # here-string: Homebrew bash 5 deadlocks on a here-string over 512 bytes on the development Mac
  # (CLAUDE.md, SMA-612). The normalisation follows assert_pins: `\`-continued lines are joined
  # into one, so a flag or an assignment on a continuation line is seen on its instruction's line.
  # Comment lines are dropped FIRST, because Docker drops them too, including a comment line in
  # the middle of a continuation; dropping them also means a comment that mentions PAIGASUS_ or
  # --frozen-lockfile can neither false-red nor false-green either check.
  local norm norm_rc=0
  norm="$(mktemp "${TMPDIR:-/tmp}/paigasus-console-pins.XXXXXX")" || norm=""
  if [ -z "$norm" ]; then
    echo "::error::assert_console_pins: mktemp failed; the ENV/ARG and --frozen-lockfile checks could not run." >&2
    return 1
  fi
  awk '/^[[:space:]]*#/ { next } /\\[[:space:]]*$/ { sub(/\\[[:space:]]*$/, " "); printf "%s", $0; next } { print }' "$df" > "$norm" || norm_rc=$?
  if [ "$norm_rc" -ne 0 ]; then
    rm -f "$norm"
    echo "::error::assert_console_pins: awk exited ${norm_rc} while normalising ts/Dockerfile; the ENV/ARG and --frozen-lockfile checks could not run." >&2
    return 1
  fi

  # A PAIGASUS_ assignment on an ENV or ARG instruction, in any position on the (joined)
  # instruction: a later variable on a multi-variable ENV line (ts/Dockerfile's own house style),
  # a continuation line, an indented instruction, or an ARG. The instruction keyword matches
  # case-insensitively, because Docker parses `env` and `ENV` the same; PAIGASUS_ stays
  # case-sensitive, as IAM_/GATEWAY_ do in assert_pins. This check reads ENV and ARG only. A
  # PAIGASUS_ in a RUN, COPY, ADD or ONBUILD step or in a heredoc body is S-PAIGASUS's job, below.
  local n_baked baked_rc=0
  n_baked="$(grep -cE '^[[:space:]]*([Ee][Nn][Vv]|[Aa][Rr][Gg])[[:space:]]+.*PAIGASUS_' "$norm")" || baked_rc=$?
  if [ "$baked_rc" -gt 1 ]; then
    rm -f "$norm"
    echo "::error::assert_console_pins: grep exited ${baked_rc} on the normalised ts/Dockerfile; the PAIGASUS_* check could not run." >&2
    return 1
  fi
  if [ "${n_baked:-0}" -ne 0 ]; then
    echo "::error::ts/Dockerfile bakes a PAIGASUS_* env var; console config is deployment-varying and must stay runtime-only. The joined instruction(s) follow." >&2
    grep -nE '^[[:space:]]*([Ee][Nn][Vv]|[Aa][Rr][Gg])[[:space:]]+.*PAIGASUS_' "$norm" >&2 || true
    rm -f "$norm"
    return 1
  fi

  # S-FROM (SMA-670 gap 2a, D6). Every image that the build reads must be one of the two
  # digest-pinned FROM images. So the normalised file holds exactly two FROM instructions, and each
  # one matches one of the two pin regexes above. An extra stage, an unpinned stage, or a `from`
  # instruction in lower case all red here. The count and the pins read the SAME normalised view.
  local from_lines from_rc=0 n_from n_from_bad
  from_lines="$(grep -E '^[[:space:]]*[Ff][Rr][Oo][Mm][[:space:]]' "$norm")" || from_rc=$?
  if [ "$from_rc" -gt 1 ]; then
    rm -f "$norm"
    echo "::error::assert_console_pins: grep exited ${from_rc} on the normalised ts/Dockerfile; the FROM check could not run." >&2
    return 1
  fi
  # printf into grep -c reads the whole input: grep -c is not an early-exit reader.
  n_from="$(printf '%s\n' "$from_lines" | grep -c .)" || n_from=0
  n_from_bad="$(printf '%s\n' "$from_lines" | grep -vE "$rt_pin_re|$bd_pin_re" | grep -c .)" || n_from_bad=0
  if [ "$n_from" -ne 2 ] || [ "$n_from_bad" -ne 0 ]; then
    echo "::error::ts/Dockerfile has ${n_from} FROM instruction(s), or a FROM that is not one of the two digest-pinned stages (the node builder and the distroless runtime); an extra or unpinned stage pulls an unpinned image into the build. The FROM lines of the normalised file follow." >&2
    printf '%s\n' "$from_lines" >&2
    rm -f "$norm"
    return 1
  fi

  # S-COPYFROM (SMA-670 D6). A `--from=<image>` pulls an image that no FROM line pins. So every
  # `--from=` value (COPY --from=) and every `from=` inside a `--mount=` argument (RUN --mount=…)
  # must name the builder stage or the named build context `bindings`. Each extraction is guarded:
  # grep rc 1 is "none found", and only rc > 1 is a failure.
  local cf_from cf_from_rc=0 cf_mount cf_mount_rc=0 cf_values cf_bad cf_v
  cf_from="$(grep -oE -- '--from=[^[:space:],]+' "$norm")" || cf_from_rc=$?
  cf_mount="$(grep -oE -- '--mount=[^[:space:]]+' "$norm")" || cf_mount_rc=$?
  if [ "$cf_from_rc" -gt 1 ] || [ "$cf_mount_rc" -gt 1 ]; then
    rm -f "$norm"
    echo "::error::assert_console_pins: grep exited ${cf_from_rc}/${cf_mount_rc} on the normalised ts/Dockerfile; the --from= check could not run." >&2
    return 1
  fi
  # grep rc 1 below means "no value at all" or "no bad value", and both leave the variable empty,
  # which is the correct reading. `from=` also finds the value inside each `--from=` match.
  cf_values="$(printf '%s\n' "$cf_from" "$cf_mount" | grep -oE 'from=[^[:space:],]+' | sed 's/^from=//')" || cf_values=""
  cf_bad="$(printf '%s\n' "$cf_values" | grep -vxE 'builder|bindings')" || cf_bad=""
  if [ -n "$cf_bad" ]; then
    while IFS= read -r cf_v; do
      echo "::error::ts/Dockerfile reads from '${cf_v}', which is not the builder stage or the bindings context; a --from=<image> pulls an image that no FROM line pins." >&2
    done < <(printf '%s\n' "$cf_bad")
    rm -f "$norm"
    return 1
  fi

  # S-PAIGASUS (SMA-670 gap 2c, the text half). The ENV/ARG check above has passed, so every
  # PAIGASUS_ that is left is outside an ENV or ARG instruction: a RUN, COPY, ADD or ONBUILD step,
  # a heredoc body line, or a continuation that the normaliser does not join (a blank line inside
  # an ENV value). None may name PAIGASUS_ at all. There is no PAIGASUS_COMPILED_* exemption:
  # ts/Dockerfile never writes those values, createNextConfig does (SMA-670 D7). Comment lines
  # cannot trigger this, because the normaliser dropped them. The lines are printed without line
  # numbers, because a normalised line number is not a ts/Dockerfile line number.
  local n_named named_rc=0
  n_named="$(grep -c 'PAIGASUS_' "$norm")" || named_rc=$?
  if [ "$named_rc" -gt 1 ]; then
    rm -f "$norm"
    echo "::error::assert_console_pins: grep exited ${named_rc} on the normalised ts/Dockerfile; the PAIGASUS_ outside ENV/ARG check could not run." >&2
    return 1
  fi
  if [ "${n_named:-0}" -ne 0 ]; then
    echo "::error::ts/Dockerfile names PAIGASUS_ outside an ENV/ARG instruction (a RUN, COPY, ADD or ONBUILD step, or a heredoc body); console config is deployment-varying and must stay runtime-only. The line(s) of the normalised file follow." >&2
    grep 'PAIGASUS_' "$norm" >&2 || true
    rm -f "$norm"
    return 1
  fi

  # EVERY `pnpm install` invocation carries --frozen-lockfile, not only the first one found. Each
  # invocation is cut at the next `&&`, `;` or `|`, so two installs chained in one RUN are two
  # invocations here. `--frozen-lockfile` must stand as a bare flag. Any `--frozen-lockfile=<value>`
  # form (`=false` switches it off) and `--no-frozen-lockfile` are rejected outright, even beside
  # a bare flag, so the check never has to decide which of two conflicting flags pnpm obeys.
  #
  # S-INSTALL (SMA-670 gap 2b). The extraction takes EVERY pnpm invocation first, from the word
  # `pnpm` to the next `&&`, `;` or `|`. awk then splits each one into whitespace-separated tokens
  # and keeps it when a token is exactly `install`, `i`, `install-test` or `it`. So
  # `pnpm --filter x install`, `pnpm -C ts i` and a bare `pnpm i` are installs, and `pnpm info`,
  # `pnpm import`, `pnpm init` and `pnpm exec next build` are not. awk tokens, not `\b`, `\<` or
  # `\>`: those are not POSIX ERE, and BSD grep and GNU grep read them differently. Each token
  # loses its quote, backtick and parenthesis characters before the compare, so
  # `sh -c "pnpm install"` stays an install, as it was under the old `pnpm[[:space:]]+install`
  # match (MEASURED: without the strip the token is `install"` and the install is missed). The awk
  # has no `exit`, so it reads its whole input. `\047` is the single quote.
  local inv inv_rc=0 inst n_inst n_frozen n_unfrozen
  inv="$(grep -oE '(^|[^[:alnum:]_./-])pnpm([[:space:]][^&;|]*)?' "$norm")" || inv_rc=$?
  rm -f "$norm"
  if [ "$inv_rc" -gt 1 ]; then
    echo "::error::assert_console_pins: grep exited ${inv_rc} on the normalised ts/Dockerfile; the --frozen-lockfile check could not run." >&2
    return 1
  fi
  inst="$(printf '%s\n' "$inv" | awk '{ for (i = 1; i <= NF; i++) { t = $i; gsub(/["\047()`]/, "", t); if (t == "install" || t == "i" || t == "install-test" || t == "it") { print; next } } }')" || inst=""
  if [ -z "$inst" ]; then
    echo "::error::ts/Dockerfile: no 'pnpm install'/'pnpm i' instruction found; the image would not be built from the committed lockfile." >&2
    return 1
  fi
  # printf into grep -c reads the whole input: grep -c is not an early-exit reader.
  n_inst="$(printf '%s\n' "$inst" | grep -c .)" || n_inst=0
  n_frozen="$(printf '%s\n' "$inst" | grep -cE -- '--frozen-lockfile([[:space:]]|$)')" || n_frozen=0
  n_unfrozen="$(printf '%s\n' "$inst" | grep -cE -- '--frozen-lockfile=|--no-frozen-lockfile')" || n_unfrozen=0
  if [ "$n_unfrozen" -ne 0 ] || [ "$n_frozen" -ne "$n_inst" ]; then
    echo "::error::ts/Dockerfile: ${n_frozen} of ${n_inst} 'pnpm install'/'pnpm i' invocation(s) carry --frozen-lockfile, and ${n_unfrozen} switch it off; every install must be frozen, or the image would not be built from the committed lockfile. The invocations follow." >&2
    printf '%s\n' "$inst" >&2
    return 1
  fi

  # S-HCTIMEOUT (SMA-670 D2). On the RAW file. The healthcheck printf line must hold exactly one
  # AbortSignal.timeout(<ms>), the HEALTHCHECK instruction must hold --timeout=<s>s, and the signal
  # must fire before Docker kills the probe. Without the signal, a server that accepts the
  # connection and never answers hangs the probe until Docker kills it, and only undici's 300 s
  # headersTimeout bounds a `docker exec` of the same program.
  local hc_line hc_line_rc=0 n_hc_line n_sig sig_ms hc_to_s
  hc_line="$(grep -E '^RUN printf .*> /app/healthcheck\.mjs$' "$df")" || hc_line_rc=$?
  if [ "$hc_line_rc" -gt 1 ]; then
    echo "::error::assert_console_pins: grep exited ${hc_line_rc} on ts/Dockerfile; the healthcheck timeout check could not run." >&2
    return 1
  fi
  n_hc_line="$(printf '%s\n' "$hc_line" | grep -c .)" || n_hc_line=0
  n_sig="$(printf '%s\n' "$hc_line" | grep -oE 'AbortSignal\.timeout\([0-9]+\)' | grep -c .)" || n_sig=0
  sig_ms="$(printf '%s\n' "$hc_line" | sed -n 's/.*AbortSignal\.timeout(\([0-9][0-9]*\)).*/\1/p' | sed -n 1p)" || sig_ms=""
  hc_to_s="$(grep -E '^[[:space:]]*HEALTHCHECK[[:space:]]' "$df" | grep -oE -- '--timeout=[0-9]+s' | sed -n 's/^--timeout=\([0-9]*\)s$/\1/p' | sed -n 1p)" || hc_to_s=""
  if [ "$n_hc_line" -ne 1 ] || [ "$n_sig" -ne 1 ] || [ -z "$sig_ms" ] || [ -z "$hc_to_s" ] \
    || [ "$sig_ms" -ge $((hc_to_s * 1000)) ]; then
    echo "::error::ts/Dockerfile's healthcheck.mjs fetch has no AbortSignal.timeout(<ms>) below the HEALTHCHECK --timeout (found: ${n_hc_line} healthcheck printf line(s), ${n_sig} signal(s), signal ${sig_ms:-<none>} ms, HEALTHCHECK --timeout ${hc_to_s:-<none>} s; only a --timeout=<N>s value in whole seconds is read); without it, a server that stops answering hangs the probe until Docker kills it." >&2
    return 1
  fi
  echo "  ts/Dockerfile: distroless Node ${base_major} and builder Node ${builder_node}/pnpm ${builder_pnpm} match .prototools, both FROM lines digest-pinned, no parser directive, exactly 2 FROM stages, every --from= is builder/bindings, no PAIGASUS_* anywhere, all ${n_inst} pnpm install(s) --frozen-lockfile, healthcheck signal ${sig_ms} ms < --timeout=${hc_to_s}s"
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
  # `docker buildx build`, not bare `docker build` (SMA-658 PR1 CI fix): plain `docker build` is
  # not guaranteed to route through the builder that `docker buildx use` (what
  # docker/setup-buildx-action selects) made current — build_oci below hit exactly this gap, so
  # both build paths now say `buildx` explicitly. See build_oci's comment for the measured proof.
  # --load: docker/setup-buildx-action makes a `docker-container` builder CURRENT, and that
  # driver does not reliably auto-load its output into the local `docker images` store on every
  # Docker version (it happens to on 29.6.2, but the CI runner's version is not guaranteed to
  # match). Without --load the failure mode is silent here and loud at the first `docker run`
  # below ("No such image"). Under the plain `docker` driver (no buildx container) --load is a
  # no-op-safe `--output=type=docker`, so it costs nothing locally.
  docker buildx build \
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

app_for() {
  case "$1" in
    iam)     echo "iam-console" ;;
    gateway) echo "gateway-console" ;;
    *) echo "unknown console: $1" >&2; return 1 ;;
  esac
}

base_path_for() {
  case "$1" in
    iam)     echo "/iam" ;;
    gateway) echo "/gateway" ;;
    *) echo "unknown console: $1" >&2; return 1 ;;
  esac
}

# A route INSIDE that zone's `(console)` route group, relative to its basePath (SMA-634). It is the
# one runtime proof that the image loads the kernel: the `(console)` layout imports
# @paigasus/console-core, which evaluates the kernel's wasm at module scope.
#
# PER ZONE, and not `/orgs` for both. gateway-console has no `/orgs` page at all — its zone overview
# is `(console)/overview/page.tsx`, and its only `orgs` routes are parameterised
# (`orgs/[org]`). `/gateway/orgs` therefore answered 404, from the ROOT not-found, with the
# `(console)` layout never evaluated — and the probe's old "any non-5xx passes" rule read that as a
# pass. A zone added here needs a route of its own; there is no default on purpose.
console_probe_path_for() {
  case "$1" in
    iam)     echo "/orgs" ;;
    gateway) echo "/overview" ;;
    *) echo "unknown console: $1" >&2; return 1 ;;
  esac
}

# There is no --no-cache-filter here. That flag exists on build_one because rs/Dockerfile's
# rootfs stage is byte-identical between services and BuildKit would cache-hit it, leaving the
# second service's chisel manifest empty. Every stage in ts/Dockerfile references APP, so no
# stage is shared between the two console builds and there is nothing to force.
# `docker buildx build`, not bare `docker build`, for the same reason as build_one: on the
# GitHub-hosted runner a bare `docker build` does not reliably route through the builder that
# docker/setup-buildx-action made current (SMA-658, measured; see build_oci's comment). On this
# development Mac `docker build` is itself an alias for buildx, so a local run cannot show the gap.
# SMA-634: the console install now reaches @paigasus/kernel, whose two `file:` dependencies live
# under rs/crates/bindings — outside the ts/ build context. They arrive through a NAMED context.
# It is rooted at rs/crates/bindings, not at rs/: a sub-directory context carries no .dockerignore
# of its own, so it needs no edit of rs/.dockerignore (which excludes **/*.wasm) and it cannot
# upload rs/target/. The path is $ROOT-anchored, like every other path here, so the function does
# not depend on the caller's working directory. Without the flag the build fails in a recognisable
# way: `COPY --from=bindings` reads `bindings` as an image reference.
build_console_one() {
  local service="$1" app base_path tag
  app="$(app_for "$service")"
  base_path="$(base_path_for "$service")"
  tag="${REGISTRY}/paigasus-${app}:${REVISION}"
  echo "== build ${app} =="
  docker buildx build \
    --progress=plain \
    --load \
    -f "$ROOT/ts/Dockerfile" \
    --build-context "bindings=$ROOT/rs/crates/bindings" \
    --build-arg "APP=${app}" \
    --build-arg "BASE_PATH=${base_path}" \
    --label "org.opencontainers.image.title=paigasus-${app}" \
    --label "org.opencontainers.image.description=Paigasus ${service} console" \
    --label "org.opencontainers.image.source=https://github.com/SMK1085/paigasus-core" \
    --label "org.opencontainers.image.revision=${REVISION}" \
    --label "org.opencontainers.image.licenses=Apache-2.0" \
    -t "$tag" -t "${app}:dev" \
    "$ROOT/ts"
  echo "  built ${tag}"
}

# SMA-658: the release build. The same image as build_one, exported as an OCI ARCHIVE instead of
# being loaded, so its bytes (and so its digest) are fixed before anything is pushed.
# --provenance=false --sbom=false: buildx would otherwise wrap the image in an index with its own
# attestation manifests; the release path attests through GitHub instead (spec D4).
# name=<crate>:dev: records the image's own identity inside the archive; load_oci tags it as
# whatever name its caller passes (smoke expects <crate>:dev), by digest, not from this name.
#
# `docker buildx build`, never bare `docker build` (SMA-658 PR1 CI fix, PR 270). MEASURED on the
# GitHub-hosted runner: bare `docker build --output type=oci,...` failed with "OCI exporter is
# not supported for the docker driver", although images.yml already runs
# docker/setup-buildx-action, which creates a `docker-container` builder and switches to it
# (`use: true`, its default). The action's own docs describe that switch as making the builder
# current "for subsequent docker buildx commands", not for the classic `docker build` CLI path —
# and Docker's own exporter docs are explicit: "The docker driver doesn't support these
# exporters. You must use docker-container or some other driver." So on this runner, plain
# `docker build` resolved to the classic `docker` driver regardless of the builder
# docker/setup-buildx-action had selected, and the OCI exporter has no path there. Spelling the
# command as `docker buildx build` removes that ambiguity: it always talks to buildx and always
# uses the current builder — the `docker-container` one in CI (OCI export works, per Docker's
# docs), and whatever builder is current locally (unchanged behaviour there: Docker Desktop
# already aliases `docker build` to the same buildx call, which is how the containerd-image-store
# Mac measurement in docs/ops/RUNBOOK-containers.md was taken). Do NOT "simplify" this back to
# bare `docker build` — that is the exact regression this comment exists to prevent.
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
  docker buildx build \
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

# SMA-658 spec § 4.2: the image the smoke suite tests must be the image in the archive. What a
# local image's `.Id` reports depends on the daemon's image store (measured M3, local Docker
# 29.8): the containerd store gives the MANIFEST digest, the classic store gives the CONFIG
# digest. So the check accepts the one value that matches this daemon's store, and prints every
# value once per runner so CI records M3 for each runner label.
#
# CI fix (PR 270): `docker load -i <oci-archive>` of a buildx OCI-layout archive needs the
# CONTAINERD image store to parse it. MEASURED on `ubuntu-latest` and `ubuntu-24.04-arm` (both
# arch legs): it fails with `open .../blobs/json: no such file or directory`, because those
# runners' Docker uses the CLASSIC store. It only works on this Mac because Docker Desktop uses
# the containerd store. So the archive is no longer handed to `docker load` at all — it goes
# through a throwaway local registry instead: start one (the same `registry:2` container and free
# -port pattern `start_registry`/`rehearse` below already use), push the archive with `crane push`
# (crane is proto-pinned; `rehearse` already pushes this way), then `docker pull` it back BY
# DIGEST and tag it as $name. Docker verifies the manifest bytes it downloads hash to the digest
# it was asked for and fails the pull otherwise, so a successful pull is already proof the
# manifest is intact; the store-dependent `.Id` check below then gives the SAME guarantee the old
# `docker load`-based check gave, because it is the STORE, not the ingestion path, that decides
# whether a local image's `.Id` is its manifest digest (containerd) or its config digest
# (classic) — both a `docker load` and a `docker pull` land in the same local store afterwards.
# A second `--output type=docker` export would run the smoke test on different bytes than the
# published archive, which defeats the check; skopeo's docker-daemon transport would add an
# unpinned tool. The registry (and its scratch dir) are removed in an EXIT trap so they are gone
# on every exit path, including a `set -e` abort partway through — measured: a RETURN trap, the
# pattern `build_one`/`build_oci` use for their build log, does NOT fire when `set -e` aborts a
# function from inside, only on that function's normal return, so it would leak the registry
# container on exactly the failures this check exists to catch.
#
# CI fix, PR 270: the EXIT trap must not read a variable the function it was set in declared
# `local` — a trap fires at SCRIPT exit, after the function has already returned and its locals
# have gone out of scope, so under `set -u` the trap itself dies with `reg_name: unbound
# variable` (MEASURED on all three CI legs: every one printed M3 for the first crate, then died
# on that exact line before reaching the second crate). `LOAD_OCI_REG`/`LOAD_OCI_TMPDIR` below
# are script-global instead, initialised to the empty string so `load_oci_cleanup` is a safe
# no-op for every command that never runs `load_oci` at all, and the trap that calls it is
# registered ONCE, at top level — not re-registered per call, so a second `load-oci` invocation
# in the same script process reads the same always-defined globals rather than risking a second,
# possibly stale, trap body. `load_oci` also calls `load_oci_cleanup` explicitly once it is done
# with the registry, so a normal, successful call leaves nothing behind for a second call to
# collide with; the trap remains as the backstop for a `set -e` abort or a signal before that
# point is reached.
LOAD_OCI_REG=""
LOAD_OCI_TMPDIR=""

load_oci_cleanup() {
  if [ -n "$LOAD_OCI_REG" ]; then
    docker rm -f "$LOAD_OCI_REG" >/dev/null 2>&1 || true
    LOAD_OCI_REG=""
  fi
  if [ -n "$LOAD_OCI_TMPDIR" ]; then
    rm -rf "$LOAD_OCI_TMPDIR"
    LOAD_OCI_TMPDIR=""
  fi
}
trap load_oci_cleanup EXIT

load_oci() {
  local archive="$1" name="$2" digests manifest config store driver loaded size expected
  local reg repo
  digests="$(decide oci-digests "$archive")"
  manifest="$(kv "$digests" manifest)"
  config="$(kv "$digests" config)"

  if ! command -v crane >/dev/null 2>&1; then
    echo "::error::crane is not on PATH; run 'proto install crane'" >&2
    return 2
  fi
  LOAD_OCI_REG="load-oci-registry-$$"
  reg="$(start_registry "$LOAD_OCI_REG")"
  # `127.0.0.1`, not the `localhost` `start_registry` returns (kept as-is for `rehearse`, which
  # needs it — see its own comment): measured on this Mac, `docker pull`/`docker tag` resolve
  # `localhost` to `::1` first and time out, because the registry container is published on
  # `127.0.0.1` only. `crane push` below is unaffected either way (it is given `--insecure`
  # explicitly), so using the literal IP for the whole function sidesteps the resolution order
  # entirely rather than depending on it.
  reg="${reg/localhost/127.0.0.1}"
  repo="${reg}/paigasus-load-oci"

  LOAD_OCI_TMPDIR="$(mktemp -d "${TMPDIR:-/tmp}/paigasus-load-oci.XXXXXX")"
  tar -xf "$archive" -C "$LOAD_OCI_TMPDIR"
  crane push --insecure "$LOAD_OCI_TMPDIR" "${repo}:load" >/dev/null
  rm -rf "$LOAD_OCI_TMPDIR"
  LOAD_OCI_TMPDIR=""

  docker pull "${repo}@${manifest}" >/dev/null
  docker tag "${repo}@${manifest}" "$name"

  loaded="$(docker image inspect --format '{{.Id}}' "$name")"
  size="$(docker image inspect --format '{{.Size}}' "$name")"
  driver="$(docker info --format '{{json .DriverStatus}}')"
  store="classic"
  case "$driver" in *io.containerd.snapshotter*) store="containerd" ;; esac
  expected="$config"
  [ "$store" = "containerd" ] && expected="$manifest"
  echo "M3 arch=$(docker version --format '{{.Server.Arch}}') docker=$(docker version --format '{{.Server.Version}}') store=${store} loaded_id=${loaded} manifest=${manifest} config=${config} size=${size}"
  # Clean up now rather than waiting for the script-exit trap: a successful call must not leave
  # its registry running for a SECOND `load-oci` call in the same script process to collide
  # with. The trap above stays registered as the backstop for every path that returns before
  # this line runs.
  load_oci_cleanup
  if [ "$loaded" != "$expected" ]; then
    echo "::error::${name} loaded as ${loaded} (pulled by digest via a local registry), but the ${store} store should report ${expected}: the loaded image is not the archive's image." >&2
    return 1
  fi
  echo "  ${name} is the archive's image (${store} store, loaded via a local registry)"
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

# --- console smoke (SMA-513 spec § 5.4 and § 5.5) ------------------------------------------------
# A text allowlist over ts/Dockerfile's COPY instructions cannot see a missing .next/static: Next's
# standalone output carries none of its own (measured, SMA-510) and the staging copy in the builder
# stage is what supplies it. An image built without that copy still answers 200 on
# <basePath>/healthz, so a probe-based smoke passes while every client chunk 404s. These assertions
# fetch the SERVED CHUNK instead, which is the only form that fails on the real defect.
#
# This is the ONLY copy of the served-chunk assertion. Do not add a second one elsewhere (for
# example as a script under ts/tests/): a second copy would not be pinned by this gate and would
# drift from it.
#
# Script-global, not `local`: the EXIT trap fires after smoke_consoles has returned and its locals
# have gone out of scope, and under `set -u` a trap that reads a dead local dies with `unbound
# variable` — the measured PR 270 failure recorded on load_oci_cleanup above. An EXIT trap and not
# the RETURN trap the task brief asked for, for the reason recorded there too: a RETURN trap does
# NOT fire when `set -e` aborts a function from inside, which is exactly the path that would leak a
# container.
CONSOLE_SMOKE_NAMES=""

console_smoke_cleanup() {
  local n
  for n in $CONSOLE_SMOKE_NAMES; do
    docker rm -f "$n" >/dev/null 2>&1 || true
  done
  CONSOLE_SMOKE_NAMES=""
}

# A FULL, schema-valid dummy runtime configuration, not just PAIGASUS_ZONE and PAIGASUS_ZONES. Both
# consoles validate their ENTIRE runtime config (@paigasus/auth's OIDC/session shape,
# @paigasus/discovery's PAIGASUS_SERVICES, and PAIGASUS_IAM_GRPC_URL — see ts/apps/<app>/lib/
# config.ts) on the FIRST request, and a missing or malformed variable throws "Invalid runtime
# configuration", which the app renders as a 500. From the outside that 500 looks exactly like a
# missing .next/static — both leave the page with no chunk URL to find — so without this block the
# assertion cannot tell "not staged" from "not configured" apart.
#
# One shared array covers both zones: iam-console only requires an "iam" entry in
# PAIGASUS_SERVICES, gateway-console requires both "iam" AND "gateway"
# (ts/apps/gateway-console/lib/config.ts), so the map below carries both unconditionally.
#
# WARNING, INTENTIONAL COUPLING: a new required key added to a console's lib/config.ts schema makes
# this array stale and turns every 200 here into a 500. That is supposed to fail LOUDLY — add the
# key here; do NOT add a fallback in the app or relax the assertions below.
CONSOLE_SMOKE_ENV=(
  -e "PAIGASUS_OIDC_ISSUER=https://idp.example.com"
  -e "PAIGASUS_OIDC_CLIENT_ID=dummy-client"
  -e "PAIGASUS_OIDC_CLIENT_SECRET=dummy-secret"
  -e "PAIGASUS_PUBLIC_ORIGIN=https://console.example.com"
  -e "PAIGASUS_SESSION_STORE=memory"
  -e "PAIGASUS_SERVICES={\"iam\":\"http://iam:8080\",\"gateway\":\"http://gateway:8080\"}"
  -e "PAIGASUS_IAM_GRPC_URL=http://iam:9090"
)

# SMA-670 gap 3: the wall-clock bound, in seconds, on the smoke row that runs the image's own
# HEALTHCHECK program with `docker exec`. The program's own fetch signal (2500 ms, ts/Dockerfile)
# ends a hang first; this bound is for a hang that the signal does not end.
CONSOLE_HC_DEADLINE=20

# Walks the image's staged tree with the image's OWN node — the runtime base is distroless and has
# no shell, so there is no `find` in there to call. Prints `public=0|1` on line 1 and one staged
# .next/static path per line after it, with the BUILD_ID directory rewritten to the literal
# <BUILD_ID>. MEASURED (SMA-513): Next generates BUILD_ID as a random nanoid — no next.config.ts in
# this repo sets generateBuildId — so the host build and the image build never share one, and an
# un-normalised comparison of the two trees can NEVER pass. Sorting is left to the caller, which
# puts both sides through the same `LC_ALL=C sort`: also MEASURED, node's Array.sort and the host's
# `sort` disagreed on `chunks/3_j6cf7txpq_5.js` vs `chunks/3h4osm35n9wui.js`, which would have
# reported drift between two byte-identical trees.
CONSOLE_STAGED_TREE_JS='
const fs = require("fs");
const root = process.argv[1];
const id = fs.readFileSync(root + "/.next/BUILD_ID", "utf8").trim();
const walk = (d, p = "") =>
  fs.readdirSync(d, { withFileTypes: true })
    .flatMap((e) => (e.isDirectory() ? walk(d + "/" + e.name, p + e.name + "/") : [p + e.name]));
const rel = walk(root + "/.next/static")
  .map((f) => (id !== "" && f.indexOf(id + "/") === 0 ? "<BUILD_ID>/" + f.slice(id.length + 1) : f));
console.log(["public=" + (fs.existsSync(root + "/public") ? "1" : "0")].concat(rel).join("\n"));
'

# --- SMA-670 smoke rows ----------------------------------------------------------------------------
# smoke_consoles calls each function in this section as `<fn> … || ec=1`. Because of the `||`,
# errexit is OFF inside them: each one checks the rc of every command itself and must not depend on
# `set -e`. Each one keeps its JS program in a `local`, so it needs no global except ROOT (and
# with_deadline, for the HEALTHCHECK row). ci/images/console-selftest.sh copies them out of this
# file with awk and calls them in this same `|| rc=$?` shape against a stub `docker`.

# R-NODE (SMA-670 gap 1). The runtime base pins only the Node MAJOR (distroless publishes no
# patch-level tags), so nothing else records which Node the image runs. This row prints it. A
# different major, an unparseable version or an unreadable .prototools pin is an error. A different
# minor or patch is a WARNING and the row stays green (SMA-670 D1): no change in this repository can
# make the patch equal, because the runtime tag `nonroot` holds no version and the digest is the only
# pin. It needs only the image, not a running container. stdout only is parsed; docker's own stderr
# passes through.
console_node_version_row() {
  local app="$1" pin ver_out ver_rc=0 line maj min pat pmaj pmin ppat advice
  local pin_re='^([0-9]+)\.([0-9]+)\.([0-9]+)$' ver_re='^v([0-9]+)\.([0-9]+)\.([0-9]+)$'
  pin="$(sed -n 's/^node = "\([0-9.]*\)"$/\1/p' "$ROOT/.prototools")" || pin=""
  if ! [[ $pin =~ $pin_re ]]; then
    echo "::error::${app}: runtime Node version NOT checked — no node = \"X.Y.Z\" pin in .prototools." >&2
    return 1
  fi
  pmaj="${BASH_REMATCH[1]}"; pmin="${BASH_REMATCH[2]}"; ppat="${BASH_REMATCH[3]}"
  ver_out="$(docker run --rm --entrypoint /nodejs/bin/node "${app}:dev" --version)" || ver_rc=$?
  if [ "$ver_rc" -ne 0 ]; then
    echo "::error::${app}: runtime Node version NOT checked — docker exited ${ver_rc} on ${app}:dev before node printed a version, so the image is missing or unreadable." >&2
    return 1
  fi
  line="$(printf '%s\n' "$ver_out" | sed -n 1p)" || line=""
  if ! [[ $line =~ $ver_re ]]; then
    echo "::error::${app}: could not parse the runtime Node version from '${line}' — /nodejs/bin/node --version must print vX.Y.Z." >&2
    return 1
  fi
  maj="${BASH_REMATCH[1]}"; min="${BASH_REMATCH[2]}"; pat="${BASH_REMATCH[3]}"
  if [ "$maj" -ne "$pmaj" ]; then
    echo "::error::${app}: the runtime image runs Node ${line#v}, but .prototools pins ${pin} — a different major; the runtime FROM line in ts/Dockerfile and .prototools disagree." >&2
    return 1
  fi
  if [ "$min" -ne "$pmin" ] || [ "$pat" -ne "$ppat" ]; then
    if [ "$min" -lt "$pmin" ] || { [ "$min" -eq "$pmin" ] && [ "$pat" -lt "$ppat" ]; }; then
      advice="The runtime is older: a later runtime digest refresh closes the gap."
    else
      advice="The runtime is newer: bump .prototools and the builder FROM line together."
    fi
    echo "::warning::${app}: the runtime image runs Node ${line#v}, .prototools pins ${pin}. distroless publishes no patch-level tags, so only the major is held. ${advice}" >&2
    echo "  ${app}: runtime Node ${line#v}, same major as .prototools ${pin} (minor/patch differ, see the warning)"
    return 0
  fi
  echo "  ${app}: runtime Node ${line#v} matches .prototools"
}

# R-CONFIG (SMA-670 gap 2c, the behavioural half). Two reads of the image, and both need only the
# image. (1) Config.Env must hold no PAIGASUS_* key; the error names the key, never the value.
# (2) The image's own node walks /app without following symlinks, and no file whose base name
# starts with `.env` may be there outside node_modules: Next loads `.env*` from the server's own
# directory at runtime, so a value in such a file is baked configuration. A dependency can ship an
# `.env.example`, so a path with a node_modules directory in it is not reported. The walk still
# counts those files, and a count under 100 means that it read the wrong tree (measured: 1367 files
# in iam-console, 1324 in gateway-console).
console_image_config_row() {
  local app="$1" rc=0 env_out env_rc=0 keys key walk_out walk_rc=0 first n paths paths_rc=0
  local walk_js='
const fs = require("fs");
let walked = 0;
const found = [];
const walk = (d) => {
  for (const e of fs.readdirSync(d, { withFileTypes: true })) {
    const p = d + "/" + e.name;
    if (e.isDirectory()) { walk(p); continue; }
    walked++;
    if (e.name.startsWith(".env")) found.push(p);
  }
};
walk(process.argv[1]);
console.log(["walked=" + walked].concat(found).join("\n"));
'
  env_out="$(docker image inspect --format '{{range .Config.Env}}{{println .}}{{end}}' "${app}:dev")" || env_rc=$?
  if [ "$env_rc" -ne 0 ]; then
    echo "::error::${app}: image config NOT checked — docker image inspect exited ${env_rc} on ${app}:dev, so the image is missing or unreadable." >&2
    rc=1
  else
    keys="$(printf '%s\n' "$env_out" | sed -n 's/^\(PAIGASUS_[^=]*\)=.*$/\1/p')" || keys=""
    if [ -n "$keys" ]; then
      while IFS= read -r key; do
        echo "::error::${app}:dev bakes ${key} into Config.Env — console config is deployment-varying and must stay runtime-only (the value is not printed)." >&2
      done < <(printf '%s\n' "$keys")
      rc=1
    fi
  fi
  walk_out="$(docker run --rm --entrypoint /nodejs/bin/node "${app}:dev" -e "$walk_js" /app)" || walk_rc=$?
  if [ "$walk_rc" -ne 0 ]; then
    echo "::error::${app}: .env scan NOT checked — the walk exited ${walk_rc} on ${app}:dev, so the image is missing or unreadable." >&2
    return 1
  fi
  first="$(printf '%s\n' "$walk_out" | sed -n 1p)" || first=""
  n=""
  case "$first" in walked=*) n="${first#walked=}" ;; esac
  case "$n" in ''|*[!0-9]*) n="" ;; esac
  if [ -z "$n" ] || [ "$n" -lt 100 ]; then
    echo "::error::${app}: .env scan walked ${n:-an unreadable number of ('${first}')} files under /app — too few to prove anything; the walk read the wrong tree." >&2
    return 1
  fi
  # The node_modules filter is here, not in the JS, so the self-test's stub rows exercise it.
  # grep -v rc 1 means "every path was under node_modules", which leaves `paths` empty. Under
  # pipefail, this pipeline's own rc is grep's rc (printf and sed do not fail here), so rc 0/1
  # both read as today; rc > 1 means grep itself could not run, and a silent paths="" there would
  # be a false-clear on a secret scan (global-constraints.md: a grep rc > 1 needs its own
  # ::error::, per assert_console_pins's cf_from_rc/cf_mount_rc precedent).
  paths="$(printf '%s\n' "$walk_out" | sed -n '2,$p' | grep -v '/node_modules/')" || paths_rc=$?
  if [ "$paths_rc" -gt 1 ]; then
    echo "::error::${app}: .env scan NOT checked — grep exited ${paths_rc} while filtering the walk output for node_modules paths." >&2
    return 1
  fi
  if [ -n "$paths" ]; then
    echo "::error::${app}: the image holds .env file(s) under /app outside node_modules; Next loads them at runtime, so a value in them is baked configuration. The paths follow." >&2
    printf '%s\n' "$paths" >&2
    return 1
  fi
  if [ "$rc" -ne 0 ]; then return 1; fi
  echo "  ${app}: no PAIGASUS_* in Config.Env and no .env file in /app (${n} files walked)"
}

# R-HEALTH (SMA-670 gap 3). The image's OWN HEALTHCHECK program, run inside the running container.
# Nothing else in this suite executes /app/healthcheck.mjs: ts/Dockerfile writes it with a `printf`
# that carries a backtick template literal and a `%s` substitution, so an escaping or path
# regression would otherwise ship with every other row green. `docker exec` of the image's node
# needs no shell. The call runs under with_deadline, so a probe that hangs cannot hang this suite.
# Its output goes to a mktemp FILE, never to a `$( )` capture: MEASURED (SMA-670 M7), a captured
# with_deadline waits for its full deadline under bash 5, because the watchdog's orphan `sleep`
# holds the capture pipe open. A timeout is reported only when the rc is 143 or 137 AND the elapsed
# time reached the deadline; any other 137 (for example from the OOM killer) takes the "exited"
# message. A killed `docker exec` client can leave node running in the container;
# console_smoke_cleanup and the EXIT trap remove the container.
console_healthcheck_row() {
  local name="$1" app="$2" base_path="$3" deadline="$4" out hc_rc=0 start elapsed deadline_ok
  case "$deadline" in
    ''|*[!0-9]*) deadline_ok=0 ;;
    *) deadline_ok=1 ;;
  esac
  if [ "$deadline_ok" -eq 0 ] || [ "$deadline" -lt 1 ]; then
    echo "::error::${app}: HEALTHCHECK program NOT checked — the deadline '${deadline}' is not a positive integer number of seconds." >&2
    return 1
  fi
  out="$(mktemp "${TMPDIR:-/tmp}/paigasus-console-hc.XXXXXX")" || out=""
  if [ -z "$out" ]; then
    echo "::error::${app}: HEALTHCHECK program NOT checked — mktemp failed." >&2
    return 1
  fi
  start=$SECONDS
  with_deadline "$deadline" docker exec "$name" /nodejs/bin/node /app/healthcheck.mjs >"$out" 2>&1 || hc_rc=$?
  elapsed=$((SECONDS - start))
  if [ "$hc_rc" -eq 0 ]; then
    rm -f "$out"
    echo "  ${app}: HEALTHCHECK program /app/healthcheck.mjs exits 0"
    return 0
  fi
  if { [ "$hc_rc" -eq 143 ] || [ "$hc_rc" -eq 137 ]; } && [ "$elapsed" -ge "$deadline" ]; then
    echo "::error::${app}: the image's HEALTHCHECK program (/app/healthcheck.mjs) did not finish within ${deadline}s against a server that renders ${base_path} — the probe hangs; check the fetch timeout in ts/Dockerfile's healthcheck printf. Its output follows." >&2
  else
    echo "::error::${app}: the image's HEALTHCHECK program (/app/healthcheck.mjs) exited ${hc_rc} against a server that renders ${base_path} — check the healthcheck printf in ts/Dockerfile. Its output follows." >&2
  fi
  cat "$out" >&2 || true
  rm -f "$out"
  return 1
}

# Every `$( )` here is either guarded with `|| <var>=""` and followed by an explicit check that
# prints its own named ::error::, or provably unable to fail before its own message. That is not
# decoration: under `set -euo pipefail` an unguarded failing capture aborts the whole script on
# curl's or docker's generic message, BEFORE the branch that would have named the cause, and it
# also cancels the other console's rows.
#
# Likewise `ec=1` and never `return 1` inside the battery: one failing console must not hide the
# other, and the verdict is taken once at the end. The closing line is an `if`, not
# `[ "$ec" -eq 0 ] && echo …` — a failing `[ ]` as the last top-level command would make the
# function return 1 on its own.
smoke_consoles() {
  local service app base_path console_path other name port origin status html chunk bytes code uid console_status
  local run_out img_out img_public img_list host_public host_list img_dirs host_dirs
  local host_std host_static host_id run_rc sh_rc img_rc cstate
  local ec=0 bad started
  # This REPLACES the script-global `trap load_oci_cleanup EXIT` at the top of the load-oci
  # section, exactly as `smoke()` and `rehearse` already do — harmless today because no dispatch
  # arm reaches `load_oci` and `smoke_consoles` in one process. A future arm that chains them
  # would leak the load-oci registry container SILENTLY; make that arm call `load_oci_cleanup`
  # itself, or fold both cleanups into one trap body, rather than assuming this line is inert.
  trap console_smoke_cleanup EXIT
  console_smoke_cleanup

  # The zone list comes from the caller (the `all-consoles` arm passes `console_services`), so the
  # list lives in ONE place rather than being restated here. An empty list must not read as a pass.
  if [ "$#" -eq 0 ]; then
    echo "::error::smoke_consoles: called with no zones — nothing was smoked, and an empty run must not report OK." >&2
    return 1
  fi

  for service in "$@"; do
    # GUARDED, because $service is now caller-supplied rather than a loop literal: app_for and
    # base_path_for print their own "unknown console: …" and return 1, and an unguarded capture
    # would abort the script there and cancel every remaining zone's rows.
    app="$(app_for "$service")" || app=""
    base_path="$(base_path_for "$service")" || base_path=""
    console_path="$(console_probe_path_for "$service")" || console_path=""
    if [ -z "$app" ] || [ -z "$base_path" ] || [ -z "$console_path" ]; then
      echo "::error::smoke_consoles: unknown console zone '${service}' — add it to app_for, base_path_for and console_probe_path_for." >&2
      ec=1
      continue
    fi
    # BINARY, not a list. With a third zone C this picks ONE other prefix, so C's chunk would be
    # probed against /iam alone, with nothing saying so. (That step-4 row proves only that a
    # basePath is in effect — see its comment — not that zones do not collide.) The
    # PAIGASUS_ZONES JSON literal in the `docker run` below is a third hardcoded copy of the same
    # two-zone assumption. A third zone needs both rewritten, not extended.
    if [ "$service" = "iam" ]; then other="/gateway"; else other="/iam"; fi
    name="smoke-${app}-${RUN_ID}"
    # Registered BEFORE the container is created, so a `docker run` that fails part-way still has
    # its name cleaned up. $RUN_ID is the same $$ suffix every other container name in this file
    # carries, so two concurrent runs against one daemon never collide.
    CONSOLE_SMOKE_NAMES="$CONSOLE_SMOKE_NAMES $name"

    echo "== smoke ${app} =="
    bad=0
    started=0
    # -p 0:3000 asks the daemon for a free ephemeral port. GUARDED: without it a name collision or
    # an image that will not start aborts the script on docker's own message and the gateway
    # console is never checked.
    # The rc is captured, not folded into an empty string: docker's own message names the real
    # reason (no such image, name already in use, port already allocated, daemon unreachable) and
    # discarding it leaves the reader with a guess. It is printed on its own lines rather than
    # inside the annotation, because a `::error::` line that carries an embedded newline stops
    # being one annotation.
    run_rc=0
    run_out="$(docker run -d --name "$name" -p 0:3000 \
      -e PAIGASUS_ZONE="$service" \
      -e PAIGASUS_ZONES="{\"iam\":\"/iam\",\"gateway\":\"/gateway\"}" \
      "${CONSOLE_SMOKE_ENV[@]}" \
      "${app}:dev" 2>&1)" || run_rc=$?
    if [ "$run_rc" -ne 0 ]; then
      echo "::error::${app}: the container did not start from ${app}:dev — docker exited ${run_rc}; its own message follows. If the image is missing, run 'ci/images/run.sh build-console ${service}' first." >&2
      printf '%s\n' "$run_out" >&2
      ec=1; bad=1
    else
      started=1
    fi

    if [ "$bad" -eq 0 ]; then
      # `sed -n 1p` reads its whole input and is the approved substitute for `head -1` (the
      # repo bans piping into an early-exit reader). It is needed because `docker port` prints one
      # line per published binding — an IPv6 line as well as the IPv4 one on some daemons — and
      # `${port##*:}` over both lines would take the second binding's port.
      port="$(docker port "$name" 3000/tcp | sed -n 1p)" || port=""
      port="${port##*:}"
      case "$port" in
        ''|*[!0-9]*)
          # A container that starts and then crashes at once also leaves `docker port` empty, so
          # the state is read to tell the two causes apart. GUARDED: the container can already be
          # gone, and `docker inspect` then exits non-zero; an empty state takes the generic arm.
          cstate="$(docker inspect -f '{{.State.Status}} {{.State.ExitCode}}' "$name" 2>/dev/null)" || cstate=""
          case "$cstate" in
            exited*|dead*)
              echo "::error::${app}: the container started and then EXITED (state/exit code: ${cstate}) before its port could be read — the image's entrypoint crashed. Its last log lines follow." >&2
              ;;
            *)
              echo "::error::${app}: could not read the published host port for container port 3000/tcp ('docker port' gave '${port:-<nothing>}', state '${cstate:-<unreadable>}') — the container started but published no port. Its last log lines follow." >&2
              ;;
          esac
          docker logs "$name" 2>&1 | tail -30 >&2 || true
          ec=1; bad=1
          ;;
      esac
    fi

    origin=""
    if [ "$bad" -eq 0 ]; then origin="http://127.0.0.1:${port}"; fi

    # Step 1: the page renders. Two curl calls, not one.
    #
    # The CANONICAL url, with NO trailing slash. Next compiles an internal, priority 308 redirect
    # for "<basePath>/" -> "<basePath>" whenever trailingSlash is false (the default; neither
    # console's next.config.ts sets it), confirmed by reading .next/routes-manifest.json out of the
    # built image. A request to "<basePath>/" without -L therefore reads a 4-byte redirect body and
    # finds no chunk URL in it — an assertion that can never pass on a correct image.
    # `-L --max-redirs 3` follows that redirect if it ever fires, so this keeps working if
    # trailingSlash is flipped to true and the redirect direction reverses. `--max-time` bounds the
    # request so a hung connection, or -L chasing an off-origin redirect (a future auth gate
    # sending this to a real IdP), cannot run the CI gate indefinitely.
    status=""
    if [ "$bad" -eq 0 ]; then
      # GUARDED: a 500 from an invalid runtime config, or any other non-2xx, must reach the named
      # message below rather than abort the script after ~20s of retries on curl's generic one.
      # `-w '%{http_code}'` on a status-only probe (-o /dev/null, no -f) carries the HTTP status
      # into that message without entangling status parsing with the page BODY the second call
      # grabs; a combined single-curl capture was considered and rejected as the more fragile.
      status="$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 -L --max-redirs 3 \
        --retry 20 --retry-delay 1 --retry-all-errors "${origin}${base_path}")" || status=""
      if [ "$status" != "200" ]; then
        echo "::error::${app}: ${base_path} did not render (HTTP ${status:-no response}) — check the container's runtime configuration (CONSOLE_SMOKE_ENV above) against ts/apps/${app}/lib/config.ts, or read 'docker logs ${name}'." >&2
        docker logs "$name" 2>&1 | tail -30 >&2 || true
        ec=1; bad=1
      fi
    fi

    # The image's OWN HEALTHCHECK program (console_healthcheck_row above, SMA-670 gap 3). It runs
    # only once the page rendered, because the probe it makes is the same server's
    # <basePath>/healthz. It does not set `bad`: the chunk rows below do not depend on it.
    if [ "$bad" -eq 0 ]; then
      console_healthcheck_row "$name" "$app" "$base_path" "$CONSOLE_HC_DEADLINE" || ec=1
    fi

    # SMA-634. A (console) route, which imports @paigasus/console-core and so evaluates the kernel's
    # wasm. A wasm that cannot load 500s every `(console)` page while the public page above stays
    # 200. This is the only runtime proof this suite has that a console image loads the kernel.
    #
    # ONLY A 3xx PASSES. The earlier rule — any non-5xx — accepted a 404, and a 404 is exactly the
    # answer this probe must not trust: when the route is renamed or removed, Next answers from the
    # ROOT not-found, the `(console)` layout never evaluates, and the probe reports success on a run
    # that proved nothing. Deliberately no `-L`: the redirect is the assertion, and following it
    # would reach the IdP and lose it.
    #
    # The route is PER ZONE, and it did not used to be. Both zones were probed at a hardcoded
    # `/orgs`; gateway-console has no such page — its zone overview is `(console)/overview` and its
    # only `orgs` routes are parameterised — so `/gateway/orgs` answered from the root not-found on
    # every run, and that half of this check asserted nothing at all. See console_probe_path_for.
    #
    # RESIDUAL, MEASURED on this host, stated so a green here is not read as more than it is. The
    # 3xx comes from proxy.ts's middleware, not from the route: proxy.ts gates the whole zone on
    # cookie PRESENCE (ADR-0017 decision 7) and deliberately imports no @paigasus/console-core, so a
    # cookie-less request is turned back before Next routes it and before any kernel code runs. A
    # path that does NOT exist answers 307 here too (measured with `/gateway/orgs`). So this probe
    # rejects a plain 404 and a 500, but it still does not prove the `(console)` layout evaluated.
    #
    # The obvious fix does not work in THIS suite, and here is the measurement, so the next person
    # does not spend the run finding out again. Sending a forged `__Host-pgs_sid` cookie does carry
    # the request past the middleware — the absent-path control answered 404, the real route did
    # not — but every `(console)` route then answered 500, from
    # `AuthConfigError: PAIGASUS_SESSION_STORE cannot be "memory" when PAIGASUS_ZONES declares more
    # than one zone`, thrown in `authRuntime` before the page renders. A wasm failure 500s the same
    # way, so the two are not distinguishable by status. Closing this residual means giving the
    # smoke containers a session store they can actually use, which is a change to CONSOLE_SMOKE_ENV
    # and to what this suite deploys, not a change to this probe.
    if [ "$bad" -eq 0 ]; then
      console_status="$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 --retry 5 --retry-delay 1 \
        --retry-all-errors "${origin}${base_path}${console_path}")" || console_status=""
      case "$console_status" in
        3??) echo "  ${app}: ${base_path}${console_path} redirects (${console_status}) — the route exists and nothing 500s" ;;
        *)
          echo "::error::${app}: ${base_path}${console_path} answered '${console_status:-no response}' — a (console) route must redirect an unauthenticated request (3xx). A 5xx is usually the kernel's wasm chunk failing to load, which 500s every (console) page. A 404 means the route is gone: Next then answers from the ROOT not-found, the (console) layout never evaluates, and nothing here proves the kernel loads — add the zone's route to console_probe_path_for. Read 'docker logs ${name}'." >&2
          docker logs "$name" 2>&1 | tail -30 >&2 || true
          ec=1; bad=1
          ;;
      esac
    fi

    html=""
    if [ "$bad" -eq 0 ]; then
      # The probe already confirmed 200, but the container can still regress between the two calls
      # — a connection drop, a timeout the probe's retries happened to dodge, or -L exceeding
      # --max-redirs — so this call is GUARDED too and carries its own, smaller retry budget: the
      # probe's 20 retries absorb the container's startup wait, and these 5 cover only a transient
      # error on a server that already answered 200.
      html="$(curl -fsSL --max-redirs 3 --max-time 30 --retry 5 --retry-delay 1 \
        --retry-all-errors "${origin}${base_path}")" || html=""
      if [ -z "$html" ]; then
        echo "::error::${app}: ${base_path} answered HTTP 200 to the status probe, but the body fetch itself failed or returned nothing (connection drop, timeout, or too many redirects) — a transport failure between the two calls, not a staging or runtime-configuration failure." >&2
        ec=1; bad=1
      fi
    fi

    # Step 2: extract one chunk URL. No pipe into an early-exit reader: `grep -oE` and `sort -u`
    # both read their whole input, and `sed -n 1p` is the approved stand-in for `head -1`.
    # GUARDED: `grep -oE` exits 1 on no match even after reading everything, and under
    # `set -o pipefail` an unguarded assignment from that pipeline would abort the script here
    # instead of reaching the named branch below.
    chunk=""
    if [ "$bad" -eq 0 ]; then
      chunk="$(printf '%s' "$html" | grep -oE "${base_path}/_next/static/[^\"']+\.js" | sort -u | sed -n 1p)" || chunk=""
      if [ -z "$chunk" ]; then
        # Step 1 already confirmed HTTP 200 and followed any redirect, so an invalid runtime
        # configuration and an unfollowed redirect are no longer possible causes HERE; naming them
        # would send a future reader to the wrong place.
        echo "::error::${app}: ${base_path} rendered (HTTP 200) but no ${base_path}/_next/static/*.js URL appears in the page — check the page markup or the basePath wiring, not staging or runtime configuration." >&2
        ec=1; bad=1
      fi
    fi

    # Step 3: the chunk is served, with a body. When .next/static was never staged the file is
    # simply ABSENT from the served tree, so this is a straight 404 rather than a 200 with an empty
    # body, and `curl -fsS` fails outright — GUARDED for the same reason as step 2.
    bytes=""
    if [ "$bad" -eq 0 ]; then
      bytes="$(curl -fsS --max-time 30 "${origin}${chunk}" | wc -c | tr -d ' ')" || bytes=""
      if [ "${bytes:-0}" -lt 1 ]; then
        echo "::error::${app}: ${chunk} is not served (404 or empty body) — .next/static was not staged into the image; see the staging RUN block in ts/Dockerfile." >&2
        ec=1; bad=1
      fi
    fi

    # Step 4: the other zone's prefix does not serve it. SMA-513 acceptance criterion 3 has two
    # halves (SMA-670 spec § 4.4). Steps 2 to 4 of this smoke test prove the PER-IMAGE half
    # (SMA-513 spec D4): each zone emits its asset URLs under its own basePath and serves them
    # there. This step alone proves only that a basePath is in effect in this one container:
    # MEASURED on iam-console:dev, the real chunk also 404s under /zzz/… and with no prefix at all,
    # so any unknown prefix gives this result, and this step does not prove that the two zones'
    # assets do not collide. The ONE-ORIGIN half is kind row R2
    # (ts/apps/iam-console/tests/cluster/phase-a/cross-zone.spec.ts): both zones hydrate through
    # one Traefik ingress.
    # GUARDED: `curl` without -f still exits non-zero on a connection failure or a timeout, and an
    # unguarded capture would abort the script before the named message.
    if [ "$bad" -eq 0 ]; then
      code="$(curl -s -o /dev/null -w '%{http_code}' --max-time 30 \
        "${origin}${other}${chunk#"$base_path"}")" || code=""
      if [ -z "$code" ]; then
        echo "::error::${app}: the ${other} probe got no HTTP response at all (connection failure or timeout) — the basePath check could not be made." >&2
        ec=1
      elif [ "$code" != "404" ]; then
        echo "::error::${app}: ${other} also served the chunk (HTTP ${code}); the chunk is served outside ${base_path}, so no basePath is in effect." >&2
        ec=1
      else
        echo "  ${app}: serves ${chunk} (${bytes} bytes), 404 under ${other} (a basePath is in effect: the per-image half of AC 3; the one-origin half is kind row R2, cross-zone.spec.ts)"
      fi
    fi

    # The identity checks do not depend on the chunk chain, so they run even when it failed.
    # `-o pid,uid`, not `-o user`: `user` is resolved through NSS, so on a Linux runner where uid
    # 65532 has a synthesized name this would print that name and false-negative a correct image.
    # This mirrors assert_uid above; docker requires `pid` in the format to correlate processes.
    if [ "$started" -eq 1 ]; then
      uid="$(docker top "$name" -o pid,uid 2>/dev/null | sed -n 2p | awk '{print $NF}')" || uid=""
      if [ "$uid" != "65532" ]; then
        echo "::error::${app} runs as uid ${uid:-<unreadable>}; the console images must run as 65532." >&2
        ec=1
      else
        echo "  ${app}: runs as uid 65532"
      fi
    else
      echo "::error::${app}: uid not checked — the container never started." >&2
    fi

    # Only 127 is the pass. MEASURED on this host (Docker 29.8.0): the probe exits 0 when a shell
    # exists (alpine:latest), 127 when the image has no /bin/sh (the distroless images under test),
    # and 125 when docker itself refuses before the entrypoint runs — a missing or unpullable
    # image. A bare `if … else` folds that 125 into the else branch and prints a GREEN
    # "no shell in the runtime image" row for an image that was never read, which is the shape
    # the uid check three lines above already avoids.
    sh_rc=0
    docker run --rm --entrypoint /bin/sh "${app}:dev" -c true >/dev/null 2>&1 || sh_rc=$?
    if [ "$sh_rc" -eq 0 ]; then
      echo "::error::${app}:dev has a shell; the runtime base must stay distroless." >&2
      ec=1
    elif [ "$sh_rc" -eq 127 ]; then
      echo "  ${app}: no shell in the runtime image"
    else
      echo "::error::${app}: shell absence NOT checked — docker exited ${sh_rc} on ${app}:dev before reaching an entrypoint, so the image is missing or unreadable and nothing was proved about the runtime base." >&2
      ec=1
    fi

    # SMA-670: image-only rows. They need only the image, so they run whether or not the
    # container started. ci/images/console-selftest.sh pins each call line, `|| ec=1` included.
    console_node_version_row "$app" || ec=1
    console_image_config_row "$app" || ec=1

    # Spec § 5.5 assertion 4 — staged-tree parity. ts/Dockerfile's staging of .next/static and
    # public/ and ts/apps/<app>/moon.yml's `build` script are a SECOND staging site each, created
    # deliberately; this is what keeps the two from drifting apart.
    #
    # It compares the staged TREES, not the two scripts' text, so the two divergences the Task 2
    # review recorded are tolerated by construction: moon.yml's `rm -rf .next/static` before
    # `next build` (the Dockerfile needs no counterpart — `**/.next` in ts/.dockerignore makes every
    # builder start cold) and moon.yml's app-name prefix on its two error messages both leave the
    # staged tree identical.
    host_std="$ROOT/ts/apps/${app}/.next/standalone/apps/${app}"
    host_static="$host_std/.next/static"
    if [ -d "$host_static" ]; then
      # Two causes, two messages, and the rc separates them. node's own uncaught ENOENT on
      # .next/static (or on .next/BUILD_ID) exits 1, and only THAT says the staging copy did not
      # run. docker refusing the image exits 125/126/127 before node starts, which proves nothing
      # about staging at all. stderr is captured rather than discarded, for the same reason as the
      # `docker run -d` above: the tool's own message names the cause.
      img_rc=0
      img_out="$(docker run --rm --entrypoint /nodejs/bin/node "${app}:dev" \
        -e "$CONSOLE_STAGED_TREE_JS" "/app/apps/${app}" 2>&1)" || img_rc=$?
      if [ "$img_rc" -eq 1 ]; then
        echo "::error::${app}: /app/apps/${app}/.next/static is absent or unreadable inside the image — the staging copy in ts/Dockerfile did not run. node's message follows." >&2
        printf '%s\n' "$img_out" >&2
        ec=1
      elif [ "$img_rc" -ne 0 ]; then
        echo "::error::${app}: staged-tree parity NOT checked — docker exited ${img_rc} on ${app}:dev before node ran, so the image is missing or unreadable and nothing was proved about staging. Its message follows." >&2
        printf '%s\n' "$img_out" >&2
        ec=1
      elif [ -z "$img_out" ]; then
        echo "::error::${app}: the staged-tree walk exited 0 but printed nothing — that is neither a staging failure nor a docker failure; inspect ${app}:dev by hand." >&2
        ec=1
      else
        # Line 1 is the public= marker, the rest is the tree. `sed -n 1p` / `sed -n '2,$p'` read
        # their whole input; neither is an early-exit reader.
        img_public="$(printf '%s\n' "$img_out" | sed -n 1p)"
        img_list="$(printf '%s\n' "$img_out" | sed -n '2,$p' | LC_ALL=C sort)"
        host_public="public=0"
        if [ -d "$host_std/public" ]; then host_public="public=1"; fi
        host_id="$(cat "$host_std/.next/BUILD_ID" 2>/dev/null)" || host_id=""
        if [ -z "$host_id" ]; then
          echo "::error::${app}: the host build at ${host_std} has no .next/BUILD_ID — that build is broken or half-written; re-run 'moon run ${app}-ts:build'." >&2
          ec=1
        else
          host_list="$(cd "$host_static" && find . -type f | sed 's#^\./##' \
            | sed "s#^${host_id}/#<BUILD_ID>/#" | LC_ALL=C sort)" || host_list=""
          if [ "$img_public" != "$host_public" ]; then
            echo "::error::${app}: the image staged ${img_public} but the host build staged ${host_public} — ts/Dockerfile and ts/apps/${app}/moon.yml disagree on staging public/." >&2
            ec=1
          fi
          if [ "$img_list" != "$host_list" ]; then
            # Two distinct causes produce a difference here, and they send a reader to different
            # places, so they get different messages. A missing or partial staging copy changes
            # which TOP-LEVEL directories exist under .next/static; two builds of different source
            # keep the same directories and change only the content-hashed file names inside them.
            #
            # ASSUMPTION, recorded deliberately: a host build and an image build of the SAME source
            # produce the same chunk file names. Measured true here — Turbopack derives them from
            # content — but nothing enforces it. Four things would break it, and all four are
            # toolchain events rather than code changes: a Next or Turbopack bump that changes
            # chunk hashing; a compile-time variable that differs between the host build and the
            # builder stage; the platform split (macOS host against a linux builder); and the
            # Dockerfile's filtered `pnpm install` resolving a different optional platform
            # dependency. When it breaks it breaks on EVERY run, loudly, into the branch below
            # whose message already says this is not a Dockerfile-vs-moon.yml drift. That is an
            # acceptable failure shape, so there is no shape-only fallback here on purpose.
            img_dirs="$(printf '%s\n' "$img_list" | sed 's#/.*##' | LC_ALL=C sort -u)"
            host_dirs="$(printf '%s\n' "$host_list" | sed 's#/.*##' | LC_ALL=C sort -u)"
            if [ "$img_dirs" != "$host_dirs" ]; then
              echo "::error::${app}: the image's staged .next/static holds different top-level directories from the host build's — ts/Dockerfile and ts/apps/${app}/moon.yml have drifted. Diff (< host, > image) follows." >&2
            else
              echo "::error::${app}: the image's staged .next/static holds the same directories as the host build's but different files, so the two were built from DIFFERENT sources. Re-run 'moon run ${app}-ts:build' so this parity claim compares like with like; on its own this is NOT a ts/Dockerfile vs ts/apps/${app}/moon.yml drift. If a FRESH build does not clear this, that is a finding — report it; do NOT delete .next to silence it, because that only moves this check into its 'not checked' arm. Diff (< host, > image) follows." >&2
            fi
            diff <(printf '%s\n' "$host_list") <(printf '%s\n' "$img_list") >&2 || true
            ec=1
          else
            echo "  ${app}: staged tree matches the host build ($(printf '%s\n' "$img_list" | grep -c . || true) files, ${img_public})"
          fi
        fi
      fi
    else
      # Deliberate, and it says so out loud rather than passing silently: a check that quietly
      # skips is the failure mode this repository has paid for repeatedly. Nothing here sets ec —
      # the absence of a host build is not a defect.
      #
      # LOCAL ONLY, and the message says so. .github/workflows/images.yml runs no host build, so
      # CI ALWAYS takes this arm and the staged-tree parity check gates NOTHING there. Making it
      # gate would mean adding a TypeScript toolchain and a second Next build to that job. That is
      # a follow-up, recorded in the PR description and in docs/ops/RUNBOOK-containers.md — until
      # it lands, do not read a green CI `all-consoles` as parity coverage.
      echo "  ${app}: no host build at ${host_static}; staged-tree parity NOT CHECKED this run (CI never runs a host build, so a green CI run is NOT parity coverage — run 'moon run ${app}-ts:build' locally to check it)"
    fi
  done

  console_smoke_cleanup
  if [ "$ec" -ne 0 ]; then return 1; fi
  echo "== CONSOLE SMOKE OK =="
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
USAGE="usage: ci/images/run.sh build [iam|gateway] | ci/images/run.sh build-oci <iam|gateway> <outdir> | ci/images/run.sh load-oci <archive> <image-name> | ci/images/run.sh smoke [iam|gateway]... | ci/images/run.sh all | ci/images/run.sh rehearse <archive.oci.tar>... | ci/images/run.sh build-console [iam|gateway] | ci/images/run.sh all-consoles"
cmd="${1:?$USAGE}"
target="${2:-}"
services=("iam" "gateway")
[ -n "$target" ] && services=("$target")
console_services=("iam" "gateway")
[ -n "$target" ] && console_services=("$target")

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
  build-console)
    assert_console_pins
    for s in "${console_services[@]}"; do build_console_one "$s"; done
    ;;
  all-consoles)
    if [ -n "$target" ]; then
      echo "usage: ci/images/run.sh all-consoles takes no service argument — use 'build-console [iam|gateway]' to build one" >&2
      exit 1
    fi
    assert_console_pins
    for s in "${console_services[@]}"; do build_console_one "$s"; done
    # The zone list is passed, not restated inside smoke_consoles, so the build loop and the smoke
    # loop cannot disagree about which zones this run covers.
    smoke_consoles "${console_services[@]}"
    ;;
  *)
    echo "unknown command: $cmd" >&2
    echo "$USAGE" >&2
    exit 1
    ;;
esac
