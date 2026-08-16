# SMA-376 — `paigasus-kernel` crates.io publish metadata + gate — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `paigasus-kernel` a genuinely publishable crates.io artifact (complete metadata, `publish = true`), and add a CI gate that proves it stays that way and cannot release `0.0.0`.

**Architecture:** Two units. Unit 1 is manifest/doc data — the crate's metadata, a crate-local `README.md` + `LICENSE`, an `include` allowlist, a `[lints.rust]` override, and a `release = false` block in `rs/release-plz.toml`. Unit 2 is one bash gate, `ci/publish-metadata/run.sh`, run as the Moon task `repo:publish-metadata`: it reads `cargo metadata` and `release-plz.toml` (pure checks in an embedded `python3` heredoc) and shells out to `cargo publish --dry-run` / `cargo package --list` (impure checks in bash).

**Tech Stack:** Rust/cargo 1.95 (pinned via `rs/rust-toolchain.toml`), Moon 2.3.2, bash, `python3` (`json` + `tomllib`), release-plz 0.3.158.

**Spec:** `docs/superpowers/specs/2026-08-16-sma-376-kernel-cratesio-publish-design.md` — read it before starting. Decision IDs (D1–D9) referenced below live there.

## Global Constraints

- **Every source file opens with an SPDX header** — `// SPDX-License-Identifier: Apache-2.0` for Rust, `# SPDX-License-Identifier: Apache-2.0` for bash/Python. Markdown docs carry **none**.
- **`version` stays `"0.0.0"`.** Do not bump it. Do not touch `[workspace.dependencies] paigasus-kernel = { path = ..., version = "0.0.0" }` in `rs/Cargo.toml`. (D1)
- **Only `paigasus-kernel` becomes publishable.** Every other crate keeps `publish = false`.
- **All cargo invocations run from `rs/`**, never the repo root. `rs/rust-toolchain.toml` (`channel = "1.95.0"`) and `rs/.cargo/config.toml` are discovered by walking up from **CWD**, not from `--manifest-path`; there is also no repo-root `Cargo.toml`. Every existing cargo gate does this (`moon.yml:219`, `:143`, `:162`).
- **Conventional commits with a workspace scope**, subject **lowercase** and **≤100 chars**. Never put a bare `#NNN` in the body (breaks `footer-leading-blank` in commitlint). Write "owner/repo PR NNN".
- **Do not use `--no-verify`.** The worktree is provisioned; `commitlint` runs and must pass.
- **Bash PATH:** prefix commands with `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` so `moon`/`release-plz` resolve to the repo-pinned versions (shims first).
- **Exit-code contract for the gate:** `0` = pass, `1` = assertion failed (the repo is wrong), `2` = infrastructure failed (cargo/network/parse error). A broken invocation must never read as "all checks passed".
- Work happens in the worktree `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-376` on branch `feature/sma-376-publish-paigasus-kernel`.

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `rs/crates/libs/paigasus-kernel/Cargo.toml` | crates.io identity: metadata, `include` allowlist, lint override, `publish = true` | 1 |
| `rs/crates/libs/paigasus-kernel/README.md` (new) | the crates.io / docs.rs landing page | 1 |
| `rs/crates/libs/paigasus-kernel/LICENSE` (new) | Apache-2.0 text embedded in the artifact | 1 |
| `rs/crates/libs/paigasus-kernel/src/lib.rs` | crate-level rustdoc (currently contradicts the new description) | 1 |
| `rs/release-plz.toml` | the structural release block while the floor is `0.0.0` | 2 |
| `ci/publish-metadata/run.sh` (new) | the whole gate: Checks 0, 1, 2, 2b, 3 | 3 |
| `ci/publish-metadata/run.sh` (`--negative-control`) | proves each check can report red | 4 |
| `moon.yml` | the `repo:publish-metadata` task + its `inputs` | 5 |
| `.github/workflows/ci.yml` | add `:publish-metadata` to the `moon ci` target array | 5 |
| `CLAUDE.md` | document the gate in the full-graph command | 5 |

---

## Task 1: Make the crate publishable

**Files:**
- Modify: `rs/crates/libs/paigasus-kernel/Cargo.toml`
- Modify: `rs/crates/libs/paigasus-kernel/src/lib.rs:1-8`
- Create: `rs/crates/libs/paigasus-kernel/README.md`
- Create: `rs/crates/libs/paigasus-kernel/LICENSE`

**Interfaces:**
- Consumes: nothing.
- Produces: a publishable `paigasus-kernel` — i.e. `cargo metadata` reports `publish: null` for it, and its `description`/`repository`/`readme`/`keywords`/`categories` are non-empty. Task 3's `EXPECTED_PUBLISHABLE=("paigasus-kernel")` depends on this.

- [ ] **Step 1: Write the failing test (run the assertions that will become Checks 1 and 2b)**

There is no test framework for manifest data; the executable assertions are cargo itself. Run both from `rs/`:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs
cargo metadata --format-version 1 --no-deps | python3 -c '
import json,sys
p=[x for x in json.load(sys.stdin)["packages"] if x["name"]=="paigasus-kernel"][0]
print("publish   :", p.get("publish"))
print("description:", p.get("description"))
print("keywords  :", p.get("keywords"), "categories:", p.get("categories"))
'
cargo package --list --no-verify -p paigasus-kernel 2>/dev/null | grep -E 'moon.yml|README.md|LICENSE' || echo "(none of moon.yml/README.md/LICENSE matched as expected-for-now)"
```

Expected **before** the change: `publish: []`, `description: None`, `keywords: []`, `categories: []`, and the listing contains `moon.yml` but neither `README.md` nor `LICENSE`.

- [ ] **Step 2: Replace the `[package]` table**

Replace the whole `[package]` block in `rs/crates/libs/paigasus-kernel/Cargo.toml` (including deleting the two-line `# TODO(SMA-376)` comment) with:

```toml
[package]
name = "paigasus-kernel"
# 0.0.0 is the pre-release stub floor. SMA-407 moves every package to the 0.1.0 floor
# together (crates.io/PyPI/npm in lockstep) and lets release-plz cut the first tag;
# publishing THIS version would permanently burn it on crates.io. `rs/release-plz.toml`
# therefore carries `release = false`, and `moon run repo:publish-metadata` fails if that
# block is removed while a publishable crate is still at 0.0.0.
version = "0.0.0"
description = "Pure-logic behavioral kernel for Paigasus — resource names (PRN), UUIDv7 minting, and Cedar entity UIDs."
repository = "https://github.com/SMK1085/paigasus-core"
homepage = "https://github.com/SMK1085/paigasus-core#readme"
readme = "README.md"
keywords = ["paigasus", "kernel", "prn", "uuid7", "cedar"]
categories = ["data-structures", "parser-implementations"]
# ALLOWLIST, not a denylist. Cargo's default include is "every non-ignored file in the
# package dir", which shipped the monorepo's `moon.yml` to crates.io consumers and would
# ship whatever the dir gains next. Enumerating what belongs is the version that cannot
# leak. The proptest files are kept deliberately: a vendoring consumer can run the suite.
include = ["src/**/*.rs", "tests/**/*.rs", "Cargo.toml", "README.md", "LICENSE"]
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
publish = true
```

- [ ] **Step 3: Override the lint table (do NOT leave `[lints] workspace = true`)**

Replace the existing trailing block:

```toml
[lints]
workspace = true
```

with:

```toml
# NOT `workspace = true`. Cargo INLINES the resolved lint table into the published
# manifest, and docs.rs builds a published crate as the ROOT package on nightly, where
# cargo's `--cap-lints allow` does not apply. Inheriting the workspace's
# `warnings = "deny"` would let the first new rustc warning silently kill docs.rs builds
# of a released crate. CI strictness is unaffected: the Moon `lint` task passes
# `-D warnings` explicitly, which is already how clippy is handled (rs/Cargo.toml:200-201).
[lints.rust]
warnings = "warn"

[lints.clippy]
all = "warn"
```

- [ ] **Step 4: Create `rs/crates/libs/paigasus-kernel/README.md`**

No SPDX header (markdown docs carry none in this repo):

```markdown
# paigasus-kernel

Pure-logic behavioral kernel for Paigasus — the cross-language primitives that must
behave identically everywhere: Paigasus Resource Names (`Prn`), UUIDv7 minting from
injected bytes, and Cedar entity UIDs.

No I/O, no FFI, no adapters. The Python, Node and browser bindings live in
[`rs/crates/bindings/`](https://github.com/SMK1085/paigasus-core/tree/main/rs/crates/bindings)
and call into this crate rather than reimplementing it (ADR-0005).

Licensed under the Apache License, Version 2.0.
```

- [ ] **Step 5: Copy the license into the crate**

A real copy, not a symlink (symlinks interact badly with archive packaging):

```bash
cp LICENSE rs/crates/libs/paigasus-kernel/LICENSE
```

- [ ] **Step 6: Fix the crate-level rustdoc**

`rs/crates/libs/paigasus-kernel/src/lib.rs` currently says *"Empty until real logic lands."* — that becomes the docs.rs landing page and now contradicts the crate description. Replace lines 1–8 (the SPDX header plus the `//!` block, up to and including the blank line before `pub mod cedar;`) with:

```rust
// SPDX-License-Identifier: Apache-2.0

//! Pure-logic behavioral kernel for Paigasus.
//!
//! The cross-language primitives that must behave identically in every runtime:
//! [`Prn`] (Paigasus Resource Names), [`mint_uuid7`] (UUIDv7 from injected bytes — no
//! ambient entropy, so the crate builds for `wasm32-unknown-unknown`), and
//! [`to_cedar_uid`] (Cedar entity UIDs).
//!
//! No I/O, no FFI, and no adapter dependencies live here. The Python, Node and browser
//! bindings under `rs/crates/bindings/` call into this crate rather than reimplementing
//! it (ADR-0005).

```

Do **not** add `#![doc = include_str!("../README.md")]` — the README's H1 would render as a duplicate title in rustdoc.

- [ ] **Step 7: Run the assertions from Step 1 again — they must now pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs
cargo metadata --format-version 1 --no-deps | python3 -c '
import json,sys
p=[x for x in json.load(sys.stdin)["packages"] if x["name"]=="paigasus-kernel"][0]
assert p.get("publish") is None, p.get("publish")
for f in ("description","license","repository","readme"):
    assert (p.get(f) or "").strip(), f
assert 1 <= len(p["keywords"]) <= 5 and 1 <= len(p["categories"]) <= 5
print("metadata OK")
'
cargo package --list --no-verify -p paigasus-kernel --allow-dirty 2>/dev/null > /tmp/kernel-listing.txt
grep -qx 'README.md' /tmp/kernel-listing.txt && echo "README.md packaged"
grep -qx 'LICENSE'   /tmp/kernel-listing.txt && echo "LICENSE packaged"
grep -qx 'moon.yml'  /tmp/kernel-listing.txt && echo "FAIL: moon.yml still packaged" || echo "moon.yml excluded"
```

Expected: `metadata OK`, `README.md packaged`, `LICENSE packaged`, `moon.yml excluded`.

- [ ] **Step 8: Verify the packaged manifest no longer denies warnings**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs
cargo package -p paigasus-kernel --no-verify --allow-dirty >/dev/null 2>&1
grep -A1 '\[lints.rust\]' target/package/paigasus-kernel-0.0.0/Cargo.toml
```

Expected: `warnings = "warn"` (NOT `"deny"`). This is D7 — the docs.rs protection.

- [ ] **Step 9: Verify the crate still builds and its tests pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-kernel-rs:build paigasus-kernel-rs:test paigasus-kernel-rs:lint paigasus-kernel-rs:fmt
```

Expected: all four green. `lint` in particular must still fail on warnings — the `-D warnings` flag comes from the Moon task, not the manifest, so relaxing `[lints.rust]` must not have weakened CI.

- [ ] **Step 10: Commit**

```bash
git add rs/crates/libs/paigasus-kernel/
git commit -F - <<'EOF'
feat(rs): make paigasus-kernel publishable on crates.io (SMA-376)

Add the crates.io metadata the crate has lacked since SMA-357, flip
publish = true, and drop the TODO. The version stays on the 0.0.0 stub
floor: the bump and the crates.io/PyPI/npm lockstep belong to SMA-407.

Package contents move to an include allowlist so the monorepo's moon.yml
stops shipping to consumers, and the lint table is overridden to warn
rather than inherited: cargo inlines it into the published manifest and
docs.rs builds without cap-lints, so an inherited deny would let the
first new rustc warning kill the docs build. CI strictness is unchanged
because the Moon lint task passes -D warnings itself.
EOF
```

---

## Task 2: Block release-plz while the floor is `0.0.0`

**Files:**
- Modify: `rs/release-plz.toml`

**Interfaces:**
- Consumes: Task 1's publishable `paigasus-kernel` at version `0.0.0`.
- Produces: `[workspace] release = false` in `rs/release-plz.toml` — the fact Task 3's Check 3 asserts.

- [ ] **Step 1: Write the failing test**

Check 3's assertion, expressed directly:

```bash
python3 - <<'PY'
import tomllib, sys
with open("rs/release-plz.toml","rb") as f:
    cfg = tomllib.load(f)
blocked = cfg.get("workspace", {}).get("release") is False
per_pkg = {e.get("name") for e in cfg.get("package", []) if e.get("release") is False}
print("workspace release blocked:", blocked, "| per-package blocks:", per_pkg)
sys.exit(0 if blocked or "paigasus-kernel" in per_pkg else 1)
PY
echo "exit=$?"
```

Expected **before** the change: `workspace release blocked: False | per-package blocks: set()`, `exit=1`.

- [ ] **Step 2: Add the release block**

In `rs/release-plz.toml`, insert `release = false` as the first key under `[workspace]`, directly above `features_always_increment_minor`, with this comment:

```toml
[workspace]
# Publishing is BLOCKED while packages sit at the 0.0.0 stub floor: releasing 0.0.0
# would permanently burn that version on crates.io. SMA-407 removes this line as part of
# moving every package to the 0.1.0 floor. `repo:publish-metadata` asserts the pairing —
# this block cannot be removed while a publishable crate is still at 0.0.0.
release = false
# Conventional-Commit -> semver classification (the contract SMA-398 asserts).
```

Leave the existing `features_always_increment_minor` / `dependencies_update` lines and the `[changelog]` table untouched.

- [ ] **Step 3: Run the test again — it must pass**

Re-run the Step 1 snippet. Expected: `workspace release blocked: True`, `exit=0`.

- [ ] **Step 4: Verify release-plz still accepts the config**

An unknown key would be rejected at config-load time, so a successful load is meaningful:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
release-plz update --config rs/release-plz.toml --manifest-path rs/Cargo.toml 2>&1 | head -3
```

Expected: the first line is `INFO using release-plz config file rs/release-plz.toml` (the command then does real work against the repo — that is fine, `update` only rewrites versions when a release is due, and `release = false` now prevents that; if it reports it would change files, **revert any manifest edit it made** with `git checkout -- rs/` and record that in the task notes).

- [ ] **Step 5: Verify the SMA-398 parity harness is unaffected**

It derives its fixture by grepping the single `features_always_increment_minor` line, so an added key should be inert. Confirm rather than assume:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:release-parity
```

Expected: green.

- [ ] **Step 6: Commit**

```bash
git add rs/release-plz.toml
git commit -F - <<'EOF'
feat(rs): block release-plz releases while packages sit at the 0.0.0 floor (SMA-376)

paigasus-kernel is now publishable, so a live release path would release
0.0.0 and permanently burn that version on crates.io. release = false is
the structural block release-plz honors however it is invoked — an action,
the pinned binary, or a script — unlike grepping workflows for a publish
command. SMA-407 removes it when it moves the floor to 0.1.0.
EOF
```

---

## Task 3: The gate script

**Files:**
- Create: `ci/publish-metadata/run.sh`

**Interfaces:**
- Consumes: Task 1's publishable crate, Task 2's release block.
- Produces: an executable gate. Exit `0` pass, `1` assertion failure, `2` infrastructure failure. Task 4 adds a `--negative-control` argument to this same file; Task 5 wires it as `repo:publish-metadata`.

- [ ] **Step 1: Write the script**

Create `ci/publish-metadata/run.sh` with exactly this content:

```bash
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
            # A [[package]] entry OVERRIDES [workspace] for that package, so the effective
            # value is the package's when present. Anything other than an explicit False —
            # including release = true and an unset value — leaves the crate releasable.
            effective = package_release.get(name, workspace_release)
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
```

- [ ] **Step 2: Make it executable and run it — it must pass**

```bash
chmod +x ci/publish-metadata/run.sh
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/publish-metadata/run.sh; echo "exit=$?"
```

Expected: `publish-metadata: paigasus-kernel OK`, `publish-metadata: all checks passed`, `exit=0`.

- [ ] **Step 3: Prove Check 0 catches the vacuity trap**

This is the most important assertion in the plan — without it the whole gate can be disabled by one line.

Use this backup/restore idiom for **every** deliberate-break step below. Restoring a file
rolls its mtime *backwards*, so cargo reuses output built from the temporary edit — hence
the `touch`. Do not use `mv file.bak file` for the same reason.

```bash
cp rs/crates/libs/paigasus-kernel/Cargo.toml /tmp/kernel-cargo.bak
sed -i '' 's/^publish = true$/publish = false/' rs/crates/libs/paigasus-kernel/Cargo.toml
bash ci/publish-metadata/run.sh; echo "exit=$?"
cp /tmp/kernel-cargo.bak rs/crates/libs/paigasus-kernel/Cargo.toml
touch rs/crates/libs/paigasus-kernel/Cargo.toml
```

Expected: `FATAL: no publishable crate found ...`, `exit=2`.

(`sed -i ''` is the BSD/macOS form. On Linux use `sed -i`.)

- [ ] **Step 4: Prove Check 1 catches emptied metadata**

```bash
cp rs/crates/libs/paigasus-kernel/Cargo.toml /tmp/kernel-cargo.bak
sed -i '' 's/^description = .*/description = ""/' rs/crates/libs/paigasus-kernel/Cargo.toml
bash ci/publish-metadata/run.sh; echo "exit=$?"
cp /tmp/kernel-cargo.bak rs/crates/libs/paigasus-kernel/Cargo.toml
touch rs/crates/libs/paigasus-kernel/Cargo.toml
```

Expected: `paigasus-kernel: \`description\` is missing or empty`, `exit=1`.

Repeat the same edit/run/restore cycle for each of these, confirming `exit=1` every time:
- add a sixth keyword → `6 keywords (crates.io max 5)`
- change a keyword to `"paigasuspaigasuspaigasus"` (24 chars) → `is 24 chars (crates.io max 20)`
- change a keyword to `"-leading-hyphen"` → `must start alphanumeric`

- [ ] **Step 5: Prove Check 2b catches a leaking artifact**

```bash
cp rs/crates/libs/paigasus-kernel/Cargo.toml /tmp/kernel-cargo.bak
sed -i '' 's|^include = .*|include = ["src/**/*.rs", "tests/**/*.rs", "Cargo.toml", "README.md"]|' rs/crates/libs/paigasus-kernel/Cargo.toml
bash ci/publish-metadata/run.sh; echo "exit=$?"
cp /tmp/kernel-cargo.bak rs/crates/libs/paigasus-kernel/Cargo.toml
touch rs/crates/libs/paigasus-kernel/Cargo.toml
```

Expected: `Check 2b FAILED: paigasus-kernel does not package LICENSE`, `exit=1`.

- [ ] **Step 6: Prove Check 3 catches an unblocked `0.0.0`**

```bash
cp rs/release-plz.toml /tmp/release-plz.bak
sed -i '' '/^release = false$/d' rs/release-plz.toml
bash ci/publish-metadata/run.sh; echo "exit=$?"
cp /tmp/release-plz.bak rs/release-plz.toml
```

Expected: `paigasus-kernel: publishable at 0.0.0 but rs/release-plz.toml does not block its release`, `exit=1`.

- [ ] **Step 7: Confirm the tree is clean and the gate is green again**

```bash
git status --short          # must show ONLY ci/publish-metadata/run.sh as untracked
bash ci/publish-metadata/run.sh; echo "exit=$?"
```

Expected: no modified files, `exit=0`. If any manifest is still modified, the restores in Steps 3–6 failed — fix before committing.

- [ ] **Step 8: Confirm the toolchain provenance**

```bash
cd rs && cargo --version && cd ..
```

Expected: `cargo 1.95.0` (the pinned toolchain), proving the `cd "$RS_DIR"` in `main()` does its job.

- [ ] **Step 9: Commit**

```bash
git add ci/publish-metadata/run.sh
git commit -F - <<'EOF'
feat(ci): gate crates.io publishability for every publishable crate (SMA-376)

Assert the metadata crates.io validates at upload time, that each crate
packages exactly the intended files, and that a 0.0.0 crate cannot be
released by release-plz.

The publishable set is compared for strict equality against an expected
list rather than merely iterated: it is discovered from the same publish
flag the gate protects, so flipping that flag back would otherwise turn
every check green over an empty set. cargo runs from rs/ so it picks up
the pinned toolchain, --allow-dirty is applied only outside CI because it
changes what gets packaged, and infrastructure failures exit 2 so a broken
invocation can never read as a pass.
EOF
```

---

## Task 4: `--negative-control` mode

**Files:**
- Modify: `ci/publish-metadata/run.sh`

**Interfaces:**
- Consumes: Task 3's `metadata_checks` and `assert_package_list` functions.
- Produces: `bash ci/publish-metadata/run.sh --negative-control` — exits `0` when every check reports red on a deliberately broken fixture, `1` if any check fails to.

Task 3's Steps 3–6 proved the checks bite *by hand*. This makes that proof runnable, matching `ci/affected-graph/run.sh` and `ci/release-parity/run.sh`, which both ship a negative-control mode.

- [ ] **Step 1: Write the failing test**

```bash
bash ci/publish-metadata/run.sh --negative-control; echo "exit=$?"
```

Expected **before** the change: the flag is ignored, the normal run happens, `exit=0` — i.e. the mode does not exist.

- [ ] **Step 2: Add the negative-control function**

Insert this function immediately above `main()` in `ci/publish-metadata/run.sh`:

```bash
# --negative-control — drive the SAME check code with deliberately broken fixtures and
# assert each reports red. Without this, a refactor can quietly turn the gate vacuous and
# every CI run stays green. Uses fixtures rather than mutating the repo, so it is fast,
# deterministic, and cannot leave the tree dirty.
negative_control() {
  local tmp failures=0
  tmp="$(mktemp -d)"

  _meta() { # $1 out-file, $2 JSON object for the single package
    python3 - "$1" "$2" <<'PY'
import json, sys
pkg = json.loads(sys.argv[2])
with open(sys.argv[1], "w", encoding="utf-8") as fh:
    json.dump({"packages": [pkg]}, fh)
PY
  }

  _expect_red() { # $1 label, rest = command
    local label="$1"; shift
    if "$@" >/dev/null 2>&1; then
      echo "NEGATIVE CONTROL FAILED: $label did not report red" >&2
      failures=$((failures + 1))
    else
      echo "  ok — $label reports red"
    fi
  }

  local good_rp="$tmp/good-release-plz.toml" bad_rp="$tmp/bad-release-plz.toml"
  printf '[workspace]\nrelease = false\n' >"$good_rp"
  printf '[workspace]\n' >"$bad_rp"

  local base='{"name":"paigasus-kernel","version":"0.0.0","publish":null,
    "manifest_path":"/nowhere/Cargo.toml","description":"d","license":"Apache-2.0",
    "repository":"r","readme":"README.md","keywords":["k"],"categories":["c"]}'

  # Check 0 — empty publishable set.
  printf '{"packages":[{"name":"x","version":"0.0.0","publish":[],"manifest_path":"/x"}]}' \
    >"$tmp/empty.json"
  _expect_red "Check 0 (empty publishable set)" \
    metadata_checks "$tmp/empty.json" "$good_rp" "paigasus-kernel"

  # Check 0 — set differs from expected.
  _meta "$tmp/wrong-name.json" "$(printf '%s' "$base" | sed 's/paigasus-kernel/some-other-crate/')"
  _expect_red "Check 0 (unexpected publishable crate)" \
    metadata_checks "$tmp/wrong-name.json" "$good_rp" "paigasus-kernel"

  # Check 1 — each rule, one fixture apiece.
  _meta "$tmp/no-desc.json" "$(printf '%s' "$base" | sed 's/"description":"d"/"description":""/')"
  _expect_red "Check 1 (empty description)" \
    metadata_checks "$tmp/no-desc.json" "$good_rp" "paigasus-kernel"

  _meta "$tmp/six-kw.json" "$(printf '%s' "$base" | sed 's/"keywords":\["k"\]/"keywords":["a","b","c","d","e","f"]/')"
  _expect_red "Check 1 (six keywords)" \
    metadata_checks "$tmp/six-kw.json" "$good_rp" "paigasus-kernel"

  _meta "$tmp/long-kw.json" "$(printf '%s' "$base" | sed 's/"keywords":\["k"\]/"keywords":["aaaaaaaaaaaaaaaaaaaaa"]/')"
  _expect_red "Check 1 (21-char keyword)" \
    metadata_checks "$tmp/long-kw.json" "$good_rp" "paigasus-kernel"

  _meta "$tmp/bad-kw.json" "$(printf '%s' "$base" | sed 's/"keywords":\["k"\]/"keywords":["-nope"]/')"
  _expect_red "Check 1 (keyword with a leading hyphen)" \
    metadata_checks "$tmp/bad-kw.json" "$good_rp" "paigasus-kernel"

  _meta "$tmp/no-cat.json" "$(printf '%s' "$base" | sed 's/"categories":\["c"\]/"categories":[]/')"
  _expect_red "Check 1 (no categories)" \
    metadata_checks "$tmp/no-cat.json" "$good_rp" "paigasus-kernel"

  # Check 3 — a 0.0.0 crate with no release block.
  _meta "$tmp/stub.json" "$base"
  _expect_red "Check 3 (0.0.0 crate not release-blocked)" \
    metadata_checks "$tmp/stub.json" "$bad_rp" "paigasus-kernel"

  # The per-package override hole: [[package]] beats [workspace], so a `release = true`
  # entry leaves the crate releasable even with the workspace block in place. This is the
  # edit a maintainer makes when activating release-plz for one crate.
  local override_rp="$tmp/override-release-plz.toml"
  printf '[workspace]\nrelease = false\n\n[[package]]\nname = "paigasus-kernel"\nrelease = true\n' >"$override_rp"
  _expect_red "Check 3 (per-package release = true override)" \
    metadata_checks "$tmp/stub.json" "$override_rp" "paigasus-kernel"

  # Check 2b — a listing missing LICENSE, and one containing moon.yml.
  printf 'Cargo.toml\nREADME.md\nsrc/lib.rs\n' >"$tmp/missing-license.txt"
  _expect_red "Check 2b (LICENSE not packaged)" \
    assert_package_list "$tmp/missing-license.txt" "fixture"

  printf 'Cargo.toml\nREADME.md\nLICENSE\nmoon.yml\n' >"$tmp/leaks-moon.txt"
  _expect_red "Check 2b (moon.yml packaged)" \
    assert_package_list "$tmp/leaks-moon.txt" "fixture"

  # Positive control: a clean fixture must pass, or every "red" above is meaningless.
  _meta "$tmp/good.json" "$(printf '%s' "$base" | sed 's/"version":"0.0.0"/"version":"0.1.0"/')"
  if ! metadata_checks "$tmp/good.json" "$bad_rp" "paigasus-kernel" >/dev/null 2>&1; then
    echo "NEGATIVE CONTROL FAILED: the clean fixture did not pass — the checks reject everything" >&2
    failures=$((failures + 1))
  else
    echo "  ok — clean fixture passes (checks are not vacuously red)"
  fi

  rm -rf "$tmp"
  if [ "$failures" -gt 0 ]; then
    echo "negative control: $failures check(s) failed to bite" >&2
    return 1
  fi
  echo "negative control: every check reports red on a broken fixture"
}
```

- [ ] **Step 3: Dispatch on the flag**

Change the last line of the script from `main "$@"` to:

```bash
if [ "${1:-}" = "--negative-control" ]; then
  negative_control
else
  main "$@"
fi
```

- [ ] **Step 4: Run the negative control — it must pass**

```bash
bash ci/publish-metadata/run.sh --negative-control; echo "exit=$?"
```

Expected: an `ok — ...` line for every fixture, including the positive control, then `negative control: every check reports red on a broken fixture` and `exit=0`.

- [ ] **Step 5: Prove the negative control itself can fail**

Temporarily neuter one rule so a fixture stops being caught:

```bash
cp ci/publish-metadata/run.sh /tmp/run.sh.bak
sed -i '' 's/if len(keywords) > 5:/if len(keywords) > 500:/' ci/publish-metadata/run.sh
bash ci/publish-metadata/run.sh --negative-control; echo "exit=$?"
cp /tmp/run.sh.bak ci/publish-metadata/run.sh
```

Expected: `NEGATIVE CONTROL FAILED: Check 1 (six keywords) did not report red`, `exit=1`. If it still exits 0, the negative control is itself vacuous — fix it before moving on.

- [ ] **Step 6: Confirm the normal run still passes**

```bash
bash ci/publish-metadata/run.sh; echo "exit=$?"
git status --short   # must show no modified files
```

Expected: `exit=0`, clean tree.

- [ ] **Step 7: Commit**

```bash
git add ci/publish-metadata/run.sh
git commit -F - <<'EOF'
feat(ci): add a negative-control mode to the publish-metadata gate (SMA-376)

Drive the same check code with deliberately broken fixtures and assert
each one reports red, plus a positive control so the checks cannot pass
by rejecting everything. Fixtures rather than repo mutation keeps it fast
and unable to leave the tree dirty.
EOF
```

---

## Task 5: Wire the gate into Moon, CI and the docs

**Files:**
- Modify: `moon.yml` (append after the `promtool` task, the current last task)
- Modify: `.github/workflows/ci.yml:184`
- Modify: `CLAUDE.md:64-68`

**Interfaces:**
- Consumes: `ci/publish-metadata/run.sh` from Tasks 3–4.
- Produces: the Moon target `:publish-metadata`, runnable as `moon run repo:publish-metadata` and selected by `moon ci`.

- [ ] **Step 1: Write the failing test**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:publish-metadata; echo "exit=$?"
```

Expected **before** the change: Moon errors that the task does not exist.

- [ ] **Step 2: Add the Moon task**

Append to the end of `moon.yml` (after the `promtool` task's `inputs` block), keeping the file's two-space task indentation:

```yaml
  publish-metadata:
    description: 'Assert every publishable crate carries crates.io-valid metadata, packages exactly the intended files, and cannot be released by release-plz while still at the 0.0.0 stub floor (SMA-376).'
    script: 'bash ci/publish-metadata/run.sh'
    toolchain: 'system'
    # DELIBERATELY BROAD on rs/crates/**: the publishable set is discovered at RUNTIME from
    # `cargo metadata`, so per-crate globs would go stale the day a new crate flips
    # publish = true and Moon would serve a cached pass over sources it never tracked. A
    # vacuous gate is worse than a slow one. The rest of the list is every input that
    # DETERMINES the answer: the toolchain pin and cargo config (they drive the verify
    # build), release-plz.toml (Check 3), and .gitignore (it drives cargo's file
    # enumeration). Omitting those is the exact staleness ci/next-env/run.sh documents.
    inputs:
      - 'ci/publish-metadata/run.sh'
      - 'rs/Cargo.toml'
      - 'rs/Cargo.lock'
      - 'rs/crates/**/*'
      - 'rs/rust-toolchain.toml'
      - 'rs/.cargo/config.toml'
      - 'rs/release-plz.toml'
      - '.gitignore'
```

- [ ] **Step 3: Run it through Moon — it must pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:publish-metadata; echo "exit=$?"
```

Expected: `exit=0`. If Moon reports the task as skipped/cached on a first run, that is a red flag — re-run with `--force` and confirm it actually executes.

- [ ] **Step 4: Measure the cold cost (the spec requires this number)**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
rm -rf rs/target/package
time moon run repo:publish-metadata --force
```

Record the wall-clock time. Write it into the spec's *Cost is not yet measured* paragraph, replacing that paragraph's "Measure it on the implementation PR and record the number." sentence with the measured figure and the conditions (cold `rs/target/package`, local machine). Note in the same edit that the CI figure may differ.

- [ ] **Step 5: Add the target to the CI array**

In `.github/workflows/ci.yml`, line 184, append ` :publish-metadata` to the end of the `T=(...)` array, immediately after `:release-parity-ts`. The array must stay on one line.

- [ ] **Step 6: Add the target to the CLAUDE.md full-graph command**

In `CLAUDE.md`, the *Gotchas* bullet at lines 64–68, append `:publish-metadata` to the documented command so the pre-push instruction stays complete — put it after `:release-parity-ts` and before `--base origin/main`.

- [ ] **Step 7: Verify the wiring is consistent**

The two lists must not drift:

```bash
grep -o ':publish-metadata' .github/workflows/ci.yml CLAUDE.md
```

Expected: one hit in each file.

- [ ] **Step 8: Commit**

```bash
git add moon.yml .github/workflows/ci.yml CLAUDE.md docs/superpowers/specs/2026-08-16-sma-376-kernel-cratesio-publish-design.md
git commit -F - <<'EOF'
feat(ci): run the publish-metadata gate in the moon ci graph (SMA-376)

Register repo:publish-metadata and add it to both the CI target array and
the documented full-graph command, so the gate cannot exist without
running. Inputs stay broad over rs/crates because the publishable set is
discovered at runtime, and they now include the toolchain pin, cargo
config, release-plz config and gitignore, each of which determines the
result.
EOF
```

---

## Task 6: Full-graph verification and issue bookkeeping

**Files:**
- Modify: `docs/superpowers/specs/2026-08-16-sma-376-kernel-cratesio-publish-design.md` (only if verification surfaces a correction)
- No code changes expected.

**Interfaces:**
- Consumes: everything from Tasks 1–5.
- Produces: evidence that the whole graph is green, and Linear reflecting the re-scoped ACs.

- [ ] **Step 1: Rebase onto `origin/main`**

A peer session is landing SMA-438 (`paigasus-proto-derive`), which adds a crate to `rs/Cargo.toml`. The gate's Check 0 compares the publishable set for strict equality, so a new crate must be confirmed `publish = false`:

```bash
git fetch origin
git rebase origin/main
```

Resolve any conflict in `rs/Cargo.toml` by keeping **both** sides' workspace dependency entries.

- [ ] **Step 2: Re-run the gate after the rebase**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/publish-metadata/run.sh; echo "exit=$?"
```

Expected: `exit=0`. If Check 0 now fails naming a new crate, that crate arrived publishable — verify whether that is intended before adding it to `EXPECTED_PUBLISHABLE`.

- [ ] **Step 3: Run the full CI graph**

Per-project Moon tasks do not run the repo-level gates, and this change touches a workspace manifest, `moon.yml` and `release-plz.toml`:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :promtool :observability-drift :nats-permissions \
  :release-parity :release-parity-py :release-parity-ts :publish-metadata \
  --base origin/main --include-relations
```

Expected: green. If Moon reports an unattributed failure, diagnose it with
`jq '.actions[]|select(.status=="failed")' .moon/cache/ciReport.json` — note the repo has no `jq`, so use:
`python3 -c 'import json;print([a for a in json.load(open(".moon/cache/ciReport.json"))["actions"] if a["status"]=="failed"])'`

Pay particular attention to `:release-parity` (the `release-plz.toml` edit) and `:affected-smoke` (the spec records that `repo` tasks are filtered out of its expected sets, so it should be unaffected — confirm rather than assume).

- [ ] **Step 4: Re-run the negative control one final time**

```bash
bash ci/publish-metadata/run.sh --negative-control; echo "exit=$?"
```

Expected: `exit=0` with every `ok — ...` line present.

- [ ] **Step 5: Confirm the Definition of Done**

Walk the spec's *Definition of done* list and tick each item against the actual tree. Any item that cannot be ticked is a blocker — report it rather than proceeding.

- [ ] **Step 6: Update Linear (a deliverable, per D8)**

SMA-376's ACs say "choose a real 0.x version" and "set up release tooling (release-plz) / crates.io publishing". Neither is delivered here; both are SMA-407's items 1 and 3 verbatim. Edit the SMA-376 issue description to strike those two bullets and note they are tracked by SMA-407, so the issue cannot close with ACs silently unmet. Add a comment on SMA-388 noting that flipping `paigasus-proto` to `publish = true` will red `Check 0` until the crate is added to `EXPECTED_PUBLISHABLE`, and will then require the same metadata fields.

- [ ] **Step 7: Commit any spec corrections**

Only if Steps 1–5 surfaced a correction to the spec:

```bash
git add docs/superpowers/specs/2026-08-16-sma-376-kernel-cratesio-publish-design.md
git commit -F - <<'EOF'
docs(rs): record the verified publish-metadata gate results (SMA-376)

Fold the measured cost and any correction surfaced by the full-graph run
back into the design doc.
EOF
```

---

## Self-Review

**Spec coverage.** Every *Definition of done* item maps to a task: manifest fields, `include`, `[lints.rust]`, `publish = true`, TODO removal → Task 1 Steps 2–3; rustdoc → Task 1 Step 6; README + LICENSE + packaged contents → Task 1 Steps 4–5, 7; `release = false` → Task 2; `run.sh` with Checks 0–3, exit-code separation and SPDX → Task 3; `--negative-control` → Task 4; Moon task + `ci.yml` + `CLAUDE.md` → Task 5; deliberate-break checks → Task 3 Steps 3–6 (each spec-listed break is covered, and `readme = "NOPE.md"` is covered by Task 1 Step 7's `cargo package --list`, which errors on a missing readme); measured cost → Task 5 Step 4; Linear ACs → Task 6 Step 6; full graph → Task 6 Step 3.

**Placeholder scan.** No TBD/TODO/"implement later"/"similar to Task N". Every code step carries the literal content. The one deliberate `TODO(SMA-376)` reference is an instruction to *delete* an existing comment.

**Type consistency.** `metadata_checks` (3 args: metadata json, release-plz toml, comma-separated expected) and `assert_package_list` (2 args: listing file, package name) are defined in Task 3 and called with those exact signatures in Task 4's negative control. `EXPECTED_PUBLISHABLE`, `REQUIRED_PACKAGED`, `FORBIDDEN_PACKAGED`, `REPO_ROOT`, `RS_DIR` are declared once in Task 3 and referenced consistently. The tab-separated `name\tmanifest-dir` contract printed by `metadata_checks` matches the `while IFS=$'\t' read -r name dir` consumer in `main`.

**Known trap flagged inline.** Task 3 Step 3 restores a `.bak` file, which rolls mtime *backwards* and makes cargo reuse output built from the temporary edit — hence the `touch` after every restore. The same `touch` appears in Steps 4 and 5.
