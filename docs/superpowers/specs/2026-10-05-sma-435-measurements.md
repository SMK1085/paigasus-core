# SMA-435 — Measurements

This file records the spikes of the SMA-435 plan
(`docs/superpowers/plans/2026-10-05-sma-435-pin-binaryen-wasm-opt.md`, Task 1 and Task 7) for the
design `docs/superpowers/specs/2026-10-04-sma-435-pin-binaryen-wasm-opt-design.md`. Each section
states the command, the result and the date.

**Tool versions:** proto 0.61.1 (`.prototools`), Moon 2.5.3, binaryen `version_133` (the nested pin
`rs/crates/bindings/paigasus-wasm/.prototools`), wasm-pack 0.15.0, Docker image `ubuntu:24.04`
and `LOCKSTEP_IMAGE` of `.github/workflows/wasm-lockstep.yml`.

---

## S1 — binaryen on Linux x86_64 (plain ubuntu:24.04)

**Date:** 2026-10-05. **Command:** `$SP/s1.sh` (plan Task 1 Step 4).

| Item | Result |
| -- | -- |
| Machine | `== machine: x86_64` |
| Root resolves `wasm-opt` | No. `Error: proto::tool::unknown_id` (`wasm-opt is not a built-in plugin and has not been configured with [plugins] in a .prototools file`). The script printed `root does not resolve wasm-opt: expected`. |
| Checksum line(s) | `[wasm-opt] Verifying checksum against binaryen-version_133-x86_64-linux.tar.gz.sha256` and `[DEBUG 06:27:30.318] proto_core::flow::install  Successfully verified, checksum matches  tool="wasm-opt"` |
| `wasm-opt --version` | `wasm-opt version 133 (version_133)` |
| `ldd bin/wasm-opt` | `statically linked` |
| Runs without `lib/` | yes (`wasm-opt version 133 (version_133)` and `runs without lib/: yes`) |
| Sizes: raw, `-O` | 50902 bytes, 35471 bytes |

The binary path was `.proto/tools/wasm-opt/133.0.0/binaryen-version_133/bin/wasm-opt`. The `-O`
size matches the expected 35,471 bytes.

## S2 — Moon, the proto shims and the nested `.prototools` (macOS arm64)

**Date:** 2026-10-05.

**S2a. Command:** `$SP/s2a.sh`, then `$SP/count-projects.sh after`.

| Item | Result |
| -- | -- |
| proto, Moon | `proto 0.61.1`, `moon 2.5.3` |
| `moon setup` | rc=0 |
| `moon run paigasus-kernel-ts:test --force` | rc=0 |
| `moon run paigasus-wasm-rs:test paigasus-wasm-rs:lint --force` | rc=0 |
| `moon query projects` baseline, after | rc=0 and 35 projects, rc=0 and 35 projects |
| `BEFORE` | `/Users/smaschek/.proto/tools/wasm-opt absent` |
| `AFTER` | `/Users/smaschek/.proto/tools/wasm-opt absent` |

The project count is 35, not the 33 that the root `CLAUDE.md` records. The count did not change
between the two measurements, so the finding for S2 is not affected. The root `CLAUDE.md` figure
is out of date.

The `grep -i -E 'wasm-opt|binaryen'` over the Moon logs found 13 lines. Every line is a cargo
`Compiling` or `Checking` line or a vitest `RUN` line. Each one matches only because the
worktree path contains `sma-435-binaryen`. No line is a `wasm-opt` or binaryen install, a download or a wasm-pack skip
message. Example (verbatim):
`paigasus-kernel-ts:test |    Compiling paigasus-wasm v0.2.0 (/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-435-binaryen/rs/crates/bindings/paigasus-wasm)`.

**S2b. Command:** `$SP/s2b.sh`. The isolated `PROTO_HOME` first held a `wasm-pack` install, then
the first `proto install wasm-opt` ran. Diff of shims and bins
(`inode mtime sha256-prefix path`), before to after:

```
1c1
< 172978164 1791181703 dde7a887eba84afc shims/registry.json
---
> 172978216 1791181705 e189c0a63add7e26 shims/registry.json
2a3
> 172978215 1791181705 e8920f1d662751fb shims/wasm-opt
3a5,7
> 172978217 1791181705 81041e09f332df94 bin/wasm-opt
> 172978218 1791181705 81041e09f332df94 bin/wasm-opt-133
> 172978219 1791181705 81041e09f332df94 bin/wasm-opt-133.0
```

The install printed `wasm-opt version 133 (version_133)`. The first `proto install wasm-opt`
adds the `wasm-opt` shim and three `wasm-opt` bin entries. It also rewrites `shims/registry.json`
(new inode, new hash). It does not change the `wasm-pack` shim or the `wasm-pack` bins
(inode, mtime and hash are the same). `shims/registry.lock` is also the same. So no OTHER tool
shim changes. Only the shared registry file changes.

## S4a — binaryen in the wasm-lockstep container (nobody, no capabilities)

**Date:** 2026-10-05. **Command:** `$SP/s4a.sh`, then `$SP/docker-wait.sh sma435-s4a ...`. The
container used `--cap-drop=ALL --security-opt=no-new-privileges --user 65534:65534` and a Docker
volume with uid 65534 ownership.

| Item | Result |
| -- | -- |
| Container exit code | `container sma435-s4a exited 0` |
| id line | `== id: uid=65534(nobody) gid=65534(nogroup) groups=65534(nogroup)` |
| proto version | `== proto: proto 0.61.1` |
| Version line | `== version: wasm-opt version 133 (version_133)` |
| Sizes: raw, `-O` | `== sizes: raw 50902, -O 35471` |

The sizes are the same as in S1. S4b (the full `container.sh build` and the `optimize-wasm`
line) runs in Task 7.

## S5 — features each binary uses

**Date:** 2026-10-05. **Command:** `$SP/s5.sh`. The tool was the isolated binaryen 133 of S2b.
The input was the committed `paigasus_wasm_bg.wasm` (raw, 50902 bytes) and its `-O` output
(35471 bytes).

**Deviation from the plan script, one line.** The first run used `--all-features
--strip-target-features` to make the stripped copy. It failed the `all-declared` control for both
binaries with `[parse exception: compact imports not supported (at 0:219)]`. Fix round 1 repeated
it on `raw.wasm`. `wasm-opt raw.wasm --all-features --strip-target-features -o chk-nf.wasm` gave
rc=0. Validating `chk-nf.wasm` with `--mvp-features` and the eight `--enable-*` flags gave
`[parse exception: compact imports not supported (at 0:219)]` and `Fatal: error parsing wasm`,
rc=1. The same validation with `--enable-compact-imports` added gave rc=0. So `--all-features`
makes the output use compact imports, and a reader without that feature cannot parse them. The
fix: the strip step uses `--mvp-features` plus the eight declared `--enable-*` flags instead.
Both controls then held.

**Method limit (coordinator ruling).** The leave-one-out method cannot separate implied
features. A feature that another enabled feature implies stays on when the method switches it
off. So `unused` for `bulk-memory` and `bulk-memory-opt` (and, by the same logic, other implied
features such as `mutable-globals`) means "validation did not need this flag alone", not "the
binary does not use this feature". `USED` is reliable. `unused` is not.

Control lines:

```
control raw all-declared: valid
control raw mvp-only: invalid (expected)
control opt all-declared: valid
control opt mvp-only: invalid (expected)
```

Table:

```
== raw:    50902 bytes
raw mutable-globals unused
raw nontrapping-float-to-int USED
raw bulk-memory unused
raw sign-ext USED
raw reference-types USED
raw multivalue USED
raw bulk-memory-opt unused
raw call-indirect-overlong unused
== opt:    35471 bytes
opt mutable-globals unused
opt nontrapping-float-to-int USED
opt bulk-memory unused
opt sign-ext USED
opt reference-types USED
opt multivalue USED
opt bulk-memory-opt unused
opt call-indirect-overlong unused
```

Among the features that the method can detect, the `-O` binary needs no feature that the raw
binary does not need. Both binaries need the same four: `nontrapping-float-to-int`, `sign-ext`,
`reference-types` and `multivalue`. The method cannot see implied features. These are
`bulk-memory`, `bulk-memory-opt`, `call-indirect-overlong` and `mutable-globals`. The table does
not prove that `-O` adds none of them.

**Direct check of the `target_features` section (fix round 1).** Command: `$SP/s5b.sh`, which
runs `$SP/tf.mjs`. The script reads `WebAssembly.Module.customSections(m, 'target_features')` of
`s5/raw.wasm` and `s5/opt.wasm` and decodes it. Output (the hex is the section payload):

```
raw 8 +bulk-memory +bulk-memory-opt +call-indirect-overlong +multivalue +mutable-globals +nontrapping-fptoint +reference-types +sign-ext
opt 8 +mutable-globals +nontrapping-fptoint +bulk-memory +sign-ext +reference-types +multivalue +bulk-memory-opt +call-indirect-overlong
```

Both binaries declare the same eight features. The order differs, so the section bytes are not
identical. The set is identical. This backs the phrase "eight declared features". It also shows
that `-O` adds no declared feature, including the implied ones. The section declares what the
tool wrote. It does not prove what the code uses, but `-O` adds no entry to it.

## Verdict of Task 1

- S1: PASS. The checksum is verified, the binary is static, and it runs on Linux x86_64.
- S2a: PASS. Three Moon commands gave rc 0, the project count stayed 35, and no `wasm-opt` install started.
- S2b: PASS (recorded fact). The first install rewrites `shims/registry.json` only. No other tool shim changed.
- S4a: PASS. The install and the run work as uid 65534 with no capabilities.
- S5: PASS, with the method limit. Among the detectable features, `-O` adds none. The
  `target_features` sections of both binaries list the same eight features.

## AC 7 — affected selection

Command: `moon query tasks --affected`, with the file `ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs`
on stdin. The list holds one target for each `tasks[project][task]`. Moon 2.5.3, Task 2.

Before the `test` input exists (the script file was not yet in the tree):

```
paigasus-kernel-ts:generate-wasm
repo:actionlint
repo:input-liveness
repo:next-public-free
ts:fmt
ts:lint
```

`paigasus-kernel-ts:test` is not in the list.

After the line `- 'scripts/optimize-wasm.mjs'` is in the `test` inputs of `moon.yml`:

```
paigasus-kernel-ts:generate-wasm
paigasus-kernel-ts:test
repo:actionlint
repo:input-liveness
repo:next-public-free
ts:fmt
ts:lint
```

`paigasus-kernel-ts:test` is now in the list. An edit to the script alone selects the test task.

## Task 3 — regeneration and check 5

Output of `moon run paigasus-kernel-ts:generate-wasm` (`stdout.log`):

```text
generate-wasm: wasm-pack 0.15.0, sources touched
optimize-wasm: /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-435-binaryen/rs/crates/bindings/paigasus-wasm/.wasmpack-regen-out/paigasus_wasm_bg.wasm: 50950 -> 35560 bytes, wasm-opt version 133, flags -O, proto 0.60.2
generate-wasm: wrote 5 files into rs/crates/bindings/paigasus-wasm/
generate-wasm: commit all five, and run `rm -rf ts/node_modules && pnpm -C ts install` if a git operation replaced them
```

The log holds four lines, not three. The last line is the existing reminder of the task. The `--post` identity check passed. The committed binary is 50902 bytes before and 35560 bytes after. The four glue files did not change. The local proto binary is 0.60.2.

Check 5 failed on the raw binary (`has 0 paigasus.wasm-opt sections, not 1`). It passed after the regeneration, together with check 2 (390 of 390 tests).

A change of `rs/crates/bindings/paigasus-wasm/.prototools` selects these tasks:

```text
paigasus-kernel-ts:generate-wasm
paigasus-kernel-ts:test
repo:actionlint
repo:input-liveness
repo:publish-metadata
```

## Mutation battery (Task 7)

**Date:** 2026-10-05. Command for the rows 2, 3, 5 and 6: `moon run paigasus-kernel-ts:test`, with the proto shims first on `PATH`. Each undo was the exact reverse edit. The tree was clean after every undo.

| # | Edit | Observed red | Undo |
| -- | -- | -- | -- |
| 3 | `wasm-opt = ['-O', '-g']` in the release profile of `paigasus-wasm/Cargo.toml` | Check 7 failed: `check 7: Cargo.toml keeps wasm-opt = false in the wasm-pack release profile`. Check 6 passed. 1 failed, 392 passed. | `wasm-opt = false` |
| 2 | Delete `wasm-opt = false` from the release profile | Check 6 failed: `check 6: the fresh build is a raw wasm-pack output` (`The fresh build has no name section, so wasm-pack optimized it`). Check 7 failed. 2 failed, 391 passed. | Insert `wasm-opt = false` again |
| 4 | Delete the step-8 block of `optimize-wasm.mjs` | `vitest run tests/optimize-wasm.test.ts` failed 3 tests: `the output keeps a name section`, `the output is not valid wasm`, `the output drops an export`. 40 passed. | Insert the block again |
| 5 | Delete the version comparison of `resolveWasmOpt` (the `if (version.status !== 0 \|\| actual !== expected)` block) | Moon ran the task (hash `d9e7b9a5`, not a cache hit). It failed `exits 1 on another wasm-opt version and names both`. 1 failed, 392 passed. | Insert the block again |
| 6 | Pin `wasm-opt = "132.0.0"` in the nested `.prototools` | Check 5 failed: `paigasus_wasm_bg.wasm carries "binaryen=133;flags=-O", and .../.prototools demands "binaryen=132;flags=-O"`. 1 failed, 392 passed. | `wasm-opt = "133.0.0"` |
| 1 | Delete the `optimize-wasm.mjs optimize` line of `generate-wasm`, run `generate-wasm` | Binary 50950 bytes, no marker. Check 5 failed: `paigasus_wasm_bg.wasm has 0 paigasus.wasm-opt sections, not 1`. | Insert the line, run `generate-wasm` again |

Mutation 1, after the undo and the second `generate-wasm`: the log line was `optimize-wasm: ...: 50950 -> 35560 bytes, wasm-opt version 133, flags -O, proto 0.60.2`. `git status --porcelain` was empty. The five files are byte-identical to HEAD, so the regeneration is deterministic on this host. Then `moon run paigasus-kernel-ts:test` passed (16 files, 393 tests).

### Which wasm-opt wasm-pack ran (mutations 2 and 3)

In both mutations the wasm-pack log said `found wasm-opt at "/Users/smaschek/.proto/shims/wasm-opt"`. So wasm-pack used the pinned binary through the proto shim on `PATH`. It did not download binaryen. This holds because the proto shim was on `PATH`. A host without the shim on `PATH` was not measured. There, wasm-pack may download its own binaryen. The `~/Library/Caches/.wasm-pack` listing was the same before and after both runs: five `wasm-bindgen-cargo-install-*` entries (0.2.125 to 0.2.129) and no `wasm-opt-*` entry. Nothing needed removal.

This means that, on a host with the shim on `PATH`, check 7 is the only guard that sees mutation 3. Check 6 passes there, because `-g` keeps the `name` section. In mutation 2, check 6 fails because the shim `wasm-opt` removes the `name` section (flags not recorded).

## S4b — the full container run

**Date:** 2026-10-05. Command: `bash ci/wasm-lockstep/container.sh build` in the lockstep image, with the flags of `wasm-lockstep.yml` (`--cap-drop=ALL --security-opt=no-new-privileges --user 65534:65534`) plus `--platform linux/amd64`. The input was a `git archive` of HEAD, owned by uid 65534. The container exited 0.

```text
generate-wasm: wasm-pack 0.15.0, sources touched
optimize-wasm: /work/rs/crates/bindings/paigasus-wasm/.wasmpack-regen-out/paigasus_wasm_bg.wasm: 50902 -> 35512 bytes, wasm-opt version 133, flags -O, proto 0.60.2
generate-wasm: wrote 5 files into rs/crates/bindings/paigasus-wasm/
glue paigasus_wasm.js: identical
glue paigasus_wasm_bg.js: identical
glue paigasus_wasm.d.ts: identical
glue paigasus_wasm_bg.wasm.d.ts: identical
binary: 35512 bytes (Linux) vs 35560 bytes (committed)
optimize-wasm: .../paigasus_wasm_bg.wasm carries binaryen=133;flags=-O
verify rc=0
```

The Linux binary is 48 bytes smaller than the macOS binary. The raw input is 50902 bytes on Linux and 50950 bytes on macOS. The four glue files are byte-identical on both hosts. The container needs network access for the proto install of binaryen. S4b passes.

## Local gate

**Date:** 2026-10-05. Command: the `ci-targets` list of the root `CLAUDE.md` with `--base origin/main --include-relations`, in one `moon ci` call. PATH had a shim directory with only `bash -> /bin/bash` (bash 3.2.57) first. `PROTO_REPORTER=text` was set. Result: 67 completed (8 cached), 5 failed. All five failures are bash-version artifacts of this host. Each gate passed when run directly under Homebrew bash 5.3.20.

| Gate | Moon result under bash 3.2 | Direct result under bash 5.3.20 |
| -- | -- | -- |
| `repo:version-lockstep` | fail: `line 62: kernel: unbound variable`. The script declares associative arrays with `declare -A` at `ci/version-lockstep/run.sh:62` and `:73`. Bash 3.2 lacks `declare -A`. | rc 0, `all 20 version-lockstep sites agree` |
| `repo:publish-metadata` | fail: `declare: -A: invalid option` | rc 0, `all checks passed` |
| `repo:ruff-ci` | fail: `mapfile: command not found` | rc 0, `15 files clean` |
| `repo:next-public-free` | fail: 7 self-test rows, `expected rc 0, got 1` | rc 0, `763 tracked ts/ files free of NEXT_PUBLIC_` |
| `repo:actionlint` | fail: two false `cargo-lock-step` rows | rc 0, preflight `pipe capacity 65536 bytes (floor 8192)` |

Every other task passed, among them `repo:affected-smoke` (31 s), `repo:wasm-lockstep` (cached), `repo:input-liveness`, `paigasus-kernel-ts:test`, `paigasus-wasm-rs:test`, both `test-e2e` tasks of the consoles and `paigasus-console-core-ts:test-e2e`.
