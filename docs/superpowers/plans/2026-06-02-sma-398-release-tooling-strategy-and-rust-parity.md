# SMA-398 Release-Tooling Strategy + Rust Parity Slice — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Land a dormant `release-plz` configuration plus a tool-agnostic, multi-crate dry-run semver-parity smoke test that asserts the commit→semver classification contract, satisfying SMA-398's AC for the Rust ecosystem.

**Architecture:** A bash harness (`ci/release-parity/`) builds a throwaway two-crate Cargo workspace in a temp dir, derives its `release-plz.toml` from the real `rs/release-plz.toml` (so it exercises production classification settings), replays each row of a synthetic-commit expectation table, runs `release-plz update` against the disposable fixture, and asserts the resulting per-crate version. A per-ecosystem module behind a fixed function interface (`ecosystem::build_fixture / apply_commit / run_update / version`) lets the Python/TS slices (E3/E4) reuse the orchestrator. The check runs as a Moon task on the `repo` project, wired into `moon ci` per-PR on the affected graph.

**Tech Stack:** bash, Moon 2.2.5, proto (vendored TOML plugin), release-plz `0.3.158` (cargo-dist binary), Cargo workspace fixtures.

**Spec:** `docs/superpowers/specs/2026-06-02-sma-398-release-tooling-strategy-and-rust-parity-design.md` (S1–S6, F1–F5).

---

## File Structure

| File | Create/Modify | Responsibility |
|------|---------------|----------------|
| `.proto/plugins/release-plz.toml` | Create | Vendored proto TOML plugin resolving release-plz GitHub release binaries (mirrors `buf.toml`/`lefthook.toml`). |
| `.prototools` | Modify | Pin `release-plz = "0.3.158"` + register the plugin. |
| `rs/release-plz.toml` | Create | **Dormant** release config; the single source of the classification settings the harness derives from. |
| `ci/release-parity/cases.tsv` | Create | Expectation table as data (id, subject, footer, 0.x, 1.x, discriminating). |
| `ci/release-parity/ecosystems/release-plz.sh` | Create | Rust/release-plz module: fixture build + config derivation + commit + update + version read. |
| `ci/release-parity/run.sh` | Create | Tool-agnostic orchestrator: case loop, attribution assertion, negative control, exit code. |
| `ci/release-parity/README.md` | Create | Records the SMA-385 (scope≠path) rationale, the derived-config invariant, and the 0.x degeneracy. |
| `moon.yml` (root, `repo` project) | Modify | Add the `release-parity` task with explicit inputs. |
| `.github/workflows/ci.yml` | Modify | Add `:release-parity` to the `moon ci` target array. |
| Notion ADR-00XX | Create | Strategy of record (S1–S6). |
| Linear E3/E4/E-activate | Create | Decomposition follow-ups. |

---

## Task 1: Author ADR-00XX (strategy of record, before code)

CLAUDE.md requires a Notion ADR for significant choices before code. This records S1–S6 and **refines** (does not silently reverse) the Polyglot Monorepo Scoping doc.

**Files:** Notion (Architecture Decision Records collection).

- [ ] **Step 1: Find the next ADR number**

Use the Notion MCP: fetch the "Architecture Decision Records" index page (`368830e8fbaa816cb411c7ee1682c175`) and read the highest existing `ADR-00NN`. The new one is the next integer (the spec calls it ADR-00XX as a placeholder).

- [ ] **Step 2: Create the ADR page**

Create a child page under the ADR index titled `ADR-00NN: Polyglot versioning & release strategy`, with sections:
- **Context:** every paigasus-core artifact is a `0.0.0` stub; the Scoping doc §3 #4 / §4 mandate lockstep for the kernel/proto artifacts; SMA-371 AC-E needs a commit→semver parity gate.
- **Decision:** the S1–S6 table verbatim from the spec (hybrid versioning; per-language tools with kernel/proto as Rust byproducts; `0.1.0` floor + tool-owned tags; dormant-until-real; scope≠file-path; the 0.x contract with `always_bump_minor_for_0 = true`).
- **Relationship to existing docs:** this ADR **refines** Scoping §3 #4 / §4 — it scopes their lockstep mandate explicitly to the kernel/proto families and records per-package independence for unrelated packages. (Do **not** mark them superseded.)
- **Consequences:** lockstep propagation mechanism (maturin/napi) deferred to E-activate; parity gate (SMA-398) is the enforcement.

- [ ] **Step 3: Back-reference from the Scoping doc**

Add a one-line callout near Scoping §4 pointing to the new ADR ("Versioning strategy refined in ADR-00NN").

- [ ] **Step 4: Record the number in the spec**

Replace `ADR-00XX` with the allocated `ADR-00NN` in the spec file, then commit:

```bash
git add docs/superpowers/specs/2026-06-02-sma-398-release-tooling-strategy-and-rust-parity-design.md
git commit -m "docs(release): record ADR-00NN number in SMA-398 spec"
```

---

## Task 2: Vendor the release-plz proto plugin + pin the version

**Files:**
- Create: `.proto/plugins/release-plz.toml`
- Modify: `.prototools`

- [ ] **Step 1: Write the vendored plugin**

Create `.proto/plugins/release-plz.toml` (mirrors `buf.toml`; cargo-dist ships `.tar.gz` archives and **no** checksum file, so checksum verification is omitted with a note):

```toml
# Vendored proto TOML plugin for the release-plz CLI (SMA-398).
#
# Resolves official release-plz GitHub release binaries (cargo-dist tarballs).
# Used only by the ci/release-parity harness, not general dev/CI tooling.
#
# NOTE: release-plz publishes no per-asset checksum file (unlike buf/lefthook),
# so resolution is HTTPS-from-GitHub only. The exact version is pinned in
# .prototools, so the resolved asset is deterministic.
# NOTE: cargo-dist uses Rust target arches (x86_64/aarch64) = proto's default
# {arch} tokens, so no [install.arch] remap is needed. macOS ships aarch64 only.

name = "release-plz"
type = "cli"

[platform.linux]
download-file = "release-plz-{arch}-unknown-linux-gnu.tar.gz"

[platform.macos]
download-file = "release-plz-{arch}-apple-darwin.tar.gz"

[platform.windows]
download-file = "release-plz-{arch}-pc-windows-msvc.zip"

[install]
download-url = "https://github.com/release-plz/release-plz/releases/download/release-plz-v{version}/{download_file}"

[resolve]
git-url = "https://github.com/release-plz/release-plz"
```

- [ ] **Step 2: Pin the version + register the plugin in `.prototools`**

Add the version line under the existing pins and the plugin under `[plugins]`:

```toml
buf = "1.70.0"
lefthook = "2.1.8"
moon = "2.2.5"
release-plz = "0.3.158"

[plugins]
buf = "file://./.proto/plugins/buf.toml"
lefthook = "file://./.proto/plugins/lefthook.toml"
release-plz = "file://./.proto/plugins/release-plz.toml"
```

- [ ] **Step 3: Verify proto installs and runs release-plz**

Run: `proto install release-plz && release-plz --version`
Expected: prints `release-plz 0.3.158`.

If proto installs the archive but cannot locate the binary inside it, inspect the layout (`tar tzf "$(ls ~/.proto/tools/release-plz/0.3.158/*.tar.gz 2>/dev/null)"` or the cached download) and add a per-platform `bin-path = "<inner>/release-plz"` to each `[platform.*]` section, then re-run. This is the one expected adjustment point for a cargo-dist tarball.

- [ ] **Step 4: Commit**

```bash
git add .proto/plugins/release-plz.toml .prototools
git commit -m "build(release): vendor release-plz proto plugin pinned at 0.3.158 (SMA-398)"
```

---

## Task 3: Dormant `rs/release-plz.toml`

**Files:**
- Create: `rs/release-plz.toml`

- [ ] **Step 1: Write the dormant config**

The harness greps `always_bump_minor_for_0` from this file (Task 5), so it MUST be present and literal.

```toml
# Dormant release-plz configuration (SMA-398).
#
# Present so the SMA-398 parity harness derives its fixture config from the REAL
# classification settings (ci/release-parity). NO release-plz workflow is wired
# yet: activation (0.0.0 -> 0.1.0, live release PRs/tags) is deferred to
# E-activate. Real crates stay `publish = false`; this file changes no publish
# state and cuts no tags.

[workspace]
# Conventional-Commit -> semver classification (the contract SMA-398 asserts).
# In 0.x: fix -> patch, feat -> minor, breaking (! or BREAKING CHANGE) -> minor.
# always_bump_minor_for_0 keeps feat distinguishable from fix in 0.x.
always_bump_minor_for_0 = true
dependencies_update = true

[workspace.changelog]
sort_commits = "newest"
```

- [ ] **Step 2: Verify dormancy — no release workflow exists**

Run: `test ! -f .github/workflows/release-plz.yml && ! grep -rl 'release-plz' .github/workflows/ 2>/dev/null && echo DORMANT-OK`
Expected: prints `DORMANT-OK` (no workflow triggers release-plz).

- [ ] **Step 3: Commit**

```bash
git add rs/release-plz.toml
git commit -m "feat(release): dormant release-plz config + 0.x classification settings (SMA-398)"
```

---

## Task 4: Expectation table + README (the data)

**Files:**
- Create: `ci/release-parity/cases.tsv`
- Create: `ci/release-parity/README.md`

- [ ] **Step 1: Write `cases.tsv`**

**Tab-separated** (literal tabs between fields). Columns: `id  subject  footer  expected_0x  expected_1x  discriminating`. Use `-` for "no footer". The harness asserts `expected_0x`; `expected_1x` is carried but unasserted (staged for the 1.0 transition, F2).

```
# id	subject	footer	expected_0x	expected_1x	discriminating
fix	fix: tweak crate a	-	0.1.1	1.0.1	no
feat	feat: add to crate a	-	0.2.0	1.1.0	no
fix-bang	fix!: change crate a	-	0.2.0	2.0.0	yes
fix-footer	fix: change crate a	BREAKING CHANGE: drop old behavior	0.2.0	2.0.0	yes
feat-bang	feat!: change crate a	-	0.2.0	2.0.0	no
```

- [ ] **Step 2: Write `README.md`**

```markdown
# release→semver parity harness (SMA-398)

Asserts the commit→semver classification contract by dry-running the configured
release tool over synthetic Conventional Commits in a disposable fixture repo.

## Why a multi-crate fixture (F4 / SMA-385)
Release tools map commits to packages by **changed file path**, not commit scope
(SMA-385's root cause). The fixture has two independent crates; every case
touches crate `a` and asserts `a` bumps while `b` stays at baseline — testing
path→package attribution, not just bump magnitude. Do NOT "simplify" this into a
single-crate fixture or scope-only commits: that silently stops testing the bug
class this harness exists for.

## Why the fixture config is derived, not copied (F3)
The fixture `release-plz.toml` is generated from the real `rs/release-plz.toml`
(the classification keys are grepped out, semver-check forced off). A hand-copied
config would drift and validate the wrong settings.

## 0.x degeneracy (F2)
With `always_bump_minor_for_0 = true`, `feat:` already bumps minor, so `feat!:`
and `feat:`+footer are NON-discriminating in 0.x (all = 0.2.0). The breaking
marker is only testable on a **patch-base** (`fix!:`, `fix:`+footer): a tool that
drops the marker yields 0.1.1, which the harness catches. Breaking-vs-feature by
magnitude (breaking → major) only becomes discriminating at 1.0 — the 1.x column
in cases.tsv is staged for that transition.
```

- [ ] **Step 3: Commit**

```bash
git add ci/release-parity/cases.tsv ci/release-parity/README.md
git commit -m "feat(ci): SMA-398 parity expectation table + harness rationale"
```

---

## Task 5: release-plz ecosystem module

**Files:**
- Create: `ci/release-parity/ecosystems/release-plz.sh`

- [ ] **Step 1: Write the module**

Provides the four functions the orchestrator calls. Builds a two-independent-crate workspace, derives the fixture config from the real one, applies a case commit touching crate `a`, runs `release-plz update`, and reads a crate's version.

```bash
#!/usr/bin/env bash
# release-plz ecosystem module for the SMA-398 parity harness.
# Interface: ecosystem::build_fixture / apply_commit / run_update / version
set -euo pipefail

A_CRATE="paigasus-release-parity-a"
B_CRATE="paigasus-release-parity-b"

ecosystem::_crate_dir() { # dir slot(a|b) -> path
  case "$2" in
    a) printf '%s/crates/%s' "$1" "$A_CRATE" ;;
    b) printf '%s/crates/%s' "$1" "$B_CRATE" ;;
    *) echo "bad slot: $2" >&2; return 1 ;;
  esac
}

ecosystem::_derive_config() { # real_toml out_toml   (F3)
  local real="$1" out="$2" bump
  bump="$(grep -E '^[[:space:]]*always_bump_minor_for_0[[:space:]]*=' "$real" || true)"
  if [ -z "$bump" ]; then
    echo "FATAL: rs/release-plz.toml lacks always_bump_minor_for_0 — parity would test stale settings" >&2
    return 1
  fi
  {
    echo "[workspace]"
    printf '%s\n' "${bump#"${bump%%[![:space:]]*}"}"   # left-trimmed
    echo "semver_check = false"                          # orthogonal to classification
  } >"$out"
}

ecosystem::build_fixture() { # dir real_release_plz_toml
  local dir="$1" real="$2" c
  mkdir -p "$dir/crates/$A_CRATE/src" "$dir/crates/$B_CRATE/src"
  cat >"$dir/Cargo.toml" <<'EOF'
[workspace]
resolver = "3"
members = ["crates/*"]
EOF
  for c in "$A_CRATE" "$B_CRATE"; do
    cat >"$dir/crates/$c/Cargo.toml" <<EOF
[package]
name = "$c"
version = "0.1.0"
edition = "2024"
publish = false
EOF
    echo "// seed" >"$dir/crates/$c/src/lib.rs"
  done
  ecosystem::_derive_config "$real" "$dir/release-plz.toml"
  (
    cd "$dir"
    git init -q
    git config user.email "parity@example.com"
    git config user.name "parity"
    git add -A
    git commit -qm "chore: seed fixture"
    git tag "$A_CRATE-v0.1.0"   # release-plz default workspace tag pattern
    git tag "$B_CRATE-v0.1.0"
  )
}

ecosystem::apply_commit() { # dir slot subject footer
  local dir="$1" slot="$2" subject="$3" footer="$4" cdir
  cdir="$(ecosystem::_crate_dir "$dir" "$slot")"
  printf '// change for: %s\n' "$subject" >>"$cdir/src/lib.rs"
  (
    cd "$dir"
    git add -A
    if [ "$footer" = "-" ]; then
      git commit -qm "$subject"
    else
      git commit -qm "$subject" -m "$footer"
    fi
  )
}

ecosystem::run_update() { # dir
  # Disposable fixture: let `update` write, then read the result. Offline so the
  # crates.io index isn't consulted for the (nonexistent) fixture crate names.
  ( cd "$1" && CARGO_NET_OFFLINE=true release-plz update >/dev/null 2>&1 )
}

ecosystem::version() { # dir slot -> version string
  local cdir
  cdir="$(ecosystem::_crate_dir "$1" "$2")"
  grep -m1 -E '^version[[:space:]]*=' "$cdir/Cargo.toml" | sed -E 's/.*"([^"]+)".*/\1/'
}
```

- [ ] **Step 2: Smoke-test the module by hand**

Run:
```bash
bash -c '
set -euo pipefail
source ci/release-parity/ecosystems/release-plz.sh
d=$(mktemp -d); trap "rm -rf $d" EXIT
ecosystem::build_fixture "$d" rs/release-plz.toml
ecosystem::apply_commit "$d" a "feat: add to crate a" "-"
ecosystem::run_update "$d"
echo "a=$(ecosystem::version "$d" a) b=$(ecosystem::version "$d" b)"
'
```
Expected: `a=0.2.0 b=0.1.0` (feat → minor in 0.x with the knob on; crate b untouched).

If `release-plz update` errors offline, drop `CARGO_NET_OFFLINE=true` from `ecosystem::run_update` (CI has network; the crates.io index lookup for a nonexistent crate is deterministic) and re-run. If the version doesn't move, confirm the baseline tags match release-plz's default workspace tag pattern (`<crate>-v<version>`) and that `semver_check = false` is in the derived config.

- [ ] **Step 3: Commit**

```bash
git add ci/release-parity/ecosystems/release-plz.sh
git commit -m "feat(ci): release-plz ecosystem module for SMA-398 parity harness"
```

---

## Task 6: Orchestrator `run.sh` (case loop + attribution + negative control)

**Files:**
- Create: `ci/release-parity/run.sh`

- [ ] **Step 1: Write the orchestrator**

```bash
#!/usr/bin/env bash
# SMA-398 release->semver parity harness (tool-agnostic core).
# usage: run.sh [--ecosystem NAME] [--negative-control]
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ECOSYSTEM="release-plz"
NEGATIVE=0
while [ $# -gt 0 ]; do
  case "$1" in
    --ecosystem) ECOSYSTEM="$2"; shift 2 ;;
    --negative-control) NEGATIVE=1; shift ;;
    -h|--help) echo "usage: run.sh [--ecosystem NAME] [--negative-control]"; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

# shellcheck source=ci/release-parity/ecosystems/release-plz.sh
source "$HERE/ecosystems/$ECOSYSTEM.sh"

REPO_ROOT="$(cd "$HERE/../.." && pwd)"
REAL_TOML="$REPO_ROOT/rs/release-plz.toml"
BASELINE="0.1.0"
CASES="$HERE/cases.tsv"

# returns 0 iff crate a bumps to $expected AND crate b stays at baseline.
check_case() { # id subject footer expected
  local id="$1" subject="$2" footer="$3" expected="$4" dir got_a got_b
  dir="$(mktemp -d)"
  # shellcheck disable=SC2064
  trap "rm -rf '$dir'" RETURN
  ecosystem::build_fixture "$dir" "$REAL_TOML"
  ecosystem::apply_commit "$dir" a "$subject" "$footer"
  ecosystem::run_update "$dir"
  got_a="$(ecosystem::version "$dir" a)"
  got_b="$(ecosystem::version "$dir" b)"
  if [ "$got_a" = "$expected" ] && [ "$got_b" = "$BASELINE" ]; then
    printf 'PASS  %-12s a=%s b=%s\n' "$id" "$got_a" "$got_b"
    return 0
  fi
  printf 'FAIL  %-12s a:exp=%s got=%s | b:exp=%s got=%s\n' \
    "$id" "$expected" "$got_a" "$BASELINE" "$got_b" >&2
  return 1
}

if [ "$NEGATIVE" = 1 ]; then
  echo "== negative control: feeding a deliberately wrong expectation =="
  if check_case "neg-fix-bang" "fix!: deliberately wrong" "-" "0.1.1"; then
    echo "negative-control FAILED: harness accepted a wrong expectation" >&2
    exit 1
  fi
  echo "negative-control OK: harness reported red as expected"
  exit 0
fi

rc=0
while IFS=$'\t' read -r id subject footer expected_0x _expected_1x _discr; do
  case "$id" in ''|'#'*) continue ;; esac
  check_case "$id" "$subject" "$footer" "$expected_0x" || rc=1
done <"$CASES"

if [ "$rc" = 0 ]; then echo "== all parity cases passed =="; else echo "== parity FAILURES (see above) ==" >&2; fi
exit "$rc"
```

- [ ] **Step 2: Make both scripts executable**

```bash
chmod +x ci/release-parity/run.sh ci/release-parity/ecosystems/release-plz.sh
```

- [ ] **Step 3: Run the harness — expect all cases green**

Run: `ci/release-parity/run.sh`
Expected: `PASS` for `fix` (0.1.1), `feat` (0.2.0), `fix-bang` (0.2.0), `fix-footer` (0.2.0), `feat-bang` (0.2.0), each with `b=0.1.0`, then `== all parity cases passed ==`, exit 0.

- [ ] **Step 4: Run the negative control — expect it to report red, then pass**

Run: `ci/release-parity/run.sh --negative-control`
Expected: a `FAIL neg-fix-bang` line, then `negative-control OK: harness reported red as expected`, exit 0. (Proves the harness has teeth — F2 anti-false-green.)

- [ ] **Step 5: Commit**

```bash
git add ci/release-parity/run.sh ci/release-parity/ecosystems/release-plz.sh
git commit -m "feat(ci): SMA-398 parity orchestrator with attribution + negative control"
```

---

## Task 7: Moon task `release-parity`

**Files:**
- Modify: `moon.yml` (root, `repo` project)

- [ ] **Step 1: Add the task**

Append under the existing `tasks:` map in the root `moon.yml` (the `repo`, `language: bash` project). Explicit `inputs` keep it affected ONLY by the release config + harness + pin (so `.prototools` pin bumps re-run it — the cadence decision, §9):

```yaml
  release-parity:
    description: 'Dry-run release-plz over synthetic commits; assert commit->semver parity (SMA-398).'
    script: 'ci/release-parity/run.sh'
    toolchain: 'system'
    inputs:
      - 'ci/release-parity/**/*'
      - 'rs/release-plz.toml'
      - '.prototools'
      - '.proto/plugins/release-plz.toml'
```

(Do not set `options.runInCI: false` — unlike `install-hooks`, this task must run in CI.)

- [ ] **Step 2: Run via Moon**

Run: `moon run repo:release-parity`
Expected: the harness runs and prints `== all parity cases passed ==`; Moon reports the task succeeded.

- [ ] **Step 3: Confirm affected scoping**

Run: `moon query tasks --affected --json` after `touch ci/release-parity/cases.tsv` (or reason from inputs): `repo:release-parity` should be affected by a change under `ci/release-parity/`, `rs/release-plz.toml`, `.prototools`, or `.proto/plugins/release-plz.toml`, and not by unrelated files.

- [ ] **Step 4: Commit**

```bash
git add moon.yml
git commit -m "feat(ci): add repo:release-parity Moon task (SMA-398)"
```

---

## Task 8: Wire into CI (per-PR affected)

**Files:**
- Modify: `.github/workflows/ci.yml` (the `moon ci` target array, ~line 138)

- [ ] **Step 1: Add `:release-parity` to the target array**

Change:

```yaml
          T=(:build :test :lint :fmt :typecheck :breaking)
```

to:

```yaml
          T=(:build :test :lint :fmt :typecheck :breaking :release-parity)
```

(The `:release-parity` target resolves to `repo:release-parity`; `moon ci --base origin/main` runs it only when its declared inputs are in the diff.)

- [ ] **Step 2: Verify release-plz resolves in a CI-like shell**

`ci.yml` already runs `proto install` (Task 2 makes that install release-plz). Confirm the task can see it: with proto shims on `PATH`,

Run: `moon run repo:release-parity`
Expected: green (same as Task 7 Step 2). If release-plz is not found under `toolchain: 'system'`, confirm `~/.proto/shims` / `~/.proto/bin` are on `PATH` (the env this repo documents for proto-managed tools).

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/ci.yml
git commit -m "feat(ci): run release-parity in the moon ci affected graph (SMA-398)"
```

---

## Task 9: Create decomposition follow-up issues

**Files:** Linear (via MCP `save_issue`).

- [ ] **Step 1: Create E3 (Python)**

Title: `CI: python-semantic-release dormant config + Py semver-parity adapter`.
Body: dormant python-semantic-release config for `paigasus-ml` + `paigasus-workflows` (the language-native Py packages only — kernel/proto wrappers are maturin byproducts, out of scope); add `ci/release-parity/ecosystems/python-semantic-release.sh` implementing the same `ecosystem::*` interface; reuse `cases.tsv`. `relatedTo` SMA-398; `blockedBy` ADR-00NN.
Team: `Sven Maschek`. Project: `Paigasus Polyglot`. Labels: `area:ci`.

- [ ] **Step 2: Create E4 (TypeScript)**

Title: `CI: semantic-release dormant config + TS semver-parity adapter`.
Body: dormant semantic-release (+ monorepo path-filtering) config for `@paigasus/sdk` + `@paigasus/ui` only; add `ci/release-parity/ecosystems/semantic-release.sh` against the same interface. `relatedTo` SMA-398; `blockedBy` ADR-00NN.

- [ ] **Step 3: Create E-activate**

Title: `Release activation: 0.0.0 -> 0.1.0 floor + kernel/proto lockstep wiring + live workflows`.
Body: the riskiest activation step (F5). Move each package off `0.0.0` to the `0.1.0` floor and let release-plz cut the FIRST tag without a hand-placed tag (the SMA-385 metadata-loss trap); wire maturin/napi so kernel/proto Py/TS artifacts derive the Rust crate version (S1 lockstep); enable the live release workflows. `relatedTo` SMA-398; `blockedBy` ADR-00NN.

- [ ] **Step 4: Confirm links**

Verify each new issue shows `relatedTo SMA-398` and `blockedBy ADR-00NN` (or a note if ADRs aren't tracked as issues).

---

## Self-Review

**1. Spec coverage**

| Spec item | Task |
|-----------|------|
| ADR-00XX (S1–S6), refines Scoping doc | Task 1 |
| Pin release-plz / `.prototools` (§8) | Task 2 |
| Dormant `rs/release-plz.toml` (S4/§3/§4) | Task 3 |
| `always_bump_minor_for_0 = true` (S6) | Task 3 |
| Expectation table as data, patch-base discriminating cases (S6/F2) | Task 4 (`cases.tsv`) |
| 1.x columns staged unasserted (F2) | Task 4 (`expected_1x`), Task 6 (reads `expected_0x` only) |
| scope≠path, *tested* via attribution (S5/F4) | Task 5 (two-crate fixture) + Task 6 (`b` baseline assert) |
| Fixture config derived from real (F3) | Task 5 (`_derive_config`) |
| Multi-crate fixture (F4) | Task 5 |
| Tool-agnostic harness + ecosystem seam (§7) | Task 5 (module) + Task 6 (orchestrator) |
| Negative control / anti-false-green (§7) | Task 6 Step 4 |
| Moon task, inputs incl. `.prototools` (§9) | Task 7 |
| CI per-PR affected, no nightly (§9) | Task 8 |
| Decomposition E3/E4/E-activate; E3/E4 govern language-native only (S2/§10) | Task 9 |
| First-activation `0.0.0→0.1.0` routed out (F5/§11) | Task 9 Step 3 |
| Dormancy verifiable (S4) | Task 3 Step 2 |

No spec item is unmapped.

**2. Placeholder scan**

`ADR-00XX` → `ADR-00NN` is resolved by Task 1 (allocate the real number) and propagated. The three verify-and-adjust points (proto tarball bin-path in Task 2; offline/tag-pattern in Task 5) are tool-vendoring gates with the exact debug command given, not hand-waved core logic. No `TBD`/"handle edge cases"/"similar to" placeholders.

**3. Type/interface consistency**

The `ecosystem::` interface is consistent across Task 5 (defines `build_fixture`, `apply_commit`, `run_update`, `version`, `_crate_dir`, `_derive_config`) and Task 6 (calls exactly those). `cases.tsv` columns (Task 4) match the `read` in Task 6 (`id subject footer expected_0x _expected_1x _discr`). Crate names (`paigasus-release-parity-a/-b`), baseline (`0.1.0`), and tag pattern (`<crate>-v0.1.0`) are identical in Tasks 5 and 6. The Moon target `repo:release-parity` (Task 7) matches the `:release-parity` CI target (Task 8).

---

## Execution Handoff

Plan complete. Two execution options:

1. **Subagent-Driven (recommended)** — fresh subagent per task, review between tasks.
2. **Inline Execution** — execute tasks in this session with checkpoints.
