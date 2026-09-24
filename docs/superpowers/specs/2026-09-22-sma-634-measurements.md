# SMA-634 — Measurements

This file records the measurements that the SMA-634 design
(`docs/superpowers/specs/2026-09-22-sma-634-kernel-napi-consumers-design.md`) is built on. Three
spikes made them on 2026-09-22, in the worktree
`.claude/worktrees/sma-634-kernel-napi` at `8c1297b7`. Each section states the command, the
result and the date. A spike keeps no code: every spike restored the worktree and committed
nothing.

**Tool versions for every section:** Next 16.3.5 (Turbopack is the default), pnpm 11.3.0, node
24.16.0, vitest 5.0.1, @napi-rs/cli 3.10.3, wasm-pack 0.15.0 (the `.prototools` pin), rustc and
cargo 1.95.0, wasm-bindgen 0.2.127 (`rs/Cargo.lock`). Measure again after a bump of any of them.

---

## S1 — Can Next trace the napi binary through a pnpm `link:`? (spike 1)

**Date:** 2026-09-22. **Question:** revision 1 of the design proposed `link:` for both binding
packages, so that a build after the install would still reach the `.node` file. The adversarial
challenge said that the tracing of a file outside `outputFileTracingRoot` was not measured.

**Method.** A throwaway Next app in `ts/apps/` with one route handler that calls the kernel. For
each variant: `pnpm exec next build`, then `find .next/standalone` for the binary and for
symlinks, then a copy of the standalone tree to a directory outside the repository, then
`node <copy>/…/server.js` and a `curl` of the route.

| Variant | Build | Regular `.node` in the tree | Copy outside the repository |
| -- | -- | -- | -- |
| V0: `file:` (the state today) | exit 1 | – | – |
| V1: `link:`, tracing root `ts/` | exit 1 | – | – |
| V2: V1 + `serverExternalPackages` | exit 1 | – | – |
| V2b: V2 + a direct `link:` dependency of the app | exit 1 | – | – |
| V3a–c: `link:`, tracing root at the repository root | exit 1 | – | – |
| V4: webpack (`--webpack`) | exit 1 | – | – |
| V3d-ts: a runtime `createRequire` + `outputFileTracingIncludes`, root `ts/` | exit 0 | no | HTTP 500 |
| **V3d-repo: the same at the repository root** | **exit 0** | **yes** | **HTTP 200** |
| V3d-noinc: V3d-repo without the include (control) | exit 0 | no | HTTP 500 |
| W-ts: `@paigasus/wasm` through `link:`, root `ts/` | exit 1 | – | – |
| **W-repo: the same at the repository root** | **exit 0** | n/a (bundled) | **HTTP 200** |

**Errors, verbatim.**

- V0: `Error: Cannot find native binding.` … `[cause]: Error: Cannot find module as expression is
  too dynamic`. The `.node` file was in the crate directory **before** the install. The `file:`
  install still did not copy it: the installed copy held only `index.d.ts`, `index.js` and
  `package.json`. This reproduces the defect that SMA-634 reports.
- V1, V2, V2b, W-ts: `Error: Turbopack build failed with 1 error: ./packages/paigasus-kernel/src/
  index.ts:2:1 Error: Module not found: Can't resolve '@paigasus/node-bindings'` (W-ts names
  `'@paigasus/wasm'`). Turbopack does not resolve a symlink whose target is outside its root.
- V3a–c: `Error: Turbopack build failed with 1 error: ./rs/crates/bindings/paigasus-node-bindings/
  index.js Error: non-ecmascript placeable asset asset is not placeable in ESM chunks, so it
  doesn't have a module id`, after nine `Module not found` warnings for the other platform
  candidates.
- V4: `Module parse failed: Unexpected character '<0x7f>' (1:0)` on the `.node` file.

**Other results.** `serverExternalPackages` had no effect in any variant and printed no message. A
tracing root at the repository root moves the server entry point from
`standalone/apps/<app>/server.js` to `standalone/ts/apps/<app>/server.js`. No variant that built
left a symlink that points out of the standalone tree.

**Bonus.** `napi build` replaces the `.node` file by rename, not in place: the inode changed on
each rebuild (124060757 → 124077310 → 124077415), and @napi-rs/cli 3.10.3 `dist/cli.js` calls
`copyFile(source, temporaryPath, COPYFILE_EXCL)` and then `rename`.

**Conclusion.** The napi route needs three changes together: the repository root as the tracing
root, a load that the bundler cannot see, and an explicit include. The wasm route works with a
plain import, but at the repository root only. Revision 1 of the design is disproved.

---

## S2 — Does the W1 shape build, test and run? (spike 2)

**Date:** 2026-09-22. **Question:** W1 keeps `file:`, commits `paigasus_wasm_bg.wasm` so that it
is present at install time, and gives the kernel a wasm entry for Node consumers.

### S2-A — Next

| Test | Result |
| -- | -- |
| Build the wasm as `paigasus-kernel-ts:build` does, then `pnpm install` | PASS. The installed copy holds all five files. **pnpm hard-links them to the crate directory** (the same inode, a link count of 2). It does not copy them |
| `next build` of a fixture route that imports the kernel wasm entry, with `createNextConfig()` and the tracing root `ts/` | PASS, exit 0, the route is dynamic (`ƒ /api/k`) |
| The standalone tree | One `.wasm` chunk at `apps/<app>/.next/server/chunks/…paigasus_wasm_bg….wasm`, 48227 bytes, with the sha256 of the source. 29 symlinks, none of which leaves the tree. `@paigasus/wasm` is bundled, so the tree holds no `node_modules/@paigasus` |
| The copy outside the repository, `GET <base>/api/k` | HTTP 200. A valid input gives the built PRN and its org field. `not-a-prn` gives `"kind":"wrong-field-count"` |
| A bare config: only `basePath`, `output: 'standalone'` and `outputFileTracingRoot` | Also PASS. No `transpilePackages`, no `serverExternalPackages` and no Turbopack wasm rule are necessary |

**Red control.** Without the `.wasm` file in a fresh `node_modules`, the build fails:

```
Error: Turbopack build failed with 1 error:
./node_modules/.pnpm/@paigasus+wasm@file+..+rs+crates+bindings+paigasus-wasm/node_modules/@paigasus/wasm/paigasus_wasm.js:2:1
Error: Module not found: Can't resolve './paigasus_wasm_bg.wasm'
```

This is the error that SMA-511 § 13 row 10 (B2) recorded. Its cause is the absent file, not the
route.

### S2-B — vitest in a console configuration

| Test | Result |
| -- | -- |
| A test in `@paigasus/console-core` that imports the kernel wasm entry, with the package's present vitest config (environment `node`, conditions `['node', 'import', 'default']`, the `server-only` alias) | PASS. vitest externalizes `@paigasus/wasm`, and node 24 loads the `.wasm` natively |
| The same test with `vite-plugin-wasm` added | PASS, and the plugin is not used on this path |

Both runs print `ExperimentalWarning: Importing WebAssembly module instances is an experimental
feature and might change at any time` on stderr. The only necessary change is the dependency on
the kernel package.

### S2-C — the pnpm refresh trap (R1 to R4)

The red control above was reachable only after a fresh `node_modules`. The four steps:

| Step | Action | Result |
| -- | -- | -- |
| R1 | Delete the `.wasm` file in the crate directory, then `pnpm install` | "Already up to date". The installed copy stays, and the build **passes** |
| R2 | Delete the installed `@paigasus+wasm…` directory, then `pnpm install` | "Already up to date". pnpm does not make the directory again. The build fails with `Can't resolve '@paigasus/wasm'` |
| R3 | `pnpm install --force` | The same as R2 |
| R4 | `rm -rf ts/node_modules`, then `pnpm install` | A new copy, without the `.wasm` file. The build gives the B2 error |

Because of the hard link, an in-place overwrite of a crate file (a `cp` onto it) changes the
installed copy at once. An unlink and replace (a delete, `git checkout`, a branch switch) breaks
the link, and only a new `node_modules` repairs it. CI always installs into a new `node_modules`,
so CI is not affected.

---

## S3 — Are the wasm outputs the same on every host? (spikes 2 and 3)

**Date:** 2026-09-22. **Command**, run from `rs/crates/bindings/paigasus-wasm` on each host, which
is the command that `paigasus-kernel-ts:test` runs today:

```
wasm-pack build . --target bundler --release --no-pack --out-dir <out> --out-name paigasus_wasm -- --locked
```

**Hosts.** macOS arm64 (the development Mac); Linux arm64 and Linux amd64 in Docker, from
`rust:1.95.0-bookworm` at the digest that `rs/Dockerfile` pins, with wasm-pack 0.15.0. The amd64
run is emulated.

### S3-1 — The glue is host-independent; the binary is not

| File | macOS arm64 | Linux arm64 | Linux amd64 |
| -- | -- | -- | -- |
| `paigasus_wasm.js` (399 B) | `b3d6da34…` | the same | the same |
| `paigasus_wasm_bg.js` (12454 B) | `ca63dc9f…` | the same | the same |
| `paigasus_wasm.d.ts` (2114 B) | `75821ee4…` | the same | the same |
| `paigasus_wasm_bg.wasm.d.ts` (1561 B) | `94c0af8a…` | the same | the same |
| `paigasus_wasm_bg.wasm` | `ec78a0cb…` (48227 B) | `44255560…` (48163 B) | `9bb39930…` (48163 B) |

**The four glue files are byte-identical on all three hosts. Only the binary differs, and it
differs on every pair of hosts.** The difference is in the `code`, `export`, `data`, `name` and
`producers` sections. The `data` section has a different length on macOS (2115 B) and on Linux
(2039 B), and the two Linux builds agree there. The shifted `i32.const` immediates and the small
changes of function index agree with an absolute build path inside the panic-location data.

Three builds on one host (two in `rs/target`, one in a clean `CARGO_TARGET_DIR`) gave the same
sha256. So the build is deterministic on one host.

`--remap-path-prefix` for the cargo home, the sysroot and the worktree did **not** make the three
hosts equal. Without a remap, the macOS binary holds absolute home paths
(`/Users/<user>/.cargo/registry/…`, `/Users/<user>/.rustup/toolchains/…`).

`wasm-opt = false` is set in `[package.metadata.wasm-pack.profile.release]`, so no host downloads
binaryen. The wasm-bindgen CLI is a `cargo install` build on macOS (`producers: walrus 0.26.5`)
and a downloaded binary on Linux (`producers: walrus 0.26.4`).

### S3-2 — A version-only bump changes the binary only

`version` in `rs/crates/bindings/paigasus-wasm/Cargo.toml` from `0.1.0` to `0.1.1`, then
`cargo update -p paigasus-wasm --workspace` (which rewrites only that entry of `rs/Cargo.lock`),
then a rebuild on macOS:

| File | Changed? |
| -- | -- |
| `paigasus_wasm.js`, `paigasus_wasm_bg.js`, `paigasus_wasm.d.ts`, `paigasus_wasm_bg.wasm.d.ts` | no |
| `paigasus_wasm_bg.wasm` | **yes** (`ec78a0cb…` → `99fa5d47…`) |

The binary holds the crate version. The glue does not.

### S3-3 — The import and export lists are host-independent

`WebAssembly.Module.imports()` and `WebAssembly.Module.exports()` in node 24.16.0, sorted and
hashed, for the macOS build, the two Linux builds and the version-bumped macOS build: **all four
give the same hash.** Two imports and 21 exports, with the same names and kinds.

- Imports: `./paigasus_wasm_bg.js.__wbg_Error_<hash>` and
  `./paigasus_wasm_bg.js.__wbindgen_init_externref_table`, both functions.
- Exports: `memory`, `__wbindgen_externrefs`, `__abort_handler`, `__instance_terminated`,
  `__externref_table_dealloc`, `__wbindgen_free`, `__wbindgen_malloc`, `__wbindgen_realloc`,
  `__wbindgen_start`, and the twelve kernel functions `sum`, `mintUuid7`, `prnBuild`,
  `prnCanonicalize`, `prnCedarEntityId`, `prnCedarEntityType`, `prnErrorKind`, `prnOrg`,
  `prnRegion`, `prnResourceId`, `prnResourceType` and `prnService`.

### S3-4 — The committed glue is stale today

`git show HEAD:<path>` against a fresh build on this host:

| File | Result |
| -- | -- |
| `paigasus_wasm.js`, `paigasus_wasm.d.ts`, `paigasus_wasm_bg.wasm.d.ts` | identical |
| `paigasus_wasm_bg.js` | **differs in exactly one line** |

```diff
-export function __wbg_Error_92b29b0548f8b746(arg0, arg1) {
+export function __wbg_Error_408e67f47ca7b58b(arg0, arg1) {
     const ret = Error(getStringFromWasm0(arg0, arg1));
     return ret;
 }
```

The committed symbol comes from commit `0b2e346a` (SMA-448). The committed glue with a current
binary fails in node:

```
LinkError: WebAssembly.Instance(): Import #0 "./paigasus_wasm_bg.js" "__wbg_Error_408e67f47ca7b58b": function import requires a callable
```

`paigasus-kernel-ts:build` hides this defect, because it overwrites the committed glue on every
run.

---

## What these measurements decide

- S1 removes the napi route through `link:` (design revision 1).
- S2-A and S2-B show that the W1 shape needs no Next change and no vitest change.
- S2-C gives the local refresh rule for a developer.
- S3-1 and S3-3 make a host-independent drift gate possible: the glue, the import list and the
  export list can be compared, and the binary bytes cannot.
- S3-2 shows that a release version bump changes only the binary, so no check of the gate reads a
  value that a release PR changes.
- S3-4 is a defect that this design repairs.
