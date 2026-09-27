#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# SMA-716: release-plz-only suites for `release_commits` and `changelog_include`.
#
# Sourced by ecosystems/release-plz.sh only. run.sh reaches this code through the hooks
# ecosystem::extra_suite and ecosystem::extra_negative_control. The python-semantic-release and
# semantic-release modules define neither hook, so repo:release-parity-py and -ts skip it.
# Not in cases.tsv: that file is the cross-tool parity contract, and the two other tools have no
# release_commits equivalent (spec section 4.3).
#
# Return codes of every rpf:: suite and check: 0 pass, 1 an assertion failed, 2 infrastructure.
# run.sh calls the hooks as `hook || rc=$?`. Bash turns errexit OFF inside a function that is
# called that way, so each step below checks its own status. Do not rely on `set -e` here.
#
# Uses ecosystem::_derive_config and ecosystem::run_update from release-plz.sh. Do not add a
# second release-plz call site in this file: repo:affected-smoke A10 waives only the one in
# ecosystem::run_update, by its exact text.
set -euo pipefail

# --- shared helpers -------------------------------------------------------------------------

# F3 for the new key: copy `release_commits` VERBATIM from the real config. A missing or a
# duplicated key is rc 2, so the suite cannot test a stale regex. The mutation modes still
# read the key first, so a control cannot pass because the real key is gone.
rpf::_release_commits_line() { # real_toml -> the key line on stdout
  local real="$1" n line
  n="$(grep -cE '^[[:space:]]*release_commits[[:space:]]*=' "$real" || true)"
  if [ "$n" != 1 ]; then
    echo "FATAL: rs/release-plz.toml has ${n:-0} release_commits lines, expected 1 (SMA-716 F3)" >&2
    return 2
  fi
  line="$(grep -E '^[[:space:]]*release_commits[[:space:]]*=' "$real")" || return 2
  printf '%s\n' "${line#"${line%%[![:space:]]*}"}"
}

rpf::_write_config() { # real_toml out_toml with_release_commits(0|1)
  local real="$1" out="$2" with_rc="$3" rc_line
  ecosystem::_derive_config "$real" "$out" || return 2
  rc_line="$(rpf::_release_commits_line "$real")" || return 2
  if [ "$with_rc" = 1 ]; then
    printf '%s\n' "$rc_line" >>"$out" || return 2
  fi
}

rpf::_git_init() { # dir
  ( cd "$1" &&
    git init -q &&
    git config maintenance.auto false &&   # SMA-708: no background maintenance in a fixture
    git config gc.auto 0 &&
    git config user.email "parity@example.com" &&
    git config user.name "parity" &&
    git config commit.gpgsign false &&
    git config tag.gpgsign false ) || return 2
}

rpf::_write_workspace() { # dir
  printf '[workspace]\nresolver = "3"\nmembers = ["crates/*"]\n' >"$1/Cargo.toml" || return 2
}

rpf::_write_crate() { # dir name [dependency line]
  local cdir="$1/crates/$2"
  mkdir -p "$cdir/src" || return 2
  printf '[package]\nname = "%s"\nversion = "0.1.0"\nedition = "2024"\npublish = false\n' "$2" \
    >"$cdir/Cargo.toml" || return 2
  if [ -n "${3-}" ]; then
    printf '\n[dependencies]\n%s\n' "$3" >>"$cdir/Cargo.toml" || return 2
  fi
  printf '// seed\n' >"$cdir/src/lib.rs" || return 2
}

rpf::_seed_and_tag() { # dir crate...
  local dir="$1" c
  shift
  ( cd "$dir" && git add -A && git commit -qm "chore: seed fixture" ) || return 2
  for c in "$@"; do
    ( cd "$dir" && git tag "$c-v0.1.0" ) || return 2
  done
}

# kind src: append a comment to src/lib.rs. kind toml: append a comment to Cargo.toml (the P2
# shape: a comment-only manifest edit, spec section 1).
rpf::_commit() { # dir crate kind subject [body]
  local dir="$1" crate="$2" kind="$3" subject="$4" body="${5-}"
  case "$kind" in
    src) printf '// change for: %s\n' "$subject" >>"$dir/crates/$crate/src/lib.rs" || return 2 ;;
    toml) printf '# comment-only edit for: %s\n' "$subject" >>"$dir/crates/$crate/Cargo.toml" || return 2 ;;
    *) echo "FATAL: rpf::_commit: bad kind $kind" >&2; return 2 ;;
  esac
  ( cd "$dir" && git add -A ) || return 2
  if [ -n "$body" ]; then
    ( cd "$dir" && git commit -qm "$subject" -m "$body" ) || return 2
  else
    ( cd "$dir" && git commit -qm "$subject" ) || return 2
  fi
}

rpf::_version() { # Cargo.toml -> the first `version =` value
  awk -F'"' '/^version[[:space:]]*=/ && v == "" { v = $2 } END { print v }' "$1"
}

# The first RELEASE section: from the first `## [<digit>` heading up to the next `## [`.
# `## [Unreleased]` is skipped. release-plz's last_changes() reads the same section for the
# release PR body (next_ver.rs:432-437, READ). A missing file gives an empty section.
rpf::_first_release_section() { # changelog out_file
  if [ ! -f "$1" ]; then
    : >"$2"
    return 0
  fi
  awk '/^## \[/ { if (started) done = 1; else if ($0 ~ /^## \[[0-9]/) started = 1 }
       started && !done { print }' "$1" >"$2"
}

rpf::_heading_of() { # section_file line -> the last `### ` heading above the first exact match
  awk -v want="$2" '/^### / { h = $0 } $0 == want && !seen { seen = 1; found = h } END { print found }' "$1"
}

rpf::_check_versions() { # id dir want crate...
  local id="$1" dir="$2" want="$3" c got bad="" all=""
  shift 3
  for c in "$@"; do
    got="$(rpf::_version "$dir/crates/$c/Cargo.toml")"
    all="$all $c=$got"
    if [ "$got" != "$want" ]; then bad=1; fi
  done
  if [ -z "$bad" ]; then
    printf 'PASS  %-14s%s\n' "$id" "$all"
    return 0
  fi
  printf 'FAIL  %-14s exp=%s got:%s\n' "$id" "$want" "$all" >&2
  return 1
}

# HEADING `-` means: the line only has to be in the section.
rpf::_check_section() { # id dir crate version heading line...
  local id="$1" dir="$2" crate="$3" ver="$4" heading="$5" sec first line n under
  shift 5
  sec="$dir/.rpf-section-$crate"
  rpf::_first_release_section "$dir/crates/$crate/CHANGELOG.md" "$sec" || return 1
  first="$(sed -n 1p "$sec")"
  case "$first" in
    "## [$ver]"*) ;;
    *) printf 'FAIL  %-14s %s: first release heading is %s, expected ## [%s]\n' \
         "$id" "$crate" "${first:-<none>}" "$ver" >&2
       return 1 ;;
  esac
  for line in "$@"; do
    n="$(grep -cxF -- "$line" "$sec" || true)"
    if [ "${n:-0}" = 0 ]; then
      printf 'FAIL  %-14s %s: "%s" is not in ## [%s]\n' "$id" "$crate" "$line" "$ver" >&2
      return 1
    fi
    if [ "$heading" != "-" ]; then
      under="$(rpf::_heading_of "$sec" "$line")"
      if [ "$under" != "$heading" ]; then
        printf 'FAIL  %-14s %s: "%s" is under %s, expected %s\n' \
          "$id" "$crate" "$line" "${under:-<no heading>}" "$heading" >&2
        return 1
      fi
    fi
  done
  printf 'PASS  %-14s %s: ## [%s] has %s line(s)\n' "$id" "$crate" "$ver" "$#"
}

# --- the filter fixture (spec section 4.2 table, one crate per row) -------------------------

# Rows r01-r23 are the spec's classification table. r24-r26 are the plan's Review Focus rows.
RPF_ROWS="r01 r02 r03 r04 r05 r06 r07 r08 r09 r10 r11 r12 r13 r14 r15 r16 r17 r18 r19 r20 r21 r22 r23 r24 r25 r26"

rpf::_row_expected() { # id -> version from the 0.1.0 baseline
  case "$1" in
    r01|r02|r04|r05|r06|r20|r23|r26) echo 0.1.1 ;;
    r03|r18|r19|r21|r25) echo 0.2.0 ;;
    r07|r08|r09|r10|r11|r12|r13|r14|r15|r16|r17|r22|r24) echo 0.1.0 ;;
    *) echo "FATAL: no expected version for row $1" >&2; return 2 ;;
  esac
}

rpf::_apply_row() { # dir id
  local d="$1" c="rpf-$2"
  case "$2" in
    r01) rpf::_commit "$d" "$c" src 'fix: x' ;;
    r02) rpf::_commit "$d" "$c" src 'fix(rs): x' ;;
    r03) rpf::_commit "$d" "$c" src 'feat(contracts): x' ;;
    r04) rpf::_commit "$d" "$c" src 'perf(rs): x' ;;
    r05) rpf::_commit "$d" "$c" src 'fix(deps): x' ;;
    r06) rpf::_commit "$d" "$c" src 'fix(py,ts): x' ;;
    r07) rpf::_commit "$d" "$c" src 'fix(ci): x' ;;
    r08) rpf::_commit "$d" "$c" src 'feat(ci): x' ;;
    r09) rpf::_commit "$d" "$c" src 'perf(ci): x' ;;
    r10) rpf::_commit "$d" "$c" src 'fix(repo): x' ;;
    r11) rpf::_commit "$d" "$c" src 'fix(workspace): x' ;;
    r12) rpf::_commit "$d" "$c" src 'fix(rs,ci): x' ;;
    r13) rpf::_commit "$d" "$c" src 'fix(cid): x' ;;
    r14) rpf::_commit "$d" "$c" src 'chore(rs): x' ;;
    r15) rpf::_commit "$d" "$c" src 'build(deps): x' ;;
    r16) rpf::_commit "$d" "$c" src 'refactor(rs): x' ;;
    r17) rpf::_commit "$d" "$c" src 'Revert "feat(rs): x (#1)"' ;;
    r18) rpf::_commit "$d" "$c" src 'fix(ci)!: x' ;;
    r19) rpf::_commit "$d" "$c" src 'chore(rs): x' 'BREAKING CHANGE: y' ;;
    r20) rpf::_commit "$d" "$c" src 'refactor(rs): x' && rpf::_commit "$d" "$c" src 'fix(rs): y' ;;
    r21) rpf::_commit "$d" "$c" src 'feat(ci): x' && rpf::_commit "$d" "$c" src 'fix(rs): y' ;;
    r22) rpf::_commit "$d" "$c" toml 'fix(ci): x' ;;
    r23) rpf::_commit "$d" "$c" toml 'fix(rs): x' ;;
    r24) rpf::_commit "$d" "$c" src 'fix(ci): x' 'fix(rs): y' ;;
    r25) rpf::_commit "$d" "$c" src 'chore(rs): x' 'BREAKING-CHANGE: y' ;;
    r26) rpf::_commit "$d" "$c" src 'fix(rs, py): x' ;;
    *) echo "FATAL: no commits for row $2" >&2; return 2 ;;
  esac
}

rpf::_build_filter_fixture() { # dir real_toml with_release_commits
  local dir="$1" real="$2" with_rc="$3" id crates="rpf-b"
  rpf::_write_workspace "$dir" || return 2
  rpf::_write_crate "$dir" rpf-b || return 2
  for id in $RPF_ROWS; do
    rpf::_write_crate "$dir" "rpf-$id" || return 2
    crates="$crates rpf-$id"
  done
  rpf::_write_config "$real" "$dir/release-plz.toml" "$with_rc" || return 2
  rpf::_git_init "$dir" || return 2
  # shellcheck disable=SC2086  # the crate list splits into one argument per crate
  rpf::_seed_and_tag "$dir" $crates || return 2
}

rpf::filter_suite() { # real_toml mode(real|no-release-commits) -> 0/1/2
  local real="$1" mode="$2" with_rc dir id want fails=0
  case "$mode" in
    real) with_rc=1 ;;
    no-release-commits) with_rc=0 ;;
    *) echo "FATAL: rpf::filter_suite: bad mode $mode" >&2; return 2 ;;
  esac
  dir="$(mktemp -d)" || return 2
  if ! rpf::_build_filter_fixture "$dir" "$real" "$with_rc"; then
    echo "FATAL: filter fixture build failed" >&2; rm -rf "$dir"; return 2
  fi
  for id in $RPF_ROWS; do
    if ! rpf::_apply_row "$dir" "$id"; then
      echo "FATAL [$id]: commit failed" >&2; rm -rf "$dir"; return 2
    fi
  done
  if ! ecosystem::run_update "$dir"; then
    echo "FATAL: release-plz update failed on the filter fixture" >&2; rm -rf "$dir"; return 2
  fi
  for id in $RPF_ROWS; do
    if ! want="$(rpf::_row_expected "$id")"; then rm -rf "$dir"; return 2; fi
    rpf::_check_versions "$id" "$dir" "$want" "rpf-$id" || fails=$((fails + 1))
  done
  rpf::_check_versions b "$dir" 0.1.0 rpf-b || fails=$((fails + 1))
  # Row 20, C1: a non-releasing commit is carried into the next release and its section.
  rpf::_check_section r20-changelog "$dir" rpf-r20 0.1.1 - '- *(rs)* x' '- *(rs)* y' \
    || fails=$((fails + 1))
  rm -rf "$dir"
  if [ "$fails" != 0 ]; then return 1; fi
}

# --- entry points for the hooks in release-plz.sh -------------------------------------------

rpf::suites() { # real_toml -> 0/1/2
  local real="$1" rc worst=0
  rc=0; rpf::filter_suite "$real" real || rc=$?
  if [ "$rc" = 2 ]; then return 2; fi
  if [ "$rc" != 0 ]; then worst=1; fi
  return "$worst"
}
