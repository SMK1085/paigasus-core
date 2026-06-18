# SMA-428 — napi-rs cross-platform `.node` prebuild matrix (infra-only) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build and verify a cross-platform `.node` prebuild + packaging pipeline for `@paigasus/node-bindings` up to (but not including) `npm publish`, leaving both packages `private:true` / `0.0.0`.

**Architecture:** A dedicated, `moon`-decoupled GitHub Actions workflow (`.github/workflows/prebuild.yml`) builds the 7 platform `.node` addons on native-arch runners (musl cross-compiled via `cargo-zigbuild` — see amendment below), then an `assemble` job generates the per-platform npm packages **in CI** (nothing committed), asserts the publish-artifact shape with `napi prepublish --dry-run --no-gh-release` + `npm pack --dry-run`, and proves install-time platform resolution with one real install on the `linux-x64-gnu` host. Package metadata is added to both `@paigasus/node-bindings` and `@paigasus/kernel`; the single-host local `moon` build is untouched.

**Tech Stack:** `@napi-rs/cli` v3 (`^3.7.2`), GitHub Actions (native runners + `cargo-zigbuild` for musl), proto/Moon toolchain pins (node 24.16.0, pnpm 11.3.0, Rust 1.95.0), pnpm.

> **Post-implementation amendments (2026-06-18, after CI verification — see the spec's "CI verification findings").** Two design points changed under real CI: **(1) musl** — the originally-planned job-level napi-rs Alpine container (Task 4 below) was dropped (GitHub bans JS actions in Alpine containers on arm64 runners; the image's pnpm 9 can't read the pnpm-11 lockfile). Both musl targets now cross-compile via `cargo-zigbuild` on the glibc `ubuntu` runners (`napi build -x`, zig via `pip install ziglang`), with **no** `container:` key and **no** `matrix.container == ''` gating — every leg runs the toolchain steps. **(2) `--no-gh-release` IS required** on `napi prepublish` (re-added — the Task-1 spike wrongly removed it; `ghRelease` defaults on and the flag is accepted despite being absent from `--help`). The Task 4/Task 5 code blocks below predate these fixes; the committed `prebuild.yml` is authoritative.

**Spec:** `docs/superpowers/specs/2026-06-17-sma-428-napi-prebuild-matrix-design.md` (+ `-review.md`).

**Branch:** `feature/sma-428-napi-rs-cross-platform-node-prebuild-matrix-npm-publish-for` (already created, off `main`).

**Conventions:** SPDX header on source files (config files like `package.json`/workflow YAML are exempt per CONTRIBUTING, but YAML may carry a `# SPDX-License-Identifier: Apache-2.0` comment — match the repo: existing `.github/workflows/ci.yml` has **no** SPDX header, so omit it on `prebuild.yml`). Conventional commits scoped `feat(ci)` / `chore(ts)` etc. Commits are SSH-signed via 1Password; if a commit fails with "failed to fill whole buffer", 1Password is locked — ask the user to unlock and retry.

---

## Task 1: De-risk spike — confirm napi v3 packaging commands + single-host install resolution (local, darwin-arm64)

This is a **throwaway** spike (spec §6 spike risks #1–#2). It confirms the exact `@napi-rs/cli@3.7.2` subcommand/flag surface and the local install-resolution incantation **before** the workflow is written, then records findings. No production files change except the committed findings doc.

**Files:**
- Create: `docs/superpowers/specs/2026-06-17-sma-428-spike-findings.md`
- Scratch only (discard): a `mktemp -d` smoke project; transient `npm/` dirs + `.node` + `*.tgz` in the crate (all gitignored or removed).

- [ ] **Step 1: Confirm the v3 CLI surface**

Run from repo root:
```bash
cd ts/packages/paigasus-kernel
pnpm exec napi --version
pnpm exec napi build --help
pnpm exec napi create-npm-dirs --help
pnpm exec napi artifacts --help
pnpm exec napi prepublish --help
```
Expected: version `3.7.x`; record the exact flags for `create-npm-dirs` (where it writes `npm/`), `artifacts` (the input dir of built `.node`s + `--npm-dir`), and `prepublish` (confirm `--dry-run` and `--no-gh-release` exist, and the `--tag-style` default). Capture any flag that differs from this plan's assumptions.

- [ ] **Step 2: Build the host addon + generate npm dirs**

Run (still in `ts/packages/paigasus-kernel`):
```bash
CRATE=../../../rs/crates/bindings/paigasus-node-bindings
pnpm exec napi build --platform --release --cwd "$CRATE"
pnpm exec napi create-npm-dirs --cwd "$CRATE"
ls "$CRATE/npm"
cat "$CRATE/npm/darwin-arm64/package.json"
```
Expected: `$CRATE/paigasus-node-bindings.darwin-arm64.node` exists; `$CRATE/npm/<platform>/` dirs created for each `napi.targets` entry (after Task 2 adds them — for the spike, you may pass `--cwd` against the current package.json which still has only `binaryName`; if `create-npm-dirs` emits nothing without `targets`, note it and re-run this step after Task 2's package.json is drafted locally). Record the generated per-platform `package.json` shape (`os`/`cpu`/`libc`/`main`).

- [ ] **Step 3: Dry-run prepublish + pack**

```bash
CRATE=../../../rs/crates/bindings/paigasus-node-bindings
pnpm exec napi artifacts --cwd "$CRATE"   # adjust flags per Step 1 --help
pnpm exec napi prepublish --dry-run --no-gh-release --cwd "$CRATE"
( cd "$CRATE" && npm pack --dry-run )
```
Expected: `prepublish --dry-run` logs the `optionalDependencies` it *would* write and the addon copies it *would* make, touches nothing, and creates **no** GitHub release. `npm pack --dry-run` on the crate lists `index.js` + `index.d.ts` **only** once Task 2's `files` fix is applied (note if it currently also lists `*.node`). Record whether `prepublish` needs `artifacts` to have run first.

- [ ] **Step 4: Prove single-host install resolution**

```bash
CRATE_ABS="$(cd ../../../rs/crates/bindings/paigasus-node-bindings && pwd)"
MAIN_TGZ="$(cd "$CRATE_ABS" && npm pack | tail -1)"
PLAT_TGZ="$(cd "$CRATE_ABS/npm/darwin-arm64" && npm pack | tail -1)"
SMOKE="$(mktemp -d)"
cd "$SMOKE" && npm init -y >/dev/null
npm install "$CRATE_ABS/$MAIN_TGZ" "$CRATE_ABS/npm/darwin-arm64/$PLAT_TGZ"
node -e "const b=require('@paigasus/node-bindings'); if (b.sum(2,3)!==5) { console.error('FAIL', b.sum(2,3)); process.exit(1);} console.log('resolved + loaded OK');"
```
Expected: prints `resolved + loaded OK`. This is the darwin-arm64 rehearsal of the `linux-x64-gnu` CI check (spec §2.4). Note whether the loader resolved via the `@paigasus/node-bindings-darwin-arm64` package path (not a bundled `.node`), and whether `NAPI_RS_ENFORCE_VERSION_CHECK` needed to stay unset for the `0.0.0` version to load.

- [ ] **Step 5: Record findings + clean up scratch**

Write `docs/superpowers/specs/2026-06-17-sma-428-spike-findings.md` capturing: confirmed `napi` v3.7.2 subcommands + exact flags (esp. `artifacts` input dir + `prepublish` flags + `--tag-style` default), the working install-resolution recipe, and any deviation from this plan (so Tasks 4–5 use the verified commands). Then remove scratch:
```bash
rm -rf "$SMOKE"
rm -rf ../../../rs/crates/bindings/paigasus-node-bindings/npm
rm -f ../../../rs/crates/bindings/paigasus-node-bindings/*.node ../../../rs/crates/bindings/paigasus-node-bindings/*.tgz
git status --porcelain   # only the findings doc should be new
```
Expected: `git status` shows only `docs/superpowers/specs/2026-06-17-sma-428-spike-findings.md` untracked (the `npm/`, `.node`, `.tgz` are gitignored or removed).

- [ ] **Step 6: Commit**

```bash
git add docs/superpowers/specs/2026-06-17-sma-428-spike-findings.md
git commit -m "docs(ts): SMA-428 spike findings — napi v3 packaging + install-resolution"
```

---

## Task 2: `@paigasus/node-bindings/package.json` — `napi.targets`, npm metadata, `files` fix

**Files:**
- Modify: `rs/crates/bindings/paigasus-node-bindings/package.json`

- [ ] **Step 1: Rewrite the manifest**

Replace the entire file contents with (adjust `engines.node` only if Task 1 found a napi-imposed floor):
```json
{
  "name": "@paigasus/node-bindings",
  "version": "0.0.0",
  "private": true,
  "license": "Apache-2.0",
  "description": "Node-API (napi-rs) binding for the Paigasus Rust kernel.",
  "keywords": ["paigasus", "napi", "napi-rs", "node-api", "ffi", "rust"],
  "homepage": "https://github.com/SMK1085/paigasus-core#readme",
  "repository": {
    "type": "git",
    "url": "git+https://github.com/SMK1085/paigasus-core.git",
    "directory": "rs/crates/bindings/paigasus-node-bindings"
  },
  "engines": {
    "node": ">=20"
  },
  "main": "index.js",
  "types": "index.d.ts",
  "napi": {
    "binaryName": "paigasus-node-bindings",
    "targets": [
      "x86_64-apple-darwin",
      "aarch64-apple-darwin",
      "x86_64-pc-windows-msvc",
      "x86_64-unknown-linux-gnu",
      "aarch64-unknown-linux-gnu",
      "x86_64-unknown-linux-musl",
      "aarch64-unknown-linux-musl"
    ]
  },
  "files": ["index.js", "index.d.ts"],
  "publishConfig": {
    "access": "public"
  },
  "scripts": {
    "build": "napi build --platform",
    "build:release": "napi build --platform --release"
  },
  "devDependencies": {
    "@napi-rs/cli": "^3"
  }
}
```
Key changes vs current: added `description`/`keywords`/`homepage`/`repository`/`engines`/`publishConfig`; added `napi.targets` (the 7 triples — v3 key); **removed `*.node` from `files`** (now `["index.js", "index.d.ts"]`). `private:true` / `version:0.0.0` unchanged (publish deferred → SMA-407).

- [ ] **Step 2: Validate JSON + the `files` fix locally**

```bash
node -e "const p=require('./rs/crates/bindings/paigasus-node-bindings/package.json'); console.log('targets', p.napi.targets.length); console.log('files', JSON.stringify(p.files)); if(p.files.includes('*.node')) process.exit(1);"
```
Expected: `targets 7`, `files ["index.js","index.d.ts"]`, exit 0 (no `*.node`).

- [ ] **Step 3: Confirm the local `moon` build still works (single-host, unchanged path)**

```bash
moon run paigasus-kernel-ts:build
```
Expected: PASS — `napi build --platform` still produces the host `.node`; the metadata/`files`/`targets` additions don't affect the single-host build. (If `moon` is not on PATH, export `~/.proto/shims:~/.proto/bin` first per the repo PATH note.)

- [ ] **Step 4: Commit**

```bash
git add rs/crates/bindings/paigasus-node-bindings/package.json
git commit -m "feat(ts): add napi.targets + npm metadata to @paigasus/node-bindings, drop *.node from files (SMA-428)"
```

---

## Task 3: `@paigasus/kernel/package.json` — npm metadata + double-blocked breadcrumb

**Files:**
- Modify: `ts/packages/paigasus-kernel/package.json`

- [ ] **Step 1: Add metadata + breadcrumb**

Edit the manifest to add metadata fields and a publish-blocked breadcrumb, keeping `private:true`, `version:0.0.0`, `type`, `exports`, `dependencies`, `devDependencies`, and `scripts` exactly as they are. The result:
```json
{
  "name": "@paigasus/kernel",
  "_comment_exports": "node → the napi binding (compiled .node); browser/default → the wasm binding (paigasus-wasm, .wasm + glue). Conditions point at SOURCE (bundler-aware consumers walk TS); switch to ./dist/* when tsup wiring lands, IN LOCKSTEP with flipping `private: false`. workerd intentionally omitted (no verified workerd path yet — SMA-427 H3); it falls through to `default`.",
  "_comment_publish": "Publish is DOUBLE-BLOCKED, deferred beyond SMA-428 (which only adds static metadata): (1) exports point at source — needs the tsup/dist build (SMA-420 deferral) before a pack ships JS instead of raw .ts, and the `file:` deps must become real npm packages; (2) the version flip off 0.0.0 + private:false belong to release activation (SMA-407).",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "license": "Apache-2.0",
  "description": "Cross-language Paigasus kernel for Node and the browser (napi + wasm bindings).",
  "keywords": ["paigasus", "kernel", "napi", "wasm", "ffi"],
  "homepage": "https://github.com/SMK1085/paigasus-core#readme",
  "repository": {
    "type": "git",
    "url": "git+https://github.com/SMK1085/paigasus-core.git",
    "directory": "ts/packages/paigasus-kernel"
  },
  "publishConfig": {
    "access": "public"
  },
  "exports": {
    ".": {
      "node": "./src/index.ts",
      "browser": "./src/wasm.ts",
      "default": "./src/wasm.ts"
    }
  },
  "scripts": {
    "typecheck": "tsc -p tsconfig.json --noEmit"
  },
  "dependencies": {
    "@paigasus/node-bindings": "file:../../../rs/crates/bindings/paigasus-node-bindings",
    "@paigasus/wasm": "file:../../../rs/crates/bindings/paigasus-wasm"
  },
  "devDependencies": {
    "@napi-rs/cli": "catalog:",
    "typescript": "catalog:",
    "vite-plugin-wasm": "catalog:",
    "vitest": "catalog:"
  }
}
```

- [ ] **Step 2: Validate JSON + exports unchanged**

```bash
node -e "const p=require('./ts/packages/paigasus-kernel/package.json'); if(p.exports['.'].node!=='./src/index.ts') process.exit(1); if(!p.repository||!p._comment_publish) process.exit(1); console.log('ok');"
```
Expected: `ok` (exports still point at source; metadata + breadcrumb present).

- [ ] **Step 3: Confirm typecheck/build still green**

```bash
moon run paigasus-kernel-ts:build
```
Expected: PASS (metadata-only change; no exports/tsup change).

- [ ] **Step 4: Commit**

```bash
git add ts/packages/paigasus-kernel/package.json
git commit -m "chore(ts): add npm metadata + publish-blocked breadcrumb to @paigasus/kernel (SMA-428)"
```

---

## Task 4: `.github/workflows/prebuild.yml` — cross-platform `build` matrix job

> ⚠️ **SUPERSEDED — do NOT implement the YAML in this task as-is.** The matrix + steps below are the
> *original* design using a job-level napi-rs **Alpine container** for the two musl legs. CI proved
> that unworkable (GitHub bans JS actions in Alpine containers on **arm64** runners; the image ships
> **pnpm 9** vs the repo's **pnpm-11** lockfile). The committed **`.github/workflows/prebuild.yml`** is
> the authoritative source: every leg runs on a glibc/macOS/Windows host with **no `container:` key**,
> and the two **musl** targets cross-compile via `cargo-zigbuild` (`napi build -x`, zig via
> `pip install ziglang`). See the **Post-implementation amendments** note near the top of this plan and
> the design spec (`…-sma-428-napi-prebuild-matrix-design.md`, §1/§6 + *CI verification findings*). The
> block below is retained for historical context only.

**Files:**
- Create: `.github/workflows/prebuild.yml`

- [ ] **Step 1: Write the workflow with the `build` job**

Create `.github/workflows/prebuild.yml` (use the exact `napi` flags Task 1 confirmed; the commands below are the canonical v3 form):
```yaml
name: prebuild

on:
  workflow_dispatch:
  push:
    branches: [main]

# Build-only verification: read the repo, build addons, run dry-run packaging. No publish, and
# --no-gh-release means no release is created, so no contents:write is needed (SMA-428 / SMA-407
# adds publish creds at activation).
permissions:
  contents: read

concurrency:
  group: prebuild-${{ github.workflow }}-${{ github.ref }}
  cancel-in-progress: ${{ github.event_name == 'workflow_dispatch' }}

jobs:
  build:
    name: build ${{ matrix.platform }}
    runs-on: ${{ matrix.runner }}
    container: ${{ matrix.container }}
    timeout-minutes: 30
    strategy:
      fail-fast: false
      matrix:
        include:
          - { platform: darwin-x64,       target: x86_64-apple-darwin,        runner: macos-15-intel,    container: '' }
          - { platform: darwin-arm64,     target: aarch64-apple-darwin,       runner: macos-latest,      container: '' }
          - { platform: win32-x64-msvc,   target: x86_64-pc-windows-msvc,     runner: windows-latest,    container: '' }
          - { platform: linux-x64-gnu,    target: x86_64-unknown-linux-gnu,   runner: ubuntu-latest,     container: '' }
          - { platform: linux-arm64-gnu,  target: aarch64-unknown-linux-gnu,  runner: ubuntu-24.04-arm,  container: '' }
          - { platform: linux-x64-musl,   target: x86_64-unknown-linux-musl,  runner: ubuntu-latest,     container: 'ghcr.io/napi-rs/napi-rs/nodejs-rust:lts-alpine' }
          - { platform: linux-arm64-musl, target: aarch64-unknown-linux-musl, runner: ubuntu-24.04-arm,  container: 'ghcr.io/napi-rs/napi-rs/nodejs-rust:lts-alpine' }
    steps:
      - name: Checkout
        uses: actions/checkout@df4cb1c069e1874edd31b4311f1884172cec0e10  # v6.0.3
        with:
          persist-credentials: false

      # Host legs: pin node/pnpm/rust via proto (matches .prototools / rust-toolchain.toml).
      # musl container legs SKIP this and use the image's bundled Rust/Node by design (spec §2 / M2:
      # the .node is a leaf artifact, so the image toolchain carries no cross-version rmeta hazard).
      - name: Set up proto + Moon (host legs)
        if: ${{ matrix.container == '' }}
        uses: moonrepo/setup-toolchain@261c62cb5b0f580c7be7c8cd0f023a2e96756095  # v0
        with:
          cache: false

      - name: Install Moon-managed toolchains (host legs)
        if: ${{ matrix.container == '' }}
        run: moon setup

      # Cross-target artifacts differ by triple → triple in the cache key (spec §2 / L2).
      - name: Cache Rust (cargo + target)
        if: ${{ matrix.container == '' }}
        uses: actions/cache@27d5ce7f107fe9357f9df03efb73ab90386fccae  # v5.0.5
        with:
          path: |
            ~/.cargo/registry
            ~/.cargo/git
            rs/target
          key: prebuild-rust-${{ runner.os }}-${{ matrix.target }}-${{ hashFiles('rs/rust-toolchain.toml') }}-${{ hashFiles('rs/Cargo.lock') }}
          restore-keys: |
            prebuild-rust-${{ runner.os }}-${{ matrix.target }}-${{ hashFiles('rs/rust-toolchain.toml') }}-

      # Attach the cross target to the pinned 1.95.0 toolchain — run from rs/ so rust-toolchain.toml's
      # override selects 1.95.0, not the runner default (mirrors ci.yml's serial pre-install; spec §2).
      - name: Add Rust target (pinned toolchain)
        working-directory: rs
        run: rustup target add ${{ matrix.target }}

      - name: Install JS workspace deps
        run: pnpm --dir ts install --frozen-lockfile

      - name: Build the addon
        working-directory: ts/packages/paigasus-kernel
        run: pnpm exec napi build --platform --release --target ${{ matrix.target }} --cwd ../../../rs/crates/bindings/paigasus-node-bindings

      - name: Upload prebuild artifact
        uses: actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02  # v4.6.2
        with:
          name: prebuild-${{ matrix.platform }}
          path: rs/crates/bindings/paigasus-node-bindings/paigasus-node-bindings.${{ matrix.platform }}.node
          if-no-files-found: error
```
Notes for the executor: pin action SHAs to whatever the repo already uses where possible (the `checkout`/`cache` SHAs above are copied from `ci.yml`; verify `upload-artifact`/`download-artifact` SHAs against the latest v4 and pin them). Inside the musl container, `moon`/`proto` are absent by design — `rustup`, `pnpm`, and `node` come from the image; confirm in Task 1/Task 6 that the image provides them on PATH (if `pnpm` is missing in the image, add a `corepack enable` / `npm i -g pnpm@11.3.0` step gated on `matrix.container != ''`).

- [ ] **Step 2: Validate YAML syntax**

```bash
command -v actionlint >/dev/null && actionlint .github/workflows/prebuild.yml || python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/prebuild.yml')); print('yaml ok')" 2>/dev/null || echo "no local yaml linter — Task 6 CI dispatch is the real verification"
```
Expected: `yaml ok` (or actionlint clean). If no linter is available, the statement printed is acceptable — the real verification is Task 6.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/prebuild.yml
git commit -m "feat(ci): add cross-platform napi prebuild matrix workflow (SMA-428)"
```

---

## Task 5: `prebuild.yml` — `assemble` job (dry-run packaging + single-host install resolution)

**Files:**
- Modify: `.github/workflows/prebuild.yml`

- [ ] **Step 1: Append the `assemble` job**

Add to `.github/workflows/prebuild.yml` after the `build` job (use Task 1's confirmed `artifacts`/`prepublish` flags):
```yaml
  assemble:
    name: assemble + verify (dry-run, no publish)
    runs-on: ubuntu-latest   # ubuntu-latest == linux-x64-gnu, one of the 7 targets (spec §2.4)
    needs: build
    timeout-minutes: 20
    steps:
      - name: Checkout
        uses: actions/checkout@df4cb1c069e1874edd31b4311f1884172cec0e10  # v6.0.3
        with:
          persist-credentials: false

      - name: Set up proto + Moon
        uses: moonrepo/setup-toolchain@261c62cb5b0f580c7be7c8cd0f023a2e96756095  # v0
        with:
          cache: false

      - name: Install Moon-managed toolchains
        run: moon setup

      - name: Install JS workspace deps
        run: pnpm --dir ts install --frozen-lockfile

      - name: Download all prebuilt addons
        uses: actions/download-artifact@d3f86a106a0bac45b974a628896c90dbdf5c8093  # v4.3.0
        with:
          path: rs/crates/bindings/paigasus-node-bindings/artifacts
          pattern: prebuild-*
          merge-multiple: true

      # Generate the per-platform npm/<platform>/ dirs IN CI (decision #4 — nothing committed),
      # then sort the downloaded .node files into them.
      - name: Generate npm dirs + distribute artifacts
        working-directory: ts/packages/paigasus-kernel
        run: |
          CRATE=../../../rs/crates/bindings/paigasus-node-bindings
          pnpm exec napi create-npm-dirs --cwd "$CRATE"
          # `napi artifacts` default input dir is ./artifacts (spike Task 1) — the download-artifact
          # step above writes the .node files into $CRATE/artifacts, so do NOT pass --output-dir.
          pnpm exec napi artifacts --cwd "$CRATE" --npm-dir npm

      # Dry-run prepublish: assert os/cpu/libc + main paths + optionalDependencies resolve, touching
      # nothing. `--no-gh-release` IS REQUIRED — ghRelease defaults ON, so without it prepublish enters
      # createGhRelease→getRepoInfo and fails on the shallow CI checkout ("No release commit found")
      # even under --dry-run. The flag IS accepted (clipanion auto-negates booleans) though absent from
      # --help (the Task-1 spike misread this; CI confirmed). --tag-style left at the default `lerna`
      # (inert under dry-run; SMA-407 owns the boundary). Do NOT set NAPI_RS_ENFORCE_VERSION_CHECK.
      - name: Verify packaging (prepublish dry-run)
        working-directory: ts/packages/paigasus-kernel
        run: pnpm exec napi prepublish --dry-run --no-gh-release --npm-dir npm --cwd ../../../rs/crates/bindings/paigasus-node-bindings

      # Assert the MAIN package tarball is loader-only (the §3 files fix) + each per-platform tarball
      # carries exactly one .node.
      - name: Verify tarball contents (pack dry-run)
        working-directory: rs/crates/bindings/paigasus-node-bindings
        run: |
          set -euo pipefail
          echo "== main package ==" ; npm pack --dry-run
          npm pack --dry-run --json | node -e '
            const pkgs = JSON.parse(require("fs").readFileSync(0));
            const files = pkgs[0].files.map(f => f.path);
            if (files.some(f => f.endsWith(".node"))) { console.error("FAIL: main package ships a .node", files); process.exit(1); }
            if (!files.includes("index.js") || !files.includes("index.d.ts")) { console.error("FAIL: missing loader glue", files); process.exit(1); }
            console.log("main package is loader-only OK");
          '

      # Single-host REAL install-resolution check on linux-x64-gnu (spec §2.4 / review H2): pack the
      # main + linux-x64-gnu per-platform packages, install both into a scratch project so the
      # optional dep resolves via the PACKAGE PATH, and load sum across the FFI boundary.
      - name: Verify install-time platform resolution (linux-x64-gnu)
        working-directory: rs/crates/bindings/paigasus-node-bindings
        run: |
          set -euo pipefail
          CRATE_ABS="$(pwd)"
          MAIN_TGZ="$CRATE_ABS/$(npm pack | tail -1)"
          PLAT_TGZ="$CRATE_ABS/npm/linux-x64-gnu/$(cd npm/linux-x64-gnu && npm pack | tail -1)"
          SMOKE="$(mktemp -d)"
          cd "$SMOKE" && npm init -y >/dev/null
          npm install "$MAIN_TGZ" "$PLAT_TGZ"
          test -d node_modules/@paigasus/node-bindings-linux-x64-gnu || { echo "FAIL: linux-x64-gnu optional dep not installed"; exit 1; }
          node -e 'const b=require("@paigasus/node-bindings"); if(b.sum(2,3)!==5){console.error("FAIL sum=",b.sum(2,3));process.exit(1);} console.log("install-resolution + FFI load OK");'
```
Notes: pin the `download-artifact` SHA to the current v4 release. Spike Task 1 confirmed the exact flags (see `docs/superpowers/specs/2026-06-17-sma-428-spike-findings.md`): `napi artifacts` default input dir is `./artifacts` (matches the download path → no `--output-dir`); `--npm-dir npm` is the correct flag (not `--dist`); there is **no** `--no-gh-release` (omit `--gh-release`); `prepublish` needs `artifacts` to have populated `npm/` first, so the step order above is correct. **Task 6 watch item:** confirm `napi artifacts`'s default `./artifacts` resolves relative to `--cwd "$CRATE"` (i.e. `$CRATE/artifacts`, where the download lands); if the CI run shows it reading the wrong dir, pass an explicit `--output-dir "$CRATE/artifacts"`.

- [ ] **Step 2: Validate YAML syntax**

```bash
command -v actionlint >/dev/null && actionlint .github/workflows/prebuild.yml || python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/prebuild.yml')); print('yaml ok')" 2>/dev/null || echo "no local yaml linter — Task 6 CI dispatch is the real verification"
```
Expected: `yaml ok` / actionlint clean / the no-linter notice.

- [ ] **Step 3: Commit**

```bash
git add .github/workflows/prebuild.yml
git commit -m "feat(ci): add assemble job — dry-run packaging + single-host install-resolution check (SMA-428)"
```

---

## Task 6: Real CI verification + confirm `moon ci` is unaffected

The workflow triggers on `workflow_dispatch` + `push:main`, neither of which fires on a feature branch. To verify **before** merge, temporarily add this branch to the `push` trigger, get a real run, then revert. **Pushing is outward-facing — confirm with the user before Step 2 (the repo convention is "push only when asked").**

**Files:**
- Modify (temporary, reverted in Step 5): `.github/workflows/prebuild.yml`

- [ ] **Step 1: Temporarily widen the push trigger to this branch**

Edit `.github/workflows/prebuild.yml`:
```yaml
on:
  workflow_dispatch:
  push:
    branches: [main, feature/sma-428-napi-rs-cross-platform-node-prebuild-matrix-npm-publish-for]
```

- [ ] **Step 2: Commit + push (after user confirmation)**

```bash
git add .github/workflows/prebuild.yml
git commit -m "ci: TEMP run prebuild on the SMA-428 branch (reverted in next commit)"
git push -u origin feature/sma-428-napi-rs-cross-platform-node-prebuild-matrix-npm-publish-for
```

- [ ] **Step 3: Watch the run to completion**

```bash
sleep 10
RUN_ID="$(gh run list --workflow=prebuild.yml --branch feature/sma-428-napi-rs-cross-platform-node-prebuild-matrix-npm-publish-for --limit 1 --json databaseId -q '.[0].databaseId')"
gh run watch "$RUN_ID" --exit-status
```
Expected: exit 0 — all 7 `build` legs and the `assemble` job succeed. If a leg fails, debug per the spec's spike risks (musl image PATH for pnpm/rust → add `corepack enable`; `macos-15-intel` unavailable → switch `darwin-x64` to cross-build `--target x86_64-apple-darwin` on `macos-latest`; wrong `napi artifacts`/`prepublish` flags → apply Task 1's findings), fix, recommit, re-push, re-watch. Do not proceed until green.

- [ ] **Step 4: Confirm the assemble job's three assertions in the logs**

```bash
gh run view "$RUN_ID" --log | grep -E "main package is loader-only OK|install-resolution \+ FFI load OK" || true
```
Expected: both success lines present (loader-only main package; install-resolution + FFI load). Also confirm `prepublish` logged the 7 `optionalDependencies` and created **no** GitHub release (`gh release list` shows nothing new).

- [ ] **Step 5: Revert the temporary trigger**

Edit `.github/workflows/prebuild.yml` back to:
```yaml
on:
  workflow_dispatch:
  push:
    branches: [main]
```
```bash
git add .github/workflows/prebuild.yml
git commit -m "ci: revert TEMP SMA-428 branch trigger; prebuild runs on dispatch + main only"
git push
```

- [ ] **Step 6: Confirm `moon ci` is unaffected**

```bash
moon run :build :test :typecheck :lint
```
Expected: PASS — the metadata/`files`/`targets` edits and the new workflow don't change the affected-graph build/test/typecheck/lint outcomes; local `paigasus-kernel-ts` build/test still green. (The PR's `ci.yml` run on push confirms the same in CI.)

---

## Verification (maps to spec acceptance criteria)

1. **Matrix build (spec V1):** Task 6 Step 3 — all 7 `build` legs green, each uploads its `.node`.
2. **Dry-run assembly (spec V2):** Task 6 Step 4 — `prepublish --dry-run --no-gh-release` + `npm pack --dry-run` show loader-only main, one `.node` per platform package, os/cpu/libc set, 7 optionalDependencies.
3. **Single-host install resolution (spec V3):** Task 6 Step 4 — `install-resolution + FFI load OK` on linux-x64-gnu.
4. **No publish / no state change (spec V4):** no `npm publish`, no GitHub release; both packages `private:true`/`0.0.0` (Tasks 2–3); nothing under `npm/` committed (Task 1 Step 5, generated only in CI).
5. **No regression (spec V5):** Task 6 Step 6 — `moon ci` build/test/typecheck/lint green.

## Self-review notes

- **Spec coverage:** §1 matrix → Task 4; §2 build job + caching + musl toolchain → Task 4; §2 assemble + dry-run + single-host check → Task 5; §3 node-bindings package.json → Task 2; §4 kernel metadata → Task 3; §5 moon.yml-unchanged → asserted in Tasks 2/3/6; §6 release boundary (`--no-gh-release`, tag-style deferred) → Task 5; spike risks → Task 1. L1 (darwin-x64 build-verified only) is inherent to the matrix (no install leg for it — intentional). L3 (Linear reconcile) done at spec time.
- **No publish path exists anywhere** in the workflow (`--dry-run` + `--no-gh-release`, `permissions: contents: read`).
- **Type/name consistency:** package name `@paigasus/node-bindings`; per-platform `@paigasus/node-bindings-<platform>`; the 7 `platform`↔`target` pairs are identical across Tasks 4–6; `sum(2,3)===5` is the FFI assertion used in Tasks 1 and 5.
