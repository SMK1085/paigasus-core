# SMA-420 — Task 1 spike findings (napi binding chain on macOS)

**Date:** 2026-06-16
**Host:** macOS (Darwin 25.5.0), `arm64` (Apple Silicon)
**Tooling:** Node v24.16.0, pnpm 11.3.0, cargo 1.95.0, moon 2.3.2, `@napi-rs/cli` 3.7.2
**Spec:** `docs/superpowers/specs/2026-06-15-sma-420-ts-kernel-napi-binding-design.md` (§1, §6, decisions)
**Mirrors:** SMA-419 (Python/PyO3) spike posture.

Load-bearing spike: it stands up the real `paigasus-node-bindings` crate **and** validates the
napi integration end-to-end against the §6 checks. All checks PASS. A few spike-contingent
behaviors differed from the task's primary assumption — recorded below so Task 2 can be adjusted.

## Summary

| Check | What | Result |
| --- | --- | --- |
| S-versions | napi/napi-build/napi-derive resolution | PASS — napi 3.9.2, napi-build 2.3.2, napi-derive 3.5.6 (no fallback) |
| S1 | macOS cdylib link via plain cargo (no undefined N-API symbols) | PASS |
| S2 | `@napi-rs/cli` provisioning + `napi build` artifacts | PASS (with a registry note + a `--platform` adjustment) |
| S3 | Import + call from Node | PASS — `sum(2,3) === 5` |
| S4 | `i64`→JS `number` (not BigInt) | PASS — `sum(a: number, b: number): number` |
| S5 | Cache-bust on a Rust edit | PASS — both kernel + binding recompile; shared `rs/target/` |

## S-versions — resolved napi versions (no fallback needed)

The primary version spec resolved cleanly. `moon run paigasus-node-bindings-rs:build` locked:

- `napi 3.9.2` — `default-features = false, features = ["napi8"]` accepted; the `napi8` feature is
  valid for the resolved major, so **neither fallback fired** (no need to drop `features` or pin a
  specific minor).
- `napi-build 2.3.2` — `napi-build = "2"` resolves cleanly against `napi 3` (the pairing the task
  flagged as spike-contingent is fine).
- `napi-derive 3.5.6`.

These are pinned in `rs/Cargo.lock` (committed). `@napi-rs/cli` resolved to **3.7.2** (the npm
`^3` devDep) — the v3 CLI line as the spec assumed.

**Decision:** keep the `rs/Cargo.toml` `[workspace.dependencies]` entries exactly as written.

## S1 — macOS cdylib link via plain cargo (gate) — PASS

`moon run paigasus-node-bindings-rs:build` compiled the crate as a `cdylib` with **no**
`Undefined symbols ... napi_*` linker error. The `rs/.cargo/config.toml`
`-undefined dynamic_lookup` apple-darwin flags (shared with PyO3, comment broadened this task) are
picked up because moon runs the task with cwd inside the crate (so cargo walks up into `rs/`).
`build.rs` (`napi_build::setup()`) ran as part of the compile. No S1 fallback needed.

**Decision:** the reused macOS link flags are sufficient for napi; no napi-specific link config.

## S2 — `@napi-rs/cli` provisioning + `napi build` artifacts — PASS (two adjustments)

### Registry note (affects how the standalone spike install behaved; informs Task 2)

A **standalone** `pnpm install` in the crate dir (outside `ts/`) failed with `ERR_PNPM_FETCH_401`
for `@napi-rs/cli`: the developer's global `~/.npmrc` points the default registry at a private AWS
CodeArtifact mirror (401 for public packages). The repo's `ts/.npmrc` pins
`registry=https://registry.npmjs.org/` **precisely to neutralize this**, but a standalone install in
the crate dir does **not** see `ts/.npmrc` (it's outside `ts/`). For the spike, overriding with
`pnpm install --registry=https://registry.npmjs.org/` resolved `@napi-rs/cli 3.7.2` cleanly.

**Implication for Task 2 (important):** the real install must run **under the `ts/` pnpm workspace**
(where `ts/.npmrc` pins public npm) — i.e. the `file:` link from `@paigasus/kernel` to this crate is
installed by the ts workspace's `pnpm install`, NOT by a standalone install in the crate dir. This
matches the spec's "co-located package reached via a `file:` specifier, not a pnpm workspace
member" design. Do **not** rely on a crate-dir `pnpm install`; it picks up the wrong (private)
registry. (CI runs `pnpm install` from `ts/` too, so CI is unaffected.)

### `napi build` invocation + artifact filenames (the `--platform` adjustment)

The task assumed `napi build` emits `index.js` + `*.node`. **@napi-rs/cli v3 does not, by default:**

- `napi build` (no flag) → `paigasus-node-bindings.node` (binaryName, **no platform suffix**) +
  `index.d.ts`, and **NO `index.js` loader**. `package.json` `main: "index.js"` would dangle.
- `napi build --platform` → `index.js` (the CJS loader) + `index.d.ts` +
  `paigasus-node-bindings.darwin-arm64.node` (**platform-suffixed**). This is the shape the task
  wants (`index.js` committed; the `.node` gitignored — `*.node` matches the suffixed name).

The generated `index.js` loader hard-codes the **platform-suffixed** name
(`require('./paigasus-node-bindings.darwin-arm64.node')`), so a plain `napi build` (unsuffixed
`.node`) would NOT be found by the committed loader. The `--platform` flag is therefore **required**
for self-consistency.

**Decisions applied:**
- `package.json` `build` script changed to `napi build --platform` (and `build:release` to
  `napi build --platform --release`). This is the exact invocation Task 2's
  `paigasus-kernel-ts` `build`/`test` tasks should run (via `pnpm exec napi build --platform`
  pointed at the co-located crate dir, or `pnpm --filter @paigasus/node-bindings run build`).
- **Committed generated artifacts:** `index.js` (the `--platform` CJS loader) + `index.d.ts`.
- **Gitignored:** `paigasus-node-bindings.darwin-arm64.node` (matches `*.node`; rebuilt per host).
- The `napi build` shares `rs/target/` with cargo (the same target dir moon uses) — see S5; cache
  reuse confirmed. The double-compile (spec §5) holds: `napi build` recompiles only changed crates.

Only `napi` needs a cargo-machete ignore (consumed purely via attribute macros). `napi-derive` is
visibly used (`use napi_derive::napi`), and `napi-build` (build.rs) + `paigasus-kernel` (direct call)
are real uses — so `[package.metadata.cargo-machete] ignored = ["napi"]` is the minimal correct set
(matching the py crate's single-entry `ignored = ["pyo3"]`). Verified: `cargo machete` passes with
only `napi` ignored; a `napi-derive` entry is redundant. (Corrected after the Task 1 quality review.)

## S3 — import + call — PASS

```
node --input-type=module -e "import b from './…/index.js'; console.log(b.sum(2,3))"  →  5
```

`typeof b.sum(2,3) === 'number'` and `b.sum(2,3) === 5` (strict) are both true. The generated
`index.js` is **CJS** (`module.exports = nativeBinding; module.exports.sum = …`); Node's ESM↔CJS
interop exposes it as the ESM default import, so `import b from './index.js'; b.sum` works. A named
`import { sum }` also works via CJS named-export interop (the loader assigns `module.exports.sum`).

**Decision for Task 2:** `@paigasus/kernel`'s `src/index.ts` can `export { sum } from
"@paigasus/node-bindings"` (the spec's shape) — named re-export resolves against the CJS loader.

## S4 — `i64`→JS `number` mapping — PASS (no BigInt)

The binding deliberately narrows to `i32` at the FFI boundary (`pub fn sum(a: i32, b: i32) -> i32`,
casting to/from the kernel's `i64`), per spec decision #5 / review F3. The generated `index.d.ts`:

```
export declare function sum(a: number, b: number): number
```

NOT `bigint`. `sum(2,3) === 5` (not `5n`). The "silent BigInt test failure" risk is avoided.

**Decision:** the `i32` surface is correct; the Task 2 vitest can assert `expect(sum(2,3)).toBe(5)`
directly (no `5n` accommodation needed).

## S5 — cache-bust on a Rust edit — PASS

Edited `rs/crates/libs/paigasus-kernel/src/lib.rs` (`a + b` → `a + b + 0`), re-ran
`napi build --platform`, then reverted. The rebuild log shows:

```
Compiling paigasus-kernel v0.0.0 (…/rs/crates/libs/paigasus-kernel)
Compiling paigasus-node-bindings v0.0.0 (…/rs/crates/bindings/paigasus-node-bindings)
```

Both recompiled (not all-cached); the rest of the dep tree was cache-reused (the `napi build`
shares `rs/target/`). Kernel restored to `a + b` afterward (git-clean; no stray `.bak`).

**Freshness mechanism (the SMA-419 S4 analog):** because `napi build` recompiles the changed Rust
crates against `rs/target/`, running it inside `paigasus-kernel-ts`'s `build`/`test` tasks (spec §2,
the F1 fix) guarantees the `.node` is fresh before the vitest import — a Rust-source edit re-runs the
napi compile rather than asserting against a stale `.node`. **Decision for Task 2:** the
`paigasus-kernel-ts` `test` task must run `napi build --platform` (fresh) before `vitest run`,
mirroring the py `test`'s `uv sync --reinstall-package … && pytest`.

## Open items handed to Task 2

- **`vitest.config.ts`:** likely **needed**. The smoke test imports `@paigasus/kernel`, which
  re-exports a **native** `.node` via the `file:`-linked binding. vitest's default transform/SSR
  pipeline can mis-handle native `.node` requires and the CJS loader; a minimal `vitest.config.ts`
  (e.g. `test.environment: 'node'`, and likely `server.deps.external` / `deps.optimizer` excluding
  the native binding, or `pool: 'forks'`) is the probable fix. Confirm during Task 2 — if the plain
  inherited `vitest run` resolves the native import, no config is needed; budget for adding one.
- **napi build invocation for Task 2:** `napi build --platform` (NOT bare `napi build`) — run via
  the ts pnpm workspace so the correct (public) registry + the `file:` link resolve.
- **Committed vs generated:** commit `index.js` + `index.d.ts`; `*.node` stays gitignored.
- **`exports`/`main`:** the committed `index.js` is CJS; `@paigasus/kernel`'s `node` condition can
  point at source re-exporting `@paigasus/node-bindings` (whose `main: "index.js"` resolves the CJS
  loader). Watch ESM/CJS interop in the wrapper if it ships as pure ESM.

## Freshness — cargo mtime caveat (affects BOTH FFI guards)

The freshness guard relies on `napi build` / maturin recompiling the changed Rust crate before the
assertion. But cargo's incrementality is **mtime-based**: after a warm `rs/target/` plus a git op
that leaves a source file's mtime OLDER than its existing artifact (checkout / rebase / stash →
mtime inversion), cargo reports "up to date" and does NOT recompile, so the FFI artifact is re-linked
STALE. The review reproduced `sum(2,3) → 6` against a kernel whose source is `a + b` — a silent
false red/green that defeats the guard's entire purpose.

- **ts (napi) — FIXED here:** `paigasus-kernel-ts`'s `build`/`test` tasks now prepend
  `cargo clean -p paigasus-kernel -p paigasus-node-bindings --target aarch64-apple-darwin
  --manifest-path ../../../rs/Cargo.toml` before `napi build`, forcing a content-correct recompile
  of just those two crates (cached deps untouched; a few seconds). `--target aarch64-apple-darwin`
  is required because `napi --platform` builds into the per-triple subdir, which a bare
  `cargo clean -p` (host dir only) does not touch — single-host, cross-platform matrix deferred
  (ADR-0006 / SMA-376/407). Verified: with the trap (target built from `a+b+1`, source reverted to
  `a+b` with a 2000-01-01 mtime), the unfixed task asserts a stale 6 (false red) while the fixed
  task recompiles and passes; a real `a+b → a+b+1` edit still fails correctly.
- **py (maturin) — LATENT, NOT fixed here:** `uv sync --reinstall-package paigasus-py-bindings`
  forces a *wheel* rebuild, but maturin's underlying cargo is still mtime-incremental, so the same
  inversion could re-link a stale wheel. A symmetric forced-rebuild for the py side **plus** a CI
  `rs/target` cache-invalidation policy is a tracked FOLLOW-UP, not SMA-420.

## Task 2 resolutions (recorded after implementing the wrapper + smoke test)

All open items above resolved as follows (host: macOS arm64, same toolchain):

- **`@napi-rs/cli` wiring (the devDep gotcha):** added `'@napi-rs/cli': ^3` to the `ts/`
  catalog and `"@napi-rs/cli": "catalog:"` as a devDep of `@paigasus/kernel`. pnpm does NOT
  install a `file:` dep's devDeps, so the CLI must be a devDep of the *consumer*. After
  `pnpm install` under `ts/`, `pnpm exec napi --version` → `3.7.2` from the kernel package.
- **napi build invocation (exact):**
  `pnpm exec napi build --platform --cwd ../../../rs/crates/bindings/paigasus-node-bindings`,
  run from the `paigasus-kernel-ts` task cwd (`ts/packages/paigasus-kernel`). The `--cwd` flag
  makes the CLI resolve from the ts workspace while cargo runs inside `rs/` (picks up the
  apple-darwin link flags, S1). The literal plan command `pnpm --filter @paigasus/node-bindings
  build` does NOT work (a `file:` package is not a workspace member; `--filter` can't target it).
  Moon's `command:` rejects `&&`, so both `build` and `test` use `script:` (mirrors the py task).
- **Generated-glue churn:** `napi build --platform` regenerates `index.js`/`index.d.ts`
  byte-identically (the surface is unchanged) — `git status` on the crate dir is clean, no
  spurious diff committed. The `.node` stays gitignored.
- **`vitest.config.ts`: YES, needed — but for a DIFFERENT reason than predicted.** The plain
  inherited `vitest run` *passed* on first run. The real problem only surfaces on a Rust edit:
  **pnpm COPIES a `file:` dep into its store at install time and never re-copies a rebuilt
  binary.** So `napi build` rewrites the crate-dir `.node`, but importing via the package name
  loads a **STALE store `.node`** — a kernel edit (`a+b` → `a+b+1`) still passed against the old
  value (silent false-green). Fix: `vitest.config.ts` aliases `@paigasus/node-bindings` to the
  **crate-dir** `index.js` (absolute, via `fileURLToPath(new URL(...))`), so vitest loads the
  fresh `.node` that `napi build` rewrites. With the alias, the broken-kernel test correctly
  FAILS (`Expected 5, Received 6`) and the real kernel passes. (`test.server.deps.external:
  [/\.node$/]` is also set to keep the addon out of vitest's transform; `deps` lives under
  `test`, not top-level `server`, in vitest 4 — top-level fails typecheck.) This pnpm
  store-copy gotcha is the TS analog of SMA-419's `uv sync --reinstall-package` wheel-freshness
  fix; **Task 3's affected-graph guard / any future CI must NOT assume importing by package name
  sees a rebuilt `.node`.**
- **tsconfig change (not in the literal file list, but required for the gates):** the kernel
  `tsconfig.json` gained `customConditions: ["node"]` (so tsc/eslint walk `src/index.ts`, not the
  `default` `unsupported.ts`) and `include: ["src/**/*", "tests/**/*", "vitest.config.ts"]` (so
  eslint's typed projectService covers the test + config; `rootDir` dropped — it rejects files
  outside `src/`, inert under `noEmit`). Without this, `ts:lint` errors (`projectService` parse
  error on the test file + `no-unsafe-call` on `sum`) and `tsc` resolves the wrong export branch.
- **prettier:** the repo config is `singleQuote: true, printWidth: 200`; the new `.ts` files use
  single quotes (the spec's double-quote literals were reformatted to satisfy `ts:fmt`).

## Files created / modified by this spike

- `rs/Cargo.toml` — added `napi`/`napi-derive`/`napi-build` to `[workspace.dependencies]`.
- `rs/Cargo.lock` — new napi deps locked (committed; workspace tracks it).
- `rs/.cargo/config.toml` — broadened the macOS link-flag comment to name napi alongside PyO3.
- `rs/crates/bindings/paigasus-node-bindings/{Cargo.toml,build.rs,src/lib.rs,package.json,.gitignore,moon.yml}`
  — the new napi binding crate (mirrors paigasus-py-bindings); `build` script uses `--platform`.
- `rs/crates/bindings/paigasus-node-bindings/{index.js,index.d.ts}` — generated, committed.
- (gitignored) `rs/crates/bindings/paigasus-node-bindings/paigasus-node-bindings.darwin-arm64.node`.

Spike scaffolding (`node_modules/`, `pnpm-lock.yaml` from the standalone install) was removed and is
NOT committed — the real install happens via the ts pnpm workspace in Task 2.
