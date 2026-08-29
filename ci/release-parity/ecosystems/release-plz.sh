#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# release-plz ecosystem module for the SMA-398 parity harness.
# Interface: ecosystem::build_fixture / apply_commit / run_update / version
set -euo pipefail

A_CRATE="paigasus-release-parity-a"
B_CRATE="paigasus-release-parity-b"

# Abort with the harness's OWN vocabulary. run.sh prints "infrastructure error
# (rc=2)" at :67 and :81, but this module is sourced at run.sh:21 — so an exit from
# here fires DURING the source and run.sh never reaches either line. Without this
# string the abort would be unclassifiable, and CLAUDE.md tells readers to grep for
# it. Deliberately duplicated in python-semantic-release.sh rather than shared: one
# module is sourced per run, and a ci/lib/ layer was considered and rejected
# (SMA-596 D4).
_rp_fatal() { # line...
  echo "FATAL: release-parity ABORTED: infrastructure error (rc=2)" >&2
  printf '       %s\n' "$@" >&2
  exit 2
}

# release-plz and the `cargo metadata` it spawns run inside a temp fixture OUTSIDE
# this repo. The proto `release-plz` shim resolves its version by walking up from
# CWD to find .prototools — from /tmp that fails (CI: proto::tool::unknown_id;
# recorded on that CI run, NOT reproduced locally — SMA-596 L2). So resolve the
# absolute tool binaries once, from the repo, and invoke those directly. Do not
# delete this dance as redundant; it is what stops the SMA-398 bug returning.
#
# `--reporter text` is REQUIRED, not cosmetic. proto prints NDJSON on stdout when it
# detects an agent environment (AI_AGENT / CLAUDECODE / CLAUDE_CODE_ENTRYPOINT), and
# `proto bin` still exits 0 while doing it — so an unflagged capture silently yields
# a JSON blob instead of a path, and every `||` fallback is skipped because nothing
# failed. That is SMA-596.
#
# There is deliberately NO fallback. This harness exists to compare one pinned
# release-plz's classification behaviour; a silently substituted binary produces a
# verdict about the wrong tool. Failing loudly is correct here.
#
# `exit` from a module that run.sh sources at top level is intentional — do NOT
# "fix" it to `return`. run.sh parses its arguments (:12) and handles --help (:15)
# before the source (:21), so this cannot pre-empt either.
_RP_SELF="${BASH_SOURCE[0]:-$0}"
_RP_REPO_ROOT="$(cd "$(dirname "$_RP_SELF")/../../.." && pwd)"
RELEASE_PLZ_BIN="$(cd "$_RP_REPO_ROOT" && proto --reporter text bin release-plz)" || _rp_fatal \
  "release-plz.sh: 'proto --reporter text bin release-plz' failed." \
  "Run 'proto install' from the repo root." \
  "An older proto without --reporter also lands here (SMA-596 D1)."
# -f AND -x: `-x` alone is true for a searchable DIRECTORY, and invoking a
# directory fails with status 126 further down — the late, confusing failure this
# assertion exists to prevent.
if [ ! -f "$RELEASE_PLZ_BIN" ] || [ ! -x "$RELEASE_PLZ_BIN" ]; then _rp_fatal \
  "release-plz.sh: release-plz did not resolve to an executable file." \
  "Got: ${RELEASE_PLZ_BIN:-<empty>}" \
  "If that looks like JSON, proto's agent-mode NDJSON leaked past --reporter text (SMA-596)."
fi

# release-plz shells out to `cargo metadata`; pass an explicit, CWD-independent
# cargo (rustup proxy / real binary, not a CWD-sensitive shim). This fallback is
# KEPT, unlike release-plz's: it is a real reachable default, and cargo is not the
# tool under test. The assertion below is what stops a bad value surfacing later as
# a confusing cargo error instead of a resolution error (SMA-596 D2.1).
CARGO_BIN="$( command -v cargo 2>/dev/null || true )"
[ -n "$CARGO_BIN" ] || CARGO_BIN="$HOME/.cargo/bin/cargo"
if [ ! -f "$CARGO_BIN" ] || [ ! -x "$CARGO_BIN" ]; then _rp_fatal \
  "release-plz.sh: cargo did not resolve to an executable file." \
  "Got: ${CARGO_BIN:-<empty>}" \
  "Install Rust, or put cargo on PATH."
fi

ecosystem::_crate_dir() { # dir slot(a|b) -> path
  case "$2" in
    a) printf '%s/crates/%s' "$1" "$A_CRATE" ;;
    b) printf '%s/crates/%s' "$1" "$B_CRATE" ;;
    *) echo "bad slot: $2" >&2; return 1 ;;
  esac
}

ecosystem::_derive_config() { # real_toml out_toml   (F3)
  local real="$1" out="$2" key
  # F3: copy the classification knob from the REAL config verbatim; fail loudly
  # if it's gone, so the harness can't silently test stale settings.
  key="$(grep -E '^[[:space:]]*features_always_increment_minor[[:space:]]*=' "$real" || true)"
  if [ -z "$key" ]; then
    echo "FATAL: rs/release-plz.toml lacks features_always_increment_minor — parity would test stale settings" >&2
    return 1
  fi
  {
    echo "[workspace]"
    printf '%s\n' "${key#"${key%%[![:space:]]*}"}"        # left-trimmed, verbatim
    echo "semver_check = false"                            # orthogonal to classification
    echo "git_only = true"                                 # avoids crates.io registry lookup for nonexistent fixture crates
    # Tera template so release-plz looks for tags like paigasus-release-parity-a-v0.1.0,
    # matching what build_fixture creates. Without this, git_only defaults to ^v(\d+\.\d+\.\d+)$.
    echo 'git_tag_name = "{{ package }}-v{{ version }}"'
  } >"$out"
}

ecosystem::build_fixture() { # dir real_release_plz_toml
  local dir="$1" real="$2" c
  mkdir -p "$dir/crates/$A_CRATE/src" "$dir/crates/$B_CRATE/src"
  cat >"$dir/Cargo.toml" <<'EOF'
[workspace]
resolver = "3"
members = ["crates/*"]
EOF
  for c in "$A_CRATE" "$B_CRATE"; do
    cat >"$dir/crates/$c/Cargo.toml" <<EOF
[package]
name = "$c"
version = "0.1.0"
edition = "2024"
publish = false
EOF
    echo "// seed" >"$dir/crates/$c/src/lib.rs"
  done
  ecosystem::_derive_config "$real" "$dir/release-plz.toml"
  (
    cd "$dir"
    git init -q
    git config user.email "parity@example.com"
    git config user.name "parity"
    git config commit.gpgsign false   # fixture repo: never sign (CI/dev have no key)
    git config tag.gpgsign false
    git add -A
    git commit -qm "chore: seed fixture"
    git tag "$A_CRATE-v0.1.0"   # release-plz default workspace tag pattern
    git tag "$B_CRATE-v0.1.0"
  )
}

ecosystem::apply_commit() { # dir slot subject footer
  local dir="$1" slot="$2" subject="$3" footer="$4" cdir
  cdir="$(ecosystem::_crate_dir "$dir" "$slot")"
  printf '// change for: %s\n' "$subject" >>"$cdir/src/lib.rs"
  (
    cd "$dir"
    git add -A
    if [ "$footer" = "-" ]; then
      git commit -qm "$subject"
    else
      git commit -qm "$subject" -m "$footer"
    fi
  )
}

ecosystem::run_update() { # dir
  # Disposable fixture: let `update` write, then read the result. Offline so the
  # crates.io index isn't consulted for the (nonexistent) fixture crate names.
  # Capture output and replay it on failure so CI failures are diagnosable.
  local out
  if ! out="$(cd "$1" && CARGO="$CARGO_BIN" CARGO_NET_OFFLINE=true "$RELEASE_PLZ_BIN" update 2>&1 >/dev/null)"; then
    printf '%s\n' "$out" >&2
    return 1
  fi
}

ecosystem::version() { # dir slot -> version string
  local cdir
  cdir="$(ecosystem::_crate_dir "$1" "$2")"
  grep -m1 -E '^version[[:space:]]*=' "$cdir/Cargo.toml" | sed -E 's/.*"([^"]+)".*/\1/'
}
