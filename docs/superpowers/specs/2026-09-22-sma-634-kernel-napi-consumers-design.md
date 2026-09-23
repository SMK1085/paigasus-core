# SMA-634 — Node consumers load the kernel through its wasm entry

**Linear:** [SMA-634](https://linear.app/smaschek/issue/SMA-634/ts-paigasuskernel-napi-binding-never-reaches-node-modules-files)
**Related:** SMA-511 (spec § 4.7, D6), SMA-512, SMA-513 (spec F5)
**ADR:** ADR-0005 (cross-language behavior lives once in `paigasus-kernel`);
ADR-0022 (in-monorepo TypeScript consumers use the wasm binding, on the server too), written on
2026-09-22 for this decision
**Date:** 2026-09-22
**Revision:** 3, design W1 with a host-independent drift gate. Revision 1 (napi through `link:`)
was disproved by measurement. Revision 2 used a stamp file, which the second challenge disproved.
**Measurements:** `docs/superpowers/specs/2026-09-22-sma-634-measurements.md`. § 16 records the
two challenge rounds.

---

## 1. Problem

`@paigasus/kernel` imports `@paigasus/node-bindings` under the `node` export condition. That
package is a pnpm `file:` dependency on `rs/crates/bindings/paigasus-node-bindings`. pnpm installs
a `file:` target one time, at install time, and only with the paths in the `files` allowlist. The
allowlist is `["index.js", "index.d.ts"]`, so the `.node` binary never reaches `node_modules`.

A Next 16 app that imports the kernel from a server component fails `next build` with `Cannot find
native binding`. So SMA-511 added a hand-written PRN reader to the consoles, as a recorded
ADR-0005 exception. It now lives in `ts/packages/paigasus-console-core/src/prn-tenancy.ts`.

This design lets Node consumers in the monorepo load the kernel, and it removes the ADR-0005
exception.

---

## 2. Decision history

1. Revision 1 chose approach A: change both binding dependencies to `link:`, and use the napi
   entry in Next. The user approved it.
2. Challenge round 1 returned **NEEDS REWORK**. Its blocker: the tracing of a `.node` file outside
   `outputFileTracingRoot` was not measured, and two tool rules predicted a failure.
3. Spike S1 measured it (measurements § S1). The prediction was right. Turbopack does not resolve
   a symlink whose target is outside its root. The only napi route that works needs the repository
   root as the tracing root, a load that the bundler cannot see, and an explicit include. That
   moves the standalone entry point for every console build, e2e helper and image script.
4. The user chose **W1**: in-monorepo Node consumers use the kernel wasm entry, with a committed
   `.wasm` file and the current `file:` link.
5. Spike S2 measured W1 (measurements § S2). It builds, it runs from a copy outside the
   repository, and it needs no Next change and no vitest change.
6. Revision 2 held the committed artifact to a **stamp file**: a hash over the build inputs.
   Challenge round 2 returned **NEEDS REWORK** for it. The stamp holds absolute paths through
   `cargo tree`, so CI could never compute the value that the developer committed; and every
   kernel release PR would make the stamp stale with no repair path inside the release flow.
7. Spike S3 measured what a host-independent gate can compare (measurements § S3). The wasm glue,
   the import list and the export list are the same on macOS, Linux arm64 and Linux amd64. Only
   the binary bytes differ. Revision 3 drops the stamp and compares those three things instead.

---

## 3. As-built findings

Read from the tree at `8c1297b7`. F10 to F17 come from the spikes and are dated 2026-09-22.

### F1 — The published napi model is correct and does not change

The main `@paigasus/node-bindings` npm package is loader-only. Seven per-platform packages carry
the binaries (`prebuild.yml`, `release.yml`). `prebuild.yml` asserts that the main tarball holds
no `.node` file. The `files` allowlist is correct for this model. The defect is only that the
monorepo `file:` link cannot use this model.

### F2 — The wasm `files` allowlist already includes the binary

`rs/crates/bindings/paigasus-wasm/package.json` lists `paigasus_wasm_bg.wasm` in `files`. The
`.wasm` is gitignored, so it is absent at install time in CI. SMA-511 § 13 row 10 (B2) failed for
this reason only.

### F3 — `paigasus-kernel-ts:build` is the only writer of the committed wasm glue

The `build` task runs wasm-pack into `.wasmpack-out` and copies `paigasus_wasm*` into the crate
directory. The `test` task builds into `.wasmpack-test-out` and aliases vitest at that directory.
It does not write the crate directory. Both tasks also run `napi build`.

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
`console-core/package.json` did not change `moon project paigasus-console-core-ts` "Depends on",
with the Moon cache on and with `MOON_CACHE=off`. So a dependency without `dependsOn` changes no
project case in `ci/affected-graph/run.sh`.

### F8 — The console image install will reach `rs/`

`ts/Dockerfile` uses `ts/` as its build context and runs `pnpm install --frozen-lockfile --filter
"@paigasus/${APP}..."`. When `console-core` depends on `@paigasus/kernel`, that subgraph includes
the kernel's two `file:` dependencies at `../rs/crates/bindings/…`, which are outside the context.

### F9 — Next config

`createNextConfig()` lists `@paigasus/kernel` in `transpilePackages`, sets `output: 'standalone'`
and takes `outputFileTracingRoot` (`ts/`) from each app. It sets no `serverExternalPackages`.

### F10 — W1 builds and runs (S2-A)

With the `.wasm` present at install time, pnpm installs all five `@paigasus/wasm` files and
**hard-links** them to the crate directory. A fixture route that imports the kernel wasm entry
builds with `createNextConfig()` and the tracing root `ts/`. The standalone tree holds one `.wasm`
chunk, and no symlink leaves the tree. A copy outside the repository answers HTTP 200 with the
correct values. A bare Next config also works. Without the `.wasm` file, the build fails with
`Can't resolve './paigasus_wasm_bg.wasm'`.

### F11 — The console vitest config needs no change (S2-B)

The present `console-core` config loads the kernel wasm entry. vitest externalizes
`@paigasus/wasm`, and node 24 loads the `.wasm` natively. `vite-plugin-wasm` is not necessary.
Node prints an `ExperimentalWarning` on stderr.

### F12 — The `.wasm` bytes depend on the host (S3-1)

One host is deterministic: three macOS builds gave the same sha256. macOS, Linux arm64 and Linux
amd64 gave three different values, also with `--remap-path-prefix`. Without a remap, the macOS
binary holds absolute home paths. So no check may compare the binary byte for byte.

### F13 — The committed wasm glue is stale today (S3-4)

The committed `paigasus_wasm_bg.js` has `__wbg_Error_92b29b0548f8b746` (commit `0b2e346a`,
SMA-448). A current build gives `__wbg_Error_408e67f47ca7b58b`. The committed glue with a current
binary fails in node with a `LinkError`. The `build` task hides this, because it overwrites the
glue on every run.

### F14 — pnpm does not refresh a `file:` copy after an unlink (S2-C)

An in-place overwrite of a crate file changes the installed copy at once, through the hard link.
An unlink and replace (a delete, `git checkout`, a branch switch) breaks the link. A plain `pnpm
install`, and also `pnpm install --force`, then keeps the old copy. Only `rm -rf ts/node_modules`
followed by an install repairs it. CI installs into a new `node_modules`, so CI is not affected.

### F15 — The glue is host-independent (S3-1)

`paigasus_wasm.js`, `paigasus_wasm_bg.js`, `paigasus_wasm.d.ts` and `paigasus_wasm_bg.wasm.d.ts`
are byte-identical on the three hosts. This is what makes a comparison of the committed glue with
a fresh build possible in CI.

### F16 — A version bump changes the binary only (S3-2)

A version-only bump of the wasm crate changed `paigasus_wasm_bg.wasm` and none of the four glue
files. So a release PR changes no value that this design's gate compares.

### F17 — The import and export lists are host-independent (S3-3)

`WebAssembly.Module.imports()` and `.exports()` gave the same two imports and the same 21 exports
on the three hosts and after a version bump.

### F18 — The gate obligations of `ci/affected-graph/cargo_moon_parity.py`

- `FFI_MARKERS` (line 136) holds `napi build` and `wasm-pack`. A task whose resolved invocation
  holds one of them is an FFI task.
- A5 (`check_ffi_inputs`) demands `FFI_TASK_INPUTS` on every FFI task: `rs/Cargo.lock`,
  `rs/Cargo.toml`, `rs/rust-toolchain.toml`, `rs/.cargo/config.toml` and `.prototools`.
- A7 demands, for every FFI task, `{src}/src/**/*` and `{src}/Cargo.toml` of every Rust project in
  the project's `dependsOn` closure, plus `{src}/build.rs` when one exists on disk.
  `REQUIRED_WRAPPER_CLOSURE` pins that closure for `paigasus-kernel-ts` to `paigasus-kernel-rs`,
  `paigasus-node-bindings-rs` and `paigasus-wasm-rs`.
- A8 demands an `ALLOW_UNLOCKED_CARGO` entry, with a reason, for a task that reaches cargo through
  a wrapper. `paigasus-kernel-ts:build` and `:test` have one already.

### F19 — `ts:lint` and `ts:fmt` do not cover a `scripts/` directory

`ts/moon.yml`'s `sources` file group lists `packages/*/src/**/*`, `packages/*/testing/**/*`,
`apps/*/…` and `tooling/**/*`. Its `tests` group lists `packages/*/tests/**/*`. A new file under
`ts/packages/paigasus-kernel/scripts/` is in neither group, so an edit there would serve a cached
lint and format pass.

---

## 4. Decisions

| # | Topic | Decision |
| -- | -- | -- |
| D1 | The Node route | The kernel's `.` export resolves to the **wasm** entry under every condition. The napi entry moves to a new `./napi` subpath. So every consumer in the monorepo, now and later, gets a loadable binding by default, and the napi entry is reachable only on purpose |
| D2 | The link | Both kernel binding dependencies stay `file:` |
| D3 | The artifact | `paigasus_wasm_bg.wasm` is committed, together with the glue from the same build. The `build` and `test` tasks never write the committed files. A new task, `paigasus-kernel-ts:generate-wasm`, writes them from a script file |
| D4 | The drift gate | Four host-independent checks (§ 5.4): the committed glue equals a fresh build; the import and export lists of the committed binary equal a fresh build's; the committed pair replays the full parity corpus; the installed copy equals the committed files. **No stamp, and no comparison of the binary bytes** (F12, F15, F16, F17) |
| D5 | Delivery | One PR. The kernel commits come first, so a reviewer can read them alone |
| D6 | Next config | No change (F10) |
| D7 | The image | A named build context supplies the two binding directories for the install. The image compiles no Rust. The SMA-513 F5 statement "no Rust, no napi" stays true. "No wasm" becomes false: the image holds the bundled `.wasm` chunk |
| D8 | ADR-0005 | `prn-tenancy.ts` becomes an IAM adapter over the kernel. The exception is removed. The kernel stays the one source of PRN behavior. ADR-0022 (written 2026-09-22) records the binding choice: in-monorepo TypeScript consumers use wasm, on the server too. ADR-0005 keeps its napi clause for published consumers, and its implementation status links to ADR-0022 |

---

## 5. The kernel

### 5.1 The exports

`ts/packages/paigasus-kernel/package.json`:

```json
"exports": {
  ".": "./src/wasm.ts",
  "./napi": "./src/index.ts"
}
```

The wasm entry is platform-neutral and loads in Next, in vitest and in a browser bundle. The napi
entry loads only where the `.node` file is beside the loader, which today is the kernel's own
tests. A consumer that wants it must name `@paigasus/kernel/napi`.

The `_comment_exports` field is rewritten. It must **not** say "published consumers": this package
is `private: true` at version `0.0.0`, and its `_comment_publish` field records that publication is
double-blocked. A published npm consumer uses `@paigasus/node-bindings` directly, and that model
is correct and unchanged (F1).

This change touches four more files:

- `ts/packages/paigasus-kernel/vitest.config.ts`. The browser project's
  `'@paigasus/kernel' → src/wasm.ts` alias can go, because the `.` export is now the wasm entry
  under every condition. The five napi test files import `@paigasus/kernel`, which now resolves to
  the wasm entry, so they change to `@paigasus/kernel/napi`. The `@paigasus/node-bindings` alias
  to the crate directory stays: it is what makes a rebuilt `.node` visible (F14 applies to it too).
- `ts/packages/paigasus-kernel/tests/{sum,uuid7,prn-canonical,prn-fields,cedar}.test.ts`: the
  import line only. The five `*.wasm.test.ts` files do not change.
- `ts/packages/paigasus-kernel/tsconfig.json`. `customConditions: ["node"]` exists so that tsc
  resolves `.` to the napi entry. Both entry files are inside `include: ["src/**/*"]`, so both are
  type-checked without it. The plan removes the setting and rewrites the comment, or keeps it with
  a corrected comment if a measured reason appears. Either way the comment must match the new map.
- `ts/packages/paigasus-kernel/src/binding-parity.types.ts` does not change. It imports the two
  binding packages directly, not through the kernel's own exports, so the parity guard still
  compares the napi surface with the wasm surface.

### 5.2 The committed artifacts

- `rs/crates/bindings/paigasus-wasm/.gitignore` keeps `*.wasm` and adds `!/paigasus_wasm_bg.wasm`.
  The leading slash anchors the negation at the crate root, so a same-named file in a future
  sub-directory stays ignored. The header comment is rewritten: the binary and the glue are now
  both committed, and `generate-wasm` is their only writer.
- The PR commits `paigasus_wasm_bg.wasm`, `paigasus_wasm_bg.js`, `paigasus_wasm.js`,
  `paigasus_wasm.d.ts` and `paigasus_wasm_bg.wasm.d.ts` from one `generate-wasm` run. This also
  repairs the stale glue (F13).
- A new `.gitattributes` at the repository root (none exists today):
  `rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm binary` and `linguist-generated=true`
  for the four glue files. The `binary` attribute keeps git from a diff attempt and makes the
  Windows checkout safe without the NUL heuristic. `prebuild.yml` checks the tree out on
  `windows-latest`, so this matters.
- **A rebase or merge conflict in these five files** is never resolved by hand. Take either side,
  then run `generate-wasm` and commit its result. The gate then proves that the five files agree
  with the source.

### 5.3 `generate-wasm`, and the unchanged `build` inputs

**`paigasus-kernel-ts:generate-wasm`** is a new Moon task with `options.runInCI: false` and
`options.cache: false`. It is not in `ci.yml`'s `T=(…)` array, because CI never runs it. Its
`script:` holds the literal `wasm-pack` call, so `cargo_moon_parity.py` still derives it as an FFI
task (F18). A wrapper script that hides the call would make it invisible to A5, A7 and A8.

The task's `script:` block holds **three** steps, in this order, so that the literal `wasm-pack`
call stays visible to the marker scan while the logic stays in a reviewable file:

```
node scripts/generate-wasm.mjs --pre
( cd ../../../rs/crates/bindings/paigasus-wasm && wasm-pack build . --target bundler --release \
    --no-pack --out-dir .wasmpack-out --out-name paigasus_wasm \
    -- --config 'target.wasm32-unknown-unknown.rustflags=[…]' --locked )
node scripts/generate-wasm.mjs --post
```

`--pre`:

1. Fails when `RUSTFLAGS` or `CARGO_ENCODED_RUSTFLAGS` is set, and says to unset it. cargo's
   rustflags sources are mutually exclusive, so an exported value would silently drop the remap.
   The remap travels as `--config`, not as an environment variable, because a `--config` value
   survives a path that holds a space.
2. Asserts that `wasm-pack --version` equals the `.prototools` pin.
3. `touch`es `rs/crates/libs/paigasus-kernel/src/lib.rs` and
   `rs/crates/bindings/paigasus-wasm/src/lib.rs`. cargo's freshness is mtime-based, and a git
   operation can leave a source older than an artifact; the `build` and `test` tasks carry the same
   `touch` for the measured `sum(2,3) → 6` failure.

`--post`:

4. Rejects the result when the new binary holds `/Users/` or `/home/`. The search is for both, not
   for `$HOME` alone, because `CARGO_HOME` can be outside the home directory.
5. Copies the five files into the crate directory with an in-place overwrite, so that the
   hard-linked installed copy changes too (F14), and then removes `.wasmpack-out`.

The task has **no** early exit on an "unchanged" condition. Revision 2 had one, keyed on the
stamp; without a stamp there is nothing to compare, because the binary bytes cannot decide it
(F12). So the task always rebuilds, and it needs no `--force` flag.

`ts/moon.yml`'s `sources` file group gains `packages/*/scripts/**/*`, so that `ts:lint` and
`ts:fmt` key on the new script (F19).

**`paigasus-kernel-ts:build`** stops running wasm-pack and stops copying the glue. It keeps `napi
build` and `tsc`, and it drops `/rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm` from its
`outputs`. **Its `/rs/crates/bindings/paigasus-wasm/**` inputs stay.** The task still matches
`napi build`, so it stays an FFI task, and A7 demands the closure inputs of `paigasus-wasm-rs`
(F18). Its `tsc` also reads the committed `paigasus_wasm.d.ts` through the parity guard.

**`paigasus-kernel-ts:test`** keeps its wasm-pack build into `.wasmpack-test-out`. That fresh build
is what the gate compares against, so it is now load-bearing twice.

### 5.4 The drift gate

The gate is a new vitest file, `ts/packages/paigasus-kernel/tests/committed-wasm.test.ts`, in the
`node` project of `paigasus-kernel-ts:test`, plus a helper that node runs in a child process,
`ts/packages/paigasus-kernel/tests/wasm-probe.mjs`. Both are under `tests/`, which `ts:lint`
already covers, and the test file must be added to the `node` project's explicit `include` list or
it never runs.

**Why a kernel test and not a `repo:*` gate.** A new `repo:*` gate has seven registration
obligations (`ci/CLAUDE.md`), including `ci.yml`'s `T=(…)` array and the root `CLAUDE.md`
`ci-targets` block. `paigasus-kernel-ts:test` is already in the CI graph, it already keys on every
Rust input of the wasm build, and it already produces the fresh build that checks 1 and 2 need.

**Why a child process for checks 2 and 3.** The `node` vitest project has no wasm plugin, and its
`server.deps.external` rule names `.node` files only. An import of the committed
`paigasus_wasm.js` through vitest would meet Vite's own wasm handling, not the loader that a
console uses. `wasm-probe.mjs`, run with `node`, is the same path as a console server (F11).

The four checks:

1. **Glue equality.** The four committed glue files are compared byte for byte with
   `.wasmpack-test-out`'s. They are host-independent (F15), so this is valid in CI. A failure
   names `moon run paigasus-kernel-ts:generate-wasm` and the five files to commit.
2. **Interface equality.** `wasm-probe.mjs` reads `WebAssembly.Module.imports()` and `.exports()`
   of the committed binary and of `.wasmpack-test-out`'s, sorts them by name and kind, and
   compares. The lists are host-independent and survive a version bump (F16, F17).
3. **Behavior.** `wasm-probe.mjs` instantiates the **committed** glue with the **committed**
   binary and replays every vector of all five corpus files that `tests/corpus.ts` loads: `sum`,
   `uuid7`, `prn_canonical`, `prn_cedar` and `prn_fields`. A glue and binary mismatch fails here
   with a `LinkError` (F13), and a stale binary fails on the vector that changed.
4. **The installed copy.** This check does **not** live in the kernel task. Moon's hasher ignores
   `node_modules` (`.moon/workspace.yml`), so a cached pass would replay while the installed copy
   is another branch's. It lives in the `setupFiles` of the `console-core` and app vitest configs,
   which is where a stale copy changes a result: it compares the sha256 of the installed
   `paigasus_wasm_bg.wasm` and `paigasus_wasm_bg.js` with the committed files and fails with `rm
   -rf ts/node_modules && pnpm -C ts install` (F14). In CI it always passes.

New `test` task inputs: the five committed wasm files.

**What the gate does not prove.** A change in the Rust kernel that alters behavior but changes no
glue file, no export name and no corpus vector is not detected, and the consoles would ship the old
binary. The bound on that residual is `repo:parity-corpus-drift`, which regenerates the vectors
from the kernel and fails on any diff: a behavior change that the generator emits must change a
vector, and check 3 then reds. A behavior change outside the generator's output is the residual.
This is written down because it is what the stamp of revision 2 was meant to cover, and the stamp
could not be made to work (§ 2 step 6).

**Negative controls (red-first for the gate).** Each control isolates one check:

| Control | Must red | Must stay green |
| -- | -- | -- |
| Restore the glue of commit `0b2e346a` over the committed glue | 1 | – |
| Rename one exported kernel function in Rust, regenerate, and copy **only** the new binary over the committed one | 2 | 1 |
| Change one error-kind string in Rust, regenerate, and copy **only** the new binary | 3 | 1, 2 |
| Change one byte of the installed copy only | 4 | 1, 2, 3 |

Restore after each control with `git checkout --` of the five artifact paths only, never of the
tree, and then `rm -rf ts/node_modules && pnpm -C ts install`. These five files are generated, so a
checkout of them discards no hand-written work.

### 5.5 Kernel records

- `ts/packages/paigasus-kernel/moon.yml`: the comments on `build` and `test` change to match
  § 5.3 and § 5.4. The `build` comment must no longer describe wasm-pack or `.wasmpack-out`.
- `rs/crates/bindings/paigasus-wasm/.gitignore`: the header comment (§ 5.2).
- `.github/workflows/prebuild.yml` (the comment that says the `build` task owns `.wasmpack-out`)
  and `.github/workflows/ci.yml` (the comment that says `build` and `test` both call wasm-pack
  concurrently). The serial `rustup target add` step stays: `test` still calls wasm-pack.

---

## 6. `console-core` uses the kernel

### 6.1 The dependency

`ts/packages/paigasus-console-core/package.json` gets `"@paigasus/kernel": "workspace:*"` in
`dependencies`. `moon.yml` gets no `dependsOn` edge and no task `deps` (§ 7).

### 6.2 `prn-tenancy.ts` becomes an IAM adapter

The file imports `prnBuild`, `prnErrorKind`, `prnService`, `prnRegion`, `prnOrg`,
`prnResourceType` and `prnResourceId` from `@paigasus/kernel`. The exports, the `null` and
`TypeError` behavior and the lower-case ids do not change. No caller in the two apps changes.

| Export | New source |
| -- | -- |
| `parseTenancyPrn(prn)` | Reject an over-long input first (see below). If `prnErrorKind(prn) !== ''`, return `null`. Otherwise read the five fields. Then apply the IAM rule without change: service `iam`; an empty region; resource type `organization`, `team` or `project`; `prnOrg` is `''` for an organization and a UUID for a team or a project. For an organization, `orgId` is the resource id. Ids are lower-case |
| `organizationPrn(orgId)` | `requireUuid('orgId', orgId)`, then `prnBuild('iam', '', '', 'organization', orgId)` |
| `teamPrn(orgId, teamId)` | `requireUuid` on both, then `prnBuild('iam', '', orgId, 'team', teamId)` |
| `projectPrn(orgId, projectId)` | `requireUuid` on both, then `prnBuild('iam', '', orgId, 'project', projectId)` |
| `ROOT_PRN` | Stays a constant. The test against the live Rust `root_prn()` stays |
| `isUuid` | Stays a local regex. It guards the shape of a URL segment. It is not PRN grammar (F5) |

The file removes the hand-written field grammar. It **keeps a length guard** before the first
kernel call. This is a resource limit, not grammar: an unbounded string crosses into wasm linear
memory, which never shrinks. The guard's comment says so, so that a later reader does not remove
it as a duplicate of the kernel's own 512-byte rule.

The header comment states that the file holds the IAM tenancy rule over the kernel grammar, and
that it is not an ADR-0005 exception.

A non-empty region stays a `null` result. This is IAM policy: `TenancyRef` has no region field.

`prnBuild` can throw for an input that `requireUuid` accepts only if the kernel grammar changes.
The builders do not catch that error.

### 6.3 Tests

`tests/unit/prn-tenancy.test.ts` keeps the corpus replay, the builder round trips, the `ROOT_PRN`
checks and the negative table. They now test the adapter.

A new **delegation test** proves that the kernel does the parsing. The old reader satisfies a
structure test (its only regex is the UUID one, and its grammar is `split(':')` and `indexOf('/')`),
so a text check alone proves nothing. The delegation test uses `vi.mock('@paigasus/kernel')`:

- `prnErrorKind` returns a non-empty kind for a PRN that is valid and of a tenancy shape. Then
  `parseTenancyPrn` must return `null`. A hand-written grammar would return a `TenancyRef`.
- `prnOrg` and `prnResourceId` return marked values. Those values must appear in the result.
- Each builder is called, and the test asserts the exact five arguments that `prnBuild` receives.

A text check stays beside it, but it bans `split(` and `indexOf(` in `src/prn-tenancy.ts` rather
than counting regex literals.

The consoles' unit, integration and e2e tests run against the real kernel and must pass without
change. The e2e tiers open org, team and project pages, which call `parseTenancyPrn` and the
builders.

### 6.4 No ESLint rule

Revision 2 added a rule that banned the bare `@paigasus/kernel` import in the console packages, so
that nobody would reach the napi entry in a Next build. D1 removes the reason for it: the bare
import **is** the wasm entry now, and the napi entry needs an explicit `/napi` subpath. A rule
would also cost more than it looks: a new `no-restricted-imports` block that matches the same
files replaces an existing block, `patterns` match with gitignore semantics that reach a subpath,
and a new `files` scope needs a `BOUNDARY_SCOPES` entry.

---

## 7. Moon wiring

- `console-core`'s `build`, `typecheck` and `test`, and the `build`, `typecheck`, `test` and
  `test-e2e` tasks of both apps, get these inputs:
  - `/ts/packages/paigasus-kernel/src/**/*`
  - `/ts/packages/paigasus-kernel/package.json`
  - `/rs/crates/bindings/paigasus-wasm/paigasus_wasm.js`,
    `…/paigasus_wasm_bg.js`, `…/paigasus_wasm_bg.wasm`, `…/paigasus_wasm.d.ts`,
    `…/paigasus_wasm_bg.wasm.d.ts` (the five files by name, not a `paigasus_wasm*` glob)
  - `/rs/crates/bindings/paigasus-wasm/package.json`
  `console-core`'s `build` runs the same `tsc` as `typecheck` and shares the `&upstreams` anchor,
  so it needs the inputs as much as `typecheck` does.
- No task gets `deps` on a kernel task, and no project gets a `dependsOn` edge. The consoles read
  committed files, so they need no Rust build and no Rust toolchain.
- `generate-wasm` runs wasm-pack, so `cargo_moon_parity.py` derives it as an FFI task (F18). It
  must declare, although it is `cache: false`:
  - A5: `/rs/Cargo.lock`, `/rs/Cargo.toml`, `/rs/rust-toolchain.toml`, `/rs/.cargo/config.toml`,
    `/.prototools`;
  - A7: `src/**/*` and `Cargo.toml` of `paigasus-kernel-rs`, `paigasus-node-bindings-rs` and
    `paigasus-wasm-rs`, plus `paigasus-node-bindings/build.rs`;
  - A8: an `ALLOW_UNLOCKED_CARGO` entry in `ci/affected-graph/cargo_moon_parity.py`. Its reason is
    the measured SMA-601 fact that `paigasus-kernel-ts:build` already records: wasm-pack makes its
    own unlocked cargo call before the build that `-- --locked` reaches.
- F7 shows that no project case in `ci/affected-graph/run.sh` changes. The plan runs
  `repo:affected-smoke` under system `/bin/bash` 3.2 (M4) and re-baselines a case only if the run
  shows a change.
- `ts/apps/iam-console/moon.yml`'s `dependsOn` comment says that every package the app compiles is
  listed. The app now compiles `@paigasus/kernel` through `console-core` without an edge, so the
  comment is corrected to name the inputs rule instead. `gateway-console` carries the same comment.

---

## 8. Required-CI proof in the console builds

Each app's Moon `build` task already checks `.next/standalone/apps/<app>/server.js`. Beside that
check it asserts that a **regular file** whose name holds `paigasus_wasm_bg` and ends in `.wasm`
exists under `.next/standalone/apps/<app>/.next/server/chunks/`. `find -type f`, without `-L`, and
no pipe into an early-exit reader: `repo:actionlint` check 13 scans `moon.yml` scripts, and the
existing build script is written in that style already.

So `moon ci` fails if Turbopack stops bundling the wasm. The check does not depend on `images.yml`,
which is not a required check.

Revision 2 also proposed a symlink check. It is dropped: W1 bundles the wasm into a chunk, so no
symlink is expected, nothing produces one, and a check that can never fire hides its own faults.

---

## 9. The console image

- `ci/images/run.sh` `build_console_one` adds `--build-context bindings="$ROOT/rs/crates/bindings"`.
  The narrow context is deliberate: a context rooted at `rs/` would need an edit of
  `rs/.dockerignore` (which excludes `**/*.wasm`) and could upload `rs/target/`. A sub-directory
  context carries no `.dockerignore` of its own, so neither question arises. A missing flag fails
  in a recognizable way: `COPY --from=bindings` then reads `bindings` as an image reference.
  The comment above the function stays correct — no build **stage** is shared between the two
  console builds — but it gains one line about the new context.
- `ts/Dockerfile`, in the builder stage before `pnpm install`:
  `COPY --from=bindings paigasus-wasm /rs/crates/bindings/paigasus-wasm` and the same for
  `paigasus-node-bindings`. With `WORKDIR /build` as `ts/`, the `file:` paths resolve to
  `/rs/crates/bindings/…`. No Rust is compiled. The napi directory is necessary only because the
  kernel declares the dependency; the image never loads it.
- `ts/Dockerfile` adds the same regular-file `.wasm` chunk check as § 8, after the `server.js`
  check.
- The console smoke in `ci/images/run.sh` gains one probe: a request to one `(console)` route that
  must answer a redirect, and must not answer 500. Module evaluation happens before the session
  redirect, so a wasm that fails to load gives 500 there. The present smoke fetches `${base_path}`
  only, which renders the public page and imports no console code, so today nothing in the image
  path would load the wasm.
- `.github/workflows/images.yml`'s `pull_request` filter gains
  `ts/packages/paigasus-kernel/package.json` and nothing else. `docs/ops/RUNBOOK-containers.md`
  states the rule: a file belongs on the filter when a change to it can break the image build **but
  not** the ordinary build. The kernel's `file:` dependency list is such a file, because the
  Dockerfile's `COPY` list is hand-maintained and only the Docker path reads it. The committed wasm
  files and the Rust sources are **not** added: § 7's inputs already make `moon ci` re-run
  `next build` for both consoles on such a change, and `rs/crates/bindings/paigasus-wasm/**` would
  also put two cold `--release` service builds on every wasm PR. The filter list is restated in
  `docs/ops/RUNBOOK-containers.md` and `rs/CLAUDE.md`, and both are updated with it.

---

## 10. Developer workflow and traps

- **After an edit to the Rust kernel or the wasm binding:** run `moon run
  paigasus-kernel-ts:generate-wasm`, then commit the five wasm files. Check 1, 2 or 3 reds until
  the committed files agree with the source.
- **After a branch switch, a rebase or a `git checkout` that changes the committed wasm files:**
  run `rm -rf ts/node_modules && pnpm -C ts install`. A plain `pnpm install` does not refresh the
  installed copy (F14). Check 4, in the console test setup, tells you when this is necessary.
- **A conflict in the five artifacts on a rebase:** take either side, then run `generate-wasm` and
  commit its result (§ 5.2).
- **Only one host needs to run `generate-wasm`.** A second host makes different bytes (F12) and a
  diff that says nothing. Do not regenerate to "fix" a diff you did not cause.
- **A Dependabot bump inside the wasm dependency closure** changes the glue only when the FFI
  surface or the wasm-bindgen version changes. Check 1 reds then, and someone runs `generate-wasm`
  on that branch. A bump that changes only the binary bytes reds nothing, which is the residual in
  § 5.4.
- **vitest stderr** in `console-core` and the apps shows node's `ExperimentalWarning` for the wasm
  import (F11). It is harmless. Do not read it as a failure.
- **Windows reserved names** apply to every path component, directories included. No new file or
  directory here uses one (`committed-wasm.test.ts`, `wasm-probe.mjs`, `generate-wasm.mjs`).
- **The root `.gitignore` `build/` rule** ignores any directory named `build/`. No new path here
  uses one.
- Every new source file opens with an SPDX header: `// SPDX-License-Identifier: Apache-2.0`.

---

## 11. Error handling

Every new check fails closed.

- A glue mismatch, an interface mismatch, a corpus failure or a stale installed copy fails a test,
  with a message that names the command to run.
- `generate-wasm` stops when a rustflags variable is set, when the wasm-pack version is not the
  pinned one, or when the new binary holds an absolute home path.
- A missing `.wasm` chunk fails the console `build` task and the image build.
- A kernel error in `parseTenancyPrn` becomes `null`. A bad id in a builder becomes a `TypeError`.
  This is the current contract.

---

## 12. Measurements to take in the plan

| # | What | Rule |
| -- | -- | -- |
| M1 | `generate-wasm` twice on this host: are the five files equal, and does the home-path search find nothing? | The glue must be equal. If the binary differs between two runs on one host, stop and report: F12 measured that it does not |
| M2 | The runtime of the four checks, and of the `test` task as a whole, before and after | Record it in the PR |
| M3 | The four negative controls of § 5.4, each with its "must stay green" column | Each must red the named check only, and all must pass after the restore |
| M4 | `repo:affected-smoke` under system `/bin/bash` 3.2 after the Moon changes, and `repo:input-liveness` | Re-baseline a case only if the run shows a change, and record why. `input-liveness` fails a declared input that matches no tracked file, so the five artifact inputs must exist first |
| M5 | The console image builds on amd64 in the PR, and on arm64 through `gh workflow run images.yml --ref <branch>`, and the new `(console)` probe answers a redirect | Required before the merge |
| M6 | The size of the `bindings` build context that BuildKit uploads | Record it. It must hold no `target/` directory |

---

## 13. Recorded limits

- The committed `.wasm` bytes depend on the host that made them (F12). The gate proves the glue,
  the interface and the behavior, not the bytes. § 5.4 records the residual and its bound.
- The provenance of the binary is one developer's machine. A reviewer cannot read the bytes.
  Checks 2 and 3 are what make an unreviewed change to the module visible.
- `/healthz` does not load the kernel. A container whose wasm chunk fails to load reports healthy.
  Readiness stays config-only. The new `(console)` probe in the image smoke (§ 9) and the build
  check (§ 8) are what make this failure visible instead.
- A wasm load failure is not limited to the PRN pages: `console-core/src/index.ts` re-exports
  `prn-tenancy`, the `(console)` layout imports the package, and `@paigasus/wasm` declares
  `paigasus_wasm.js` side-effectful. So such a failure returns 500 for every `(console)` page.
- The napi entry stays unusable in a Next build. It is reachable at `@paigasus/kernel/napi` for the
  kernel's own tests. Published npm consumers of `@paigasus/node-bindings` are not affected (F1).
- `isUuid`, `ROOT_PRN`, the length guard and the IAM tenancy rule stay hand-written. They are IAM
  domain rules or resource limits, not kernel grammar.
- The wasm route in a Next **client** bundle stays unproven. No client consumer exists.
- This spec supersedes the SMA-511 § 12 D6 limit and the "no wasm" part of SMA-513 F5. Those specs
  are historical records, and this PR does not edit them.

---

## 14. Files touched (one PR)

**Kernel commits**

- `ts/packages/paigasus-kernel/package.json` (the exports map and the comments)
- `ts/packages/paigasus-kernel/moon.yml` (`generate-wasm`, the changed `build`, the `test` inputs)
- `ts/packages/paigasus-kernel/vitest.config.ts`, `tsconfig.json`
- `ts/packages/paigasus-kernel/tests/{sum,uuid7,prn-canonical,prn-fields,cedar}.test.ts` (the
  import line)
- `ts/packages/paigasus-kernel/tests/committed-wasm.test.ts` and `tests/wasm-probe.mjs` (new)
- `ts/packages/paigasus-kernel/scripts/generate-wasm.mjs` (new)
- `ts/moon.yml` (the `sources` file group)
- `rs/crates/bindings/paigasus-wasm/.gitignore`
- `rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm` (new), `paigasus_wasm_bg.js`,
  `paigasus_wasm.js`, `paigasus_wasm.d.ts`, `paigasus_wasm_bg.wasm.d.ts`
- `.gitattributes` (new)
- `ci/affected-graph/cargo_moon_parity.py` (the `ALLOW_UNLOCKED_CARGO` entry)

**Console commits**

- `ts/packages/paigasus-console-core/package.json`, `moon.yml`, `src/prn-tenancy.ts`,
  `vitest.config.ts` (the setup file), `tests/unit/prn-tenancy.test.ts`, a new delegation test
- `ts/apps/iam-console/moon.yml`, `ts/apps/gateway-console/moon.yml`, and both apps'
  `vitest.config.ts`
- `ts/pnpm-lock.yaml`

**Image commits**

- `ts/Dockerfile`, `ci/images/run.sh`, `.github/workflows/images.yml`

**Records**

- `ts/CLAUDE.md`: replace the SMA-634 entry and the "fallback C" entry with the W1 facts (§ 10)
- `.github/workflows/prebuild.yml` and `.github/workflows/ci.yml` (the wasm-pack comments, § 5.5)
- `docs/ops/RUNBOOK-containers.md` and `rs/CLAUDE.md` (the `images.yml` filter list, § 9)
- `ci/affected-graph/run.sh` and `README.md` only if M4 shows a change

---

## 15. Acceptance

- `@paigasus/kernel` resolves to the wasm entry in Next and in vitest, and `@paigasus/kernel/napi`
  resolves for the kernel's own napi tests.
- `prn-tenancy.ts` delegates the PRN grammar to the kernel. The delegation test proves it, and the
  text check bans the old grammar calls.
- The committed glue equals a fresh build; the interface lists match; the committed pair replays
  all five corpus files.
- The four negative controls each red their own check and leave the others green.
- Both console builds contain a regular `.wasm` chunk.
- The consoles' unit, integration and e2e tiers pass.
- Both console images build on amd64 and arm64 without a Rust compile, and the new `(console)`
  probe answers a redirect.
- The published npm model is unchanged: `prebuild.yml`'s loader-only assertion still passes.
- The full gate graph in the root `CLAUDE.md` passes. The bash-split gates are re-run with the
  correct bash, as the root `CLAUDE.md` describes.

---

## 16. Challenge log

### Round 1 (revision 1, verdict NEEDS REWORK)

| Severity | Finding | Disposition |
| -- | -- | -- |
| BLOCKER | D1 rests on an unmeasured tracing premise | Folded. Measured in S1. The premise was false, and the design changed to W1 |
| MAJOR | Affected-graph cases need a re-baseline for a new `dependsOn` | **Corrected in round 2.** W1 adds no `dependsOn` (F7), so no PROJECT case changes. But `generate-wasm` is an FFI task, so A5, A7 and A8 apply to it (§ 7, F18). Revision 2 called this "moot", which was wrong |
| MAJOR | Console tasks need `inputs` on the kernel, not only `deps` | Folded (§ 7) |
| MAJOR | The red-first run never reaches steps 3–5 | Folded. The gate has four isolated negative controls (§ 5.4), and § 8 checks the real builds |
| MAJOR | The fixture collides with kernel globs and needs dependencies | Moot. No fixture. The consoles are the consumer proof |
| MAJOR | Two kernel tasks can rewrite the shared `.node` | Moot for W1. Also measured: `napi build` replaces the file by rename |
| MAJOR | Dependabot proposes Rust bumps for `ts/Dockerfile` | Moot. No Rust stage in `ts/Dockerfile` |
| MAJOR | D2 rejects the wasm alternative without a reason | Folded. W1 adopts it |
| MAJOR | Required CI cannot detect a tracing regression | Folded (§ 8) |
| MINOR | The new corpus test cannot tell the two readers apart | **Corrected in round 2.** A structure test has the same defect. A delegation test replaces it (§ 6.3) |
| MINOR | The builder row is ambiguous for `organizationPrn` | Folded. One call per builder (§ 6.2) |
| MINOR | Windows reserved names apply to directories too | Folded (§ 10) |
| MINOR | `--build-context rs=rs` depends on the working directory | Folded, then narrowed: `bindings="$ROOT/rs/crates/bindings"` (§ 9) |
| MINOR | The `images.yml` filter list is out of date | **Corrected in round 2.** The RUNBOOK rule allows one addition only (§ 9) |
| MINOR | A8 does not check the new cargo call | **Corrected in round 2.** There is no cargo call in `ts/Dockerfile`, but `generate-wasm` needs an A8 entry (§ 7). Revision 2 called this "moot", which was wrong |
| MINOR | Console vitest configs may need the `.node` external rule | Moot. Measured: no config change for wasm (F11) |
| MINOR | `typecheck` does not need the binary | Folded. No task deps on a kernel build, so no contributor needs Rust for a console task |
| MINOR | An SMA-662 record about `serverExternalPackages` becomes false | Moot. No Next config change |
| MINOR | `/healthz` does not load the kernel | Folded as a recorded limit, with the new `(console)` probe (§ 9, § 13) |
| MINOR | The image smoke script is not defined | Folded. The probe is defined in § 9 |
| MINOR | A bash harness can hang on the development Mac | Folded. The gate is a vitest file and a node child process |
| MINOR | M4 (SBOM) may be unnecessary | Moot. No Rust in the console image |
| QUESTION | Can Dependabot resolve a `link:` outside `/ts`? | Moot. `file:` stays |
| QUESTION | Does PR 2 need an ADR update? | No. The kernel stays the one source of PRN behavior, and no Rust enters the image (D8) |
| QUESTION | The fallback if the named context ignores `rs/.dockerignore` | Folded. The narrow context removes the question (§ 9) |
| QUESTION | Debug `.node` in e2e, release in the image | Moot. No `.node` in the consoles |
| QUESTION | Why does `@paigasus/wasm` move to `link:`? | Moot. `file:` stays |

### Round 2 (revision 2, verdict NEEDS REWORK)

| Severity | Finding | Disposition |
| -- | -- | -- |
| BLOCKER | The stamp is not host-independent, so check 1 reds in CI | Folded by removal. No stamp (D4). S3 measured what can be compared instead |
| BLOCKER | Every kernel release PR reds the stamp with no repair path | Folded by removal. S3-2 measured that a version bump changes only the binary bytes, which no check compares (F16) |
| MAJOR | The Moon changes red A5, A7 and A8, and two round-1 dispositions were wrong | Folded (§ 7, F18). The `build` task keeps its wasm inputs, `generate-wasm` declares the FFI and closure inputs and gets an A8 entry, and both dispositions are corrected above |
| MAJOR | `generate-wasm` can commit a stale binary under a fresh stamp | Folded (§ 5.3): a script file, a `touch` first, no early exit at all, the rustflags guard, the `--config` form and the version assertion |
| MAJOR | The structure test cannot tell the two readers apart | Folded. A delegation test (§ 6.3) |
| MAJOR | The stamp is over-inclusive and will red on unrelated PRs | Folded by removal |
| MAJOR | A simpler gate exists and was not evaluated | Folded. It is the design now (§ 5.4), with its residual written down |
| MINOR | Checks 2 and 3 name an unmeasured loading mechanism | Folded. A child node process, and the test file must join the explicit `include` list (§ 5.4) |
| MINOR | § 5.4 replays two corpora, § 15 says "the full corpus" | Folded. All five files that `tests/corpus.ts` loads |
| MINOR | The ESLint ban has two known traps in this repo | Folded by removal. D1 removes the reason for the rule (§ 6.4) |
| MINOR | § 7's input list has gaps | Folded. `console-core:build`, and the script directory joins the `ts:lint` file group (§ 5.3, F19) |
| MINOR | Stamp completeness nits | Moot. No stamp |
| MINOR | The Docker named context is assumed; a narrower one avoids the question | Folded (§ 9) |
| MINOR | The `images.yml` additions break the workflow's own rule | Folded. One addition, with the rule quoted (§ 9) |
| MINOR | No `.gitattributes` exists for a tracked binary | Folded (§ 5.2) |
| MINOR | § 8's symlink check can never fire | Folded by removal (§ 8) |
| MINOR | Check 4 is defeated by the Moon cache | Folded. It moves into the console vitest setup files (§ 5.4) |
| MINOR | § 13 understates the failure radius, and the smoke never loads the wasm | Folded (§ 9, § 13) |
| MINOR | `_comment_exports` would record a false reason | Folded. D1 moves napi to `./napi`, and the comment must not say "published consumers" (§ 5.1) |
| MINOR | Removing `MAX_LEN` sends unbounded input across the FFI boundary | Folded. The length guard stays, as a resource limit (§ 6.2) |
| MINOR | The measurement evidence is in a session scratchpad | Folded. `docs/superpowers/specs/2026-09-22-sma-634-measurements.md` |
| MINOR | Comment drift is not listed | Folded (§ 5.5, § 7, § 14) |
| MINOR | The provenance of the committed binary is not recorded | Folded. Checks 2 and 3 are the control, and § 13 records the limit |
| QUESTION | Is the whole glue host-independent? | Measured: yes, all four files (F15) |
| QUESTION | Does a version bump change the bytes and the glue? | Measured: the binary only (F16) |
| QUESTION | Will the failure message print the computed hash? | Moot. No hash |
| QUESTION | Which other in-monorepo Node consumers import the kernel? | Answered by D1: the default entry is now the wasm entry, so a future consumer is not affected |
| QUESTION | Does the Linear issue accept that the napi defect stays? | Open for the human at the approval gate. The issue's scope sentence allows it ("or record that Node consumers use the wasm entry and export it"), and § 13 records it |
| QUESTION | What if BuildKit ignores `rs/.dockerignore` for the named context? | Moot. The narrow context (§ 9) |
