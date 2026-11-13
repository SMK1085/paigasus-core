# SMA-767: a stripped macOS binding that dyld rejects (mis-aligned LINKEDIT string pool)

Status: revised after the spec challenge, for GATE 1. Linear: SMA-767.
Branch: `feature/sma-767-maturin-linkedit-dlopen`.

## 1. Problem

On the development Mac, `import paigasus_py_bindings` fails for every local maturin build:

```
dlopen(...paigasus_py_bindings.abi3.so, 0x0002): ... (mis-aligned LINKEDIT string pool, fileOffset=0x00062514)
```

So `moon run paigasus-kernel-py:test` fails locally (rc 2). CI (Linux) is not affected, because
Linux uses ELF, not Mach-O.

## 2. Measured cause

All facts in this section are MEASURED on 2026-11-13 on the development Mac, unless a line says
"inferred". §9 lists the scripts.

Host: macOS 27.2 (build 26B123), arm64. Command Line Tools SDK 27.2, `ld-27106.1`, Apple clang
21.0.0. Rust 1.95.0 (LLVM 22.1.2), maturin 1.9.6, MACOSX_DEPLOYMENT_TARGET 11.0 (set by maturin).

### 2.1 The chain

1. The workspace has no `[profile.release]`. So a release build uses the cargo default
   `strip = "debuginfo"`. `cargo build --release -v` passes `-C strip=debuginfo` to 26 of the
   rustc units, including every build script and proc macro. A dev build passes it to none
   (0 of 34 units). So the defect concerns release builds only.
2. On macOS, rustc 1.95 does that strip with its bundled `rust-objcopy` (LLVM), not with Apple
   `strip`. Proof: a cloned toolchain with a logging wrapper in place of `rust-objcopy`. The log
   shows `rust-objcopy --strip-debug <target>/release/deps/libpaigasus_py_bindings.dylib`. It also
   shows the same call for 8 build scripts and 2 proc-macro dylibs.
3. `rust-objcopy` writes the LINKEDIT tables back to back with no padding: the symbol table
   (16-byte entries), then the indirect symbol table (4-byte entries), then the string pool.
4. The py dylib has 215 indirect symbols, an odd count. So the string pool starts on a 4-byte
   boundary: `symoff 381176 + 1292 × 16 = 401848 = indirectsymoff`; `401848 + 215 × 4 = 402708 =
   stroff = 0x62514`. 402708 mod 8 = 4.
5. dyld on macOS 27.2 rejects a string pool that is not 8-byte aligned, but only in a binary whose
   `LC_BUILD_VERSION` `sdk` field is new. The same mis-aligned bytes, stamped with `vtool` and
   re-signed: `sdk 27.2` is rejected; `sdk 26.5` and `sdk 15.0` load. Every local build has
   `sdk 27.2`, because the host SDK is 27.2.
6. maturin does not change the file. The cargo dylib and the `.so` in the wheel are
   byte-identical (`shasum` equal).

ld64 itself (no strip step) pads the string pool: `434808 + 860 = 435668`, but `stroff = 435672`.

### 2.2 The matrix (all local, `sdk 27.2`)

| Build | stroff | mod 8 | Load |
|---|---|---|---|
| maturin `--release`, fresh `CARGO_TARGET_DIR` | 402708 | 4 | rejected |
| cargo release, default strip | 402708 | 4 | rejected |
| cargo release, `strip = "symbols"` | 383876 | 4 | rejected |
| cargo release, `strip = "none"` | 435672 | 0 | loads |
| cargo debug (no strip) | 807032 | 0 | loads |
| release, MACOSX_DEPLOYMENT_TARGET 12.0 / 27.0 / unset | 402708 | 4 | rejected |
| unstripped dylib + Apple `/usr/bin/strip -S` | 402712 | 0 | loads |
| unstripped dylib + Apple `/usr/bin/strip -x` | 383896 | 0 | loads |
| unstripped dylib + `rust-objcopy --strip-debug` | 402708 | 4 | rejected |
| unstripped dylib + `rust-objcopy --strip-all` | 383844 | 4 | rejected |
| maturin + `-C strip=none` as an apple-darwin target rustflag | 435672 | 0 | loads |
| napi binding, cargo release, default strip | 470128 | 0 | loads (198 indirect symbols, even) |
| napi binding, cargo release, `strip = "none"` | 503056 | 0 | loads |

In every build, `stroff mod 8 = 4` with `sdk 27.2` failed to load, and `stroff mod 8 = 0` loaded.

### 2.3 Candidate causes from the issue, and the verdict

- **Stale or huge `rs/target`**: ruled out. A fresh `CARGO_TARGET_DIR` in a scratch directory
  fails at the same offset `0x00062514`.
- **macOS SDK / linker against Rust 1.95**: the linker is not at fault. The ld64 output loads.
  The fault is the post-link strip tool (`rust-objcopy`). The SDK matters only because dyld
  enforces the rule for a binary linked with the new SDK (§2.1.5).
- **The napi binding on the same host**: its release build loads, but only by chance (198 is
  even). Its dev build, which `paigasus-kernel-ts:build` uses, is never stripped.
- **Deployment target**: not a factor.

### 2.4 The published artefacts and CI

Inspected with `otool` only. The downloads were not executed.

| Artefact | sdk | indirect syms | stroff | mod 8 |
|---|---|---|---|---|
| PyPI `paigasus-py-bindings` 0.2.0, `macosx_11_0_arm64` | 26.5 | 215 | 402428 | 4 |
| PyPI `paigasus-py-bindings` 0.2.0, `macosx_10_12_x86_64` | 26.5 | 215 | 397956 | 4 |
| npm `@paigasus/node-bindings-darwin-arm64` 0.2.0 | 26.5 | 192 | 435872 | 0 |

- The two PyPI wheels are mis-aligned, but they carry `sdk 26.5`. By §2.1.5 they load on macOS 27
  (inferred: the same rule held for a stamped local file; the wheels themselves were not loaded).
- CI is green because the CI runners link with SDK 26.5 (inferred from the artefacts). When the
  runner image moves to SDK 27, the published macOS wheels will stop loading on macOS 27. Today
  this is a latent defect for users, and an active defect for local builds.
- `@paigasus/node-bindings-darwin-x64` 0.2.0 was not inspected.
- Upstream (not verified): a web search found LLVM commit #203680, "[llvm-objcopy][MachO] Align
  __LINKEDIT entries to pointer size". I did not read it. I do not know which Rust release
  contains it.

## 3. Decisions

- **D1. Fix: add `-C strip=none` to the apple-darwin rustflags in `rs/.cargo/config.toml`.** Both
  triples (`x86_64-apple-darwin`, `aarch64-apple-darwin`) already have a `rustflags` list there.
  Append `"-C", "strip=none"` to each. rustc gets the profile's `-C strip=debuginfo` first and the
  rustflag last, and the last one wins (measured in the `-v` log). Measured: the maturin wheel
  built this way loads (row in §2.2). That measurement used the env form of the key
  (`CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS`), not an edit of the file. The real Moon task
  with the edited file is not measured yet (§6 step 1).

  Why this place:
  - It applies only to macOS builds. A profile override cannot be limited to one target. Measured
    in a `rust:1.95.0-bookworm` container: `strip = "none"` grows the Linux py `.so` from 582072 to
    4488136 bytes (7.7 times), and the Linux napi `.so` from 671056 to 4577928 bytes. On macOS the
    growth is +39 KB (508112 to 547600), because Mach-O keeps DWARF out of the dylib.
  - It covers both bindings with one change: maturin, `uv` (PEP 517), and `napi build` all run
    cargo from inside `rs/`. The `wheels.yml` darwin legs run maturin with `working-directory: rs`
    and set no `RUSTFLAGS` (only the musl leg does).
  - In a release build without `--target`, it also reaches the build scripts and proc macros
    (the rustflag appears on the build-script rustc lines in the `-v` log). This covers R1 for
    local builds.
  - The CI darwin legs pass `--target`. Then the rustflag reaches only the target units, not the
    host build scripts and proc macros. That is acceptable while CI links with SDK 26.5.

  Rejected:
  - `[profile.release.package.<binding>] strip = "none"` in `rs/Cargo.toml`. Measured to work on
    all paths, and the sdist carries it. Rejected because of the 7.7 times Linux growth above.
  - `[tool.maturin] rustc-args` in the binding's `pyproject.toml`. Measured to work. Rejected: it
    applies to every platform (same Linux growth) and does not cover the napi binding.
  - A workspace-wide `[profile.release] strip = "none"`. Same Linux cost, and it changes every
    service binary and container image.
  - Strip with Apple `strip` after the build. It loads, but cargo has no stable hook. Each build
    path would need its own post-step.
  - A newer Rust toolchain. No release with a known fix (§2.4).
  - A documented workaround only. A cheap repo-local fix exists.

- **D2. The sdist gap.** maturin does not ship `rs/.cargo/config.toml` in the sdist (SMA-578).
  So a user who builds the sdist on a host with SDK 27 gets a binary that does not load.
  Provisional — needs Sven (P1). The plan first measures one option: ship a `.cargo/config.toml`
  at the sdist root with `[tool.maturin] include` (`format = "sdist"`), then build from the
  extracted sdist and import. If that works, adopt it and add the D3 guard to the macOS leg of
  `sdist-verify`. If it does not work, accept the gap and record it in `py/README.md` and in the
  `rs/.cargo/config.toml` comment. The anti-goal "a fix that the sdist does not carry" is
  therefore conditional on P1.

- **D3. CI guard.** Add an assertion to the darwin steps that exist today:
  - `wheels.yml`, step "Assert Mach-O arch and minimum macOS (darwin only)": each `*.abi3.so`.
  - `prebuild.yml`, step "Verify Mach-O architecture (darwin only)": both `.node` files.

  The parse must not use a pipe into a reader that exits early (`repo:actionlint` check 13). Use
  the idiom that the minos parse already uses:
  `otool -l "$f" | awk '/^ *stroff /&&!d{v=$2; d=1} END{print v}'`. Check that the value has only
  digits with `case`. Fail when the value is empty, not numeric, or `stroff mod 8 ≠ 0`. The error
  names SMA-767. The guard is needed because the runner's binaries carry `sdk 26.5`, so dyld on
  the runner does not check, and the import step cannot catch a regression.

  P3 (provisional — needs Sven): check `stroff` only. If more fields are checked later, use a
  per-field rule (`stroff` and `symoff` mod 8, `LC_CODE_SIGNATURE` `dataoff` mod 16), never one
  rule for every `*off` field.

- **D4. The guard must bite.** Before the merge, run the exact D3 text from both workflows,
  statically, against:
  - the unfixed local dylib (`stroff 402708`): must fail;
  - the PyPI 0.2.0 arm64 and x86_64 wheels: must fail;
  - the npm darwin-arm64 0.2.0 `.node`: must pass;
  - a file with no `LC_SYMTAB` (for example a text file): must fail;
  - the fixed local dylib: must pass.
  Record all outputs in the PR.

- **D5. Records.**
  - A comment in `rs/.cargo/config.toml` next to the new flag: the cause, the measurement, the
    SDK gate, and the removal condition (D6).
  - One entry in `rs/CLAUDE.md`: do not remove the flag until D6 holds; do not add maturin
    `--strip`/`strip = true` or napi `--strip`, because each one runs `rust-objcopy` again; a
    "mis-aligned LINKEDIT" error on macOS means a `rust-objcopy`-stripped Mach-O with a new SDK.
  - A short note in `py/README.md`: after you pull this change, run the "After a Rust change"
    command once. The shared `py/.venv` keeps the broken binding until then, because uv's cache
    key for the binding does not include `rs/.cargo/config.toml` (inferred from uv's default cache
    keys, not measured). Also a pointer to the `rs/CLAUDE.md` entry.
  - A pointer in the wheels bullet of `.github/CLAUDE.md`, next to the other Mach-O assertions.

- **D6. Removal condition.** Remove the flag only when the pinned toolchain's `rust-objcopy` pads
  the string pool. Test it on a dylib with an odd indirect symbol count:
  `stroff > indirectsymoff + 4 × nindirectsyms`. The D3 guard alone cannot prove it, because an
  even count passes by chance.

- **D7. No version bump and no re-release.** Provisional — needs Sven (P4). The published 0.2.0
  wheels load on macOS 27 today (inferred, §2.4). The next normal release ships aligned wheels.
  The commit type is `fix(rs)`. `rs/.cargo/config.toml` is outside every crate directory, so
  release-plz probably sees no package change (inferred; the plan must confirm it).

- **D8. Rollback.** Revert D1 and D3 together. A revert of D1 alone makes the D3 guard red on
  the next odd count.

## 4. Goals and non-goals

Goals:
- `moon run paigasus-kernel-py:test` passes on the development Mac (macOS 27.2).
- Local release builds of both bindings load on this Mac.
- CI fails when a darwin binding has a mis-aligned string pool, before the runner SDK moves to 27.

Non-goals:
- A fix for `rust-objcopy`, or a report upstream.
- A change to Linux or Windows artefacts, service binaries, container images, or the wasm binding.
- A local Moon gate for this alignment. The local import already fails loudly.

Anti-goals:
- A fix that grows the Linux wheels or prebuilds.
- A fix that works only through maturin and leaves `cargo build --release` broken.

## 5. Affected-graph cost

`rs/.cargo/config.toml` is a task input for 61 tasks (`rs/CLAUDE.md`, A10). The PR selects them in
`moon ci`. Both `wheels.yml` and `prebuild.yml` list `rs/.cargo/config.toml` in their
`pull_request` paths, so the PR runs both guards before the merge. `images.yml` does not list it.
The `.github/workflows/*` edits select `repo:actionlint` and the other workflow gates.

## 6. Verification for the plan

1. `moon run paigasus-kernel-py:test --force` on the Mac: rc 0.
2. `napi build --platform --release` for the napi crate on the Mac: `stroff` must be the
   unstripped value (503056 at the time of writing), not 470128. The count is even, so alignment
   alone cannot show the effect.
3. D4, all five cases.
4. The "After a Rust change" command from `py/README.md`, then `moon run py:test` on the Mac: rc 0.
5. If P1 adopts the sdist option: a wheel built from the extracted sdist outside the repo loads.
   Read its build log for cargo warnings.
6. Read the CI artefact sizes from this PR's `wheels.yml` and `prebuild.yml` runs. Linux and
   Windows sizes must not change against `main`.
7. `nm -ap <CI darwin .so> | grep OSO`: record the paths (R3).
8. The full gate graph from the root `CLAUDE.md` before the push. Read the gate notes on bash
   versions for this Mac.

## 7. Provisional calls — need Sven

- **P1.** The sdist gap: try to ship `.cargo/config.toml` in the sdist; if that fails, accept and
  document the gap.
- **P2.** The fix is a macOS-only rustflag in `rs/.cargo/config.toml`, not a profile override, to
  keep Linux artefacts at their current size. It covers the napi binding too.
- **P3.** The CI guard checks `stroff` only.
- **P4.** No out-of-band re-release of 0.2.0.

## 8. Risks and residuals

- **R1.** Proc macros and build scripts in a release build with `--target` are still stripped by
  `rust-objcopy`. rustc `dlopen`s the proc macros. Today all of them are aligned (2 proc-macro
  dylibs, 8 build scripts, measured). The CI darwin legs use `--target` and SDK 26.5, so they are
  not at risk today. Local builds without `--target` are covered by D1.
- **R2.** The guard runs in `wheels.yml` and `prebuild.yml`. A PR that edits only
  `rs/.cargo/config.toml` runs both before the merge. A PR that edits only binding source does
  not, but `release.yml` calls both workflows, and `approve-release` needs them. So every publish
  is gated.
- **R3.** The unstripped darwin `.so` keeps 4 N_OSO debug-map entries. Each holds the absolute
  path of a `std` rlib under the builder's rustup home (locally `/Users/<home>/.rustup/...`; in CI
  `/Users/runner/...`). No personal data ships from CI. The build is less reproducible.
- **R4.** A user who installs the sdist on a host with SDK 27 (for example PyPy) gets a broken
  binary, unless P1 closes the gap.
- **R5.** The shared `py/.venv` of every Mac checkout keeps the broken binding until the README
  command runs (D5).

## 9. Measurement scripts

The scripts lived in a scratch directory under `$TMPDIR` (`sma767.*`). They are not in the repo.

- `matrix.sh`: cargo build per row (`CARGO_PROFILE_RELEASE_STRIP`, `MACOSX_DEPLOYMENT_TARGET`),
  copy the dylib to `paigasus_py_bindings.abi3.so`, read `stroff` with `otool -l`, import it.
- `manual.sh`: run Apple `strip` and `rust-objcopy` by hand on the unstripped dylib, then import.
- `wrap.sh`: `cp -Rc` the toolchain, replace `rust-objcopy` with a logger, build with `RUSTC` set.
- `sdk.sh`: `vtool -set-build-version macos 11.0 <sdk> -replace`, `codesign -f -s -`, import.
- `rflag.sh`: `CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS` (the env form of the config key)
  with `-C strip=none` appended, maturin build, import.
- `linux.sh`: in `rust:1.95.0-bookworm`, release build with `strip` `debuginfo` and `none`, size.
- `fix.sh`, `sdist.sh`, `moontask.sh`: apply each profile-based candidate, build, import, restore
  the file with `git checkout --`.
