# Stand up a TS kernel binding (napi-rs) + wire the cascade Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a kernel value cross `Rust kernel → napi-rs → .node addon → Node/TypeScript` at runtime, prove it with a vitest, wire the Moon cascade `paigasus-kernel-rs → paigasus-node-bindings-rs → paigasus-kernel-ts`, and extend the affected-graph guard so a kernel/binding edit cascades into the TS stack.

**Architecture:** Co-located napi layout (the Node mirror of the SMA-419 Python wheel): a new Rust crate `rs/crates/bindings/paigasus-node-bindings` (cdylib) wraps `paigasus-kernel`, with a co-located `package.json` (`@paigasus/node-bindings`) beside its `Cargo.toml`. `@napi-rs/cli`'s `napi build` runs cargo from *inside* `rs/` (so `rs/.cargo/config.toml`'s macOS link flags resolve) and post-processes the cdylib into a `.node` addon + generated `index.js`/`index.d.ts`. `ts/packages/paigasus-kernel` (`@paigasus/kernel`) depends on it via a pnpm `file:` link and re-exports `sum` behind a `node`-conditioned `exports` map (a `default` stub throws until the wasm sibling lands). Moon gets the new edges; the affected-graph guard moves `paigasus-node-bindings-rs` + `paigasus-kernel-ts` into the kernel-edit must-include set.

**Tech Stack:** Rust (napi 3 / napi-derive 3 / napi-build, cdylib), `@napi-rs/cli` 3, pnpm workspace (catalog: vitest), Moon 2.3.2, bash guard script. wasm (`paigasus-wasm`) is a deferred follow-up.

**Spec:** `docs/superpowers/specs/2026-06-15-sma-420-ts-kernel-napi-binding-design.md`

---

## Conventions for every task

- Run all commands from the repo root `/Users/smaschek/dev/paigasus/paigasus-core` unless a step says otherwise. (Watch the shell cwd — `cd ts && …` persists; prefer subshells `(cd ts && …)`.)
- Proto-managed tools (`moon`, `pnpm`/node, `cargo`/rust, `buf`) are off the default non-interactive PATH. Ensure they're reachable first: `export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"`. There is no macOS `timeout` binary.
- Commits are **SSH-signed via 1Password** (`op-ssh-sign`). If `git commit` fails with `1Password: failed to fill whole buffer`, 1Password is locked — unlock it and retry. A `commit-msg` lefthook runs commitlint: Conventional Commits, **scope required**, allowed types `feat|fix|docs|chore|refactor|test|ci|build|perf|style|revert`, allowed scopes `rs|py|ts|contracts|ci|docs|deps|release|repo|claude|workspace`, header ≤100 chars, body lines ≤100 chars, subject must **not** start upper-case (lead with a lowercase verb; put `SMA-420` mid/end).
- Branch is already `feature/sma-420-stand-up-a-ts-kernel-binding-wasmnapi-wire-the-cascade-to`.
- End every commit body with the footer (blank line before it):
  `Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>`

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `rs/Cargo.toml` | add `napi`/`napi-derive` (deps) + `napi-build` (build-dep) to `[workspace.dependencies]` | **Modify** |
| `rs/crates/bindings/paigasus-node-bindings/Cargo.toml` | napi cdylib crate wrapping the kernel | **Create** |
| `rs/crates/bindings/paigasus-node-bindings/build.rs` | `napi_build::setup()` (cdylib link setup) | **Create** |
| `rs/crates/bindings/paigasus-node-bindings/src/lib.rs` | `#[napi] fn sum` → calls `paigasus_kernel::sum` | **Create** |
| `rs/crates/bindings/paigasus-node-bindings/package.json` | `@paigasus/node-bindings` — napi config + `@napi-rs/cli` + build script | **Create** |
| `rs/crates/bindings/paigasus-node-bindings/.gitignore` | ignore the platform `*.node` (commit the generated `index.js`/`index.d.ts`) | **Create** |
| `rs/crates/bindings/paigasus-node-bindings/index.js` + `index.d.ts` | generated napi loader + types (committed, repo commit-generated-code posture) | **Create** (by `napi build`) |
| `rs/crates/bindings/paigasus-node-bindings/moon.yml` | `paigasus-node-bindings-rs`: `dependsOn` kernel-rs, `^:build` | **Create** |
| `rs/.cargo/config.toml` | broaden the comment to name napi alongside PyO3 | **Modify** |
| `ts/packages/paigasus-kernel/package.json` | `file:` dep, conditional `exports`, vitest devDep | **Modify** |
| `ts/packages/paigasus-kernel/src/index.ts` | `node` re-export of `sum` | **Modify** |
| `ts/packages/paigasus-kernel/src/unsupported.ts` | throwing stub for non-node runtimes | **Create** |
| `ts/packages/paigasus-kernel/tests/sum.test.ts` | vitest runtime FFI smoke test | **Create** |
| `ts/packages/paigasus-kernel/moon.yml` | `dependsOn` node-bindings-rs; `build`/`test` override (napi build) | **Modify** |
| `ts/pnpm-lock.yaml` | records the `file:` link + `@napi-rs/cli` + vitest | **Modify** (regenerated) |
| `ci/affected-graph/run.sh` | kernel→bindings case + new `binding-oneway-node` case | **Modify** |
| `ci/affected-graph/README.md` | guard maintenance note: the ts edge landed | **Modify** |
| `moon.yml` (root) | add `rs/crates/*/*/package.json` to `affected-smoke` inputs | **Modify** |
| `docs/superpowers/specs/2026-06-16-sma-420-spike-findings.md` | record the spike answers (load-bearing) | **Create** |

---

## Task 1: Spike — stand up the napi binding crate + validate the chain on macOS (load-bearing; do this first)

Exploratory by design (the spec's headline risk). It creates the **real** binding crate (needed regardless), validates the napi integration against concrete checks on the macOS host, and **records the answers** in a findings note that Tasks 2–3 depend on. If a check fails, stop and apply its fallback before proceeding.

**Files:**
- Modify: `rs/Cargo.toml`
- Create: `rs/crates/bindings/paigasus-node-bindings/{Cargo.toml,build.rs,src/lib.rs,package.json,.gitignore,moon.yml}`
- Modify: `rs/.cargo/config.toml`
- Create: `docs/superpowers/specs/2026-06-16-sma-420-spike-findings.md`

- [ ] **Step 1: Add napi-rs to the Cargo workspace deps**

In `rs/Cargo.toml`, add to `[workspace.dependencies]` (next to the existing `pyo3` block):

```toml
# napi-rs — Rust↔Node FFI for the node binding crate (ADR-0005). napi/napi-derive are the
# runtime + proc-macro (consumed via #[napi] macros); napi-build runs in build.rs to set up the
# cdylib link. Node resolves N-API symbols at load (like CPython resolves PyO3's), so plain
# `cargo build` links the cdylib without an embedded Node; macOS needs the rs/.cargo/config.toml
# `-undefined dynamic_lookup` flags (shared with PyO3). Versions track the @napi-rs/cli 3 line
# (Polyglot Monorepo Scoping §3); spike confirms the napi-build pairing.
napi = { version = "3", default-features = false, features = ["napi8"] }
napi-derive = "3"
napi-build = "2"
```

> **Spike-contingent (S-versions):** if cargo can't resolve `napi-build = "2"` against `napi 3`, use the version cargo reports as compatible (record it in findings). If the `napi8` feature is unknown for the resolved major, drop `features`/`default-features` and use `napi = "3"`.

- [ ] **Step 2: Create the crate's `Cargo.toml`**

Create `rs/crates/bindings/paigasus-node-bindings/Cargo.toml` (mirrors `paigasus-py-bindings/Cargo.toml`):

```toml
[package]
name = "paigasus-node-bindings"
version = "0.0.0"
edition.workspace = true
license.workspace = true
rust-version.workspace = true
authors.workspace = true
publish = false

[lib]
# Only cdylib: Node loads this artifact (renamed to .node by `napi build`); no other Rust crate
# consumes it. A napi cdylib leaves N-API symbols undefined (Node resolves them at load), so a
# Rust test harness for this target can't link. Kernel logic is unit-tested in paigasus-kernel;
# the FFI boundary is proven by compilation + the runtime vitest. Disable the (un-linkable)
# test/doctest targets so `cargo nextest --no-tests=pass` stays green (mirrors paigasus-py-bindings).
crate-type = ["cdylib"]
test = false
doctest = false

[dependencies]
napi.workspace = true
napi-derive.workspace = true
paigasus-kernel.workspace = true

[build-dependencies]
napi-build.workspace = true

[package.metadata.cargo-machete]
# napi + napi-derive are consumed only through attribute macros (#[napi]) — the canonical
# cargo-machete false-positive (like pyo3 on the py crate); :machete is a blocking gate (SMA-375).
# napi-build is used in build.rs and paigasus-kernel is called directly — neither needs an ignore.
ignored = ["napi", "napi-derive"]

[lints]
workspace = true
```

- [ ] **Step 3: Create `build.rs` and `src/lib.rs`**

Create `rs/crates/bindings/paigasus-node-bindings/build.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0
fn main() {
    napi_build::setup();
}
```

Create `rs/crates/bindings/paigasus-node-bindings/src/lib.rs`:

```rust
// SPDX-License-Identifier: Apache-2.0

//! napi-rs binding shim for `paigasus-kernel` (ADR-0005): exposes the kernel's pure functions
//! to Node/TypeScript. Compiled as a cdylib that `@napi-rs/cli` post-processes into a `.node`
//! addon. The affected-graph cascade `paigasus-kernel-rs → paigasus-node-bindings-rs` is proven
//! by this crate compiling against a real `paigasus_kernel::*` call (SMA-420).

use napi_derive::napi;

/// Node-callable wrapper over [`paigasus_kernel::sum`] (the canonical first-binding shape — a
/// real value crossing the FFI boundary). Uses `i32` so napi-rs maps the surface to a JS
/// `number` deterministically (spec decision #5 / review F3): an `i64` return can surface as a
/// `BigInt` on some napi-rs versions (`5n !== 5`). The kernel fn is `i64`; we cast at the
/// boundary. A future kernel fn needing the full `i64` range gets explicit BigInt handling then.
#[napi]
pub fn sum(a: i32, b: i32) -> i32 {
    paigasus_kernel::sum(a as i64, b as i64) as i32
}
```

- [ ] **Step 4: Create the co-located `package.json`, `.gitignore`, and `moon.yml`**

Create `rs/crates/bindings/paigasus-node-bindings/package.json` (SPDX-exempt config file; the co-located npm package — the napi analog of SMA-419's co-located maturin `pyproject.toml`):

```json
{
  "name": "@paigasus/node-bindings",
  "version": "0.0.0",
  "private": true,
  "license": "Apache-2.0",
  "main": "index.js",
  "types": "index.d.ts",
  "napi": {
    "binaryName": "paigasus-node-bindings"
  },
  "files": ["index.js", "index.d.ts", "*.node"],
  "scripts": {
    "build": "napi build",
    "build:release": "napi build --release"
  },
  "devDependencies": {
    "@napi-rs/cli": "^3"
  }
}
```

> **Spike-contingent (S2):** the napi config key is `binaryName` for `@napi-rs/cli` v3; if the resolved cli rejects it (v2 used `name`), use what its `--help`/error reports. Confirm the build output filenames (`index.node` vs `paigasus-node-bindings.<platform>.node` + a generated `index.js` loader); record them.

Create `rs/crates/bindings/paigasus-node-bindings/.gitignore` (the platform binary is rebuilt per host; the generated `index.js`/`index.d.ts` are **committed**, matching the repo's commit-generated-code posture — Scoping §1 — so `@paigasus/kernel`'s `typecheck` resolves the binding's types without a prebuild):

```gitignore
# Platform-specific napi addon — rebuilt by `napi build` per host/CI; never committed.
*.node
```

Create `rs/crates/bindings/paigasus-node-bindings/moon.yml` (mirrors `paigasus-py-bindings/moon.yml`):

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-node-bindings-rs'
# Moon-side layer label for this FFI crate (no native `binding` layer exists). Built like a
# library but NOT published as an rlib — ships as a Node .node addon via @napi-rs/cli.
layer: 'library'
language: 'rust'

# The kernel→binding edge (ADR-0005): a kernel change must rebuild this crate. The task-level
# `^:build` is what propagates `affected` in `moon ci --include-relations` — a project-level
# `dependsOn` alone does NOT (SMA-389 D3). Mirrors paigasus-py-bindings-rs.
dependsOn:
  - 'paigasus-kernel-rs'

tasks:
  build:
    deps: ['^:build']
  test:
    deps: ['^:build']
```

- [ ] **Step 5: Broaden the `.cargo/config.toml` comment**

In `rs/.cargo/config.toml`, the link flags already cover napi (Node resolves N-API symbols at load, exactly like libpython). Update only the comment's first sentence so it names both bindings:

Replace:
```
# SMA-409: paigasus-py-bindings is a PyO3 `extension-module` cdylib — at link time its
# libpython symbols are intentionally undefined (CPython resolves them when it loads the
# module). Linux/ELF permits undefined symbols in shared objects by default; the macOS
```
with:
```
# SMA-409/420: the FFI binding cdylibs (paigasus-py-bindings = PyO3 extension-module;
# paigasus-node-bindings = napi-rs) leave their host-runtime symbols intentionally undefined at
# link time — CPython resolves _Py* and Node resolves napi_* when they load the module.
# Linux/ELF permits undefined symbols in shared objects by default; the macOS
```

- [ ] **Step 6: Verify the Rust gate compiles + the napi build links + imports (the spike checks)**

Run each command; record the result in the findings note (Step 7). Stop on the first hard failure and apply its fallback.

```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"

# (S1 macOS link, gate) The crate compiles as a cdylib via plain cargo (the fmt/clippy/nextest
# gate path). Expected: builds; on macOS NO "Undefined symbols ... napi_*" linker error.
moon run paigasus-node-bindings-rs:build 2>&1 | tee /tmp/sma420-cargo.log
# S1 FALLBACK if it fails to link on macOS: confirm build.rs ran (napi_build::setup) and that
# rs/.cargo/config.toml's apple-darwin flags are picked up (cargo must run from inside rs/ — moon
# runs the task from the crate dir, so this holds). Record the exact error.

# (S2 napi-cli + file:) Install so @napi-rs/cli is present, then build the .node from INSIDE the
# crate dir (cwd stays in rs/ → link flags resolve). Expected: emits index.node + index.js +
# index.d.ts in the crate dir.
(cd ts && pnpm install) 2>&1 | tee /tmp/sma420-pnpm.log   # links @paigasus/node-bindings devDeps once kernel file: dep is added in Task 2; for the spike, install @napi-rs/cli directly if needed:
(cd rs/crates/bindings/paigasus-node-bindings && pnpm install && pnpm run build) 2>&1 | tee /tmp/sma420-napi.log
ls -1 rs/crates/bindings/paigasus-node-bindings/*.node rs/crates/bindings/paigasus-node-bindings/index.{js,d.ts}
# S2 FALLBACK: if `napi` is not found, @napi-rs/cli didn't install — record how it must be
# provisioned (devDep on the co-located package installed via the ts pnpm workspace `file:` link
# in Task 2, vs a direct install here). If the .node fails to link, see S1.

# (S3 import + S4 number mapping) Load the addon from Node and confirm the value + the type.
node --input-type=module -e "import b from './rs/crates/bindings/paigasus-node-bindings/index.js'; console.log(b.sum(2,3))"
grep -n "sum" rs/crates/bindings/paigasus-node-bindings/index.d.ts
# S3: prints exactly 5. S4: index.d.ts shows `sum(a: number, b: number): number` (NOT bigint).
# S4 FALLBACK: if it shows bigint, the i32 narrowing in src/lib.rs didn't take — re-check the
# signature is i32 (not i64). Record the generated signature verbatim.

# (S5 cache-bust) A kernel Rust-source edit must trigger a real napi recompile, not a cache hit.
sed -i.bak 's/a + b/a + b + 0/' rs/crates/libs/paigasus-kernel/src/lib.rs   # no-op value change
(cd rs/crates/bindings/paigasus-node-bindings && pnpm run build) 2>&1 | tee /tmp/sma420-rebuild.log
mv rs/crates/libs/paigasus-kernel/src/lib.rs.bak rs/crates/libs/paigasus-kernel/src/lib.rs       # revert
grep -iE "Compiling paigasus|Finished" /tmp/sma420-rebuild.log
# S5: the rebuild log shows `Compiling paigasus-kernel` + `Compiling paigasus-node-bindings`
#   (not an all-cached build). Record whether `napi build` alone re-triggers cargo on a source
#   change — Task 2's test task relies on this for the F4 cache-bust.
```

- [ ] **Step 7: Write the findings note**

Create `docs/superpowers/specs/2026-06-16-sma-420-spike-findings.md` recording, per check: PASS/FAIL, observed behavior, and the decision it drives. Explicitly record: (S2) the exact `napi build` invocation Task 2 will run from the kernel-ts task and how `@napi-rs/cli`/the `file:` link resolve; (S2) the generated artifact filenames; (S4) the generated `sum` signature; (S5) the freshness mechanism; (S-versions) the resolved napi/napi-build versions; and whether a `vitest.config.ts` will be needed (carried into Task 2 S6).

- [ ] **Step 8: Commit the crate + findings (clean the spike's generated `.node`/installs first)**

```bash
# index.js + index.d.ts ARE committed (generated, repo posture); *.node is gitignored.
git add rs/Cargo.toml \
        rs/crates/bindings/paigasus-node-bindings/Cargo.toml \
        rs/crates/bindings/paigasus-node-bindings/build.rs \
        rs/crates/bindings/paigasus-node-bindings/src/lib.rs \
        rs/crates/bindings/paigasus-node-bindings/package.json \
        rs/crates/bindings/paigasus-node-bindings/.gitignore \
        rs/crates/bindings/paigasus-node-bindings/index.js \
        rs/crates/bindings/paigasus-node-bindings/index.d.ts \
        rs/crates/bindings/paigasus-node-bindings/moon.yml \
        rs/.cargo/config.toml \
        docs/superpowers/specs/2026-06-16-sma-420-spike-findings.md
git commit -m "feat(rs): stand up paigasus-node-bindings napi crate + validate the chain (SMA-420)

Spike findings recorded in docs/superpowers/specs/2026-06-16-sma-420-spike-findings.md.

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 2: Re-export the public surface via `@paigasus/kernel` + runtime FFI smoke test (TDD)

**Files:**
- Modify: `ts/packages/paigasus-kernel/package.json`
- Modify: `ts/packages/paigasus-kernel/src/index.ts`
- Create: `ts/packages/paigasus-kernel/src/unsupported.ts`
- Test: `ts/packages/paigasus-kernel/tests/sum.test.ts`
- Modify: `ts/packages/paigasus-kernel/moon.yml`

- [ ] **Step 1: Write the failing test**

Create `ts/packages/paigasus-kernel/tests/sum.test.ts`:

```typescript
// SPDX-License-Identifier: Apache-2.0
import { describe, expect, it } from "vitest";
import { sum } from "@paigasus/kernel";

describe("kernel FFI", () => {
  it("crosses the napi boundary", () => {
    expect(sum(2, 3)).toBe(5);
    expect(sum(-4, 4)).toBe(0);
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run:
```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
(cd ts && pnpm install && pnpm --filter @paigasus/kernel exec vitest run)
```
Expected: FAIL — vitest can't resolve `@paigasus/kernel`'s `sum` (the wrapper still has `export {};` and no `@paigasus/node-bindings` dep / vitest devDep yet). (`pnpm install` may itself error until Step 3 adds the `file:` dep + vitest — that counts as red.)

- [ ] **Step 3: Wire the wrapper — `file:` dep, conditional exports, vitest devDep**

Replace `ts/packages/paigasus-kernel/package.json` with:

```json
{
  "name": "@paigasus/kernel",
  "_comment_exports": "node → the napi binding (loads a compiled .node); default (browser/Edge/workerd) → a stub that throws until paigasus-wasm lands (SMA-420). Conditions point at SOURCE (bundler-aware consumers walk TS); switch to ./dist/* when tsup wiring lands, IN LOCKSTEP with flipping `private: false`.",
  "version": "0.0.0",
  "private": true,
  "type": "module",
  "license": "Apache-2.0",
  "exports": {
    ".": {
      "node": "./src/index.ts",
      "default": "./src/unsupported.ts"
    }
  },
  "scripts": {
    "typecheck": "tsc -p tsconfig.json --noEmit"
  },
  "dependencies": {
    "@paigasus/node-bindings": "file:../../../rs/crates/bindings/paigasus-node-bindings"
  },
  "devDependencies": {
    "typescript": "catalog:",
    "vitest": "catalog:"
  }
}
```

Replace `ts/packages/paigasus-kernel/src/index.ts` with:

```typescript
// SPDX-License-Identifier: Apache-2.0
export { sum } from "@paigasus/node-bindings";
```

Create `ts/packages/paigasus-kernel/src/unsupported.ts`:

```typescript
// SPDX-License-Identifier: Apache-2.0
throw new Error(
  "@paigasus/kernel has no browser/Edge binding yet — wasm (paigasus-wasm) is a tracked follow-up",
);
```

- [ ] **Step 4: Override the kernel-ts `build`/`test` so they produce the `.node` (review F1)**

Replace `ts/packages/paigasus-kernel/moon.yml` with the following. `^:build` only builds the *upstream* `paigasus-node-bindings-rs:build` (= plain `cargo build` → the cdylib in `rs/target`, NOT the `index.node`), and Moon does not auto-order a project's `test` after its own `build` — so the napi build is run *inside* these tasks (the napi mirror of the py `test`'s explicit `uv sync --reinstall-package … && pytest`):

```yaml
$schema: 'https://moonrepo.dev/schemas/project.json'

id: 'paigasus-kernel-ts'
layer: 'library'
language: 'typescript'

# Cross-language edge to the napi binding crate (ADR-0005). dependsOn buys build ordering +
# affected-graph propagation under `moon ci --include-relations` (the task-level `^:build` carries
# the cascade; a project dependsOn alone does not — SMA-389 D3), and provisions the Rust toolchain
# in this project's task context so `napi build` can shell out to cargo.
dependsOn:
  - 'paigasus-node-bindings-rs'

tasks:
  # Produce the .node + regenerate the committed glue, THEN typecheck. Overrides the inherited
  # `tsc --noEmit` build (which would not produce the addon). `^:build` only builds the upstream
  # cargo gate, not this artifact (review F1).
  build:
    command: 'pnpm --filter @paigasus/node-bindings build && pnpm exec tsc -p tsconfig.json --noEmit'
    deps: ['^:build']
    inputs:
      - 'src/**/*'
      - 'tsconfig.json'
      - 'package.json'
      - '/ts/tsconfig.base.json'
      - '/ts/pnpm-lock.yaml'
      - '/rs/crates/bindings/paigasus-node-bindings/src/**/*'
    outputs:
      - '/rs/crates/bindings/paigasus-node-bindings/index.node'
  # Rebuild the .node fresh (cache-bust on a Rust edit, review F4), THEN run vitest. Mirrors the
  # py test task's `uv sync --reinstall-package … && pytest`.
  test:
    command: 'pnpm --filter @paigasus/node-bindings build && pnpm exec vitest run'
    deps: ['^:build']
    inputs:
      - 'src/**/*'
      - 'tests/**/*'
      - 'package.json'
      - '/ts/pnpm-lock.yaml'
      - '/rs/crates/bindings/paigasus-node-bindings/src/**/*'
```

> **Spike-contingent (from Task 1 S2 + S6), apply before running Step 5:**
> - **S2 (invocation/cwd):** if `pnpm --filter @paigasus/node-bindings build` does not resolve the `file:`-linked package, use the exact invocation the spike recorded that runs `napi build` with cargo cwd **inside `rs/`** (e.g. a `script:` that `cd`s into the crate dir then `pnpm exec napi build`). Update both `command`s identically.
> - **S2 (output filename):** if the spike showed a platform-suffixed `*.node` (not `index.node`), fix the `build` task's `outputs:` accordingly.
> - **S6 (vitest loads the addon):** if vitest fails to import the `.node`, create `ts/packages/paigasus-kernel/vitest.config.ts` externalizing the native addon, e.g.
>   `export default { test: { server: { deps: { external: [/\.node$/, "@paigasus/node-bindings"] } } } };`
>   and add it to the `test` task `inputs`.

- [ ] **Step 5: Run the test to verify it passes**

Run:
```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
moon run paigasus-kernel-ts:test
```
Expected: PASS — the task runs `napi build` (fresh `.node`) then vitest; `kernel FFI › crosses the napi boundary` passes.

- [ ] **Step 6: Confirm the whole-tree gates still pass**

Run:
```bash
moon run paigasus-kernel-ts:build paigasus-kernel-ts:typecheck ts:lint ts:fmt
```
Expected: all PASS. `build` produces the addon then typechecks; `typecheck` resolves `@paigasus/node-bindings` via its committed `index.d.ts`; lint/fmt are clean (the new `tests/` + `unsupported.ts` are prettier/eslint-clean).

- [ ] **Step 7: Commit (include the regenerated lockfile + any regenerated binding glue)**

```bash
git add ts/packages/paigasus-kernel/package.json \
        ts/packages/paigasus-kernel/src/index.ts \
        ts/packages/paigasus-kernel/src/unsupported.ts \
        ts/packages/paigasus-kernel/tests/sum.test.ts \
        ts/packages/paigasus-kernel/moon.yml \
        ts/pnpm-lock.yaml \
        rs/crates/bindings/paigasus-node-bindings/index.js \
        rs/crates/bindings/paigasus-node-bindings/index.d.ts
# add ts/packages/paigasus-kernel/vitest.config.ts too if S6 required it
git commit -m "feat(ts): re-export sum via the napi binding + runtime FFI smoke test (SMA-420)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 3: Extend the affected-graph guard for the kernel→ts cascade

**Files:**
- Modify: `ci/affected-graph/run.sh`
- Modify: `moon.yml` (root — `affected-smoke` inputs)
- Modify: `ci/affected-graph/README.md`

- [ ] **Step 1: Verify the cascade reaches the ts wrapper (pre-check)**

Run:
```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
printf '%s\n' 'rs/crates/libs/paigasus-kernel/src/lib.rs' \
  | moon query projects --affected --downstream deep \
  | python3 -c 'import sys,json;print(sorted(p["id"] for p in json.load(sys.stdin)["projects"]))'
```
Expected: includes `paigasus-kernel-rs`, `paigasus-py-bindings-rs`, `paigasus-node-bindings-rs`, `paigasus-kernel-py`, **`paigasus-kernel-ts`**, `paigasus-gateway-rs` (and `repo`); and does **not** include any other `-ts` project.

- [ ] **Step 2: Update the `kernel->bindings` case (must-include + forbid-regex)**

In `ci/affected-graph/run.sh`, replace the `kernel->bindings` case:

```bash
  run_case "kernel->bindings" "rs/crates/libs/paigasus-kernel/src/lib.rs" \
    "paigasus-kernel-rs,paigasus-py-bindings-rs,paigasus-gateway-rs,paigasus-kernel-py" \
    '-ts$|^contracts$|^py$|^ts$|^paigasus-(proto|workflows|ml)-py$'
```

with (adds the node binding + ts wrapper to must-include; drops the blanket `-ts$` forbid but still forbids the *unrelated* ts projects + the existing non-ts negatives — SMA-409 F5 / SMA-419 §5):

```bash
  # kernel edit -> kernel + both bindings + gateway + both language wrappers (SMA-419/420). Still
  # nothing else cross-stack: no contracts / py root, no UNRELATED py packages (proto/workflows/ml),
  # and no UNRELATED ts packages (proto/sdk/ui/console/docs/commitlint-config).
  run_case "kernel->bindings" "rs/crates/libs/paigasus-kernel/src/lib.rs" \
    "paigasus-kernel-rs,paigasus-py-bindings-rs,paigasus-gateway-rs,paigasus-kernel-py,paigasus-node-bindings-rs,paigasus-kernel-ts" \
    '^(commitlint-config|paigasus-console|paigasus-docs|paigasus-proto|paigasus-sdk|paigasus-ui)-ts$|^contracts$|^py$|^ts$|^paigasus-(proto|workflows|ml)-py$'
```

- [ ] **Step 3: Add the `binding-oneway-node` case**

In `ci/affected-graph/run.sh`, immediately after the existing `binding-oneway` (py) `run_case`, add:

```bash
  # node binding edit -> the node binding + the ts wrapper that depends on it (SMA-420); still
  # one-directional w.r.t. the kernel (must not drag in paigasus-kernel-rs).
  run_case "binding-oneway-node" "rs/crates/bindings/paigasus-node-bindings/src/lib.rs" \
    "paigasus-node-bindings-rs,paigasus-kernel-ts" '^paigasus-kernel-rs$'
```

(The existing `contracts->proto`, `binding-oneway` (py), `--negative-control` (uses `paigasus-proto-py`, still not a kernel dependent), and `assert_include_relations` are unchanged.)

- [ ] **Step 4: Add the co-located `package.json` to the `affected-smoke` inputs**

In the root `moon.yml`, the `affected-smoke` task watches graph-defining files but not the new co-located `package.json`. Add it under the existing `rs/crates/*/*/pyproject.toml` line:

```yaml
      - 'rs/crates/*/*/pyproject.toml'
      - 'rs/crates/*/*/package.json'
```

- [ ] **Step 5: Update the guard README**

In `ci/affected-graph/README.md`, replace the kernel/binding bullets:

```markdown
- **kernel edit** → `paigasus-kernel-rs` + `paigasus-py-bindings-rs` + `paigasus-gateway-rs`
  + `paigasus-kernel-py` (the py wrapper now wraps the wheel, SMA-419); still **no `*-ts` /
  `contracts` / unrelated `*-py`** (`paigasus-proto/workflows/ml-py`).
- **binding edit** → `paigasus-py-bindings-rs` + `paigasus-kernel-py`; still one-directional
  w.r.t. the kernel (never drags in `paigasus-kernel-rs`).
```

with:

```markdown
- **kernel edit** → `paigasus-kernel-rs` + `paigasus-py-bindings-rs` + `paigasus-node-bindings-rs`
  + `paigasus-gateway-rs` + `paigasus-kernel-py` + `paigasus-kernel-ts` (both language wrappers now
  wrap their bindings, SMA-419/420); still **no `contracts` / unrelated `*-py`
  (`paigasus-proto/workflows/ml-py`) / unrelated `*-ts`** (`paigasus-proto/sdk/ui/console/docs-ts`,
  `commitlint-config-ts`).
- **py binding edit** → `paigasus-py-bindings-rs` + `paigasus-kernel-py`; one-directional w.r.t.
  the kernel.
- **node binding edit** → `paigasus-node-bindings-rs` + `paigasus-kernel-ts`; one-directional
  w.r.t. the kernel.
```

And replace the maintenance note body:

```markdown
The **must-include** sets are durable. The **must-exclude** (cross-stack-isolation) assertions
track current topology. Both the **py** and **ts** kernel-wrapper edges have now landed
(SMA-419/420). The `kernel->bindings` forbid-regex enumerates the *unrelated* ts/py packages a
kernel edit must not reach; each newly-added ts/py package must be hand-added to that enumeration
or it is silently unasserted. Consolidating this into a completeness/default-deny meta-check is a
tracked follow-up (SMA-420 review F4) — it would reverse the deliberate "positive-superset, not
strict equality" choice (SMA-409), so it gets its own decision.
```

- [ ] **Step 6: Verify the guard passes and can still fail**

Run:
```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
moon run repo:affected-smoke
# Expected: PASS for contracts->proto, kernel->bindings, binding-oneway, binding-oneway-node,
# ci-include-relations; ends "== affected-graph cascade intact ==".

ci/affected-graph/run.sh --negative-control
# Expected: "negative-control OK: harness reported red as expected" (exit 0).
```

- [ ] **Step 7: Commit**

```bash
git add ci/affected-graph/run.sh ci/affected-graph/README.md moon.yml
git commit -m "ci(repo): extend affected-graph guard for the kernel->ts cascade (SMA-420)

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Task 4: Full-gate verification + record the ADR note (AC #4)

**Files:** none in-repo (verification only; commit only if a gate reformats/regenerates a file).

- [ ] **Step 1: Run the full `moon ci` gate set locally**

Run (mirrors the CI task array; `moon run` builds the whole affected set without a base diff):
```bash
export PATH="$HOME/.proto/bin:$HOME/.proto/shims:$PATH"
moon run :build :test :lint :fmt :deny :machete :typecheck :affected-smoke
```
Expected: every task PASS. Watch `:machete`/`:deny` (the new crate's `napi`/`napi-derive` are machete-ignored; `paigasus-kernel`/`napi-build` are real uses) and `:test`/`:build` (the new ts tasks build the `.node`).

- [ ] **Step 2: Confirm the Rust gates are clean**

Run:
```bash
(cd rs && cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo nextest run --workspace --no-tests=pass)
```
Expected: all green — the new crate compiles, contributes no tests (`test = false`), and the kernel unit test still runs.

- [ ] **Step 3: Record the binding-mechanism choice on ADR-0005 (AC #4)**

ADR-0005 already names `paigasus-node-bindings` and `paigasus-wasm`, so no new ADR. In Notion, append a short note to **ADR-0005** ("ADR-0005: Shared behavioral logic in Rust …"): *napi-rs (`paigasus-node-bindings`) was stood up first (SMA-420); wasm-bindgen (`paigasus-wasm`) is the tracked follow-up for browser/Edge.* Link the SMA-420 spec. (Notion action — no repo file; if Notion is unavailable, hand off this one step to the maintainer.)

- [ ] **Step 4: Verify against acceptance criteria**

Confirm each AC with the evidence gathered:
1. **AC #1** — `paigasus-node-bindings` wraps `paigasus_kernel::sum`; `napi build` produces the `.node`; `@paigasus/kernel` re-exports it and the vitest import works; `cargo machete`/`cargo deny` green.
2. **AC #2** — `printf 'rs/crates/libs/paigasus-kernel/src/lib.rs' | moon query projects --affected --downstream deep` includes `paigasus-kernel-ts`; `moon run paigasus-kernel-ts:test` passes the round-trip.
3. **AC #3** — `moon run repo:affected-smoke` green (incl. `binding-oneway-node`); `--negative-control` reports red; the full gate set (Step 1) is green.
4. **AC #4** — the napi-first note is on ADR-0005.

- [ ] **Step 5: Commit any gate-produced changes (if any)**

```bash
# Only if Step 1 reformatted a file or regenerated a lockfile/glue:
git add -A && git commit -m "chore(repo): SMA-420 gate-produced fixups

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>"
```

---

## Self-Review notes (author)

- **Spec coverage:** §1 layout (crate + co-located pkg + `.cargo` comment + exports) → Tasks 1–2; §2 Moon edges → Task 1 (node-bindings) + Task 2 (kernel-ts override) + Task 3 (cascade verify); §3 conditional exports + smoke test → Task 2; §4 guard → Task 3; §5 double-compile → Task 1/Task 2 (test self-rebuild) + Task 4 (`:machete`); §6 spike checks 1–5 + the i64/number F3 → Task 1; §7 ADR note → Task 4; verification/ACs → Task 4.
- **Spike-gated items** are flagged inline (S-versions, S2 invocation/cwd + output filename + napi config key, S4 signature, S5 freshness, S6 vitest addon load) and resolved by Task 1's findings note before the dependent task runs — decisions to record, not placeholders.
- **Type consistency:** `sum(a: number, b: number): number` (Rust `i32`) is used identically in `src/lib.rs`, the generated `index.d.ts`, the `@paigasus/kernel` re-export, the vitest test, and the guard cases. Moon ids `paigasus-node-bindings-rs` / `paigasus-kernel-ts` and the package name `@paigasus/node-bindings` are used identically across crate `moon.yml`, the wrapper `package.json` `file:` dep, the `build`/`test` `pnpm --filter`, and the guard cases.
- **Decision recorded beyond the spec:** the generated `index.js`/`index.d.ts` are **committed** (repo commit-generated-code posture, Scoping §1) while `*.node` is gitignored — this lets `paigasus-kernel-ts:typecheck` resolve the binding's types without a prebuild. A napi codegen-drift guard (mirroring proto's `codegen-drift.yml`) is a future concern, not this issue.
```
