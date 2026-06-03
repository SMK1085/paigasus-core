#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# python-semantic-release ecosystem module for the SMA-398 parity harness (SMA-405).
# Interface: ecosystem::build_fixture / apply_commit / run_update / version
#
# PSR has NO path-based monorepo attribution: it versions one package from ALL
# commits since that package's last matching tag, regardless of changed files. So
# unlike release-plz's single repo + two crates, each slot here is its OWN git
# repo. A commit applied to slot `a` is invisible to slot `b`, so `b` staying at
# baseline tests "PSR makes no release without a qualifying commit" — the analogue
# of release-plz's path->package attribution, which is a release-plz/cargo concern
# PSR does not claim (SMA-385). See ci/release-parity/README.md.
set -euo pipefail

A_MOD="paigasus_release_parity_a"
B_MOD="paigasus_release_parity_b"
A_TAGFMT="paigasus-release-parity-a-v{version}"
B_TAGFMT="paigasus-release-parity-b-v{version}"
BASELINE="0.1.0"

# PSR is a Python package in py/'s uv dev-deps. The fixture lives in /tmp, outside
# the repo, where `uv run` can't resolve the project — so resolve the absolute
# semantic-release binary once, from py/ (mirrors release-plz.sh's RELEASE_PLZ_BIN).
# `uv run --frozen` also bootstraps py/.venv from uv.lock (there is no separate
# `uv sync` CI step).
_PSR_SELF="${BASH_SOURCE[0]:-$0}"
_PSR_REPO_ROOT="$(cd "$(dirname "$_PSR_SELF")/../../.." && pwd)"
PSR_BIN="$( (cd "$_PSR_REPO_ROOT/py" && uv run --frozen python -c 'import shutil,sys; sys.stdout.write(shutil.which("semantic-release") or "")') 2>/dev/null || true )"
[ -n "$PSR_BIN" ] || PSR_BIN="$( command -v semantic-release 2>/dev/null || echo semantic-release )"

# Real configs the fixture derives its classification settings from (F3). Both
# packages must agree (review F1).
_PSR_ML_TOML="$_PSR_REPO_ROOT/py/packages/paigasus-ml/pyproject.toml"
_PSR_WF_TOML="$_PSR_REPO_ROOT/py/packages/paigasus-workflows/pyproject.toml"

ecosystem::_slot_dir() { # dir slot(a|b) -> path
  case "$2" in
    a|b) printf '%s/%s' "$1" "$2" ;;
    *) echo "bad slot: $2" >&2; return 1 ;;
  esac
}

ecosystem::_slot_mod() { # slot -> python module name
  case "$1" in a) printf '%s' "$A_MOD" ;; b) printf '%s' "$B_MOD" ;; *) return 1 ;; esac
}

ecosystem::_slot_baseline_tag() { # slot -> baseline tag
  case "$1" in
    a) printf 'paigasus-release-parity-a-v%s' "$BASELINE" ;;
    b) printf 'paigasus-release-parity-b-v%s' "$BASELINE" ;;
    *) return 1 ;;
  esac
}

# Extract a flat `key = value` from a [tool.semantic_release] block; empty if absent.
ecosystem::_psr_key() { # toml key -> value
  grep -E "^[[:space:]]*$2[[:space:]]*=" "$1" 2>/dev/null | head -n1 \
    | sed -E "s/^[[:space:]]*$2[[:space:]]*=[[:space:]]*//; s/[[:space:]]*(#.*)?$//" || true
}

# Derive + validate classification from the REAL configs (F3 + review F1/F2).
# Echoes "major_on_zero allow_zero_version"; fails loudly on a missing key, a
# non-true allow_zero_version, or an ml/workflows disagreement.
ecosystem::_derive_classification() {
  local ml_moz ml_azv wf_moz wf_azv
  ml_moz="$(ecosystem::_psr_key "$_PSR_ML_TOML" major_on_zero)"
  ml_azv="$(ecosystem::_psr_key "$_PSR_ML_TOML" allow_zero_version)"
  wf_moz="$(ecosystem::_psr_key "$_PSR_WF_TOML" major_on_zero)"
  wf_azv="$(ecosystem::_psr_key "$_PSR_WF_TOML" allow_zero_version)"

  if [ -z "$ml_moz" ] || [ -z "$wf_moz" ]; then
    echo "FATAL: PSR config lacks major_on_zero (ml='$ml_moz' wf='$wf_moz') — parity would test stale settings" >&2
    return 1
  fi
  if [ -z "$ml_azv" ] || [ -z "$wf_azv" ]; then
    echo "FATAL: PSR config lacks allow_zero_version (ml='$ml_azv' wf='$wf_azv') — PSR would leave 0.x and the breaking-row assertions become meaningless" >&2
    return 1
  fi
  if [ "$ml_azv" != "true" ] || [ "$wf_azv" != "true" ]; then
    echo "FATAL: allow_zero_version must be true (ml='$ml_azv' wf='$wf_azv') — PSR would jump to 1.0.0" >&2
    return 1
  fi
  if [ "$ml_moz" != "$wf_moz" ] || [ "$ml_azv" != "$wf_azv" ]; then
    echo "FATAL: paigasus-ml and paigasus-workflows PSR classification keys differ (major_on_zero: $ml_moz vs $wf_moz; allow_zero_version: $ml_azv vs $wf_azv) — both must honor the canonical contract" >&2
    return 1
  fi
  printf '%s %s' "$ml_moz" "$ml_azv"
}

ecosystem::_write_pkg() { # slot_dir module tagfmt major_on_zero allow_zero_version
  local d="$1" mod="$2" tagfmt="$3" moz="$4" azv="$5"
  mkdir -p "$d/src/$mod"
  cat >"$d/pyproject.toml" <<EOF
[project]
name = "${mod//_/-}"
version = "$BASELINE"
requires-python = ">=3.12"

[build-system]
requires = ["uv_build>=0.11.16,<0.12"]
build-backend = "uv_build"

[tool.semantic_release]
version_toml = ["pyproject.toml:project.version"]
tag_format = "$tagfmt"
major_on_zero = $moz
allow_zero_version = $azv
EOF
  echo "# seed" >"$d/src/$mod/__init__.py"
}

ecosystem::build_fixture() { # dir real_release_plz_toml(ignored)
  local dir="$1" classification moz azv a_dir b_dir d tag
  classification="$(ecosystem::_derive_classification)" || return 1
  moz="${classification%% *}"; azv="${classification##* }"
  a_dir="$(ecosystem::_slot_dir "$dir" a)"
  b_dir="$(ecosystem::_slot_dir "$dir" b)"
  ecosystem::_write_pkg "$a_dir" "$A_MOD" "$A_TAGFMT" "$moz" "$azv"
  ecosystem::_write_pkg "$b_dir" "$B_MOD" "$B_TAGFMT" "$moz" "$azv"
  for d in a b; do
    tag="$(ecosystem::_slot_baseline_tag "$d")"
    (
      # Pin the default branch to `main` for determinism (PSR's default branch
      # config matches main|master, but this avoids any host init.defaultBranch).
      cd "$(ecosystem::_slot_dir "$dir" "$d")"
      git -c init.defaultBranch=main init -q
      git config user.email "parity@example.com"
      git config user.name "parity"
      git config commit.gpgsign false   # fixture repo: never sign (CI/dev have no key)
      git config tag.gpgsign false
      git add -A
      git commit -qm "chore: seed fixture"
      git tag "$tag"
      # PSR always calls `git remote get-url origin`; add a placeholder so it
      # doesn't error out. The remote URL is never actually contacted.
      git remote add origin "file:///dev/null/parity-fixture-$(basename "$d")"
    )
  done
}

ecosystem::apply_commit() { # dir slot subject footer
  local dir="$1" slot="$2" subject="$3" footer="$4" d mod
  d="$(ecosystem::_slot_dir "$dir" "$slot")"
  mod="$(ecosystem::_slot_mod "$slot")"
  printf '# change for: %s\n' "$subject" >>"$d/src/$mod/__init__.py"
  (
    cd "$d"
    git add -A
    if [ "$footer" = "-" ]; then
      git commit -qm "$subject"
    else
      git commit -qm "$subject" -m "$footer"
    fi
  )
}

ecosystem::run_update() { # dir
  # `--print` computes the next version read-only (no file/git/build mutation);
  # PSR logs go to stderr, the version to stdout. Run BOTH slots so slot `b`
  # genuinely tests "no release without a qualifying commit". Stash each slot's
  # result in a per-slot sentinel that `version` reads (the run_update->version
  # split). Fallback for a no-bump slot whose PSR build errors/empties on --print:
  # if there are no commits since baseline, treat it as the unchanged baseline.
  local dir="$1" slot d tag out
  for slot in a b; do
    d="$(ecosystem::_slot_dir "$dir" "$slot")"
    tag="$(ecosystem::_slot_baseline_tag "$slot")"
    if out="$(cd "$d" && "$PSR_BIN" version --print 2>/dev/null)"; then
      out="$(printf '%s' "$out" | tr -d '[:space:]')"
    else
      out=""
    fi
    if [ -z "$out" ]; then
      if [ -n "$(cd "$d" && git log "$tag..HEAD" --oneline 2>/dev/null)" ]; then
        echo "FATAL: 'semantic-release version --print' produced no version in slot $slot (commits present)" >&2
        (cd "$d" && "$PSR_BIN" version --print) >&2 || true
        return 1
      fi
      out="$BASELINE"   # no qualifying commit since baseline -> unchanged
    fi
    printf '%s\n' "$out" >"$d/.parity-next-version"
  done
}

ecosystem::version() { # dir slot -> version string
  local d v
  d="$(ecosystem::_slot_dir "$1" "$2")"
  IFS= read -r v <"$d/.parity-next-version" || true
  printf '%s' "$v"
}
