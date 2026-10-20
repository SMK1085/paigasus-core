# SMA-432: a verified workerd path for `@paigasus/kernel`

- Linear: SMA-432 (project "Paigasus Polyglot", milestone "Frontend", priority Low, labels
  `area:ffi`, Feature)
- Related: SMA-427 (the wasm binding; its spec §2 H3 and §8 name this follow-up), SMA-634 and
  ADR-0022 (`.` is the wasm entry under every condition; the five committed wasm artifacts),
  SMA-579 (the wasm release staging), ADR-0005 (kernel once; browser and Edge through wasm-bindgen)
- Status: **PARKED on 2026-10-04, not approved.** No consumer needs the workerd path now. The
  spec stopped at the spec-approval gate. The open questions in §11 have no answers. Revised after
  the spec challenge (see §12) and re-checked against `origin/main` `4b2817a9` on 2026-10-04 (see
  §12, "Freshness re-check").

## 0. When to revisit

Revisit this issue when one of these events occurs:

1. **A Cloudflare Worker needs kernel behavior**, for example an edge gateway that checks PRNs,
   maps Cedar entities or mints UUIDv7 ids. A sign: a `wrangler.toml` or `@cloudflare/vite-plugin`
   in the repo that imports `@paigasus/kernel`. This is the only event where this spec continues
   with small changes.
2. **A user of the published `@paigasus/wasm` reports a failure on workerd.** A cheaper first
   answer is a README line: "workerd is not supported".
3. **A console or Next middleware moves to an Edge runtime.** This needs a new spec, not this one:
   Next bundles the kernel with its own conditions (§1, Q2).
4. **The toolchain changes**: wasm-bindgen gets a workerd-compatible target, or workerd supports
   the ESM integration of wasm. Then spike S1 can show that no code is necessary.

Before you continue: run spike S1 first (§4.1, about one hour, outside the repo). It tells you if
the work is necessary at all. Then check every repo fact in this spec against `main` again. In one
week (2026-09-27 to 2026-10-04), four sections went out of date.

## 1. The problem

`@paigasus/kernel` has no `workerd` export condition. SMA-427 removed it on purpose. Under workerd
(Cloudflare Workers), the kernel's `.` resolves to `src/wasm.ts`, which imports `@paigasus/wasm`.
That package serves the `wasm-pack --target bundler` glue,
`rs/crates/bindings/paigasus-wasm/paigasus_wasm.js` (wasm-bindgen 0.2.129 since #380):

```js
/* @ts-self-types="./paigasus_wasm.d.ts" */
import * as wasm from "./paigasus_wasm_bg.wasm";
import { __wbg_set_wasm } from "./paigasus_wasm_bg.js";

__wbg_set_wasm(wasm);
wasm.__wbindgen_start();
export { mintUuid7, prnBuild, /* … 13 names … */ sum } from "./paigasus_wasm_bg.js";
```

This glue expects the ESM integration of wasm: the `.wasm` import gives the instance exports as a
namespace. workerd (and wrangler) does not do that. In workerd, a `.wasm` import gives a
`WebAssembly.Module` as the default export. So `wasm.__wbindgen_start` is `undefined`, and the
first import of the kernel throws. Nobody measured this on workerd yet (SMA-427 H3). The claim
above is from the workerd module model, and the spike in §4.1 must confirm it.

**The target consumer.** The target is a plain Cloudflare Worker that wrangler or the Cloudflare
Vite plugin (`@cloudflare/vite-plugin`) bundles. Both bundlers resolve the `workerd` condition.
A Next.js app on Cloudflare (OpenNext) is NOT a target of this issue. `@paigasus/next-config`
lists `@paigasus/kernel` in `SOURCE_ONLY_PACKAGES`
(`ts/packages/paigasus-next-config/src/index.ts:21`). So `next build` bundles the kernel with
Next's own conditions before the adapter runs, and Next's edge conditions do not include
`workerd`. A Next-on-Cloudflare path is a separate issue if a consumer needs it (Q2).

No Edge consumer of the kernel exists today. The two consoles run on the Node runtime. This issue
makes the path ready and proves it, before a consumer needs it.

## 2. Intake (Stage 0)

- **Not already done.** `git log origin/main --grep=workerd --grep=SMA-432` finds only the SMA-427
  commit `60a6da2e`. `git grep workerd origin/main` finds only documents. The kernel manifest has
  no `workerd` key, and no workerd, miniflare or wrangler package is in `ts/pnpm-lock.yaml`.
- **Not blocked.** No upstream release, person or open ticket gates it. The issue has no
  `blockedBy` relation and no comments.
- **A fact that changed since the issue was written.** SMA-634 (ADR-0022) replaced the
  condition-switched exports map. `.` is now `./src/wasm.ts` under every condition, and the napi
  entry moved to `./napi`. The issue text ("it falls through to `default`") is therefore out of
  date, but the effect is the same: workerd gets the bundler glue.
- **Re-checked on 2026-10-04.** No commit since 2026-09-27 mentions workerd or SMA-432. Three
  merged changes touch this design: SMA-673 (#360) added `prnParseFields`, the kernel's `prnParse`
  and a sixth corpus, `prn_parse.json`; #380 moved the glue to wasm-bindgen 0.2.129; SMA-693 (#363)
  added the weekly wasm-bindgen lockstep updater (§5 D9).

## 3. Acceptance criteria

1. A consumer that resolves `@paigasus/kernel` with the `workerd` condition gets a workerd entry.
   A consumer that resolves it without that condition gets `src/wasm.ts`, as today. The `./napi`
   subpath does not change.
2. The workerd entry exposes the same names with the same types as `src/wasm.ts`: the twelve
   re-exported FFI functions, `prnParse()`, `mint()` and the type `PrnParseResult`. Like
   `src/wasm.ts`, it imports `prnParseFields` but does not re-export it (SMA-673 D2). It exports
   nothing more. A compile-time guard fails `tsc` if the two surfaces drift.
3. The workerd entry keeps a synchronous surface. A consumer does not call `await init()`.
4. A test runs the kernel in a real workerd process (through Miniflare), not only a resolution
   check. The test replays all six parity corpora through the workerd entry, including the error
   paths (§6.3), and calls `mint()` once. It runs in the `test` task of a new private workspace
   package (§6.1), so CI runs it on Linux.
5. The test proves which entry the bundler chose, from the Vite build result. It fails when the
   `workerd` key is removed from the kernel manifest. Section 6.5 lists this and the other
   negative controls.
6. The published `@paigasus/wasm` package ships the new workerd glue file and its declaration. The
   release staging in `prebuild.yml` copies both and asserts that they are present. The tarball
   assertion in `release.yml` asserts that `npm pack` includes both.
7. SMA-634 check 4 (the installed copy equals the committed copy) covers the two new files.
8. No behavior changes for the consoles. Their `test`, `build` and `test-e2e` tasks stay green.
   The console image build does not install Miniflare or `workerd`.
9. An ADR-0005 note in Notion records the workerd path, before the plan starts (Q6). This is a
   Notion edit, not a repo file.

## 4. Design

### 4.1 Spike first (Phase 0, go/no-go)

The plan starts with a spike in a scratch directory outside the repo. It measures five facts. The
design below assumes the "expected" column. A different result changes the design as the last
column says.

| # | Question | Expected | If not |
|---|---|---|---|
| S1 | Does the current bundler glue fail on workerd? | It throws at import (`__wbindgen_start` is not a function). | If it runs, stop. The spec returns for review before any code (Q5). The new glue then has no demonstrated need, and it depends on two wasm-bindgen internals. |
| S2 | Does `new WebAssembly.Instance(module, imports)` run synchronously at module scope in workerd, with a `CompiledWasm` module? | Yes. workers-rs (`worker-build`) generates this exact shape. workerd forbids compiling from bytes, not instantiating a precompiled module. | Use top-level `await WebAssembly.instantiate(module, imports)` in the entry. The consumer surface stays synchronous. If TLA is also refused, stop and stand up `await init()` (§5 D4). |
| S3 | Does `miniflare` install and run under pnpm 11, with the repo's real `ts/pnpm-workspace.yaml` overrides (for example `undici >=7.29.0 <8`), and without a new `allowBuilds` entry? | Its `workerd` dependency ships per-platform binaries as optional packages, like esbuild. The overrides do not break Miniflare. | Add `workerd: true` (or `false`, if the postinstall is only a link step) to `allowBuilds` with a measured comment, as for `sharp` and `esbuild`. If an override breaks Miniflare, stop and report: do not loosen a security floor. Run `repo:osv` on the result. |
| S4 | Does Vite 8 (already in the graph) bundle the kernel for workerd with `resolve.conditions: ['workerd', 'worker', 'browser', 'module', 'import', 'default']` and `.wasm` left external as the literal specifier `./paigasus_wasm_bg.wasm`? | Yes. | Add `esbuild` as a direct dev dependency (it is already in the lockfile through tsx) and bundle with it, as wrangler does. |
| S5 | Do the real Cloudflare toolchains choose `src/workerd.ts`? In the scratch directory only, with no repo dependency: `wrangler deploy --dry-run --outdir <dir>` on a worker that imports the kernel. | wrangler chooses `src/workerd.ts` and emits the `.wasm` as a `CompiledWasm` module. | Record the difference, and change the glue or the test so that it matches wrangler. Also record which `.wasm` import form `@cloudflare/vite-plugin` expects (plain `import m from './x.wasm'`, or a `?module` suffix). If it differs from wrangler, record it as a known limit (§9). |

### 4.2 The workerd glue: `rs/crates/bindings/paigasus-wasm/workerd-entry.js` (new, hand-written)

The file uses the two committed files that wasm-pack already produces, the binary and
`paigasus_wasm_bg.js`. No second wasm-pack target and no second binary are necessary.

```js
// SPDX-License-Identifier: Apache-2.0
import * as bg from './paigasus_wasm_bg.js';
import module from './paigasus_wasm_bg.wasm';

const instance = new WebAssembly.Instance(module, { './paigasus_wasm_bg.js': bg });
bg.__wbg_set_wasm(instance.exports);
instance.exports.__wbindgen_start();

export { mintUuid7, prnBuild, /* … the 13 generated names, prnParseFields included … */ sum } from './paigasus_wasm_bg.js';
```

The export list is the full generated list (13 names), the same as `paigasus_wasm.js`. The kernel
entry, not this file, decides which names are public (§4.3).

The module body has side effects (the instantiation). §4.5 adds the file to the package's
`sideEffects` list. Without that entry, a bundler that honours `sideEffects` (webpack, Vite and
rolldown, esbuild and wrangler) can skip the body of a module that only re-exports. Then `wasm`
is undefined in `paigasus_wasm_bg.js`, and the first call fails.

Facts that the file depends on, and what holds each one:

- The binary imports three functions, all from the module name `./paigasus_wasm_bg.js`:
  `__wbg_Error_<hash>`, `__wbindgen_generic_<hash>` and `__wbindgen_init_externref_table`
  (wasm-bindgen 0.2.129; `tests/committed-wasm.test.ts:69-73`). SMA-634 check 2
  (`EXPECTED_IMPORTS`) already pins that module name. The glue passes the whole `bg` namespace as
  that module, so the count of imports does not matter to it. Only the module name does.
- `paigasus_wasm_bg.js` exports `__wbg_set_wasm`. A wasm-bindgen bump can change this. The
  workerd test (§6.2) is what catches it.
- The export list must equal the generated export list. `tsc` does NOT catch a missing name here:
  `workerd-entry.d.ts` re-exports the generated declarations, so its types do not follow the
  hand-written JS list. A static test (§6.4) compares the two lists. The Vite build (a missing
  export is a build error) and the corpus replay also catch it.

A companion `workerd-entry.d.ts` has one line, `export * from './paigasus_wasm.js';`, so the types
come from the generated declarations and cannot drift.

**No lint or format covers the hand-written glue.** It is under `rs/`, so ESLint and Prettier do
not read it. This spec accepts that. The file is about ten lines, and the static test, the Vite
build and the workerd replay run its content. The plan does not add it to a lint scope.

**Why the name is not `paigasus_wasm_workerd.js`.** Several places treat `paigasus_wasm*` as "the
generated family": `prebuild.yml` copies `.wasmpack-release-out/paigasus_wasm*`, and the SMA-634
comments warn about new files beside the five. A hand-written file must not look generated.
`generate-wasm` does not write it, and the drift gate does not compare it with a fresh build.

### 4.3 The kernel entry: `ts/packages/paigasus-kernel/src/workerd.ts` (new) and the manifest

```ts
// SPDX-License-Identifier: Apache-2.0
import { sum, …, prnParseFields, mintUuid7, … } from '@paigasus/wasm/workerd-entry.js';
import { randHex10 } from './mint-util';
import { toPrnParseResult, type PrnParseResult } from './prn-parse';
// `prnParseFields` is imported, NOT re-exported, as in src/wasm.ts (SMA-673 D2).
export { sum, …, mintUuid7, … };            // the same twelve names as src/wasm.ts
export type { PrnParseResult };
export function prnParse(prn: string): PrnParseResult { return toPrnParseResult(prnParseFields(prn)); }
/** … (doc comment, see D5) */
export function mint(): string { return mintUuid7(Date.now(), randHex10()); }
```

The file is `src/wasm.ts` with one changed import source. It exports the same fourteen values
(twelve re-exports, `prnParse`, `mint`) and one type. It has no test marker and no other extra
export (AC2).

`package.json` `exports`:

```json
".": { "workerd": "./src/workerd.ts", "default": "./src/wasm.ts" },
"./napi": "./src/index.ts"
```

`_comment_exports` gets one added sentence: `workerd` resolves to `src/workerd.ts` (SMA-432), and
every other condition still resolves to `src/wasm.ts`.

**`mint()` and `prnParse()` stay duplicated, as they are today.** `src/index.ts` (napi) and
`src/wasm.ts` each have their own one-line `mint()` and `prnParse()`, and `src/workerd.ts` gets
the same lines. The parsing logic itself is shared already: it lives in `toPrnParseResult`
(`src/prn-parse.ts`). A factory would add a module-scope call that bundlers cannot tree-shake
without a `/* @__PURE__ */` note. The whole-module type guard (§4.4) keeps the signatures equal.

### 4.4 The type guard: `src/binding-parity.types.ts`

Add `type WorkerdApi = typeof import('./workerd')` and `type WasmEntry = typeof import('./wasm')`,
and one `Exact<WasmEntry, WorkerdApi>` over the whole module type. This covers `mint` and
`prnParse` too, which the per-function napi/wasm guard (13 checks over the raw bindings) does not.
It also fails if either entry gets an extra export, for example a re-exported `prnParseFields`.

`tsc` resolves `@paigasus/wasm/workerd-entry.js` through the pnpm-installed copy of
`@paigasus/wasm`. See the rollout note in §9.

### 4.5 Release and publish: `@paigasus/wasm`

- `rs/crates/bindings/paigasus-wasm/package.json`:
  - add `workerd-entry.js` and `workerd-entry.d.ts` to `files`;
  - add `"./workerd-entry.js"` to `sideEffects` (§4.2);
  - do NOT add an `exports` map to this package (§5 D1).
- `.github/workflows/prebuild.yml`, "Stage the wasm distribution for upload" (lines 362-373): add
  `cp workerd-entry.js workerd-entry.d.ts wasm-dist/` after the glob copy, and a `test -s` for
  each, with the same `::error::` form as the existing two. This checks the staging directory
  only, not the `files` list.
- `.github/workflows/release.yml`, "Assert the wasm tarball carries its binary" (lines
  1150-1163): extend the Python check so that the `npm pack --dry-run --json` file set must also
  hold `workerd-entry.js` and `workerd-entry.d.ts`. This is the check that proves `files`. Rename
  the step to match.
- `repo:actionlint`, `ci/actionlint/release_guard.py` and the `.github/CLAUDE.md` rules apply to
  both workflow edits. Read `ci/actionlint/README.md` before the edit.

### 4.6 SMA-634 check 4: `ts/packages/paigasus-console-core/testing/installed-wasm.ts`

Add `workerd-entry.js` and `workerd-entry.d.ts` to `FILES`. The check compares the pnpm-installed
copy with the committed copy by hash. The consoles never load `workerd-entry.js`, so a stale copy
of it cannot change a console result. The value of this edit is different: pnpm installs a
`file:` dependency by its `files` list. So the check proves locally that `files` lists both new
files. The comment "ALL FIVE committed artifacts" changes to name seven files.

### 4.7 The console image: `ts/Dockerfile`

The Dockerfile copies the five named `@paigasus/wasm` files into the builder stage, and its
comment says that this is exactly the `files` list. Add `workerd-entry.js` and
`workerd-entry.d.ts` to that `COPY`, so that the comment stays true. The smoke package (§6.1) is
not a dependency of any app, so `pnpm install --filter "@paigasus/${APP}..."` does not install
Miniflare or `workerd` in the image build (AC8).

The lockfile and `pnpm-workspace.yaml` change, so `images.yml` runs on the PR. That is expected.

### 4.8 Moon

- `ts/packages/paigasus-kernel/moon.yml`:
  - `test.inputs`: add `/rs/crates/bindings/paigasus-wasm/workerd-entry.js` and
    `/rs/crates/bindings/paigasus-wasm/workerd-entry.d.ts`, named one by one, as the five
    artifacts are. The static export-list test (§6.4) runs in the kernel's `node` project.
  - `build.inputs`: add `workerd-entry.d.ts`, because `tsc` reads it through the type guard.
- The new smoke package gets its own `moon.yml` (§6.1), with a `test` task whose inputs name the
  kernel `src/**`, the kernel `package.json`, the shared replay module, the corpora, the committed
  binary, `paigasus_wasm_bg.js`, `workerd-entry.js` and the wasm `package.json`.
- `ci/affected-graph/run.sh`: add `run_task_case_ci` cases anchored on
  `rs/crates/bindings/paigasus-wasm/workerd-entry.js`. They expect `paigasus-kernel-ts:test`, the
  smoke package's `test`, and (for `workerd-entry.d.ts`) `paigasus-kernel-ts:build`. Without
  them, a removed input line gives a cached PASS.

Run `repo:affected-smoke` and `repo:input-liveness` locally after this edit, with the bash split
from the root `CLAUDE.md`. The affected-graph suite asserts task input declarations (A5, A7).

## 5. Decisions

- **D1. The condition lives in `@paigasus/kernel`, not in `@paigasus/wasm`.** The alternative was
  an `exports` map on `@paigasus/wasm` with a `workerd` key. That package has no `exports` map
  today. Adding one closes every deep import that is not listed.
  `installed-wasm.ts` does `require.resolve('@paigasus/wasm/<file>')` for all files, and
  Turbopack bundles `@paigasus/wasm` into the consoles' server chunks. The kernel-side condition
  touches neither. It also matches the issue text. The cost: a direct consumer of the published
  `@paigasus/wasm` on workerd must import `@paigasus/wasm/workerd-entry.js` by path. After a
  release, that path is a public entry under semver (Q1).
- **D2. Reuse the bundler binary and `paigasus_wasm_bg.js`; do not add `wasm-pack --target web`.**
  `--target web` emits a second binary with a different import module name (`wbg`). It exports an
  async `init()` and also a sync `initSync({ module })`, so a sync surface is possible with it.
  The reason to reject it is cost: a second committed binary, a second drift gate, a second
  `generate-wasm` out-dir and a second release copy. The hand-written glue is about ten lines
  over files that are already committed and already gated.
- **D3. Test with Miniflare in vitest, not with `@cloudflare/vitest-pool-workers` or wrangler.**
  The pool replaces vitest's runner and pins a vitest range. The catalog has vitest `^5.0.0`, and
  pool support for it is not known. wrangler brings a large dependency tree into the lockfile.
  Miniflare runs the real `workerd` binary, which is what AC4 needs. The spike (S5) runs wrangler
  once outside the repo to confirm that the real toolchain agrees with the test.
- **D4. Do not stand up `await init()`.** The issue says "this is likely where" the async surface
  gets stood up. With a precompiled `WebAssembly.Module`, sync instantiation is allowed on workerd
  (expected, S2). So the sync surface holds, and ADR-0022's "one surface" stays true. If S2 fails,
  this decision reverses, and the spec returns for review before any code.
- **D5. `Date.now()` in `mint()` is not changed.** workerd freezes `Date.now()` during a request
  (a Spectre mitigation). Two mints in one request then get the same millisecond. The kernel's
  UUIDv7 still differs, because 74 random bits follow the timestamp: `mint_uuid7` takes 80 random
  bits, and `uuid7.rs` then overwrites 4 version bits and 2 variant bits. The ordering of two ids
  from one request is then random, not time-ordered. Also, workerd does not allow
  `crypto.getRandomValues` outside a request handler (from the workerd global-scope rules; not
  measured here). So `mint()` at module scope fails on workerd. The `mint()` doc comment in
  `src/workerd.ts` records both facts. Neither is a defect to fix here.
- **D6. `edge-light` (the Vercel Edge runtime and Next.js middleware on Vercel) is out of scope.**
  That runtime is not workerd. It wants `import m from './x.wasm?module'`, which is a third glue
  shape. No consumer asks for it. Open question Q2.
- **D7. Bundle with Vite, which is already in the graph.** `@paigasus/discovery` already uses Vite
  programmatically (SMA-509), so the pattern exists. The only new npm package is `miniflare` (and
  its tree). If S4 fails, use `esbuild` (D3 is unchanged).
- **D8. The workerd smoke test lives in its own private workspace package.** The package is
  `ts/packages/paigasus-kernel-workerd-smoke` (`"private": true`, moon id with the `-ts` suffix).
  It depends on `@paigasus/kernel` and holds the `miniflare` and `vite` dev dependencies. No app
  depends on it. If the test were in the kernel, the console image build would install Miniflare,
  the `workerd` binary and a second `sharp` in both builder legs, because
  `@paigasus/console-core` depends on the kernel.

- **D9. The SMA-693 lockstep updater needs no change.** The weekly updater
  (`.github/workflows/wasm-lockstep.yml`) regenerates the five artifacts and refuses any other
  file with `R-LAYOUT` or `R-STATUS` (`ci/wasm-lockstep/lockstep_check.py:54-62`). It does not
  see `workerd-entry.js`: the stage step copies a fixed list of six files
  (`wasm-lockstep.yml:142-163`), and the `status` check reads modified files only
  (`lockstep_check.py:320-330`). A tracked, unchanged hand-written file is in neither set. The
  updater also cannot tell whether a new wasm-bindgen breaks the hand-written glue. The updater's
  PR runs the normal CI, and the smoke package's `test` inputs name `paigasus_wasm_bg.js` and the
  binary (§4.8), so that PR runs the workerd smoke. That test is the only guard. The plan
  confirms with `moon query tasks --affected` that a change to the five artifacts selects the
  smoke `test`.

## 6. Tests

### 6.1 Placement

- The workerd smoke: `ts/packages/paigasus-kernel-workerd-smoke/tests/workerd.test.ts`, run by
  vitest (`environment: 'node'`, no wasm plugin) in that package's `test` task.
- The static export-list test: `ts/packages/paigasus-kernel/tests/workerd-entry-exports.test.ts`,
  in the kernel's existing `node` project.
- The shared replay: `ts/packages/paigasus-kernel/tests/corpus-replay.mjs` (§6.3).

### 6.2 The workerd smoke

1. Bundle a small test worker, `tests/workerd/worker.ts`, with Vite `build` in library mode:
   ESM output, `resolve.conditions` from S4, `.wasm` imports external with the literal specifier
   `./paigasus_wasm_bg.wasm` (as wrangler emits it; rollup and rolldown otherwise rewrite a
   relative external to a path from `outDir`, which can fall outside Miniflare's modules root).
   Build with `write: false`, or into a temporary directory outside the repo, so that `ts:lint`
   and `ts:fmt` never read the bundle, and the root `build/` ignore rule does not apply.
   The worker imports `@paigasus/kernel` by its package name, so the real exports map chooses the
   entry. `@paigasus/wasm` is aliased to the committed crate directory, not to the pnpm store copy
   (the store copy can be stale, SMA-420). The committed files are the right target: SMA-634
   check 1 already proves the committed glue equals a fresh build.
2. **Entry proof, in Node.** From the Vite build result, collect the module ids of the output
   chunk. Assert that they include `paigasus-kernel/src/workerd.ts` and
   `paigasus-wasm/workerd-entry.js`, and that they do not include `paigasus_wasm.js` or
   `paigasus-kernel/src/wasm.ts`. This replaces a runtime marker, which would break AC2 and the
   type guard.
3. Start Miniflare with two modules: the bundle (`ESModule`) and the committed
   `paigasus_wasm_bg.wasm` (`CompiledWasm`) at `./paigasus_wasm_bg.wasm`. Set the port to 0. Set
   a fixed `compatibilityDate` that is not later than the release date of the pinned `workerd`.
4. The worker's `fetch` handler calls the shared replay (§6.3) over the six corpora (bundled as
   JSON) and calls `mint()` once, inside the handler (D5). It returns
   `{ checked, mismatches, mint }` as JSON.
5. The test asserts: `checked` equals the per-function call counts that the test computes itself
   in Node from the same corpora (not literals that can go stale); `mismatches` is an empty list;
   `mint` matches the UUIDv7 regular expression.
6. `mf.dispose()` in `afterAll`. Give the test a timeout that covers the first workerd start on a
   cold CI runner. The plan measures that start time.

### 6.3 The shared replay, with error paths

Move the replay logic out of `ts/packages/paigasus-kernel/tests/wasm-probe.mjs` into
`tests/corpus-replay.mjs`. The probe and the test worker both import it, so the two replays cannot
diverge.

The probe and the worker see different surfaces. The probe loads the raw `@paigasus/wasm` glue,
which has `prnParseFields`. The worker imports `@paigasus/kernel`, which has `prnParse` and not
`prnParseFields`. So the replay takes the API object and a `parse` adapter for the `prn_parse`
corpus. The probe passes the raw form and compares the six-element array, as today
(`wasm-probe.mjs:88-92`). The worker passes the kernel form and compares the `PrnParseResult`
object that the row describes (`{ ok: true, … }` when `error_kind` is empty, else
`{ ok: false, errorKind }`). The worker's form also runs `toPrnParseResult` on workerd. The module:

- catches per row, and records a mismatch for a throw that the row does not expect;
- counts calls per function, as the probe's `checked` does today, so a worker that skips a
  function fails on the count, not only on a row total;
- adds error-path assertions, which run the glue-sensitive code (the `__wbg_Error_*` import,
  `takeFromExternrefTable0` and the externref table that `__wbindgen_start` sets up):
  - for each `prn_canonical` row with a non-empty `error_kind`: `prnCanonicalize` throws an
    `Error` whose message equals `error_kind`;
  - `mintUuid7` with a malformed `rand_hex`: throws an `Error` with the message `bad-rand-hex`
    (`rs/crates/bindings/paigasus-wasm/src/lib.rs:26`).

The plan confirms that the existing `wasm-probe.mjs` results do not change after the move.

### 6.4 The static export-list test

`workerd-entry-exports.test.ts` reads the committed `paigasus_wasm.js` and extracts the
`export { … } from "./paigasus_wasm_bg.js"` name list. It reads `workerd-entry.js` and extracts
its export list. It asserts that the two lists are equal as sets. It also asserts that
`workerd-entry.js` calls `__wbg_set_wasm` and `__wbindgen_start` once each.

### 6.5 Existing tests

The kernel's `node` and `browser` projects keep their behavior; only the replay moves to the
shared module. `committed-wasm.test.ts` does not change: the hand-written file is not wasm-pack
output.

### 6.6 Proof that the tests bite

Each control is a temporary edit, run once, then reverted by deleting the marked edit (not by
`git checkout --`, which also reverts the change under test). Use `--no-fail-fast`. Re-run the
full set after any fix.

| # | Mutation | Expected failure |
|---|---|---|
| N1 | Remove the `workerd` key from the kernel `exports` | The entry proof (§6.2 step 2) fails: the module ids hold `src/wasm.ts` and `paigasus_wasm.js`. This is the "delete the feature" control. |
| N2 | Remove `instance.exports.__wbindgen_start()` from `workerd-entry.js` | The static test fails, and an error-path assertion fails in the workerd replay (the externref table is not initialized). The plan records which assertion fires. If no replay assertion fires, the error-path design is wrong: stop and report. |
| N3 | Remove one name from the export list in `workerd-entry.js` | The static test (§6.4) fails, and the Vite build fails with a missing-export error. `tsc` does NOT fail (§4.2). |
| N4 | Change one expected value in a corpus row copy inside the worker | `mismatches` is not empty. This proves the replay compares values. |
| N5 | Remove `"./workerd-entry.js"` from `sideEffects` in the wasm `package.json` | The workerd smoke fails (the instantiation body is dropped, or the plan measures that Vite keeps it). If Vite keeps the body, record that the control does not bite under Vite, and keep the entry for webpack and esbuild. |
| N6 | Remove the `cp workerd-entry.js` line from `prebuild.yml` | The new `test -s` step fails. Verify this with a local shell run of the step body in a scratch directory, not in CI. |
| N7 | Remove `workerd-entry.js` from `files` in the wasm `package.json` | The `release.yml` tarball assertion fails. Verify with a local run of `npm pack --dry-run --json` and the step's Python body in a scratch copy. SMA-634 check 4 also fails after a fresh install. |
| N8 | Remove the `workerd-entry.js` input line from the smoke package's `moon.yml` | The new `run_task_case_ci` case in `ci/affected-graph/run.sh` fails. |

## 7. Files

| File | Change |
|---|---|
| `rs/crates/bindings/paigasus-wasm/workerd-entry.js` | new, hand-written glue |
| `rs/crates/bindings/paigasus-wasm/workerd-entry.d.ts` | new, one re-export line |
| `rs/crates/bindings/paigasus-wasm/package.json` | `files` and `sideEffects` get the new files |
| `ts/packages/paigasus-kernel/src/workerd.ts` | new entry |
| `ts/packages/paigasus-kernel/src/binding-parity.types.ts` | whole-module guard |
| `ts/packages/paigasus-kernel/package.json` | `exports` `.` gets `workerd`; `_comment_exports` |
| `ts/packages/paigasus-kernel/tests/corpus-replay.mjs` | new shared replay with error paths |
| `ts/packages/paigasus-kernel/tests/wasm-probe.mjs` | uses the shared replay |
| `ts/packages/paigasus-kernel/tests/workerd-entry-exports.test.ts` | new static test |
| `ts/packages/paigasus-kernel/moon.yml` | inputs (§4.8) |
| `ts/packages/paigasus-kernel-workerd-smoke/package.json` | new private package; devDeps `miniflare: catalog:`, `vite: catalog:`, `vitest: catalog:` |
| `ts/packages/paigasus-kernel-workerd-smoke/moon.yml` | new `test` task and inputs |
| `ts/packages/paigasus-kernel-workerd-smoke/tsconfig.json`, `vitest.config.ts` | new |
| `ts/packages/paigasus-kernel-workerd-smoke/tests/workerd.test.ts` | new |
| `ts/packages/paigasus-kernel-workerd-smoke/tests/workerd/worker.ts` | new test worker |
| `ts/packages/paigasus-console-core/testing/installed-wasm.ts` | `FILES` and its comment |
| `ts/Dockerfile` | `COPY` the two new files |
| `ts/pnpm-workspace.yaml` | catalog entry `miniflare`; `allowBuilds` entry only if S3 needs it |
| `ts/pnpm-lock.yaml` | regenerated |
| `.github/workflows/prebuild.yml` | staging copy and `test -s` |
| `.github/workflows/release.yml` | the tarball assertion covers the two new files |
| `ci/affected-graph/run.sh` | `run_task_case_ci` cases (§4.8) |
| `rs/CLAUDE.md` | a step in the wasm-bindgen runbook (see below) |
| `ts/CLAUDE.md`, the crate `.gitignore` comment | "five artifacts" wording: add that two hand-written files sit beside the five |

**The `rs/CLAUDE.md` step.** Add one step to the wasm-bindgen runbook (the section near line 181),
in the main flow and in the dependabot flow: "The workerd glue `workerd-entry.js` is hand-written.
After a wasm-bindgen move, run the smoke package's `test` and the kernel's `test`." The runbook
now starts near line 181, and its normal path is the SMA-693 lockstep updater (§5 D9). The step
therefore also says that the updater's PR runs both tests through its own CI. Do not put the rule
in `ts/CLAUDE.md`.

Every new source file opens with the SPDX header. Run `ts:fmt` (Prettier) after the TS edits.

## 8. Verification

- `moon run paigasus-kernel-ts:test paigasus-kernel-ts:build paigasus-kernel-ts:typecheck` and
  the smoke package's `test`.
- `moon run paigasus-console-core-ts:test` (check 4) and the two consoles' `test`.
- The full graph from the root `CLAUDE.md` `ci-targets` block, with the bash split that file
  describes for this Mac. `repo:osv` (the npm side; `repo:deny` is cargo-deny only),
  `repo:actionlint` (the two workflow edits), `repo:affected-smoke` and `repo:input-liveness`
  are the gates this change can red.
- pnpm 11's `minimum-release-age`: pick a `miniflare` version older than 24 hours.
- The console image build: `images.yml` runs on the PR. Confirm that it stays green and that its
  install step does not fetch `workerd`.

## 9. Known limits and rollout

- The test proves the kernel on workerd through Miniflare, with a Vite bundle. The spike (S5)
  checks wrangler once, outside the repo. Nothing in CI proves a wrangler deploy, the Cloudflare
  Vite plugin, or a Next.js-on-Cloudflare adapter (OpenNext; see §1).
- The workerd glue depends on two wasm-bindgen internals (`__wbg_set_wasm` and the import module
  name). The tests detect a break; nothing prevents one.
- Miniflare adds a `workerd` binary to a full `pnpm install` of `ts/`, locally and in CI. It does
  not enter the console image build (D8). The plan records the size and the added install time.
- **Rollout note (one time, per machine).** `tsc` resolves `@paigasus/wasm/workerd-entry.js`
  through the installed copy. On a machine that installed before this change, the kernel `tsc`
  and the console check 4 fail until the next install. Run
  `rm -rf ts/node_modules && pnpm -C ts install`. The PR description states this step.

## 10. Out of scope

- The `edge-light` condition and the Vercel Edge runtime (D6).
- A Next.js-on-Cloudflare (OpenNext) path (§1, Q2).
- An async `init()` surface (D4), unless S2 fails.
- Publishing `@paigasus/kernel` (still double-blocked, see its `_comment_publish`).
- wasm-opt and a pinned binaryen (SMA-427 L3).
- A lint or format scope for files under `rs/crates/bindings/paigasus-wasm/` (§4.2).

## 11. Open questions

- **Q1.** D1 puts the `workerd` condition on `@paigasus/kernel` only. Do you also want a `workerd`
  condition on the published `@paigasus/wasm` package? That needs an `exports` map there, which
  must also list every deep path that `installed-wasm.ts` and the consoles use. Related: after a
  release, `@paigasus/wasm/workerd-entry.js` is a public deep path. Do you accept it as a
  supported entry under semver? A later rename or a move to `--target web` then needs a major
  version (or a pre-1.0 minor) and a changelog note.
- **Q2.** Is a Vercel Edge (`edge-light`) path wanted, or a Next.js-on-Cloudflare (OpenNext)
  path? This spec targets plain Cloudflare Workers bundled by wrangler or the Cloudflare Vite
  plugin only.
- **Q3.** Is a new dev dependency on `miniflare` (it brings the `workerd` binary, `sharp`,
  `undici` and more into `ts/pnpm-lock.yaml`) acceptable for a Low-priority issue with no live
  consumer? If yes, is the separate private smoke package (D8) the right way to keep it out of
  the app install closure? The alternative is to keep the issue in Backlog until the first Edge
  consumer exists.
- **Q4.** The issue expected `await init()` here. If S2 confirms sync instantiation on workerd,
  do you agree to keep the sync surface (D4)?
- **Q5.** If S1 shows that the current bundler glue already runs on workerd, this spec stops and
  returns for review (§4.1). Do you agree, or do you want the new glue anyway?
- **Q6.** The repo rule is "ADR before code". Will you write the ADR-0005 note (AC9) before the
  plan starts, or do you want the plan's first step to draft it for you? `.` becomes
  condition-switched again. Does ADR-0022's wording ("`.` is the wasm entry under every
  condition") also need an amendment?

## 12. Challenge changelog

Verdict from the spec-challenger: **APPROVE WITH CHANGES.** Each finding was checked against the
repo before it was folded.

Folded:

- BLOCKER `sideEffects`: confirmed (`"sideEffects": ["./paigasus_wasm.js", "./snippets/*"]`).
  §4.2 and §4.5 add `./workerd-entry.js`. N5 is the new control.
- BLOCKER `entry` marker: confirmed (`tsconfig.json` includes `tests/**/*` and sets no custom
  condition; the whole-module `Exact` forbids an extra export). The marker is removed. §6.2
  step 2 proves the entry from the Vite build module ids. AC2 and AC5 are reworded. N1 asserts on
  the module ids.
- MAJOR `tsc` does not catch a missing JS export: confirmed (the `.d.ts` re-exports the generated
  declarations). §4.2 is corrected. The static test §6.4 is new. N3 is corrected.
- MAJOR the replay has no error path: confirmed (`wasm-probe.mjs:68` calls `prnCanonicalize` only
  when `error_kind === ''`; `bad-rand-hex` is at `lib.rs:25`). §6.3 adds a shared replay module,
  error-path assertions and per-function counts. N2 is reworded.
- MAJOR Miniflare in the console image: confirmed (`ts/Dockerfile` installs with
  `--filter "@paigasus/${APP}..."`, and console-core depends on the kernel). D8 moves the test to
  a private package. §4.7 copies the two new files in the Dockerfile and notes `images.yml`.
- MAJOR the named consumer: confirmed (`SOURCE_ONLY_PACKAGES` lists the kernel). §1 names plain
  Workers as the target. S5 adds a wrangler dry-run in the scratch directory. OpenNext is out of
  scope.
- MAJOR the tarball: confirmed (`release.yml` asserts only `paigasus_wasm_bg.wasm`). §4.5 extends
  it. N7 is new.
- MINOR `makeMint`: confirmed (`src/index.ts` keeps its own `mint()`). The refactor is dropped.
- MINOR D5 facts: confirmed (`uuid7.rs` overwrites 6 bits, so 74 random bits remain). D5 is
  corrected and records the `getRandomValues` rule as believed, not measured.
- MINOR D2 reason: the text now says that `--target web` has `initSync`, and that the decision
  rests on cost.
- MINOR check 4 rationale: §4.6 is corrected. N5 (the non-control) is removed. §9 adds the
  one-time reinstall step.
- MINOR affected-graph control: §4.8 adds `run_task_case_ci` cases. N8 is new.
- MINOR test mechanics: `write: false`, the literal `.wasm` specifier, port 0 and the
  `compatibilityDate` rule are in §6.2.
- MINOR S3 overrides: S3 now runs under the real overrides and runs `repo:osv`.
- MINOR lint scope: accepted explicitly in §4.2 and §10.
- MINOR CLAUDE.md placement: the rule is a step in the `rs/CLAUDE.md` wasm-bindgen runbook. The
  "five artifacts" wording is listed in §7.
- MINOR `repo:deny`: §8 is corrected.
- QUESTIONS: the semver question joins Q1. The ADR timing is Q6. The S1 question is Q5, and S1
  now stops the work. The smoke-package question joins Q3.

Rejected: none. Two points are narrowed, not rejected:

- N5 (`sideEffects`) may not bite under Vite if Vite keeps the body. The control records that
  result instead of asserting a failure that nobody measured.
- The `getRandomValues` rule on workerd is recorded as believed, as the critique marked it.

### Freshness re-check (2026-10-04)

The spec file was lost with the old session's scratchpad. It was rebuilt byte for byte from that
session's transcript, then checked against `origin/main` `4b2817a9` by a fact-check subagent, and
the main findings were verified by hand.

- Changed: SMA-673 (#360) added `prnParseFields`, `prnParse` and `prn_parse.json`. AC2, AC4,
  §4.2, §4.3, §4.4 and §6.3 now name thirteen FFI functions, the `prnParse` surface and six
  corpora. §6.3 adds the `parse` adapter.
- Changed: wasm-bindgen 0.2.129 (#380). §1 shows the current glue. §4.2 names three imports, not
  two. The design does not depend on the count.
- New: the SMA-693 lockstep updater. D9 records why it needs no change. A subagent claim that the
  hand-written file trips `R-LAYOUT` was checked and rejected: the stage step and the status
  check see only the five artifacts and `rs/Cargo.lock`.
- Line numbers: `prebuild.yml` 362-373, `release.yml` 1150-1163, `rs/CLAUDE.md` runbook near 181.
- Still true: the exports map, `sideEffects`, `files`, `SOURCE_ONLY_PACKAGES`, `installed-wasm.ts`
  `FILES`, the Dockerfile `COPY` and `--filter` install, the vite and vitest catalog versions, no
  miniflare in the lockfile, `run_task_case_ci`, `bad-rand-hex` (now `lib.rs:26`).
