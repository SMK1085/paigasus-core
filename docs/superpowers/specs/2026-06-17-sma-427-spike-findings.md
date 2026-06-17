# SMA-427 — wasm kernel binding spike findings (Phase 0 go/no-go gate)

**Date:** 2026-06-17
**Branch:** `feature/sma-427-stand-up-the-wasm-kernel-binding-paigasus-wasm-for`
**Plan:** `docs/superpowers/plans/2026-06-17-sma-427-wasm-kernel-binding.md` (Phase 0)
**Spec:** `docs/superpowers/specs/2026-06-17-sma-427-wasm-kernel-binding-design.md` (§7 spike gate)
**Host:** macOS (Darwin 25.5.0), arm64 (`aarch64-apple-darwin`). Linux/CI checkpoints (§7.2 clean-CI
wasm32 install) are NOT exercised here — they are explicit Phase 3 CI verifications.

## Summary / gate recommendation

**GO.** The build chain validates end-to-end on macOS arm64: host build + clippy `-D warnings` +
machete all pass with no fallback; wasm-pack `--target bundler` emits the expected glue; a
`vite-plugin-wasm` + `vite-plugin-top-level-await` vitest instantiates the bundler glue and the
round-trip passes. **No structural branch is forced:**

- **§7.1 host gate:** PASS — the host-gate `--exclude` fallback is NOT needed. Phase 2 `moon.yml`
  uses the plain `deps: ['^:build']` form (no inherited-task override).
- **§7.6 Next.js:** MIXED / risk noted — **`init()` fallback remains open** (orchestrator decides).
  Turbopack build/compile/prerender succeeds; webpack `asyncWebAssembly` build fails at prerender.
  Neither is a browser-runtime hydration proof. Non-blocking for the gate (no live consumer).

**Two new Phase 2 gotchas surfaced** (not structural, but they change Phase 2 wiring — see §7.3):
`wasm-pack --out-dir .` **cleans the out-dir each run**, so it (a) overwrites `.gitignore` with a bare
`*` and (b) **deletes the hand-written `package.json`**. Phase 2's moon build/test must re-assert both
files after every `wasm-pack build`.

---

## §7.1 — Host workspace gates vs the wasm cdylib (M1 binary go/no-go)

**Commands (from `rs/`):**
```
cargo build -p paigasus-wasm
cargo clippy -p paigasus-wasm --all-targets -- -D warnings
cargo machete .
```

**Observed:**
- `cargo build -p paigasus-wasm` → `Finished` (exit 0). Resolved `wasm-bindgen v0.2.125`; the host
  cdylib linked with no `rs/.cargo/config.toml` change (the wasm crate needs none; the macOS
  `-undefined dynamic_lookup` flags were not exercised because the link succeeded outright on this
  placeholder).
- `cargo clippy -p paigasus-wasm --all-targets -- -D warnings` → `Finished` (exit 0), no warnings.
- `cargo machete .` → `cargo-machete didn't find any unused dependencies in this directory. Good job!`
  (exit 0).

**GO/NO-GO: PASS — no fallback.** The wasm-bindgen cdylib builds and lints cleanly on the host target.
The pre-designed `--exclude paigasus-wasm` host-gate exclusion is **not** required. Phase 2 Task 2.1
`moon.yml` should keep the bare `deps: ['^:build']` form (do NOT add the inherited-task override).

> Caveat: only macOS arm64 was tested. The spec called for Linux too; the clean-CI wasm32 build is the
> Phase 3 CI verification. Host build/lint passing on macOS is a strong signal but the Linux host gate
> is confirmed only when CI runs.

## §7.2 — wasm32 on a clean CI runner (H1)

**Commands:**
```
proto install                                  # installs wasm-pack 0.15.0 via the vendored plugin
rustup target list --installed | grep wasm32-unknown-unknown   # -> NOT INSTALLED initially
rustup target add wasm32-unknown-unknown       # installed locally
moon toolchain info rust                        # confirms the `targets` field exists
```

**Observed:**
- `wasm-pack --version` → `wasm-pack 0.15.0`. proto downloaded
  `wasm-pack-v0.15.0-aarch64-apple-darwin.tar.gz` and unpacked the nested binary — confirms the
  vendored plugin's asset shape (`exe-path` one dir deep, no checksum file) is correct.
- wasm32 was **absent** on the dev host until `rustup target add` (expected per plan Step 5).
- `moon toolchain info rust` shows: `targets: [string]` — "List of Rust targets to automatically
  install with `rustup`." This confirms `.moon/toolchains.yml rust.targets` is the right CI mechanism.

**GO/NO-GO: provisioning mechanism confirmed; clean-CI install is a Phase 3 CI checkpoint** (cannot be
proven on a dev host whose rustup already has the target). The `.moon/toolchains.yml rust.targets:
['wasm32-unknown-unknown']` entry is in place; Phase 3 verifies the actual CI run installs it.

## §7.3 — wasm-pack invocation + release-asset shape

**Command (the working form):**
```
wasm-pack build rs/crates/bindings/paigasus-wasm --target bundler --release --no-pack \
  --out-dir . --out-name paigasus_wasm
```
Run **from the repo root**. wasm-pack reported: `📦 Your wasm pkg is ready to publish at
rs/crates/bindings/paigasus-wasm.` — i.e. **`--out-dir .` resolves relative to the CRATE dir, not the
repo root.** No artifacts landed at the repo root. So the plan's invocation works as written from repo
root; the crate-relative `--out-dir .` is correct. (Phase 2 runs it from `ts/packages/paigasus-kernel`
with `--out-dir .` against the crate path `../../../rs/crates/bindings/paigasus-wasm`, which is the
same crate-relative resolution.)

**Exact emitted glue filenames (these pin `package.json` `files`/`module` and `.gitignore`):**
| file | committed? | size (bytes) |
|------|-----------|--------------|
| `paigasus_wasm.js`            | yes | 246    |
| `paigasus_wasm_bg.js`         | yes | 850    |
| `paigasus_wasm.d.ts`          | yes | 435    |
| `paigasus_wasm_bg.wasm.d.ts`  | yes | 233    |
| `paigasus_wasm_bg.wasm`       | **no — gitignored** | 18407 |

These match the plan's `package.json` `files`/`module`/`types` exactly — **no adjustment needed**.

`paigasus_wasm.js` does the bundler-style import the spec described:
```js
import * as wasm from "./paigasus_wasm_bg.wasm";
import { __wbg_set_wasm } from "./paigasus_wasm_bg.js";
__wbg_set_wasm(wasm);
wasm.__wbindgen_start();
export { sum } from "./paigasus_wasm_bg.js";
```
`paigasus_wasm.d.ts` exposes `export function sum(a: number, b: number): number;` — type-identical to
the napi `sum` (so the M5 parity guard in Phase 1 will pass).

**wasm-bindgen-cli auto-fetch (decision #2 invariant) confirmed:** wasm-pack built and installed
`wasm-bindgen-cli v0.2.125` into its own cache, matching the `wasm-bindgen 0.2.125` Cargo.lock
resolved. The pinned wasm-pack 0.15.0 drove 0.2.125 with no schema mismatch — the proto-pin invariant
holds at the current resolution.

### ⚠️ NEW Phase 2 gotcha — wasm-pack cleans the out-dir (changes Phase 2 wiring)

`wasm-pack build --out-dir .` **wipes/cleans the out-dir before emitting**, even with `--no-pack`.
Confirmed by re-running the build and re-inspecting the crate dir:
- It **overwrites `.gitignore`** with a bare `*` (would gitignore the committed glue + package.json).
- It **deletes the hand-written `package.json`** (the `@paigasus/wasm` `file:`-link surface).

This was reproduced twice. **Phase 2 impact:** the `moon` `build`/`test` tasks (Task 2.2) rerun
`wasm-pack build` on every invocation, so they will destroy the committed `.gitignore` and
`package.json` each run. The Phase 2 task script MUST re-assert both files after `wasm-pack build`
(e.g. `&& cp` from a kept template, or `git checkout -- .gitignore package.json`, or write them
inline), OR build to a scratch `--out-dir` and copy only the glue back. The committed `.gitignore`
carries a NOTE documenting this. This is a wiring change, NOT a structural/gate decision.

## §7.4 — vitest + vite-plugin-wasm instantiation (browser-condition round-trip)

**Setup (throwaway, in `$CLAUDE_JOB_DIR/tmp/wasm-vitest-scratch`, never touched the repo):** copied the
emitted glue (incl. the `.wasm`), installed `vitest@4.1.9` + `vite-plugin-wasm@3.6.0` +
`vite-plugin-top-level-await@1.6.0` (the catalog-target versions), aliased `@paigasus/wasm` → the glue,
prepended `browser` to the default Vite conditions, and asserted `sum(2,3) === 5` / `sum(-4,4) === 0`.

**Working plugin config (record for Phase 2 Task 2.3):**
```ts
import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vitest/config';
import wasm from 'vite-plugin-wasm';
import topLevelAwait from 'vite-plugin-top-level-await';

const wasmGlue = fileURLToPath(new URL('./glue/paigasus_wasm.js', import.meta.url));

export default defineConfig({
  plugins: [wasm(), topLevelAwait()],
  resolve: {
    conditions: ['browser', 'module', 'import', 'node', 'default'],  // additive — do NOT bare ['browser']
    alias: { '@paigasus/wasm': wasmGlue },
  },
  test: { name: 'browser', environment: 'node', include: ['tests/sum.wasm.test.ts'] },
});
```

**Observed:** `Test Files 1 passed (1) / Tests 1 passed (1)`. The bundler-target glue instantiated under
`vite-plugin-wasm` + `vite-plugin-top-level-await` in a Node `browser`-condition run; both assertions
passed.

**GO/NO-GO: PASS.** The bundler glue is instantiable under the vite wasm plugins — AC #1's proof
mechanism works. The `resolve.conditions` order above (additive `browser` prepended to the defaults) is
the spike-confirmed form for Phase 2.

> Vitest 4 note: the scratch used the simple single-project `defineConfig` shape (plugins/resolve at
> the top level). Phase 2 nests this under `test.projects: [...]` (two projects: node + browser). The
> nesting shape was NOT exercised in the scratch — verify the `test.projects` API against vitest 4.1.9
> in Phase 2 Task 2.3 (the plan already flags this); the per-project plugins/resolve/conditions content
> is what is confirmed here.

> Peer-dep note for Phase 1 catalog wiring: `vite-plugin-top-level-await@1.6.0` hard-`require`s
> `esbuild` and (transitively) `rollup`. In the strict-pnpm scratch these were not auto-provided and had
> to be added explicitly; with `node-linker=hoisted` it worked. The real `ts/` workspace already lists
> `esbuild: ^0.27.0 || ^0.28.0` and uses `vite@8.0.16` / `vitest@4.1.9` (same as the scratch), so
> esbuild resolves through vitest's own graph there. **Phase 1 action:** after `pnpm --dir ts install`,
> confirm `vite-plugin-top-level-await` finds esbuild; if not, add `esbuild` to the catalog/devDeps.

> Vite version note: the scratch resolved `vite@8.0.16`. The plugins are authored against older vite
> majors but worked against vite 8 with no shim.

## §7.5 — Freshness / cache-bust on a Rust edit

**N/A for Phase 0** — the `touch`-mtime cache-bust + crate-dir-alias staleness check is wired and proven
in Phase 2 Task 2.3 Step 4 (it depends on the `moon.yml` build/test tasks that Phase 2 creates). Not
trivially observable in Phase 0. Deferred to Phase 2 as the plan specifies.

## §7.6 — Next.js client-component import (H2 — gates the "no init()" decision)

**Setup (throwaway, `$CLAUDE_JOB_DIR/tmp/nextjs-probe`, ≤30 min time-box):** minimal Next.js 16.2.9 +
React 19 app; a `'use client'` component (`app/calc.tsx`) does `import { sum } from '@paigasus/wasm'`
and renders `sum(2, 3)` synchronously; `@paigasus/wasm` linked as `file:./glue` from the emitted glue.

**Observed:**
- **Turbopack** (`next build --turbopack`, Next 16 default, empty `turbopack: {}` config): **build
  succeeded** — `✓ Compiled successfully in ~0.5s`, TypeScript passed, and route `/` was
  `○ (Static) prerendered as static content`. The import graph + bundler glue are accepted by
  Turbopack with no special wasm config.
- **Webpack** (`next build --webpack` with `experiments.asyncWebAssembly + topLevelAwait`): **build
  FAILED** at the prerender step — `Error: ENOENT ... .next/server/static/wasm/<hash>.wasm` during
  static export of `/`. (The wasm compiled, but the prerender worker couldn't open the emitted `.wasm`
  for this minimal app.)
- The Turbopack prerendered HTML contains the literal `sum(2,3)=` prefix but the **value is empty** in
  the static output — expected, because a `'use client'` component's wasm call executes on browser
  hydration, not during static prerender. So the build proves the export resolves + Turbopack handles
  the bundler glue, but it is **NOT** a browser-runtime proof that the synchronous `sum(2,3)` value
  materializes (a headless-browser hydration check, not run in the time-box, would be needed).

**GO/NO-GO: NON-BLOCKING — risk noted, `init()` fallback REMAINS OPEN.** This matches the spec H2
nuance: Turbopack build/compile/prerender is a positive signal, but it is not the synchronous
call-site runtime proof, and the webpack path failed at prerender for the minimal app. Per the plan,
this records as "risk noted, init() fallback remains open" and does not block the gate (no live
consumer imports `@paigasus/kernel` yet — verified in SMA-420). **Orchestrator decision at the gate:**
proceed with the bundler-sync surface (decision #1) for now; the pre-agreed `await init()` (or async
accessor) browser-surface fallback stays available if a future Next.js consumer can't make the sync
call site work. No Phase 1–3 change is forced by this today, but Phase 1's `src/wasm.ts` and the
exports map are the place the `init()` fallback would land if the orchestrator chooses it.

## §7.7 — wasm-opt off + binaryen

**Observed:** with `[package.metadata.wasm-pack.profile.release] wasm-opt = false`, the build did NOT
download binaryen (no `binaryen` entry in `~/Library/Caches/.wasm-pack/`), and the unoptimized
`paigasus_wasm_bg.wasm` (~18 KB) instantiated fine in the §7.4 vitest round-trip.

**GO/NO-GO: PASS.** `wasm-opt = false` keeps the build network-free (no unpinned binaryen) and the
placeholder `.wasm` works. (Re-enabling wasm-opt later reintroduces the unpinned-binary download — the
§8 binaryen-pin follow-up.)

## §7.8 — machete ignore necessity (10-second check)

**Command:** removed the `[package.metadata.cargo-machete] ignored = ["wasm-bindgen"]` block, reran
`cargo machete .`, then restored.

**Observed:** with the explicit `use wasm_bindgen::prelude::wasm_bindgen;` in `src/lib.rs`,
`cargo machete` did **NOT** flag `wasm-bindgen` even WITHOUT the ignore (`didn't find any unused
dependencies`). So the ignore is **not strictly necessary** at the current source shape.

**Decision: KEEP the ignore (as the plan/spec specify).** Rationale (matches the spec): pyo3 keeps its
ignore despite a glob `use`; the ignore is cheap defense against future refactors that drop the
explicit `use` or switch to a glob import (which is exactly when machete would start flagging the
macro-only crate). The Cargo.toml comment documents that wasm-bindgen is macro-consumed. Restored
byte-identical (verified via `diff`).

---

## Items requiring Phase 1–3 amendments

| § | Outcome | Phase impact |
|---|---------|-------------|
| 7.1 | Host gate PASS | Task 2.1 `moon.yml`: keep `deps: ['^:build']`, **no** host-gate exclusion. |
| 7.3 | wasm-pack cleans out-dir (deletes `package.json`, overwrites `.gitignore`→`*`) | **Task 2.2:** moon build/test must re-assert `package.json` + `.gitignore` after every `wasm-pack build`. (New, not in the plan as written.) |
| 7.4 | vite plugins work; config recorded | Task 2.3: use the recorded additive-conditions config; verify the vitest-4 `test.projects` nesting; confirm esbuild resolves (Phase 1). |
| 7.6 | Turbopack OK, webpack prerender fails; no browser-runtime proof | `init()` fallback REMAINS OPEN — orchestrator decides. Lands in Phase 1 `src/wasm.ts`/exports if chosen. |
| 7.8 | machete ignore not strictly needed but kept | none (kept as specified). |

## Glue filenames (canonical, for downstream wiring)
- committed: `paigasus_wasm.js`, `paigasus_wasm_bg.js`, `paigasus_wasm.d.ts`, `paigasus_wasm_bg.wasm.d.ts`
- gitignored: `paigasus_wasm_bg.wasm`

## Commits (this phase)
- `a3fb9fd` — ci(repo): pin wasm-pack proto plugin + wasm32 target (SMA-427)
- `c16de78` — feat(rs): add paigasus-wasm wasm-bindgen kernel binding (SMA-427)
- (this doc) — docs(repo): SMA-427 spike findings + go/no-go
