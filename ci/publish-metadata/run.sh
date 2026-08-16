#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# repo:publish-metadata — assert every publishable crate is genuinely releasable (SMA-376).
#
#   Check 0  the publishable set EQUALS EXPECTED_PUBLISHABLE. This is the non-vacuity
#            control: the set is discovered from the very `publish` flag this gate exists
#            to protect, so an empty or shrunken set must be a hard failure, not a green
#            run over nothing. (Same trap as ci/osv/run.sh's "0 packages scanned" and
#            ci/next-env/run.sh's "typegen emitted nothing".)
#   Check 1  each publishable crate carries metadata crates.io accepts AT UPLOAD TIME.
#            `cargo publish --dry-run` only WARNS about a missing description, so this
#            explicit assertion is the half that actually guards the metadata.
#   Check 2  `cargo publish --dry-run` succeeds — the crate is publishABLE, packages, and
#            compiles standalone with no unversioned path dependency.
#   Check 2b the packaged file list ships README.md + LICENSE and not moon.yml.
#   Check 3  while any publishable crate is at 0.0.0, rs/release-plz.toml must block its
#            release. Releasing 0.0.0 permanently burns that version on crates.io.
#
# Exit codes: 0 pass | 1 assertion failed (the repo is wrong) | 2 infrastructure failed.
# A broken invocation must NEVER read as "all checks passed".
#
# ALL cargo invocations run from rs/. rust-toolchain.toml (1.95.0) and .cargo/config.toml
# are discovered by walking up from CWD, NOT from --manifest-path (see the note in
# rs/.cargo/config.toml and the E0514 incident recorded in rs/rust-toolchain.toml), and
# there is no repo-root Cargo.toml. Every other cargo gate in moon.yml does the same.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
RS_DIR="$REPO_ROOT/rs"

# The ONE maintained fact in this script. SMA-388 adds paigasus-proto here.
EXPECTED_PUBLISHABLE=("paigasus-kernel")

# What a published artifact must and must not contain.
REQUIRED_PACKAGED=("README.md" "LICENSE")
FORBIDDEN_PACKAGED=("moon.yml")

die_infra() { printf '%s\n' "$*" >&2; exit 2; }

# Checks 0, 1 and 3 — pure functions of (cargo metadata JSON, release-plz.toml).
# Takes file paths so --negative-control (Task 4) can drive the SAME code with fixtures.
# On success prints one "<name>\t<manifest-dir>" line per publishable crate on stdout.
metadata_checks() { # $1 metadata.json  $2 release-plz.toml  $3 comma-separated expected
  python3 - "$1" "$2" "$3" <<'PY'
import json, os, re, sys, tomllib

meta_path, rp_path, expected_csv = sys.argv[1], sys.argv[2], sys.argv[3]
expected = sorted(x for x in expected_csv.split(",") if x)

try:
    with open(meta_path, encoding="utf-8") as fh:
        meta = json.load(fh)
except Exception as exc:
    print(f"FATAL: cannot read cargo metadata JSON: {exc}", file=sys.stderr)
    sys.exit(2)


def is_publishable(pkg):
    # cargo metadata: null => publishable anywhere; [] => publish = false;
    # non-empty list => publishable to those named registries.
    value = pkg.get("publish")
    return value is None or (isinstance(value, list) and len(value) > 0)


pkgs = {p["name"]: p for p in meta.get("packages", []) if is_publishable(p)}
found = sorted(pkgs)

# --- Check 0: non-vacuity control -------------------------------------------------
if not found:
    print(
        "FATAL: no publishable crate found. Either cargo metadata is broken or every "
        "crate is publish = false. This gate must never pass over an empty set.",
        file=sys.stderr,
    )
    sys.exit(2)
if found != expected:
    print(
        f"Check 0 FAILED: publishable set {found} != expected {expected}.\n"
        "  Add the crate to EXPECTED_PUBLISHABLE in ci/publish-metadata/run.sh — "
        "or you have just silently disabled this gate.",
        file=sys.stderr,
    )
    sys.exit(1)

errors = []

# --- Check 1: metadata crates.io accepts at upload time ---------------------------
KEYWORD_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9_-]*$")
for name in found:
    pkg = pkgs[name]
    for field in ("description", "license", "repository", "readme"):
        if not (pkg.get(field) or "").strip():
            errors.append(f"{name}: `{field}` is missing or empty")
    description = pkg.get("description") or ""
    if len(description) > 1000:
        errors.append(
            f"{name}: `description` is {len(description)} chars (crates.io max 1000)"
        )
    keywords = pkg.get("keywords") or []
    if not keywords:
        errors.append(f"{name}: `keywords` is empty")
    if len(keywords) > 5:
        errors.append(f"{name}: {len(keywords)} keywords (crates.io max 5)")
    for keyword in keywords:
        if len(keyword) > 20:
            errors.append(
                f"{name}: keyword {keyword!r} is {len(keyword)} chars (crates.io max 20)"
            )
        if not KEYWORD_RE.match(keyword):
            errors.append(
                f"{name}: keyword {keyword!r} must start alphanumeric and contain only "
                "[A-Za-z0-9_-]"
            )
    categories = pkg.get("categories") or []
    if not categories:
        errors.append(f"{name}: `categories` is empty")
    if len(categories) > 5:
        errors.append(f"{name}: {len(categories)} categories (crates.io max 5)")

# --- Check 3: a 0.0.0 crate must be release-blocked -------------------------------
stubs = [n for n in found if pkgs[n].get("version") == "0.0.0"]
if stubs:
    try:
        with open(rp_path, "rb") as fh:
            release_plz = tomllib.load(fh)
    except Exception as exc:
        print(f"FATAL: cannot parse {rp_path}: {exc}", file=sys.stderr)
        sys.exit(2)
    blocked_workspace = release_plz.get("workspace", {}).get("release") is False
    blocked_packages = {
        entry.get("name")
        for entry in release_plz.get("package", [])
        if entry.get("release") is False
    }
    for name in stubs:
        if not blocked_workspace and name not in blocked_packages:
            errors.append(
                f"{name}: publishable at 0.0.0 but rs/release-plz.toml does not block its "
                "release. Releasing 0.0.0 permanently burns that version on crates.io — "
                "keep `[workspace] release = false` until SMA-407 moves the floor to 0.1.0."
            )

if errors:
    print(
        "publish-metadata: assertions failed\n  - " + "\n  - ".join(errors),
        file=sys.stderr,
    )
    sys.exit(1)

for name in found:
    print(f"{name}\t{os.path.dirname(pkgs[name]['manifest_path'])}")
PY
}

# Check 2b — assert a packaged file listing. Takes the listing as a FILE so
# --negative-control can feed it a synthetic one.
assert_package_list() { # $1 listing file  $2 package name
  local listing="$1" pkg="$2" entry rc=0
  for entry in "${REQUIRED_PACKAGED[@]}"; do
    if ! grep -qxF "$entry" "$listing"; then
      echo "Check 2b FAILED: $pkg does not package $entry" >&2
      rc=1
    fi
  done
  for entry in "${FORBIDDEN_PACKAGED[@]}"; do
    if grep -qxF "$entry" "$listing"; then
      echo "Check 2b FAILED: $pkg packages $entry — tighten the [package] include list" >&2
      rc=1
    fi
  done
  return "$rc"
}

# cargo has no distinct exit code for "the registry is down" vs "your crate is broken",
# so classify on stderr. Returns 2 for infrastructure, 1 for a real assertion failure.
classify_cargo_failure() { # $1 captured-output file
  if grep -qiE 'network|failed to fetch|spurious|could not connect|timed out|rate limit|failed to download' "$1"; then
    return 2
  fi
  return 1
}

check_package() { # $1 name  $2 manifest dir
  local pkg="$1" pkg_dir="$2" dirty=() out listing status

  # --allow-dirty changes WHAT GETS PACKAGED: cargo enumerates via git, so untracked
  # files are swept in and .cargo_vcs_info.json is stamped "dirty": true. Allow it only
  # so a developer can run this gate on uncommitted work — NEVER in CI, where the
  # assertion must be about a committed tree.
  if [ -z "${CI:-}" ] && [ -n "$(git -C "$REPO_ROOT" status --porcelain -- "$pkg_dir")" ]; then
    echo "publish-metadata: $pkg has uncommitted changes — adding --allow-dirty (local only)" >&2
    dirty=(--allow-dirty)
  fi

  out="$(mktemp)"
  listing="$(mktemp)"

  # Check 2b first: it is cheap and does not compile anything.
  if ! cargo package --list --locked -p "$pkg" ${dirty[@]+"${dirty[@]}"} >"$listing" 2>"$out"; then
    cat "$out" >&2
    status=0; classify_cargo_failure "$out" || status=$?
    rm -f "$out" "$listing"
    exit "$status"
  fi
  if ! assert_package_list "$listing" "$pkg"; then
    rm -f "$out" "$listing"
    exit 1
  fi

  # Check 2: --locked so the verify build resolves against the packaged lockfile rather
  # than whatever the registry serves this minute.
  if ! cargo publish --dry-run --locked -p "$pkg" ${dirty[@]+"${dirty[@]}"} >"$out" 2>&1; then
    cat "$out" >&2
    status=0; classify_cargo_failure "$out" || status=$?
    rm -f "$out" "$listing"
    exit "$status"
  fi

  rm -f "$out" "$listing"
  echo "publish-metadata: $pkg OK"
}

main() {
  cd "$RS_DIR"

  local meta_json
  meta_json="$(mktemp)"

  cargo metadata --format-version 1 --no-deps >"$meta_json" 2>/dev/null \
    || die_infra "FATAL: \`cargo metadata\` failed in $RS_DIR — nothing could be verified."

  local expected_csv
  expected_csv="$(IFS=,; printf '%s' "${EXPECTED_PUBLISHABLE[*]}")"

  # NOTE: declare and assign on SEPARATE lines. `local x="$(cmd)"` masks the command's
  # exit status, which would swallow the 1-vs-2 distinction these checks depend on.
  # NOTE: capture the status BEFORE cleanup. `|| { rm -f ...; exit $?; }` would evaluate
  # $? AFTER rm succeeds, exiting 0 and turning every metadata assertion failure into a
  # silent pass — the exact vacuous-gate failure this script exists to prevent.
  local status=0
  local publishable
  publishable="$(metadata_checks "$meta_json" "$RS_DIR/release-plz.toml" "$expected_csv")" \
    || status=$?
  rm -f "$meta_json"
  [ "$status" -eq 0 ] || exit "$status"

  local name dir
  while IFS=$'\t' read -r name dir; do
    [ -n "$name" ] || continue
    check_package "$name" "$dir"
  done <<<"$publishable"

  echo "publish-metadata: all checks passed"
}

main "$@"
