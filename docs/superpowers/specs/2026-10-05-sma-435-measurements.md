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
binaries with `[parse exception: compact imports not supported (at 0:219)]`. The cause:
`--all-features` also enables `compact-imports`, so the stripped copy used a compact import
section that the later `--mvp-features` run cannot read. The fix: the strip step uses
`--mvp-features` plus the eight declared `--enable-*` flags instead. Both controls then held.

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

The `-O` binary needs no feature that the raw binary does not need. Both binaries need the same
four features: `nontrapping-float-to-int`, `sign-ext`, `reference-types` and `multivalue`. The
list is the same for both, so the `-O` pass adds no browser-floor requirement. This result is
limited by the method limit above.

## Verdict of Task 1

- S1: PASS. The checksum is verified, the binary is static, and it runs on Linux x86_64.
- S2a: PASS. Three Moon commands gave rc 0, the project count stayed 35, and no `wasm-opt` install started.
- S2b: PASS (recorded fact). The first install rewrites `shims/registry.json` only. No other tool shim changed.
- S4a: PASS. The install and the run work as uid 65534 with no capabilities.
- S5: PASS, with the method limit. The `-O` binary uses no feature that the raw binary does not use.
