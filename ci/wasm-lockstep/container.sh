#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# The two container runs of .github/workflows/wasm-lockstep.yml (SMA-693 spec 5.1, steps 4 and 6).
#
#   container.sh update   cargo update of the four family roots (cargo runs no build script)
#   container.sh build    proto, moon setup, the rustup components, pnpm install, generate-wasm
#
# This script runs INSIDE the build container only: `docker run --rm --user 65534:65534` over the
# work copy at /work, with no runner environment (no CI, no GITHUB_ACTIONS, no
# ACTIONS_RUNTIME_TOKEN). Do not run it on a host: it installs proto into $HOME, makes a git
# repository in /work and writes rs/target and ts/node_modules there.
#
# WHY THE USER nobody (65534) AND NOT THE RUNNER'S UID. generate-wasm --post calls Node's
# os.userInfo(), which throws when the uid has no passwd entry, and the image has no entry for the
# runner's uid 1001. `nobody` has one. The identity markers that --post then rejects are `nobody`
# and the last segment of HOME, `lockstep-home` (generate-wasm.mjs post()).
set -euo pipefail

export HOME=/tmp/lockstep-home
mkdir -p "$HOME"
cd /work

# proto prints NDJSON on stdout in an agent environment (SMA-609). Harmless here, and the task
# script of generate-wasm captures a node call, so keep the same rule.
export PROTO_REPORTER=text

case "${1:-}" in
  update)
    cd rs
    cargo update -p wasm-bindgen -p js-sys -p web-sys -p wasm-bindgen-futures
    ;;
  build)
    # Moon's vcs client is git, and the work copy has no .git (spec 5.1: the host made the copy
    # with git archive). The repository is made HERE, inside the container, so the host never
    # runs git on the work copy. safe.directory: /work belongs to nobody (uid 65534) after the chown.
    cat > "$HOME/.gitconfig" <<'EOF'
[safe]
	directory = /work
[user]
	name = wasm-lockstep
	email = wasm-lockstep@invalid
[commit]
	gpgsign = false
[maintenance]
	auto = false
[gc]
	auto = 0
EOF
    git init -q
    git add -A
    git commit -q -m "wasm-lockstep work copy"

    # The image sets RUSTUP_HOME=/usr/local/rustup and CARGO_HOME=/usr/local/cargo. proto and Moon
    # look for the Rust toolchain under HOME (~/.rustup, as on a runner), so point both there.
    # cargo update (run 1) needs neither and keeps the image values.
    # Result: this run downloads rustup and the Rust toolchain (pinned by .moon/toolchains.yml) at
    # run time. It does not use the toolchain of the image. Not verified: proto's check of rustup.
    export RUSTUP_HOME="$HOME/.rustup" CARGO_HOME="$HOME/.cargo"

    proto_version="$(sed -n 's/^proto = "\([0-9][0-9.]*\)"$/\1/p' .prototools)"
    case "$proto_version" in
      ''|*[!0-9.]*) echo "container.sh: no proto pin in .prototools (got '${proto_version}')" >&2; exit 2 ;;
    esac
    curl -fsSL https://moonrepo.dev/install/proto.sh -o "$HOME/proto-install.sh"
    bash "$HOME/proto-install.sh" "$proto_version" --yes --no-profile
    export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"

    proto install
    moon setup
    # The serial pre-install of ci.yml: concurrent first cargo calls race on ~/.rustup/downloads.
    ( cd rs && rustup component add rustfmt clippy && rustup target add wasm32-unknown-unknown )
    pnpm --dir ts install --frozen-lockfile
    moon run paigasus-kernel-ts:generate-wasm
    ;;
  *)
    echo "container.sh: usage: container.sh update|build" >&2
    exit 2
    ;;
esac
