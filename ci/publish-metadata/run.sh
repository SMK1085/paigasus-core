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

# The ONE maintained fact in this script. SMA-388 adds paigasus-proto here.
EXPECTED_PUBLISHABLE=("paigasus-kernel")

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
  status=0
  assert_package_list "$listing" "$pkg" || status=$?
  if [ "$status" -ne 0 ]; then
    rm -f "$out" "$listing"
    exit "$status"
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
  rm -f "$meta_json"
  [ "$status" -eq 0 ] || exit "$status"

  local name dir
  # Read on FD 3, not stdin (the ci/release-parity/run.sh idiom): a loop-body subprocess
  # that reads stdin can swallow rows silently, and a silent skip reads as a false green.
  # Not a live bug today — cargo never touches stdin here, all 3/3 iterations verified —
  # but it becomes one the day SMA-388 adds a second publishable crate.
  while IFS=$'\t' read -r -u 3 name dir; do
    [ -n "$name" ] || continue
    check_package "$name" "$dir"
  done 3<<<"$publishable"

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
