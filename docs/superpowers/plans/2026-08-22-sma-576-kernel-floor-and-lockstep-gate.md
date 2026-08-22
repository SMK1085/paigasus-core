# SMA-576 — Kernel-family `0.1.0` floor + `repo:version-lockstep` gate + release-PR job

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the kernel family off the `0.0.0` stub floor to `0.1.0` in lockstep across Cargo/PyPI/npm manifests, guarded by a new `repo:version-lockstep` CI gate, and stand up release-plz's rolling release-PR job — **without publishing anything**.

**Architecture:** release-plz owns every Cargo `[package] version` (via `version_group`) and every `[workspace.dependencies]` version requirement — both measured, not assumed. A new script `ci/version-lockstep/run.sh` owns the six sites Cargo cannot reach and *verifies* all eighteen, so a `version_group` that silently stopped applying is caught rather than trusted. The workspace-wide `[workspace] release = false` is replaced by per-package settings, because it currently makes release-plz hard-error and because `dependencies_update = true` would otherwise cascade bumps and tags across most of the workspace.

**Tech Stack:** bash + python3 (the house style for `ci/*/run.sh` gates), Moon 2.3.2, release-plz 0.3.158, cargo, uv, pnpm/napi.

**Spec:** `docs/superpowers/specs/2026-08-22-sma-407-release-activation-design.md` (umbrella for SMA-407; this plan implements **SMA-576** only)

## Global Constraints

- Every new source file opens with an SPDX header: `# SPDX-License-Identifier: Apache-2.0`.
- Gate exit codes are **`0` pass | `1` the repo is wrong | `2` infrastructure failed**. A broken invocation must never read as "all checks passed".
- Moon does **not** enable errexit for `script:` blocks. Every multi-line `script:` starts `set -euo pipefail`.
- A new `repo:*` task must appear in **both** `.github/workflows/ci.yml`'s `T=(…)` array **and** the marker-delimited command in `CLAUDE.md` — `ci/affected-graph/ci_targets.py` asserts they agree. `T` must stay a **single-line** bash array.
- `SELF_SCHEDULED_GATES` and `SELF_TASK_EXPECTED_GLOBS` in `ci/affected-graph/ci_targets.py` must have **identical key sets** (asserted at `ci_targets.py:1295-1298`). Adding a gate to one requires adding it to the other.
- All `cargo` invocations run from `rs/` — `rust-toolchain.toml` and `.cargo/config.toml` are discovered by walking up from CWD, not from `--manifest-path`.
- Shell PATH needs `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"` for moon/uv/buf/cargo-nextest/release-plz.
- Conventional commits with a workspace scope; subject lowercase, ≤100 chars; no `#NNN` in the body.
- **Nothing in this plan publishes to any registry.**

## The 18 sites (the one maintained fact)

| # | Site | Group | Owner |
|---|---|---|---|
| 1 | `rs/crates/libs/paigasus-kernel/Cargo.toml` | kernel | release-plz |
| 2 | `rs/crates/bindings/paigasus-py-bindings/Cargo.toml` | kernel | release-plz |
| 3 | `rs/crates/bindings/paigasus-node-bindings/Cargo.toml` | kernel | release-plz |
| 4 | `rs/crates/bindings/paigasus-wasm/Cargo.toml` | kernel | release-plz |
| 5 | `rs/crates/libs/paigasus-proto/Cargo.toml` | proto | release-plz |
| 6 | `rs/crates/libs/paigasus-proto-derive/Cargo.toml` | proto | release-plz |
| 7 | `rs/Cargo.toml` → `[workspace.dependencies] paigasus-kernel.version` | kernel | release-plz |
| 8 | `rs/Cargo.toml` → `[workspace.dependencies] paigasus-proto.version` | proto | release-plz |
| 9 | `rs/Cargo.toml` → `[workspace.dependencies] paigasus-proto-derive.version` | proto | release-plz |
| 10 | `rs/crates/bindings/paigasus-py-bindings/pyproject.toml` → `project.version` | kernel | `--write` |
| 11 | `py/packages/paigasus-kernel/pyproject.toml` → `project.version` | kernel | `--write` |
| 12 | `py/packages/paigasus-proto/pyproject.toml` → `project.version` | proto | `--write` |
| 13 | `rs/crates/bindings/paigasus-node-bindings/package.json` → `version` | kernel | `--write` |
| 14 | `rs/crates/bindings/paigasus-wasm/package.json` → `version` | kernel | `--write` |
| 15 | `py/packages/paigasus-kernel/pyproject.toml` → `project.dependencies` pin `paigasus-py-bindings==X.Y.Z` | kernel | `--write` |
| 16 | `rs/Cargo.lock` | both | `cargo update -w` |
| 17 | `py/uv.lock` | kernel | `uv lock` |
| 18 | `rs/crates/bindings/paigasus-node-bindings/index.js` (26 × `bindingPackageVersion !== '<v>'`) | kernel | `napi build` |

**Sites 5, 6, 9, 12 are proto-group and are checked but NOT bumped by this plan** — the proto family stays at `0.0.0` until SMA-577. The gate asserts *intra-group* agreement, so kernel at `0.1.0` and proto at `0.0.0` is a passing state.

---

### Task 1: `ci/version-lockstep/run.sh` — site inventory + `--check` + `--self-test`

**Files:**
- Create: `ci/version-lockstep/run.sh`
- Create: `ci/version-lockstep/README.md`
- Modify: `py/packages/paigasus-kernel/pyproject.toml` (site 15 — pin the currently-unpinned dependency)

**Interfaces:**
- Produces: `ci/version-lockstep/run.sh` accepting `--check` (default), `--self-test`. Exit `0`/`1`/`2`.
- Produces: a bash function `group_versions()` printing `<group>\t<site-path>\t<field>\t<version>` per site, which Task 2's negative control and Task 3's `--write` both consume.

- [ ] **Step 1: Write the failing self-test**

Create `ci/version-lockstep/run.sh` containing only the self-test harness and a stub, so the first run fails loudly:

```bash
#!/usr/bin/env bash
# SPDX-License-Identifier: Apache-2.0
#
# repo:version-lockstep — asserts every version-carrying site in a lockstep family agrees
# with that family's source-of-truth Cargo crate (SMA-576, spec §4).
#
# Exit codes: 0 pass | 1 assertion failed (the repo is wrong) | 2 infrastructure failed.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SELF_SRC="${BASH_SOURCE[0]}"

die_infra() { printf 'INFRA: %s\n' "$*" >&2; exit 2; }
fail()      { printf 'FAIL: %s\n' "$*" >&2; }

# The ONE maintained fact in this script: the family membership table.
# Format: <group>|<kind>|<path>
# kind ∈ cargo-package | cargo-wsdep | pyproject | pyproject-dep | packagejson | cargo-lock | uv-lock | napi-glue
SITES=(
  "kernel|cargo-package|rs/crates/libs/paigasus-kernel/Cargo.toml"
  "kernel|cargo-package|rs/crates/bindings/paigasus-py-bindings/Cargo.toml"
  "kernel|cargo-package|rs/crates/bindings/paigasus-node-bindings/Cargo.toml"
  "kernel|cargo-package|rs/crates/bindings/paigasus-wasm/Cargo.toml"
  "proto|cargo-package|rs/crates/libs/paigasus-proto/Cargo.toml"
  "proto|cargo-package|rs/crates/libs/paigasus-proto-derive/Cargo.toml"
  "kernel|cargo-wsdep|paigasus-kernel"
  "proto|cargo-wsdep|paigasus-proto"
  "proto|cargo-wsdep|paigasus-proto-derive"
  "kernel|pyproject|rs/crates/bindings/paigasus-py-bindings/pyproject.toml"
  "kernel|pyproject|py/packages/paigasus-kernel/pyproject.toml"
  "proto|pyproject|py/packages/paigasus-proto/pyproject.toml"
  "kernel|packagejson|rs/crates/bindings/paigasus-node-bindings/package.json"
  "kernel|packagejson|rs/crates/bindings/paigasus-wasm/package.json"
  "kernel|pyproject-dep|py/packages/paigasus-kernel/pyproject.toml"
  "kernel|cargo-lock|rs/Cargo.lock"
  "kernel|uv-lock|py/uv.lock"
  "kernel|napi-glue|rs/crates/bindings/paigasus-node-bindings/index.js"
)

# Source of truth per group.
declare -A SOURCE_OF_TRUTH=(
  [kernel]="rs/crates/libs/paigasus-kernel/Cargo.toml"
  [proto]="rs/crates/libs/paigasus-proto/Cargo.toml"
)

SELF_TESTS_RAN=0
SELF_TEST_COUNT=1   # site_verdict

read_version() { # $1 kind  $2 path-or-name  -> prints the version, or exits 2
  die_infra "read_version not implemented"
}

site_verdict() { # $1 expected  $2 actual  -> prints OK or MISMATCH
  die_infra "site_verdict not implemented"
}

site_verdict_self_test() {
  local got
  got="$(site_verdict "0.1.0" "0.1.0")"
  [ "$got" = "OK" ] || { fail "self-test: equal versions should be OK, got '$got'"; return 1; }
  got="$(site_verdict "0.1.0" "0.0.0")"
  [ "$got" = "MISMATCH" ] || { fail "self-test: differing versions should be MISMATCH, got '$got'"; return 1; }
  got="$(site_verdict "0.1.0" "")"
  [ "$got" = "MISMATCH" ] || { fail "self-test: an absent version should be MISMATCH, got '$got'"; return 1; }
  SELF_TESTS_RAN=$((SELF_TESTS_RAN + 1))
}

run_self_tests() {
  SELF_TESTS_RAN=0
  site_verdict_self_test
  [ "$SELF_TESTS_RAN" -eq "$SELF_TEST_COUNT" ] \
    || die_infra "self-tests ran $SELF_TESTS_RAN, expected $SELF_TEST_COUNT"
  printf '== version-lockstep self-tests passed (%d tables) ==\n' "$SELF_TESTS_RAN"
}

MODE=check
while [ $# -gt 0 ]; do
  case "$1" in
    --check)     MODE=check; shift ;;
    --self-test) MODE=selftest; shift ;;
    *) die_infra "unknown flag: $1" ;;
  esac
done

case "$MODE" in
  selftest) run_self_tests ;;
  check)    die_infra "check not implemented" ;;
esac
```

- [ ] **Step 2: Run the self-test to verify it fails**

```bash
bash ci/version-lockstep/run.sh --self-test
```
Expected: exit `2`, `INFRA: site_verdict not implemented`.

- [ ] **Step 3: Implement `site_verdict` and `read_version`**

Replace the two stubs. `site_verdict` is a pure comparison so the self-test can drive it without touching the tree:

```bash
site_verdict() { # $1 expected  $2 actual
  if [ -n "$2" ] && [ "$1" = "$2" ]; then printf 'OK'; else printf 'MISMATCH'; fi
}

read_version() { # $1 kind  $2 path-or-name
  local kind="$1" target="$2" abs="$REPO_ROOT/$2"
  case "$kind" in
    cargo-package)
      [ -r "$abs" ] || die_infra "cannot read $target"
      python3 -c 'import sys,tomllib; print(tomllib.load(open(sys.argv[1],"rb"))["package"]["version"])' "$abs"
      ;;
    cargo-wsdep)
      python3 -c '
import sys, tomllib
d = tomllib.load(open(sys.argv[1], "rb"))["workspace"]["dependencies"].get(sys.argv[2])
print(d.get("version", "") if isinstance(d, dict) else "")
' "$REPO_ROOT/rs/Cargo.toml" "$target"
      ;;
    pyproject)
      [ -r "$abs" ] || die_infra "cannot read $target"
      python3 -c 'import sys,tomllib; print(tomllib.load(open(sys.argv[1],"rb"))["project"]["version"])' "$abs"
      ;;
    pyproject-dep)
      # The pin on paigasus-py-bindings. An UNPINNED dep prints "" and so reads as MISMATCH —
      # which is the point: uv strips [tool.uv.sources] from the built wheel, so an unpinned
      # wrapper would float against any bindings version once published (spec §4).
      python3 -c '
import re, sys, tomllib
deps = tomllib.load(open(sys.argv[1], "rb"))["project"].get("dependencies", [])
for d in deps:
    m = re.fullmatch(r"paigasus-py-bindings==([0-9][^,;\s]*)", d.strip())
    if m:
        print(m.group(1)); break
else:
    print("")
' "$abs"
      ;;
    packagejson)
      [ -r "$abs" ] || die_infra "cannot read $target"
      python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["version"])' "$abs"
      ;;
    cargo-lock)
      # Every kernel-group member must appear in the lock at the group version. Prints the
      # DISTINCT set; anything but a single value reads as MISMATCH against the expected.
      python3 -c '
import re, sys
names = {"paigasus-kernel", "paigasus-py-bindings", "paigasus-node-bindings", "paigasus-wasm"}
text = open(sys.argv[1], encoding="utf-8").read()
found = set()
for blk in text.split("[[package]]"):
    n = re.search(r"^name = \"([^\"]+)\"", blk, re.M)
    v = re.search(r"^version = \"([^\"]+)\"", blk, re.M)
    if n and v and n.group(1) in names:
        found.add(v.group(1))
print(found.pop() if len(found) == 1 else "")
' "$abs"
      ;;
    uv-lock)
      python3 -c '
import re, sys
names = {"paigasus-kernel", "paigasus-py-bindings"}
text = open(sys.argv[1], encoding="utf-8").read()
found = set()
for blk in text.split("[[package]]"):
    n = re.search(r"^name = \"([^\"]+)\"", blk, re.M)
    v = re.search(r"^version = \"([^\"]+)\"", blk, re.M)
    if n and v and n.group(1) in names:
        found.add(v.group(1))
print(found.pop() if len(found) == 1 else "")
' "$abs"
      ;;
    napi-glue)
      # napi regenerates 26 `bindingPackageVersion !== '<v>'` guards from package.json.
      # A non-uniform set prints "" and reads as MISMATCH.
      python3 -c '
import re, sys
vs = set(re.findall(r"bindingPackageVersion !== '"'"'([^'"'"']+)'"'"'", open(sys.argv[1], encoding="utf-8").read()))
print(vs.pop() if len(vs) == 1 else "")
' "$abs"
      ;;
    *) die_infra "unknown site kind: $kind" ;;
  esac
}
```

- [ ] **Step 4: Run the self-test to verify it passes**

```bash
bash ci/version-lockstep/run.sh --self-test
```
Expected: exit `0`, `== version-lockstep self-tests passed (1 tables) ==`.

- [ ] **Step 5: Implement `--check`**

```bash
run_check() {
  local rc=0 group kind target expected actual verdict
  for group in "${!SOURCE_OF_TRUTH[@]}"; do
    expected="$(read_version cargo-package "${SOURCE_OF_TRUTH[$group]}")"
    [ -n "$expected" ] || die_infra "group '$group': source of truth has no version"
    printf 'group %s: source of truth = %s\n' "$group" "$expected"
  done
  local checked=0
  for entry in "${SITES[@]}"; do
    IFS='|' read -r group kind target <<<"$entry"
    expected="$(read_version cargo-package "${SOURCE_OF_TRUTH[$group]}")"
    actual="$(read_version "$kind" "$target")"
    verdict="$(site_verdict "$expected" "$actual")"
    checked=$((checked + 1))
    if [ "$verdict" != OK ]; then
      fail "[$group] $kind $target: expected '$expected', found '${actual:-<absent or non-uniform>}'"
      rc=1
    fi
  done
  # Non-vacuity: the loop must have covered every declared site.
  [ "$checked" -eq "${#SITES[@]}" ] \
    || die_infra "checked $checked sites but ${#SITES[@]} are declared"
  if [ "$rc" -eq 0 ]; then
    printf '== all %d version-lockstep sites agree ==\n' "$checked"
  fi
  return "$rc"
}
```

Wire it: replace `check) die_infra "check not implemented" ;;` with `check) run_check ;;`.

- [ ] **Step 6: Run `--check` — expect it to FAIL on site 15**

```bash
bash ci/version-lockstep/run.sh
```
Expected: exit `1`, one failure — `[kernel] pyproject-dep py/packages/paigasus-kernel/pyproject.toml: expected '0.0.0', found '<absent or non-uniform>'`. Every other site is already uniformly `0.0.0`.

This is the gate finding a real, pre-existing defect: the published wrapper would float against any bindings version.

- [ ] **Step 7: Pin the dependency (site 15)**

In `py/packages/paigasus-kernel/pyproject.toml`, change:

```toml
dependencies = ["paigasus-py-bindings"]
```

to:

```toml
# Pinned exactly, and asserted by repo:version-lockstep. [tool.uv.sources] below is
# DEVELOPMENT-ONLY metadata — uv strips it from the built wheel — so without this pin the
# published wrapper would float against any paigasus-py-bindings version (SMA-576, spec §4).
# Under lockstep the bindings can never release independently, so an exact pin costs nothing.
dependencies = ["paigasus-py-bindings==0.0.0"]
```

- [ ] **Step 8: Run `--check` to verify it passes**

```bash
bash ci/version-lockstep/run.sh
```
Expected: exit `0`, `== all 18 version-lockstep sites agree ==`.

- [ ] **Step 9: Write `ci/version-lockstep/README.md`**

```markdown
<!-- SPDX-License-Identifier: Apache-2.0 -->

# `repo:version-lockstep`

Asserts every version-carrying site in a lockstep family agrees with that family's
source-of-truth Cargo crate (ADR-0011 S1; SMA-576).

## Why 18 sites and not 6

release-plz owns the Cargo `[package] version` of every group member and the
`[workspace.dependencies]` version *requirements* — both measured against the pinned
0.3.158, not assumed. But four classes of site are owned by nobody:

- `pyproject.toml` / `package.json` versions (maturin and napi read these, not Cargo)
- the `paigasus-py-bindings==X.Y.Z` pin in the Python wrapper — `[tool.uv.sources]` is
  development-only metadata that uv strips from the built wheel
- `rs/Cargo.lock` and `py/uv.lock`
- `rs/crates/bindings/paigasus-node-bindings/index.js`, whose 26 committed
  `bindingPackageVersion !== '<v>'` guards napi regenerates from `package.json`

`py/packages/paigasus-kernel/moon.yml` runs bare `uv sync` (not `--locked`), and
`ci.yml`'s codegen-drift gate covers only the three `**/generated` proto dirs — so the
last two drift **silently** today.

## Why `--check` verifies sites release-plz owns

A gate that trusted release-plz to have done its half would not notice a `version_group`
that silently stopped applying. Checking them costs nothing and closes that.

## Groups are checked independently

The gate asserts *intra-group* agreement. `kernel` at `0.1.0` and `proto` at `0.0.0` is a
passing state — the proto family activates in SMA-577.

## Modes

| Mode | Behaviour |
|---|---|
| `--check` (default) | Compare all 18 sites. Exit 1 on any drift. |
| `--write` | Rewrite the six sites release-plz cannot reach and regenerate the three derived ones. |
| `--negative-control` | Prove the checker can still report red. |
| `--self-test` | Fixture tables for the verdict function. |

Exit codes: `0` pass, `1` the repo is wrong, `2` infrastructure failed.
```

- [ ] **Step 10: Commit**

```bash
git add ci/version-lockstep/run.sh ci/version-lockstep/README.md py/packages/paigasus-kernel/pyproject.toml
git commit -m "feat(ci): add the version-lockstep site inventory and --check"
```

---

### Task 2: `--negative-control`

**Files:**
- Modify: `ci/version-lockstep/run.sh`
- Modify: `ci/version-lockstep/README.md`

**Interfaces:**
- Consumes: `run_check`, `read_version`, `site_verdict` from Task 1.
- Produces: `--negative-control`, exiting `0` when the checker correctly reports red and `1` when it does not.

**Why:** a gate that has lost the ability to report red is green exactly when it matters. `run_check` reads the real tree, so the control must drive it against a deliberately-drifted copy.

- [ ] **Step 1: Write the failing control**

Add to `ci/version-lockstep/run.sh`, before the flag parse:

```bash
# Copy the tree's version-carrying files into a scratch dir, drift ONE site, and assert the
# checker reports red. Driving the real run_check (not a reimplementation) is what makes this
# a control rather than a second, differently-wrong checker.
negative_control() {
  local tmp
  tmp="$(mktemp -d)" || die_infra "cannot create a scratch dir"
  # shellcheck disable=SC2064
  trap "rm -rf '$tmp'" RETURN

  # Stage exactly the files the SITES table names, plus rs/Cargo.toml (which the cargo-wsdep
  # kind reads by name rather than by path). Deriving the list from SITES rather than from a
  # hand-written glob keeps the control honest when a site is added: a new site is staged
  # automatically, so the control cannot quietly stop covering it.
  local entry kind target
  {
    printf 'rs/Cargo.toml\n'
    for entry in "${SITES[@]}"; do
      IFS='|' read -r _ kind target <<<"$entry"
      [ "$kind" = cargo-wsdep ] || printf '%s\n' "$target"
    done
  } | sort -u | ( cd "$REPO_ROOT" && tar -cf - -T - ) | ( cd "$tmp" && tar -xf - ) \
    || die_infra "cannot stage a scratch copy of the version-carrying files"

  # Drift site 13 (@paigasus/node-bindings) to a version no group member carries.
  python3 - "$tmp/rs/crates/bindings/paigasus-node-bindings/package.json" <<'PY'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
d["version"] = "99.99.99"
json.dump(d, open(p, "w"), indent=2)
PY

  local ec=0
  REPO_ROOT="$tmp" run_check >/dev/null 2>&1 || ec=$?
  if [ "$ec" -eq 1 ]; then
    printf '== negative control: version-lockstep reported red as expected ==\n'
    return 0
  fi
  fail "negative control: a drifted site was ACCEPTED (run_check exited $ec, expected 1).
      The gate can no longer report red and is green exactly when it matters."
  return 1
}
```

Add `--negative-control) MODE=negctl; shift ;;` to the flag parse and `negctl) negative_control ;;` to the mode dispatch.

- [ ] **Step 2: Run it to verify it passes**

```bash
bash ci/version-lockstep/run.sh --negative-control
```
Expected: exit `0`, `== negative control: version-lockstep reported red as expected ==`.

- [ ] **Step 3: Prove the control actually bites**

Temporarily neuter `site_verdict` so it always returns `OK`:

```bash
# Edit site_verdict's body to: printf 'OK'
bash ci/version-lockstep/run.sh --negative-control
```
Expected: exit `1`, `negative control: a drifted site was ACCEPTED`.

**Then undo the neutering by re-editing `site_verdict` back to its Task 1 body.** Do **NOT** run `git checkout -- ci/version-lockstep/run.sh`: this task's negative-control work is still uncommitted and that command would discard it. Do not restore from a `.bak` either — that rolls mtime backwards.

- [ ] **Step 4: Re-run both modes to confirm the revert**

```bash
bash ci/version-lockstep/run.sh --negative-control && bash ci/version-lockstep/run.sh
```
Expected: both exit `0`.

- [ ] **Step 5: Document it in the README**

Append to `ci/version-lockstep/README.md`:

```markdown
## The negative control

`--negative-control` stages a scratch copy of every version-carrying file, drifts
`@paigasus/node-bindings` to `99.99.99`, and asserts `run_check` exits 1. It drives the
**real** `run_check` rather than a reimplementation — a second, differently-wrong checker
would prove nothing.

Measured: with `site_verdict` neutered to always return `OK`, the real run still prints
`== all 18 version-lockstep sites agree ==` and exits 0. The control reds.
```

- [ ] **Step 6: Commit**

```bash
git add ci/version-lockstep/run.sh ci/version-lockstep/README.md
git commit -m "feat(ci): add the version-lockstep negative control"
```

---

### Task 3: `--write`

**Files:**
- Modify: `ci/version-lockstep/run.sh`
- Modify: `ci/version-lockstep/README.md`

**Interfaces:**
- Consumes: `read_version`, `SITES`, `SOURCE_OF_TRUTH` from Task 1.
- Produces: `--write`, which rewrites sites 10–15 and regenerates 16–18. Exits `0` in both the "wrote something" and "already in lockstep" cases, printing `version-lockstep: wrote N site(s)` or `version-lockstep: already in lockstep` so the release-PR job can distinguish them.

- [ ] **Step 1: Write the failing test**

```bash
bash ci/version-lockstep/run.sh --write
```
Expected: exit `2`, `INFRA: unknown flag: --write`.

- [ ] **Step 2: Implement `--write`**

```bash
write_site() { # $1 kind  $2 target  $3 version  -> prints 1 if it changed the file, else 0
  local kind="$1" target="$2" version="$3" abs="$REPO_ROOT/$2"
  case "$kind" in
    pyproject)
      python3 - "$abs" "$version" <<'PY'
import re, sys
p, v = sys.argv[1], sys.argv[2]
s = open(p, encoding="utf-8").read()
new, n = re.subn(r'(?m)^(version\s*=\s*)"[^"]*"', lambda m: f'{m.group(1)}"{v}"', s, count=1)
if n != 1:
    print("FATAL: no [project] version line", file=sys.stderr); raise SystemExit(2)
open(p, "w", encoding="utf-8").write(new)
print(int(new != s))
PY
      ;;
    pyproject-dep)
      python3 - "$abs" "$version" <<'PY'
import re, sys
p, v = sys.argv[1], sys.argv[2]
s = open(p, encoding="utf-8").read()
new, n = re.subn(r'"paigasus-py-bindings(?:==[^"]*)?"', f'"paigasus-py-bindings=={v}"', s, count=1)
if n != 1:
    print("FATAL: no paigasus-py-bindings dependency", file=sys.stderr); raise SystemExit(2)
open(p, "w", encoding="utf-8").write(new)
print(int(new != s))
PY
      ;;
    packagejson)
      python3 - "$abs" "$version" <<'PY'
import json, sys
p, v = sys.argv[1], sys.argv[2]
s = open(p, encoding="utf-8").read()
d = json.loads(s)
d["version"] = v
new = json.dumps(d, indent=2) + "\n"
open(p, "w", encoding="utf-8").write(new)
print(int(new != s))
PY
      ;;
    *) printf '0' ;;   # release-plz- and regeneration-owned kinds are not written here
  esac
}

run_write() {
  local wrote=0 group kind target expected changed
  for entry in "${SITES[@]}"; do
    IFS='|' read -r group kind target <<<"$entry"
    case "$kind" in pyproject|pyproject-dep|packagejson) ;; *) continue ;; esac
    expected="$(read_version cargo-package "${SOURCE_OF_TRUTH[$group]}")"
    changed="$(write_site "$kind" "$target" "$expected")"
    wrote=$((wrote + changed))
  done

  # Regenerate the three derived sites (16-18). Each is owned by a tool, not by this script.
  ( cd "$REPO_ROOT/rs" && cargo update -w --offline >/dev/null 2>&1 ) \
    || ( cd "$REPO_ROOT/rs" && cargo update -w >/dev/null ) \
    || die_infra "cargo update -w failed (site 16)"
  ( cd "$REPO_ROOT/py" && uv lock >/dev/null ) || die_infra "uv lock failed (site 17)"
  ( cd "$REPO_ROOT/ts" && pnpm exec napi build --platform \
      --cwd ../rs/crates/bindings/paigasus-node-bindings >/dev/null ) \
    || die_infra "napi build failed (site 18)"

  if [ "$wrote" -gt 0 ]; then
    printf 'version-lockstep: wrote %d site(s)\n' "$wrote"
  else
    printf 'version-lockstep: already in lockstep\n'
  fi
}
```

Add `--write) MODE=write; shift ;;` and `write) run_write ;;`.

- [ ] **Step 3: Run `--write` on an in-lockstep tree**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/version-lockstep/run.sh --write
```
Expected: exit `0`, `version-lockstep: already in lockstep`.

- [ ] **Step 4: Prove idempotence and that `--write` repairs drift**

```bash
python3 - <<'PY'
import json
p = "rs/crates/bindings/paigasus-wasm/package.json"
d = json.load(open(p)); d["version"] = "9.9.9"
json.dump(d, open(p, "w"), indent=2)
PY
bash ci/version-lockstep/run.sh; echo "check rc=$?"
bash ci/version-lockstep/run.sh --write
bash ci/version-lockstep/run.sh; echo "check rc=$?"
bash ci/version-lockstep/run.sh --write
```
Expected: first check `rc=1`; `--write` prints `wrote 1 site(s)`; second check `rc=0`; second `--write` prints `already in lockstep`.

- [ ] **Step 5: Restore the tree**

```bash
git checkout -- rs/crates/bindings/paigasus-wasm/package.json
bash ci/version-lockstep/run.sh
```
Expected: exit `0`.

- [ ] **Step 6: Commit**

```bash
git add ci/version-lockstep/run.sh ci/version-lockstep/README.md
git commit -m "feat(ci): add version-lockstep --write for the six unowned sites"
```

---

### Task 4: Wire `repo:version-lockstep` into Moon and CI

**Files:**
- Modify: `moon.yml` (add the task)
- Modify: `.github/workflows/ci.yml:214` (the `T=(…)` array)
- Modify: `CLAUDE.md` (the marker-delimited command)
- Modify: `ci/affected-graph/ci_targets.py` (`SELF_SCHEDULED_GATES` + `SELF_TASK_EXPECTED_GLOBS`)

**Interfaces:**
- Consumes: `ci/version-lockstep/run.sh` with `--negative-control` and the default check.
- Produces: a `repo:version-lockstep` Moon task that `moon ci` schedules.

- [ ] **Step 1: Add the Moon task**

In `moon.yml`, after the `publish-metadata` task:

```yaml
  version-lockstep:
    description: 'Assert every version-carrying site in the kernel and proto lockstep families agrees with that family source-of-truth Cargo crate (SMA-576).'
    # The negative control runs FIRST: a gate that cannot report red is worse than no gate,
    # and it is sub-second. Moon does not enable errexit for `script:` blocks, so without the
    # explicit pipefail a failing control would be masked by the passing real run.
    script: |
      set -euo pipefail
      bash ci/version-lockstep/run.sh --negative-control
      bash ci/version-lockstep/run.sh
    toolchain: 'system'
    # Every file that CARRIES a version in either family, plus the script itself. Narrower
    # than publish-metadata's rs/crates/** because the site list here is STATIC (declared in
    # SITES), not discovered at runtime — but each entry must match a tracked file or
    # repo:input-liveness reds.
    inputs:
      - 'ci/version-lockstep/run.sh'
      - '/rs/Cargo.toml'
      - '/rs/Cargo.lock'
      - '/rs/crates/libs/paigasus-kernel/Cargo.toml'
      - '/rs/crates/libs/paigasus-proto/Cargo.toml'
      - '/rs/crates/libs/paigasus-proto-derive/Cargo.toml'
      - '/rs/crates/bindings/paigasus-py-bindings/Cargo.toml'
      - '/rs/crates/bindings/paigasus-py-bindings/pyproject.toml'
      - '/rs/crates/bindings/paigasus-node-bindings/Cargo.toml'
      - '/rs/crates/bindings/paigasus-node-bindings/package.json'
      - '/rs/crates/bindings/paigasus-node-bindings/index.js'
      - '/rs/crates/bindings/paigasus-wasm/Cargo.toml'
      - '/rs/crates/bindings/paigasus-wasm/package.json'
      - '/py/uv.lock'
      - '/py/packages/paigasus-kernel/pyproject.toml'
      - '/py/packages/paigasus-proto/pyproject.toml'
```

- [ ] **Step 2: Verify the task resolves**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:version-lockstep --force
```
Expected: PASS, with both the control and the real check printing.

- [ ] **Step 3: Add it to `ci.yml`'s `T` array**

At `.github/workflows/ci.yml:214`, append ` :version-lockstep` immediately before the closing paren. **Keep it a single line.**

- [ ] **Step 4: Add it to CLAUDE.md's marker block**

Between `<!-- ci-targets:begin -->` and `<!-- ci-targets:end -->`, append `:version-lockstep` to the target list, keeping the `--base origin/main --include-relations` tail last.

- [ ] **Step 5: Register the self-scheduled-gate pins**

In `ci/affected-graph/ci_targets.py`, extend **both** constants — they must have identical key sets (`ci_targets.py:1295-1298`):

```python
SELF_TASK_EXPECTED_GLOBS = {
    "input-liveness": ("**/*",),
    "version-lockstep": (
        "ci/version-lockstep/run.sh",
        "/rs/Cargo.toml",
        "/rs/Cargo.lock",
        "/rs/crates/libs/paigasus-kernel/Cargo.toml",
        "/rs/crates/libs/paigasus-proto/Cargo.toml",
        "/rs/crates/libs/paigasus-proto-derive/Cargo.toml",
        "/rs/crates/bindings/paigasus-py-bindings/Cargo.toml",
        "/rs/crates/bindings/paigasus-py-bindings/pyproject.toml",
        "/rs/crates/bindings/paigasus-node-bindings/Cargo.toml",
        "/rs/crates/bindings/paigasus-node-bindings/package.json",
        "/rs/crates/bindings/paigasus-node-bindings/index.js",
        "/rs/crates/bindings/paigasus-wasm/Cargo.toml",
        "/rs/crates/bindings/paigasus-wasm/package.json",
        "/py/uv.lock",
        "/py/packages/paigasus-kernel/pyproject.toml",
        "/py/packages/paigasus-proto/pyproject.toml",
    ),
}
```

and, mirroring the `input-liveness` entry's whole-line discipline (the real check's command is a strict prefix of the control's, so the REAL RUN must be pinned as a whole line or deleting it stays green):

```python
SELF_SCHEDULED_GATES = {
    "input-liveness": (
        "set -euo pipefail",
        "python3 ci/affected-graph/task_inputs.py --self-test",
        "python3 ci/affected-graph/task_inputs.py",
    ),
    # Same three-line shape and the same reason: the pipefail line is as load-bearing as
    # either invocation, because Moon's script blocks take their status from the LAST command.
    "version-lockstep": (
        "set -euo pipefail",
        "bash ci/version-lockstep/run.sh --negative-control",
        "bash ci/version-lockstep/run.sh",
    ),
}
```

- [ ] **Step 6: Run the guarding gates**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:affected-smoke --force
moon run repo:input-liveness --force
moon run repo:actionlint --force
```
Expected: all PASS. `affected-smoke` is what asserts `T` and CLAUDE.md agree and that `:version-lockstep` resolves to a real CI-eligible task.

- [ ] **Step 7: Prove the `T`/CLAUDE.md pin bites**

```bash
# Remove :version-lockstep from CLAUDE.md's marker block only
moon run repo:affected-smoke --force
```
Expected: FAIL, naming the disagreement. Then restore it and re-run to green.

- [ ] **Step 8: Commit**

```bash
git add moon.yml .github/workflows/ci.yml CLAUDE.md ci/affected-graph/ci_targets.py
git commit -m "feat(ci): schedule repo:version-lockstep and pin its own wiring"
```

---

### Task 5: Move the kernel family to the `0.1.0` floor

**Files:**
- Modify: sites 1–4 (`Cargo.toml` × 4), 7 (`rs/Cargo.toml`), 10–11, 13–15
- Regenerate: 16 (`rs/Cargo.lock`), 17 (`py/uv.lock`), 18 (`index.js`)
- Modify: `rs/crates/libs/paigasus-kernel/Cargo.toml` (the stub-floor comment)

**Interfaces:**
- Consumes: `ci/version-lockstep/run.sh --write` from Task 3, which does sites 10–18 mechanically.
- Produces: the kernel family uniformly at `0.1.0`. The proto family stays at `0.0.0`.

**Why this order:** the gate landed first precisely so that *it* verifies the bump is complete. `--check` passing at the end is the deliverable's proof.

- [ ] **Step 1: Bump the four kernel-group Cargo versions (sites 1–4)**

Set `version = "0.1.0"` in each of:
- `rs/crates/libs/paigasus-kernel/Cargo.toml`
- `rs/crates/bindings/paigasus-py-bindings/Cargo.toml`
- `rs/crates/bindings/paigasus-node-bindings/Cargo.toml`
- `rs/crates/bindings/paigasus-wasm/Cargo.toml`

- [ ] **Step 2: Replace the stub-floor comment on `paigasus-kernel`**

The existing comment charters this work and is now stale. Replace it with:

```toml
# The 0.1.0 floor (ADR-0011 S3). release-plz cuts every tag; never hand-place a `*-vX.Y.Z`
# tag — manual tags lack release-plz's tracking metadata and silently stop future bumps
# (the SMA-385 trap). This version is held in lockstep with the py/npm binding artifacts by
# `repo:version-lockstep`, and release-plz keeps the group together via `version_group`.
version = "0.1.0"
```

- [ ] **Step 3: Bump the kernel workspace dependency requirement (site 7)**

In `rs/Cargo.toml`:

```toml
paigasus-kernel = { path = "crates/libs/paigasus-kernel", version = "0.1.0" }
```

Leave `paigasus-proto` and `paigasus-proto-derive` at `version = "0.0.0"` — the proto family activates in SMA-577.

- [ ] **Step 4: Let `--write` do sites 10–18**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
bash ci/version-lockstep/run.sh --write
```
Expected: `version-lockstep: wrote 5 site(s)` — sites 10, 11, 13, 14 and the site-15 pin. Site 12 (`py/packages/paigasus-proto/pyproject.toml`) is **not** among them: it is proto-group and stays at `0.0.0`.

- [ ] **Step 5: Run the gate**

```bash
bash ci/version-lockstep/run.sh
```
Expected: exit `0`, `== all 18 version-lockstep sites agree ==`.

- [ ] **Step 6: Verify the derived sites really moved**

```bash
grep -c "bindingPackageVersion !== '0.1.0'" rs/crates/bindings/paigasus-node-bindings/index.js
grep -A1 'name = "paigasus-kernel"' rs/Cargo.lock | head -4
grep -A1 'name = "paigasus-py-bindings"' py/uv.lock | head -4
grep 'paigasus-py-bindings==' py/packages/paigasus-kernel/pyproject.toml
```
Expected: `26`; `version = "0.1.0"` in both lockfiles; `paigasus-py-bindings==0.1.0`.

- [ ] **Step 7: Confirm `repo:publish-metadata` still passes**

```bash
moon run repo:publish-metadata --force
```
Expected: PASS. Check 3 now goes **vacuously satisfied** — no publishable crate remains at `0.0.0`, so it skips its block entirely. `[workspace] release = false` is still present and harmless; Task 6 replaces it.

- [ ] **Step 8: Run the full affected graph**

```bash
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site :input-liveness :promtool :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata :version-lockstep --base origin/main --include-relations
```
Expected: all PASS. If a task fails without attribution, read `.moon/cache/ciReport.json` — `jq '.actions[]|select(.status=="failed")'`.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat(rs): move the kernel family to the 0.1.0 floor in lockstep"
```

---

### Task 6: Replace `[workspace] release = false` with per-package settings

**Files:**
- Modify: `rs/release-plz.toml`
- Modify: `ci/publish-metadata/run.sh` (the `EXPECTED_PUBLISHABLE` comment, which cites SMA-388)

**Interfaces:**
- Consumes: the `0.1.0` floor from Task 5 (Check 3 must already be vacuous, or removing the block reds `repo:publish-metadata`).
- Produces: a release-plz config under which only the four kernel-group crates are releasable, and they move as one group.

**Why now, and why not just delete the line:** measured against 0.3.158 — `[workspace] release = false` makes release-plz **hard-error** with `no public packages found`, so Task 7's job could not run at all. And deleting it outright is worse: `dependencies_update = true` cascades a bump into every transitive dependent (measured), and Cargo's `publish = false` suppresses publishing but **not tagging**, so the first release would permanently tag most of the workspace. Per-package `release = false` removes a package from the proposal entirely (measured).

- [ ] **Step 1: Rewrite `rs/release-plz.toml`**

```toml
# Live release-plz configuration (SMA-576).
#
# The SMA-398 parity harness DERIVES its fixture config from the classification keys below,
# so this file is the single source of truth for commit->semver behaviour. Do not duplicate
# them into ci/release-parity/.

[workspace]
# NO workspace-level `release` key. It was `release = false` while every package sat at the
# 0.0.0 stub floor; measured against release-plz 0.3.158, that makes the tool hard-error with
# "no public packages found", so a release-PR job cannot run under it. Releasability is now
# declared per package below — which is also what stops `dependencies_update` from cascading
# tags into crates nobody intended to release (spec §8).
#
# Conventional-Commit -> semver classification (the contract SMA-398 asserts).
# In 0.x: fix -> patch, feat -> minor, breaking (! or BREAKING CHANGE) -> minor.
features_always_increment_minor = true
# Kept ON deliberately. It cascades a patch bump into every transitive dependent of a bumped
# crate (measured: a crate neither in the version group nor touched by the commit was still
# bumped, logged "dependencies changed"). That is correct for a workspace that vendors its own
# libs; the per-package `release = false` entries below are what stop the cascade turning into
# tags. Turning it off instead would change what the parity fixture derives from this file.
dependencies_update = true

# --- Releasable: the kernel family (ADR-0011 S1) ------------------------------------------
# One version across crates.io/PyPI/npm. `version_group` holds them together, and it DOES
# apply to crates whose Cargo manifest says `publish = false` (measured) — which is exactly
# what the three binding crates need, since they ship as maturin/napi/wasm byproducts rather
# than to crates.io. `repo:version-lockstep` asserts the non-Cargo manifests follow.
[[package]]
name = "paigasus-kernel"
version_group = "kernel"
release = true

[[package]]
name = "paigasus-py-bindings"
version_group = "kernel"
release = true

[[package]]
name = "paigasus-node-bindings"
version_group = "kernel"
release = true

[[package]]
name = "paigasus-wasm"
version_group = "kernel"
release = true

# --- Not releasable ------------------------------------------------------------------------
# Every other member. `release = false` removes a package from the release-PR proposal
# ENTIRELY (measured: it is neither bumped nor listed), which is what keeps the
# dependencies_update cascade from bumping and permanently tagging crates nobody released.
#
# paigasus-proto / paigasus-proto-derive join the "proto" version_group in SMA-577, once they
# carry publishable metadata. paigasus-gateway / paigasus-iam stay at 0.0.0 deliberately:
# their `env!("CARGO_PKG_VERSION")` feeds the ServiceInfo descriptor, and ADR-0020 skew
# reporting is parked on that value (SMA-505 R7).
[[package]]
name = "paigasus-proto"
release = false

[[package]]
name = "paigasus-proto-derive"
release = false

[[package]]
name = "paigasus-iam-core"
release = false

[[package]]
name = "paigasus-kernel-parity"
release = false

[[package]]
name = "paigasus-logging"
release = false

[[package]]
name = "paigasus-observability"
release = false

[[package]]
name = "paigasus-service-info"
release = false

[[package]]
name = "paigasus-gateway"
release = false

[[package]]
name = "paigasus-iam"
release = false

[changelog]
sort_commits = "newest"
```

- [ ] **Step 2: Verify release-plz proposes only the kernel group**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
cd rs && release-plz update --dry-run 2>&1 | tail -20; cd ..
```
Expected: only kernel-group crates appear, if any. **Record the actual output verbatim in the PR description** — the spec's §10 predicts release-plz proposes the manifest version (no bump) because its baseline is the crates.io registry, not tags. This is the first real observation of that.

- [ ] **Step 3: Confirm the parity gates still pass**

```bash
moon run repo:release-parity --force
moon run repo:release-parity-py --force
moon run repo:release-parity-ts --force
```
Expected: all PASS. The harness derives its fixture from this file's classification keys, and `features_always_increment_minor` and `dependencies_update` are unchanged.

- [ ] **Step 4: Confirm `repo:publish-metadata` still passes**

```bash
moon run repo:publish-metadata --force
```
Expected: PASS. Check 3 is vacuous (nothing publishable at `0.0.0`), so the removed block is not required.

- [ ] **Step 5: Update the stale `EXPECTED_PUBLISHABLE` comment**

In `ci/publish-metadata/run.sh:47`, the comment reads `# The ONE maintained fact in this script. SMA-388 adds paigasus-proto here.` Change the citation to SMA-577, which now owns that work:

```bash
# The ONE maintained fact in this script. SMA-577 adds paigasus-proto AND
# paigasus-proto-derive here — both, because the derive crate must publish first.
EXPECTED_PUBLISHABLE=("paigasus-kernel")
```

- [ ] **Step 6: Commit**

```bash
git add rs/release-plz.toml ci/publish-metadata/run.sh
git commit -m "feat(rs): make release-plz releasability per-package and group the kernel family"
```

---

### Task 7: `.github/workflows/release.yml` — the release-PR job

**Files:**
- Create: `.github/workflows/release.yml`

**Interfaces:**
- Consumes: `rs/release-plz.toml` from Task 6 and `ci/version-lockstep/run.sh --write` from Task 3.
- Produces: a live rolling release-PR job. **No `release` job, no tags, no publishing** — those land in SMA-577 … SMA-580.

- [ ] **Step 1: Write the workflow**

```yaml
# SPDX-License-Identifier: Apache-2.0
name: release

# Only the RELEASE-PR half is live (SMA-576). The release job — tags + registry publishes —
# lands with SMA-580 behind `vars.PAIGASUS_RELEASE_ENABLED`. Nothing here publishes.
on:
  push:
    branches:
      - main

# release-plz force-updates its PR branch on every run, so two rapid merges to main would
# race. Do NOT cancel in progress: a cancelled run can leave the branch half-written.
concurrency:
  group: release-pr
  cancel-in-progress: false

permissions:
  contents: read

jobs:
  release-pr:
    name: release PR
    runs-on: ubuntu-latest
    steps:
      - name: Checkout
        uses: actions/checkout@v5
        with:
          fetch-depth: 0
          # A PR opened with the default GITHUB_TOKEN does NOT trigger `pull_request`
          # workflows, so `moon ci` — the required check on the Protect main ruleset —
          # would never run and the release PR could never be merged. The App token is
          # what makes the PR mergeable.
          token: ${{ secrets.RELEASE_PLZ_TOKEN }}

      - name: Set up proto + Moon
        uses: moonrepo/setup-toolchain@v0

      - name: Install Moon-managed toolchains
        run: moon docker setup

      - name: Install JS workspace deps
        run: pnpm --dir ts install --frozen-lockfile

      # release-plz first, ALWAYS, then --write. release-plz owns every Cargo version and the
      # workspace dependency requirements; --write then brings the six sites it cannot reach
      # (pyproject/package.json/the dependency pin) plus the three regenerated ones into line.
      # Reversing the order would stamp the OLD version.
      - name: Open or update the release PR
        uses: release-plz/action@v0
        with:
          command: release-pr
          manifest_path: rs/Cargo.toml
        env:
          GITHUB_TOKEN: ${{ secrets.RELEASE_PLZ_TOKEN }}

      - name: Stamp the non-Cargo manifests
        run: bash ci/version-lockstep/run.sh --write

      - name: Commit the stamp onto the release PR branch
        run: |
          set -euo pipefail
          if git diff --quiet; then
            echo "version-lockstep: nothing to stamp"
            exit 0
          fi
          git config user.name  "paigasus-release[bot]"
          git config user.email "paigasus-release[bot]@users.noreply.github.com"
          BRANCH="$(git rev-parse --abbrev-ref HEAD)"
          git add -A
          git commit -m "chore(rs): stamp the non-Cargo manifests to the release version"
          git push origin "HEAD:$BRANCH"
```

- [ ] **Step 2: Lint the workflow**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:actionlint --force
```
Expected: PASS. `branches:` is written as a **block sequence** — the inline `branches: [main]` form makes the gate's extractor fail all four trigger keys loudly.

- [ ] **Step 3: Confirm `main` resolves as a branch**

`repo:actionlint` requires every wildcard-free `branches:` entry to resolve as `refs/remotes/origin/<name>`. `main` does. No `BRANCH_SKIP` entry is needed.

- [ ] **Step 4: Record the required secret**

The workflow needs a `RELEASE_PLZ_TOKEN` repository secret (a GitHub App installation token or fine-grained PAT with `contents: write` + `pull-requests: write`). **This is a human step — note it in the PR description.** Until it exists the job fails at checkout, which is loud and harmless: no other workflow depends on it.

- [ ] **Step 5: Commit**

```bash
git add .github/workflows/release.yml
git commit -m "feat(ci): add the live release-plz release-PR job"
```

---

### Task 8: Documentation

**Files:**
- Modify: `CLAUDE.md` (a Gotchas entry)
- Modify: `docs/superpowers/specs/2026-08-22-sma-407-release-activation-design.md` (status)

- [ ] **Step 1: Add the CLAUDE.md gotcha**

Append to the Gotchas section:

```markdown
- The kernel family (`paigasus-kernel` + the three binding crates + their `pyproject.toml` /
  `package.json` faces) carries **one version** across eighteen sites, asserted by
  `repo:version-lockstep` (`ci/version-lockstep/run.sh`). release-plz owns every Cargo
  `[package] version` — via per-package `version_group` — **and** the `[workspace.dependencies]`
  version requirements; both were measured against the pinned 0.3.158, as was the fact that
  `version_group` applies to crates whose Cargo manifest says `publish = false`. The script owns
  the six sites Cargo cannot reach (`--write`) and checks all eighteen, because a `version_group`
  that silently stopped applying would otherwise go unnoticed. Two of the sites drift SILENTLY
  without it: `py/uv.lock` (its `moon.yml` runs bare `uv sync`, not `--locked`) and the 26
  `bindingPackageVersion` guards in the committed napi glue (the codegen-drift gate covers only
  the three `**/generated` proto dirs).
- `rs/release-plz.toml` declares releasability **per package**, never workspace-wide. A
  `[workspace] release = false` makes release-plz hard-error (`no public packages found`), and
  simply deleting it is worse: `dependencies_update = true` cascades a patch bump into every
  transitive dependent, and Cargo's `publish = false` suppresses publishing but **not tagging** —
  so the first release would permanently tag most of the workspace. Per-package `release = false`
  removes a package from the proposal entirely. `paigasus-gateway` / `paigasus-iam` stay at
  `0.0.0` deliberately: their `env!("CARGO_PKG_VERSION")` feeds `ServiceInfo`, and ADR-0020 skew
  reporting is parked on that value (SMA-505 R7).
```

- [ ] **Step 2: Mark the spec's SMA-576 scope delivered**

Update the spec's status line to note SMA-576 is implemented and 577–580 remain.

- [ ] **Step 3: Run the full graph one final time**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site :input-liveness :promtool :observability-drift :nats-permissions :release-parity :release-parity-py :release-parity-ts :publish-metadata :version-lockstep --base origin/main --include-relations
```
Expected: all PASS.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md docs/superpowers/specs/2026-08-22-sma-407-release-activation-design.md
git commit -m "docs(repo): record the version-lockstep and per-package releasability rules"
```

---

## Deferred out of this plan, deliberately

- **The `ci/actionlint/run.sh` assertion guarding the release job's `if:` guard.** The SMA-576 issue lists it, but 576 ships **no `release` job** — there is nothing to guard. It must land with the issue that introduces that job (SMA-580), together with its guard-the-guard obligations: a new verdict function + self-test table, `SELF_TEST_COUNT` 9 → 10, and a whole-line `ACTIONLINT_SH_CALL_SITES` entry.
- The ADR-0011 amendment (spec §13) — it records decisions spanning all five children; it lands with the last one that settles them.
