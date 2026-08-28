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
#   Check 1c each publishable crate declares its OWN [lints.*] table and does not deny.
#            Cargo inlines the resolved table into the published manifest and docs.rs builds
#            on nightly as the root package, where --cap-lints allow does not apply — so an
#            inherited (or hand-written) `warnings = "deny"` silently kills docs.rs builds
#            on the first new rustc lint, months later (SMA-577).
#   Check 1d each publishable crate declares a non-empty `include` ALLOWLIST containing
#            README.md and LICENSE. Membership is literal: "**/*" is rejected, because it
#            would "cover" both while reinstating the moon.yml leak 2b exists to catch.
#   Check 2  `cargo publish --dry-run` succeeds, once per PUBLISH GROUP — a connected
#            component of the in-set dependency graph, computed at runtime from `cargo
#            metadata` — not once per package: a per-package dry-run of a crate with an
#            unpublished in-tree dependency cannot succeed (`-p paigasus-proto` alone exits
#            101, `no matching package named 'paigasus-proto-derive'`, until the derive crate
#            is on crates.io). The group is publishABLE, packages, and compiles with no
#            unversioned path dependency OUTSIDE the group.
#   Check 2b the packaged file list ships README.md + LICENSE and not moon.yml.
#   Check 2c the packaged set does not contain EVERY tracked file in the crate dir. This is
#            Check 1d's rule enforced BEHAVIOURALLY: 1d rejects catch-all `include` entries
#            by spelling, and that list can never be complete — /**, /*, **/, /, **/**,
#            */**, */*, ?*, [a-z]* and **/*.* were all MEASURED to package a crate's whole
#            root, and any glob that happens to match everything joins them. If the allowlist
#            held nothing back it matched everything, whatever it was spelled. Note this is a
#            SUBSET test, not equality: a catch-all can also sweep untracked files in, making
#            the packaged set a strict superset while every tracked file still ships (SMA-577).
#   Check 3  while any publishable crate is at 0.0.0, rs/release-plz.toml must block its
#            release. Releasing 0.0.0 permanently burns that version on crates.io.
#   Check 4  .github/workflows/security-scan.yml still INVOKES the freshness check on a real,
#            non-comment run: line whose exit status is not discarded, and does not suppress
#            it with continue-on-error or if:. Nothing else guards a workflow job:
#            repo:actionlint's call-site machinery is keyed on ci.yml only (SMA-529).
#
# The P* checks are the PYTHON arm (SMA-578). The crates.io half above is discovered from
# Cargo's `publish` flag; PyPI has no equivalent, and in this repo the version field means
# "in a lockstep family" rather than "publishable" — paigasus-py-bindings is `publish =
# false` on the Cargo side and PyPI-bound at the same time. So the Python set is discovered
# from an explicit `[tool.paigasus] pypi = true` MARKER: the publish decision itself.
#
#   Check P0 the PyPI-bound set EQUALS EXPECTED_PYPI_PUBLISHABLE — this arm's non-vacuity
#            control, mirroring Check 0. The manifests it compares are DISCOVERED at
#            runtime (pypi_scan_paths, a one-level `py/packages/*/pyproject.toml` glob plus
#            the bindings manifest, which lives outside py/), not hand-listed: for this arm
#            the scan set IS the discovery, so a stale list silently shrinks the gate rather
#            than reporting red. Measured before SMA-578 review I1: a new PyPI-bound package
#            with no description, an SPDX/classifier clash and a declared-but-absent LICENSE
#            passed rc 0.
#   Check P1 each PyPI-bound distribution carries the [project] metadata PyPI needs, does
#            not pair an SPDX license expression with a `License ::` trove classifier (PyPI
#            hard-rejects that, SMA-378), and — for the crates whose SOURCES SHIP IN AN
#            SDIST — carries Check 1c's own non-denying lint table. 1c cannot see those
#            crates: it iterates the `publish = true` set, and paigasus-py-bindings is not
#            in it, yet maturin ships the workspace Cargo.toml verbatim so an sdist consumer
#            compiles it as the ROOT package where `--cap-lints allow` does not apply.
#   Check P2 the files those fields NAME exist on disk. uv_build does not auto-glob license
#            files (SMA-378), so a declared-but-absent LICENSE ships a wheel with no licence
#            text and nothing else notices.
#   Check P-D6 .github/workflows/wheels.yml declares neither `secrets:` nor
#            `id-token: write`. It is pull_request-triggered and same-repo PRs receive
#            repository secrets, so moving the upload into it — the natural refactor once
#            the artifacts are there — would reopen SMA-407 §7 review M2. wheels.yml's own
#            header comment cites this gate; that check is what makes the citation true.
#
# The Python arm is deliberately SPELLING-LEVEL and pure-Python: this gate is in ci.yml's
# required `moon ci` target list under `toolchain: 'system'`, which installs no maturin, so
# no check here may build an artifact. Behavioural wheel/sdist assertions live in
# .github/workflows/wheels.yml (SMA-578 review M6).
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

# --- SMA-578: the PyPI arm ---------------------------------------------------------
# The PyPI-bound set, discovered from the `[tool.paigasus] pypi` MARKER — not from the
# version field. In this repo `version != "0.0.0"` means "in a lockstep family"
# (repo:version-lockstep writes it), and paigasus-py-bindings is simultaneously
# `publish = false` on the Cargo side and PyPI-bound (SMA-578 review M7).
#
# py/packages/paigasus-proto is DELIBERATELY absent: it is version-locked with the proto
# family and its name is reserved on PyPI, but no publish path uploads it yet. SMA-579
# owns that decision (SMA-578 §9.3) — it must be recorded, not made by omission.
EXPECTED_PYPI_PUBLISHABLE=("paigasus-kernel" "paigasus-py-bindings")

# The scan set. The py/packages half is DISCOVERED AT RUNTIME from a single-level glob, not
# hand-maintained. The distinction matters more here than it looks: the crates.io half is
# immune to a stale list by construction, because Check 0 compares EXPECTED_PUBLISHABLE
# against a runtime `cargo metadata` walk of the whole workspace. For the Python half the
# scan set IS the discovery, so a hand-written list silently SHRINKS the gate — a new
# PyPI-bound package with no description, an SPDX/classifier clash and a declared-but-absent
# LICENSE was MEASURED to pass green, rc 0, with nothing in moon.yml even selecting the task
# (SMA-578 review I1). With discovery at runtime, Check P0's strict equality becomes a real
# completeness assertion: a new marked package reds the gate until someone adds it to
# EXPECTED_PYPI_PUBLISHABLE, exactly as a new publishable crate does on the Cargo side.
#
# ONE LEVEL, never `**/`: a recursive glob sweeps in py/pyproject.toml (a uv virtual root
# with NO [project] table, which this arm classifies as rc 2 infrastructure) and, in a
# provisioned tree, ts/node_modules/.pnpm/…/node-gyp/gyp/pyproject.toml. `py/packages/*/`
# matches neither. The bindings manifest is named literally because it lives OUTSIDE py/.
PYPI_SCAN_GLOB='py/packages/*/pyproject.toml'
PYPI_SCAN_EXTRA=("rs/crates/bindings/paigasus-py-bindings/pyproject.toml")

# Required [project] keys for a PyPI-bound distribution.
PYPI_REQUIRED_FIELDS=("description" "readme" "license" "license-files" "authors" "classifiers")

# Check P1 (continued) — Check 1c's rule, extended to crates whose SOURCES SHIP IN A
# PUBLISHED SDIST rather than only to `publish = true` crates. maturin ships the workspace
# Cargo.toml verbatim (measured), so an sdist consumer compiles as the ROOT package where
# `--cap-lints allow` does not apply. Check 1c misses paigasus-py-bindings precisely
# because that crate is `publish = false` (SMA-578 review B2).
SDIST_SHIPPED_CRATES=("rs/crates/bindings/paigasus-py-bindings" "rs/crates/libs/paigasus-kernel")

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
elif not any(
    isinstance(v, dict) and v for k, v in lints.items() if k != "workspace"
):
    # `[lints] workspace = false` is VALID TOML and, per cargo's reference, is equivalent to
    # omitting the key entirely — it declares no local lint namespace at all. It is not caught
    # by the `is True` arm above, and it used to reach the level checks below, where an absent
    # `rust`/`clippy` key yields no findings and the crate PASSED. That contradicts this
    # check's own stated rule ("must declare its own"), so it is rejected here rather than
    # falling through. Same treatment for any `[lints]` table whose only key is `workspace`.
    errors.append(
        f"{name}: `[lints]` declares no NON-EMPTY local namespace. `workspace = false` is "
        "valid but equivalent to having no lint table at all — it inherits nothing AND "
        "declares nothing — and a present-but-empty `[lints.rust]` is the same vacuity in a "
        "different shape: it satisfies 'has a local table' while setting no lint. The rule is "
        "discipline, not only hazard-avoidance: declare a per-crate `[lints.rust]` / "
        '`[lints.clippy]` table that actually sets `warnings = "warn"`, so a crate cannot '
        "drift into workspace inheritance by deletion."
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
    # A catch-all entry defeats the allowlist even when the required literals are ALSO
    # listed: `["README.md", "LICENSE", "**/*"]` satisfies literal membership while
    # packaging the whole directory, reinstating the exact moon.yml leak Check 2b exists
    # to catch. Rejecting the bare `["**/*"]` shape via literal membership was not enough.
    # MEASURED against cargo 1.95.0, not guessed. A probe crate carrying `private/secret.txt`
    # plus `include = ["README.md", "LICENSE", <pattern>]` was packaged for each candidate;
    # these six ship the secret, `./**` and a scoped `src/**/*.rs` do not. `/*` is the
    # counter-intuitive one — cargo applies it recursively, unlike strict gitignore reading —
    # and `/**` is what a reviewer found bypassing the original three-entry list.
    CATCH_ALLS = ("**/*", "**", "*", "/**", "/*", "**/")
    catch_alls = [e for e in include if isinstance(e, str) and e.strip() in CATCH_ALLS]
    if catch_alls:
        errors.append(
            f"{name}: `include` contains the catch-all {catch_alls[0]!r}, which packages the "
            "whole crate directory and makes the rest of the allowlist decorative. Enumerate "
            "what belongs — an allowlist that matches everything is not an allowlist."
        )
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
# Check 2c — the packaged set must not equal the crate's tracked file set.
#
# Check 1d rejects catch-all `include` entries by SPELLING, and that list can never be
# complete: `**/*`, `**`, `*`, `/**`, `/*`, `**/`, `/`, `**/**`, `*/**`, `*/*`, `?*`,
# `[a-z]*` and `**/*.*` were all MEASURED to package a crate's whole root, and any glob
# that happens to match everything joins them. Enumerating spellings is the wrong tool.
#
# This is the same invariant checked BEHAVIOURALLY instead: if the include shipped every
# tracked file, it matched everything, and it is a catch-all whatever it is spelled. That
# holds for a spelling nobody has thought of yet.
#
# Takes BOTH listings as files so --negative-control drives the same code with synthetic
# sets — there is no way to fixture a git-tracked directory cheaply.
#
# NOTE the failure direction. A crate whose tracked files are ALL legitimately publishable
# would false-red here. That is deliberate and is the repo's standing preference (see
# EXPECTED_SITE_COUNT's comment in ci/version-lockstep/run.sh): a gate that can only
# false-red is safe, one that can silently absorb a bypass is not. Today every crate dir
# carries a moon.yml that must never ship, so the sets cannot legitimately coincide.
assert_not_catch_all() { # $1 packaged listing  $2 tracked listing  $3 package name
  local packaged="$1" tracked="$2" pkg="$3"

  if [ ! -r "$packaged" ] || [ ! -r "$tracked" ]; then
    echo "FATAL: Check 2c cannot read its listings for $pkg" >&2
    return 2
  fi
  # Non-vacuity: an empty tracked set would make the comparison meaningless and pass.
  if [ ! -s "$tracked" ]; then
    echo "FATAL: Check 2c got an EMPTY tracked-file set for $pkg — it would assert nothing" >&2
    return 2
  fi

  # Cargo synthesizes these into every tarball; they are not crate sources and must not
  # enter the comparison.
  local pkg_norm
  pkg_norm="$(grep -vxE 'Cargo\.lock|Cargo\.toml\.orig|\.cargo_vcs_info\.json' "$packaged" \
    | LC_ALL=C sort -u)"
  local trk_norm
  trk_norm="$(LC_ALL=C sort -u "$tracked")"

  # SUBSET, not equality. The invariant is "the allowlist excluded something", i.e. at
  # least one tracked file did NOT ship. Equality was the first attempt and is WRONG: any
  # extra entry in the tarball defeats it, and a catch-all readily produces extras (a probe
  # under --allow-dirty swept .git/** in, so packaged was a strict SUPERSET of tracked and
  # the equality test passed a genuine catch-all). Asking "was anything held back?" is
  # immune to extras.
  local excluded
  excluded="$(LC_ALL=C comm -23 <(printf '%s\n' "$trk_norm") <(printf '%s\n' "$pkg_norm"))"

  if [ -z "$excluded" ]; then
    echo "Check 2c FAILED: $pkg packages EVERY tracked file in its directory — the" >&2
    echo "  \`include\` list held nothing back, so it matched everything and is a catch-all" >&2
    echo "  however it is spelled. Check 1d's denylist of literal catch-all patterns cannot" >&2
    echo "  see a spelling it does not know (measured: /**, /*, **/, /, **/**, */**, */*," >&2
    echo "  ?*, [a-z]* and **/*.* all package a crate's whole root). Enumerate what belongs." >&2
    return 1
  fi
}

check_package_list() { # $1 name  $2 manifest dir
  local pkg="$1" pkg_dir="$2" dirty=() out listing tracked status

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
  if [ "$status" -eq 0 ]; then
    # Tracked files only, paths relative to the crate dir — the same shape cargo prints.
    # Deliberately NOT `find`: untracked scratch would inflate the set and mask a
    # catch-all, and a gate's assertion must be about the committed tree.
    tracked="$(mktemp)"
    # $pkg_dir is ABSOLUTE (metadata_checks prints os.path.dirname(manifest_path)), so it
    # is passed to -C directly rather than joined onto REPO_ROOT.
    if ! git -C "$pkg_dir" ls-files >"$tracked" 2>/dev/null; then
      echo "FATAL: Check 2c could not list tracked files for $pkg" >&2
      rm -f "$out" "$listing" "$tracked"
      return 2
    fi
    assert_not_catch_all "$listing" "$tracked" "$pkg" || status=$?
    rm -f "$tracked"
  fi
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

# Expand the scan set against a root, printing one absolute manifest path per line. Takes
# the root as an ARGUMENT so --negative-control drives the same discovery with a fixture
# tree — the idiom every other check in this file follows.
# Exit: 0 pass | 2 infrastructure (the glob matched nothing, a literal extra is absent, or
# the root is not a directory — in each case the arm would scan LESS than it believes it
# does, which is the vacuity this whole file exists to rule out).
pypi_scan_paths() { # $1 repo root
  local root="${1:-}"
  if [ -z "$root" ] || [ ! -d "$root" ]; then
    printf 'FATAL: pypi_scan_paths: %s is not a directory\n' "${root:-<no argument>}" >&2
    return 2
  fi

  local -a hits=()
  local p
  # The glob is expanded in a SUBSHELL, `cd`-ed to $root: main() runs from rs/, so a bare
  # glob would silently expand against the wrong directory, and `nullglob` must not leak
  # into the rest of the script. Reading the result line by line is safe here because a
  # path containing a newline cannot be a tracked pyproject.toml in this repo.
  while IFS= read -r p; do
    [ -n "$p" ] || continue
    hits+=("$root/$p")
  done < <(cd "$root" && shopt -s nullglob && printf '%s\n' $PYPI_SCAN_GLOB)

  if [ "${#hits[@]}" -eq 0 ]; then
    printf 'FATAL: %s matched no manifest under %s. The PyPI arm would then scan only the\n' \
      "$PYPI_SCAN_GLOB" "$root" >&2
    printf '       literal extras — a silently shrunken gate, not a pass.\n' >&2
    return 2
  fi

  if [ "${#PYPI_SCAN_EXTRA[@]}" -eq 0 ]; then
    printf 'FATAL: PYPI_SCAN_EXTRA is empty — the bindings manifest lives outside py/ and\n' >&2
    printf '       would go unscanned.\n' >&2
    return 2
  fi

  local extra
  for extra in "${PYPI_SCAN_EXTRA[@]}"; do
    if [ ! -f "$root/$extra" ]; then
      printf 'FATAL: %s/%s does not exist — a literal scan entry went stale.\n' "$root" "$extra" >&2
      return 2
    fi
    hits+=("$root/$extra")
  done

  printf '%s\n' "${hits[@]}"
}

# Checks P0/P1/P2 — the PyPI packaging-metadata arm (SMA-578 §8).
#
#   Check P0  the PyPI-bound set EQUALS EXPECTED_PYPI_PUBLISHABLE. The non-vacuity control
#             for this arm, mirroring Check 0: the set is discovered from the very marker
#             this gate protects, so a shrunken set must be a hard failure rather than a
#             green run over nothing.
#   Check P1  each PyPI-bound distribution carries the metadata PyPI needs, and does NOT
#             supply an SPDX license expression alongside a `License ::` trove classifier —
#             PyPI hard-rejects that combination (SMA-378).
#   Check P2  the files those fields NAME exist on disk. uv_build does not auto-glob license
#             files (SMA-378), so a declared-but-absent LICENSE means a wheel that ships no
#             license text — and nothing else notices.
#
# Takes the pyproject paths as arguments so --negative-control drives the SAME code with
# fixtures. Exit: 0 pass | 1 the repo is wrong | 2 infrastructure.
#
# NOTE: this arm is SPELLING-LEVEL and pure-Python by design. It runs inside `moon ci`'s
# required check under `toolchain: 'system'`, where no maturin is installed, so it must
# never build an artifact; the behavioural wheel/sdist assertions live in
# .github/workflows/wheels.yml (SMA-578 review M6).
assert_pypi_metadata() { # $@ pyproject paths
  # The rule sets are exported HERE rather than at the call site so main() and
  # negative_control()'s direct fixture calls both see them. A caller that forgot would
  # make the heredoc raise KeyError and exit 1 — a broken invocation reading as "the repo
  # is wrong", the exact 1-vs-2 confusion this file's exit-code contract rules out.
  EXPECTED_PYPI_PUBLISHABLE="${EXPECTED_PYPI_PUBLISHABLE[*]}" \
  PYPI_REQUIRED_FIELDS="${PYPI_REQUIRED_FIELDS[*]}" \
  python3 - "$@" <<'PY'
import os, sys, tomllib

expected = set(os.environ.get("EXPECTED_PYPI_PUBLISHABLE", "").split())
required = os.environ.get("PYPI_REQUIRED_FIELDS", "").split()
paths, errors, found = sys.argv[1:], [], {}

if not paths:
    print("FATAL: no pyproject paths given — this check would pass vacuously", file=sys.stderr)
    raise SystemExit(2)
if not expected or not required:
    print("FATAL: empty rule set — this check would pass vacuously", file=sys.stderr)
    raise SystemExit(2)

for p in paths:
    try:
        with open(p, "rb") as fh:
            doc = tomllib.load(fh)
    except (OSError, tomllib.TOMLDecodeError) as exc:
        # Infrastructure, not "the repo is wrong": an unreadable or unparsable manifest is
        # a different failure mode from a manifest that is present and wrong.
        print(f"FATAL: cannot read {p}: {exc}", file=sys.stderr)
        raise SystemExit(2)
    proj = doc.get("project")
    if not isinstance(proj, dict) or "name" not in proj:
        print(f"FATAL: {p} has no [project] table with a name", file=sys.stderr)
        raise SystemExit(2)
    name = proj["name"]
    if doc.get("tool", {}).get("paigasus", {}).get("pypi") is True:
        found[name] = (p, proj)

# P0 — strict equality. The set is discovered from the very marker this gate protects, so
# a shrunken set must be a hard failure, not a green run over nothing (mirrors Check 0).
if set(found) != expected:
    errors.append(
        f"Check P0 FAILED: PyPI-bound set is {sorted(found)}, expected {sorted(expected)}"
    )

for name, (p, proj) in sorted(found.items()):
    # P1 — required metadata.
    for field in required:
        if not proj.get(field):
            errors.append(f"Check P1 FAILED: {name} ({p}) has no [project] {field}")
    # P1 — the SPDX-vs-classifier rule (SMA-378): PyPI hard-rejects an SPDX license
    # expression supplied ALONGSIDE a License :: trove classifier.
    if proj.get("license") and any(
        str(c).startswith("License ::") for c in proj.get("classifiers", [])
    ):
        errors.append(
            f"Check P1 FAILED: {name} supplies an SPDX license AND a 'License ::' "
            f"classifier — PyPI rejects the combination; drop the classifier"
        )
    # P2 — the files those fields name must EXIST. uv_build does not auto-glob license
    # files (SMA-378), so a missing file means a wheel that ships no license text.
    base = os.path.dirname(p)
    for rel in [proj.get("readme")] + list(proj.get("license-files") or []):
        if isinstance(rel, str) and not os.path.isfile(os.path.join(base, rel)):
            errors.append(
                f"Check P2 FAILED: {name} declares {rel!r} but {base}/{rel} does not exist"
            )

for e in errors:
    print(e, file=sys.stderr)
raise SystemExit(1 if errors else 0)
PY
}

# Check P1 (continued) — apply assert_lint_table (Check 1c's rule) to the crates whose
# sources ship inside a published sdist. Reuses the existing checker rather than carrying a
# second copy.
#
# Takes the crate dirs as ARGUMENTS. It used to read SDIST_SHIPPED_CRATES directly and so
# could not be driven with fixtures, which left it with no red-path control: the only row
# touching this wrapper was the rc-0 real-repo one (the rc-1 row called assert_lint_table
# DIRECTLY), and replacing the body's `|| rc=$?` with `|| true` was MEASURED to leave the
# harness fully green. That is the guard-the-guard shape SMA-542 recorded — a fixture table
# exercises the verdict function and never its invocation — so the wrapper now takes
# arguments like every other check here and carries rc-1/rc-2 rows of its own.
#
# The empty-set guard `return`s rather than calling die_infra for the same reason: die_infra
# exits the process, so the guard could not be asserted from the negative control at all.
assert_sdist_lint_tables() { # $@ crate dirs (absolute)
  local dir rc=0 sub=0
  if [ "$#" -eq 0 ]; then
    printf 'FATAL: assert_sdist_lint_tables got no crate dirs — it would pass vacuously.\n' >&2
    return 2
  fi
  for dir in "$@"; do
    sub=0
    assert_lint_table "$dir/Cargo.toml" || sub=$?
    if [ "$sub" -ne 0 ]; then
      # Disambiguate the delegated banner. assert_lint_table prints "Check 1c FAILED", but
      # this file documents Check 1c as iterating the `publish = true` set — so a
      # maintainer reading that banner for a `publish = false` crate concludes the message
      # is impossible. Name the arm that actually selected it.
      printf 'Check P1 (sdist-shipped crate %s): the Check 1c banner above was emitted by the\n' \
        "$dir" >&2
      printf '  SDIST arm, which applies 1c'"'"'s rule to crates whose SOURCES ship in an sdist.\n' >&2
      printf '  Check 1c itself iterates the `publish = true` set and cannot reach this crate\n' >&2
      printf '  (SMA-578 review B2); see the P1 entry in this file'"'"'s header.\n' >&2
      rc=$sub
      [ "$rc" -ne 2 ] || return 2
    fi
  done
  return "$rc"
}

# Check P-D6 — assert wheels.yml never gains registry credentials (SMA-578 D6). It carries
# a pull_request trigger, and same-repo PRs receive repository secrets — moving the upload
# into it, the natural refactor once the artifacts are there, would reopen SMA-407 §7/M2.
# The workflow's own header comment claims this gate asserts it; this function is what makes
# that claim true.
#
# THREE spellings are banned, because the first two alone honoured the header's literal
# wording while leaving its stated RATIONALE unasserted. A normal job needs no `secrets:`
# key at all to read the `secrets` CONTEXT — that key appears only for reusable-workflow
# pass-through — so
#
#     env:
#       MATURIN_PYPI_TOKEN: ${{ secrets.PYPI_API_TOKEN }}
#
# was MEASURED to pass, which is precisely the credential the decision exists to keep out
# (SMA-578 review I2). Hence the third pattern.
#
# None of the patterns is line-anchored, because YAML inline FLOW mappings evade an anchor:
# `permissions: { id-token: write }` and `secrets: inherit` inside a flow mapping were both
# measured green against `(?m)^\s*…` (review Minor 2). repo:actionlint bans inline flow only
# for trigger filters, so nothing else forbids that spelling here.
#
# Dropping the anchor is only safe because comments are stripped FIRST: wheels.yml's own
# header quotes every banned spelling verbatim, so an unanchored match over raw text would
# make the workflow fail on the very comment describing the rule.
assert_wheels_has_no_credentials() { # $1 workflow path
  python3 - "$1" <<'PY'
import re, sys

# (pattern, message). Non-vacuity: an empty table would pass on anything.
PATTERNS = (
    (r'(?:^|[\s{,])id-token\s*:\s*write\b', "declares `id-token: write`"),
    (r'(?:^|[\s{,])secrets\s*:', "declares `secrets:`"),
    (r'\$\{\{\s*secrets\.', "reads the `secrets` context (`${{ secrets.… }}`)"),
)

if not PATTERNS:
    print("FATAL: empty pattern table — this check would pass vacuously", file=sys.stderr)
    raise SystemExit(2)

try:
    text = open(sys.argv[1], encoding="utf-8").read()
except OSError as exc:
    print(f"FATAL: cannot read {sys.argv[1]}: {exc}", file=sys.stderr)
    raise SystemExit(2)


def strip_comments(src):
    """Blank out YAML comments so the unanchored patterns cannot match prose.

    YAML starts a comment at a `#` that is at the start of a line or preceded by
    whitespace, and not inside a quoted scalar. That is exactly the rule applied here —
    without it, wheels.yml's own header (which quotes `secrets:` and `id-token: write` to
    state the ban) would trip the ban it documents.
    """
    out = []
    for line in src.splitlines():
        in_single = in_double = False
        cut = None
        for i, ch in enumerate(line):
            if ch == "'" and not in_double:
                in_single = not in_single
            elif ch == '"' and not in_single:
                in_double = not in_double
            elif ch == "#" and not in_single and not in_double and (i == 0 or line[i - 1] in " \t"):
                cut = i
                break
        out.append(line if cut is None else line[:cut])
    return "\n".join(out)


body = strip_comments(text)
bad = [msg for pattern, msg in PATTERNS if re.search(pattern, body, re.M)]
if bad:
    print("Check P-D6 FAILED: wheels.yml " + " and ".join(bad) +
          " — it is pull_request-triggered, so a same-repo PR would receive the "
          "credential. Publishing belongs in release.yml (SMA-407 §7 review M2).",
          file=sys.stderr)
    raise SystemExit(1)
PY
}

# Discovery + Checks P0/P1/P2, composed. main() and --negative-control both go through THIS
# function rather than each assembling the steps themselves, so a fixture row exercises the
# composition production actually uses — the wiring, not only the verdict (SMA-542).
# Takes the root as an argument. Exit: 0 pass | 1 the repo is wrong | 2 infrastructure.
run_pypi_arm() { # $1 repo root
  local rc=0 line
  local scan
  # Declared and assigned on SEPARATE lines: `local x="$(cmd)"` masks the command's exit
  # status, which would swallow discovery's rc-2 infrastructure signal (same note as main()).
  scan="$(pypi_scan_paths "${1:-}")" || rc=$?
  [ "$rc" -eq 0 ] || return "$rc"

  local -a paths=()
  while IFS= read -r line; do
    [ -n "$line" ] || continue
    paths+=("$line")
  done <<<"$scan"

  assert_pypi_metadata "${paths[@]}"
}

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

  # `workspace = false` is valid TOML and equivalent to omitting the key — it declares no
  # local namespace. Before this row it reached the level checks, found no rust/clippy key,
  # and PASSED. Two shapes, because they fail on different halves of the same rule.
  printf '[package]\nname = "f"\n[lints]\nworkspace = false\n' >"$tmp/lints-ws-false.toml"
  _expect_rc 1 "Check 1c (workspace = false declares no local namespace)" \
    assert_lint_table "$tmp/lints-ws-false.toml"

  printf '[package]\nname = "f"\n[lints]\nworkspace = false\n[lints.rust]\nwarnings = "warn"\n' \
    >"$tmp/lints-ws-false-plus-local.toml"
  _expect_rc 0 "Check 1c (workspace = false WITH a local table — passes)" \
    assert_lint_table "$tmp/lints-ws-false-plus-local.toml"

  # A present-but-EMPTY namespace is the same vacuity in a different shape: it satisfies
  # "has a local table" while setting no lint at all.
  printf '[package]\nname = "f"\n[lints.rust]\n' >"$tmp/lints-empty-ns.toml"
  _expect_rc 1 "Check 1c (present but EMPTY [lints.rust] sets no lint)" \
    assert_lint_table "$tmp/lints-empty-ns.toml"

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

  # The bare wildcard above fails on literal membership. This one SATISFIES membership and
  # must still fail: a catch-all beside the required literals packages everything anyway.
  printf '[package]\nname = "f"\ninclude = ["Cargo.toml", "README.md", "LICENSE", "**/*"]\n' \
    >"$tmp/inc-catchall-plus-literals.toml"
  _expect_rc 1 "Check 1d (catch-all alongside the required literals)" \
    assert_include_allowlist "$tmp/inc-catchall-plus-literals.toml"

  # One row per MEASURED root-wide pattern. Three of these (`/**`, `/*`, `**/`) bypassed the
  # first version of this check, so the set is pinned by fixture rather than by memory.
  for _pat in '/**' '/*' '**/'; do
    printf '[package]\nname = "f"\ninclude = ["README.md", "LICENSE", "%s"]\n' "$_pat" \
      >"$tmp/inc-rootwide.toml"
    _expect_rc 1 "Check 1d (root-wide pattern '$_pat' alongside the literals)" \
      assert_include_allowlist "$tmp/inc-rootwide.toml"
  done

  # A SCOPED glob must still pass — the check rejects catch-alls, not globs.
  printf '[package]\nname = "f"\ninclude = ["src/**/*.rs", "README.md", "LICENSE"]\n' \
    >"$tmp/inc-scoped-glob.toml"
  _expect_rc 0 "Check 1d (scoped glob is not a catch-all — passes)" \
    assert_include_allowlist "$tmp/inc-scoped-glob.toml"

  # --- Check 2c fixtures — the BEHAVIOURAL catch-all detector ----------------------
  # 1d's denylist can never enumerate every glob that matches everything; 2c catches the
  # outcome instead. These sets are synthetic because a git-tracked directory cannot be
  # fixtured cheaply.
  printf 'Cargo.toml\nREADME.md\nLICENSE\nmoon.yml\nsrc/lib.rs\n' >"$tmp/tracked.txt"

  # Packaged == tracked: the include matched everything. Must red whatever it was spelled.
  printf 'Cargo.toml\nREADME.md\nLICENSE\nmoon.yml\nsrc/lib.rs\n' >"$tmp/pkg-everything.txt"
  _expect_rc 1 "Check 2c (packaged set equals the tracked set — a catch-all)" \
    assert_not_catch_all "$tmp/pkg-everything.txt" "$tmp/tracked.txt" f

  # Cargo's own synthesized entries must NOT make the sets differ — otherwise a real
  # catch-all would slip through simply because the tarball carries Cargo.lock.
  printf 'Cargo.lock\nCargo.toml\nCargo.toml.orig\n.cargo_vcs_info.json\nREADME.md\nLICENSE\nmoon.yml\nsrc/lib.rs\n' \
    >"$tmp/pkg-everything-plus-generated.txt"
  _expect_rc 1 "Check 2c (cargo-generated entries do not mask a catch-all)" \
    assert_not_catch_all "$tmp/pkg-everything-plus-generated.txt" "$tmp/tracked.txt" f

  # REGRESSION ROW. The first version of 2c compared the sets for EQUALITY and this shape
  # slipped through: a catch-all that ALSO sweeps files outside the tracked set (measured —
  # a probe under --allow-dirty pulled .git/** in) makes packaged a strict SUPERSET, so the
  # sets are unequal while every tracked file still shipped. The subset test catches it.
  printf 'Cargo.toml\nREADME.md\nLICENSE\nmoon.yml\nsrc/lib.rs\n.git/HEAD\n.git/index\nstray.tmp\n' \
    >"$tmp/pkg-superset.txt"
  _expect_rc 1 "Check 2c (catch-all that also sweeps extras — superset, not equal)" \
    assert_not_catch_all "$tmp/pkg-superset.txt" "$tmp/tracked.txt" f

  # A real allowlist excludes something — here moon.yml. Must pass.
  printf 'Cargo.lock\nCargo.toml\nCargo.toml.orig\nREADME.md\nLICENSE\nsrc/lib.rs\n' \
    >"$tmp/pkg-subset.txt"
  _expect_rc 0 "Check 2c (a real allowlist excludes something — passes)" \
    assert_not_catch_all "$tmp/pkg-subset.txt" "$tmp/tracked.txt" f

  # Non-vacuity: an empty tracked set must be infrastructure, not a silent pass.
  : >"$tmp/tracked-empty.txt"
  _expect_rc 2 "Check 2c (empty tracked set is infra, not a pass)" \
    assert_not_catch_all "$tmp/pkg-subset.txt" "$tmp/tracked-empty.txt" f

  _expect_rc 2 "Check 2c (unreadable listing is infra)" \
    assert_not_catch_all "$tmp/pkg-subset.txt" "$tmp/does-not-exist.txt" f

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


  # --- SMA-578: the PyPI arm must be able to report red -------------------------------
  local pyd="$tmp/py"; mkdir -p "$pyd"

  # A well-formed, PyPI-bound distribution. Rows below mutate a copy of it with sed, so
  # each fixture differs from the passing baseline in exactly the one way it names.
  _pyproj() { # $1 dir
    mkdir -p "$1"
    { printf '[project]\nname = "paigasus-kernel"\nversion = "0.1.0"\n'
      printf 'description = "d"\nreadme = "README.md"\nlicense = "Apache-2.0"\n'
      printf 'license-files = ["LICENSE"]\nauthors = [{ name = "a" }]\n'
      printf 'classifiers = ["Typing :: Typed"]\n'
      printf '\n[tool.paigasus]\npypi = true\n'
    } >"$1/pyproject.toml"
  }
  # A second marked distribution, so the P0 set matches EXPECTED and the rows below fail
  # on the rule they NAME rather than on P0.
  _pybind() { _pyproj "$1"; sed -i.bak 's/paigasus-kernel/paigasus-py-bindings/' "$1/pyproject.toml"; }

  # --- SMA-578 review I1: runtime DISCOVERY of the scan set, driven with a fixture tree ---
  local scanroot="$tmp/scanroot"
  mkdir -p "$scanroot/py/packages/alpha" "$scanroot/py/packages/beta" \
           "$scanroot/rs/crates/bindings/paigasus-py-bindings"
  : >"$scanroot/py/packages/alpha/pyproject.toml"
  : >"$scanroot/py/packages/beta/pyproject.toml"
  : >"$scanroot/rs/crates/bindings/paigasus-py-bindings/pyproject.toml"
  # The two shapes a recursive glob would wrongly sweep in — a uv virtual root one level
  # UP, and a vendored manifest three levels DOWN. Neither may appear in the output.
  : >"$scanroot/py/pyproject.toml"
  mkdir -p "$scanroot/ts/node_modules/.pnpm/node-gyp/gyp"
  : >"$scanroot/ts/node_modules/.pnpm/node-gyp/gyp/pyproject.toml"

  _expect_rc 0 "Check P0 (discovery walks a fixture tree)" \
    pypi_scan_paths "$scanroot"

  local want_scan got_scan
  want_scan="$scanroot/py/packages/alpha/pyproject.toml
$scanroot/py/packages/beta/pyproject.toml
$scanroot/rs/crates/bindings/paigasus-py-bindings/pyproject.toml"
  got_scan="$(pypi_scan_paths "$scanroot")"
  if [ "$got_scan" = "$want_scan" ]; then
    echo "  ok — Check P0 (discovery finds BOTH py packages and the bindings extra, and neither py/pyproject.toml nor the node_modules copy)"
  else
    echo "NEGATIVE CONTROL FAILED: discovery output wrong. want:" >&2
    printf '%s\n' "$want_scan" >&2
    echo "got:" >&2
    printf '%s\n' "$got_scan" >&2
    failures=$((failures + 1))
  fi

  # A NEW package under py/packages is picked up with no edit to this file — the property
  # the hand-maintained list did not have (a new PyPI-bound package passed green, rc 0).
  mkdir -p "$scanroot/py/packages/newpkg"; : >"$scanroot/py/packages/newpkg/pyproject.toml"
  if pypi_scan_paths "$scanroot" | grep -q '/py/packages/newpkg/pyproject.toml$'; then
    echo "  ok — Check P0 (a NEW py/packages member enters the scan set automatically)"
  else
    echo "NEGATIVE CONTROL FAILED: discovery missed a new py/packages member" >&2
    failures=$((failures + 1))
  fi
  rm -rf "$scanroot/py/packages/newpkg"

  # ... and end to end: that new package, marked pypi and thoroughly malformed, must red the
  # arm rather than pass unseen. This is the exact scenario measured green before I1.
  local badroot="$tmp/badroot"
  mkdir -p "$badroot/py/packages/newpkg" "$badroot/rs/crates/bindings/paigasus-py-bindings"
  printf '[project]\nname = "paigasus-newpkg"\nversion = "0.1.0"\nreadme = "README.md"\n' \
    >"$badroot/py/packages/newpkg/pyproject.toml"
  printf 'license = "Apache-2.0"\nlicense-files = ["LICENSE"]\n' \
    >>"$badroot/py/packages/newpkg/pyproject.toml"
  printf 'classifiers = ["License :: OSI Approved :: Apache Software License"]\n' \
    >>"$badroot/py/packages/newpkg/pyproject.toml"
  printf '\n[tool.paigasus]\npypi = true\n' >>"$badroot/py/packages/newpkg/pyproject.toml"
  _pybind "$badroot/rs/crates/bindings/paigasus-py-bindings"
  : >"$badroot/rs/crates/bindings/paigasus-py-bindings/README.md"
  : >"$badroot/rs/crates/bindings/paigasus-py-bindings/LICENSE"
  _expect_rc 1 "Check P0 (an unlisted, malformed PyPI-bound package reds the arm — the I1 regression)" \
    run_pypi_arm "$badroot"

  # Discovery's own non-vacuity guards.
  local emptyroot="$tmp/emptyroot"
  mkdir -p "$emptyroot/py/packages" "$emptyroot/rs/crates/bindings/paigasus-py-bindings"
  : >"$emptyroot/rs/crates/bindings/paigasus-py-bindings/pyproject.toml"
  _expect_rc 2 "Check P0 (the glob matching nothing is INFRA, not a shrunken pass)" \
    pypi_scan_paths "$emptyroot"

  local noextra="$tmp/noextra"; mkdir -p "$noextra/py/packages/alpha"
  : >"$noextra/py/packages/alpha/pyproject.toml"
  _expect_rc 2 "Check P0 (a literal scan extra gone stale is INFRA)" \
    pypi_scan_paths "$noextra"

  _expect_rc 2 "Check P0 (discovery with no root argument is INFRA)" \
    pypi_scan_paths
  _expect_rc 2 "Check P0 (discovery against a non-directory is INFRA)" \
    pypi_scan_paths "$tmp/no-such-root"

  _pyproj "$pyd/ok"; : >"$pyd/ok/README.md"; : >"$pyd/ok/LICENSE"
  _pybind "$pyd/ok2"; : >"$pyd/ok2/README.md"; : >"$pyd/ok2/LICENSE"
  _expect_rc 0 "Check P0/P1/P2 (a well-formed pair passes — not vacuously red)" \
    assert_pypi_metadata "$pyd/ok/pyproject.toml" "$pyd/ok2/pyproject.toml"

  # P0 — one distribution short of EXPECTED_PYPI_PUBLISHABLE.
  _expect_rc 1 "Check P0 (shrunken publishable set)" \
    assert_pypi_metadata "$pyd/ok/pyproject.toml"

  # P1 — a required [project] key is absent.
  _pyproj "$pyd/nodesc"; : >"$pyd/nodesc/README.md"; : >"$pyd/nodesc/LICENSE"
  sed -i.bak '/^description = /d' "$pyd/nodesc/pyproject.toml"
  _pybind "$pyd/nodesc2"; : >"$pyd/nodesc2/README.md"; : >"$pyd/nodesc2/LICENSE"
  _expect_rc 1 "Check P1 (a required [project] field is missing)" \
    assert_pypi_metadata "$pyd/nodesc/pyproject.toml" "$pyd/nodesc2/pyproject.toml"

  # P2 — declared LICENSE does not exist on disk.
  _pyproj "$pyd/nolicfile"; : >"$pyd/nolicfile/README.md"
  _pybind "$pyd/nolic2"; : >"$pyd/nolic2/README.md"; : >"$pyd/nolic2/LICENSE"
  _expect_rc 1 "Check P2 (declared-but-absent LICENSE)" \
    assert_pypi_metadata "$pyd/nolicfile/pyproject.toml" "$pyd/nolic2/pyproject.toml"

  # P1 — SPDX expression AND a License:: trove classifier. The classifier REPLACES the
  # baseline line rather than being appended: a second `classifiers =` key is a TOML
  # duplicate-key error, which this arm reports as rc 2 (infrastructure) — the fixture
  # would then fail before ever reaching the rule it names.
  _pyproj "$pyd/spdxclash"
  sed -i.bak 's|^classifiers = .*|classifiers = ["License :: OSI Approved :: Apache Software License"]|' \
    "$pyd/spdxclash/pyproject.toml"
  : >"$pyd/spdxclash/README.md"; : >"$pyd/spdxclash/LICENSE"
  _pybind "$pyd/spdx2"; : >"$pyd/spdx2/README.md"; : >"$pyd/spdx2/LICENSE"
  _expect_rc 1 "Check P1 (SPDX license alongside a License:: classifier)" \
    assert_pypi_metadata "$pyd/spdxclash/pyproject.toml" "$pyd/spdx2/pyproject.toml"

  # Infrastructure, not assertion: no [project] table at all (py/pyproject.toml's shape).
  printf '[tool.uv.workspace]\nmembers = ["packages/*"]\n' >"$pyd/virtual.toml"
  _expect_rc 2 "Check P0 (a manifest with no [project] table is INFRA, rc 2, not rc 1)" \
    assert_pypi_metadata "$pyd/virtual.toml"

  # Infrastructure: no paths at all. Without this the arm would pass over nothing.
  _expect_rc 2 "Check P0 (an empty scan set is INFRA, rc 2, not a vacuous pass)" \
    assert_pypi_metadata

  # Infrastructure: unparsable TOML.
  printf 'this is not = = toml\n' >"$pyd/broken.toml"
  _expect_rc 2 "Check P0 (unparsable manifest is INFRA, not a repo defect)" \
    assert_pypi_metadata "$pyd/broken.toml"

  # The REAL sdist-shipped crates must satisfy the assertion, invoked exactly as main()
  # invokes it.
  _expect_rc 0 "Check P1 (the real sdist-shipped crates carry their own lint tables)" \
    assert_sdist_lint_tables "${SDIST_SHIPPED_CRATES[@]/#/$REPO_ROOT/}"

  # --- SMA-578 review I3: the sdist wrapper's OWN red path -----------------------------
  # These rows drive assert_sdist_lint_tables, not assert_lint_table, so neutering the
  # wrapper's `|| sub=$?` propagation (measured to survive the previous table green) reds
  # here instead.
  mkdir -p "$tmp/sdist-deny" "$tmp/sdist-ok" "$tmp/sdist-broken"
  printf '[package]\nname = "x"\n\n[lints]\nworkspace = true\n' >"$tmp/sdist-deny/Cargo.toml"
  printf '[package]\nname = "y"\n\n[lints.rust]\nwarnings = "warn"\n' >"$tmp/sdist-ok/Cargo.toml"
  printf 'this is not = = toml\n' >"$tmp/sdist-broken/Cargo.toml"
  _expect_rc 1 "Check P1 (the sdist WRAPPER propagates a denying crate's rc 1)" \
    assert_sdist_lint_tables "$tmp/sdist-deny"
  _expect_rc 1 "Check P1 (the sdist wrapper reds even when a LATER crate is clean)" \
    assert_sdist_lint_tables "$tmp/sdist-deny" "$tmp/sdist-ok"
  _expect_rc 2 "Check P1 (the sdist wrapper propagates rc 2 ahead of rc 1)" \
    assert_sdist_lint_tables "$tmp/sdist-broken"
  _expect_rc 0 "Check P1 (the sdist wrapper accepts a crate with its own warn table)" \
    assert_sdist_lint_tables "$tmp/sdist-ok"
  _expect_rc 2 "Check P1 (the sdist wrapper with no crate dirs is INFRA, not a vacuous pass)" \
    assert_sdist_lint_tables

  # --- SMA-578 review I2 + Minor 2: the credential spellings an anchored regex missed ---
  # The measured bypass: a normal job needs no `secrets:` KEY to read the `secrets` CONTEXT.
  { printf 'on:\n  pull_request:\njobs:\n  a:\n    steps:\n'
    printf '      - name: upload to PyPI\n        env:\n'
    printf '          MATURIN_PYPI_TOKEN: ${{ secrets.PYPI_API_TOKEN }}\n'
    printf '        run: maturin upload dist/*\n'
  } >"$tmp/ctx-wheels.yml"
  _expect_rc 1 "Check P-D6 (a \${{ secrets.… }} context read in a job env)" \
    assert_wheels_has_no_credentials "$tmp/ctx-wheels.yml"

  # Inline FLOW mappings — invisible to a line-anchored pattern.
  printf 'on:\n  pull_request:\njobs:\n  a:\n    permissions: { id-token: write, contents: read }\n' \
    >"$tmp/flow-idtoken.yml"
  _expect_rc 1 "Check P-D6 (id-token: write inside an inline flow mapping)" \
    assert_wheels_has_no_credentials "$tmp/flow-idtoken.yml"
  printf 'on:\n  pull_request:\njobs: { a: { uses: ./.github/workflows/x.yml, secrets: inherit } }\n' \
    >"$tmp/flow-secrets.yml"
  _expect_rc 1 "Check P-D6 (secrets: inherit inside an inline flow mapping)" \
    assert_wheels_has_no_credentials "$tmp/flow-secrets.yml"

  # Comment stripping. Dropping the line anchor is only safe because comments go first —
  # wheels.yml's own header quotes every banned spelling to STATE the ban, so without this
  # the workflow would fail on the comment describing the rule. A quoted `#` must NOT be
  # read as a comment, or the ban could be smuggled past behind one.
  { printf '# this workflow must never declare `secrets:` or `id-token: write`\n'
    printf 'on:\n  pull_request:\njobs:\n  a:\n    name: "sharp # sign"\n'
    printf '    permissions:\n      contents: read\n'
  } >"$tmp/comment-wheels.yml"
  _expect_rc 0 "Check P-D6 (banned spellings quoted in a COMMENT do not trip the ban)" \
    assert_wheels_has_no_credentials "$tmp/comment-wheels.yml"
  printf 'on:\n  pull_request:\njobs:\n  a:\n    name: "x # y"\n    permissions:\n      id-token: write\n' \
    >"$tmp/hash-in-string.yml"
  _expect_rc 1 "Check P-D6 (a # inside a quoted scalar does not blind the scan to what follows)" \
    assert_wheels_has_no_credentials "$tmp/hash-in-string.yml"

  # D6 — wheels.yml must never carry registry credentials.
  printf 'on:\n  pull_request:\njobs:\n  a:\n    permissions:\n      id-token: write\n' \
    >"$tmp/bad-wheels.yml"
  _expect_rc 1 "Check P-D6 (id-token: write in wheels.yml)" \
    assert_wheels_has_no_credentials "$tmp/bad-wheels.yml"
  printf 'on:\n  workflow_call:\n    secrets:\n      PYPI_TOKEN:\n' >"$tmp/secrets-wheels.yml"
  _expect_rc 1 "Check P-D6 (a workflow_call secrets: declaration in wheels.yml)" \
    assert_wheels_has_no_credentials "$tmp/secrets-wheels.yml"
  printf 'on:\n  pull_request:\njobs:\n  a:\n    permissions:\n      contents: read\n' \
    >"$tmp/good-wheels.yml"
  _expect_rc 0 "Check P-D6 (a credential-free wheels.yml passes)" \
    assert_wheels_has_no_credentials "$tmp/good-wheels.yml"
  _expect_rc 2 "Check P-D6 (workflow file unreadable is INFRA)" \
    assert_wheels_has_no_credentials "$tmp/no-such-wheels.yml"
  # The REAL workflow must satisfy the same assertion the fixtures do — this is what makes
  # wheels.yml's own "repo:publish-metadata asserts this" header comment true.
  _expect_rc 0 "Check P-D6 (the real wheels.yml passes)" \
    assert_wheels_has_no_credentials "$REPO_ROOT/.github/workflows/wheels.yml"
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
  # rows silently, and a silent skip reads as a false green. Not a live bug — cargo never
  # touches stdin here, all 3/3 iterations verified — but SMA-577 is what made it matter:
  # it added paigasus-proto and paigasus-proto-derive as the second and third publishable
  # crates (absorbing SMA-388), so this loop now actually runs multiple iterations instead
  # of one.
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


  # --- SMA-578: the PyPI arm. Absolute paths throughout, because main() runs from rs/. -
  status=0; run_pypi_arm "$REPO_ROOT" || status=$?
  [ "$status" -eq 0 ] || exit "$status"
  status=0; assert_sdist_lint_tables "${SDIST_SHIPPED_CRATES[@]/#/$REPO_ROOT/}" || status=$?
  [ "$status" -eq 0 ] || exit "$status"
  status=0; assert_wheels_has_no_credentials "$REPO_ROOT/.github/workflows/wheels.yml" || status=$?
  [ "$status" -eq 0 ] || exit "$status"

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
