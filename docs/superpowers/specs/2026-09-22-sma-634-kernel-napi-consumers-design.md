# SMA-634 — Node consumers load the kernel through its wasm entry

**Linear:** [SMA-634](https://linear.app/smaschek/issue/SMA-634/ts-paigasuskernel-napi-binding-never-reaches-node-modules-files)
**Related:** SMA-511 (spec § 4.7, D6), SMA-512, SMA-513 (spec F5)
**ADR:** ADR-0005 (cross-language behavior lives once in `paigasus-kernel`)
**Date:** 2026-09-22
**Revision:** 2, design W1. Revision 1 (napi through `link:`) was disproved by measurement (§ 2).
§ 16 records the challenge.

---

## 1. Problem

`@paigasus/kernel` imports `@paigasus/node-bindings` under the `node` export condition. That
package is a pnpm `file:` dependency on `rs/crates/bindings/paigasus-node-bindings`. pnpm installs a
`file:` target one time, at install time, and only with the paths in the `files` allowlist. The
allowlist is `["index.js", "index.d.ts"]`, so the `.node` binary never reaches `node_modules`.

A Next 16 app that imports the kernel from a server component fails `next build` with `Cannot find
native binding`. So SMA-511 added a hand-written PRN reader to the consoles, as a recorded ADR-0005
exception. It now lives in `ts/packages/paigasus-console-core/src/prn-tenancy.ts`.

This design lets Node consumers in the monorepo load the kernel, and it removes the ADR-0005
exception.

---

## 2. Decision history

1. Revision 1 chose approach A: change both binding dependencies to `link:`, and use the napi entry
   in Next. The user approved it.
2. The adversarial challenge returned **NEEDS REWORK**. Its blocker: the tracing of a `.node` file
   outside `outputFileTracingRoot` was not measured, and two tool rules predicted a failure.
3. A spike measured the question on 2026-09-22 (Next 16.3.5 with Turbopack, pnpm 11.3.0, node
   24.16.0, @napi-rs/cli 3.10.3). The logs are in the session scratchpad,
   `…/scratchpad/sma634-spike/<variant>.{build,server}.log`. The results:

| Variant | Build | Regular `.node` in standalone | Copy outside the repo |
| -- | -- | -- | -- |
| V0: `file:` (today) | exit 1, `Cannot find native binding` | – | – |
| V1: `link:`, tracing root `ts/` | exit 1, `Can't resolve '@paigasus/node-bindings'` | – | – |
| V2: V1 + `serverExternalPackages` | exit 1, same error | – | – |
| V3a–c: `link:`, tracing root at the repo root | exit 1, `non-ecmascript placeable asset` | – | – |
| V4: webpack | exit 1, `Module parse failed` on the `.node` | – | – |
| V3d-repo: repo root + a runtime `createRequire` + `outputFileTracingIncludes` | exit 0 | yes | HTTP 200 |
| W-ts: wasm through `link:`, tracing root `ts/` | exit 1, `Can't resolve '@paigasus/wasm'` | – | – |
| W-repo: wasm through `link:`, repo root | exit 0 | n/a (bundled) | HTTP 200 |

   Turbopack does not resolve a symlink whose target is outside its root. `serverExternalPackages`
   is silently ignored for a `link:` package. The only napi path that works needs a bundler
   workaround in the kernel entry and a move of the tracing root, and that move changes the
   `server.js` path in every console build, e2e helper and image script.
4. The user chose **W1**: in-monorepo Node consumers use the kernel wasm entry, with a committed
   `.wasm` file and the current `file:` link.
5. A second spike measured W1 on 2026-09-22. The logs are in `…/scratchpad/sma634-w1/`. § 3 F10 to
   F13 hold the results.

---

## 3. As-built findings (measured 2026-09-22)

Read from the tree at `8c1297b7`, unless the finding names a spike.

### F1 — The published napi model is correct and does not change

The main `@paigasus/node-bindings` npm package is loader-only. Seven per-platform packages carry
the binaries (`prebuild.yml`, `release.yml`). `prebuild.yml` asserts that the main tarball holds no
`.node` file. The `files` allowlist is correct for this model. The defect is only that the monorepo
`file:` link cannot use this model.

### F2 — The wasm `files` allowlist already includes the binary

`rs/crates/bindings/paigasus-wasm/package.json` lists `paigasus_wasm_bg.wasm` in `files`. The
`.wasm` is gitignored (`rs/crates/bindings/paigasus-wasm/.gitignore`), so it is absent at install
time in CI. SMA-511 § 13 row 10 (B2) failed for this reason only.

### F3 — `paigasus-kernel-ts:build` is the only writer of the committed wasm glue

The `build` task runs wasm-pack into `.wasmpack-out` and copies `paigasus_wasm*` into the crate
directory. The `test` task builds into `.wasmpack-test-out` and aliases vitest at that directory.
It does not write the crate directory.

### F4 — Every caller of `prn-tenancy` runs on the server

`console-core/src/index.ts` and `prn-tenancy.ts` import `server-only`. The callers are server
components and `server-only` modules in `iam-console` and `gateway-console`. Neither `proxy.ts`
imports `@paigasus/console-core`, and `paigasus/boundaries/app-middleware` enforces that.

### F5 — The kernel has PRN grammar only

The kernel has no IAM tenancy rule, no IAM root PRN and no UUID predicate. `prnErrorKind` returns
`""` for a valid PRN. `prnOrg` returns `""` for an absent org. The other accessors and `prnBuild`
throw an `Error` whose message is the PRN error kind.

### F6 — No gate reads the `file:` specifiers

Only `ts/pnpm-lock.yaml` records them.

### F7 — Moon adds no project edge for a `workspace:*` dependency

Measured in this worktree: `"@paigasus/kernel": "workspace:*"` added to
`console-core/package.json` did not change `moon project paigasus-console-core-ts` "Depends on", with
the Moon cache on and with `MOON_CACHE=off`. So a dependency without `dependsOn` changes no
project case in `ci/affected-graph/run.sh`.

### F8 — The console image install will reach `rs/`

`ts/Dockerfile` uses `ts/` as its build context and runs `pnpm install --frozen-lockfile --filter
"@paigasus/${APP}..."`. When `console-core` depends on `@paigasus/kernel`, that subgraph includes
the kernel's two `file:` dependencies at `../rs/crates/bindings/…`, which are outside the context.
`rs/.dockerignore` excludes `**/*.wasm`.

### F9 — Next config

`createNextConfig()` lists `@paigasus/kernel` in `transpilePackages`, sets `output: 'standalone'`
and takes `outputFileTracingRoot` (`ts/`) from each app. It sets no `serverExternalPackages`.

### F10 — W1 builds and runs (W1 spike, part A)

- With the `.wasm` present at install time, pnpm installs all five `@paigasus/wasm` files. **pnpm
  hard-links them to the crate directory** (same inode, link count 2).
- A fixture route that imports `@paigasus/kernel/wasm` builds with `createNextConfig()` and the
  tracing root `ts/` (exit 0). A bare config without `transpilePackages` also builds.
- The standalone tree holds one `.wasm` chunk at
  `apps/<app>/.next/server/chunks/…paigasus_wasm_bg….wasm`, with the source sha256. No symlink
  points out of the tree. `@paigasus/wasm` is bundled, so standalone has no `node_modules/@paigasus`.
- The copy outside the repo answers HTTP 200 with the correct PRN for a valid input, and with
  `kind: "wrong-field-count"` for `not-a-prn`.
- Red control: without the `.wasm` in a fresh `node_modules`, the build fails with `Module not
  found: Can't resolve './paigasus_wasm_bg.wasm'`.

### F11 — The console vitest config needs no change (W1 spike, part B)

The present `console-core` vitest config (environment `node`, conditions `['node', 'import',
'default']`) loads `@paigasus/kernel/wasm`. vitest externalizes `@paigasus/wasm`, and Node 24 loads
the `.wasm` natively. `vite-plugin-wasm` is not necessary. Node prints `ExperimentalWarning:
Importing WebAssembly module instances is an experimental feature` on stderr.

### F12 — The `.wasm` bytes depend on the host (W1 spike, part C)

- On one host the build is deterministic: three macOS builds gave the same sha256 (48227 bytes).
- macOS, Linux arm64 and Linux amd64 gave three different sha256 values, also with
  `--remap-path-prefix`. The code, element, `name` and `producers` sections differ. The wasm-bindgen
  CLI is a `cargo install` build on macOS and a prebuilt binary on Linux.
- The macOS `.wasm` without remapping holds absolute home paths (`/Users/<user>/.cargo/registry/…`,
  `/Users/<user>/.rustup/toolchains/…`).
- `wasm-opt = false` is set, so wasm-pack downloads no binaryen.

So a gate that compares the committed `.wasm` byte for byte with a new build cannot work.

### F13 — The committed wasm glue is stale today (W1 spike)

A new build on all three hosts makes `paigasus_wasm_bg.js` with `__wbg_Error_408e67f47ca7b58b`. The
committed file has `__wbg_Error_92b29b0548f8b746` (commit `0b2e346a`, SMA-448). The committed glue
with a current `.wasm` fails in Node with `LinkError: … "__wbg_Error_408e67f47ca7b58b": function
import requires a callable`. The `build` task hides this, because it overwrites the glue on each run.

### F14 — pnpm does not refresh a `file:` copy after an unlink (W1 spike, R1–R4)

- An in-place overwrite of a crate file (`cp` onto it) changes the installed copy at once, because
  of the hard link.
- An unlink and replace (a delete, `git checkout`, a branch switch) breaks the hard link. A plain
  `pnpm install` then reports "Already up to date" and keeps the old copy. `pnpm install --force`
  does not repair a deleted installed directory. Only `rm -rf ts/node_modules` followed by `pnpm
  install` makes a new copy.
- CI always installs into a new `node_modules`, so CI is not affected.

---

## 4. Decisions

| # | Topic | Decision |
| -- | -- | -- |
| D1 | The Node route | In-monorepo Node consumers use the kernel wasm entry through a new `./wasm` subpath export. The napi binding stays for published npm consumers only. Its `files` allowlist is correct for that model (F1) |
| D2 | The link | Both kernel binding dependencies stay `file:` |
| D3 | The artifact | `paigasus_wasm_bg.wasm` is committed, together with the glue from the same build. The normal `build` and `test` tasks never write the committed files. A separate task, `paigasus-kernel-ts:generate-wasm`, writes them, with the home and repository paths remapped |
| D4 | The drift gate | Four checks in `paigasus-kernel-ts:test` (§ 5.4): a stamp over the build inputs, the committed glue and `.wasm` load together, the committed pair replays the full parity corpus, and the installed copy matches the committed file |
| D5 | Delivery | **One PR.** Revision 1 split the work because of the image risk. W1 has no Rust stage in the image, and the console migration is small. The kernel commits come first in the PR, so a reviewer can read them alone |
| D6 | Next config | No change (F10) |
| D7 | The image | A small change only: a named build context supplies the two binding directories for the install, and `rs/.dockerignore` keeps the committed `.wasm`. The image build compiles no Rust. The SMA-513 F5 statement "no Rust, no napi" stays true. "No wasm" becomes false: the image holds the bundled `.wasm` chunk |
| D8 | ADR-0005 | `prn-tenancy.ts` becomes an IAM adapter over the kernel wasm entry. The exception is removed. No ADR change is necessary: the kernel stays the one source of PRN behavior |

---

## 5. The kernel

### 5.1 The `./wasm` export

`ts/packages/paigasus-kernel/package.json` `exports` gets `"./wasm": "./src/wasm.ts"`. The `.`
entry does not change. The `_comment_exports` field states: in-monorepo Node consumers import
`@paigasus/kernel/wasm`; the `node` condition of `.` is for published npm consumers, whose
per-platform packages carry the `.node` file (SMA-634). No kernel README exists, and this design
adds none.

### 5.2 The committed artifacts

- `rs/crates/bindings/paigasus-wasm/.gitignore` keeps `*.wasm` and adds
  `!paigasus_wasm_bg.wasm`, so only that one file is committed.
- The PR commits a new `paigasus_wasm_bg.wasm`, `paigasus_wasm_bg.js`, `paigasus_wasm.js`,
  `paigasus_wasm.d.ts` and `paigasus_wasm_bg.wasm.d.ts` from one `generate-wasm` run. This also
  repairs the stale glue (F13).
- A new committed stamp file, `rs/crates/bindings/paigasus-wasm/paigasus_wasm.stamp`, holds the
  stamp hash (§ 5.4 check 1) and the host that ran `generate-wasm`.

### 5.3 The `generate-wasm` task and the changed `build` task

**`paigasus-kernel-ts:generate-wasm`** is a new Moon task. It has `options.runInCI: false` and
`options.cache: false`. It is not in `ci.yml`'s `T=(…)` array, because CI never runs it.

1. It computes the stamp (§ 5.4 check 1). If the committed stamp is equal, it prints "up to date"
   and stops. This keeps a second host from making a byte diff with no source change (F12).
2. It runs wasm-pack from inside the crate directory into `.wasmpack-out`, with the same flags as
   today. It sets `CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS` with `--remap-path-prefix` for
   `$CARGO_HOME`, the rustup sysroot (`rustc --print sysroot`) and the repository root. It uses the
   target-scoped variable, not `RUSTFLAGS`, so the host build scripts and the apple-darwin link
   flags in `rs/.cargo/config.toml` do not change.
3. It checks that the new `.wasm` holds no absolute home path (a byte search for the value of
   `$HOME`). If it finds one, it stops with an error.
4. It copies the five files into the crate directory with an in-place overwrite (`cp` onto the
   existing file), so the hard-linked installed copy changes too (F14). Then it writes the stamp.

**`paigasus-kernel-ts:build`** stops running wasm-pack and stops copying glue. No task uses that
output after this change: the kernel's browser tests use the `test` task's scratch build. The build
task keeps `napi build` and `tsc`. Its declared output `paigasus_wasm_bg.wasm` is removed. Its
`/rs/crates/bindings/paigasus-wasm/**` inputs are removed.

### 5.4 The drift gate

The gate is a new vitest file, `ts/packages/paigasus-kernel/tests/committed-wasm.test.ts`, in the
`node` project of `paigasus-kernel-ts:test`.

**Why a kernel test and not a `repo:*` gate.** A new `repo:*` gate has seven registration
obligations (`ci/CLAUDE.md`, "Registering a new `repo:*` gate"), including `ci.yml`'s `T=(…)`
array and the root `CLAUDE.md` `ci-targets` block. `paigasus-kernel-ts:test` is already in the CI
graph. Its inputs already hold every stamp input except the new files, so an edit that can make the
stamp stale already selects it.

The four checks:

1. **Stamp.** A Node script, `ts/packages/paigasus-kernel/scripts/wasm-stamp.mjs`, computes a
   sha256 over, in a fixed order:
   - the path and content of every file in `rs/crates/libs/paigasus-kernel/src/**`,
     `rs/crates/libs/paigasus-kernel/Cargo.toml`, `rs/crates/bindings/paigasus-wasm/src/**`,
     `rs/crates/bindings/paigasus-wasm/Cargo.toml`, `rs/Cargo.toml`, `rs/rust-toolchain.toml` and
     the stamp script itself;
   - the `wasm-pack` line of `.prototools`;
   - the output of `cargo tree -p paigasus-wasm --target wasm32-unknown-unknown -e normal,build
     --prefix none --locked`, run from `rs/`. This covers the lockfile entries of the wasm
     dependency closure only, so a `Cargo.lock` bump of an unrelated crate does not make the stamp
     stale. `cargo tree` reads metadata and compiles nothing.
   The test fails when the computed value differs from `paigasus_wasm.stamp`. The message says: run
   `moon run paigasus-kernel-ts:generate-wasm` and commit the six files.
2. **Load.** The test imports the committed `rs/crates/bindings/paigasus-wasm/paigasus_wasm.js` by
   its file URL, not through an alias, and calls `sum(2, 3)`. A glue and `.wasm` mismatch fails
   here with a `LinkError` (F13).
3. **Corpus.** The test replays every vector of `prn_fields.json` and `prn_canonical.json` through
   the committed pair. The existing `*.wasm.test.ts` files keep testing the new scratch build, so
   the committed pair and the source are both held to the corpus.
4. **Installed copy.** The test resolves `@paigasus/wasm` with Node's resolver from the kernel
   package and compares the sha256 of the installed `paigasus_wasm_bg.wasm` and
   `paigasus_wasm_bg.js` with the committed files. The message says: run `rm -rf ts/node_modules &&
   pnpm -C ts install` (F14). In CI this check always passes. It exists for local runs.

New `test` task inputs: the five committed wasm files, the stamp file and `scripts/**/*`.

**Negative controls (red-first for the gate).** The plan runs each one and records the red result,
then restores the files by deletion of a marked edit, not by `git checkout` of the whole tree:

| Control | The check that must red |
| -- | -- |
| A comment edit in `rs/crates/libs/paigasus-kernel/src/lib.rs` | 1 |
| The glue from commit `0b2e346a` with the new `.wasm` | 2 |
| A `.wasm` built from a kernel with one changed error-kind string, with the stamp rewritten | 3 |
| A changed byte in the installed copy only | 4 |

The control for check 3 must compile, so it changes a string value, not code shape.

### 5.5 Kernel records

- `ts/packages/paigasus-kernel/moon.yml`: the comments on `build` and `test` change to match
  § 5.3 and § 5.4.
- `rs/crates/bindings/paigasus-wasm/.gitignore`: its header comment changes to say that the
  `.wasm` and the glue are committed from `generate-wasm`.

---

## 6. `console-core` uses the kernel

### 6.1 The dependency

`ts/packages/paigasus-console-core/package.json` gets `"@paigasus/kernel": "workspace:*"` in
`dependencies`. `moon.yml` gets no `dependsOn` edge and no task `deps` (§ 7).

### 6.2 `prn-tenancy.ts` becomes an IAM adapter

The file imports `prnBuild`, `prnErrorKind`, `prnService`, `prnRegion`, `prnOrg`,
`prnResourceType` and `prnResourceId` from `@paigasus/kernel/wasm`. The exports, the `null` and
`TypeError` behavior and the lower-case ids do not change. No caller in the two apps changes.

| Export | New source |
| -- | -- |
| `parseTenancyPrn(prn)` | If `prnErrorKind(prn) !== ''`, return `null`. Otherwise read the five fields. Then apply the IAM rule without change: service `iam`; an empty region; resource type `organization`, `team` or `project`; `prnOrg` is `''` for an organization and a UUID for a team or a project. For an organization, `orgId` is the resource id. Ids are lower-case |
| `organizationPrn(orgId)` | `requireUuid('orgId', orgId)`, then `prnBuild('iam', '', '', 'organization', orgId)` |
| `teamPrn(orgId, teamId)` | `requireUuid` on both, then `prnBuild('iam', '', orgId, 'team', teamId)` |
| `projectPrn(orgId, projectId)` | `requireUuid` on both, then `prnBuild('iam', '', orgId, 'project', projectId)` |
| `ROOT_PRN` | Stays a constant. The test against the live Rust `root_prn()` stays |
| `isUuid` | Stays a local regex. It guards the shape of a URL segment. It is not PRN grammar (F5) |

The file removes `MAX_LEN` and the hand-written field grammar. The header comment states that the
file holds the IAM tenancy rule over the kernel grammar, and that it is not an ADR-0005 exception.

A non-empty region stays a `null` result. This is IAM policy: `TenancyRef` has no region field.

`prnBuild` can throw for an input that `requireUuid` accepts only if the kernel grammar changes.
The builders do not catch that error.

### 6.3 Tests

- `tests/unit/prn-tenancy.test.ts` keeps the corpus replay, the builder round trips, the
  `ROOT_PRN` checks and the negative table. They now test the adapter.
- A new structure test reads `src/prn-tenancy.ts` as text. It asserts that the file imports from
  `@paigasus/kernel/wasm`, and that the `UUID` regex is the only regular expression literal in the
  file. So a hand-written grammar cannot come back without a red test.
- The consoles' unit, integration and e2e tests run against the real kernel. They must pass
  without change. The e2e tiers open org, team and project pages, which call `parseTenancyPrn` and
  the builders.

### 6.4 An ESLint rule

`ts/packages/paigasus-next-config/src/eslint.mjs` bans the bare `@paigasus/kernel` import in
`packages/paigasus-console-core/**` and `apps/**`, with the message "use `@paigasus/kernel/wasm`
in a Next or console package (SMA-634)". The subpath stays allowed. A row in
`tests/boundaries.test.ts` proves the ban, and a second row proves that the subpath passes. This
stops a later change from choosing the napi entry in a Next build by accident.

---

## 7. Moon wiring

- `console-core` `test`, `typecheck` and `lint` (where the task exists), and the `build`,
  `typecheck`, `test` and `test-e2e` tasks of both apps, get these inputs:
  - `/ts/packages/paigasus-kernel/src/**/*`
  - `/ts/packages/paigasus-kernel/package.json`
  - `/rs/crates/bindings/paigasus-wasm/paigasus_wasm*` (the five committed files)
  - `/rs/crates/bindings/paigasus-wasm/package.json`
- No task gets `deps` on a kernel task, and no project gets a `dependsOn` edge. The consoles read
  committed files, so they need no Rust build.
- A Rust kernel edit selects no console task. That is correct: the consoles use the committed
  `.wasm`, and the stamp check in `paigasus-kernel-ts:test` reds until `generate-wasm` runs. The
  regenerated files then select the console tasks through the inputs above.
- F7 shows that no project case in `ci/affected-graph/run.sh` changes. The task cases
  (`kernel->consumer-tasks`, `lockfile->all-lint`, and the ui and app-shell console cases) edit no
  path in the new inputs. The plan runs `repo:affected-smoke` under system `/bin/bash` 3.2 to
  confirm this, and re-baselines a case only if the run shows a change.

---

## 8. Required-CI proof in the console builds

Each app's Moon `build` task already checks `.next/standalone/apps/<app>/server.js`. Beside that
check it adds:

- a regular file that matches `*paigasus_wasm_bg*.wasm` under
  `.next/standalone/apps/<app>/.next/server/chunks/` (`find -type f`, without `-L`);
- no symlink in `.next/standalone` whose target resolves outside `.next/standalone`.

So `moon ci` fails if Turbopack stops bundling the wasm, without the image workflow.

---

## 9. The console image

- `ci/images/run.sh` `build_console_one` adds `--build-context rs="$ROOT/rs"`. The comment on
  lines 376–378 changes, because a named context is new there.
- `ts/Dockerfile` builder stage, before `pnpm install`: `COPY --from=rs
  crates/bindings/paigasus-wasm /rs/crates/bindings/paigasus-wasm` and the same for
  `crates/bindings/paigasus-node-bindings`. With `WORKDIR /build` as `ts/`, the `file:` paths
  resolve to `/rs/crates/bindings/…`. No Rust is compiled. The napi directory is necessary only
  because the kernel declares it. The image does not load it.
- `rs/.dockerignore` adds `!crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm` after
  `**/*.wasm`. The `rs/Dockerfile` context then also holds this 48 KB file. That is harmless.
- `ts/Dockerfile` adds the same regular-file `.wasm` chunk check as § 8, after the `server.js`
  check.
- `.github/workflows/images.yml` `pull_request` paths add `rs/crates/bindings/paigasus-wasm/**`,
  `rs/crates/bindings/paigasus-node-bindings/package.json` and `ts/packages/paigasus-kernel/**`.
  `rs/.dockerignore` is already in the list.
- The existing console smoke test is unchanged. `/healthz` does not load the kernel (§ 13).

---

## 10. Developer workflow and traps

- **After an edit to the Rust kernel or the wasm binding:** run `moon run
  paigasus-kernel-ts:generate-wasm`, then commit the five wasm files and the stamp. The stamp check
  reds until you do.
- **After a branch switch, a rebase or a `git checkout` that changes the committed wasm files:**
  run `rm -rf ts/node_modules && pnpm -C ts install`. A plain `pnpm install` does not refresh the
  installed copy (F14). Check 4 of the gate tells you when this is necessary.
- **A Dependabot bump inside the wasm dependency closure** (for example `wasm-bindgen`) reds the
  stamp check. Someone must run `generate-wasm` on the Dependabot branch and push the result.
- **Only one host needs to run `generate-wasm`.** A second host makes different bytes (F12), but
  the task stops early when the stamp is equal.
- **vitest stderr** in `console-core` and the apps shows Node's `ExperimentalWarning` for the wasm
  import (F11). It is harmless. Do not read it as a failure.
- **Windows reserved names** apply to every path component, directories included. No new file or
  directory in this design uses one (`committed-wasm.test.ts`, `wasm-stamp.mjs`,
  `paigasus_wasm.stamp`).
- **The root `.gitignore` `build/` rule** ignores any directory named `build/`. No new path in this
  design uses one.
- Every new source file opens with an SPDX header: `// SPDX-License-Identifier: Apache-2.0`. The
  stamp file is data, and it has no header.

---

## 11. Error handling

Every new check fails closed.

- A stale stamp, a glue and `.wasm` mismatch, a corpus failure or a stale installed copy fails
  `paigasus-kernel-ts:test`, with a message that names the command to run.
- `generate-wasm` stops if the new `.wasm` holds a home path.
- A missing `.wasm` chunk, or a symlink out of the standalone tree, fails the console `build` task
  and the image build.
- A kernel error in `parseTenancyPrn` becomes `null`. A bad id in a builder becomes a `TypeError`.
  This is the current contract.

---

## 12. Measurements to take in the plan

| # | What | Rule |
| -- | -- | -- |
| M1 | `generate-wasm` twice on one host with the remap flags: are the bytes equal, and does the home-path search find nothing? | If the bytes differ on one host, stop and report |
| M2 | The runtime of the four gate checks, and of `cargo tree` in check 1 | Record it in the PR |
| M3 | Does BuildKit apply `rs/.dockerignore` to the named context `rs`? | Either answer is safe, because the `COPY` names two directories. Record the context upload size |
| M4 | The four negative controls of § 5.4 | Each must red, and each must pass after the restore |
| M5 | `repo:affected-smoke` under system `/bin/bash` 3.2 after the Moon changes | Re-baseline a case only if the run shows a change, and record why |
| M6 | The console image builds on amd64 in the PR, and on arm64 through `gh workflow run images.yml --ref <branch>` | Required before the merge |

---

## 13. Recorded limits

- The committed `.wasm` bytes depend on the host that made them (F12). The gate proves the source
  inputs and the behavior, not the bytes.
- A Dependabot bump inside the wasm dependency closure needs a manual `generate-wasm` commit.
- `/healthz` does not load the kernel. A container whose wasm chunk fails to load reports healthy
  while the PRN pages return 500. Readiness stays config-only. The build checks in § 8 and § 9 make
  this failure unlikely.
- The napi entry stays unusable in a Next build. Published npm consumers are not affected (F1).
- `isUuid`, `ROOT_PRN` and the IAM tenancy rule stay hand-written. They are IAM domain rules, not
  kernel grammar.
- The wasm route in a Next **client** bundle stays unproven. No client consumer exists.
- This spec supersedes the SMA-511 § 12 D6 limit and the "no wasm" part of SMA-513 F5. Those specs
  are historical records, and this PR does not edit them.

---

## 14. Files touched (one PR)

**Kernel commits**

- `ts/packages/paigasus-kernel/package.json` (the `./wasm` export and the comments)
- `ts/packages/paigasus-kernel/moon.yml` (`generate-wasm`, the changed `build`, the `test` inputs)
- `ts/packages/paigasus-kernel/scripts/wasm-stamp.mjs` (new)
- `ts/packages/paigasus-kernel/tests/committed-wasm.test.ts` (new)
- `rs/crates/bindings/paigasus-wasm/.gitignore`
- `rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm` (new), `paigasus_wasm_bg.js`,
  `paigasus_wasm.js`, `paigasus_wasm.d.ts`, `paigasus_wasm_bg.wasm.d.ts`,
  `paigasus_wasm.stamp` (new)

**Console commits**

- `ts/packages/paigasus-console-core/package.json`, `moon.yml`, `src/prn-tenancy.ts`,
  `tests/unit/prn-tenancy.test.ts`, a new structure test
- `ts/apps/iam-console/moon.yml`, `ts/apps/gateway-console/moon.yml`
- `ts/packages/paigasus-next-config/src/eslint.mjs`, `tests/boundaries.test.ts`
- `ts/pnpm-lock.yaml`

**Image commits**

- `ts/Dockerfile`, `rs/.dockerignore`, `ci/images/run.sh`, `.github/workflows/images.yml`

**Records**

- `ts/CLAUDE.md`: replace the SMA-634 entry and the "fallback C" entry with the W1 facts (§ 10)
- `ci/affected-graph/run.sh` and `README.md` only if M5 shows a change

---

## 15. Acceptance

- `@paigasus/kernel/wasm` resolves, and `prn-tenancy.ts` imports it and holds no PRN grammar. The
  structure test proves this.
- The committed `.wasm` and glue load together and pass the full parity corpus in
  `paigasus-kernel-ts:test`.
- The four negative controls each red the gate.
- Both console builds contain a regular `.wasm` chunk and no symlink out of the standalone tree.
- The consoles' unit, integration and e2e tiers pass.
- Both console images build on amd64 and arm64 without a Rust compile.
- The published npm model is unchanged: `prebuild.yml`'s loader-only assertion still passes.
- The full gate graph in the root `CLAUDE.md` passes. The bash-split gates are re-run with the
  correct bash, as the root `CLAUDE.md` describes.

---

## 16. Challenge log

### Round 1 (revision 1, verdict NEEDS REWORK)

| Severity | Finding | Disposition |
| -- | -- | -- |
| BLOCKER | D1 rests on an unmeasured tracing premise | Folded. Measured (§ 2). The premise was false, and the design changed to W1 |
| MAJOR | Affected-graph cases need a re-baseline for a new `dependsOn` | Moot. W1 adds no `dependsOn`, and F7 shows that a `workspace:*` dependency adds no edge. M5 confirms |
| MAJOR | Console tasks need `inputs` on the kernel, not only `deps` | Folded (§ 7) |
| MAJOR | The red-first run never reaches steps 3–5 | Moot. The fixture is gone. The gate has four negative controls (§ 5.4), and § 8 checks the real builds |
| MAJOR | The fixture collides with kernel globs and needs dependencies | Moot. No fixture |
| MAJOR | Two kernel tasks can rewrite the shared `.node` | Moot for W1. Also measured: `napi build` replaces the file by rename |
| MAJOR | Dependabot proposes Rust bumps for `ts/Dockerfile` | Moot. No Rust stage in `ts/Dockerfile` |
| MAJOR | D2 rejects the wasm alternative without a reason | Folded. W1 adopts it |
| MAJOR | Required CI cannot detect a traced symlink that escapes the tree | Folded (§ 8) |
| MINOR | The new corpus test cannot tell the two readers apart | Folded. A structure test replaces it (§ 6.3) |
| MINOR | The builder row is ambiguous for `organizationPrn` | Folded. One call per builder (§ 6.2) |
| MINOR | Windows reserved names apply to directories too | Folded (§ 10) |
| MINOR | `--build-context rs=rs` depends on the working directory | Folded. `rs="$ROOT/rs"` (§ 9) |
| MINOR | The `images.yml` filter list is out of date | Folded (§ 9) |
| MINOR | A8 does not check the new cargo call | Moot. No cargo call in `ts/Dockerfile` |
| MINOR | Console vitest configs may need the `.node` external rule | Moot. Measured: no config change for wasm (F11) |
| MINOR | `typecheck` does not need the binary | Moot. No task deps on a kernel build |
| MINOR | An SMA-662 record about `serverExternalPackages` becomes false | Moot. No Next config change |
| MINOR | `/healthz` does not load the kernel | Folded as a recorded limit (§ 13) |
| MINOR | The image smoke script is not defined | Moot. The smoke test is unchanged, and the Dockerfile checks the chunk (§ 9) |
| MINOR | A bash harness can hang on the development Mac | Folded. The gate is a vitest file in Node |
| MINOR | M4 (SBOM) may be unnecessary | Moot. No Rust in the console image |
| QUESTION | Can Dependabot resolve a `link:` outside `/ts`? | Moot. `file:` stays |
| QUESTION | Does PR 2 need an ADR update? | No. The kernel stays the one source of PRN behavior, and no Rust enters the image (D8) |
| QUESTION | The fallback if the named context ignores `rs/.dockerignore` | Folded. The `COPY` names two directories, so either answer is safe (M3) |
| QUESTION | Debug `.node` in e2e, release in the image | Moot. No `.node` in the consoles |
| QUESTION | Why does `@paigasus/wasm` move to `link:`? | Moot. `file:` stays |

### Round 2 (revision 2)

This section records the second challenge.
