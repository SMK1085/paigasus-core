# SMA-435: pin binaryen (`wasm-opt`) through proto, and optimize the wasm kernel

- Linear: SMA-435. Related: SMA-427 (L3, the `wasm-opt = false` line), SMA-375 (proto pinning),
  SMA-634 (the five committed artifacts and the drift gate), SMA-693 (the wasm-lockstep workflow).
- Status: design approved in chat on 2026-10-04. Challenged once (APPROVE WITH CHANGES); all
  findings folded in (§12). The written spec waits for review.
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
| F8 | `wasm-opt -O` on the committed binary needs no `--enable-*` flag. It reads the `target_features` section. Declared features: mutable-globals, nontrapping-float-to-int, bulk-memory, sign-ext, reference-types, multivalue, bulk-memory-opt, call-indirect-overlong. | `--detect-features --print-features` |
| F9 | Sizes: input 50,902; `-O` 35,471; `-Oz` 35,439; `-O -g` 41,741 bytes. | run |
| F10 | `wasm-opt` is deterministic: the same input gives the same bytes. It is NOT idempotent: `-O` on an `-O` output gives 35,390 bytes. | `cmp` |
| F11 | The raw wasm-pack release output has 1 `name` custom section. The `-O` output has 0. `-O -g` keeps it. `target_features` and `producers` stay. | `WebAssembly.Module.customSections` |
| F12 | The `-O` binary has the same 23 exports and the same 3 imports as the raw binary. `tests/wasm-probe.mjs --corpus` passes on it: sum 67, uuid7 20, prn_canonical 23, prn_cedar 7, prn_fields 6, prn_parse 24. | the drift gate's own probe |
| F13 | `ci.yml` runs a bare `proto install` and does not cache `~/.proto`. | `ci.yml` L69-78 |
| F14 | The drift gate (`tests/committed-wasm.test.ts`) does NOT compare the binary bytes. It compares the four glue files, the literal interface, and a corpus run on both binaries. | the test file |
| F15 | npm `binaryen` is not an alternative: AssemblyScript publishes it, it is one version behind (132.0.0), and it unpacks to 108 MB. | `pnpm view binaryen` |
| F16 | The vitest `node` project lists its test files one by one (`vitest.config.ts` L31-41). A new test file runs only if it is added there. | read |
| F17 | No TOML parser is in `ts/pnpm-lock.yaml`. | grep |

## 2. Goals

1. binaryen's `wasm-opt` is pinned through a vendored proto plugin with checksum verification.
2. The committed `paigasus_wasm_bg.wasm` and the published `@paigasus/wasm` binary are optimized
   with `wasm-opt -O`, by the pinned binaryen version.
3. wasm-pack never runs its own `wasm-opt` in the `release` or `profiling` profile, so it never
   downloads an unpinned binaryen.
4. CI fails if the committed binary was not optimized by the pinned version, if the pin moved
   without a regeneration, or if the wasm-pack configuration lets wasm-pack optimize.
5. `ci.yml` does not download binaryen.
6. A change to the optimize script runs its tests in PR CI.

## 3. Non-goals

- No intended change to the browser floor of `@paigasus/wasm`. `wasm-opt` uses only the declared
  features (F8). It can start to USE a declared feature that the raw binary did not use (for
  example `memory.init` from memory packing). This is inferred, not measured; S5 (§6) measures it.
- No `-g`. The `name` section is removed on purpose. A browser stack trace through the kernel then
  shows `wasm-function[N]`. A panic message still reaches JavaScript through the wasm-bindgen
  `Error` shim.
- No Windows support in the plugin. No job and no developer runs it on Windows.
- No change to the `test` task's wasm-pack call.
- No change to how `prebuild.yml` and `release.yml` publish. Steps are added to the wasm build only.
- No change to the root `.prototools`. A change there starts five path-filtered workflows
  (`prebuild`, `wheels`, `images`, `chart`, `security-scan`) and every Moon task with a
  `/.prototools` input, for a comment only.

## 4. Approaches

- **A. Proto plugin, nested pin, an explicit optimize step (chosen).** wasm-pack keeps
  `wasm-opt = false`. A script runs the pinned `wasm-opt` on wasm-pack's output. The pin lives in
  the crate's own `.prototools`, so `ci.yml` never installs it (F6, F13).
- **A2. Fallback if S2 fails: root plugin entry, version in the script.** A `[plugins]` entry for
  `wasm-opt` in the root `.prototools` with NO version line, and the script calls
  `proto install wasm-opt 133.0.0` and `proto bin wasm-opt 133.0.0`. Before use, measure (as for
  F6) that a bare `proto install` skips a plugin that has no version.
- **B. Proto plugin, pin in the root `.prototools`.** Every `ci.yml` run downloads 111 MB and
  unpacks it for a tool that no `ci.yml` task uses (F2, F13). Rejected.
- **C. Root pin, and an explicit tool list in `ci.yml` instead of a bare `proto install`.** A tool
  added later is not installed in CI until somebody also edits the list. Rejected.
- **D. Let wasm-pack find the pinned `wasm-opt` on `PATH`.** The proof that wasm-pack used the
  pinned binary is a match on wasm-pack's log text, and wasm-pack decides which path it runs.
  Rejected: the explicit step removes the question.
- **E. npm `binaryen`.** Rejected (F15).

## 5. Design

### 5.1 `.proto/plugins/binaryen.toml` (new)

Next to the 15 existing plugins. SPDX header comment, and a comment block in the house style of
`cargo-machete.toml`. The comment names the nested pin file (§5.2), because the root `.prototools`
does not change. The tool id is `wasm-opt`.

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

The macOS block is measured (F3-F5). The Linux block is measured in plan Task 1 (S1, S4).

### 5.2 `rs/crates/bindings/paigasus-wasm/.prototools` (new)

```toml
# SPDX-License-Identifier: Apache-2.0
# <comment: why this pin is here and not in the root file — F2, F6, F13 — and how to bump it>
wasm-opt = "133.0.0"

[plugins]
wasm-opt = "file://../../../../.proto/plugins/binaryen.toml"
```

The relative path starts at this file (F7).

### 5.3 `rs/crates/bindings/paigasus-wasm/Cargo.toml`

`wasm-opt = false` stays under `[package.metadata.wasm-pack.profile.release]`. Add the same line
under `[package.metadata.wasm-pack.profile.profiling]`, because wasm-pack's `--profiling` runs
`wasm-opt` by default. The `dev` profile does not run it. The comment says: wasm-pack must not run
`wasm-opt`, because it would download an unpinned binaryen; `scripts/optimize-wasm.mjs` optimizes
with the pinned binary; drift-gate check 7 fails if either line changes.

### 5.4 `ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs` (new)

Node only, no new dependency. Exit 1 is a finding, exit 2 an infrastructure error, in line with
the repository's gates. Two modes:

```
node scripts/optimize-wasm.mjs optimize <out-dir>   optimize <out-dir>/paigasus_wasm_bg.wasm in place
node scripts/optimize-wasm.mjs verify <dir>         check <dir>/paigasus_wasm_bg.wasm only
```

`<out-dir>` and `<dir>` resolve against `process.cwd()`. The crate directory and its `.prototools`
come from `import.meta.url` (the same way `generate-wasm.mjs` L21-22 finds the repository root).
There is no environment variable and no flag for them, so production has no seam that can point
the script at another pin. The module also exports small pure helpers (§5.4.3), and runs a mode
only when it is the entry point (`import.meta.url` equals `pathToFileURL(process.argv[1])`).

#### 5.4.1 The marker

After `wasm-opt` finishes, the script appends one custom section of its own to the binary:
name `paigasus.wasm-opt`, payload the UTF-8 text `binaryen=<major>;flags=-O`. A custom section is
id 0, a LEB128 size, a LEB128 name length, the name, and the payload. It does not change the
imports, the exports or the glue. This marker is what the gates check. The `name` section (F11)
is only a secondary signal for a raw wasm-pack output.

#### 5.4.2 `optimize <out-dir>`

1. Run every `proto` call with the crate directory as the working directory, so the nested pin
   applies (F6). In the child environment, set `PROTO_REPORTER=text` (SMA-609) and delete
   `PROTO_WASM_OPT_VERSION` (proto reads it before any file).
2. Read the pin from the crate `.prototools` with `^wasm-opt\s*=\s*"(\d+)\.0\.0"\s*$`
   (multiline). No match: exit 2.
3. Run `proto install wasm-opt`. It does nothing when the version is installed. Non-zero: exit 2.
4. Resolve the binary with `proto --reporter text bin wasm-opt`. Take the last non-empty line.
   Require a regular, executable file. Else exit 2.
5. Run `<bin> --version`. Require `^wasm-opt version <major> \(version_<major>\)$` on the last
   non-empty line. Else exit 1 and print the expected and the actual value.
6. **Optimize-once guard.** Read the input. Exit 1 if it already has a `paigasus.wasm-opt`
   section (optimized before; F10). Exit 1 if it has no `name` section. The message names the
   three causes: wasm-pack optimized it (check `Cargo.toml`), it was optimized by another tool,
   or a rustc, wasm-bindgen or `strip` change removed the section (R2).
7. Run `<bin> <in> -O -o <out-dir>/.optimize-wasm.tmp`. The temporary name does not start with
   `paigasus_wasm`, so the staging glob `cp .wasmpack-release-out/paigasus_wasm*`
   (`prebuild.yml` L367) cannot copy it. Non-zero: exit 1 with its stderr.
8. Require that the temporary output compiles, has zero `name` sections, has zero
   `paigasus.wasm-opt` sections, and has the same export names as the input. Else exit 1.
9. Append the marker (§5.4.1) and rename the temporary file over the input. A rename is correct:
   the input sits in a scratch out-dir, not in the crate directory, so no pnpm hard link points at
   it (the SMA-634 F14 rule applies to the crate copy only).
10. On every failure path after step 7, delete the temporary file. The input is never changed on
    failure.
11. Print one line: the input size, the output size and the `wasm-opt` version.

The flag list is `-O` and nothing more (F8). It lives in this one script, so the committed binary
and the published binary cannot get different flags.

#### 5.4.3 `verify <dir>` and the exported helpers

`verify` reads the pin (step 2) and requires that `<dir>/paigasus_wasm_bg.wasm` has exactly one
`paigasus.wasm-opt` section with the payload `binaryen=<pin major>;flags=-O`, and zero `name`
sections. It needs no `proto` and no network.

Exported helpers, used by `verify`, by the drift gate (§5.6) and by the unit tests:
`customSectionCount(bytes, name)`, `customSectionPayloads(bytes, name)`, `expectedMarker()` (reads
the nested pin), and `appendCustomSection(bytes, name, payload)`.

### 5.5 Call sites and wiring

- **`paigasus-kernel-ts:generate-wasm`** (`ts/packages/paigasus-kernel/moon.yml`): one new line
  `node scripts/optimize-wasm.mjs optimize ../../../rs/crates/bindings/paigasus-wasm/.wasmpack-regen-out`
  between the `wasm-pack` line and `generate-wasm.mjs --post`. The `wasm-pack` call stays a literal
  (FFI_MARKERS in `ci/affected-graph/cargo_moon_parity.py`). `--post` then runs its identity check
  on the optimized binary, which is the binary that is committed. Add
  `/rs/crates/bindings/paigasus-wasm/.prototools` and `/.proto/plugins/binaryen.toml` to the task's
  `inputs`. The task is `cache: false`, and A5/A7 only check that the required inputs are present,
  so these two entries are for a human reader only. Update the comment block above the task.
- **`paigasus-kernel-ts:test`**: the wasm-pack call does not change (F12, F14), so `ci.yml` needs
  no binaryen (goal 5). Add two `inputs`: `scripts/optimize-wasm.mjs` (the unit tests import it,
  and checks 5 and 7 use its helpers) and `/rs/crates/bindings/paigasus-wasm/.prototools` (check 5
  reads the pin). On Moon 2.5.3 only inputs select a task, so without these an edit to the script
  or a pin bump selects nothing in CI.
- **`vitest.config.ts`**: add `tests/optimize-wasm.test.ts` to the `include` list of the `node`
  project (F16).
- **`prebuild.yml`, `assemble` job, "Build the wasm distribution"**: after the `test -s` line, run
  `node "$GITHUB_WORKSPACE/ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs" optimize .wasmpack-release-out`.
  The job checks out the whole repository, `moon setup` and L248-263 put the pinned `node` on
  `PATH`, and the job has no `if:`, so it runs on `pull_request` (challenge finding; S3 closed).
- **`prebuild.yml`, new step after "Stage the wasm distribution for upload"**: run, on
  `rs/crates/bindings/paigasus-wasm/wasm-dist`:
  `optimize-wasm.mjs verify`, `tests/wasm-probe.mjs --interfaces` and `tests/wasm-probe.mjs --corpus`.
  `wasm-dist` holds the glue, the binary and a `package.json` with `"type": "module"`, so the probe
  works there. This is the lasting check on the binary that ships.
- **`prebuild.yml` `paths:`**: add to both lists `.proto/plugins/binaryen.toml`,
  `ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs` and
  `ts/packages/paigasus-kernel/tests/wasm-probe.mjs`; add
  `rs/crates/bindings/paigasus-wasm/.prototools` to the `pull_request` list (`rs/**` covers it on
  `push`). A comment explains the exception to the "no `ts/` path on push" rule (L12-15): these
  files are on the release path.
- **wasm-lockstep container**: no code change. `generate-wasm` installs binaryen there through
  §5.4.2 step 3 (111 MB, once a week). S4 measures it before the merge.

### 5.6 Drift gate: three new checks in `tests/committed-wasm.test.ts`

SMA-634 already uses "check 4" (the installed-copy check), so the new checks are 5, 6 and 7. All
are host-independent and run in the test process: counting sections needs no import, so the
child-process reason for checks 2 and 3 (the console loader, SMA-634 F11) does not apply. They use
the helpers of §5.4.3.

- **Check 5:** the committed `paigasus_wasm_bg.wasm` has exactly one `paigasus.wasm-opt` section,
  its payload equals `expectedMarker()`, and it has zero `name` sections. It fails when somebody
  commits a binary that was not optimized, was optimized by another binaryen version, or when the
  nested pin moved without a regeneration. Remedy in the message: run `generate-wasm`.
- **Check 6:** the fresh `.wasmpack-test-out/paigasus_wasm_bg.wasm` has exactly one `name` section
  and zero `paigasus.wasm-opt` sections. It is the control for check 5 (a raw build has the marker
  that `optimize` step 6 requires), and it fails when wasm-pack optimizes and removes the section.
  Remedy: restore `wasm-opt = false`.
- **Check 7:** `rs/crates/bindings/paigasus-wasm/Cargo.toml` holds `wasm-opt = false` under both
  `[package.metadata.wasm-pack.profile.release]` and `[...profile.profiling]`. This catches
  `wasm-opt = ['-O', '-g']`, which check 6 cannot see (the `name` section stays). No TOML parser is
  in the lockfile (F17), so a small section-scoped line reader does this; it must accept spaces
  around `=` and a trailing comment.

### 5.7 Regenerate the committed artifacts

Run `moon run paigasus-kernel-ts:generate-wasm` on one host and commit the five files. Expected:
the binary becomes about 35.5 KB (F9 plus the marker), and the four glue files do not change (glue
is written before `wasm-opt` runs). If a glue file changes, stop and find why.

### 5.8 Documentation

- `rs/CLAUDE.md`, wasm section: one entry. The pin is nested and why; how to bump binaryen (change
  the nested pin, run `generate-wasm`, commit the five files); never optimize twice (F10); never
  remove `wasm-opt = false`. Also L202: the network prerequisite of `generate-wasm` now includes
  binaryen.
- `ts/CLAUDE.md` L127 ("four checks"): now seven.
- The header of `tests/committed-wasm.test.ts` (L3-13): checks 5-7.
- The `generate-wasm` comment block in `ts/packages/paigasus-kernel/moon.yml` (L200-241).
- `contracts/CLAUDE.md` L52-59 (the wasm FFI rules): one line on the optimize step.
- `ci/wasm-lockstep/README.md` L104-105 (third-party code in the container): binaryen.
- `ci/wasm-lockstep/lockstep_check.py` L63: the size comment.
- The comments in §5.1, §5.2 and §5.3. No root `CLAUDE.md` change, no root `.prototools` change.

## 6. Spikes (plan Task 1, before other tasks)

- **S1. Linux, plain.** In `docker run --platform linux/amd64 ubuntu:24.04`, install proto 0.61.1,
  then `proto install wasm-opt` from the crate directory of a repository copy. Record the checksum
  line and `wasm-opt --version`. Confirm that `bin/wasm-opt` runs without `lib/` (`ldd`). Do not
  install host software.
- **S2. Moon, proto shims and the nested file.** Confirm that Moon does not read the nested
  `.prototools` as a toolchain pin: `moon setup`, `moon query projects`,
  `moon run paigasus-kernel-ts:test`, and `moon run paigasus-wasm-rs:test paigasus-wasm-rs:lint`
  (its `cargo nextest` shim runs from the crate directory) behave as before, and no `wasm-opt`
  install starts. Log `proto --version` from inside `generate-wasm`, because Moon can use another
  proto (0.60.2 in the container, `ci/wasm-lockstep/README.md` L224). Record whether the first
  `proto install wasm-opt` rewrites files under `~/.proto/shims` (`ci/CLAUDE.md` L29 records a
  shim `EACCES` under concurrency). If S2 fails, use fallback A2 (§4) and send the spec back for
  review.
- **S3.** Closed by the challenge (§5.5).
- **S4. The wasm-lockstep container.** Run `bash ci/wasm-lockstep/container.sh build` in
  `$LOCKSTEP_IMAGE` (`rust:1.95.0-bookworm`) with the exact flags of `wasm-lockstep.yml` L138
  (uid 65534, `--cap-drop=ALL`) plus `--platform linux/amd64`, on a `git archive` copy owned by
  uid 65534. Record the `optimize-wasm` output line and the binary size. Optional: the SMA-693 M0
  run on a scratch branch named `feature/sma-435-…-scratch`.
- **S5. Used features.** Compare the features each binary actually uses: validate the raw and the
  optimized binary with `wasm-opt --mvp-features` plus each declared feature switched on in turn
  (or an equivalent validator), and record any feature that only the optimized binary needs. A
  new one is a browser-floor finding: stop and report.

If S1 or S4 fails, stop and report before any other task.

## 7. Acceptance criteria

1. `proto install wasm-opt`, run from the crate directory, installs a checksum-verified binaryen
   133 on macOS arm64 and Linux x86_64 (measured).
2. A bare `proto install` at the repository root does not install `wasm-opt` (measured in CI: the
   `ci.yml` log of the PR shows no binaryen download).
3. `generate-wasm` produces an optimized binary with the marker. The five files are committed.
   The four glue files are unchanged.
4. `prebuild.yml`'s `assemble` job runs `optimize` and the new verify step, on the PR run.
5. Drift-gate checks 1-7 pass in `paigasus-kernel-ts:test`.
6. `optimize-wasm.mjs` fails closed in each case of §8.
7. An edit to `optimize-wasm.mjs` alone selects `paigasus-kernel-ts:test` in `moon ci` (measured
   with `moon query tasks --affected`, one target per task).
8. The full `moon ci` target list from the root `CLAUDE.md` passes in CI.

## 8. Test strategy

- **Unit tests** `tests/optimize-wasm.test.ts`. Each test builds a fixture tree in a temporary
  directory: a copy of `optimize-wasm.mjs` at `<tmp>/ts/packages/paigasus-kernel/scripts/`, a
  `<tmp>/rs/crates/bindings/paigasus-wasm/.prototools`, and a stub `proto` first on `PATH` whose
  `bin wasm-opt` prints the path of a stub `wasm-opt`. The stub `wasm-opt` writes its argv to a
  file and copies the input to the output with a configurable change. The input is a tiny wasm
  module with a `name` section, built in the test. No test downloads anything. Cases:
  - the correct version: pass; the stub received exactly `[<in>, '-O', '-o', <tmp>]`; the output
    has the marker and no `name` section;
  - another version: rc 1, both versions in the message;
  - `proto install` fails: rc 2;
  - no binary, or `proto bin` prints a directory: rc 2;
  - `proto bin` prints an NDJSON line before the path (SMA-609): pass;
  - no pin: rc 2; a pin with other spacing (`wasm-opt="133.0.0"`, trailing spaces): pass;
  - `PROTO_WASM_OPT_VERSION=1.0.0` in the environment: the stub `proto` sees it unset;
  - an input with no `name` section, and an input that already has the marker: rc 1;
  - the stub `wasm-opt` exits non-zero; its output keeps a `name` section; its output is not valid
    wasm; its output drops an export: each rc 1;
  - for every rc 1 and rc 2 case after step 7: the input is byte-identical afterwards and no
    `.optimize-wasm.tmp` remains;
  - `verify`: pass on a marked binary; rc 1 on a wrong payload, a second marker, or a `name`
    section;
  - the helpers: `appendCustomSection` then `customSectionPayloads` round-trips a payload longer
    than 127 bytes (a two-byte LEB128 size);
  - check 7's reader: accepts `wasm-opt=false` and `wasm-opt = false # note`; rejects
    `wasm-opt = ['-O']` and a `wasm-opt = false` line that sits in another section.
- **Integration:** `paigasus-kernel-ts:test` (checks 1-7).
- **Mutation battery (delete the feature).** Record each result in the PR:
  1. Remove the `optimize` line from `generate-wasm` and regenerate: check 5 fails.
  2. Remove `wasm-opt = false` from the release profile: check 7 fails, and the `test` task's
     fresh build fails check 6. Record which `wasm-opt` wasm-pack ran: with the proto shim on
     `PATH`, it can find the pinned binary through the shim instead of its own download.
  3. Change the release line to `wasm-opt = ['-O', '-g']`: check 7 fails; check 6 passes (this
     proves why check 7 exists).
  4. Delete the step-8 checks from the script, make the stub keep the `name` section: the unit
     test fails.
  5. Delete the step-5 version comparison from the script, then run `moon run
     paigasus-kernel-ts:test` WITHOUT `--force`: the task is selected and the unit test fails.
  6. Bump the nested pin to `132.0.0` and do not regenerate: check 5 fails.
  Restore each mutation with an exact reverse edit, not `git checkout --`. Re-run the whole battery
  after any fix.
- **CI:** the PR runs `prebuild.yml` because it edits that file. Record that the `assemble` job
  ran both new steps. Record that `images.yml` passed if the PR starts it.

## 9. Files

| File | Change |
|---|---|
| `.proto/plugins/binaryen.toml` | new |
| `rs/crates/bindings/paigasus-wasm/.prototools` | new: pin and plugin entry |
| `rs/crates/bindings/paigasus-wasm/Cargo.toml` | `profiling` line; comment |
| `rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm` | regenerated, optimized, marked |
| `rs/crates/bindings/paigasus-wasm/paigasus_wasm*.{js,d.ts}` | regenerated; expected unchanged |
| `ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs` | new |
| `ts/packages/paigasus-kernel/tests/optimize-wasm.test.ts` | new |
| `ts/packages/paigasus-kernel/tests/committed-wasm.test.ts` | checks 5-7; header |
| `ts/packages/paigasus-kernel/vitest.config.ts` | `include` entry |
| `ts/packages/paigasus-kernel/moon.yml` | `generate-wasm` line, inputs, comment; `test` inputs |
| `.github/workflows/prebuild.yml` | optimize step; verify step; `paths:` entries |
| `rs/CLAUDE.md`, `ts/CLAUDE.md`, `contracts/CLAUDE.md` | §5.8 |
| `ci/wasm-lockstep/README.md`, `ci/wasm-lockstep/lockstep_check.py` | §5.8 (comment and docs only) |

## 10. Risks

- **R1.** binaryen moves on. The pin does not follow, because no Dependabot ecosystem covers proto.
  This is the same as every other proto pin. The `rs/CLAUDE.md` entry names the bump steps.
- **R2.** The `name` section of a raw build disappears: a rustc or wasm-bindgen default changes
  (the weekly lockstep moves wasm-bindgen), or somebody adds `strip = true` to a `[profile.release]`
  in `rs/Cargo.toml` for smaller service images. Then check 6 fails, `optimize` refuses every
  input, and the weekly wasm-lockstep build fails until the guard changes. The failure is loud and
  the message names these causes. The fix is to drop the `name` half of step 6; the marker stays
  the real optimize-once guard.
- **R3.** `-O` changes behaviour on a path that no corpus row reaches. The corpus covers the 13
  kernel exports (F12) and now also runs on the published binary (§5.5).
- **R4.** The nested `.prototools` surprises a reader who expects every pin in the root file. The
  plugin comment and the `rs/CLAUDE.md` entry are the mitigation.
- **R5.** S2 fails. Then fallback A2.
- **R6.** Check 2 now compares an optimized binary with a raw binary. Equal exports are a
  `wasm-opt` guarantee, and step 8 enforces it. Equal imports are not:
  `remove-unused-module-elements` can remove an import that becomes unreachable. F12 measured equal
  imports for today's kernel only. If they differ later, check 2 fails and its `generate-wasm`
  remedy cannot fix it. The fix then is a deliberate change of check 2.

## 11. Rollout and rollback

- `paigasus-wasm` has `publish = false`, so release-plz does not process it. The optimized binary
  reaches npm at the next kernel-group release.
- The wasm-lockstep bot PR opened on 2026-10-04 (SMA-693 M3) holds a raw binary. After this merge
  it fails check 5 until the workflow runs again.
- Rollback is a revert of the PR. No data or state changes.
- Commit type: decided at the spec gate (§12, open question).

## 12. Decisions and challenge log

Brainstorm decisions (2026-10-04): pin AND enable now; the pin is nested (approach A); `-O`, no `-g`.

Challenge (Opus, APPROVE WITH CHANGES), folded in:

| Finding | Severity | Change |
|---|---|---|
| A script-only edit runs no check in PR CI | BLOCKER | §5.5: vitest `include`, `test` inputs, `prebuild.yml` paths; AC 7; mutation 5 |
| The `name` marker does not prove the pinned version | MAJOR | §5.4.1 own marker section; check 5 compares it with the pin |
| Check 5 (old) does not protect goal 3 | MAJOR | check 7 reads `Cargo.toml`; `profiling` profile; mutation 3 |
| No lasting check on the published binary | MAJOR | §5.5 verify + probe step on `wasm-dist` |
| The wasm-lockstep path first runs after the merge | MAJOR | S4 |
| The unit-test seam is not chosen | MAJOR | §5.4: fixture-tree copy, no env or flag seam |
| Guards without a failing test | MAJOR | §8: step-3, step-8, argv and cleanup tests; mutation 4 |
| "check 4" is taken | MINOR | checks are 5, 6, 7 |
| S2 misses the crate's own project and the proto version | MINOR | S2 widened |
| S3 can be closed | MINOR | closed in §5.5 |
| Base dir and temp name | MINOR | §5.4 and step 7 |
| R2 lists too few causes | MINOR | R2 widened; step-6 message |
| Check 2 compares optimized with raw | MINOR | R6; step 8 compares exports |
| Browser floor is inferred | MINOR | §3 says so; S5 measures |
| Documentation list | MINOR | §5.8 |
| Cost of the root `.prototools` comment | MINOR | the comment is dropped; root file unchanged |
| No rollout section | MINOR | §11 |
| Wrong reason for the `generate-wasm` inputs | MINOR | §5.5 corrected |
| Mutation 2 isolates the wrong cache | MINOR | mutation 2 records which binary ran |
| A simpler fallback | MINOR | approach A2 replaces C as the fallback |
| The §5.6 choice can be made | MINOR | in-process, shared helpers |
| Q: shim rewrite on first install | QUESTION | measured in S2 |
| Q: binaryen `producers` marker | QUESTION | not needed: own marker |
| Q: `PROTO_WASM_OPT_VERSION` | QUESTION | §5.4.2 step 1 deletes it |
| Q: commit type and changelog | QUESTION | open for the spec gate |

Rejected: none.

Open question for the spec gate: the conventional-commit type (`perf(rs)` proposed, because the
user-visible effect is a 30% smaller binary), and whether `@paigasus/wasm` gets a changelog line.
