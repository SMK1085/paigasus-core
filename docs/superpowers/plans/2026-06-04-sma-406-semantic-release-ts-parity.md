# SMA-406 — semantic-release dormant config + TS semver-parity adapter — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a dormant semantic-release config to `@paigasus/sdk` + `@paigasus/ui` and a TypeScript adapter to the `ci/release-parity/` harness, so `run.sh --ecosystem semantic-release` asserts the commit→semver contract — including the *documented* 0.x divergence (breaking → `1.0.0`).

**Architecture:** semantic-release classifies natively (no version-aware 0.x clamp), so the adapter asserts its strict-semver result via a new generic `ecosystem::expected` hook in `run.sh` (red on drift). Per-package isolation is provided by a small **in-repo path-filter** semantic-release plugin (no third-party monorepo plugin). The fixture is one git repo with two package dirs, driven through that same filter. The adapter computes versions via the semantic-release **JS API** (a tiny in-repo runner), not by scraping the CLI log.

**Tech Stack:** bash (harness), Node ESM (`.mjs` plugin + runner), `semantic-release` + `@semantic-release/commit-analyzer` (pnpm dev-deps in `ts/`), Moon task wiring.

**Spec:** `docs/superpowers/specs/2026-06-04-sma-406-semantic-release-ts-parity-design.md`. **ADR-0011** amended 2026-06-04 (S6 documented-exception clause). Lifecycle decision routed to **SMA-407**.

> **Two deliberate refinements over the spec (both reduce risk; flagged for sync):**
> 1. **JS API, not CLI+parse.** `run_update` invokes a node runner using `semanticRelease({dryRun:true, ci:false}, {cwd})`, which returns `result.nextRelease.version` (or `false` for no release). This removes spec Risk 3 (dry-run log parsing) and the no-bump git-log fallback, and Risk 2 (binary/plugin resolution) — the runner lives under `ts/`, so `import 'semantic-release'` resolves from `ts/node_modules` with no symlink/`require.resolve` gymnastics.
> 2. **Single delegating plugin.** The `plugins` array is `["<path-filter>"]` only (NOT `["<path-filter>", "@semantic-release/commit-analyzer"]` as the spec sketched). semantic-release runs `analyzeCommits` for every listed plugin and takes the **max** release type; listing commit-analyzer separately would classify the *unfiltered* commit set and defeat the path filter. The path-filter imports and delegates to commit-analyzer on the filtered commits.

---

## File Structure

| File | Responsibility | Action |
|------|----------------|--------|
| `ts/package.json` | add `semantic-release` + `@semantic-release/commit-analyzer` dev-deps (catalog refs) | Modify |
| `ts/pnpm-workspace.yaml` | catalog pins for the two new deps | Modify |
| `ts/pnpm-lock.yaml` | resolved lock (pnpm-generated) | Modify |
| `ts/tooling/semantic-release-path-filter.mjs` | the in-repo path-filter plugin (`analyzeCommits`: git-log path filter → delegate to commit-analyzer) | Create |
| `ts/tooling/semantic-release-next-version.mjs` | JS-API runner: dry-run one package, print next version (or empty) | Create |
| `ts/packages/paigasus-sdk/.releaserc.json` | dormant production config for `@paigasus/sdk` | Create |
| `ts/packages/paigasus-ui/.releaserc.json` | dormant production config for `@paigasus/ui` | Create |
| `ci/release-parity/run.sh` | add the generic `ecosystem::expected` seam (`resolve_expected`) | Modify |
| `ci/release-parity/ecosystems/semantic-release.sh` | the adapter: `build_fixture`/`apply_commit`/`run_update`/`version`/`ecosystem::expected` + F3 derivation/guards | Create |
| `ci/release-parity/README.md` | semantic-release section (divergence table, path-filter, what slot `b` tests) | Modify |
| `moon.yml` (root, `repo` project) | `release-parity-ts` task | Modify |
| `.github/workflows/ci.yml` | add `:release-parity-ts` to the `T=(…)` target array | Modify |

**Note:** keep `ts/tooling/` free of `package.json`/`moon.yml` so Moon does not treat it as a project. The `.mjs` files must pass `moon run ts:lint`/`ts:fmt` (add an `eslint.config.js` override or ignore entry for `tooling/**` if the React-oriented rules misfire — see Task 2).

---

## Task 1: Add semantic-release dev-dependencies to `ts/`

**Files:**
- Modify: `ts/pnpm-workspace.yaml` (catalog)
- Modify: `ts/package.json` (root workspace devDependencies)
- Modify: `ts/pnpm-lock.yaml` (pnpm-generated)

- [ ] **Step 1: Add catalog pins**

In `ts/pnpm-workspace.yaml`, under the `catalog:` map, add (in a new "Release tooling (SMA-406)" comment group, mirroring the existing comment-grouped style):

```yaml
  # Release tooling (SMA-406 — dormant semantic-release + parity adapter)
  semantic-release: ^25.0.0
  '@semantic-release/commit-analyzer': ^13.0.0
```

> Use the actual latest-stable majors at implementation time. Verify on npm: `npm view semantic-release version` and `npm view @semantic-release/commit-analyzer version`, then set the caret range to that major. Record the resolved versions in the commit message (spec Risk 6).

- [ ] **Step 2: Reference them from the root workspace package**

In `ts/package.json`, add to `devDependencies` (alphabetical with the others):

```json
    "@semantic-release/commit-analyzer": "catalog:",
    "semantic-release": "catalog:",
```

- [ ] **Step 3: Install (generates the lockfile entries)**

Run: `pnpm --dir ts install`
Expected: completes; `ts/pnpm-lock.yaml` gains `semantic-release` + `@semantic-release/commit-analyzer` (and their transitive deps). No `ERR_PNPM_*`.

- [ ] **Step 4: Verify the binary resolves**

Run: `pnpm --dir ts exec semantic-release --version`
Expected: prints a version like `25.x.x` (confirms install + binary on the workspace path).

- [ ] **Step 5: Verify commit-analyzer is importable as ESM (interop check)**

Run:
```bash
node --input-type=module -e "import a from '@semantic-release/commit-analyzer'; console.log(typeof a.analyzeCommits)" \
  --eval-paths 2>/dev/null || \
( cd ts && node --input-type=module -e "import a from '@semantic-release/commit-analyzer'; console.log(typeof a.analyzeCommits)" )
```
Expected: prints `function`. (If the default import is undefined, the fallback is a namespace import `import * as a` — note which works; Task 2 uses it.)

- [ ] **Step 6: Commit**

```bash
git add ts/package.json ts/pnpm-workspace.yaml ts/pnpm-lock.yaml
git commit -m "build(ts): add semantic-release + commit-analyzer dev-deps (SMA-406)"
```

---

## Task 2: In-repo path-filter plugin

**Files:**
- Create: `ts/tooling/semantic-release-path-filter.mjs`
- Possibly modify: `ts/eslint.config.js` (ignore/override for `tooling/**`)

- [ ] **Step 1: Write the plugin**

Create `ts/tooling/semantic-release-path-filter.mjs`:

```js
// SPDX-License-Identifier: Apache-2.0
//
// In-repo semantic-release path-filter (SMA-406). Replaces the abandoned
// `semantic-release-monorepo` plugin: restricts the analyzed commits to those
// touching the current package's directory (cwd) before delegating
// classification to `@semantic-release/commit-analyzer`.
//
// IMPORTANT: this must be the ONLY `analyzeCommits` provider in the `plugins`
// array. semantic-release runs `analyzeCommits` for every plugin and takes the
// max release type, so also listing `@semantic-release/commit-analyzer`
// separately would classify the UNFILTERED commit set and defeat the filter.
import { execFileSync } from 'node:child_process';
import commitAnalyzer from '@semantic-release/commit-analyzer';

export async function analyzeCommits(pluginConfig, context) {
  const { cwd, commits } = context;
  // semantic-release commit objects carry no file list, so ask git which commits
  // touched this package dir (cwd) and intersect with the since-last-release set.
  const touched = new Set(
    execFileSync('git', ['log', '--format=%H', '--', '.'], { cwd, encoding: 'utf8' })
      .split('\n')
      .filter(Boolean),
  );
  const filtered = commits.filter((commit) => touched.has(commit.hash));
  return commitAnalyzer.analyzeCommits(pluginConfig, { ...context, commits: filtered });
}
```

> If Task 1 Step 5 showed the default import is undefined, change line 13 to `import * as commitAnalyzer from '@semantic-release/commit-analyzer';`.

- [ ] **Step 2: Smoke-check the module loads + exports `analyzeCommits`**

Run:
```bash
cd ts && node --input-type=module -e "import * as m from './tooling/semantic-release-path-filter.mjs'; if (typeof m.analyzeCommits!=='function') { console.error('FAIL'); process.exit(1)}; console.log('OK')"
```
Expected: prints `OK` (confirms the file parses, the commit-analyzer import resolves, and the export exists). This is the unit-level gate; full behavior is exercised by the harness in Task 6.

- [ ] **Step 3: Ensure lint/format pass for the new file**

Run: `pnpm --dir ts exec prettier --write tooling/semantic-release-path-filter.mjs && moon run ts:lint`
Expected: prettier formats it; lint passes. If lint errors on the Node-script idioms (e.g. React/JSX rules), add to `ts/eslint.config.js` a targeted block, e.g.:

```js
  { files: ['tooling/**/*.mjs'], languageOptions: { globals: { process: 'readonly' } }, rules: {} },
```
or add `'tooling/**'` to the global `ignores` if the project lints only app/package source. Re-run until clean.

- [ ] **Step 4: Commit**

```bash
git add ts/tooling/semantic-release-path-filter.mjs ts/eslint.config.js
git commit -m "feat(release): in-repo semantic-release path-filter plugin (SMA-406)"
```

---

## Task 3: JS-API next-version runner

**Files:**
- Create: `ts/tooling/semantic-release-next-version.mjs`

- [ ] **Step 1: Write the runner**

Create `ts/tooling/semantic-release-next-version.mjs`:

```js
// SPDX-License-Identifier: Apache-2.0
//
// Dry-run a single package through semantic-release via its JS API and print the
// computed next version to stdout (empty string if no release is due). Used by
// the SMA-406 parity adapter. The JS API returns the structured next release, so
// we never scrape the human-readable CLI log. semantic-release's own logs are
// routed to stderr so stdout carries ONLY the version.
import semanticRelease from 'semantic-release';

const cwd = process.argv[2];
if (!cwd) {
  process.stderr.write('usage: semantic-release-next-version.mjs <package-dir>\n');
  process.exit(2);
}

try {
  const result = await semanticRelease(
    { dryRun: true, ci: false },
    { cwd, stdout: process.stderr, stderr: process.stderr },
  );
  // `result` is `false` when no release is due (e.g. no qualifying commit).
  process.stdout.write(result ? result.nextRelease.version : '');
} catch (err) {
  process.stderr.write(`\nsemantic-release JS API failed in ${cwd}: ${err?.message ?? err}\n`);
  process.exit(1);
}
```

- [ ] **Step 2: Build a throwaway fixture and confirm the runner prints a bumped version**

Run (verifies JS API plumbing — dry-run, no network/token, version extraction — before wiring the full adapter):

```bash
set -euo pipefail
TS_DIR="$(cd ts && pwd)"
FIX="$(mktemp -d)"
mkdir -p "$FIX/a/src"
cat > "$FIX/a/package.json" <<'EOF'
{ "name": "parity-probe-a", "version": "0.1.0", "private": true, "type": "module" }
EOF
cat > "$FIX/a/.releaserc.json" <<EOF
{ "branches": ["main"], "tagFormat": "a-v\${version}",
  "plugins": [ "$TS_DIR/tooling/semantic-release-path-filter.mjs" ] }
EOF
echo "// seed" > "$FIX/a/src/index.mjs"
( cd "$FIX/a" && git -c init.defaultBranch=main init -q \
  && git config user.email p@e.com && git config user.name p \
  && git config commit.gpgsign false && git config tag.gpgsign false \
  && git add -A && git commit -qm "chore: seed" && git tag a-v0.1.0 \
  && git remote add origin file:///dev/null/probe \
  && echo "// feat change" >> src/index.mjs && git add -A && git commit -qm "feat: add thing" )
echo "next version:"
node "$TS_DIR/tooling/semantic-release-next-version.mjs" "$FIX/a"; echo
rm -rf "$FIX"
```
Expected: prints `0.2.0` (a `feat` from a `0.1.0` baseline). If it errors about a missing remote or branch, note the exact message — adjust `build_fixture` in Task 6 (e.g. pass `repositoryUrl`/`branches` in the runner options). If it prints nothing for the `feat`, the plugin/config wiring is wrong — fix before proceeding.

> **Note:** the divergence values themselves (`fix!:`→`1.0.0`, and a no-commit slot→empty/baseline) are asserted end-to-end by the harness in Task 6 Step 6 (`fix-bang`/`fix-footer`/`feat-bang` rows = `1.0.0`; slot `b` = `0.1.0`). Step 2 above only needs to confirm the JS-API plumbing prints a real version; no separate probe is required here.

- [ ] **Step 3: Lint/format + commit**

```bash
pnpm --dir ts exec prettier --write tooling/semantic-release-next-version.mjs && moon run ts:lint
git add ts/tooling/semantic-release-next-version.mjs
git commit -m "feat(release): semantic-release JS-API next-version runner (SMA-406)"
```

---

## Task 4: Generic `ecosystem::expected` seam in `run.sh`

**Files:**
- Modify: `ci/release-parity/run.sh`

- [ ] **Step 1: Add the `resolve_expected` helper**

In `ci/release-parity/run.sh`, immediately AFTER the `source "$HERE/ecosystems/$ECOSYSTEM.sh"` line (currently line 21) and its blank line, add:

```sh
# Default: the canonical 0.x expectation (expected_0x). An ecosystem MAY define
# `ecosystem::expected` to assert a documented, intentional divergence (e.g.
# semantic-release's strict-semver breaking->major). release-plz / PSR do NOT
# define it, so their behavior is byte-for-byte unchanged.
resolve_expected() { # id subject footer expected_0x expected_1x discr -> expected
  if declare -F ecosystem::expected >/dev/null; then
    ecosystem::expected "$@"
  else
    printf '%s' "$4"
  fi
}
```

- [ ] **Step 2: Route the case loop through it**

In the `while IFS=$'\t' read … done 3<"$CASES"` loop, replace the line:

```sh
  ec=0; check_case "$id" "$subject" "$footer" "$expected_0x" || ec=$?
```
with:

```sh
  expected="$(resolve_expected "$id" "$subject" "$footer" "$expected_0x" "$_expected_1x" "$_discr")"
  ec=0; check_case "$id" "$subject" "$footer" "$expected" || ec=$?
```

> Leave the `--negative-control` block UNCHANGED: it passes its own explicit wrong expectation (`0.1.1`) directly to `check_case` and must NOT be routed through `resolve_expected` (it stays a genuine wrong-expectation probe for every ecosystem). See spec §2.

- [ ] **Step 3: Regression check — release-plz parity still green**

Run: `ci/release-parity/run.sh --ecosystem release-plz`
Expected: `== all parity cases passed ==`, exit 0 (proves the default path is unchanged). Requires the proto-managed `release-plz` + `cargo` on PATH (`export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"` if needed — see project memory).

- [ ] **Step 4: Regression check — python-semantic-release parity still green**

Run: `ci/release-parity/run.sh --ecosystem python-semantic-release`
Expected: `== all parity cases passed ==`, exit 0. (Requires `uv` on PATH.)

- [ ] **Step 5: Commit**

```bash
git add ci/release-parity/run.sh
git commit -m "feat(ci): generic ecosystem::expected hook in release-parity run.sh (SMA-406)"
```

---

## Task 5: Dormant production configs for `@paigasus/sdk` + `@paigasus/ui`

**Files:**
- Create: `ts/packages/paigasus-sdk/.releaserc.json`
- Create: `ts/packages/paigasus-ui/.releaserc.json`

- [ ] **Step 1: Write the sdk config**

Create `ts/packages/paigasus-sdk/.releaserc.json`:

```json
{
  "branches": ["main"],
  "tagFormat": "@paigasus/sdk-v${version}",
  "plugins": ["../../tooling/semantic-release-path-filter.mjs"]
}
```

> Plugin specifier is a path RELATIVE to the package dir (semantic-release runs with cwd = package dir at activation). No `releaseRules`, no `preset` override → native strict-semver classification (the documented divergence). No `@semantic-release/npm`/`github` (publish plugins; SMA-407). Packages stay `private: true` + `version: "0.0.0"` — do NOT change them (dormancy).

- [ ] **Step 2: Write the ui config**

Create `ts/packages/paigasus-ui/.releaserc.json` (identical except the tag namespace):

```json
{
  "branches": ["main"],
  "tagFormat": "@paigasus/ui-v${version}",
  "plugins": ["../../tooling/semantic-release-path-filter.mjs"]
}
```

- [ ] **Step 3: Verify the namespaced tag form is a valid git ref (spec Risk 5)**

Run:
```bash
git check-ref-format "refs/tags/@paigasus/sdk-v0.1.0" && echo "VALID" || echo "INVALID — sanitize tagFormat to paigasus-sdk-v\${version}"
```
Expected: `VALID`. If `INVALID`, change both `tagFormat`s to `paigasus-sdk-v${version}` / `paigasus-ui-v${version}` and re-run.

- [ ] **Step 4: Verify configs are well-formed JSON**

Run: `node -e "JSON.parse(require('fs').readFileSync('ts/packages/paigasus-sdk/.releaserc.json','utf8')); JSON.parse(require('fs').readFileSync('ts/packages/paigasus-ui/.releaserc.json','utf8')); console.log('OK')"`
Expected: `OK`.

- [ ] **Step 5: Commit**

```bash
git add ts/packages/paigasus-sdk/.releaserc.json ts/packages/paigasus-ui/.releaserc.json
git commit -m "feat(release): dormant semantic-release config for sdk + ui (SMA-406)"
```

---

## Task 6: The adapter — `ci/release-parity/ecosystems/semantic-release.sh`

**Files:**
- Create: `ci/release-parity/ecosystems/semantic-release.sh`

This is driven by the unchanged `run.sh` 4-function interface plus the `ecosystem::expected` hook from Task 4. Build it function-by-function, then run the harness.

- [ ] **Step 1: Write the module header + binary/path resolution + `ecosystem::expected`**

Create `ci/release-parity/ecosystems/semantic-release.sh`:

```bash
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
```

- [ ] **Step 2: Write the F3 derivation + guards (node helper invoked from bash)**

Append to the module:

```bash
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
```

- [ ] **Step 3: Write `build_fixture`**

Append:

```bash
ecosystem::build_fixture() { # dir real_release_plz_toml(ignored)
  local dir="$1" preset slot sdir pkg tagfmt presetline
  preset="$(ecosystem::_derive_classification)" || return 1   # F3 + guards (loud-fails abort)
  [ -f "$_SR_PATH_FILTER" ] || { echo "FATAL: path-filter missing: $_SR_PATH_FILTER" >&2; return 1; }
  [ -f "$_SR_RUNNER" ] || { echo "FATAL: runner missing: $_SR_RUNNER" >&2; return 1; }

  ( cd "$dir" && git -c init.defaultBranch=main init -q \
    && git config user.email "parity@example.com" && git config user.name "parity" \
    && git config commit.gpgsign false && git config tag.gpgsign false )

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

  ( cd "$dir" && git add -A && git commit -qm "chore: seed fixture" \
    && git tag "a-v$BASELINE" && git tag "b-v$BASELINE" \
    && git remote add origin "file:///dev/null/parity-semantic-release" )
}
```

- [ ] **Step 4: Write `apply_commit`, `run_update`, `version`**

Append:

```bash
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
  # Compute each slot's next version read-only via the semantic-release JS API.
  # The runner prints the version (or empty = no release) to stdout; its logs go
  # to stderr. The in-repo path-filter (wired into each slot config) restricts
  # analysis to commits under that slot dir, so slot `b` (no commit under b/)
  # gets no release -> baseline. Run from `ts/` is unnecessary: the runner lives
  # under ts/, so `import 'semantic-release'` resolves from ts/node_modules.
  local dir="$1" slot sdir out
  for slot in a b; do
    sdir="$(ecosystem::_slot_dir "$dir" "$slot")"
    if ! out="$(node "$_SR_RUNNER" "$sdir" 2>/tmp/sr-parity-$slot.err)"; then
      echo "FATAL: semantic-release runner failed for slot $slot" >&2
      cat "/tmp/sr-parity-$slot.err" >&2 || true
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
```

- [ ] **Step 5: Make executable**

Run: `chmod +x ci/release-parity/ecosystems/semantic-release.sh`
Expected: no output (mirrors the other adapters' mode).

- [ ] **Step 6: Run the harness — expect all cases green**

Run: `ci/release-parity/run.sh --ecosystem semantic-release`
Expected (slot `a` = `resolve_expected` value, slot `b` = `0.1.0`):
```
PASS  fix          a=0.1.1 b=0.1.0
PASS  feat         a=0.2.0 b=0.1.0
PASS  fix-bang     a=1.0.0 b=0.1.0
PASS  fix-footer   a=1.0.0 b=0.1.0
PASS  feat-bang    a=1.0.0 b=0.1.0
== all parity cases passed ==
```
If a breaking row shows `a=1.0.0` but FAILs, the hook isn't wired (check Task 4). If a breaking row shows `a=0.2.0`, the tool isn't classifying breaking→major (check the path-filter delegation / preset). If slot `b` ≠ `0.1.0`, the path-filter isn't filtering by path (check `git log -- .` in the plugin and that commits land under the slot dir).

- [ ] **Step 7: Negative control — expect red**

Run: `ci/release-parity/run.sh --ecosystem semantic-release --negative-control`
Expected: `negative-control OK: harness reported red as expected`, exit 0. (Real `fix!`→`1.0.0` ≠ fed `0.1.1`.)

- [ ] **Step 8: Commit**

```bash
git add ci/release-parity/ecosystems/semantic-release.sh
git commit -m "feat(ci): semantic-release parity adapter + documented 0.x divergence (SMA-406)"
```

---

## Task 7: F3 guard verification (prove the guards have teeth)

**Files:** none modified (temporary edits, all reverted).

- [ ] **Step 1: `releaseRules` clamp fails loud**

Temporarily edit `ts/packages/paigasus-sdk/.releaserc.json` plugins to `[["../../tooling/semantic-release-path-filter.mjs", { "releaseRules": [{ "breaking": true, "release": "minor" }] }]]`, then run: `ci/release-parity/run.sh --ecosystem semantic-release`
Expected: aborts with rc 2 and the message `FATAL: …/paigasus-sdk/.releaserc.json has commit-analyzer releaseRules — …`. **Revert the edit.**

- [ ] **Step 2: sdk/ui disagreement fails loud**

Temporarily add `"preset": "conventionalcommits"` to the sdk plugin options only (`[["../../tooling/…", {"preset":"conventionalcommits"}]]`), leave ui as default, then run: `ci/release-parity/run.sh --ecosystem semantic-release`
Expected: aborts with `FATAL: @paigasus/sdk and @paigasus/ui semantic-release classification differs …`. **Revert the edit.**

- [ ] **Step 3: Drift would go red (assertion teeth)**

Temporarily edit `ecosystem::expected` in the adapter to `printf '0.2.0'` for breaking rows, run the harness, confirm the three breaking rows FAIL (tool returns `1.0.0` ≠ asserted `0.2.0`). **Revert the edit.**

- [ ] **Step 4: Confirm clean state**

Run: `git status --short && ci/release-parity/run.sh --ecosystem semantic-release`
Expected: no modified files; `== all parity cases passed ==`. (No commit — this task only verifies.)

---

## Task 8: CI wiring — `release-parity-ts` Moon task + `moon ci` target

**Files:**
- Modify: `moon.yml` (root `repo` project)
- Modify: `.github/workflows/ci.yml`

- [ ] **Step 1: Add the Moon task**

In root `moon.yml`, under `tasks:`, after the `release-parity-py:` task, add:

```yaml
  release-parity-ts:
    description: 'Dry-run semantic-release over synthetic commits; assert commit->semver parity + documented 0.x divergence (SMA-406).'
    script: 'ci/release-parity/run.sh --ecosystem semantic-release'
    toolchain: 'system'
    inputs:
      - 'ci/release-parity/**/*'
      - 'ts/packages/paigasus-sdk/.releaserc.json'
      - 'ts/packages/paigasus-ui/.releaserc.json'
      - 'ts/tooling/semantic-release-path-filter.mjs'
      - 'ts/tooling/semantic-release-next-version.mjs'
      - 'ts/pnpm-lock.yaml'
      - '.prototools'
```

- [ ] **Step 2: Add to the `moon ci` target array**

In `.github/workflows/ci.yml`, in the "moon ci (affected graph)" step, change the `T=(…)` line from:

```bash
            T=(:build :test :lint :fmt :typecheck :breaking :release-parity :release-parity-py)
```
to:

```bash
            T=(:build :test :lint :fmt :typecheck :breaking :release-parity :release-parity-py :release-parity-ts)
```

- [ ] **Step 3: Run the task through Moon**

Run: `moon run repo:release-parity-ts`
Expected: `== all parity cases passed ==`, task succeeds. (Needs `node` + the pnpm-installed deps; run `pnpm --dir ts install` first if `ts/node_modules` is absent. Ensure proto/Moon PATH per project memory.)

- [ ] **Step 4: Validate workflow + Moon config syntax**

Run: `moon ci :release-parity-ts --base origin/main` (or `moon project repo` to confirm the task parses)
Expected: Moon recognizes `release-parity-ts`; no schema error. If `actionlint` is available, run it on `.github/workflows/ci.yml`.

- [ ] **Step 5: Commit**

```bash
git add moon.yml .github/workflows/ci.yml
git commit -m "feat(ci): wire release-parity-ts into Moon + moon ci (SMA-406)"
```

---

## Task 9: README section

**Files:**
- Modify: `ci/release-parity/README.md`

- [ ] **Step 1: Append the semantic-release section**

After the existing python-semantic-release section, add:

````markdown
## semantic-release adapter (SMA-406)
Run with `run.sh --ecosystem semantic-release`. It reuses this same `cases.tsv`.

Unlike release-plz and PSR — both aligned to the canonical 0.x contract —
semantic-release has **no version-aware 0.x clamp**, so it cannot be cleanly
aligned (its only lever, commit-analyzer `releaseRules`, is version-blind and
would mis-clamp post-1.0). Per the canonical contract from a `0.1.0` baseline:

| commit | canonical | semantic-release |
|--------|-----------|------------------|
| `fix:`  | 0.1.1 | 0.1.1 ✓ |
| `feat:` | 0.2.0 | 0.2.0 ✓ |
| `fix!:` / `fix:`+`BREAKING CHANGE:` / `feat!:` | 0.2.0 | **1.0.0** (documented divergence) |

So this adapter **documents** the divergence rather than aligning (ADR-0011 S6,
amended 2026-06-04). The harness asserts it via run.sh's generic
`ecosystem::expected` hook (breaking→`1.0.0`); the gate goes **red** if a
semantic-release upgrade changes the classification. The sub-1.0 lifecycle
consequence (TS-native packages leave 0.x on their first breaking change) is
routed to SMA-407.

### Why an in-repo path-filter (not a monorepo plugin)
The canonical `semantic-release-monorepo` (pmowrer) is abandoned + ESM-broken.
Per-package isolation is instead a small in-repo plugin
(`ts/tooling/semantic-release-path-filter.mjs`) that restricts `analyzeCommits`
to commits touching the package dir, then delegates to
`@semantic-release/commit-analyzer`. It is the ONLY `analyzeCommits` provider in
the `plugins` array (listing commit-analyzer separately would analyze the
unfiltered set and take the max, defeating the filter).

### One repo, two package dirs; versions via the JS API
The fixture is a single git repo with two package dirs through that same
path-filter, so slot `b` staying at baseline tests **path→package attribution**
(the mechanism the real `sdk`/`ui` config ships) — paralleling release-plz's
cargo attribution. Next versions are computed via the semantic-release **JS API**
(`ts/tooling/semantic-release-next-version.mjs`, `dryRun`), which returns the
structured next release (or `false`), so the adapter never scrapes the CLI log.

### Fixture config derived from BOTH real configs (F3)
`build_fixture` reads the classification (`preset`; absence of `releaseRules`)
from the real `paigasus-sdk` **and** `paigasus-ui` `.releaserc.json`, and fails
loudly if either adds a `releaseRules` clamp (the documented divergence would no
longer hold) or if the two disagree. Both configs are task inputs, so editing
either re-runs this check.
````

- [ ] **Step 2: Commit**

```bash
git add ci/release-parity/README.md
git commit -m "docs(ci): document semantic-release parity adapter + 0.x divergence (SMA-406)"
```

---

## Task 10: Full acceptance verification (spec Verification plan)

**Files:** none (final gate; run every spec item).

- [ ] **Step 1: Harness green + correct values**

Run: `ci/release-parity/run.sh --ecosystem semantic-release`
Expected: 5 PASS lines (`fix`=0.1.1, `feat`=0.2.0, the three breaking=1.0.0, all `b`=0.1.0) + `== all parity cases passed ==`. (Spec Verification #1, #2.)

- [ ] **Step 2: Negative control red**

Run: `ci/release-parity/run.sh --ecosystem semantic-release --negative-control`
Expected: `negative-control OK …`, exit 0. (Spec #3.)

- [ ] **Step 3: No-regression on the other ecosystems**

Run: `ci/release-parity/run.sh --ecosystem release-plz && ci/release-parity/run.sh --ecosystem python-semantic-release`
Expected: both `== all parity cases passed ==`. (Confirms the `run.sh` hook change is non-disruptive.)

- [ ] **Step 4: Dormancy holds**

Run: `node -e "for (const p of ['ts/packages/paigasus-sdk/package.json','ts/packages/paigasus-ui/package.json']){const j=require('./'+p); if(j.version!=='0.0.0'||j.private!==true){console.error('FAIL '+p);process.exit(1)}} console.log('dormant OK')"` and `git grep -l 'semantic-release' .github/workflows || echo 'no live workflow (good)'`
Expected: `dormant OK`; no live semantic-release workflow. (Spec #7.)

- [ ] **Step 5: Affected wiring spot-check**

Run: `moon query tasks --affected` after `touch ts/packages/paigasus-sdk/.releaserc.json` (or inspect `moon ci :release-parity-ts --base origin/main`), and confirm a Rust-only change does NOT pull in `release-parity-ts`.
Expected: editing a TS release config / `ts/tooling/*` / `ts/pnpm-lock.yaml` / `ci/release-parity/**` triggers `release-parity-ts`; an unrelated/Rust-only change does not. (Spec #8.)

- [ ] **Step 6: Branch is clean, all work committed**

Run: `git status --short && git log --oneline origin/main..HEAD`
Expected: clean tree; the Task 1–9 commits listed. Ready for PR (branch `feature/sma-406-…` auto-links to SMA-406).

---

## Self-review notes (for the executor)

- **Spec coverage:** dormant config (Task 5), in-repo path-filter (Task 2), JS-API runner (Task 3), `ecosystem::expected` hook (Task 4), adapter + F3 guards (Tasks 6–7), CI task (Task 8), README (Task 9), full verification (Task 10). All spec deliverables + Verification-plan items mapped.
- **Refinements vs spec:** JS API instead of CLI+parse (resolves Risk 2/3); single delegating plugin instead of two listed (avoids the max-release-type trap). Both are flagged in the header; sync the spec if desired.
- **Open implementation unknowns (resolved by the early tasks, not deferred):** exact `semantic-release` major (Task 1), commit-analyzer ESM import shape (Task 1 Step 5 → Task 2), JS-API dry-run behavior incl. remote/branches (Task 3 Step 2), namespaced-tag validity (Task 5 Step 3). Each has an inline fallback.
- **Do not** change `private`/`version` on the real packages, add a live workflow, or edit `cases.tsv`.
