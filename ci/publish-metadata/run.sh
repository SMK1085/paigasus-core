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
#   Check 1b each category is a REAL crates.io slug, validated against the committed
#            snapshot ci/publish-metadata/crates-io-categories.txt. crates.io DROPS unknown
#            slugs and publishes anyway, and `cargo publish --dry-run` returns before the
#            warning that would have said so — so Check 2 cannot catch this (SMA-529).
#   Check 2  `cargo publish --dry-run` succeeds — the crate is publishABLE, packages, and
#            compiles standalone with no unversioned path dependency.
#   Check 2b the packaged file list ships README.md + LICENSE and not moon.yml.
#   Check 3  while any publishable crate is at 0.0.0, rs/release-plz.toml must block its
#            release. Releasing 0.0.0 permanently burns that version on crates.io.
#   Check 4  .github/workflows/security-scan.yml still INVOKES the freshness check on a real,
#            non-comment run: line whose exit status is not discarded, and does not suppress
#            it with continue-on-error or if:. Nothing else guards a workflow job:
#            repo:actionlint's call-site machinery is keyed on ci.yml only (SMA-529).
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

SNAPSHOT="$REPO_ROOT/ci/publish-metadata/crates-io-categories.txt"

# The heredocs below `cd` into rs/, so an import cannot rely on CWD. Exported once rather
# than per-call so --negative-control's direct function calls see it too.
export PYTHONPATH="$REPO_ROOT/ci/publish-metadata${PYTHONPATH:+:$PYTHONPATH}"

# The ONE maintained fact in this script. SMA-577 added paigasus-proto AND
# paigasus-proto-derive here — both, because the derive crate must publish first.
EXPECTED_PUBLISHABLE=("paigasus-kernel" "paigasus-proto" "paigasus-proto-derive")

# What a published artifact must and must not contain.
REQUIRED_PACKAGED=("README.md" "LICENSE")
FORBIDDEN_PACKAGED=("moon.yml")

die_infra() { printf '%s\n' "$*" >&2; exit 2; }

# Checks 0, 1 and 3 — pure functions of (cargo metadata JSON, release-plz.toml).
# Takes file paths so --negative-control (Task 4) can drive the SAME code with fixtures.
# On success prints one "<name>\t<manifest-dir>" line per publishable crate on stdout.
metadata_checks() { # $1 metadata.json  $2 release-plz.toml  $3 expected-csv  $4 snapshot
  python3 - "$1" "$2" "$3" "${4:-}" <<'PY'
import json, os, re, sys, tomllib

# A MISSING 4th argument is a broken invocation, not a reason to skip Check 1b. Exit 2:
# silently skipping would make every fixture below pass while asserting nothing, and an
# IndexError here exits 1, which a non-zero-only harness reports as a successful red.
if len(sys.argv) < 5 or not sys.argv[4].strip():
    print("FATAL: the snapshot path was not passed to metadata_checks", file=sys.stderr)
    sys.exit(2)

meta_path, rp_path, expected_csv, snapshot_path = (
    sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
)
expected = sorted(x for x in expected_csv.split(",") if x)

# The local variable `categories` in the Check 1 loop below shadows the module name, so
# import it under a distinct name.
import categories as categories_module

try:
    known_slugs = categories_module.load_snapshot(snapshot_path, categories_module._today_utc())
except categories_module.SnapshotError as exc:
    print(f"category snapshot: {exc}", file=sys.stderr)
    sys.exit(1)

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
    for category in categories:
        if not isinstance(category, str):
            errors.append(f"{name}: category {category!r} is not a string")
            continue
        if category != category.strip():
            errors.append(
                f"{name}: category {category!r} has surrounding whitespace"
            )
            continue
        if category in known_slugs:
            continue
        hint = categories_module.nearest(category, known_slugs)
        suggestion = f" Did you mean {hint!r}?" if hint else ""
        errors.append(
            f"{name}: category {category!r} is not a crates.io category slug."
            f"{suggestion} crates.io DROPS unknown slugs — the publish succeeds and the "
            "crate appears uncategorized, and `cargo publish --dry-run` returns before "
            "the warning that would have told you. crates.io matches slugs EXACTLY and "
            "case-sensitively at publish time (its read API does not, which is why a "
            "browser check misleads). Valid slugs: "
            "ci/publish-metadata/crates-io-categories.txt"
        )

# --- Check 3: a 0.0.0 crate must be release-blocked -------------------------------
stubs = [n for n in found if pkgs[n].get("version") == "0.0.0"]
if stubs:
    try:
        with open(rp_path, "rb") as fh:
            release_plz = tomllib.load(fh)
    except FileNotFoundError:
        # A missing config removes the release block entirely — that IS the repo defect
        # Check 3 exists to catch, not an infrastructure problem. Route it into errors so
        # it exits 1, same as any other unblocked-0.0.0 finding.
        for name in stubs:
            errors.append(
                f"{name}: publishable at 0.0.0 but {rp_path} does not exist, so its "
                "release cannot be blocked. Releasing 0.0.0 permanently burns that "
                "version on crates.io."
            )
        release_plz = None
    except Exception as exc:
        print(f"FATAL: cannot parse {rp_path}: {exc}", file=sys.stderr)
        sys.exit(2)
    if release_plz is not None:
        workspace_release = release_plz.get("workspace", {}).get("release")
        package_release = {
            entry["name"]: entry.get("release")
            for entry in release_plz.get("package", [])
            if "name" in entry
        }
        for name in stubs:
            # A [[package]] entry OVERRIDES [workspace] for that package — but release-plz
            # treats an OMITTED `release` key inside `[[package]]` as "inherit the workspace
            # value", not as an explicit unset-is-releasable override. dict.get()'s default
            # only fires when the KEY is absent, so a `[[package]]` block that names the
            # package but never sets `release =` (key present, value None) must still fall
            # through to workspace_release, same as if there were no `[[package]]` entry at
            # all. Anything other than an explicit False — including release = true and a
            # workspace-inherited None — leaves the crate releasable.
            effective = (
                package_release[name]
                if package_release.get(name) is not None
                else workspace_release
            )
            if effective is not False:
                errors.append(
                    f"{name}: publishable at 0.0.0 but rs/release-plz.toml does not block "
                    "its release. Releasing 0.0.0 permanently burns that version on "
                    "crates.io — keep `[workspace] release = false` (and no `[[package]] "
                    "release = true` override) until SMA-407 moves the floor to 0.1.0."
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

  # Non-vacuity control, same reason Check 0 has one: an empty rule set would make this
  # function return 0 on ANY listing, including one that both misses LICENSE and leaks
  # moon.yml. Infrastructure failure, not an assertion failure.
  if [ "${#REQUIRED_PACKAGED[@]}" -eq 0 ] || [ "${#FORBIDDEN_PACKAGED[@]}" -eq 0 ]; then
    echo "FATAL: Check 2b rule lists are empty — this check would pass vacuously" >&2
    return 2
  fi

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

# Check 1c — a publishable crate must carry its OWN lint table, and that table must not
# deny. Cargo INLINES the resolved lint table into the published manifest, and docs.rs
# builds a published crate as the ROOT package on nightly, where `--cap-lints allow` does
# not apply — so an inherited (or hand-written) `warnings = "deny"` lets the first new
# rustc warning silently kill docs.rs builds of an already-released crate, months later.
# Neither `[lints]` nor `include` appears in `cargo metadata`, so this reads the manifest.
# Takes a PATH so --negative-control drives the same code with fixtures.
assert_lint_table() { # $1 manifest path
  python3 - "$1" <<'PY'
import sys, tomllib

path = sys.argv[1]
try:
    with open(path, "rb") as fh:
        manifest = tomllib.load(fh)
except Exception as exc:
    # Unreadable or malformed TOML is INFRASTRUCTURE, not a repo defect: nothing was
    # asserted, so it must not read as "the crate is fine" nor as "the crate is wrong".
    print(f"FATAL: cannot parse {path}: {exc}", file=sys.stderr)
    sys.exit(2)

lints = manifest.get("lints")
name = manifest.get("package", {}).get("name", path)
errors = []

if not isinstance(lints, dict) or not lints:
    errors.append(
        f"{name}: no `[lints.*]` table. A publishable crate must declare its own — see the "
        "rationale on paigasus-kernel/Cargo.toml. The rule is discipline, not only "
        "hazard-avoidance: without it a crate can drift into workspace inheritance by deletion."
    )
elif lints.get("workspace") is True:
    errors.append(
        f"{name}: inherits workspace lints (`[lints] workspace = true`). Cargo inlines the "
        "RESOLVED table into the published manifest, and docs.rs builds published crates as "
        "the root package on nightly where `--cap-lints allow` does not apply, so the "
        "workspace's `warnings = \"deny\"` would let the first new rustc warning silently "
        "kill docs.rs builds. Declare a per-crate table with `warnings = \"warn\"`."
    )
else:
    # Both TOML spellings: the string form `warnings = "deny"` and the table form
    # `warnings = { level = "deny", priority = -1 }`. Checking only the string form would
    # let the table form through while the crate carries the hazard in full.
    def level_of(value):
        if isinstance(value, str):
            return value
        if isinstance(value, dict):
            return value.get("level")
        return None

    for table, key in (("rust", "warnings"), ("clippy", "all")):
        level = level_of((lints.get(table) or {}).get(key))
        if level in ("deny", "forbid"):
            errors.append(
                f"{name}: `[lints.{table}] {key}` is {level!r}. Its own table is not enough — "
                f"a published crate must not deny, or docs.rs breaks on the first new lint. "
                f'Use "warn"; CI strictness comes from the Moon lint task\'s explicit -D warnings.'
            )

if errors:
    print("Check 1c FAILED\n  - " + "\n  - ".join(errors), file=sys.stderr)
    sys.exit(1)
PY
}

# Check 1d — a publishable crate must carry an `include` ALLOWLIST covering README.md and
# LICENSE. Cargo's default include is "every non-ignored file in the package dir", which
# sweeps moon.yml into the tarball and would ship whatever the dir gains next. Check 2b
# catches the OUTCOME (a leaked moon.yml) but only for files someone added to
# FORBIDDEN_PACKAGED, and only after a listing exists; 1d asserts the RULE.
assert_include_allowlist() { # $1 manifest path
  python3 - "$1" <<'PY'
import sys, tomllib

REQUIRED = ("README.md", "LICENSE")
path = sys.argv[1]
try:
    with open(path, "rb") as fh:
        manifest = tomllib.load(fh)
except Exception as exc:
    print(f"FATAL: cannot parse {path}: {exc}", file=sys.stderr)
    sys.exit(2)

package = manifest.get("package", {})
name = package.get("name", path)
include = package.get("include")
errors = []

if include is None:
    errors.append(
        f"{name}: no `[package] include`. Cargo's default packages EVERY non-ignored file "
        "in the crate dir — moon.yml today, and whatever the dir gains next. Enumerate what "
        "belongs; an allowlist is the version that cannot leak."
    )
elif isinstance(include, dict):
    # `include` is workspace-inheritable, so `include.workspace = true` parses as
    # {"workspace": True} — non-empty, truthy, and NOT a list. A naive "declares a
    # non-empty include" test passes it vacuously, which is why this arm is explicit.
    errors.append(
        f"{name}: `include` is inherited (`include.workspace = true`). A publishable crate "
        "must carry its OWN allowlist — the packaged file set is per-crate, and an inherited "
        "one cannot be right for every member."
    )
elif not isinstance(include, list):
    errors.append(f"{name}: `include` is {type(include).__name__}, expected a list of strings")
elif not include:
    errors.append(f"{name}: `include` is empty — it would package nothing")
else:
    for entry in include:
        if not isinstance(entry, str):
            errors.append(f"{name}: include entry {entry!r} is not a string")
    missing = [r for r in REQUIRED if r not in include]
    if missing:
        errors.append(
            f"{name}: `include` does not list {', '.join(missing)}. Membership is LITERAL — "
            "a wildcard such as \"**/*\" is deliberately NOT accepted, because it would "
            "'cover' these files while reinstating exactly the moon.yml leak Check 2b exists "
            "to catch. Add the exact strings."
        )

if errors:
    print("Check 1d FAILED\n  - " + "\n  - ".join(errors), file=sys.stderr)
    sys.exit(1)
PY
}

# Guard-the-guard (SMA-542): a new check's own CALL SITE is what goes unguarded. The
# fixture rows below exercise this function; only this assertion covers its INVOCATION in
# the workflow, and repo:actionlint's equivalent machinery is keyed on ci.yml alone.
# Takes the workflow path as a FILE so --negative-control drives the same code.
assert_freshness_call_site() { # $1 workflow file
  local wf="$1" rc=0

  if [ ! -f "$wf" ] || [ ! -r "$wf" ]; then
    echo "FATAL: cannot read $wf — the call-site pin cannot assert anything" >&2
    return 2
  fi

  # Anchored to a real, non-comment `run:` key AND to the actual command, not merely the
  # flag text — a `run:` line containing the `--check-categories-freshness` substring is
  # NOT enough: `run: echo --check-categories-freshness` matches the flag while invoking
  # nothing (measured bypass). The value must invoke ci/publish-metadata/run.sh itself.
  # `[[:space:]]` and `(- )?` are ERE and portable across BSD and GNU grep; the literal
  # dot in the path is escaped so it does not match "any character".
  local hit_line hit_lineno
  hit_line="$(grep -nE '^[[:space:]]*(- )?run:[^#]*ci/publish-metadata/run\.sh[^#]*--check-categories-freshness' "$wf" | head -n1 || true)"
  if [ -z "$hit_line" ]; then
    echo "Check 4 FAILED: $wf no longer invokes --check-categories-freshness on a real," >&2
    echo "  non-comment run: line. The category snapshot's ONLY drift detector would be" >&2
    echo "  silently disabled." >&2
    rc=1
  else
    hit_lineno="${hit_line%%:*}"
    # Mirror ci/actionlint/run.sh check 8's discard-tail reasoning: a wrapper's exit status
    # can be swallowed by a trailing ||/&&/;/| on the SAME line even though the invocation
    # itself is present and uncommented.
    case "$hit_line" in
      *'||'*|*'&&'*|*';'*|*'|'*)
        echo "Check 4 FAILED: $wf line $hit_lineno discards the freshness invocation's" >&2
        echo "  exit status with a ||/&&/;/| tail. A red freshness check would not fail" >&2
        echo "  the job." >&2
        printf '  %s\n' "$hit_line" >&2
        rc=1
        ;;
    esac
  fi

  # Any continue-on-error other than the literal `false` can suppress that job's red. The
  # file carries none today, so a whole-file rule is both simple and strict; a legitimate
  # future use needs an explicit exemption added here with a reason.
  local offending
  offending="$(grep -n 'continue-on-error:' "$wf" | grep -v 'continue-on-error: *false' || true)"
  if [ -n "$offending" ]; then
    echo "Check 4 FAILED: $wf suppresses a failure with continue-on-error:" >&2
    printf '  %s\n' "$offending" >&2
    rc=1
  fi

  # Any `if:` key can suppress the JOB (or step) that runs the freshness check without
  # touching continue-on-error at all — measured: `if: false` on category-slugs: leaves this
  # gate at rc 0 with no other change. The file carries no `if:` today, so a whole-file rule
  # is both simple and strict. A legitimate future `if:` needs a deliberate exemption added
  # HERE with a reason — the point is that changing conditional execution in the one file
  # holding the only drift detector must be loud, never silent.
  local if_offending
  if_offending="$(grep -n 'if:' "$wf" || true)"
  if [ -n "$if_offending" ]; then
    echo "Check 4 FAILED: $wf gained an if: key, which can suppress the freshness job or" >&2
    echo "  step without touching continue-on-error. No if: is expected in this file; add" >&2
    echo "  a deliberate, reasoned exemption in assert_freshness_call_site if one is ever" >&2
    echo "  legitimately needed." >&2
    printf '  %s\n' "$if_offending" >&2
    rc=1
  fi

  return "$rc"
}

# cargo has no distinct exit code for "the registry is down" vs "your crate is broken",
# so classify on stderr. Returns 2 for infrastructure, 1 for a real assertion failure.
classify_cargo_failure() { # $1 captured-output file
  # A real compile/packaging failure always wins. rustc diagnostics quote source lines,
  # which can contain words like "network" — matching those flipped genuine defects into
  # the retryable bucket.
  if grep -qE '^error\[E[0-9]+\]|could not compile|failed to verify package tarball' "$1"; then
    return 1
  fi
  # Transient conditions only. A permanent 4xx (a dependency that does not exist on the
  # registry) is a REAL publishability failure, so it must NOT land here.
  if grep -qiE 'spurious network error|could not connect|connection timed out|network failure|rate limit|HTTP status 50[234]' "$1"; then
    return 2
  fi
  return 1
}

# A PUBLISH GROUP is a connected component of the in-set dependency graph: nodes are the
# publishable crates, and an edge joins A-B when A depends on B and both are publishable.
# Derived, not declared, so it needs no coupling to release-plz.toml's version_group and
# cannot go stale when a dependency is added or removed. Today: {paigasus-kernel} and
# {paigasus-proto-derive, paigasus-proto}.
publish_groups() { # $1 metadata.json  $2 expected-csv -> one TAB-separated group per line
  python3 - "$1" "$2" <<'PY'
import json, sys

try:
    with open(sys.argv[1], encoding="utf-8") as fh:
        meta = json.load(fh)
except Exception as exc:
    print(f"FATAL: cannot read cargo metadata JSON: {exc}", file=sys.stderr)
    sys.exit(2)

expected = {x for x in sys.argv[2].split(",") if x}
pkgs = {p["name"]: p for p in meta.get("packages", []) if p["name"] in expected}
if not pkgs:
    print("FATAL: no publishable package matched the expected set", file=sys.stderr)
    sys.exit(2)

adjacency = {name: set() for name in pkgs}
for name, pkg in pkgs.items():
    for dep in pkg.get("dependencies", []):
        other = dep.get("name")
        if other in pkgs and other != name:
            adjacency[name].add(other)
            adjacency[other].add(name)

seen, groups = set(), []
for name in sorted(pkgs):
    if name in seen:
        continue
    component, stack = set(), [name]
    while stack:
        current = stack.pop()
        if current in component:
            continue
        component.add(current)
        seen.add(current)
        stack.extend(adjacency[current] - component)
    groups.append(sorted(component))

for group in sorted(groups):
    print("\t".join(group))
PY
}

# Populated by main()'s enumeration loop: name -> manifest dir, the same fact
# metadata_checks's "<name>\t<manifest-dir>" stdout already carries. check_publish_group
# consults this map by name rather than re-deriving a package's dir via a fresh `cargo
# metadata` shell-out per package inside the dirty check — that would spawn a subprocess
# for a fact main() already holds, and create a SECOND SOURCE OF TRUTH for the input that
# decides --allow-dirty, a flag that CHANGES WHAT GETS PACKAGED. A divergence between the
# two would silently mis-decide it.
declare -A PKG_DIR=()

# Every package name Check 2 was ACTUALLY invoked with, appended BY THE HELPER. main()
# compares this against the set the Check 2b loop enumerated, so deleting an invocation
# leaves this short and the assertion fires — a one-line deletion is caught. What remains
# open is deleting the invocation AND the assertion together (a two-site edit).
CHECK2_INVOKED=()

# Check 2b — the packaged file list. Per package: `cargo package --list` performs no build,
# so it has no chicken-and-egg (verified: it succeeds for paigasus-proto with the derive
# crate absent from crates.io).
check_package_list() { # $1 name  $2 manifest dir
  local pkg="$1" pkg_dir="$2" dirty=() out listing status

  if [ -z "${CI:-}" ] && [ -n "$(git -C "$REPO_ROOT" status --porcelain -- "$pkg_dir")" ]; then
    echo "publish-metadata: $pkg has uncommitted changes — adding --allow-dirty (local only)" >&2
    dirty=(--allow-dirty)
  fi

  out="$(mktemp)"
  listing="$(mktemp)"
  if ! cargo package --list --locked -p "$pkg" ${dirty[@]+"${dirty[@]}"} >"$listing" 2>"$out"; then
    cat "$out" >&2
    status=0; classify_cargo_failure "$out" || status=$?
    rm -f "$out" "$listing"
    return "$status"
  fi
  status=0
  assert_package_list "$listing" "$pkg" || status=$?
  rm -f "$out" "$listing"
  return "$status"
}

# Check 2 — one `cargo publish --dry-run` per publish group. --locked so the verify build
# resolves against the packaged lockfile rather than whatever the registry serves this
# minute. Manifest dirs come from PKG_DIR (populated by main()'s enumeration loop), not
# from a fresh `cargo metadata` call — see PKG_DIR's own comment for why.
check_publish_group() { # $@ package names in ONE group
  local pkgs=("$@") flags=() dirty_pkgs=() out status pkg pkg_dir

  # Non-vacuity: an empty group would make this return 0 having asserted nothing.
  # Defence-in-depth — Check 0 already exits 2 on an empty publishable set and pins the set
  # by strict equality, so this is unreachable from main() today.
  if [ "${#pkgs[@]}" -eq 0 ]; then
    echo "FATAL: Check 2 invoked with an empty package list — it would assert nothing" >&2
    return 2
  fi

  CHECK2_INVOKED+=("${pkgs[@]}")

  # --allow-dirty CHANGES WHAT GETS PACKAGED (untracked files are swept in), and one flag
  # covers the whole invocation — so it must be the UNION over the group, not a per-package
  # decision, or a dirty paigasus-proto would silently package paigasus-proto-derive dirty
  # too. Local only: CI sets CI, which skips the dirty check entirely.
  if [ -z "${CI:-}" ]; then
    for pkg in "${pkgs[@]}"; do
      pkg_dir="${PKG_DIR[$pkg]:-}"
      if [ -z "$pkg_dir" ]; then
        echo "FATAL: Check 2 has no manifest dir for $pkg — PKG_DIR was not populated for it. This is infrastructure, not a repo defect: nothing about $pkg was asserted." >&2
        return 2
      fi
      if [ -n "$(git -C "$REPO_ROOT" status --porcelain -- "$pkg_dir")" ]; then
        dirty_pkgs+=("$pkg")
      fi
    done
    if [ "${#dirty_pkgs[@]}" -gt 0 ]; then
      echo "publish-metadata: --allow-dirty for group [${pkgs[*]}] — forced by: ${dirty_pkgs[*]} (local only)" >&2
      flags=(--allow-dirty)
    fi
  fi

  for pkg in "${pkgs[@]}"; do
    flags+=(-p "$pkg")
  done

  out="$(mktemp)"
  if ! cargo publish --dry-run --locked "${flags[@]}" >"$out" 2>&1; then
    cat "$out" >&2
    status=0; classify_cargo_failure "$out" || status=$?
    rm -f "$out"
    return "$status"
  fi
  rm -f "$out"
  echo "publish-metadata: group [${pkgs[*]}] OK"
}

# Guard-the-guard (SMA-542): a new check's own CALL SITE is what goes unguarded. The
# fixture rows exercise check_publish_group; only this covers its INVOCATION. Because
# CHECK2_INVOKED is written BY THE HELPER, deleting an invocation leaves it short and this
# fires. Exit 2, not 1: a Check 2 that silently ran over fewer crates than were enumerated
# is a broken gate, not a wrong repo.
assert_check2_covered_everything() { # $@ the names the per-package loop enumerated
  local enumerated invoked
  enumerated="$(printf '%s\n' "$@" | LC_ALL=C sort -u)"
  invoked="$(printf '%s\n' ${CHECK2_INVOKED[@]+"${CHECK2_INVOKED[@]}"} | LC_ALL=C sort -u)"
  if [ "$enumerated" != "$invoked" ]; then
    echo "FATAL: Check 2 ran over a different set than Check 2b enumerated." >&2
    echo "  enumerated: $(echo $enumerated)" >&2
    echo "  Check 2 ran: $(echo $invoked)" >&2
    echo "  Either a publish group was dropped or a Check 2 invocation was deleted." >&2
    return 2
  fi
}

# --negative-control — drive the SAME check code with deliberately broken fixtures and
# assert each reports red. Without this, a refactor can quietly turn the gate vacuous and
# every CI run stays green. Uses fixtures rather than mutating the repo, so it is fast,
# deterministic, and cannot leave the tree dirty.
negative_control() {
  local tmp failures=0
  tmp="$(mktemp -d)"

  # The module's own controls run here too, so `run.sh --negative-control` is the single
  # command that proves every layer of this gate can report red.
  if ! python3 "$REPO_ROOT/ci/publish-metadata/categories.py" --self-test; then
    echo "NEGATIVE CONTROL FAILED: categories.py self-test" >&2
    failures=$((failures + 1))
  fi

  _meta() { # $1 out-file, $2 JSON object for the single package
    python3 - "$1" "$2" <<'PY'
import json, sys
pkg = json.loads(sys.argv[2])
with open(sys.argv[1], "w", encoding="utf-8") as fh:
    json.dump({"packages": [pkg]}, fh)
PY
  }

  # Assert an EXACT exit code, not merely "non-zero". The file's headline contract is
  # 0 pass / 1 the repo is wrong / 2 infrastructure (see the header), and a harness that
  # cannot tell 1 from 2 leaves that contract unasserted for every row below. It also
  # silently absorbs a BROKEN INVOCATION: a Python heredoc reading an argument that was
  # never passed raises IndexError and exits 1, which a non-zero check reports as
  # "ok — reports red" while proving nothing about the rule the row names (measured).
  _expect_rc() { # $1 want-rc, $2 label, rest = command
    local want="$1" label="$2"; shift 2
    local got=0
    "$@" >/dev/null 2>&1 || got=$?
    if [ "$got" -ne "$want" ]; then
      echo "NEGATIVE CONTROL FAILED: $label — expected rc $want, got rc $got" >&2
      failures=$((failures + 1))
    else
      echo "  ok — $label (rc $got)"
    fi
  }

  local good_rp="$tmp/good-release-plz.toml" bad_rp="$tmp/bad-release-plz.toml"
  printf '[workspace]\nrelease = false\n' >"$good_rp"
  printf '[workspace]\n' >"$bad_rp"

  local base='{"name":"paigasus-kernel","version":"0.0.0","publish":null,
    "manifest_path":"/nowhere/Cargo.toml","description":"d","license":"Apache-2.0",
    "repository":"r","readme":"README.md","keywords":["k"],
    "categories":["data-structures"]}'

  # A REAL snapshot for the pre-existing rows. Without it every one of them would fail on
  # a missing 4th argument (rc 2) instead of on the rule it names.
  local fix_snap="$tmp/snapshot.txt"
  printf '# fetched: %s\n# count: 3\ndata-structures\nparser-implementations\naerospace::drones\n' \
    "$(date -u +%Y-%m-%d)" >"$fix_snap"

  # Check 0 — empty publishable set.
  printf '{"packages":[{"name":"x","version":"0.0.0","publish":[],"manifest_path":"/x"}]}' \
    >"$tmp/empty.json"
  _expect_rc 2 "Check 0 (empty publishable set)" \
    metadata_checks "$tmp/empty.json" "$good_rp" "paigasus-kernel" "$fix_snap"

  # Check 0 — set differs from expected.
  _meta "$tmp/wrong-name.json" "$(printf '%s' "$base" | sed 's/paigasus-kernel/some-other-crate/')"
  _expect_rc 1 "Check 0 (unexpected publishable crate)" \
    metadata_checks "$tmp/wrong-name.json" "$good_rp" "paigasus-kernel" "$fix_snap"

  # Check 1 — each rule, one fixture apiece.
  _meta "$tmp/no-desc.json" "$(printf '%s' "$base" | sed 's/"description":"d"/"description":""/')"
  _expect_rc 1 "Check 1 (empty description)" \
    metadata_checks "$tmp/no-desc.json" "$good_rp" "paigasus-kernel" "$fix_snap"

  _meta "$tmp/six-kw.json" "$(printf '%s' "$base" | sed 's/"keywords":\["k"\]/"keywords":["a","b","c","d","e","f"]/')"
  _expect_rc 1 "Check 1 (six keywords)" \
    metadata_checks "$tmp/six-kw.json" "$good_rp" "paigasus-kernel" "$fix_snap"

  _meta "$tmp/long-kw.json" "$(printf '%s' "$base" | sed 's/"keywords":\["k"\]/"keywords":["aaaaaaaaaaaaaaaaaaaaa"]/')"
  _expect_rc 1 "Check 1 (21-char keyword)" \
    metadata_checks "$tmp/long-kw.json" "$good_rp" "paigasus-kernel" "$fix_snap"

  _meta "$tmp/bad-kw.json" "$(printf '%s' "$base" | sed 's/"keywords":\["k"\]/"keywords":["-nope"]/')"
  _expect_rc 1 "Check 1 (keyword with a leading hyphen)" \
    metadata_checks "$tmp/bad-kw.json" "$good_rp" "paigasus-kernel" "$fix_snap"

  _meta "$tmp/no-cat.json" "$(printf '%s' "$base" | sed 's/"categories":\["data-structures"\]/"categories":[]/')"
  _expect_rc 1 "Check 1 (no categories)" \
    metadata_checks "$tmp/no-cat.json" "$good_rp" "paigasus-kernel" "$fix_snap"

  # --- Check 1b: category slugs must be real crates.io slugs ---------------------------
  _meta "$tmp/bad-cat.json" "$(printf '%s' "$base" | sed 's/"data-structures"/"data-structure"/')"
  _expect_rc 1 "Check 1b (category is not a crates.io slug)" \
    metadata_checks "$tmp/bad-cat.json" "$good_rp" "paigasus-kernel" "$fix_snap"

  # crates.io's PUBLISH path matches exactly (categories::slug.eq_any), while its READ api
  # lowercases. A differently-cased slug is therefore DROPPED at publish, and this row is
  # what stops a future "relax this to case-insensitive" edit reintroducing that false green.
  _meta "$tmp/case-cat.json" "$(printf '%s' "$base" | sed 's/"data-structures"/"Data-Structures"/')"
  _expect_rc 1 "Check 1b (category differs only in case)" \
    metadata_checks "$tmp/case-cat.json" "$good_rp" "paigasus-kernel" "$fix_snap"

  _meta "$tmp/ws-cat.json" "$(printf '%s' "$base" | sed 's/"data-structures"/" data-structures"/')"
  _expect_rc 1 "Check 1b (category has surrounding whitespace)" \
    metadata_checks "$tmp/ws-cat.json" "$good_rp" "paigasus-kernel" "$fix_snap"

  _meta "$tmp/nested-ok.json" "$(printf '%s' "$base" | sed 's/"version":"0.0.0"/"version":"0.1.0"/; s/"data-structures"/"aerospace::drones"/')"
  _expect_rc 0 "Check 1b (::-nested slug present in the snapshot)" \
    metadata_checks "$tmp/nested-ok.json" "$bad_rp" "paigasus-kernel" "$fix_snap"

  _meta "$tmp/nested-bad.json" "$(printf '%s' "$base" | sed 's/"data-structures"/"aerospace::drone"/')"
  _expect_rc 1 "Check 1b (::-nested slug absent from the snapshot)" \
    metadata_checks "$tmp/nested-bad.json" "$good_rp" "paigasus-kernel" "$fix_snap"

  # The snapshot guards, driven through the SAME entry point the real run uses.
  _expect_rc 1 "Check 1b (snapshot file missing)" \
    metadata_checks "$tmp/nested-ok.json" "$bad_rp" "paigasus-kernel" "$tmp/nope.txt"

  printf '# fetched: %s\n# count: 0\n' "$(date -u +%Y-%m-%d)" >"$tmp/empty-snap.txt"
  _expect_rc 1 "Check 1b (snapshot contains no slugs)" \
    metadata_checks "$tmp/nested-ok.json" "$bad_rp" "paigasus-kernel" "$tmp/empty-snap.txt"

  # Two valid slugs (categories.py's ABSOLUTE_FLOOR is 2) so this fires on the count
  # mismatch it names, not on the floor guard.
  printf '# fetched: %s\n# count: 99\ndata-structures\nparser-implementations\n' \
    "$(date -u +%Y-%m-%d)" >"$tmp/count-snap.txt"
  _expect_rc 1 "Check 1b (snapshot count disagrees with its body)" \
    metadata_checks "$tmp/nested-ok.json" "$bad_rp" "paigasus-kernel" "$tmp/count-snap.txt"

  printf '# count: 2\ndata-structures\nparser-implementations\n' >"$tmp/nodate-snap.txt"
  _expect_rc 1 "Check 1b (snapshot has no fetched: header)" \
    metadata_checks "$tmp/nested-ok.json" "$bad_rp" "paigasus-kernel" "$tmp/nodate-snap.txt"

  printf '# fetched: 2000-01-01\n# count: 2\ndata-structures\nparser-implementations\n' >"$tmp/old-snap.txt"
  _expect_rc 1 "Check 1b (snapshot older than the staleness bound)" \
    metadata_checks "$tmp/nested-ok.json" "$bad_rp" "paigasus-kernel" "$tmp/old-snap.txt"

  printf '# fetched: %s\n# count: 1\n<html><title>502</title>\n' "$(date -u +%Y-%m-%d)" >"$tmp/html-snap.txt"
  _expect_rc 1 "Check 1b (snapshot is an HTML error page)" \
    metadata_checks "$tmp/nested-ok.json" "$bad_rp" "paigasus-kernel" "$tmp/html-snap.txt"

  # CRLF must be TOLERATED — the repo ships no .gitattributes, and rejecting it would red
  # every PR on a CRLF checkout with a message that is wrong about what is broken. Contains
  # aerospace::drones (nested-ok.json's category) plus a second slug to clear the floor.
  printf '# fetched: %s\r\n# count: 2\r\naerospace::drones\r\ndata-structures\r\n' \
    "$(date -u +%Y-%m-%d)" >"$tmp/crlf-snap.txt"
  _expect_rc 0 "Check 1b (CRLF snapshot is tolerated)" \
    metadata_checks "$tmp/nested-ok.json" "$bad_rp" "paigasus-kernel" "$tmp/crlf-snap.txt"

  # A broken INVOCATION must not read as "Check 1b skipped".
  _expect_rc 2 "Check 1b (snapshot argument not passed)" \
    metadata_checks "$tmp/nested-ok.json" "$bad_rp" "paigasus-kernel"

  # Check 3 — a 0.0.0 crate with no release block.
  _meta "$tmp/stub.json" "$base"
  _expect_rc 1 "Check 3 (0.0.0 crate not release-blocked)" \
    metadata_checks "$tmp/stub.json" "$bad_rp" "paigasus-kernel" "$fix_snap"

  # The per-package override hole: [[package]] beats [workspace], so a `release = true`
  # entry leaves the crate releasable even with the workspace block in place. This is the
  # edit a maintainer makes when activating release-plz for one crate.
  local override_rp="$tmp/override-release-plz.toml"
  printf '[workspace]\nrelease = false\n\n[[package]]\nname = "paigasus-kernel"\nrelease = true\n' >"$override_rp"
  _expect_rc 1 "Check 3 (per-package release = true override)" \
    metadata_checks "$tmp/stub.json" "$override_rp" "paigasus-kernel" "$fix_snap"

  # --- Check 1c fixtures ---------------------------------------------------------
  printf '[package]\nname = "f"\n[lints]\nworkspace = true\n' >"$tmp/lints-inherit.toml"
  _expect_rc 1 "Check 1c (inherits workspace lints)" \
    assert_lint_table "$tmp/lints-inherit.toml"

  printf '[package]\nname = "f"\n' >"$tmp/lints-absent.toml"
  _expect_rc 1 "Check 1c (no lint table at all)" \
    assert_lint_table "$tmp/lints-absent.toml"

  printf '[package]\nname = "f"\n[lints.rust]\nwarnings = "deny"\n' >"$tmp/lints-deny-str.toml"
  _expect_rc 1 "Check 1c (own table but warnings = deny, string form)" \
    assert_lint_table "$tmp/lints-deny-str.toml"

  printf '[package]\nname = "f"\n[lints.rust]\nwarnings = { level = "forbid", priority = -1 }\n' \
    >"$tmp/lints-forbid-tbl.toml"
  _expect_rc 1 "Check 1c (own table but warnings = forbid, table form)" \
    assert_lint_table "$tmp/lints-forbid-tbl.toml"

  printf '[package]\nname = "f"\n[lints.rust]\nwarnings = "warn"\n[lints.clippy]\nall = "deny"\n' \
    >"$tmp/lints-clippy-deny.toml"
  _expect_rc 1 "Check 1c (clippy.all = deny)" \
    assert_lint_table "$tmp/lints-clippy-deny.toml"

  printf '[package]\nname = "f"\n[lints.rust]\nwarnings = "warn"\n[lints.clippy]\nall = "warn"\n' \
    >"$tmp/lints-good.toml"
  _expect_rc 0 "Check 1c (own table, warn — passes)" \
    assert_lint_table "$tmp/lints-good.toml"

  _expect_rc 2 "Check 1c (malformed TOML is infra, not a repo defect)" \
    assert_lint_table "$tmp/does-not-exist.toml"

  # --- Check 1d fixtures ---------------------------------------------------------
  printf '[package]\nname = "f"\n' >"$tmp/inc-absent.toml"
  _expect_rc 1 "Check 1d (no include key)" \
    assert_include_allowlist "$tmp/inc-absent.toml"

  printf '[package]\nname = "f"\ninclude = []\n' >"$tmp/inc-empty.toml"
  _expect_rc 1 "Check 1d (empty include)" \
    assert_include_allowlist "$tmp/inc-empty.toml"

  printf '[package]\nname = "f"\n[package.include]\nworkspace = true\n' >"$tmp/inc-inherit.toml"
  _expect_rc 1 "Check 1d (include.workspace = true is not an allowlist)" \
    assert_include_allowlist "$tmp/inc-inherit.toml"

  printf '[package]\nname = "f"\ninclude = ["src/**/*.rs", "Cargo.toml", "README.md"]\n' \
    >"$tmp/inc-no-license.toml"
  _expect_rc 1 "Check 1d (include omits LICENSE)" \
    assert_include_allowlist "$tmp/inc-no-license.toml"

  printf '[package]\nname = "f"\ninclude = ["**/*"]\n' >"$tmp/inc-wildcard.toml"
  _expect_rc 1 "Check 1d (a wildcard is not literal membership)" \
    assert_include_allowlist "$tmp/inc-wildcard.toml"

  printf '[package]\nname = "f"\ninclude = ["src/**/*.rs", "Cargo.toml", "README.md", "LICENSE"]\n' \
    >"$tmp/inc-good.toml"
  _expect_rc 0 "Check 1d (proper allowlist — passes)" \
    assert_include_allowlist "$tmp/inc-good.toml"

  _expect_rc 2 "Check 1d (malformed TOML is infra, not a repo defect)" \
    assert_include_allowlist "$tmp/does-not-exist.toml"

  # --- Check 2 grouping + invoked-set fixtures -----------------------------------
  _expect_rc 2 "Check 2 (empty package list is non-vacuous)" \
    check_publish_group

  # The invoked-set assertion must fire when Check 2 covered less than 2b enumerated.
  CHECK2_INVOKED=("paigasus-kernel")
  _expect_rc 2 "Check 2 (invoked set shorter than the enumerated set)" \
    assert_check2_covered_everything "paigasus-kernel" "paigasus-proto"
  CHECK2_INVOKED=("paigasus-kernel" "paigasus-proto")
  _expect_rc 0 "Check 2 (invoked set matches the enumerated set)" \
    assert_check2_covered_everything "paigasus-proto" "paigasus-kernel"
  CHECK2_INVOKED=()

  # publish_groups must separate independent crates and join dependent ones.
  printf '%s' '{"packages":[
    {"name":"a","dependencies":[]},
    {"name":"b","dependencies":[{"name":"c"}]},
    {"name":"c","dependencies":[]}]}' >"$tmp/groups.json"
  local got_groups
  got_groups="$(publish_groups "$tmp/groups.json" "a,b,c")"
  if [ "$got_groups" != "$(printf 'a\nb\tc')" ]; then
    echo "NEGATIVE CONTROL FAILED: publish_groups — expected 'a' and 'b<TAB>c', got: $got_groups" >&2
    failures=$((failures + 1))
  else
    echo "  ok — publish_groups separates independent crates and joins dependent ones"
  fi

  # Check 2b — a listing missing LICENSE, and one containing moon.yml.
  printf 'Cargo.toml\nREADME.md\nsrc/lib.rs\n' >"$tmp/missing-license.txt"
  _expect_rc 1 "Check 2b (LICENSE not packaged)" \
    assert_package_list "$tmp/missing-license.txt" "fixture"

  printf 'Cargo.toml\nREADME.md\nLICENSE\nmoon.yml\n' >"$tmp/leaks-moon.txt"
  _expect_rc 1 "Check 2b (moon.yml packaged)" \
    assert_package_list "$tmp/leaks-moon.txt" "fixture"

  # --- Check 4: the freshness job's call site ------------------------------------------
  local wf_ok="$tmp/wf-ok.yml" wf_gone="$tmp/wf-gone.yml" wf_coe="$tmp/wf-coe.yml"
  printf 'jobs:\n  freshness:\n    steps:\n      - run: ci/publish-metadata/run.sh --check-categories-freshness\n' >"$wf_ok"
  printf 'jobs:\n  freshness:\n    steps:\n      - run: echo nothing\n' >"$wf_gone"
  printf 'jobs:\n  freshness:\n    steps:\n      - run: ci/publish-metadata/run.sh --check-categories-freshness\n        continue-on-error: true\n' >"$wf_coe"

  # F1 regression guard: the reviewer's measured attack — comment the invocation out and
  # replace the step body with something inert. A bare `grep -qF` substring match survives
  # this (the literal still appears, just behind a `#`); the anchored run: match must not.
  local wf_commented="$tmp/wf-commented.yml"
  printf 'jobs:\n  freshness:\n    steps:\n      - # run: ci/publish-metadata/run.sh --check-categories-freshness (disabled)\n        run: echo disabled\n' >"$wf_commented"

  # F4 regression guard: the reviewer's measured MAJOR bypass — a run: line containing the
  # FLAG TEXT but not the actual command. The old regex matched on the flag substring alone
  # and passed this; it must now fail, since nothing here invokes the gate.
  local wf_flag_only="$tmp/wf-flag-only.yml"
  printf 'jobs:\n  freshness:\n    steps:\n      - run: echo --check-categories-freshness\n' >"$wf_flag_only"

  # F1 regression guard: the invocation is present and uncommented, but its exit status is
  # discarded by a trailing tail on the SAME run: line — mirrors ci/actionlint/run.sh check 8.
  local wf_discarded="$tmp/wf-discarded.yml"
  printf 'jobs:\n  freshness:\n    steps:\n      - run: ci/publish-metadata/run.sh --check-categories-freshness || true\n' >"$wf_discarded"

  # F2 regression guard: `if: false` on the job suppresses it entirely without touching
  # continue-on-error at all — measured to leave the old gate at rc 0.
  local wf_if_false="$tmp/wf-if-false.yml"
  printf 'jobs:\n  category-slugs:\n    if: false\n    steps:\n      - run: ci/publish-metadata/run.sh --check-categories-freshness\n' >"$wf_if_false"

  _expect_rc 0 "Check 4 (workflow invokes the freshness check)" \
    assert_freshness_call_site "$wf_ok"
  _expect_rc 1 "Check 4 (freshness invocation deleted)" \
    assert_freshness_call_site "$wf_gone"
  _expect_rc 1 "Check 4 (freshness step suppressed by continue-on-error)" \
    assert_freshness_call_site "$wf_coe"
  _expect_rc 1 "Check 4 (invocation commented out, replaced with an inert run:)" \
    assert_freshness_call_site "$wf_commented"
  _expect_rc 1 "Check 4 (run: line contains only the flag text, not the command)" \
    assert_freshness_call_site "$wf_flag_only"
  _expect_rc 1 "Check 4 (invocation present but its exit status is discarded by || true)" \
    assert_freshness_call_site "$wf_discarded"
  _expect_rc 1 "Check 4 (job suppressed by if: false)" \
    assert_freshness_call_site "$wf_if_false"
  _expect_rc 2 "Check 4 (workflow file unreadable)" \
    assert_freshness_call_site "$tmp/no-such-workflow.yml"

  # The REAL workflow must satisfy the same assertion the fixtures do.
  _expect_rc 0 "Check 4 (the real security-scan.yml passes)" \
    assert_freshness_call_site "$REPO_ROOT/.github/workflows/security-scan.yml"

  # Positive control: a clean fixture must pass, or every "red" above is meaningless.
  _meta "$tmp/good.json" "$(printf '%s' "$base" | sed 's/"version":"0.0.0"/"version":"0.1.0"/')"
  _expect_rc 0 "clean fixture passes (checks are not vacuously red)" \
    metadata_checks "$tmp/good.json" "$bad_rp" "paigasus-kernel" "$fix_snap"

  rm -rf "$tmp"
  if [ "$failures" -gt 0 ]; then
    echo "negative control: $failures check(s) failed to bite" >&2
    return 1
  fi
  echo "negative control: every check reports red on a broken fixture"
}

main() {
  cd "$RS_DIR"

  # Before anything expensive: if the freshness job is gone, the snapshot is unmaintained
  # and every other check here is validating against data nothing keeps current.
  assert_freshness_call_site "$REPO_ROOT/.github/workflows/security-scan.yml" || exit $?

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
  publishable="$(metadata_checks "$meta_json" "$RS_DIR/release-plz.toml" "$expected_csv" "$SNAPSHOT")" \
    || status=$?
  [ "$status" -eq 0 ] || { rm -f "$meta_json"; exit "$status"; }

  local groups
  groups="$(publish_groups "$meta_json" "$expected_csv")" || status=$?
  rm -f "$meta_json"
  [ "$status" -eq 0 ] || exit "$status"

  # Checks 1c, 1d and 2b — per package. Read on FD 3, not stdin (the
  # ci/release-parity/run.sh idiom): a loop-body subprocess that reads stdin can swallow
  # rows silently, and a silent skip reads as a false green. Not a live bug today — cargo
  # never touches stdin here, all 3/3 iterations verified — but it becomes one the day
  # SMA-388 adds a second publishable crate.
  local name dir enumerated=()
  while IFS=$'\t' read -r -u 3 name dir; do
    [ -n "$name" ] || continue
    enumerated+=("$name")
    PKG_DIR[$name]="$dir"
    status=0; assert_lint_table "$dir/Cargo.toml" || status=$?
    [ "$status" -eq 0 ] || exit "$status"
    status=0; assert_include_allowlist "$dir/Cargo.toml" || status=$?
    [ "$status" -eq 0 ] || exit "$status"
    status=0; check_package_list "$name" "$dir" || status=$?
    [ "$status" -eq 0 ] || exit "$status"
  done 3<<<"$publishable"

  # Check 2 — one dry-run per publish group.
  local group_line
  while IFS= read -r -u 3 group_line; do
    [ -n "$group_line" ] || continue
    local group_pkgs
    IFS=$'\t' read -r -a group_pkgs <<<"$group_line"
    status=0; check_publish_group "${group_pkgs[@]}" || status=$?
    [ "$status" -eq 0 ] || exit "$status"
  done 3<<<"$groups"

  assert_check2_covered_everything "${enumerated[@]}" || exit $?

  echo "publish-metadata: all checks passed"
}

# Explicit dispatch (ci/release-parity/run.sh's style): an unrecognized argument must
# exit 2 with a usage message, never fall through to the normal run — a typo'd
# `--negativecontrol` silently running the full gate and printing a pass is exactly the
# kind of broken invocation this script's exit-code contract exists to rule out.
case "${1:-}" in
  '') main "$@" ;;
  --negative-control) negative_control ;;
  --check-categories-freshness)
    exec python3 "$REPO_ROOT/ci/publish-metadata/categories.py" \
      --check-freshness --snapshot "$SNAPSHOT" ;;
  --refresh-categories)
    exec python3 "$REPO_ROOT/ci/publish-metadata/categories.py" \
      --refresh --snapshot "$SNAPSHOT" ;;
  -h|--help)
    echo "usage: run.sh [--negative-control | --check-categories-freshness |"
    echo "               --refresh-categories | -h|--help]"
    exit 0 ;;
  *) echo "unknown arg: $1" >&2; exit 2 ;;
esac
