# SMA-435: pin binaryen (`wasm-opt`) through proto, and optimize the wasm kernel

- Linear: SMA-435. Related: SMA-427 (L3, the `wasm-opt = false` line), SMA-375 (proto pinning),
  SMA-634 (the five committed artifacts and the drift gate).
- Status: approved in chat on 2026-10-04 (design). Written spec waits for review.
- History: a first draft from 2026-09-27 was lost with its session scratchpad. This spec starts
  again from new measurements (§1.1). It keeps four ideas of that draft: the unit tests with a
  stub `wasm-opt`, the mutation battery, the Linux container measurement and the
  `workflow_dispatch` run.

## 1. Problem

SMA-427 set `wasm-opt = false` in `rs/crates/bindings/paigasus-wasm/Cargo.toml`
(`[package.metadata.wasm-pack.profile.release]`). The reason: with optimization on, wasm-pack
downloads a binaryen release that no file in this repository pins. That goes against the
proto-pinning rule (SMA-375).

The kernel binary is no longer a placeholder. `paigasus_wasm_bg.wasm` is 50,902 bytes with 23
exports (13 kernel functions). It is committed (SMA-634), and `prebuild.yml` builds a release copy
for `@paigasus/wasm`. Both copies are not optimized.

### 1.1 Measured facts (2026-10-04, macOS arm64, proto 0.61.1)

| # | Fact | How measured |
|---|---|---|
| F1 | binaryen's latest release is `version_133` (2026-09-21). Assets: `binaryen-version_133-{x86_64,aarch64}-linux.tar.gz`, `-{arm64,x86_64}-macos.tar.gz`, `-{x86_64,arm64}-windows.tar.gz`, each with a `.sha256` file. | `gh api repos/WebAssembly/binaryen/releases/latest` |
| F2 | The Linux tarballs are about 111 MB. Of that, 66.5 MB is `lib/libbinaryen.a`, which `wasm-opt` does not need at run time. `bin/wasm-opt` is 18.7 MB. The macOS arm64 tarball is 7.8 MB and installs to 25 MB. | asset sizes; `tar -tzv` on the x86_64-linux tarball |
| F3 | A proto TOML plugin installs it. `version-pattern = "^version_(?<major>\\d+)$"` maps `version_133` to `133.0.0`, and `{versionMajor}` builds the file name back. `proto versions wasm-opt` lists `1.0.0` to `133.0.0`. | isolated `PROTO_HOME`, `proto install wasm-opt` |
| F4 | proto verifies the `.sha256`. With the wrong checksum file, the install fails with `proto::install::invalid_checksum` (rc 1). | negative control |
| F5 | On macOS, `bin/wasm-opt` links `@rpath/libbinaryen.dylib`. The tarball holds `lib/libbinaryen.dylib`, and proto keeps the whole tree, so the binary runs. `--version` prints `wasm-opt version 133 (version_133)`. | `otool -L`, run |
| F6 | proto reads `.prototools` files UPWARDS from the working directory. A pin in a nested `.prototools` is invisible to a bare `proto install` at the repository root ("No versions have been configured, nothing to install!"). From the nested directory, `proto install` and `proto bin wasm-opt` resolve it. | two directory layouts, fresh `PROTO_HOME` |
| F7 | A `file://` plugin path in a nested `.prototools` resolves relative to that file, not to the working directory. | `proto bin` from a sub-directory |
| F8 | `wasm-opt -O` on the committed binary needs no `--enable-*` flag. It reads the `target_features` section. Detected features: mutable-globals, nontrapping-float-to-int, bulk-memory, sign-ext, reference-types, multivalue, bulk-memory-opt, call-indirect-overlong. | `--detect-features --print-features` |
| F9 | Sizes: input 50,902; `-O` 35,471; `-Oz` 35,439; `-O -g` 41,741 bytes. | run |
| F10 | `wasm-opt` is deterministic: the same input gives the same bytes. It is NOT idempotent: `-O` on an `-O` output gives 35,390 bytes. | `cmp` |
| F11 | The raw wasm-pack release output has 1 `name` custom section. The `-O` output has 0. `-O -g` keeps it. `target_features` and `producers` stay. | `WebAssembly.Module.customSections` |
| F12 | The `-O` binary has the same 23 exports and the same 3 imports as the raw binary. `tests/wasm-probe.mjs --corpus` passes on it: sum 67, uuid7 20, prn_canonical 23, prn_cedar 7, prn_fields 6, prn_parse 24. | the drift gate's own probe |
| F13 | `ci.yml` runs a bare `proto install` and does not cache `~/.proto`. | `ci.yml` L69-78 |
| F14 | The drift gate (`tests/committed-wasm.test.ts`) does NOT compare the binary bytes. It compares the four glue files, the literal interface, and a corpus run on both binaries. | the test file |
| F15 | npm `binaryen` is not an alternative: AssemblyScript publishes it, it is one version behind (132.0.0), and it unpacks to 108 MB. | `pnpm view binaryen` |

## 2. Goals

1. binaryen's `wasm-opt` is pinned through a vendored proto plugin with checksum verification.
2. The committed `paigasus_wasm_bg.wasm` and the published `@paigasus/wasm` binary are optimized
   with `wasm-opt -O`, by the pinned binary only.
3. wasm-pack never runs its own `wasm-opt`, so it never downloads an unpinned binaryen.
4. A CI check fails if the committed binary is not optimized, or if wasm-pack starts to optimize.
5. `ci.yml` does not download binaryen.

## 3. Non-goals

- No change to the browser floor of `@paigasus/wasm`. `wasm-opt` adds no wasm feature. The
  features come from rustc (F8).
- No `-g`. The `name` section is removed on purpose. A browser stack trace through the kernel then
  shows `wasm-function[N]`. A panic message still reaches JavaScript through the wasm-bindgen
  `Error` shim.
- No Windows support in the plugin. No job and no developer runs it on Windows.
- No change to the `test` task's wasm-pack call.
- No change to how `prebuild.yml` publishes. Only one step is added to its wasm build.

## 4. Approaches

- **A. Proto plugin, nested pin, an explicit optimize step (chosen).** wasm-pack keeps
  `wasm-opt = false`. A script runs the pinned `wasm-opt` on wasm-pack's output. The pin lives in
  the crate's own `.prototools`, so `ci.yml` never installs it (F6, F13).
- **B. Proto plugin, pin in the root `.prototools`.** Every `ci.yml` run downloads 111 MB and
  unpacks it for a tool that no `ci.yml` task uses (F2, F13). Rejected.
- **C. Root pin, and an explicit tool list in `ci.yml` instead of a bare `proto install`.** A tool
  added later is not installed in CI until somebody also edits the list. Rejected.
- **D. Let wasm-pack find the pinned `wasm-opt` on `PATH`.** The proof that wasm-pack used the
  pinned binary is a match on wasm-pack's log text, and wasm-pack decides which path it runs. A
  proto shim is not the pinned binary itself. Rejected: the explicit step removes the question.
- **E. npm `binaryen`.** Rejected (F15).

## 5. Design

### 5.1 `.proto/plugins/binaryen.toml` (new)

Next to the 15 existing plugins. SPDX header comment, and a comment block in the house style of
`cargo-machete.toml`. The tool id is `wasm-opt`.

```toml
name = "wasm-opt"
type = "cli"

[resolve]
git-url = "https://github.com/WebAssembly/binaryen"
version-pattern = "^version_(?<major>\\d+)$"

[platform.linux.arch]
x86_64 = "x86_64"
aarch64 = "aarch64"
[platform.macos.arch]
x86_64 = "x86_64"
aarch64 = "arm64"

[platform.linux]
download-file = "binaryen-version_{versionMajor}-{arch}-linux.tar.gz"
checksum-file = "binaryen-version_{versionMajor}-{arch}-linux.tar.gz.sha256"
exe-path = "binaryen-version_{versionMajor}/bin/wasm-opt"
[platform.macos]
download-file = "binaryen-version_{versionMajor}-{arch}-macos.tar.gz"
checksum-file = "binaryen-version_{versionMajor}-{arch}-macos.tar.gz.sha256"
exe-path = "binaryen-version_{versionMajor}/bin/wasm-opt"

[install]
download-url = "https://github.com/WebAssembly/binaryen/releases/download/version_{versionMajor}/{download_file}"
checksum-url = "https://github.com/WebAssembly/binaryen/releases/download/version_{versionMajor}/{checksum_file}"
```

The macOS block is measured (F3-F5). The Linux block is measured in plan Task 1 (§6, S1).

### 5.2 `rs/crates/bindings/paigasus-wasm/.prototools` (new)

```toml
# SPDX-License-Identifier: Apache-2.0
# <comment: why this pin is here and not in the root file — F2, F6, F13 — and how to bump it>
wasm-opt = "133.0.0"

[plugins]
wasm-opt = "file://../../../../.proto/plugins/binaryen.toml"
```

The relative path starts at this file (F7). The root `.prototools` gets a comment line that names
this file, so a reader of the root pins finds it.

### 5.3 `rs/crates/bindings/paigasus-wasm/Cargo.toml`

`wasm-opt = false` stays. The comment changes. It says: wasm-pack must not run `wasm-opt`, because
it would download an unpinned binaryen; `scripts/optimize-wasm.mjs` optimizes with the pinned
binary; and drift-gate check 5 (§5.6) fails if this line goes.

### 5.4 `ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs` (new)

Usage: `node scripts/optimize-wasm.mjs <out-dir>`. It optimizes `<out-dir>/paigasus_wasm_bg.wasm`
in place. Node only, no new dependency. Exit 1 means a finding, exit 2 an infrastructure error, in
line with the repository's gates.

1. Set the working directory for every `proto` call to the crate directory, so the nested pin
   applies (F6). Set `PROTO_REPORTER=text` in the child environment (standing rule, SMA-609).
2. Read the pin from the crate `.prototools` with `^wasm-opt\s*=\s*"(\d+)\.0\.0"\s*$` (multiline).
   No match: exit 2.
3. Run `proto install wasm-opt`. It does nothing when the version is installed. Non-zero: exit 2.
4. Resolve the binary with `proto --reporter text bin wasm-opt`. Take the last non-empty line.
   Require a regular, executable file. Else exit 2.
5. Run `<bin> --version`. Require `^wasm-opt version <major> \(version_<major>\)$` on the last
   non-empty line. Else exit 1 and print the expected and the actual value.
6. **Optimize-once guard.** Read the input. Require exactly one `name` custom section
   (`WebAssembly.Module.customSections(module, 'name')`). Zero means the file is already optimized,
   or wasm-pack optimized it (F10, F11): exit 1, and name both causes.
7. Run `<bin> <in> -O -o <out-dir>/paigasus_wasm_bg.wasm.opt-tmp`. Non-zero: exit 1 with its
   stderr.
8. Require that the temporary output compiles and has zero `name` sections. Then rename it over
   the input. A rename is correct here: the input sits in a scratch out-dir, not in the crate
   directory, so no pnpm hard link points at it (the SMA-634 F14 rule applies to the crate copy
   only).
9. Print one line: the input size, the output size and the `wasm-opt` version.

The flag list is `-O` and nothing more (F8). It lives in this one script, so the committed binary
and the published binary cannot get different flags.

### 5.5 Call sites

- **`paigasus-kernel-ts:generate-wasm`** (`ts/packages/paigasus-kernel/moon.yml`): one new line
  `node scripts/optimize-wasm.mjs ../../../rs/crates/bindings/paigasus-wasm/.wasmpack-regen-out`
  between the `wasm-pack` line and `generate-wasm.mjs --post`. The `wasm-pack` call stays a literal
  (FFI_MARKERS in `ci/affected-graph/cargo_moon_parity.py`). `--post` then runs its identity check
  on the optimized binary, which is the binary that is committed. Add
  `/rs/crates/bindings/paigasus-wasm/.prototools` and `/.proto/plugins/binaryen.toml` to the task's
  `inputs`. The task is `cache: false`, so this is documentation for a reader and for the A5/A7
  assertions, which read the declaration.
- **`prebuild.yml`, "Build the wasm distribution"**: after the `test -s` line, run
  `node "$GITHUB_WORKSPACE/ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs" .wasmpack-release-out`.
  The plan confirms how `node` resolves in that job (the job already uses
  `proto --reporter text bin node` at L251). Add `.proto/plugins/binaryen.toml` to both `paths:`
  lists of the workflow, so a plugin change runs this job. The nested `.prototools` is under
  `rs/**` for `push`; for `pull_request` add it explicitly.
- **`paigasus-kernel-ts:test`**: no change. The drift gate needs no optimized fresh build (F12,
  F14), so `ci.yml` needs no binaryen (goal 5).
- **wasm-lockstep container**: no change. It runs `generate-wasm`, and step 3 of §5.4 installs
  binaryen there (111 MB, once a week).

### 5.6 Drift gate: two new checks in `tests/committed-wasm.test.ts`

Both are host-independent, like checks 1-3.

- **Check 4:** the committed `paigasus_wasm_bg.wasm` has zero `name` sections. It fails when
  somebody commits a binary that was not optimized.
- **Check 5:** the fresh `.wasmpack-test-out/paigasus_wasm_bg.wasm` has exactly one `name`
  section. It is the control for check 4: it proves that a raw build has the marker that check 4
  looks for. It also fails if somebody removes `wasm-opt = false`, because wasm-pack then runs its
  own unpinned `wasm-opt` and removes the section (goal 4).

The failure messages name the remedy: run `generate-wasm` for check 4; restore `wasm-opt = false`
for check 5. The section count runs in the test process if `WebAssembly.Module` is available
there; else through `wasm-probe.mjs` in a new mode. The plan picks one after it reads why checks 2
and 3 use a child process.

### 5.7 Regenerate the committed artifacts

Run `moon run paigasus-kernel-ts:generate-wasm` on one host and commit the five files. Expected:
the binary becomes about 35 KB, and the four glue files do not change (glue is written before
`wasm-opt` runs). If a glue file changes, stop and find why.

### 5.8 Documentation

- `rs/CLAUDE.md`, wasm section: one entry. The pin is nested and why; how to bump binaryen (change
  the nested pin, run `generate-wasm`, commit the five files); never optimize twice (F10); never
  remove `wasm-opt = false`.
- The comments in §5.2, §5.3 and the root `.prototools`.
- No root `CLAUDE.md` change.

## 6. Spikes (plan Task 1, before other tasks)

- **S1. Linux.** In `docker run ubuntu:24.04` (x86_64; aarch64 too if the host can emulate it),
  install proto 0.61.1, then `proto install wasm-opt` from the crate directory of a repository
  copy. Record the checksum line and `wasm-opt --version`. Confirm that `bin/wasm-opt` runs without
  `lib/` (F2 suggests a static binary; not yet measured). Do not install host software.
- **S2. Moon and the nested file.** Confirm that `moon` does not read the nested `.prototools` as a
  toolchain pin: `moon setup`, `moon query projects`, and `moon run paigasus-kernel-ts:test` behave
  as before, and no `wasm-opt` install starts. Moon 2.x reads proto config for toolchains; this is
  inferred safe, not measured.
- **S3. `node` in the prebuild wasm job.** Find how the job resolves `node`, and whether
  `ts/packages/paigasus-kernel/scripts/` is checked out there.

If S1 fails, stop and report before any other task. If S2 fails, the fallback is approach C (§4),
and the spec goes back for review.

## 7. Acceptance criteria

1. `proto install wasm-opt`, run from the crate directory, installs a checksum-verified binaryen
   133 on macOS arm64 and Linux x86_64 (measured).
2. A bare `proto install` at the repository root does not install `wasm-opt` (measured in CI: the
   `ci.yml` log of the PR shows no binaryen download).
3. `generate-wasm` produces an optimized binary. The five files are committed. The binary has no
   `name` section. The four glue files are unchanged.
4. `prebuild.yml`'s wasm job runs `optimize-wasm.mjs`, and the uploaded `wasm-dist` binary has no
   `name` section (measured on a run of the branch).
5. Drift-gate checks 1-5 pass in `paigasus-kernel-ts:test`.
6. `optimize-wasm.mjs` fails closed in each case of §8.
7. The full `moon ci` target list from the root `CLAUDE.md` passes in CI.

## 8. Test strategy

- **Unit tests** `tests/optimize-wasm.test.ts`, with a stub `proto` and a stub `wasm-opt` on a
  temporary `PATH` and a temporary crate directory:
  - the correct version: pass, and the output has no `name` section (use a tiny wasm module with a
    `name` section as the input);
  - another version: rc 1, both versions in the message;
  - no binary, or `proto bin` prints a directory: rc 2;
  - `proto bin` prints an NDJSON line before the path (SMA-609): pass;
  - no pin, and a pin with other whitespace (`wasm-opt="133.0.0"`, trailing spaces): rc 2 and
    pass;
  - an input with no `name` section: rc 1, and the input is not changed;
  - the stub `wasm-opt` exits non-zero: rc 1, and the input is not changed.
  The unit test must not download anything.
- **Integration:** `paigasus-kernel-ts:test` (checks 1-5).
- **Mutation battery (delete the feature, not red-first).** Record each result in the PR:
  1. Remove the `optimize-wasm.mjs` line from `generate-wasm` and regenerate: check 4 fails.
  2. Remove `wasm-opt = false` from `Cargo.toml`: check 5 fails. Run it in a scratch
     `PROTO_HOME`-isolated environment, and accept that wasm-pack downloads its own binaryen into
     its cache for this one run.
  3. Run `optimize-wasm.mjs` twice on the same out-dir: the second run exits 1 (§5.4 step 6).
  4. Put a stub `proto` first on `PATH` whose `bin wasm-opt` prints the path of a stub `wasm-opt`
     that reports version 132: rc 1. (A pin change alone cannot test this, because step 3
     installs the new pin.)
  Restore each mutation with an exact reverse edit, not `git checkout --`. Re-run the whole battery
  after any fix.
- **CI:** the PR runs `prebuild.yml` because it edits that file. If its wasm job does not run on
  `pull_request`, run the workflow with `workflow_dispatch` on the branch, or record that it did
  not run.

## 9. Files

| File | Change |
|---|---|
| `.proto/plugins/binaryen.toml` | new |
| `rs/crates/bindings/paigasus-wasm/.prototools` | new: pin and plugin entry |
| `.prototools` | one comment line |
| `rs/crates/bindings/paigasus-wasm/Cargo.toml` | comment only |
| `rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm` | regenerated, optimized |
| `rs/crates/bindings/paigasus-wasm/paigasus_wasm*.{js,d.ts}` | regenerated; expected unchanged |
| `ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs` | new |
| `ts/packages/paigasus-kernel/tests/optimize-wasm.test.ts` | new |
| `ts/packages/paigasus-kernel/tests/committed-wasm.test.ts` | checks 4 and 5 |
| `ts/packages/paigasus-kernel/moon.yml` | `generate-wasm` script line and inputs |
| `.github/workflows/prebuild.yml` | optimize step; `paths:` entries |
| `rs/CLAUDE.md` | one entry |

## 10. Risks

- **R1.** binaryen moves on. The pin does not follow, because no Dependabot ecosystem covers proto.
  This is the same as every other proto pin. The `rs/CLAUDE.md` entry names the bump steps.
- **R2.** A future rustc stops writing a `name` section in a release build. Then check 5 fails and
  `optimize-wasm.mjs` refuses every input. The failure is loud, and the message names the marker.
  The fix is a different marker (for example a size or a custom section of our own).
- **R3.** `-O` changes behaviour on a path that no corpus row reaches. The corpus covers the 13
  kernel exports (F12). Paths outside it are not covered.
- **R4.** The nested `.prototools` surprises a reader who expects every pin in the root file. The
  root comment line (§5.2) and the `rs/CLAUDE.md` entry are the mitigation.
- **R5.** S2 fails (moon reads the nested file). Then the fallback is approach C, with its own cost.

## 11. Decisions (from the 2026-10-04 brainstorm)

- Pin AND enable now (not pin only, not defer).
- The pin is nested in the crate directory (approach A).
- `-O`, with no `-g`.
