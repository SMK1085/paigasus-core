# SMA-428 Spike Findings — napi v3 packaging + install-resolution

**Date:** 2026-06-17
**Host:** darwin-arm64 (macOS 25.5.0, Node 24.16.0, pnpm 11.3.0)
**@napi-rs/cli version confirmed:** 3.7.2

> **Correction (2026-06-18, after CI):** Finding #1 below — that `--no-gh-release` does not exist — is
> **WRONG**. `ghRelease` defaults **on**, and `--no-gh-release` **is** accepted (clipanion auto-negates
> booleans) even though it's absent from `--help`. Omitting it makes `prepublish` fail in CI's shallow
> checkout (`createGhRelease`→`getRepoInfo`, "No release commit found"). The workflow + spec use
> `napi prepublish --dry-run --no-gh-release`. See the spec's *CI verification findings*.

---

## Step 1 — Confirmed CLI flag surface

### `napi build`

Run from `ts/packages/paigasus-kernel` via `pnpm exec napi build`:

```
--target,-t         cross-compile target triple (passed to `cargo build --target`)
--cwd               working dir for napi (all paths relative to this)
--manifest-path     path to Cargo.toml
--platform          add platform triple to generated .node filename
--output-dir,-o     where built files land (default: crate folder)
--release,-r        build in release mode
--cross-compile,-x  use cargo-zigbuild (non-windows) / cargo-xwin (windows)
--use-napi-cross    use @napi-rs/cross-toolchain for linux arm/arm64/x64 gnu
--esm               emit ESM binding instead of CJS
--no-js             skip JS binding generation
--features,-F       cargo features
```

Key: `--cwd` makes all paths (including the cargo manifest) relative to that dir. The established pattern from SMA-420 is correct: `--cwd ../../../rs/crates/bindings/paigasus-node-bindings`.

### `napi create-npm-dirs`

```
--cwd               working dir
--npm-dir           where npm/ subdirs are placed (default: <cwd>/npm)
--dry-run           no filesystem changes
```

**Source:** reads `napi.targets` array from the crate's `package.json`. Without `napi.targets`, the command does nothing (no platform dirs created). The 7 target triples must be present before this command runs.

### `napi artifacts`

```
--cwd               working dir
--output-dir,-o,-d  dir containing the already-built .node files
                    DEFAULT IS ./artifacts (for CI GHA artifact download!)
--npm-dir           where per-platform npm/ dirs live (default: <cwd>/npm)
--build-output-dir  only needed for wasm32-wasi-* targets
```

**DEVIATION FROM PLAN:** The plan does not mention `--output-dir`. In CI the GHA action downloads per-platform `.node` files into an `./artifacts/` dir, so the default works in CI. For local single-host use you need `--output-dir .` to pick up the `.node` from the crate root. Tasks 4–5 must NOT pass `--output-dir` (CI default `./artifacts` is correct for CI).

The `--npm-dir` flag in the help example is shown as `--dist` but the correct flag is `--npm-dir`. There is no `--dist` flag.

### `napi prepublish`

```
--cwd                    working dir
--npm-dir,-p             where per-platform npm/ dirs live
--tag-style,--tagstyle,-t  git tag style: `npm` or `lerna` (DEFAULT: `lerna`)
--gh-release             boolean flag — PRESENCE creates a GitHub release
--gh-release-name        release name
--gh-release-id          existing release id to update
--skip-optional-publish  skip publishing optionalDependencies packages
--dry-run                dry run (no filesystem changes)
```

**CRITICAL DEVIATION FROM PLAN:** The plan uses `--no-gh-release`. This flag does NOT exist. `--gh-release` is a boolean presence flag; omitting it means no GitHub release is created. The correct CI invocation is: `napi prepublish --dry-run` (for dry-run verify) or `napi prepublish --gh-release --gh-release-id $GH_RELEASE_ID` (for real publish).

**`--tag-style` default: `lerna`** — format is `@paigasus/node-bindings@v0.1.0` (i.e., package-name + at + tag). The `npm` style format is `v0.1.0` only. Tasks 4–5 should use `--tag-style lerna` or omit (default is lerna).

**`prepublish --dry-run` behavior observed:** exits 0 with zero output (completely silent). Does NOT create a GitHub release. Does NOT modify any file. Confirmed this is safe to run in CI without `--gh-release`.

---

## Step 2 — `create-npm-dirs` output + per-platform package.json shape

Command used:
```bash
pnpm exec napi create-npm-dirs --cwd "$CRATE"
```

Generated `npm/` subdirs (one per target in `napi.targets`):
```
npm/
  darwin-arm64/
  darwin-x64/
  linux-arm64-gnu/
  linux-arm64-musl/
  linux-x64-gnu/
  linux-x64-musl/
  win32-x64-msvc/
```

**Note:** Rust triple `aarch64-apple-darwin` → npm dir `darwin-arm64` (napi maps the triple to npm platform string).

### darwin-arm64/package.json (no libc field — darwin doesn't need it):
```json
{
  "name": "@paigasus/node-bindings-darwin-arm64",
  "version": "0.0.0",
  "cpu": ["arm64"],
  "main": "paigasus-node-bindings.darwin-arm64.node",
  "files": ["paigasus-node-bindings.darwin-arm64.node"],
  "license": "Apache-2.0",
  "os": ["darwin"]
}
```

### linux-x64-gnu/package.json (glibc):
```json
{
  "name": "@paigasus/node-bindings-linux-x64-gnu",
  "version": "0.0.0",
  "cpu": ["x64"],
  "main": "paigasus-node-bindings.linux-x64-gnu.node",
  "files": ["paigasus-node-bindings.linux-x64-gnu.node"],
  "license": "Apache-2.0",
  "os": ["linux"],
  "libc": ["glibc"]
}
```

### linux-x64-musl/package.json (musl):
```json
{
  "name": "@paigasus/node-bindings-linux-x64-musl",
  "version": "0.0.0",
  "cpu": ["x64"],
  "main": "paigasus-node-bindings.linux-x64-musl.node",
  "files": ["paigasus-node-bindings.linux-x64-musl.node"],
  "license": "Apache-2.0",
  "os": ["linux"],
  "libc": ["musl"]
}
```

Pattern: `name`, `version`, `cpu`, `main` (just the `.node` filename), `files` (just the `.node`), `license`, `os`, and (for linux) `libc`. No `engines`, no `peerDependencies`.

---

## Step 3 — Dry-run prepublish + npm pack

### `napi artifacts --output-dir .`

With `--output-dir .` (crate root, for local/single-host use):
- Reads `paigasus-node-bindings.darwin-arm64.node` from crate root
- Copies it to `npm/darwin-arm64/paigasus-node-bindings.darwin-arm64.node`
- Also writes a copy back to crate root (idempotent)
- Logs: `Read [/path/to/crate/paigasus-node-bindings.darwin-arm64.node]` and two `Write file content to [...]` lines

**In CI:** `napi artifacts` (no `--output-dir`) reads from `./artifacts/` (the GHA download-artifact default output). Do NOT add `--output-dir` in the CI workflow.

### `napi prepublish --dry-run`

Exit code 0, zero output, no files modified. Safe to run in CI for verification.

### `npm pack --dry-run` (current `files: ["index.js","index.d.ts","*.node"]`):

Lists 4 files: `index.d.ts`, `index.js`, `package.json`, `paigasus-node-bindings.darwin-arm64.node`. The `.node` IS included. After Task 2 removes `*.node` from `files`, only 3 files will be listed. This is the correct and expected behavior post-Task-2.

---

## Step 4 — Install-resolution recipe (CONFIRMED WORKING)

### Tested incantation (darwin-arm64):

```bash
CRATE="$(pwd)/rs/crates/bindings/paigasus-node-bindings"
# After Task 2: files: ["index.js","index.d.ts"] (no *.node)
MAIN_TGZ="$CRATE/paigasus-node-bindings-0.0.0.tgz"          # from: cd $CRATE && npm pack
PLAT_TGZ="$CRATE/npm/darwin-arm64/paigasus-node-bindings-darwin-arm64-0.0.0.tgz"  # from: cd $CRATE/npm/darwin-arm64 && npm pack
SMOKE=$(mktemp -d)
cd "$SMOKE"
npm init -y >/dev/null
npm install "$MAIN_TGZ" "$PLAT_TGZ"
node -e "const b=require('@paigasus/node-bindings'); if (b.sum(2,3)!==5) { process.exit(1);} console.log('resolved + loaded OK');"
```

**Output: `resolved + loaded OK`** — exit 0.

### Resolution mechanism (from index.js loader analysis):

The generated `index.js` loader for darwin-arm64 does:
1. `try { return require('./paigasus-node-bindings.darwin-arm64.node') }` — tries LOCAL file first (in the main package dir). If `files` does not include `*.node`, this file is absent → throws.
2. `catch(e) { loadErrors.push(e) }` — silently catches, falls through.
3. `try { const binding = require('@paigasus/node-bindings-darwin-arm64') }` — resolves the PLATFORM PACKAGE.
4. Version check: `if (bindingVersion !== mainVersion && process.env.NAPI_RS_ENFORCE_VERSION_CHECK && env !== '0')` — only throws if `NAPI_RS_ENFORCE_VERSION_CHECK` is set. **With env var unset (the default), version `0.0.0` loads fine.**

So:
- `NAPI_RS_ENFORCE_VERSION_CHECK` does NOT need to be set or unset — default (unset) is correct.
- The `.node` file is resolved from `node_modules/@paigasus/node-bindings-darwin-arm64/paigasus-node-bindings.darwin-arm64.node` (confirmed via `require.resolve()`).

### For CI (linux-x64-gnu):

Swap `darwin-arm64` for `linux-x64-gnu`. The mechanism is identical. The verify job just uses the linux-x64-gnu platform tarballs and runs the same `node -e` check. No special env vars needed.

---

## Step 5 — Deviations from plan that Tasks 4–5 must incorporate

| Plan assumption | Reality | Action for Tasks 4–5 |
|---|---|---|
| `prepublish --no-gh-release` exists | Flag does NOT exist. `--gh-release` is boolean (presence = create release). | Use `napi prepublish --dry-run` (no `--gh-release`) for CI verify. For real publish: `napi prepublish --gh-release --gh-release-id $ID`. |
| `napi artifacts` (no flags) works for local | Default `--output-dir` is `./artifacts` (for GHA download dir). Local builds need `--output-dir .`. | CI workflow: do NOT add `--output-dir` (default `./artifacts` is correct because `actions/download-artifact` writes there). |
| `--tag-style` default unclear | Default is `lerna` (confirmed in source: line 3277 of cli.js). | Explicit `--tag-style lerna` optional but not needed; just omit. |
| `artifacts --npm-dir` vs `--dist` | Help example shows `--dist` but actual flag is `--npm-dir`. | Use `--npm-dir npm` explicitly to match `create-npm-dirs` output. |
| `NAPI_RS_ENFORCE_VERSION_CHECK=0` needed | Not needed. The check is gated on the env var being SET. With it unset (default), version check is skipped entirely. | Do NOT set this env var in CI. |
| `npm install` links optional deps from local tarballs | Works correctly with `npm install main.tgz platform.tgz` (both as positional args). Both land in `node_modules/@paigasus/`. | CI verify: pack both, install both in tmp dir, run node assertion. |

---

## Cleanup confirmed

```
git status --porcelain  # output: (empty — completely clean)
```

- `rs/crates/bindings/paigasus-node-bindings/package.json` — restored to original (no `napi.targets`, `files` has `*.node` back).
- `npm/` dir — removed.
- `*.node` + `*.tgz` in crate dir — removed.
- Smoke temp dir — removed.
