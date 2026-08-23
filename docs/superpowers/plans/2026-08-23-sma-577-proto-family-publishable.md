# SMA-577 — Proto family publishable: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `paigasus-proto` and `paigasus-proto-derive` genuinely publishable to crates.io at `0.1.0`, and teach `repo:publish-metadata` to express the `derive → proto` publish order.

**Architecture:** Two new manifest-reading checks (1c lint table, 1d `include` allowlist) enforce the "any crate flipping `publish = true`" rule. Check 2 stops being per-package and becomes one `cargo publish --dry-run` per **publish group** — a connected component of the in-set dependency graph — so `paigasus-kernel` keeps its registry-faithful standalone assertion while the proto pair is verified together. `repo:version-lockstep` gains a per-(group, kind) lock membership table because its two lock readers span two naming namespaces.

**Tech Stack:** Bash + `python3` (`tomllib`, `json`) gate scripts; Cargo 1.95.0; release-plz 0.3.158; Moon 2.3.2; `uv`.

**Spec:** `docs/superpowers/specs/2026-08-23-sma-577-proto-family-publishable-design.md`

## Global Constraints

- **PATH:** every command prefix — `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`. Shims FIRST.
- **Working directory:** `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-577`. Do NOT `cd` to the main checkout. All cargo commands run from `rs/`.
- **Branch:** `feature/sma-577-release-activation-b-make-the-proto-family-publishable`. Never `git checkout` another branch; a peer session shares this checkout's `.git`.
- **Never bypass hooks.** No `--no-verify`. The worktree is already provisioned.
- **Commit subjects start lowercase, ≤100 chars.** Never put a bare `#NNN` or a `token: value` line in the commit BODY — commitlint fails `footer-leading-blank`. Write "PR NNN", not "#NNN".
- **Gate exit-code contract (both scripts):** `0` pass, `1` the repo is wrong, `2` infrastructure failed. A broken invocation must never read as "all checks passed".
- **`EXPECTED_PUBLISHABLE` final value:** `("paigasus-kernel" "paigasus-proto" "paigasus-proto-derive")` — exactly three.
- **Version floor:** `0.1.0` for both proto crates.
- **`EXPECTED_SITE_COUNT` final value:** `20`. **`SELF_TEST_COUNT` final value:** `2`.
- **SPDX:** every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (`#` for Python). `README.md` / `LICENSE` do NOT get one — `paigasus-kernel`'s do not.
- **Do not hand-edit `.github/CODEOWNERS`** (Moon-generated).
- **Atomic groups (spec §8):** Task 6 must land as ONE commit — `publish = true`, `EXPECTED_PUBLISHABLE`, the `0.1.0` bumps, both relocked lockfiles, and `release-plz.toml` are mutually entangled and each half alone reds the gate.

---

### Task 1: Check 1c — per-crate lint table, and no `deny`

**Files:**
- Modify: `ci/publish-metadata/run.sh` (add `assert_lint_table`; call it from `main`; add fixture rows to `negative_control`)

**Interfaces:**
- Produces: `assert_lint_table <manifest-path>` → rc `0` pass / `1` repo wrong / `2` infra. Task 2 mirrors its shape; Task 3's `main()` loop calls both.

**Why standalone, not inside `metadata_checks`:** the negative-control fixtures share a `base` package object whose `manifest_path` is `/nowhere/Cargo.toml` (`run.sh:443-446`). Roughly 22 rows derive from it, including the positive control. Manifest-reading logic inside `metadata_checks` would make every one of them fail on a missing file instead of on the rule it names.

- [ ] **Step 1: Add the function**

Insert directly after `assert_package_list`'s closing `}` (around line 264):

```bash
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
```

- [ ] **Step 2: Call it from `main`**

In `main()`, the loop currently reads:

```bash
  while IFS=$'\t' read -r -u 3 name dir; do
    [ -n "$name" ] || continue
    check_package "$name" "$dir"
  done 3<<<"$publishable"
```

Change the body to run 1c first (cheap, no compile) — replace `check_package "$name" "$dir"` with:

```bash
    status=0; assert_lint_table "$dir/Cargo.toml" || status=$?
    [ "$status" -eq 0 ] || exit "$status"
    check_package "$name" "$dir"
```

and add `local status` to `main`'s existing declarations if not already in scope (it is: `local status=0` is declared earlier in `main`).

- [ ] **Step 3: Add the negative-control fixture rows**

In `negative_control()`, after the last existing Check-1 row and before the Check 2b rows, insert:

```bash
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
```

- [ ] **Step 4: Run the negative control — the new rows must report red**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-577
bash ci/publish-metadata/run.sh --negative-control
```

Expected: exit 0, with seven new `ok — Check 1c (…)` lines and **no** `NEGATIVE CONTROL FAILED`.

- [ ] **Step 5: Run the real gate — `paigasus-kernel` must still pass**

```bash
bash ci/publish-metadata/run.sh
```

Expected: exit 0, ending `publish-metadata: all checks passed`. `paigasus-kernel` already declares `[lints.rust] warnings = "warn"` and `[lints.clippy] all = "warn"`, so 1c passes without touching it. If this reds, STOP — do not "fix" the kernel manifest.

- [ ] **Step 6: Commit**

```bash
git add ci/publish-metadata/run.sh
git commit -m "feat(repo): assert publishable crates carry their own non-denying lint table"
```

---

### Task 2: Check 1d — `include` allowlist

**Files:**
- Modify: `ci/publish-metadata/run.sh` (add `assert_include_allowlist`; call from `main`; fixtures)

**Interfaces:**
- Consumes: Task 1's `main()` loop shape.
- Produces: `assert_include_allowlist <manifest-path>` → rc `0`/`1`/`2`.

**Semantics, fixed (spec §5.6):** `include` present, a **list**, non-empty; `include.workspace = true` rejected explicitly (it parses as a truthy non-empty dict and would pass a naive check vacuously); every entry a plain string; membership **literal** — the list must contain the exact strings `README.md` and `LICENSE`. Literal beats glob-aware: it is far less code, and glob matching would accept `include = ["**/*"]`, which "covers" both files while reinstating the `moon.yml` leak Check 2b exists to catch.

- [ ] **Step 1: Add the function**

Insert directly after `assert_lint_table`'s closing `}`:

```bash
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
```

- [ ] **Step 2: Call it from `main`**

Extend the loop body added in Task 1, so it reads:

```bash
    status=0; assert_lint_table "$dir/Cargo.toml" || status=$?
    [ "$status" -eq 0 ] || exit "$status"
    status=0; assert_include_allowlist "$dir/Cargo.toml" || status=$?
    [ "$status" -eq 0 ] || exit "$status"
    check_package "$name" "$dir"
```

- [ ] **Step 3: Add the negative-control fixture rows**

Immediately after Task 1's fixture block:

```bash
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
```

- [ ] **Step 4: Run the negative control**

```bash
bash ci/publish-metadata/run.sh --negative-control
```

Expected: exit 0, seven new `ok — Check 1d (…)` lines, no failures.

- [ ] **Step 5: Run the real gate**

```bash
bash ci/publish-metadata/run.sh
```

Expected: exit 0. `paigasus-kernel`'s `include` already lists `README.md` and `LICENSE` literally.

- [ ] **Step 6: Commit**

```bash
git add ci/publish-metadata/run.sh
git commit -m "feat(repo): assert publishable crates carry an include allowlist"
```

---

### Task 3: Check 2 becomes one dry-run per publish group

**Files:**
- Modify: `ci/publish-metadata/run.sh` (add `publish_groups` + `check_publish_group`; split `check_package`; rework `main`; fixtures)

**Interfaces:**
- Consumes: `metadata_checks`'s `<name>\t<manifest-dir>` stdout.
- Produces: `publish_groups <metadata.json> <expected-csv>` → one TAB-separated group per line; `check_publish_group <name...>` → rc `0`/`1`/`2`, appending to the script-scope `CHECK2_INVOKED` array.

**Why per-group, not one combined run:** a single all-crates invocation would **weaken** `paigasus-kernel`. Measurement M3 (spec §3) proved the combined form resolves in-set dependencies from a locally staged tarball rather than crates.io; the per-package contract is "compiles standalone with no unversioned path dependency" — publishable *against the registry as it exists now*. For a crate with no in-set dependency, folding it in trades a stronger assertion for a weaker one and buys nothing.

- [ ] **Step 1: Add the group computation**

Insert before `check_package`:

```bash
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
```

- [ ] **Step 2: Split `check_package` and add the group check**

Replace the whole `check_package` function with these two. The Check 2b half keeps its per-package dirty flag; the Check 2 half computes the union across the group.

```bash
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
# resolves against the packaged lockfile rather than whatever the registry serves this minute.
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
      pkg_dir="$(cargo metadata --format-version 1 --no-deps 2>/dev/null \
        | python3 -c 'import json,sys,os; n=sys.argv[1]; print(next(os.path.dirname(p["manifest_path"]) for p in json.load(sys.stdin)["packages"] if p["name"]==n))' "$pkg")"
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
```

- [ ] **Step 3: Rework `main`**

`main` must keep `meta_json` alive until `publish_groups` has run. Replace from `local status=0` through the end of the `while` loop with:

```bash
  local status=0
  local publishable
  publishable="$(metadata_checks "$meta_json" "$RS_DIR/release-plz.toml" "$expected_csv" "$SNAPSHOT")" \
    || status=$?
  [ "$status" -eq 0 ] || { rm -f "$meta_json"; exit "$status"; }

  local groups
  groups="$(publish_groups "$meta_json" "$expected_csv")" || status=$?
  rm -f "$meta_json"
  [ "$status" -eq 0 ] || exit "$status"

  # Checks 1c, 1d and 2b — per package.
  local name dir enumerated=()
  while IFS=$'\t' read -r -u 3 name dir; do
    [ -n "$name" ] || continue
    enumerated+=("$name")
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
```

- [ ] **Step 4: Add the invoked-set assertion**

Insert directly after `check_publish_group`'s closing `}`:

```bash
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
```

- [ ] **Step 5: Add the negative-control fixture rows**

After Task 2's fixture block:

```bash
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
```

- [ ] **Step 6: Run the negative control**

```bash
bash ci/publish-metadata/run.sh --negative-control
```

Expected: exit 0, new `ok —` lines for the empty-list guard, both invoked-set rows, and `publish_groups`.

- [ ] **Step 7: Run the real gate**

```bash
bash ci/publish-metadata/run.sh
```

Expected: exit 0, with a line `publish-metadata: group [paigasus-kernel] OK` — the one-crate group. This proves the restructure is behaviour-preserving before any crate is added.

- [ ] **Step 8: Commit**

```bash
git add ci/publish-metadata/run.sh
git commit -m "refactor(repo): run publish dry-runs per publish group, not per package"
```

---

### Task 4: Proto crate metadata, README, LICENSE, include, lint tables

**Files:**
- Modify: `rs/crates/libs/paigasus-proto/Cargo.toml`, `rs/crates/libs/paigasus-proto-derive/Cargo.toml`
- Create: `rs/crates/libs/paigasus-proto/README.md`, `rs/crates/libs/paigasus-proto/LICENSE`, `rs/crates/libs/paigasus-proto-derive/README.md`, `rs/crates/libs/paigasus-proto-derive/LICENSE`

**Interfaces:**
- Produces: both manifests carrying everything Checks 1/1b/1c/1d/2b need — **except** `publish = true`, which Task 6 flips atomically. Keeping `publish = false` here means the crates stay outside `EXPECTED_PUBLISHABLE`, so the gate is unaffected by this task and it can be reviewed on its own.

**Verified:** neither crate has a `build.rs` or any non-`.rs` asset — the only non-Rust files in either directory are `Cargo.toml` and `moon.yml` — so `src/**/*.rs` omits nothing the build needs. `paigasus-proto-derive` has no `tests/` directory, so its allowlist deliberately omits `tests/**/*.rs`.

- [ ] **Step 1: Copy the LICENSE into both crate dirs**

Cargo has no mechanism to package a file from outside the crate directory, so a copy is required rather than preferred. This is how `paigasus-kernel` does it.

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-577
cp LICENSE rs/crates/libs/paigasus-proto/LICENSE
cp LICENSE rs/crates/libs/paigasus-proto-derive/LICENSE
diff -q LICENSE rs/crates/libs/paigasus-proto/LICENSE && diff -q LICENSE rs/crates/libs/paigasus-proto-derive/LICENSE
```

Expected: no output from `diff` (byte-identical).

- [ ] **Step 2: Write `rs/crates/libs/paigasus-proto/README.md`**

```markdown
# paigasus-proto

Generated protobuf message types and tonic gRPC service stubs for Paigasus, compiled from
the canonical contracts in
[`contracts/proto`](https://github.com/SMK1085/paigasus-core/tree/main/contracts/proto)
(ADR-0004).

The generated sources are committed rather than built at consume time, so this crate has no
`build.rs` and no `protoc` dependency. `AuditMetadata`-bearing messages carry
`#[derive(Auditable)]`, injected during codegen from the companion
[`paigasus-proto-derive`](https://crates.io/crates/paigasus-proto-derive) crate.

Licensed under the Apache License, Version 2.0.
```

- [ ] **Step 3: Write `rs/crates/libs/paigasus-proto-derive/README.md`**

```markdown
# paigasus-proto-derive

`#[derive(Auditable)]` for Paigasus audit metadata.

Generates the `Auditable` accessor implementation for protobuf messages that embed
`paigasus.common.v1.AuditMetadata`. The macro is injected onto the generated types during
codegen and re-exported from
[`paigasus-proto`](https://crates.io/crates/paigasus-proto)'s `audit` module, so consumers
normally depend on that crate rather than this one directly.

Licensed under the Apache License, Version 2.0.
```

- [ ] **Step 4: Rewrite `paigasus-proto`'s `[package]` block and lint table**

Replace lines 1-16 (through `publish = false`) with:

```toml
[package]
name = "paigasus-proto"
# The 0.1.0 floor (ADR-0011 S3) lands in SMA-577's atomic flip; release-plz cuts every tag.
version = "0.0.0"
description = "Generated protobuf message types and tonic gRPC service stubs for Paigasus."
repository = "https://github.com/SMK1085/paigasus-core"
homepage = "https://github.com/SMK1085/paigasus-core#readme"
readme = "README.md"
keywords = ["paigasus", "protobuf", "grpc", "tonic", "prost"]
categories = ["network-programming", "encoding"]
# ALLOWLIST, not a denylist. Cargo's default include is "every non-ignored file in the
# package dir", which swept this monorepo's moon.yml into the tarball. tests/ is kept
# deliberately: a vendoring consumer can run the suite, and auditable_derive_drift.rs
# parses src/generated/** which ships too. CHANGELOG.md is excluded, matching
# paigasus-kernel — release-plz writes one at release time and it does not ship.
include = ["src/**/*.rs", "tests/**/*.rs", "Cargo.toml", "README.md", "LICENSE"]
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
# PUBLISH ORDER: paigasus-proto-derive must publish FIRST — this crate depends on it.
# repo:publish-metadata expresses that as a publish GROUP (a connected component of the
# in-set dependency graph), running one `cargo publish --dry-run -p derive -p proto`.
#
# AuditableExample's derived Auditable impl is public API as of SMA-438 (not cfg(test), a
# deliberate reversal of an SMA-425 decision — spec D8), so it becomes semver-locked once
# this crate publishes. ACCEPTED deliberately in SMA-577: it embeds AuditMetadata, which is
# already public in the generated code, so the fixture was never the binding constraint on
# this crate's semver surface — the domain type it wraps is.
publish = false
```

Then replace the trailing `[lints]\nworkspace = true` with:

```toml
# NOT `workspace = true`. Cargo INLINES the resolved lint table into the published
# manifest, and docs.rs builds a published crate as the ROOT package on nightly, where
# cargo's `--cap-lints allow` does not apply. Inheriting the workspace's
# `warnings = "deny"` would let the first new rustc warning silently kill docs.rs builds
# of a released crate. CI strictness is unaffected: the Moon `lint` task passes
# `-D warnings` explicitly. Asserted by repo:publish-metadata Check 1c.
[lints.rust]
warnings = "warn"

[lints.clippy]
all = "warn"
```

- [ ] **Step 5: Rewrite `paigasus-proto-derive`'s `[package]` block and lint table**

Keep the SPDX header line. Replace the `[package]` block through `publish = false` with:

```toml
[package]
name = "paigasus-proto-derive"
# The 0.1.0 floor (ADR-0011 S3) lands in SMA-577's atomic flip; release-plz cuts every tag.
version = "0.0.0"
description = "Derive macro for Paigasus audit metadata — #[derive(Auditable)] for generated protobuf messages."
repository = "https://github.com/SMK1085/paigasus-core"
homepage = "https://github.com/SMK1085/paigasus-core#readme"
readme = "README.md"
keywords = ["paigasus", "protobuf", "derive", "macro", "audit"]
categories = ["development-tools::procedural-macro-helpers", "development-tools"]
# ALLOWLIST, not a denylist — see the rationale on paigasus-proto. No tests/**/*.rs here:
# this crate has no tests/ directory (its expansion assertions are unit tests under src/).
# Adding one later requires an allowlist edit, or it ships unrunnable.
include = ["src/**/*.rs", "Cargo.toml", "README.md", "LICENSE"]
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
# PUBLISH ORDER: this crate must publish BEFORE paigasus-proto, which depends on it.
publish = false
```

Replace its `[lints]\nworkspace = true` with the same two lint tables as Step 4 (repeat them verbatim; do not write "same as above" in the file).

- [ ] **Step 6: Verify the packaged file lists**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-577/rs
cargo package --list --locked --allow-dirty -p paigasus-proto
cargo package --list --locked --allow-dirty -p paigasus-proto-derive
```

Expected for BOTH: `README.md` and `LICENSE` present, `moon.yml` **absent**. `paigasus-proto` must still list all six `src/generated/**` files and its three `tests/*.rs`.

- [ ] **Step 7: Verify nothing else broke**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-577/rs
cargo clippy -p paigasus-proto -p paigasus-proto-derive --all-targets -- -D warnings
cargo nextest run -p paigasus-proto -p paigasus-proto-derive --no-tests=pass
```

Expected: both pass. The per-crate `warnings = "warn"` does NOT relax CI — the explicit `-D warnings` is what enforces it.

- [ ] **Step 8: Commit**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-577
git add rs/crates/libs/paigasus-proto rs/crates/libs/paigasus-proto-derive
git commit -m "feat(rs): add crates.io metadata, README, LICENSE and lint tables to the proto crates"
```

---

### Task 5: `repo:version-lockstep` — per-(group, kind) lock membership

**Files:**
- Modify: `ci/version-lockstep/run.sh`

**Interfaces:**
- Consumes: the existing `SITES` array, `SOURCE_OF_TRUTH`, `read_version`, `run_check`, `negative_control`.
- Produces: `LOCK_MEMBERS` associative array keyed `<group>:<kind>`; `read_version <kind> <target> [group]`; `lock_reader_self_test`.

**Why keyed by (group, kind), not group alone:** the two handlers span two **namespaces**. `cargo-lock` names are Cargo crate names (`run.sh:160`); `uv-lock` names are Python distribution names (`run.sh:185`). `paigasus-node-bindings` and `paigasus-wasm` are npm artifacts and appear nowhere in `py/uv.lock`; `paigasus-proto-derive` is a proc-macro crate with **zero** occurrences in `py/uv.lock` (verified). "The name set of the row's group" is undefined — demanding `paigasus-proto-derive` in `py/uv.lock` would be a permanent false red.

- [ ] **Step 1: Add the membership table**

Insert directly after the `SOURCE_OF_TRUTH` declaration:

```bash
# Lock-file membership, keyed <group>:<kind> — NOT by group alone, BECAUSE THE TWO KINDS
# SPAN TWO NAMESPACES. cargo-lock names are Cargo crate names; uv-lock names are Python
# distribution names. paigasus-node-bindings and paigasus-wasm are npm artifacts that
# appear nowhere in py/uv.lock, and paigasus-proto-derive is a proc-macro crate with no
# Python distribution at all. A single per-group set would demand it in py/uv.lock — a
# permanent false red — or silently weaken the check.
declare -A LOCK_MEMBERS=(
  [kernel:cargo-lock]="paigasus-kernel paigasus-py-bindings paigasus-node-bindings paigasus-wasm"
  [kernel:uv-lock]="paigasus-kernel paigasus-py-bindings"
  [proto:cargo-lock]="paigasus-proto paigasus-proto-derive"
  [proto:uv-lock]="paigasus-proto"
)
```

- [ ] **Step 2: Add the two SITES rows and bump the anchor**

Append to `SITES`, after the `napi-glue` row:

```bash
  "proto|cargo-lock|rs/Cargo.lock"
  "proto|uv-lock|py/uv.lock"
```

Change `EXPECTED_SITE_COUNT=18` to `EXPECTED_SITE_COUNT=20`, and update its preceding comment's "all 17" / site-count references to match 20.

- [ ] **Step 3: Give `read_version` the group, and make both lock arms use the table**

Change the signature line to:

```bash
read_version() { # $1 kind  $2 path-or-name  $3 group (required for lock kinds)
  local kind="$1" target="$2" group="${3:-}" abs="$REPO_ROOT/$2"
```

In the `cargo-lock)` arm, replace the hardcoded `names = {...}` line by passing the set in. Change the arm to:

```bash
    cargo-lock)
      [ -r "$abs" ] || die_infra "cannot read $target"
      [ -n "$group" ] || die_infra "cargo-lock site for '$target' was read without a group"
      local members="${LOCK_MEMBERS[$group:cargo-lock]:-}"
      [ -n "$members" ] || die_infra "no LOCK_MEMBERS entry for '$group:cargo-lock'"
      python3 - "$abs" "$members" <<'PY'
import re, sys
p, names = sys.argv[1], set(sys.argv[2].split())
try:
    text = open(p, encoding="utf-8").read()
except Exception as e:
    print(f"malformed {p}: {e}", file=sys.stderr); sys.exit(2)
present = set()
found = set()
for blk in text.split("[[package]]"):
    n = re.search(r"^name = \"([^\"]+)\"", blk, re.M)
    v = re.search(r"^version = \"([^\"]+)\"", blk, re.M)
    if n and n.group(1) in names:
        present.add(n.group(1))
        if v:
            found.add(v.group(1))
print(found.pop() if present == names and len(found) == 1 else "")
PY
      ;;
```

Apply the identical transformation to the `uv-lock)` arm, substituting `uv-lock` for `cargo-lock` in the three places it appears. **Preserve both arms' existing presence-plus-uniformity comments** (SMA-576 review finding 4 — a name absent from the lock must not be masked by the survivors' versions agreeing); only the source of `names` moves.

- [ ] **Step 4: Pass the group at the `actual` call site**

In `run_check`'s SITES loop, change:

```bash
    actual="$(read_version "$kind" "$target")" || return 2
```

to:

```bash
    actual="$(read_version "$kind" "$target" "$group")" || return 2
```

The `SOURCE_OF_TRUTH` calls and `run_write`'s call need no change — they pass `cargo-package`, and `run_write` handles only `pyproject`/`pyproject-dep`/`packagejson`.

- [ ] **Step 5: Add the lock-reader self-test**

`EXPECTED_SITE_COUNT` anchors the *number* of `SITES` rows only — it says nothing about the *contents* of a name set. And the existing negative control drifts exactly one site (the node-bindings `package.json`), so no lock handler is exercised at all; `ci/version-lockstep/README.md` records this as limitation **L2**. Insert after `site_verdict_self_test`:

```bash
# L2 closure for the lock readers: EXPECTED_SITE_COUNT cannot see a WRONG name set, and the
# negative control drifts a packagejson site, so before this table neither lock arm was
# exercised at all. Dropping paigasus-proto-derive from [proto:cargo-lock] would have been
# a silent false-green on the very change that introduced the table.
lock_reader_self_test() {
  local tmp got
  tmp="$(mktemp -d)"
  mkdir -p "$tmp/rs" "$tmp/py"

  # All members present at a uniform version -> that version.
  printf '[[package]]\nname = "paigasus-proto"\nversion = "0.1.0"\n\n[[package]]\nname = "paigasus-proto-derive"\nversion = "0.1.0"\n' \
    >"$tmp/rs/Cargo.lock"
  got="$(REPO_ROOT="$tmp" read_version cargo-lock rs/Cargo.lock proto)"
  [ "$got" = "0.1.0" ] || { fail "self-test: uniform proto cargo-lock should read 0.1.0, got '$got'"; rm -rf "$tmp"; return 1; }

  # A MEMBER MISSING must read as "" (MISMATCH), not as the survivor's version.
  printf '[[package]]\nname = "paigasus-proto"\nversion = "0.1.0"\n' >"$tmp/rs/Cargo.lock"
  got="$(REPO_ROOT="$tmp" read_version cargo-lock rs/Cargo.lock proto)"
  [ -z "$got" ] || { fail "self-test: a missing cargo-lock member must read '', got '$got'"; rm -rf "$tmp"; return 1; }

  # Non-uniform versions must read "".
  printf '[[package]]\nname = "paigasus-proto"\nversion = "0.1.0"\n\n[[package]]\nname = "paigasus-proto-derive"\nversion = "0.2.0"\n' \
    >"$tmp/rs/Cargo.lock"
  got="$(REPO_ROOT="$tmp" read_version cargo-lock rs/Cargo.lock proto)"
  [ -z "$got" ] || { fail "self-test: a non-uniform cargo-lock must read '', got '$got'"; rm -rf "$tmp"; return 1; }

  # The uv-lock arm reads its OWN namespace: proto's uv membership is one name.
  printf '[[package]]\nname = "paigasus-proto"\nversion = "0.1.0"\n' >"$tmp/py/uv.lock"
  got="$(REPO_ROOT="$tmp" read_version uv-lock py/uv.lock proto)"
  [ "$got" = "0.1.0" ] || { fail "self-test: proto uv-lock should read 0.1.0, got '$got'"; rm -rf "$tmp"; return 1; }

  rm -rf "$tmp"
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
}
```

Add `lock_reader_self_test` to `run_self_tests` after `site_verdict_self_test`, and change `SELF_TEST_COUNT=1   # site_verdict` to `SELF_TEST_COUNT=2   # site_verdict, lock_reader`.

- [ ] **Step 6: Add a second negative-control drift on a lock row**

In `negative_control`, after the existing packagejson drift and its `run_check` assertion returns, the function currently returns early on success. Restructure so BOTH drifts are exercised: change the success arm of the first check to fall through instead of `return 0`, then add:

```bash
  # Second drift: a LOCK row. The packagejson drift above exercises no lock handler, so
  # without this the new LOCK_MEMBERS table has no end-to-end control coverage.
  python3 - "$tmp/rs/Cargo.lock" <<'PY'
import re, sys
p = sys.argv[1]
text = open(p, encoding="utf-8").read()
# Drift ONE proto member so the set is non-uniform -> the reader must print "".
text = re.sub(
    r'(\[\[package\]\]\nname = "paigasus-proto-derive"\nversion = ")[^"]+(")',
    r"\g<1>99.99.99\g<2>", text, count=1)
open(p, "w", encoding="utf-8").write(text)
PY

  local ec2=0
  REPO_ROOT="$tmp" run_check >/dev/null 2>&1 || ec2=$?
  if [ "$ec2" -ne 1 ]; then
    fail "negative control: a drifted cargo-lock member was not reported (run_check exited $ec2, expected 1).
      The LOCK_MEMBERS table or the cargo-lock reader can no longer report red."
    return 1
  fi
  printf '== negative control: version-lockstep reported red on both a packagejson and a lock drift ==\n'
  return 0
```

- [ ] **Step 7: Run the self-test and negative control**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-577
bash ci/version-lockstep/run.sh --self-test
bash ci/version-lockstep/run.sh --negative-control
```

Expected: `== version-lockstep self-tests passed (2 tables) ==`, then the both-drifts line. If the self-test count mismatches, `SELF_TEST_COUNT` was not bumped.

- [ ] **Step 8: Run the real check**

```bash
bash ci/version-lockstep/run.sh --check
```

Expected: exit 0. Both proto lock rows read `0.0.0`, matching the proto source of truth, which is still `0.0.0` at this point — Task 6 moves them together.

- [ ] **Step 9: Commit**

```bash
git add ci/version-lockstep/run.sh
git commit -m "feat(repo): give version-lockstep per-group-and-kind lock membership"
```

---

### Task 6: The atomic flip — publish, `0.1.0`, locks, release-plz

**Files:**
- Modify: `rs/crates/libs/paigasus-proto/Cargo.toml`, `rs/crates/libs/paigasus-proto-derive/Cargo.toml`, `rs/Cargo.toml`, `py/packages/paigasus-proto/pyproject.toml`, `rs/release-plz.toml`, `ci/publish-metadata/run.sh`, `rs/Cargo.lock`, `py/uv.lock`

**Interfaces:**
- Consumes: everything from Tasks 1-5.

**THIS MUST BE ONE COMMIT.** Spec §8's atomic groups A1-A4: Check 0 is strict equality (either half of `EXPECTED_PUBLISHABLE` + `publish = true` alone reds it); M1 measured exit 101 for a per-package proto dry-run; Check 2 passes `--locked` so a stale lock fails the dry-run; and Check 3 errors on a publishable `0.0.0` crate that is not `release = false`.

- [ ] **Step 1: Run measurement M4 first**

Before changing the gate, prove the invocation it will run actually works at the shipped versions. Temporarily flip both crates to `publish = true` and set all four version sites to `0.1.0`, then:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-577/rs
cargo update -w
cargo publish --dry-run --locked --allow-dirty -p paigasus-kernel -p paigasus-proto-derive -p paigasus-proto 2>&1 | tee /tmp/m4.log
grep -c '^ *Verifying' /tmp/m4.log
```

Expected: exit 0 and **`3`** from the grep — one `Verifying` per named package. If fewer than 3, STOP and report: the per-group design assumed every named package is verify-compiled.

Keep these edits; they are the task's real content. Do not revert.

- [ ] **Step 2: Set both crate versions to `0.1.0`**

In `rs/crates/libs/paigasus-proto/Cargo.toml` and `rs/crates/libs/paigasus-proto-derive/Cargo.toml`, change `version = "0.0.0"` to `version = "0.1.0"` and replace the placeholder comment above it with:

```toml
# The 0.1.0 floor (ADR-0011 S3). release-plz cuts every tag; never hand-place a `*-vX.Y.Z`
# tag — manual tags lack release-plz's tracking metadata and silently stop future bumps
# (the SMA-385 trap). Held in lockstep across the proto family by `repo:version-lockstep`.
```

- [ ] **Step 3: Flip both `publish` flags**

Change `publish = false` to `publish = true` in both manifests, keeping the PUBLISH ORDER / `AuditableExample` comment blocks written in Task 4.

- [ ] **Step 4: Move the two workspace dependency pins**

In `rs/Cargo.toml`, change both to `0.1.0`:

```toml
paigasus-proto-derive = { path = "crates/libs/paigasus-proto-derive", version = "0.1.0" }
```
```toml
paigasus-proto = { path = "crates/libs/paigasus-proto", version = "0.1.0" }
```

These are **version requirements**, not versions — they are what cargo embeds in the published manifest. At `0.0.0` while the crates are `0.1.0`, `cargo publish -p paigasus-proto` resolves against a crates.io version that will never exist. Also update `rs/Cargo.toml:136`'s `PUBLISH ORDER (SMA-388)` comment to reference SMA-577 and the publish-group mechanism.

- [ ] **Step 5: Move the Python site**

In `py/packages/paigasus-proto/pyproject.toml`, change `version = "0.0.0"` to `version = "0.1.0"`.

- [ ] **Step 6: Relock both lockfiles**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-577/rs && cargo update -w
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-577/py && uv lock
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-577
git diff --stat rs/Cargo.lock py/uv.lock
```

Expected: only the proto entries move `0.0.0` → `0.1.0`. **If either lockfile shrinks by hundreds of packages, STOP** — that is the dependabot-style truncation this repo has hit three times. Compare package counts against the merge-base before proceeding.

- [ ] **Step 7: Update `rs/release-plz.toml`**

Remove the two `[[package]]` blocks for `paigasus-proto` and `paigasus-proto-derive` from the "Not releasable" section, and insert a new section after the kernel family block:

```toml
# --- Releasable: the proto family (ADR-0011 S1) --------------------------------------------
# One version across the two crates, ordered derive -> proto (paigasus-proto depends on
# paigasus-proto-derive). Its lockstep is realized structurally: the generated Rust lives
# inside paigasus-proto/src/generated, so a contracts/ change regenerates it, changes the
# crate's files, and release-plz attributes the bump BY FILE PATH (ADR-0011 S5). No contract
# version is introduced. Nothing publishes until SMA-580 flips PAIGASUS_RELEASE_ENABLED;
# `release = true` here only makes them eligible for the release PR.
[[package]]
name = "paigasus-proto"
version_group = "proto"
release = true

[[package]]
name = "paigasus-proto-derive"
version_group = "proto"
release = true
```

Update the "Not releasable" block's comment, which currently promises this change as future work, to describe the state instead.

- [ ] **Step 8: Add both crates to `EXPECTED_PUBLISHABLE`**

In `ci/publish-metadata/run.sh`, change:

```bash
EXPECTED_PUBLISHABLE=("paigasus-kernel")
```

to:

```bash
EXPECTED_PUBLISHABLE=("paigasus-kernel" "paigasus-proto" "paigasus-proto-derive")
```

and update the comment above it — it currently says "SMA-577 adds paigasus-proto AND paigasus-proto-derive here" — to record that it did.

- [ ] **Step 9: Run both gates**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-577
bash ci/publish-metadata/run.sh --negative-control
bash ci/publish-metadata/run.sh
bash ci/version-lockstep/run.sh --self-test
bash ci/version-lockstep/run.sh --negative-control
bash ci/version-lockstep/run.sh --check
```

Expected from the real publish-metadata run: **two** group lines —
`publish-metadata: group [paigasus-kernel] OK` and
`publish-metadata: group [paigasus-proto paigasus-proto-derive] OK` — then
`publish-metadata: all checks passed`. Expected from `--check`: all 20 sites agree, proto at `0.1.0`.

- [ ] **Step 10: Commit — one commit, all of it**

```bash
git add rs/crates/libs/paigasus-proto rs/crates/libs/paigasus-proto-derive rs/Cargo.toml \
        rs/Cargo.lock rs/release-plz.toml py/packages/paigasus-proto/pyproject.toml \
        py/uv.lock ci/publish-metadata/run.sh
git commit -F - <<'EOF'
feat(rs): publish the proto family at 0.1.0

Flips paigasus-proto and paigasus-proto-derive to publish = true at the
0.1.0 floor, moves the five proto version sites together, and makes both
crates releasable via a proto version_group.

These land as one commit deliberately. Check 0 compares the publishable
set by strict equality, a per-package dry-run of paigasus-proto fails
while the derive crate is off crates.io, Check 2 passes --locked so a
stale lockfile fails the dry-run, and Check 3 errors on a publishable
0.0.0 crate that is not release-blocked. Each half alone reds the gate.

Absorbs SMA-388.
EOF
```

---

### Task 7: Documentation

**Files:**
- Modify: `CLAUDE.md`, `ci/publish-metadata/README.md`, `ci/publish-metadata/run.sh` (header block, lines 4-26), `ci/version-lockstep/README.md`, `docs/superpowers/specs/2026-08-22-sma-407-release-activation-design.md`

**Why this is its own task:** none of these files is an input to the gate it documents. `repo:publish-metadata`'s inputs are `run.sh`, `categories.py`, `crates-io-categories.txt`; `repo:version-lockstep`'s is `run.sh`. So a stale README reds nothing, and this repo's gate READMEs are load-bearing — CLAUDE.md cites their limitation sections by number.

- [ ] **Step 1: Update `ci/publish-metadata/run.sh`'s header block**

Add to the check inventory in the comment at lines 4-26, after the Check 1b entry:

```
#   Check 1c each publishable crate declares its OWN [lints.*] table and does not deny.
#            Cargo inlines the resolved table into the published manifest and docs.rs builds
#            on nightly as the root package, where --cap-lints allow does not apply — so an
#            inherited (or hand-written) `warnings = "deny"` silently kills docs.rs builds
#            on the first new rustc lint, months later (SMA-577).
#   Check 1d each publishable crate declares a non-empty `include` ALLOWLIST containing
#            README.md and LICENSE. Membership is literal: "**/*" is rejected, because it
#            would "cover" both while reinstating the moon.yml leak 2b exists to catch.
```

and amend the Check 2 entry to say it runs **once per publish group** (a connected component of the in-set dependency graph), not once per package, citing that a per-package dry-run of a crate with an unpublished in-tree dependency cannot succeed.

- [ ] **Step 2: Update `ci/publish-metadata/README.md`**

Add 1c and 1d to its check inventory with the same rationale, and document the publish-group model plus the `CHECK2_INVOKED` guard, including its stated residual: a one-line deletion of an invocation is caught, but deleting the invocation *and* the assertion together is not, and the external-pin route (`PUBLISH_METADATA_SH_CALL_SITES` in `ci_targets.py` **plus** adding `ci/publish-metadata/run.sh` to `repo:affected-smoke`'s inputs) is deliberately deferred.

- [ ] **Step 3: Update `ci/version-lockstep/README.md`**

Change the hardcoded site count in both places (`:8` "Why 18 sites and not 6", `:39` "Compare all 18 sites") to 20. Amend limitation **L2** — it says the control "does NOT prove each of the eight `read_version` kinds is itself honest" — to record that the two lock kinds are now covered by `lock_reader_self_test` and a second negative-control drift, and that the remaining six are not.

- [ ] **Step 4: Update the parent design**

In `docs/superpowers/specs/2026-08-22-sma-407-release-activation-design.md`:
- §3: correct "the publishable set is **four** crates, not two" to **three**, noting the diagram was already right and the miscount came from counting the kernel *version group*'s four members.
- §14 Q6: mark **answered** — cargo 1.95's multi-package publish resolves the order, and M3 proved it consumes the upstream's packaged tarball. Cite this plan's spec.
- §14 Q7: mark **answered** — `CHANGELOG.md` does not ship, matching `paigasus-kernel`.

- [ ] **Step 5: Add the CLAUDE.md gotcha**

Append to the Gotchas list:

```markdown
- Any crate flipping `publish = true` must carry **its own `[lints.*]` table** and **its own
  `include` allowlist** — enforced by `repo:publish-metadata` Checks 1c/1d (SMA-577). Cargo
  inlines the resolved lint table into the published manifest and docs.rs builds published
  crates as the root package on nightly, where `--cap-lints allow` does NOT apply, so an
  inherited `warnings = "deny"` silently kills docs.rs builds on the first new rustc lint —
  months after the PR. 1d's membership is LITERAL: `include = ["**/*"]` is rejected, since it
  would "cover" README.md/LICENSE while reinstating the `moon.yml` leak Check 2b catches.
  Check 2 runs one `cargo publish --dry-run` per **publish group** (a connected component of
  the in-set dependency graph), NOT per package: a per-package dry-run of `paigasus-proto`
  exits 101 (`no matching package named 'paigasus-proto-derive'`) until the derive crate is on
  crates.io, while `-p paigasus-proto-derive -p paigasus-proto` exits 0. That combined form is
  registry-faithful, not a workspace shortcut — measured by breaking the derive crate's
  `include` and watching the run fail. Grouping keeps `paigasus-kernel` in a group of one so
  it retains its standalone assertion.
```

- [ ] **Step 6: Verify no CI-targets marker was disturbed**

```bash
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-577
grep -c 'ci-targets:begin' CLAUDE.md
grep -c 'ci-targets:end' CLAUDE.md
```

Expected: `1` and `1`. A second copy of either marker — even inside backticks in prose — makes the count 2 and reds `repo:affected-smoke`.

- [ ] **Step 7: Commit**

```bash
git add CLAUDE.md ci/publish-metadata/README.md ci/publish-metadata/run.sh \
        ci/version-lockstep/README.md docs/superpowers/specs/2026-08-22-sma-407-release-activation-design.md
git commit -m "docs(repo): record the publish=true rule and the publish-group dry-run"
```

---

### Task 8: Full CI gate

**Files:** none modified unless a gate reds.

**Why:** per-project Moon tasks do NOT run the repo-level gates. This change touches manifests, both lockfiles, `release-plz.toml`, and two gate scripts that several `repo:*` tasks key on.

- [ ] **Step 1: Run the full affected graph exactly as CI does**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-577
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :input-liveness :promtool :observability-drift :nats-permissions :release-parity \
  :release-parity-py :release-parity-ts :publish-metadata :version-lockstep \
  --base origin/main --include-relations
```

- [ ] **Step 2: If it reports a failure, attribute it**

Moon's summary is unattributed. Do NOT guess:

```bash
python3 -c "import json;print(json.dumps([a['node']['params'] for a in json.load(open('.moon/cache/ciReport.json'))['actions'] if a['status']=='failed'],indent=2))"
```

Expected known-safe outcomes: `repo:release-parity*` are scheduled by the `release-plz.toml` edit but must pass — `_derive_config` greps only `features_always_increment_minor`, so `[[package]]` blocks do not enter the derived fixture.

- [ ] **Step 3: Confirm the Docker-backed suites actually ran**

If `paigasus-iam-rs:test` passed suspiciously fast, its 65 Docker suites skipped silently:

```bash
docker info >/dev/null 2>&1 && echo "docker reachable" || echo "DOCKER DOWN — iam suites will skip"
```

If Docker is down, say so in the final report rather than claiming the suites passed.

- [ ] **Step 4: Commit any fixes**

Only if Step 2 surfaced real breakage. Otherwise nothing to commit — the gate run is verification, not a change.

---

## Self-Review

**Spec coverage:** §2 → Task 6 Step 8. §3 M4 → Task 6 Step 1. §4 manifests/README/LICENSE/include/lints → Task 4; `AuditableExample` decision → Task 4 Step 4 comment; SMA-439 window → recorded in the spec, no code. §5.1 groups → Task 3. §5.2 dirty union → Task 3 Step 2. §5.3 invoked-set → Task 3 Steps 4-5. §5.4 standalone functions → Tasks 1-2. §5.5 Check 1c → Task 1. §5.6 Check 1d → Task 2. §5.7 fixtures → Tasks 1-3. §6.1 release-plz → Task 6 Step 7. §6.2 five sites → Task 6 Steps 2,4,5. §6.3 `LOCK_MEMBERS` + self-test + control → Task 5. §8 atomic groups → Task 6 is one commit. §9 docs → Task 7. §10 testing → every task's verify steps + Task 8.

**Placeholder scan:** no TBD/TODO; every code step carries real content; Task 4 Step 5 explicitly says to repeat the lint tables verbatim rather than write "same as above".

**Type consistency:** `assert_lint_table`, `assert_include_allowlist`, `check_package_list`, `check_publish_group`, `publish_groups`, `assert_check2_covered_everything`, `CHECK2_INVOKED`, `LOCK_MEMBERS`, `lock_reader_self_test` — each defined once and referenced with the same name and arity throughout. `check_package` is fully replaced in Task 3 Step 2 and never referenced afterwards.
