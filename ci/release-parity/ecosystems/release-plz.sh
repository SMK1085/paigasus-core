#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# release-plz ecosystem module for the SMA-398 parity harness.
# Interface: ecosystem::build_fixture / apply_commit / run_update / version
set -euo pipefail

A_CRATE="paigasus-release-parity-a"
B_CRATE="paigasus-release-parity-b"

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
    git add -A
    git commit -qm "chore: seed fixture"
    # -c tag.gpgSign=false: the fixture repo is throwaway; global tag.gpgSign=true
    # would otherwise require a message and a signing key that CI doesn't have.
    git -c tag.gpgSign=false tag "$A_CRATE-v0.1.0"   # release-plz default workspace tag pattern
    git -c tag.gpgSign=false tag "$B_CRATE-v0.1.0"
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
  ( cd "$1" && CARGO_NET_OFFLINE=true release-plz update >/dev/null 2>&1 )
}

ecosystem::version() { # dir slot -> version string
  local cdir
  cdir="$(ecosystem::_crate_dir "$1" "$2")"
  grep -m1 -E '^version[[:space:]]*=' "$cdir/Cargo.toml" | sed -E 's/.*"([^"]+)".*/\1/'
}
