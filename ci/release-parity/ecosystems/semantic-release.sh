#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
# semantic-release (TypeScript) ecosystem module for the SMA-398 parity harness (SMA-406).
# Interface: ecosystem::build_fixture / apply_commit / run_update / version (+ ecosystem::expected).
#
# semantic-release has NO version-aware 0.x clamp, so breaking changes go to 1.0.0
# in 0.x (the documented ADR-0011 S6 exception). `ecosystem::expected` asserts that;
# the gate goes red if a semantic-release upgrade changes it, or the F3 guard fails
# loud if the real config starts clamping. Per-package isolation is an in-repo
# path-filter (NOT a third-party monorepo plugin); the next version is computed via
# the semantic-release JS API (the `next-version` runner), not by scraping the CLI.
set -euo pipefail

A_PKG="parity-release-parity-a"
B_PKG="parity-release-parity-b"
BASELINE="0.1.0"

_SR_SELF="${BASH_SOURCE[0]:-$0}"
_SR_REPO_ROOT="$(cd "$(dirname "$_SR_SELF")/../../.." && pwd)"
_SR_TS="$_SR_REPO_ROOT/ts"
_SR_PATH_FILTER="$_SR_TS/tooling/semantic-release-path-filter.mjs"
_SR_RUNNER="$_SR_TS/tooling/semantic-release-next-version.mjs"

# Real production configs the fixture derives its classification from (F3).
_SR_SDK_CFG="$_SR_TS/packages/paigasus-sdk/.releaserc.json"
_SR_UI_CFG="$_SR_TS/packages/paigasus-ui/.releaserc.json"

# Strict npm semver (no 0.x clamp): any breaking marker -> major bump from baseline.
# Documented ADR-0011 S6 exception. NOTE: `1.0.0` is the major bump of BASELINE
# (0.1.0) and is correct for ANY 0.x baseline; at the 1.0 transition (when cases.tsv's
# 1.x column is asserted) this MUST consume expected_1x / compute major-of-baseline.
ecosystem::expected() { # id subject footer expected_0x expected_1x discr
  local subject="$2" footer="$3" expected_0x="$4"
  if printf '%s' "$subject" | grep -qE '^[a-z]+(\([^)]*\))?!:' \
     || printf '%s' "$footer" | grep -q 'BREAKING CHANGE'; then
    printf '1.0.0'
  else
    printf '%s' "$expected_0x"
  fi
}

ecosystem::_slot_dir() { # dir slot(a|b) -> path
  case "$2" in a|b) printf '%s/%s' "$1" "$2" ;; *) echo "bad slot: $2" >&2; return 1 ;; esac
}
ecosystem::_slot_pkg() { case "$1" in a) printf '%s' "$A_PKG" ;; b) printf '%s' "$B_PKG" ;; *) return 1 ;; esac }

# F3: derive the classification descriptor from BOTH real configs and validate.
# Fails loudly (and the harness aborts) if either config carries `releaseRules`
# anywhere (the documented native divergence would no longer hold) or if sdk and ui
# disagree. Echoes the agreed preset (possibly empty = semantic-release default).
ecosystem::_derive_classification() {
  node -e '
    const fs = require("fs");
    function read(p) {
      let c;
      try { c = JSON.parse(fs.readFileSync(p, "utf8")); }
      catch (e) { console.error("FATAL: cannot read " + p + ": " + e.message); process.exit(1); }
      const json = JSON.stringify(c);
      if (/"releaseRules"/.test(json)) {
        console.error("FATAL: " + p + " has commit-analyzer releaseRules — the documented "
          + "breaking->1.0.0 divergence no longer holds; update the divergence table + "
          + "ecosystem::expected (and ci/release-parity/README.md).");
        process.exit(1);
      }
      let preset = c.preset || "";
      for (const pl of (c.plugins || [])) if (Array.isArray(pl) && pl[1] && pl[1].preset) preset = pl[1].preset;
      return preset;
    }
    const sdk = read(process.argv[1]);
    const ui = read(process.argv[2]);
    if (sdk !== ui) {
      console.error("FATAL: @paigasus/sdk and @paigasus/ui semantic-release classification differs "
        + "(preset: \"" + sdk + "\" vs \"" + ui + "\") — both must honor the same contract.");
      process.exit(1);
    }
    process.stdout.write(sdk);
  ' "$_SR_SDK_CFG" "$_SR_UI_CFG"
}

ecosystem::build_fixture() { # dir real_release_plz_toml(ignored)
  local dir="$1" preset slot sdir pkg tagfmt presetline
  preset="$(ecosystem::_derive_classification)" || return 1   # F3 + guards (loud-fail aborts)
  [ -f "$_SR_PATH_FILTER" ] || { echo "FATAL: path-filter missing: $_SR_PATH_FILTER" >&2; return 1; }
  [ -f "$_SR_RUNNER" ] || { echo "FATAL: runner missing: $_SR_RUNNER" >&2; return 1; }

  ( cd "$dir" && git -c init.defaultBranch=main init -q \
    && git config user.email "parity@example.com" && git config user.name "parity" \
    && git config commit.gpgsign false && git config tag.gpgsign false )

  # Keep the local bare origin (created below) and the run_update sentinels out of
  # the working tree's commits — else apply_commit's `git add -A` would pollute the
  # slot commit and break path attribution.
  printf '/origin.git/\n.parity-next-version\n.sr-stderr\n' >"$dir/.gitignore"

  for slot in a b; do
    sdir="$(ecosystem::_slot_dir "$dir" "$slot")"
    pkg="$(ecosystem::_slot_pkg "$slot")"
    tagfmt="${slot}-v\${version}"
    mkdir -p "$sdir/src"
    cat >"$sdir/package.json" <<EOF
{ "name": "$pkg", "version": "$BASELINE", "private": true, "type": "module" }
EOF
    # Fixture config: the in-repo path-filter by ABSOLUTE path (resolves from /tmp),
    # plus the F3-derived preset if any. Same single-plugin shape as the real config.
    if [ -n "$preset" ]; then
      presetline="[\"$_SR_PATH_FILTER\", { \"preset\": \"$preset\" }]"
    else
      presetline="\"$_SR_PATH_FILTER\""
    fi
    cat >"$sdir/.releaserc.json" <<EOF
{ "branches": ["main"], "tagFormat": "$tagfmt", "plugins": [ $presetline ] }
EOF
    echo "// seed $slot" >"$sdir/src/index.mjs"
  done

  # Seed commit + baseline tags, then a real LOCAL BARE origin (semantic-release runs
  # `git ls-remote --heads origin` and a no-refspec `git fetch --tags origin`, which
  # resolves the remote HEAD; a placeholder URL fails). Pin the bare repo's default
  # branch to main so its HEAD matches the pushed branch — otherwise on a host whose
  # init.defaultBranch is `master` (e.g. CI runners) the remote HEAD points at a branch
  # that was never pushed and `git fetch` dies with "couldn't find remote ref HEAD".
  # Fully offline.
  ( cd "$dir" \
    && git add -A && git commit -qm "chore: seed fixture" \
    && git tag "a-v$BASELINE" && git tag "b-v$BASELINE" \
    && git -c init.defaultBranch=main init --bare "$dir/origin.git" -q \
    && git remote add origin "$dir/origin.git" \
    && git push -q origin main --tags )
}

ecosystem::apply_commit() { # dir slot subject footer
  local dir="$1" slot="$2" subject="$3" footer="$4" sdir
  sdir="$(ecosystem::_slot_dir "$dir" "$slot")"
  printf '// change for: %s\n' "$subject" >>"$sdir/src/index.mjs"
  (
    cd "$dir"
    git add -A
    if [ "$footer" = "-" ]; then git commit -qm "$subject"; else git commit -qm "$subject" -m "$footer"; fi
  )
}

ecosystem::run_update() { # dir
  # Compute each slot's next version read-only via the semantic-release JS API runner.
  # The runner prints the version (or empty = no release) to stdout; logs to stderr.
  # The in-repo path-filter (wired into each slot config) restricts analysis to commits
  # under that slot dir, so slot `b` (no commit under b/) gets no release -> baseline.
  local dir="$1" slot sdir out
  for slot in a b; do
    sdir="$(ecosystem::_slot_dir "$dir" "$slot")"
    if ! out="$(node "$_SR_RUNNER" "$sdir" 2>"$sdir/.sr-stderr")"; then
      echo "FATAL: semantic-release runner failed for slot $slot" >&2
      cat "$sdir/.sr-stderr" >&2 || true
      return 1
    fi
    out="$(printf '%s' "$out" | tr -d '[:space:]')"
    [ -n "$out" ] || out="$BASELINE"   # JS API returned no release -> unchanged baseline
    printf '%s\n' "$out" >"$sdir/.parity-next-version"
  done
}

ecosystem::version() { # dir slot -> version string
  local sdir v
  sdir="$(ecosystem::_slot_dir "$1" "$2")"
  IFS= read -r v <"$sdir/.parity-next-version" 2>/dev/null || true
  printf '%s' "$v"
}
