# SMA-435 pin binaryen `wasm-opt` and optimize the wasm kernel Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Pin binaryen's `wasm-opt` through a vendored proto plugin and a nested `.prototools`, and optimize the committed and the published wasm kernel binary with `wasm-opt -O` from that pin.

**Architecture:** wasm-pack keeps `wasm-opt = false`. A new Node script, `ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs`, installs the binaryen that `rs/crates/bindings/paigasus-wasm/.prototools` pins, runs `wasm-opt -O` on wasm-pack's output, and appends a `paigasus.wasm-opt` marker section. `generate-wasm` (the committed binary) and the `assemble` job of `prebuild.yml` (the published binary) call it. Three new drift-gate checks (5, 6, 7) in `tests/committed-wasm.test.ts` hold the committed binary, the fresh raw build and `Cargo.toml` to the design.

**Tech Stack:** Node 24.16.0 (ESM, standard library only), vitest 4, TypeScript 6 (`tsc --noEmit`), proto 0.61.1 TOML plugin, binaryen `version_133`, wasm-pack 0.15.0, Moon 2.5.3, GitHub Actions.

**Spec:** `docs/superpowers/specs/2026-10-04-sma-435-pin-binaryen-wasm-opt-design.md` (APPROVED 2026-10-05). Read it in full before you start a task. The spec section numbers (§) below refer to it.

## Global Constraints

- Node only. No new npm dependency and no TOML parser (spec F17). Use only `node:` built-in modules.
- A plain-Node `.mjs` file under `packages/*/scripts` or `packages/*/tests` gets only the globals `process`, `URL` and `WebAssembly` from ESLint (`ts/eslint.config.js:39-40`). Import every other name (`Buffer`, and so on) from a `node:` module. Do not use `console`.
- Exit codes of the script: 1 is a finding, 2 is an infrastructure error (spec §5.4).
- SPDX header on every new source file: `// SPDX-License-Identifier: Apache-2.0` (`.mjs`, `.ts`), `# SPDX-License-Identifier: Apache-2.0` (TOML, `.prototools`, YAML, Python).
- Set `PROTO_REPORTER=text` on every `proto` call whose stdout is read, take the last non-empty line, and assert an executable regular file (SMA-596, SMA-609).
- The `wasm-pack` call stays a literal in `ts/packages/paigasus-kernel/moon.yml` (FFI_MARKERS in `ci/affected-graph/cargo_moon_parity.py`). Do not wrap it.
- Shell in a `moon.yml` `script:` must work under bash 3.2: no `mapfile`, no `declare -A`, no here-string.
- The flag list is `-O` and nothing more. No `-g` (spec §3).
- binaryen `version_133` is the proto version `133.0.0`. The pin regex is `^wasm-opt\s*=\s*"(\d+)\.0\.0"\s*$` (multiline).
- The marker: custom section name `paigasus.wasm-opt`, payload `binaryen=<major>;flags=-O` (spec §5.4.1).
- The temporary file is `<out-dir>/.optimize-wasm.tmp`. It must not start with `paigasus_wasm` (`prebuild.yml:367` stages `paigasus_wasm*`).
- No change to the root `.prototools`, to the root `CLAUDE.md`, or to the `test` task's `wasm-pack` call (spec §3).
- Commits: Conventional Commits with a workspace scope. The allowed values are in `ts/packages/commitlint-config/index.cjs:41-42`:
  `'type-enum': [2, 'always', ['feat', 'fix', 'docs', 'chore', 'refactor', 'test', 'ci', 'build', 'perf', 'style', 'revert']]` and
  `'scope-enum': [2, 'always', ['rs', 'py', 'ts', 'contracts', 'ci', 'docs', 'deps', 'release', 'repo', 'claude', 'workspace']]`.
  Header at most 100 characters. Body lines at most 100 characters. A blank line before the footer. No body line that starts with `#`.
- Every commit message ends with the line `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`.
- Never use `git commit --no-verify`. Never use `git checkout --` or `git restore` to undo a mutation: undo it with the exact reverse edit. Never use a bare `git stash`.
- If `git commit` fails with "failed to fill whole buffer", 1Password is locked. Stop and ask the coordinator to ask the user to unlock it.
- Run every command in the foreground. Do not install host software (`brew`, `pip install`, `npm -g`). A `docker run` container and a `proto install` of a tool that this repository pins are allowed.
- Write all documentation and comments in ASD-STE100 Simplified Technical English: short sentences, active voice, one idea per sentence. Keep names and code exact.
- Prettier `printWidth` is 200 in `ts/` (`ts/.prettierrc.js`). Run `ts:fmt` and `ts:lint` after every `ts/` change.

## Review Focus

These five input classes are not in the spec's test list. Each one has a test in Task 2.

1. The script is started through a symlinked path (macOS `/var` is a symlink to `/private/var`; a pnpm link is a symlink). A naive entry-point check then never runs `main()`, and the script exits 0 and does nothing. Expected: the script runs. Test: "runs when it is started through a symlinked path".
2. A killed run left `<out-dir>/.optimize-wasm.tmp`. Expected: the next run removes it and succeeds. Test: "replaces a temporary file that a killed run left behind".
3. A relative out-dir (`prebuild.yml` passes `.wasmpack-release-out` from the crate directory). Expected: it resolves against the working directory, and `wasm-opt` gets an absolute path. Test: "resolves a relative out-dir against the working directory".
4. The out-dir holds no binary (wasm-pack failed, or a typo in the path). Expected: exit 2, no temporary file. Test: "exits 2 when the out-dir holds no binary".
5. A wrong mode or a wrong argument count (a typo in `moon.yml` or `prebuild.yml`). Expected: exit 2 with a usage line, and the input is not changed. Test: "refuses the arguments … with exit 2".

---

## How to run commands in this plan

This worktree-isolated harness refuses `export PROTO_HOME=...` and `$HOME` expansion in an inline command. Put environment setup in a script file and run `/bin/bash <script>`.

- `SP` is the session scratchpad: `/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad`. If your session has another scratchpad, change the `SP=` line of each script, and nothing else.
- `WT` is the worktree: `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-435-binaryen`. Work only there.
- Task 1 Step 1 writes `$SP/run.sh`. In this plan, `RUN <command>` means: type the full command `/bin/bash /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/run.sh <command>`. `run.sh` puts the proto shims on `PATH`, sets `PROTO_REPORTER=text`, and changes to `WT`.
- Commit messages: write the message to a file in `SP` with the Write tool, then run `git add <paths>` and `git commit -F <file>` as two plain commands from `WT`.
- A Moon task with `buffer-only-failure` prints the output of a failing task only. To read a task's output, read `.moon/cache/states/<project>/<task>/stdout.log` (root `CLAUDE.md`, "Diagnosing an unattributed `moon ci` failure").
- The Bash tool stops a command after 600000 ms. For a long Docker run, this plan starts the container detached (`docker run -d`), then waits for it with `docker wait` in a separate command. If the wait times out, run the wait script again: the container keeps running.

## File map

| File | Task | Change |
|---|---|---|
| `.proto/plugins/binaryen.toml` | 1 | new: the proto plugin (spec §5.1) |
| `rs/crates/bindings/paigasus-wasm/.prototools` | 1 | new: the nested pin (spec §5.2) |
| `docs/superpowers/specs/2026-10-05-sma-435-measurements.md` | 1, 7 | new: spike and battery results |
| `ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs` | 2 | new: `optimize` and `verify`, exported helpers |
| `ts/packages/paigasus-kernel/tests/optimize-wasm.test.ts` | 2 | new: unit tests with a fixture tree and stubs |
| `ts/packages/paigasus-kernel/vitest.config.ts` | 2 | `include` entry |
| `ts/packages/paigasus-kernel/tsconfig.json` | 2 | `allowJs: true` (the tests import the `.mjs`) |
| `ts/packages/paigasus-kernel/moon.yml` | 2, 3 | `test` inputs; `generate-wasm` line, inputs, comment |
| `ts/packages/paigasus-kernel/tests/committed-wasm.test.ts` | 3, 4 | checks 5, 6, 7; header |
| `rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm` | 3 | regenerated, optimized, marked |
| `rs/crates/bindings/paigasus-wasm/Cargo.toml` | 4 | `profiling` profile line; comment |
| `.github/workflows/prebuild.yml` | 5 | optimize line; verify step; `paths:` entries |
| `rs/CLAUDE.md`, `ts/CLAUDE.md`, `contracts/CLAUDE.md` | 6 | spec §5.8 |
| `ci/wasm-lockstep/README.md`, `ci/wasm-lockstep/lockstep_check.py` | 6 | spec §5.8 (text and comment only) |

---

### Task 1: The plugin, the nested pin, and spikes S1, S2, S4a, S5

**STOP rules for this task.** Read them before Step 1.
- S1 fails (no install, no checksum verification, or `wasm-opt` does not run on Linux x86_64): stop. Report to the coordinator. Do no other task.
- S4a fails (the install or the run fails as uid 65534 with no capabilities): stop. Report. Do no other task.
- S2 fails (Moon reads the nested pin, a Moon task starts a `wasm-opt` install, or a Moon command changes its result): stop. Report that the plan must use fallback A2 (spec §4) and that the spec goes back to review. Do no other task.
- S5 finds a feature that only the optimized binary uses: stop. Report it as a browser-floor finding. Do no other task.

**Spec contradiction, resolved here.** Spec §6 S4 records "the `optimize-wasm` output line". That script does not exist in Task 1. So S4 is split: S4a (this task) installs and runs the pinned `wasm-opt` in the exact container of `wasm-lockstep.yml`. S4b (Task 7) runs the full `container.sh build` and records the `optimize-wasm` line.

**Files:**
- Create: `.proto/plugins/binaryen.toml`
- Create: `rs/crates/bindings/paigasus-wasm/.prototools`
- Create: `docs/superpowers/specs/2026-10-05-sma-435-measurements.md`
- Scratch only: `$SP/run.sh`, `$SP/count-projects.sh`, `$SP/s1.sh`, `$SP/s2a.sh`, `$SP/s2b.sh`, `$SP/s4a.sh`, `$SP/docker-wait.sh`, `$SP/s5.sh`

**Interfaces:**
- Consumes: nothing.
- Produces: the tool id `wasm-opt`; the pin `wasm-opt = "133.0.0"` in `rs/crates/bindings/paigasus-wasm/.prototools`; `$SP/run.sh` (the `RUN` prefix); `$SP/docker-wait.sh <container> <logfile>`; the isolated proto home `$SP/proto-home` with binaryen 133 installed (S5 uses it).

- [ ] **Step 1: Write the scratch helpers and record the baseline project count**

Write `$SP/run.sh`:

```bash
#!/bin/bash
# The RUN prefix of the SMA-435 plan: proto shims first, text reporter, the worktree as cwd.
set -euo pipefail
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-435-binaryen
exec "$@"
```

Write `$SP/count-projects.sh`:

```bash
#!/bin/bash
# Print the number of Moon projects. Measure the exit status of `moon query projects` unpiped.
set -uo pipefail
SP=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-435-binaryen
moon query projects > "$SP/projects-$1.json" 2> "$SP/projects-$1.err"
echo "moon query projects rc=$?"
python3 - "$SP/projects-$1.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
projects = data["projects"] if isinstance(data, dict) else data
print("project count:", len(projects))
PY
```

Write `$SP/docker-wait.sh`:

```bash
#!/bin/bash
# Wait for a detached container, save its log, remove it. Run it again if the tool call times out.
set -uo pipefail
name="$1"
log="$2"
rc="$(docker wait "$name")"
docker logs "$name" > "$log" 2>&1
docker rm "$name" > /dev/null
echo "container $name exited $rc; log: $log"
```

Run: `/bin/bash /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/count-projects.sh baseline`
Expected: `moon query projects rc=0` and `project count: 33` (root `CLAUDE.md` records 33). Write down the number.

- [ ] **Step 2: Create the plugin `.proto/plugins/binaryen.toml`**

```toml
# SPDX-License-Identifier: Apache-2.0
#
# Vendored proto TOML plugin for binaryen's `wasm-opt` (SMA-435).
#
# Resolves the official, checksummed WebAssembly/binaryen GitHub release tarballs. Same vendoring
# rationale as cargo-machete and release-plz: a static schema over official release assets.
#
# THE PIN IS NOT IN THE ROOT .prototools. It is in rs/crates/bindings/paigasus-wasm/.prototools.
# proto reads .prototools files upwards from the working directory, so the bare `proto install`
# of ci.yml at the repository root does not see the pin and does not download binaryen (111 MB on
# Linux) for a tool that no ci.yml task runs. Only ts/packages/paigasus-kernel/scripts/
# optimize-wasm.mjs uses this tool, and it runs every proto call from that crate directory.
#
# Tag convention: `version_<N>` (for example version_133), with no minor and no patch.
# version-pattern maps the tag to the semantic version <N>.0.0, and {versionMajor} builds the
# file name back. exe-path is required: the tarball nests the binary at
# binaryen-version_<N>/bin/wasm-opt. proto keeps the whole tree, so on macOS bin/wasm-opt finds
# lib/libbinaryen.dylib through @rpath.
# The arch names differ per OS: Linux assets use x86_64 and aarch64, macOS assets use x86_64 and
# arm64. So the macOS block remaps aarch64 to arm64. There is no Windows block: no job and no
# developer runs this tool on Windows.

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

- [ ] **Step 3: Create the nested pin `rs/crates/bindings/paigasus-wasm/.prototools`**

```toml
# SPDX-License-Identifier: Apache-2.0
#
# binaryen's `wasm-opt`, pinned for the wasm kernel only (SMA-435). The pin is HERE and not in the
# root .prototools. proto reads .prototools files upwards from the working directory, so the bare
# `proto install` of ci.yml at the repository root never reads this file. CI then does not
# download the 111 MB Linux tarball for a tool that no ci.yml task runs. The `file://` path below
# is relative to THIS file, not to the working directory.
#
# ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs runs every proto call from this directory
# and reads the version line below. To bump binaryen: change the version, run
# `moon run paigasus-kernel-ts:generate-wasm`, and commit the five artifacts. Check 5 of
# ts/packages/paigasus-kernel/tests/committed-wasm.test.ts fails until you do.
wasm-opt = "133.0.0"

[plugins]
wasm-opt = "file://../../../../.proto/plugins/binaryen.toml"
```

- [ ] **Step 4: Spike S1 — binaryen on plain Linux x86_64**

Write `$SP/s1.sh`:

```bash
#!/bin/bash
# S1 (SMA-435 spec § 6): proto 0.61.1 installs the pinned binaryen on Linux x86_64.
set -euo pipefail
WT=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-435-binaryen
SP=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad
rm -rf "$SP/s1" && mkdir -p "$SP/s1/repo"
git -C "$WT" archive HEAD | tar -x -C "$SP/s1/repo"
# The two new files are not committed yet. Copy them over the archive.
cp "$WT/.proto/plugins/binaryen.toml" "$SP/s1/repo/.proto/plugins/binaryen.toml"
cp "$WT/rs/crates/bindings/paigasus-wasm/.prototools" "$SP/s1/repo/rs/crates/bindings/paigasus-wasm/.prototools"
cat > "$SP/s1/inner.sh" <<'EOF'
#!/bin/bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y -qq --no-install-recommends curl ca-certificates git xz-utils gzip unzip > /dev/null
echo "== machine: $(uname -m)"
export HOME=/tmp/s1-home
mkdir -p "$HOME"
export PROTO_REPORTER=text
curl -fsSL https://moonrepo.dev/install/proto.sh -o "$HOME/proto-install.sh"
bash "$HOME/proto-install.sh" 0.61.1 --yes --no-profile
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
echo "== proto: $(proto --version)"
echo "== at the repository root, proto must NOT know wasm-opt"
if (cd /s1/repo && proto --reporter text bin wasm-opt); then echo "ROOT RESOLVED wasm-opt: UNEXPECTED"; else echo "root does not resolve wasm-opt: expected"; fi
cd /s1/repo/rs/crates/bindings/paigasus-wasm
PROTO_LOG=debug proto install wasm-opt > /s1/install.log 2>&1
echo "== checksum lines of the install log"
grep -i -E 'checksum|sha256|verif' /s1/install.log || echo "no checksum line in the debug log"
bin="$(proto --reporter text bin wasm-opt | tail -n1)"
echo "== bin: $bin"
[ -f "$bin" ] && [ -x "$bin" ]
echo "== version: $("$bin" --version)"
echo "== ldd"
ldd "$bin" || true
lib="$(dirname "$bin")/../lib"
if [ -d "$lib" ]; then
  mv "$lib" "$lib.off"
  if "$bin" --version; then echo "runs without lib/: yes"; else echo "runs without lib/: NO"; fi
  mv "$lib.off" "$lib"
else
  echo "no lib/ next to bin/"
fi
"$bin" paigasus_wasm_bg.wasm -O -o /tmp/opt.wasm
echo "== sizes: raw $(wc -c < paigasus_wasm_bg.wasm), -O $(wc -c < /tmp/opt.wasm)"
EOF
docker run --rm --platform linux/amd64 --volume "$SP/s1:/s1" --workdir /s1/repo ubuntu:24.04 bash /s1/inner.sh 2>&1 | tee "$SP/s1/result.log"
```

Run (timeout 600000): `/bin/bash /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/s1.sh`
Expected: `== machine: x86_64`; `root does not resolve wasm-opt: expected`; a checksum line from proto's debug log; `== version: wasm-opt version 133 (version_133)`; an `ldd` list with no `libbinaryen` entry (or the line `not a dynamic executable`); `runs without lib/: yes`; a `-O` size near 35,471 bytes.
Record in the measurements file: the checksum line(s) verbatim, the version line, the `ldd` output, the `lib/` result, both sizes. If the install or the version line fails: apply the S1 STOP rule.

- [ ] **Step 5: Spike S2a — Moon, the proto shims and the nested file, on this Mac**

Write `$SP/s2a.sh`:

```bash
#!/bin/bash
# S2a (SMA-435 spec § 6): Moon must not read the nested .prototools as a pin, and no Moon command
# may start a wasm-opt install. Not -e: every rc is recorded.
set -uo pipefail
WT=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-435-binaryen
SP=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
cd "$WT"
out="$SP/s2a"
rm -rf "$out" && mkdir -p "$out"
tools="$HOME/.proto/tools/wasm-opt"
if [ -e "$tools" ]; then echo "BEFORE: $tools exists"; ls -la "$tools"; else echo "BEFORE: $tools absent"; fi
echo "proto: $(proto --version)"
echo "moon: $(moon --version)"
moon setup > "$out/setup.log" 2>&1; echo "moon setup rc=$?"
moon run paigasus-kernel-ts:test --force > "$out/kernel-test.log" 2>&1; echo "paigasus-kernel-ts:test rc=$?"
moon run paigasus-wasm-rs:test paigasus-wasm-rs:lint --force > "$out/wasm-rs.log" 2>&1; echo "paigasus-wasm-rs:test+lint rc=$?"
grep -n -i -E 'wasm-opt|binaryen' "$out"/*.log || echo "no wasm-opt or binaryen line in any Moon log"
if [ -e "$tools" ]; then echo "AFTER: $tools exists"; ls -la "$tools"; else echo "AFTER: $tools absent"; fi
```

Run (timeout 600000): `/bin/bash /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/s2a.sh`
Then run: `/bin/bash /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/count-projects.sh after`
Expected: all three Moon rc values are 0; the project count equals the Step 1 baseline; `AFTER` equals `BEFORE` (absent stays absent; if it existed before, the listing does not change). A wasm-pack line that says it skips `wasm-opt` is expected. A proto install line or a binaryen download line is a failure.
If the Bash tool times out, run the script again: the second run starts from a warm cache.
Record: the proto and Moon versions, the three rc values, the two project counts, the `BEFORE` and `AFTER` lines, every `grep` hit verbatim. If any expectation fails: apply the S2 STOP rule.

- [ ] **Step 6: Spike S2b — does the first `proto install wasm-opt` rewrite other shims? (isolated proto home)**

Write `$SP/s2b.sh`:

```bash
#!/bin/bash
# S2b (SMA-435 spec § 6, challenge question): record every shim and bin entry before and after the
# first `proto install wasm-opt`, in an isolated PROTO_HOME. ci/CLAUDE.md records a shim EACCES
# under concurrency (SMA-592), so a rewrite of OTHER shims matters.
set -euo pipefail
WT=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-435-binaryen
SP=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad
P="$HOME/.proto/bin/proto"
export PROTO_HOME="$SP/proto-home"
export PROTO_REPORTER=text
export PATH="$PROTO_HOME/shims:$PROTO_HOME/bin:/usr/bin:/bin:/usr/sbin:/sbin"
rm -rf "$PROTO_HOME" && mkdir -p "$PROTO_HOME"
echo "proto: $("$P" --version)"
cd "$WT"
"$P" install wasm-pack > "$SP/s2b-wasm-pack.log" 2>&1
snapshot() {
  for f in "$PROTO_HOME"/shims/* "$PROTO_HOME"/bin/*; do
    [ -e "$f" ] || continue
    printf '%s %s %s\n' "$(stat -f '%i %m' "$f")" "$(shasum -a 256 "$f" | cut -c1-16)" "${f#$PROTO_HOME/}"
  done
}
snapshot > "$SP/s2b-before.txt"
cd "$WT/rs/crates/bindings/paigasus-wasm"
"$P" install wasm-opt > "$SP/s2b-wasm-opt.log" 2>&1
snapshot > "$SP/s2b-after.txt"
echo "== shims and bins (inode mtime sha256-prefix path): before -> after"
diff "$SP/s2b-before.txt" "$SP/s2b-after.txt" || true
bin="$("$P" --reporter text bin wasm-opt | tail -n1)"
[ -f "$bin" ] && [ -x "$bin" ]
echo "wasm-opt: $bin"
"$bin" --version
```

Run: `/bin/bash /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/s2b.sh`
Expected: the install succeeds; `wasm-opt version 133 (version_133)`. The `diff` shows new `wasm-opt` entries. Record whether any OTHER line changed (inode, mtime or hash). That is a fact to record, not a STOP condition.

- [ ] **Step 7: Spike S4a — the pinned binaryen in the wasm-lockstep build container**

The flags are the flags of `.github/workflows/wasm-lockstep.yml:138` plus `--platform linux/amd64`. The image is `LOCKSTEP_IMAGE` from `wasm-lockstep.yml:43`. A Docker volume (not a bind mount) holds the work copy, so the uid 65534 ownership is real: on macOS a bind mount hides ownership (memory: "macOS bind mount hides ownership").

Write `$SP/s4a.sh`:

```bash
#!/bin/bash
# S4a (SMA-435 spec § 6, part 1): install and run the pinned wasm-opt as nobody (65534), with no
# capabilities, in the wasm-lockstep image. Starts the container detached; docker-wait.sh waits.
set -euo pipefail
WT=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-435-binaryen
SP=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad
IMAGE='docker.io/library/rust:1.95.0-bookworm@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1'
VOL=sma435-s4a
rm -rf "$SP/s4a" && mkdir -p "$SP/s4a/src"
git -C "$WT" archive HEAD | tar -x -C "$SP/s4a/src"
cp "$WT/.proto/plugins/binaryen.toml" "$SP/s4a/src/.proto/plugins/binaryen.toml"
cp "$WT/rs/crates/bindings/paigasus-wasm/.prototools" "$SP/s4a/src/rs/crates/bindings/paigasus-wasm/.prototools"
cat > "$SP/s4a/inner.sh" <<'EOF'
#!/bin/bash
# The proto lines of ci/wasm-lockstep/container.sh `build`, then the nested install.
set -euo pipefail
export HOME=/tmp/lockstep-home
mkdir -p "$HOME"
cd /work
export PROTO_REPORTER=text
proto_version="$(sed -n 's/^proto = "\([0-9][0-9.]*\)"$/\1/p' .prototools)"
curl -fsSL https://moonrepo.dev/install/proto.sh -o "$HOME/proto-install.sh"
bash "$HOME/proto-install.sh" "$proto_version" --yes --no-profile
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
echo "== id: $(id)"
echo "== proto: $(proto --version)"
cd rs/crates/bindings/paigasus-wasm
proto install wasm-opt
bin="$(proto --reporter text bin wasm-opt | tail -n1)"
[ -f "$bin" ] && [ -x "$bin" ]
echo "== bin: $bin"
echo "== version: $("$bin" --version)"
"$bin" paigasus_wasm_bg.wasm -O -o "$HOME/opt.wasm"
echo "== sizes: raw $(wc -c < paigasus_wasm_bg.wasm), -O $(wc -c < "$HOME/opt.wasm")"
EOF
docker volume rm -f "$VOL" > /dev/null 2>&1 || true
docker volume create "$VOL" > /dev/null
# Fill the volume as root, then give it to nobody, as wasm-lockstep.yml:92-97 does on the runner.
docker run --rm --platform linux/amd64 --volume "$VOL:/work" --volume "$SP/s4a:/spike:ro" "$IMAGE" \
  bash -c 'cp -a /spike/src/. /work/ && cp /spike/inner.sh /work/.sma435-s4a.sh && chmod -R a+rwX /work && chown -R 65534:65534 /work'
docker rm -f sma435-s4a > /dev/null 2>&1 || true
docker run -d --name sma435-s4a --platform linux/amd64 --cap-drop=ALL --security-opt=no-new-privileges --user 65534:65534 --volume "$VOL:/work" --workdir /work "$IMAGE" bash .sma435-s4a.sh
echo "started sma435-s4a; now run docker-wait.sh sma435-s4a $SP/s4a/result.log"
```

Run: `/bin/bash /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/s4a.sh`
Then run (timeout 600000): `/bin/bash /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/docker-wait.sh sma435-s4a /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/s4a/result.log`
Then run: `docker volume rm sma435-s4a`
Expected: `container sma435-s4a exited 0`; in the log `uid=65534`, `== proto: proto 0.61.1`, `== version: wasm-opt version 133 (version_133)`, and two sizes.
Record: the id line, the proto version, the version line, both sizes. If the exit code is not 0: apply the S4 STOP rule.

- [ ] **Step 8: Spike S5 — the features each binary uses**

Method: strip the `target_features` section, then validate with all declared features on except one. A binary uses feature F when that validation fails. Two controls prove that the method bites.

Write `$SP/s5.sh`:

```bash
#!/bin/bash
# S5 (SMA-435 spec § 6): leave-one-out feature validation of the raw and the -O binary.
# Uses S2b's isolated wasm-opt. Not -e: a failed validation is data.
set -uo pipefail
WT=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-435-binaryen
SP=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad
P="$HOME/.proto/bin/proto"
export PROTO_HOME="$SP/proto-home"
export PROTO_REPORTER=text
cd "$WT/rs/crates/bindings/paigasus-wasm"
W="$("$P" --reporter text bin wasm-opt | tail -n1)"
[ -x "$W" ] || { echo "no isolated wasm-opt: run s2b.sh first"; exit 2; }
out="$SP/s5"
rm -rf "$out" && mkdir -p "$out"
FEATURES="mutable-globals nontrapping-float-to-int bulk-memory sign-ext reference-types multivalue bulk-memory-opt call-indirect-overlong"
"$W" --help > "$out/help.txt" 2>&1
for f in $FEATURES; do
  grep -q -- "--enable-$f" "$out/help.txt" || { echo "STOP: wasm-opt has no --enable-$f, so the method does not apply"; exit 2; }
done
cp paigasus_wasm_bg.wasm "$out/raw.wasm"
"$W" "$out/raw.wasm" -O -o "$out/opt.wasm" || { echo "STOP: -O failed"; exit 2; }
all=""
for g in $FEATURES; do all="$all --enable-$g"; done
for b in raw opt; do
  "$W" "$out/$b.wasm" --all-features --strip-target-features -o "$out/$b-nf.wasm" || { echo "STOP: strip failed for $b"; exit 2; }
  echo "== $b: $(wc -c < "$out/$b.wasm") bytes"
  if "$W" "$out/$b-nf.wasm" --mvp-features $all > /dev/null 2>&1; then echo "control $b all-declared: valid"; else echo "STOP: control $b all-declared: INVALID"; fi
  if "$W" "$out/$b-nf.wasm" --mvp-features > /dev/null 2>&1; then echo "control $b mvp-only: valid (the method does not bite)"; else echo "control $b mvp-only: invalid (expected)"; fi
  for f in $FEATURES; do
    flags=""
    for g in $FEATURES; do [ "$g" = "$f" ] || flags="$flags --enable-$g"; done
    if "$W" "$out/$b-nf.wasm" --mvp-features $flags > "$out/$b-without-$f.log" 2>&1; then echo "$b $f unused"; else echo "$b $f USED"; fi
  done
done > "$out/table.txt"
cat "$out/table.txt"
```

Run: `/bin/bash /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/s5.sh`
Expected: both `all-declared` controls `valid`; both `mvp-only` controls `invalid (expected)`; every feature that `opt` marks `USED` is also `USED` for `raw`.
Record the whole table. If a control line is not as expected, the method is not valid: report it, and do not claim a S5 result. If `opt` uses a feature that `raw` does not use: apply the S5 STOP rule.

- [ ] **Step 9: Write the measurements file**

Create `docs/superpowers/specs/2026-10-05-sma-435-measurements.md` in the style of `docs/superpowers/specs/2026-09-22-sma-634-measurements.md`. Use this structure and put the measured values from Steps 1-8 in it, verbatim where a line is quoted:

```markdown
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
| Machine | (the `uname -m` line) |
| Root resolves `wasm-opt` | (the line) |
| Checksum line(s) | (verbatim) |
| `wasm-opt --version` | (verbatim) |
| `ldd bin/wasm-opt` | (verbatim) |
| Runs without `lib/` | (yes or NO) |
| Sizes: raw, `-O` | (two numbers) |

## S2 — Moon, the proto shims and the nested `.prototools` (macOS arm64)

(S2a table: versions, the three rc values, the two project counts, BEFORE and AFTER, grep hits.
S2b: the before/after diff of the isolated shims and bins, and one sentence: other shims changed
or did not change.)

## S4a — binaryen in the wasm-lockstep container (nobody, no capabilities)

(table: id line, proto version, version line, sizes)

## S5 — features each binary uses

(the whole table.txt, the four control lines, and one sentence: the -O binary needs no feature
that the raw binary does not need, or the finding)

## Verdict of Task 1

(one line per spike: PASS or the STOP rule that applied)
```

Replace every parenthesized instruction with the measured value. Do not leave one in the file.

- [ ] **Step 10: Commit the plugin and the pin**

Write `$SP/msg-task1a.txt`:

```text
build(rs): pin binaryen wasm-opt through a nested proto pin

Add the vendored proto plugin .proto/plugins/binaryen.toml and the pin
wasm-opt = "133.0.0" in rs/crates/bindings/paigasus-wasm/.prototools.
The pin is nested, so the bare proto install of ci.yml at the repository
root does not download binaryen. Spikes S1, S2, S4a and S5 passed.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

Run: `git add .proto/plugins/binaryen.toml rs/crates/bindings/paigasus-wasm/.prototools`
Run: `git commit -F /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/msg-task1a.txt`

- [ ] **Step 11: Commit the measurements**

Write `$SP/msg-task1b.txt`:

```text
docs(rs): record the SMA-435 spike measurements

S1 (Linux x86_64), S2 (Moon and the nested pin), S4a (the wasm-lockstep
container) and S5 (the features each binary uses).

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

Run: `git add docs/superpowers/specs/2026-10-05-sma-435-measurements.md`
Run: `git commit -F /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/msg-task1b.txt`

---

### Task 2: The optimize script, its unit tests, and their wiring

The vitest `include` entry and the `test` task input are in this task, because without them the unit tests do not run (spec F16) and an edit to the script selects no test in CI (spec AC 7).

**Files:**
- Create: `ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs`
- Create: `ts/packages/paigasus-kernel/tests/optimize-wasm.test.ts`
- Modify: `ts/packages/paigasus-kernel/vitest.config.ts:39-40` (the `node` project `include`)
- Modify: `ts/packages/paigasus-kernel/tsconfig.json:3-4` (`allowJs`)
- Modify: `ts/packages/paigasus-kernel/moon.yml:149-150` (`test` inputs)

**Interfaces:**
- Consumes: the nested pin format of Task 1 (`wasm-opt = "<N>.0.0"` plus a `[plugins]` line).
- Produces, exported from `scripts/optimize-wasm.mjs` (types are JSDoc; `tsconfig.json` gets `allowJs`):
  - `MARKER_SECTION: 'paigasus.wasm-opt'`
  - `customSectionCount(bytes: Uint8Array, name: string): number`
  - `customSectionPayloads(bytes: Uint8Array, name: string): string[]` (UTF-8 payloads, file order)
  - `appendCustomSection(bytes: Uint8Array, name: string, payload: string): Buffer`
  - `pinnedMajor(text: string): string | null`
  - `expectedMarker(): string` (reads the real nested pin; throws on no pin)
  - `wasmOptDisabled(toml: string, section: string): boolean` (the check 7 reader)
  - CLI: `node scripts/optimize-wasm.mjs optimize <out-dir>` and `node scripts/optimize-wasm.mjs verify <dir>`; exit 0, 1 (finding) or 2 (infrastructure). The success line of `optimize` starts with `optimize-wasm: ` and holds `wasm-opt version <major>` and the proto version.

- [ ] **Step 1: Write the failing unit tests**

Create `ts/packages/paigasus-kernel/tests/optimize-wasm.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// Unit tests for scripts/optimize-wasm.mjs (SMA-435 spec § 8). No test downloads anything.
//
// Each command-line test builds a fixture tree in a temporary directory: a COPY of the script at
// <tmp>/ts/packages/paigasus-kernel/scripts/, a <tmp>/rs/crates/bindings/paigasus-wasm/.prototools,
// and two stubs first on PATH. The script finds its crate directory from its own location, so the
// copy reads the fixture pin and not the real one. That copy is the only seam. Production has none.
//
//   stub `proto`     logs each call (argv, cwd, two env values); `bin wasm-opt` prints the stub path
//   stub `wasm-opt`  `--version` prints a configurable version; otherwise it logs its argv, copies a
//                    configurable file (default: its input) to the `-o` path, then exits STUB_RC
//
// The input is a tiny wasm module, built from the bytes below, with a `name` section like a raw
// wasm-pack release output. `inspect` reads a binary with V8 in a child process, which is an
// independent check of the section helpers under test.
import { Buffer } from 'node:buffer';
import { spawnSync } from 'node:child_process';
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { delimiter, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { afterEach, describe, expect, it } from 'vitest';
import { appendCustomSection, customSectionCount, customSectionPayloads, MARKER_SECTION, pinnedMajor, wasmOptDisabled } from '../scripts/optimize-wasm.mjs';

const SCRIPT = fileURLToPath(new URL('../scripts/optimize-wasm.mjs', import.meta.url));
const MARKER_133 = 'binaryen=133;flags=-O';
const PLUGINS = '[plugins]\nwasm-opt = "file://../../../../.proto/plugins/binaryen.toml"\n';

// A minimal valid module: `(func (result i32) i32.const 42)`, exported as `sum`.
const HEADER = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00]; // "\0asm", version 1
const TYPE = [0x01, 0x05, 0x01, 0x60, 0x00, 0x01, 0x7f]; // id 1, 5 bytes: one type, () -> i32
const FUNCTION = [0x03, 0x02, 0x01, 0x00]; // id 3, 2 bytes: one function of type 0
const EXPORT = [0x07, 0x07, 0x01, 0x03, 0x73, 0x75, 0x6d, 0x00, 0x00]; // id 7, 7 bytes: "sum" -> function 0
const CODE = [0x0a, 0x06, 0x01, 0x04, 0x00, 0x41, 0x2a, 0x0b]; // id 10, 6 bytes: one body, no locals, i32.const 42, end
// id 0, 13 bytes: the name "name" (4 bytes), then subsection 1 (function names), 6 bytes: one entry, 0 -> "sum"
const NAME = [0x00, 0x0d, 0x04, 0x6e, 0x61, 0x6d, 0x65, 0x01, 0x06, 0x01, 0x00, 0x03, 0x73, 0x75, 0x6d];

function wasm(...parts: number[][]): Buffer {
  return Buffer.from(parts.flat());
}

const RAW = wasm(HEADER, TYPE, FUNCTION, EXPORT, CODE, NAME); // what wasm-pack writes
const OPTIMIZED = wasm(HEADER, TYPE, FUNCTION, EXPORT, CODE); // what `wasm-opt -O` writes
const NO_EXPORT = wasm(HEADER, TYPE, FUNCTION, CODE);

interface Inspection {
  names: number;
  markers: string[];
  exports: string[];
}

const INSPECT = [
  "const fs = require('node:fs');",
  'const m = new WebAssembly.Module(fs.readFileSync(0));',
  "const markers = WebAssembly.Module.customSections(m, 'paigasus.wasm-opt').map((b) => Buffer.from(b).toString('utf8'));",
  "process.stdout.write(JSON.stringify({ names: WebAssembly.Module.customSections(m, 'name').length, markers, exports: WebAssembly.Module.exports(m).map((e) => e.name) }));",
].join('\n');

// V8's own reading of a binary, in a child process.
function inspect(bytes: Uint8Array): Inspection {
  const result = spawnSync(process.execPath, ['-e', INSPECT], { input: bytes, encoding: 'utf8' });
  if (result.status !== 0) throw new Error(`V8 rejected the binary: ${result.stderr}`);
  return JSON.parse(result.stdout) as Inspection;
}

// CommonJS on purpose: the fixture tree has no package.json, so Node runs these extensionless
// files as CommonJS.
const STUB_PROTO = [
  "const fs = require('node:fs');",
  'const args = process.argv.slice(2);',
  'const call = { args, cwd: process.cwd(), pin: process.env.PROTO_WASM_OPT_VERSION ?? null, reporter: process.env.PROTO_REPORTER ?? null };',
  "fs.appendFileSync(process.env.STUB_LOG, JSON.stringify(call) + '\\n');",
  "if (args[0] === '--version') { process.stdout.write('proto 0.61.1\\n'); process.exit(0); }",
  "if (args[0] === 'install') process.exit(Number(process.env.STUB_INSTALL_RC ?? '0'));",
  "if (args.join(' ') === '--reporter text bin wasm-opt') {",
  "  if (process.env.STUB_NDJSON === '1') process.stdout.write('{\"type\":\"message\",\"message\":\"Detected an AI agent environment\"}\\n');",
  "  process.stdout.write((process.env.STUB_BIN ?? process.env.STUB_WASM_OPT) + '\\n');",
  '  process.exit(0);',
  '}',
  "process.stderr.write('stub proto: unexpected arguments ' + JSON.stringify(args) + '\\n');",
  'process.exit(64);',
].join('\n');

const STUB_WASM_OPT = [
  "const fs = require('node:fs');",
  'const args = process.argv.slice(2);',
  "if (args[0] === '--version') {",
  "  const v = process.env.STUB_VERSION ?? '133';",
  "  process.stdout.write('wasm-opt version ' + v + ' (version_' + v + ')\\n');",
  '  process.exit(0);',
  '}',
  'fs.writeFileSync(process.env.STUB_ARGV, JSON.stringify(args));',
  "const o = args.indexOf('-o');",
  'if (o >= 0) fs.copyFileSync(process.env.STUB_OUTPUT ?? args[0], args[o + 1]);',
  "const rc = Number(process.env.STUB_RC ?? '0');",
  "if (rc !== 0) process.stderr.write('stub wasm-opt: failed on purpose\\n');",
  'process.exit(rc);',
].join('\n');

interface Fixture {
  root: string;
  script: string;
  crate: string;
  out: string;
  input: string;
  temp: string;
  stubs: string;
  log: string;
  argv: string;
}

interface Run {
  status: number | null;
  stdout: string;
  stderr: string;
}

interface ProtoCall {
  args: string[];
  cwd: string;
  pin: string | null;
  reporter: string | null;
}

const roots: string[] = [];
afterEach(() => {
  for (const root of roots.splice(0)) rmSync(root, { recursive: true, force: true });
});

function fixture(pin: string = 'wasm-opt = "133.0.0"\n', input: Buffer = RAW): Fixture {
  // realpath: on macOS the temporary directory sits behind the /var -> /private/var symlink, and
  // the argv and cwd assertions compare path strings.
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'optimize-wasm-')));
  roots.push(root);
  const scripts = join(root, 'ts/packages/paigasus-kernel/scripts');
  const crate = join(root, 'rs/crates/bindings/paigasus-wasm');
  const out = join(crate, '.wasmpack-regen-out');
  const stubs = join(root, 'stubs');
  for (const dir of [scripts, out, stubs]) mkdirSync(dir, { recursive: true });
  const script = join(scripts, 'optimize-wasm.mjs');
  copyFileSync(SCRIPT, script);
  writeFileSync(join(crate, '.prototools'), `${pin}\n${PLUGINS}`);
  writeFileSync(join(out, 'paigasus_wasm_bg.wasm'), input);
  for (const [name, body] of [
    ['proto', STUB_PROTO],
    ['wasm-opt', STUB_WASM_OPT],
  ] as const) {
    writeFileSync(join(stubs, name), `#!${process.execPath}\n${body}\n`);
    chmodSync(join(stubs, name), 0o755);
  }
  return {
    root,
    script,
    crate,
    out,
    input: join(out, 'paigasus_wasm_bg.wasm'),
    temp: join(out, '.optimize-wasm.tmp'),
    stubs,
    log: join(root, 'proto-calls.jsonl'),
    argv: join(root, 'wasm-opt-argv.json'),
  };
}

function file(f: Fixture, name: string, bytes: Buffer): string {
  const path = join(f.root, name);
  writeFileSync(path, bytes);
  return path;
}

function run(f: Fixture, args: string[], env: Record<string, string> = {}, options: { cwd?: string; script?: string } = {}): Run {
  const result = spawnSync(process.execPath, [options.script ?? f.script, ...args], {
    cwd: options.cwd ?? f.root,
    encoding: 'utf8',
    env: { ...process.env, PATH: `${f.stubs}${delimiter}${process.env.PATH ?? ''}`, STUB_LOG: f.log, STUB_ARGV: f.argv, STUB_WASM_OPT: join(f.stubs, 'wasm-opt'), ...env },
  });
  return { status: result.status, stdout: result.stdout, stderr: result.stderr };
}

function protoCalls(f: Fixture): ProtoCall[] {
  if (!existsSync(f.log)) return [];
  return readFileSync(f.log, 'utf8')
    .trim()
    .split('\n')
    .map((line) => JSON.parse(line) as ProtoCall);
}

function wasmOptArgv(f: Fixture): string[] {
  return JSON.parse(readFileSync(f.argv, 'utf8')) as string[];
}

function optimizedOutput(f: Fixture): Record<string, string> {
  return { STUB_OUTPUT: file(f, 'optimized.wasm', OPTIMIZED) };
}

function expectOptimized(f: Fixture, r: Run): void {
  expect(r.status, r.stderr).toBe(0);
  expect(inspect(readFileSync(f.input))).toEqual({ names: 0, markers: [MARKER_133], exports: ['sum'] });
  expect(existsSync(f.temp)).toBe(false);
}

function expectUntouched(f: Fixture, input: Buffer = RAW): void {
  expect(readFileSync(f.input).equals(input)).toBe(true);
  expect(existsSync(f.temp)).toBe(false);
}

const RELEASE = 'package.metadata.wasm-pack.profile.release';

function cargoToml(body: string): string {
  return `[package]\nname = "x"\n\n[${RELEASE}]\n${body}\n`;
}

const AFTER_STEP_7: { label: string; env: (f: Fixture) => Record<string, string>; message: RegExp }[] = [
  { label: 'wasm-opt exits non-zero', env: (f) => ({ ...optimizedOutput(f), STUB_RC: '3' }), message: /exited 3/ },
  { label: 'the output keeps a name section', env: () => ({}), message: /name section/ },
  { label: 'the output is not valid wasm', env: (f) => ({ STUB_OUTPUT: file(f, 'garbage.wasm', Buffer.from('not wasm at all')) }), message: /not a valid wasm module/ },
  { label: 'the output drops an export', env: (f) => ({ STUB_OUTPUT: file(f, 'no-export.wasm', NO_EXPORT) }), message: /exports/ },
];

describe('optimize-wasm.mjs', { timeout: 60_000 }, () => {
  describe('the fixture bytes', () => {
    it('are modules that V8 accepts, and only RAW has a name section', () => {
      expect(inspect(RAW)).toEqual({ names: 1, markers: [], exports: ['sum'] });
      expect(inspect(OPTIMIZED)).toEqual({ names: 0, markers: [], exports: ['sum'] });
      expect(inspect(NO_EXPORT).exports).toEqual([]);
    });
  });

  describe('the section helpers', () => {
    it('count custom sections as V8 does', () => {
      expect(customSectionCount(RAW, 'name')).toBe(1);
      expect(customSectionCount(OPTIMIZED, 'name')).toBe(0);
    });

    it('round-trip a payload longer than 127 bytes (a two-byte LEB128 size)', () => {
      const payload = 'x'.repeat(200);
      const out = appendCustomSection(OPTIMIZED, 'test.section', payload);
      expect(out.readUInt8(OPTIMIZED.length)).toBe(0);
      expect(out.readUInt8(OPTIMIZED.length + 1) & 0x80).toBe(0x80);
      expect(customSectionPayloads(out, 'test.section')).toEqual([payload]);
      expect(inspect(out).exports).toEqual(['sum']);
    });

    it('reject bytes that are not wasm', () => {
      expect(() => customSectionCount(Buffer.from('not wasm'), 'name')).toThrow(/magic/);
    });

    it('read the pin and ignore the plugin line', () => {
      expect(pinnedMajor(`wasm-opt = "133.0.0"\n${PLUGINS}`)).toBe('133');
      expect(pinnedMajor(PLUGINS)).toBeNull();
      expect(pinnedMajor('wasm-opt = "133.1.0"\n')).toBeNull();
    });
  });

  describe('the check 7 reader', () => {
    it.each(['wasm-opt=false', 'wasm-opt = false # note', '  wasm-opt = false'])('accepts %j', (line) => {
      expect(wasmOptDisabled(cargoToml(line), RELEASE)).toBe(true);
    });

    it.each(["wasm-opt = ['-O']", "wasm-opt = ['-O', '-g']", 'wasm-opt = true', ''])('rejects %j', (line) => {
      expect(wasmOptDisabled(cargoToml(line), RELEASE)).toBe(false);
    });

    it('rejects a wasm-opt = false line that sits in another section', () => {
      expect(wasmOptDisabled(`[${RELEASE}]\n\n[package.metadata.wasm-pack.profile.dev]\nwasm-opt = false\n`, RELEASE)).toBe(false);
    });
  });

  describe('optimize', () => {
    it('optimizes a raw input with the pinned wasm-opt, from the crate directory', () => {
      const f = fixture();
      const r = run(f, ['optimize', f.out], optimizedOutput(f));
      expectOptimized(f, r);
      expect(wasmOptArgv(f)).toEqual([f.input, '-O', '-o', f.temp]);
      expect(r.stdout).toMatch(/wasm-opt version 133/);
      const calls = protoCalls(f);
      expect(calls.map((c) => c.args.join(' '))).toEqual(expect.arrayContaining(['install wasm-opt', '--reporter text bin wasm-opt']));
      for (const call of calls) {
        expect(call.cwd).toBe(f.crate);
        expect(call.reporter).toBe('text');
      }
    });

    it('takes the last line of `proto bin`, after an NDJSON preamble (SMA-609)', () => {
      const f = fixture();
      expectOptimized(f, run(f, ['optimize', f.out], { ...optimizedOutput(f), STUB_NDJSON: '1' }));
    });

    it('hides PROTO_WASM_OPT_VERSION from proto, which would beat the pin', () => {
      const f = fixture();
      expectOptimized(f, run(f, ['optimize', f.out], { ...optimizedOutput(f), PROTO_WASM_OPT_VERSION: '1.0.0' }));
      const calls = protoCalls(f);
      expect(calls.length).toBeGreaterThan(0);
      for (const call of calls) expect(call.pin).toBeNull();
    });

    it.each(['wasm-opt="133.0.0"\n', 'wasm-opt = "133.0.0"   \n', 'wasm-opt\t=\t"133.0.0"\r\n'])('reads the pin %j', (pin) => {
      const f = fixture(pin);
      expectOptimized(f, run(f, ['optimize', f.out], optimizedOutput(f)));
    });

    it('exits 2 when the nested .prototools has no pin', () => {
      const f = fixture('');
      const r = run(f, ['optimize', f.out], optimizedOutput(f));
      expect(r.status, r.stderr).toBe(2);
      expect(r.stderr).toMatch(/wasm-opt = "<N>\.0\.0"/);
      expectUntouched(f);
    });

    it('exits 1 on another wasm-opt version and names both', () => {
      const f = fixture();
      const r = run(f, ['optimize', f.out], { ...optimizedOutput(f), STUB_VERSION: '132' });
      expect(r.status, r.stderr).toBe(1);
      expect(r.stderr).toMatch(/version_132/);
      expect(r.stderr).toMatch(/version_133/);
      expect(existsSync(f.argv)).toBe(false);
      expectUntouched(f);
    });

    it('exits 2 when proto install fails', () => {
      const f = fixture();
      const r = run(f, ['optimize', f.out], { ...optimizedOutput(f), STUB_INSTALL_RC: '1' });
      expect(r.status, r.stderr).toBe(2);
      expectUntouched(f);
    });

    it.each(['a path that does not exist', 'a directory'])('exits 2 when proto bin prints %s', (kind) => {
      const f = fixture();
      const bin = kind === 'a directory' ? f.root : join(f.root, 'no-such-wasm-opt');
      const r = run(f, ['optimize', f.out], { ...optimizedOutput(f), STUB_BIN: bin });
      expect(r.status, r.stderr).toBe(2);
      expectUntouched(f);
    });

    it('exits 1 on an input with no name section', () => {
      const f = fixture(undefined, OPTIMIZED);
      const r = run(f, ['optimize', f.out], optimizedOutput(f));
      expect(r.status, r.stderr).toBe(1);
      expect(r.stderr).toMatch(/Cargo\.toml/);
      expectUntouched(f, OPTIMIZED);
    });

    it('exits 1 on an input that already has the marker', () => {
      const marked = appendCustomSection(RAW, MARKER_SECTION, MARKER_133);
      const f = fixture(undefined, marked);
      const r = run(f, ['optimize', f.out], optimizedOutput(f));
      expect(r.status, r.stderr).toBe(1);
      expect(r.stderr).toMatch(/optimized it before/);
      expectUntouched(f, marked);
    });

    it.each(AFTER_STEP_7)('$label: exit 1, the input is unchanged, no temporary file remains', ({ env, message }) => {
      const f = fixture();
      const r = run(f, ['optimize', f.out], env(f));
      expect(r.status, r.stderr).toBe(1);
      expect(r.stderr).toMatch(message);
      expectUntouched(f);
    });

    // Review Focus 1.
    it('runs when it is started through a symlinked path', () => {
      // Node resolves import.meta.url through symlinks but keeps process.argv[1] as given. A naive
      // entry-point check then never runs main(), and the script exits 0 having done nothing.
      const f = fixture();
      const link = join(f.root, 'link');
      symlinkSync(f.root, link);
      const r = run(f, ['optimize', f.out], optimizedOutput(f), { script: join(link, 'ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs') });
      expectOptimized(f, r);
    });

    // Review Focus 2.
    it('replaces a temporary file that a killed run left behind', () => {
      const f = fixture();
      writeFileSync(f.temp, 'left over');
      expectOptimized(f, run(f, ['optimize', f.out], optimizedOutput(f)));
    });

    // Review Focus 3.
    it('resolves a relative out-dir against the working directory', () => {
      const f = fixture();
      const r = run(f, ['optimize', '.wasmpack-regen-out'], optimizedOutput(f), { cwd: f.crate });
      expectOptimized(f, r);
      expect(wasmOptArgv(f)[0]).toBe(f.input);
    });

    // Review Focus 4.
    it('exits 2 when the out-dir holds no binary', () => {
      const f = fixture();
      rmSync(f.input);
      const r = run(f, ['optimize', f.out], optimizedOutput(f));
      expect(r.status, r.stderr).toBe(2);
      expect(existsSync(f.temp)).toBe(false);
    });

    // Review Focus 5.
    it.each<[string[]]>([[[]], [['optimize']], [['optimise', 'OUT']], [['optimize', 'OUT', 'extra']]])('refuses the arguments %j with exit 2', (args) => {
      const f = fixture();
      const r = run(
        f,
        args.map((a) => (a === 'OUT' ? f.out : a)),
      );
      expect(r.status, r.stderr).toBe(2);
      expect(r.stderr).toMatch(/usage/);
      expectUntouched(f);
    });
  });

  describe('verify', () => {
    it('passes on a binary with the pinned marker and no name section, and calls no proto', () => {
      const f = fixture(undefined, appendCustomSection(OPTIMIZED, MARKER_SECTION, MARKER_133));
      const r = run(f, ['verify', f.out]);
      expect(r.status, r.stderr).toBe(0);
      expect(protoCalls(f)).toEqual([]);
    });

    it.each<[string, Buffer]>([
      ['another payload', appendCustomSection(OPTIMIZED, MARKER_SECTION, 'binaryen=132;flags=-O')],
      ['a second marker', appendCustomSection(appendCustomSection(OPTIMIZED, MARKER_SECTION, MARKER_133), MARKER_SECTION, MARKER_133)],
      ['a name section', appendCustomSection(RAW, MARKER_SECTION, MARKER_133)],
      ['no marker', OPTIMIZED],
    ])('exits 1 on %s', (_label, bytes) => {
      const f = fixture(undefined, bytes);
      const r = run(f, ['verify', f.out]);
      expect(r.status, r.stderr).toBe(1);
    });
  });
});
```

Add the file to the `node` project `include` in `ts/packages/paigasus-kernel/vitest.config.ts`. Replace:

```ts
            'tests/committed-wasm.test.ts',
            'tests/committed-napi-glue.test.ts',
          ],
```

with:

```ts
            'tests/committed-wasm.test.ts',
            'tests/committed-napi-glue.test.ts',
            'tests/optimize-wasm.test.ts',
          ],
```

- [ ] **Step 2: Run the tests to verify that they fail**

Run: `RUN pnpm -C ts/packages/paigasus-kernel exec vitest run tests/optimize-wasm.test.ts`
Expected: FAIL. The file does not load, because `../scripts/optimize-wasm.mjs` does not exist ("Failed to load url" or "Cannot find module").

- [ ] **Step 3: Measure AC 7 before the input exists (red first)**

Write `$SP/affected.sh`:

```bash
#!/bin/bash
# Print the tasks that `moon query tasks --affected` selects for the files given as arguments.
# One target per tasks[project][task] (root CLAUDE.md: a dep entry also has a "target" key).
set -euo pipefail
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-435-binaryen
printf '%s\n' "$@" | moon query tasks --affected | python3 -c '
import sys, json
d = json.load(sys.stdin)
print("\n".join(sorted(f"{p}:{n}" for p, ts in (d.get("tasks") or {}).items() for n in ts)))'
```

Run: `/bin/bash /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/affected.sh ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs`
Expected: the list does NOT hold `paigasus-kernel-ts:test` (it can hold `ts:lint`, `ts:fmt` and `paigasus-kernel-ts:generate-wasm`). Record the list in the measurements file under a new heading "AC 7 — affected selection".

- [ ] **Step 4: Write the script**

Create `ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs`:

```js
// SPDX-License-Identifier: Apache-2.0
//
// Optimize the wasm kernel with the proto-pinned binaryen `wasm-opt`, and verify the result
// (SMA-435 spec § 5.4).
//
//   node scripts/optimize-wasm.mjs optimize <out-dir>   optimize <out-dir>/paigasus_wasm_bg.wasm in place
//   node scripts/optimize-wasm.mjs verify <dir>         check <dir>/paigasus_wasm_bg.wasm only
//
// wasm-pack keeps `wasm-opt = false` (rs/crates/bindings/paigasus-wasm/Cargo.toml), because its
// own optimizer downloads a binaryen release that no file in this repository pins (SMA-427 L3).
// This script runs the binaryen that rs/crates/bindings/paigasus-wasm/.prototools pins. It has two
// callers: the `generate-wasm` task in moon.yml (the committed binary) and the `assemble` job of
// .github/workflows/prebuild.yml (the published binary). The flag list lives here only, so the
// two binaries cannot get different flags.
//
// After wasm-opt, the script appends its own custom section `paigasus.wasm-opt` with the payload
// `binaryen=<major>;flags=-O`. `verify` and checks 5 and 6 of tests/committed-wasm.test.ts read it.
//
// Exit 1 is a finding and exit 2 an infrastructure error, as in the repository's gates.
//
// The crate directory comes from this file's own location, as in generate-wasm.mjs. There is no
// environment variable and no flag for it, so nothing can point the script at another pin. The
// unit tests (tests/optimize-wasm.test.ts) copy this file into a fixture tree instead.
import { Buffer } from 'node:buffer';
import { spawnSync } from 'node:child_process';
import { accessSync, constants, existsSync, readFileSync, realpathSync, renameSync, rmSync, statSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

// scripts -> paigasus-kernel -> packages -> ts -> repo root: four `../`, as in generate-wasm.mjs.
const ROOT = new URL('../../../../', import.meta.url);
const CRATE = new URL('rs/crates/bindings/paigasus-wasm/', ROOT);
const CRATE_DIR = fileURLToPath(CRATE);
const PROTOTOOLS = new URL('.prototools', CRATE);
const BINARY = 'paigasus_wasm_bg.wasm';
// Not `paigasus_wasm*`: prebuild.yml stages `.wasmpack-release-out/paigasus_wasm*` for upload, and
// a temporary file must never match that glob.
const TEMP = '.optimize-wasm.tmp';
// The only flags. `-O` needs no `--enable-*`: wasm-opt reads the target_features section (spec F8).
const FLAGS = ['-O'];
const WASM_HEADER = [0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];

/** The custom section that this script appends after wasm-opt. */
export const MARKER_SECTION = 'paigasus.wasm-opt';

// A failure with its exit code. The helpers throw it too, so the drift gate gets the message.
class ScriptError extends Error {
  /**
   * @param {1 | 2} code
   * @param {string} message
   */
  constructor(code, message) {
    super(message);
    this.name = 'ScriptError';
    this.code = code;
  }
}

/** @param {string} message */
const finding = (message) => new ScriptError(1, message);
/** @param {string} message */
const infra = (message) => new ScriptError(2, message);

/**
 * Read an unsigned LEB128 number of at most 32 bits.
 * @param {Uint8Array} bytes
 * @param {number} offset
 * @returns {[number, number]} the value, and the offset after it
 */
function readU32(bytes, offset) {
  let value = 0;
  let position = offset;
  for (let shift = 0; shift < 35; shift += 7) {
    const byte = bytes[position];
    if (byte === undefined) throw finding(`a LEB128 number at byte ${offset} is cut off: the file is not a complete wasm binary`);
    position += 1;
    value += (byte & 0x7f) * 2 ** shift;
    if ((byte & 0x80) === 0) return [value, position];
  }
  throw finding(`a LEB128 number at byte ${offset} is longer than 5 bytes: the file is not a valid wasm binary`);
}

/**
 * @param {number} value
 * @returns {number[]}
 */
function encodeU32(value) {
  const out = [];
  let rest = value;
  do {
    const low = rest & 0x7f;
    rest = Math.floor(rest / 128);
    out.push(rest === 0 ? low : low | 0x80);
  } while (rest !== 0);
  return out;
}

/**
 * The custom sections of a wasm binary, in file order. A section is id 0, a LEB128 size, a LEB128
 * name length, the name, and the payload. Throws a finding when the bytes are not a well-formed
 * sequence of sections.
 * @param {Uint8Array} bytes
 * @returns {{ name: string, payload: Buffer }[]}
 */
function customSections(bytes) {
  const buffer = Buffer.from(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  if (buffer.length < WASM_HEADER.length || WASM_HEADER.some((value, index) => buffer[index] !== value)) {
    throw finding('the file does not start with the wasm magic number and version 1');
  }
  const found = [];
  let position = WASM_HEADER.length;
  while (position < buffer.length) {
    const id = buffer[position];
    const [size, start] = readU32(buffer, position + 1);
    const end = start + size;
    if (end > buffer.length) throw finding(`section ${id} at byte ${position} ends after the end of the file`);
    if (id === 0) {
      const [nameLength, nameStart] = readU32(buffer, start);
      if (nameStart + nameLength > end) throw finding(`the custom section at byte ${position} has a name longer than the section`);
      found.push({ name: buffer.toString('utf8', nameStart, nameStart + nameLength), payload: buffer.subarray(nameStart + nameLength, end) });
    }
    position = end;
  }
  return found;
}

/**
 * @param {Uint8Array} bytes
 * @param {string} name
 * @returns {number} how many custom sections have this name
 */
export function customSectionCount(bytes, name) {
  return customSections(bytes).filter((section) => section.name === name).length;
}

/**
 * @param {Uint8Array} bytes
 * @param {string} name
 * @returns {string[]} the UTF-8 payloads of the custom sections with this name, in file order
 */
export function customSectionPayloads(bytes, name) {
  return customSections(bytes)
    .filter((section) => section.name === name)
    .map((section) => section.payload.toString('utf8'));
}

/**
 * @param {Uint8Array} bytes
 * @param {string} name
 * @param {string} payload
 * @returns {Buffer} a new buffer: the bytes, then one custom section. Imports, exports and glue do not change.
 */
export function appendCustomSection(bytes, name, payload) {
  const nameBytes = Buffer.from(name, 'utf8');
  const content = Buffer.concat([Buffer.from(encodeU32(nameBytes.length)), nameBytes, Buffer.from(payload, 'utf8')]);
  return Buffer.concat([Buffer.from(bytes), Buffer.from([0]), Buffer.from(encodeU32(content.length)), content]);
}

/**
 * The binaryen major version that a .prototools text pins, or null. binaryen tags are
 * `version_<N>`, so the proto version is always <N>.0.0. The `[plugins]` line of the same file
 * holds a `file://` path and does not match.
 * @param {string} text
 * @returns {string | null}
 */
export function pinnedMajor(text) {
  const match = /^wasm-opt[ \t]*=[ \t]*"(\d+)\.0\.0"[ \t\r]*$/m.exec(text);
  return match === null ? null : (match[1] ?? null);
}

/** @returns {string} */
function readPin() {
  let text;
  try {
    text = readFileSync(PROTOTOOLS, 'utf8');
  } catch (error) {
    throw infra(`cannot read ${fileURLToPath(PROTOTOOLS)}: ${error instanceof Error ? error.message : String(error)}`);
  }
  const major = pinnedMajor(text);
  if (major === null) throw infra(`${fileURLToPath(PROTOTOOLS)} has no \`wasm-opt = "<N>.0.0"\` line. binaryen tags are version_<N>, so the pin is always <N>.0.0.`);
  return major;
}

/** @param {string} major */
function markerFor(major) {
  return `binaryen=${major};flags=${FLAGS.join(' ')}`;
}

/**
 * The marker payload that the nested pin demands, for example `binaryen=133;flags=-O`.
 * @returns {string}
 */
export function expectedMarker() {
  return markerFor(readPin());
}

/**
 * True when the TOML text sets `wasm-opt = false` in the table `[section]`, and no other value
 * there. A small line reader, because the lockfile has no TOML parser (spec F17). It accepts spaces
 * around `=` and a trailing comment. A `wasm-opt` key in another table does not count.
 * @param {string} toml
 * @param {string} section for example 'package.metadata.wasm-pack.profile.release'
 * @returns {boolean}
 */
export function wasmOptDisabled(toml, section) {
  let current = '';
  const values = [];
  for (const raw of toml.split(/\r?\n/)) {
    const line = raw.trim();
    const header = /^\[\[?\s*([^\]]+?)\s*\]\]?\s*(?:#.*)?$/.exec(line);
    if (header !== null) {
      current = header[1] ?? '';
      continue;
    }
    if (current !== section) continue;
    const entry = /^wasm-opt\s*=\s*(.*?)\s*(?:#.*)?$/.exec(line);
    if (entry !== null) values.push(entry[1]);
  }
  return values.length > 0 && values.every((value) => value === 'false');
}

// The child environment of every proto call. PROTO_REPORTER=text: proto otherwise prints NDJSON in
// an agent environment and still exits 0 (SMA-609). PROTO_WASM_OPT_VERSION is deleted: proto reads
// it before any .prototools file, so a stale export would beat the pin.
function protoEnv() {
  const env = { ...process.env, PROTO_REPORTER: 'text' };
  delete env.PROTO_WASM_OPT_VERSION;
  return env;
}

// Every proto call runs in the crate directory, so the nested pin applies (spec F6).
/** @param {string[]} args */
function proto(args) {
  const result = spawnSync('proto', args, { cwd: CRATE_DIR, env: protoEnv(), encoding: 'utf8' });
  if (result.error !== undefined) throw infra(`cannot run \`proto ${args.join(' ')}\`: ${result.error.message}. Put the proto shims and bin directories on PATH.`);
  return result;
}

/** @param {string} text */
function lastLine(text) {
  return (
    text
      .split(/\r?\n/)
      .map((line) => line.trim())
      .filter((line) => line !== '')
      .at(-1) ?? ''
  );
}

/** @param {string} path */
function isExecutableFile(path) {
  if (path === '') return false;
  try {
    accessSync(path, constants.X_OK);
    return statSync(path).isFile();
  } catch {
    return false;
  }
}

// Steps 3 to 5 of spec § 5.4.2: install, resolve, compare the version.
/** @param {string} major */
function resolveWasmOpt(major) {
  const install = proto(['install', 'wasm-opt']);
  if (install.status !== 0) {
    throw infra(`\`proto install wasm-opt\` exited ${install.status} in ${CRATE_DIR}. The first run needs network access to github.com.\n${install.stdout}${install.stderr}`);
  }
  const bin = proto(['--reporter', 'text', 'bin', 'wasm-opt']);
  // The LAST line: proto can print an NDJSON preamble before the path (SMA-609).
  const path = lastLine(bin.stdout);
  if (bin.status !== 0 || !isExecutableFile(path)) {
    throw infra(`\`proto bin wasm-opt\` did not give an executable file (exit ${bin.status}, got ${JSON.stringify(path)}). If that looks like JSON, proto's agent-mode output leaked (SMA-609).`);
  }
  const version = spawnSync(path, ['--version'], { encoding: 'utf8' });
  if (version.error !== undefined) throw infra(`cannot run ${path} --version: ${version.error.message}`);
  const actual = lastLine(version.stdout);
  const expected = `wasm-opt version ${major} (version_${major})`;
  if (version.status !== 0 || actual !== expected) {
    throw finding(`${path} reports ${JSON.stringify(actual)}, and ${fileURLToPath(PROTOTOOLS)} pins ${JSON.stringify(expected)}. Run \`proto install wasm-opt\` in ${CRATE_DIR} and check that no PROTO_WASM_OPT_VERSION or global pin is set.`);
  }
  return path;
}

/**
 * @param {Uint8Array} bytes
 * @param {string} label
 * @returns {string[]} the sorted `name:kind` export list
 */
function exportNames(bytes, label) {
  let module;
  try {
    module = new WebAssembly.Module(bytes);
  } catch (error) {
    throw finding(`${label} is not a valid wasm module: ${error instanceof Error ? error.message : String(error)}`);
  }
  return WebAssembly.Module.exports(module)
    .map((entry) => `${entry.name}:${entry.kind}`)
    .sort();
}

/** @param {string} outDir an absolute path */
function optimize(outDir) {
  const major = readPin();
  const wasmOpt = resolveWasmOpt(major);
  const input = resolve(outDir, BINARY);
  const temp = resolve(outDir, TEMP);
  if (!existsSync(input)) throw infra(`${input} does not exist. Run wasm-pack into ${outDir} first.`);
  const before = readFileSync(input);

  // Step 6, the optimize-once guard. `-O` is deterministic but not idempotent (spec F10).
  if (customSectionCount(before, MARKER_SECTION) > 0) {
    throw finding(`${input} already has a ${MARKER_SECTION} section, so this script optimized it before. Optimize a fresh wasm-pack output only, never two times.`);
  }
  if (customSectionCount(before, 'name') === 0) {
    throw finding(
      `${input} has no \`name\` section, so it is not a raw wasm-pack release output. There are three causes: (1) wasm-pack optimized it itself: check that rs/crates/bindings/paigasus-wasm/Cargo.toml keeps \`wasm-opt = false\`; (2) another tool optimized it; (3) a rustc, wasm-bindgen or \`strip\` change removed the section (SMA-435 spec R2). For cause 3, remove the \`name\` half of this guard. The ${MARKER_SECTION} marker stays the real guard.`,
    );
  }
  const inputExports = exportNames(before, input);

  let done = false;
  // A temporary file from a killed run must not survive into this one.
  rmSync(temp, { force: true });
  try {
    // Step 7.
    const run = spawnSync(wasmOpt, [input, ...FLAGS, '-o', temp], { encoding: 'utf8' });
    if (run.error !== undefined) throw infra(`cannot run ${wasmOpt}: ${run.error.message}`);
    if (run.status !== 0) throw finding(`wasm-opt exited ${run.status} on ${input}:\n${run.stderr}`);
    if (!existsSync(temp)) throw finding(`wasm-opt exited 0 but wrote no ${temp}`);
    const after = readFileSync(temp);

    // Step 8.
    const outputExports = exportNames(after, 'the wasm-opt output');
    const names = customSectionCount(after, 'name');
    if (names !== 0) throw finding(`the wasm-opt output still has ${names} name section(s). \`-O\` removes it; check the flags in this script.`);
    if (customSectionCount(after, MARKER_SECTION) !== 0) throw finding(`the wasm-opt output already has a ${MARKER_SECTION} section.`);
    if (outputExports.join(',') !== inputExports.join(',')) {
      throw finding(`the wasm-opt output exports [${outputExports.join(', ')}] and the input exports [${inputExports.join(', ')}]. wasm-opt must keep every export.`);
    }

    // Step 9. A rename is correct here: the input is in a scratch out-dir, and no pnpm hard link
    // points at it (the SMA-634 F14 rule applies to the crate copy only).
    const marked = appendCustomSection(after, MARKER_SECTION, markerFor(major));
    writeFileSync(temp, marked);
    renameSync(temp, input);
    done = true;

    // Step 11. The proto version is for the log only: Moon can run another proto (spec S2).
    const protoVersion = lastLine(proto(['--version']).stdout) || 'proto version unknown';
    process.stdout.write(`optimize-wasm: ${input}: ${before.length} -> ${marked.length} bytes, wasm-opt version ${major}, flags ${FLAGS.join(' ')}, ${protoVersion}\n`);
  } finally {
    // Step 10: on every failure path the input is unchanged and no temporary file remains.
    if (!done) rmSync(temp, { force: true });
  }
}

/** @param {string} dir an absolute path */
function verify(dir) {
  const expected = expectedMarker();
  const file = resolve(dir, BINARY);
  if (!existsSync(file)) throw infra(`${file} does not exist`);
  const bytes = readFileSync(file);
  const payloads = customSectionPayloads(bytes, MARKER_SECTION);
  if (payloads.length !== 1) {
    throw finding(`${file} has ${payloads.length} ${MARKER_SECTION} sections, not 1. Run \`node scripts/optimize-wasm.mjs optimize\` on a fresh wasm-pack output exactly once.`);
  }
  if (payloads[0] !== expected) {
    throw finding(`${file} carries ${JSON.stringify(payloads[0])}, and ${fileURLToPath(PROTOTOOLS)} demands ${JSON.stringify(expected)}.`);
  }
  const names = customSectionCount(bytes, 'name');
  if (names !== 0) throw finding(`${file} has ${names} name section(s). An optimized binary has none.`);
  process.stdout.write(`optimize-wasm: ${file} carries ${expected}\n`);
}

// realpath on both sides: Node resolves import.meta.url through symlinks, but process.argv[1] keeps
// the path as given (macOS /var is a symlink to /private/var). A string comparison would then
// never run main(), and the script would exit 0 having done nothing.
function isEntryPoint() {
  const entry = process.argv[1];
  if (entry === undefined) return false;
  try {
    return realpathSync(entry) === realpathSync(fileURLToPath(import.meta.url));
  } catch {
    return false;
  }
}

function main() {
  const args = process.argv.slice(2);
  const [mode, target] = args;
  try {
    if (args.length !== 2 || target === undefined || (mode !== 'optimize' && mode !== 'verify')) {
      throw infra(`usage: node scripts/optimize-wasm.mjs optimize <out-dir> | verify <dir> (got ${JSON.stringify(args)})`);
    }
    if (mode === 'optimize') optimize(resolve(target));
    else verify(resolve(target));
  } catch (error) {
    if (error instanceof ScriptError) {
      process.stderr.write(`optimize-wasm: ${error.message}\n`);
      process.exit(error.code);
    }
    process.stderr.write(`optimize-wasm: unexpected error (infrastructure): ${error instanceof Error ? (error.stack ?? error.message) : String(error)}\n`);
    process.exit(2);
  }
}

if (isEntryPoint()) main();
```

Let `tsc` read the JSDoc types. In `ts/packages/paigasus-kernel/tsconfig.json`, replace:

```jsonc
    "noEmit": true
    // SMA-634 removed `customConditions: ["node"]`.
```

with:

```jsonc
    "noEmit": true,
    // SMA-435: tests import scripts/optimize-wasm.mjs. allowJs lets tsc read its JSDoc types.
    // There is no checkJs, so tsc does not type-check the plain-Node scripts. paigasus-next-config
    // does the same for its eslint.mjs.
    "allowJs": true
    // SMA-634 removed `customConditions: ["node"]`.
```

(The second line of each block is the first line of the existing SMA-634 comment. Keep the rest of that comment as it is.)

- [ ] **Step 5: Run the unit tests to verify that they pass**

Run: `RUN pnpm -C ts/packages/paigasus-kernel exec vitest run tests/optimize-wasm.test.ts`
Expected: PASS, every test in `optimize-wasm.mjs`. If a stub does not start, read `stderr` in the failure message: a missing `#!` interpreter means `process.execPath` holds a space, which the shebang line cannot express.

- [ ] **Step 6: Add the `test` input and measure AC 7 (green)**

In `ts/packages/paigasus-kernel/moon.yml`, `test` task, replace:

```yaml
      - 'src/**/*'
      - 'tests/**/*'
      # Check 3 of tests/committed-napi-glue.test.ts reads this file.
```

with:

```yaml
      - 'src/**/*'
      - 'tests/**/*'
      # SMA-435: tests/optimize-wasm.test.ts imports this script, and checks 5 and 7 of
      # tests/committed-wasm.test.ts use its helpers. On Moon 2.5.3 only inputs select a task, so
      # without this line an edit to the script alone runs no test in CI.
      - 'scripts/optimize-wasm.mjs'
      # Check 3 of tests/committed-napi-glue.test.ts reads this file.
```

Run: `/bin/bash /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/affected.sh ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs`
Expected: the list now holds `paigasus-kernel-ts:test`. Record the list in the measurements file ("AC 7 — affected selection", after).

- [ ] **Step 7: Run the whole kernel test task, the type check, lint and format**

Run (timeout 600000): `RUN moon run paigasus-kernel-ts:test`
Expected: PASS (checks 1-3 of the drift gate, the napi glue gate, and the new unit tests).
Run: `RUN moon run paigasus-kernel-ts:typecheck --force`
Expected: PASS. If `tsc` reports TS7016 for `../scripts/optimize-wasm.mjs`, the `allowJs` edit is missing.
Run: `RUN pnpm -C ts exec prettier --write packages/paigasus-kernel/scripts/optimize-wasm.mjs packages/paigasus-kernel/tests/optimize-wasm.test.ts packages/paigasus-kernel/vitest.config.ts packages/paigasus-kernel/moon.yml`
Run: `RUN moon run ts:fmt ts:lint`
Expected: PASS. If ESLint reports `no-undef` for a global in the `.mjs`, import that name from a `node:` module (Global Constraints). If it reports a `no-unsafe-*` rule on an import from the `.mjs`, add the missing JSDoc type to that export.

- [ ] **Step 8: Commit**

Write `$SP/msg-task2.txt`:

```text
build(ts): add the optimize-wasm script for the pinned binaryen

scripts/optimize-wasm.mjs installs the binaryen of the nested pin, runs
wasm-opt -O on a fresh wasm-pack output, and appends a paigasus.wasm-opt
marker section. verify checks the marker without proto. Unit tests run a
copy of the script in a fixture tree with a stub proto and wasm-opt. The
script is a test input, so an edit to it selects paigasus-kernel-ts:test.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

Run: `git add ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs ts/packages/paigasus-kernel/tests/optimize-wasm.test.ts ts/packages/paigasus-kernel/vitest.config.ts ts/packages/paigasus-kernel/tsconfig.json ts/packages/paigasus-kernel/moon.yml docs/superpowers/specs/2026-10-05-sma-435-measurements.md`
Run: `git commit -F /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/msg-task2.txt`

---

### Task 3: Check 5, the `generate-wasm` wiring, and the optimized committed binary

Check 5 is in this task, not in Task 4: it fails on the raw committed binary until this task regenerates it. With check 5 here, every commit of the branch is green.

**Files:**
- Modify: `ts/packages/paigasus-kernel/tests/committed-wasm.test.ts:3-17` (header, imports), after line 143 (check 5)
- Modify: `ts/packages/paigasus-kernel/moon.yml` (`test` inputs; `generate-wasm` comment, script, inputs)
- Regenerate: `rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm` (the four glue files must not change)

**Interfaces:**
- Consumes: `customSectionCount`, `customSectionPayloads`, `expectedMarker`, `MARKER_SECTION` from Task 2; the CLI `node scripts/optimize-wasm.mjs optimize <out-dir>`.
- Produces: an optimized, marked committed binary; drift-gate check 5; the `generate-wasm` order `--pre`, `--remap`, `wasm-pack`, `optimize`, `--post`.

- [ ] **Step 1: Write check 5 (the failing test)**

In `ts/packages/paigasus-kernel/tests/committed-wasm.test.ts`, replace the header lines 3-9:

```ts
// The drift gate for the five committed wasm artifacts (SMA-634 spec § 5.4). Three checks, all
// host-independent, so they hold in CI although the binary bytes differ per host (spec F12):
//
//   1. the committed glue equals the glue of the fresh build this task already makes;
//   2. the committed binary has the same import and export lists as the fresh one, and that list is
//      the REAL kernel interface, not an empty pair of lists;
//   3. the committed glue and binary instantiate together and replay all six parity corpora.
```

with:

```ts
// The drift gate for the five committed wasm artifacts (SMA-634 spec § 5.4, SMA-435 spec § 5.6).
// Every check here is host-independent, so it holds in CI although the binary bytes differ per
// host (SMA-634 spec F12):
//
//   1. the committed glue equals the glue of the fresh build this task already makes;
//   2. the committed binary has the same import and export lists as the fresh one, and that list is
//      the REAL kernel interface, not an empty pair of lists;
//   3. the committed glue and binary instantiate together and replay all six parity corpora;
//   5. the committed binary has exactly one `paigasus.wasm-opt` marker, its payload is the one the
//      nested binaryen pin demands, and it has no `name` section (SMA-435).
```

Replace the import lines 17-20:

```ts
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
```

with:

```ts
import { execFileSync } from 'node:child_process';
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { customSectionCount, customSectionPayloads, expectedMarker, MARKER_SECTION } from '../scripts/optimize-wasm.mjs';
```

Insert after the `check 3` test (after its closing `});`, before the final `});` of the `describe`):

```ts
  // Checks 5 to 7 run in this process. They count sections and import nothing, so the
  // child-process reason of checks 2 and 3 (SMA-634 spec F11) does not apply.
  it('check 5: the committed binary was optimized one time, by the pinned binaryen', () => {
    const bytes = readFileSync(new URL('paigasus_wasm_bg.wasm', CRATE));
    expect(
      customSectionPayloads(bytes, MARKER_SECTION),
      `the committed binary does not carry exactly one ${MARKER_SECTION} section with the payload that rs/crates/bindings/paigasus-wasm/.prototools demands. It was not optimized, another binaryen optimized it, or the pin moved without a regeneration. ${REGENERATE}`,
    ).toEqual([expectedMarker()]);
    expect(customSectionCount(bytes, 'name'), `the committed binary has a \`name\` section, so \`wasm-opt -O\` did not make it. ${REGENERATE}`).toBe(0);
  });
```

Add the pin as a `test` input in `ts/packages/paigasus-kernel/moon.yml`. Replace:

```yaml
      - 'scripts/optimize-wasm.mjs'
      # Check 3 of tests/committed-napi-glue.test.ts reads this file.
```

with:

```yaml
      - 'scripts/optimize-wasm.mjs'
      # SMA-435: check 5 reads the nested binaryen pin, so a pin bump must select this task.
      - '/rs/crates/bindings/paigasus-wasm/.prototools'
      # Check 3 of tests/committed-napi-glue.test.ts reads this file.
```

- [ ] **Step 2: Run the drift gate to verify that check 5 fails**

Run (timeout 600000): `RUN moon run paigasus-kernel-ts:test`
Expected: FAIL in `check 5`, with `expected [] to deeply equal [ 'binaryen=133;flags=-O' ]` (the committed binary is raw). Every other test passes.

- [ ] **Step 3: Wire the optimize step into `generate-wasm`**

In `ts/packages/paigasus-kernel/moon.yml`, insert this paragraph into the `generate-wasm` comment block, after the paragraph that ends `would write nothing.` and before the line `  generate-wasm:`. Replace:

```yaml
  # there (SMA-693 spec F2). cache: false — the task writes tracked files, and a cached replay
  # would write nothing.
  generate-wasm:
```

with:

```yaml
  # there (SMA-693 spec F2). cache: false — the task writes tracked files, and a cached replay
  # would write nothing.
  #
  # SMA-435: `optimize-wasm.mjs optimize` runs between wasm-pack and `--post`. wasm-pack keeps
  # `wasm-opt = false` (rs/crates/bindings/paigasus-wasm/Cargo.toml), because its own optimizer
  # downloads a binaryen release that no file pins. The script installs the binaryen that
  # rs/crates/bindings/paigasus-wasm/.prototools pins (the first run downloads it, so this task
  # needs network access), runs `wasm-opt -O` on the scratch binary, and appends the
  # `paigasus.wasm-opt` marker that check 5 of tests/committed-wasm.test.ts reads. `--post` then
  # runs its identity check on the optimized binary, which is the binary that is committed. The
  # glue does not change: wasm-pack writes it before wasm-opt runs.
  generate-wasm:
```

Replace the script lines:

```yaml
      ( cd ../../../rs/crates/bindings/paigasus-wasm && wasm-pack build . --target bundler --release --no-pack --out-dir .wasmpack-regen-out --out-name paigasus_wasm -- --config "target.wasm32-unknown-unknown.rustflags=$remap" --locked )
      node scripts/generate-wasm.mjs --post
```

with:

```yaml
      ( cd ../../../rs/crates/bindings/paigasus-wasm && wasm-pack build . --target bundler --release --no-pack --out-dir .wasmpack-regen-out --out-name paigasus_wasm -- --config "target.wasm32-unknown-unknown.rustflags=$remap" --locked )
      node scripts/optimize-wasm.mjs optimize ../../../rs/crates/bindings/paigasus-wasm/.wasmpack-regen-out
      node scripts/generate-wasm.mjs --post
```

Replace the end of the `generate-wasm` inputs:

```yaml
      - '/rs/.cargo/config.toml'
    options:
      runInCI: false
      cache: false

  # SMA-667.
```

with:

```yaml
      - '/rs/.cargo/config.toml'
      # SMA-435: the binaryen pin and its plugin. The task is `cache: false`, and A5 and A7 check
      # only that their required inputs are present, so these two lines are for a human reader.
      - '/rs/crates/bindings/paigasus-wasm/.prototools'
      - '/.proto/plugins/binaryen.toml'
    options:
      runInCI: false
      cache: false

  # SMA-667.
```

(`scripts/**/*` is already an input of this task, so the script itself needs no line.)

- [ ] **Step 4: Regenerate the five artifacts**

This step installs binaryen 133 into `~/.proto` through proto. That is the designed path of `generate-wasm`, not a host install.
Run (timeout 600000): `RUN moon run paigasus-kernel-ts:generate-wasm`
Expected: PASS. `.moon/cache/states/paigasus-kernel-ts/generate-wasm/stdout.log` holds three lines like:

```text
generate-wasm: wasm-pack 0.15.0, sources touched
optimize-wasm: /…/rs/crates/bindings/paigasus-wasm/.wasmpack-regen-out/paigasus_wasm_bg.wasm: 50902 -> 35512 bytes, wasm-opt version 133, flags -O, proto 0.61.1
generate-wasm: wrote 5 files into rs/crates/bindings/paigasus-wasm/
```

The raw size depends on the host. The optimized size is about 35,500 bytes: spec F9 measured 35,471, and the marker adds 41 bytes. Record the `optimize-wasm` line verbatim in the measurements file.

- [ ] **Step 5: Confirm that only the binary changed**

Run: `git status --porcelain`
Expected: exactly ` M rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm`, plus the two edited files of this task (`moon.yml`, `committed-wasm.test.ts`) and the measurements file. If a glue file (`paigasus_wasm.js`, `paigasus_wasm_bg.js`, `paigasus_wasm.d.ts`, `paigasus_wasm_bg.wasm.d.ts`) changed: STOP and find why (spec §5.7). Do not commit.
Run: `RUN node ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs verify rs/crates/bindings/paigasus-wasm`
Expected: `optimize-wasm: /…/rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm carries binaryen=133;flags=-O`.
Run: `wc -c rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm`
Record the size. Task 6 writes it into `ci/wasm-lockstep/lockstep_check.py`.

- [ ] **Step 6: Run the drift gate to verify that it passes**

Run (timeout 600000): `RUN moon run paigasus-kernel-ts:test`
Expected: PASS. Check 2 passes, because the optimized binary keeps the 23 exports and the 3 imports (spec F12). If check 2 fails on the imports, read spec R6: that is a deliberate change of check 2, not a regeneration. STOP and report.
If a console test reports that the pnpm-installed copy differs (check 4), run `RUN pnpm -C ts install` after `rm -rf ts/node_modules` (ts/CLAUDE.md rule), then run the test again.

- [ ] **Step 7: Measure that a pin bump selects the test task**

Run: `/bin/bash /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/affected.sh rs/crates/bindings/paigasus-wasm/.prototools`
Expected: the list holds `paigasus-kernel-ts:test`. Record it.

- [ ] **Step 8: Format and lint**

Run: `RUN pnpm -C ts exec prettier --write packages/paigasus-kernel/tests/committed-wasm.test.ts packages/paigasus-kernel/moon.yml`
Run: `RUN moon run ts:fmt ts:lint`
Expected: PASS.

- [ ] **Step 9: Commit**

Write `$SP/msg-task3.txt`:

```text
perf(rs): optimize the wasm kernel with the pinned binaryen wasm-opt

generate-wasm now runs scripts/optimize-wasm.mjs between wasm-pack and
--post. The committed paigasus_wasm_bg.wasm is regenerated with
wasm-opt -O from binaryen 133 and carries the paigasus.wasm-opt marker.
The four glue files do not change. Drift-gate check 5 holds the
committed binary to the nested pin.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

Run: `git add ts/packages/paigasus-kernel/tests/committed-wasm.test.ts ts/packages/paigasus-kernel/moon.yml rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm docs/superpowers/specs/2026-10-05-sma-435-measurements.md`
Run: `git commit -F /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/msg-task3.txt`

---

### Task 4: Checks 6 and 7, and the `profiling` profile line

**Files:**
- Modify: `ts/packages/paigasus-kernel/tests/committed-wasm.test.ts` (header, import, two new tests after check 5)
- Modify: `rs/crates/bindings/paigasus-wasm/Cargo.toml:31-34`

**Interfaces:**
- Consumes: `customSectionCount`, `MARKER_SECTION`, `wasmOptDisabled` from Task 2; the `.wasmpack-test-out` fresh build of the `test` task.
- Produces: drift-gate checks 6 and 7; `wasm-opt = false` under both `[package.metadata.wasm-pack.profile.release]` and `[package.metadata.wasm-pack.profile.profiling]`.

- [ ] **Step 1: Write checks 6 and 7 (the failing test)**

In `ts/packages/paigasus-kernel/tests/committed-wasm.test.ts`, replace the header lines:

```ts
//   5. the committed binary has exactly one `paigasus.wasm-opt` marker, its payload is the one the
//      nested binaryen pin demands, and it has no `name` section (SMA-435).
```

with:

```ts
//   5. the committed binary has exactly one `paigasus.wasm-opt` marker, its payload is the one the
//      nested binaryen pin demands, and it has no `name` section (SMA-435);
//   6. the fresh build has one `name` section and no marker: wasm-pack did not optimize it, so it is
//      the raw input that scripts/optimize-wasm.mjs requires. This is the control for check 5;
//   7. rs/crates/bindings/paigasus-wasm/Cargo.toml keeps `wasm-opt = false` in the wasm-pack
//      `release` and `profiling` profiles. Check 6 cannot see `wasm-opt = ['-O', '-g']`, because
//      `-g` keeps the `name` section.
```

Replace the import line:

```ts
import { customSectionCount, customSectionPayloads, expectedMarker, MARKER_SECTION } from '../scripts/optimize-wasm.mjs';
```

with:

```ts
import { customSectionCount, customSectionPayloads, expectedMarker, MARKER_SECTION, wasmOptDisabled } from '../scripts/optimize-wasm.mjs';
```

Insert after the `check 5` test:

```ts
  it('check 6: the fresh build is a raw wasm-pack output', () => {
    const bytes = readFileSync(new URL('paigasus_wasm_bg.wasm', FRESH));
    expect(
      customSectionCount(bytes, 'name'),
      'the fresh build has no `name` section, so wasm-pack optimized it itself. Restore `wasm-opt = false` under [package.metadata.wasm-pack.profile.release] in rs/crates/bindings/paigasus-wasm/Cargo.toml. If that line is in place, a rustc, wasm-bindgen or `strip` change removed the section: read SMA-435 spec R2.',
    ).toBe(1);
    expect(customSectionCount(bytes, MARKER_SECTION), `the fresh build has a ${MARKER_SECTION} section. Only scripts/optimize-wasm.mjs writes it, and the test task does not run it.`).toBe(0);
  });

  it.each(['release', 'profiling'])('check 7: Cargo.toml keeps wasm-opt = false in the wasm-pack %s profile', (profile) => {
    const section = `package.metadata.wasm-pack.profile.${profile}`;
    expect(
      wasmOptDisabled(readFileSync(new URL('Cargo.toml', CRATE), 'utf8'), section),
      `rs/crates/bindings/paigasus-wasm/Cargo.toml must hold \`wasm-opt = false\` under [${section}]. Otherwise wasm-pack runs its own wasm-opt and downloads a binaryen release that no file pins (SMA-427 L3, SMA-435). scripts/optimize-wasm.mjs optimizes with the pinned binary.`,
    ).toBe(true);
  });
```

- [ ] **Step 2: Run the drift gate to verify that check 7 fails for `profiling`**

Run (timeout 600000): `RUN moon run paigasus-kernel-ts:test`
Expected: FAIL in `check 7: … profiling profile` (expected false to be true). `check 6` and `check 7: … release profile` pass.

- [ ] **Step 3: Add the `profiling` line and update the comment**

In `rs/crates/bindings/paigasus-wasm/Cargo.toml`, replace:

```toml
[package.metadata.wasm-pack.profile.release]
# Placeholder kernel — skip wasm-opt so no unpinned binaryen is downloaded at build (SMA-427 L3).
# Re-enabling optimization later must pin binaryen via a proto plugin (tracked follow-up).
wasm-opt = false
```

with:

```toml
[package.metadata.wasm-pack.profile.release]
# wasm-pack must never run wasm-opt itself: it would download a binaryen release that no file in
# this repository pins (SMA-427 L3). ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs runs the
# proto-pinned `wasm-opt -O` on wasm-pack's output instead (SMA-435). Check 7 of
# ts/packages/paigasus-kernel/tests/committed-wasm.test.ts fails if this line or the one below
# changes.
wasm-opt = false

[package.metadata.wasm-pack.profile.profiling]
# `wasm-pack build --profiling` runs wasm-opt by default. The same rule applies. The `dev` profile
# does not run wasm-opt.
wasm-opt = false
```

- [ ] **Step 4: Run the drift gate to verify that it passes**

Run (timeout 600000): `RUN moon run paigasus-kernel-ts:test`
Expected: PASS, checks 1-3 and 5-7.
Run (timeout 600000): `RUN moon run paigasus-wasm-rs:build paigasus-wasm-rs:lint`
Expected: PASS (the metadata table is not read by cargo).

- [ ] **Step 5: Format and lint**

Run: `RUN pnpm -C ts exec prettier --write packages/paigasus-kernel/tests/committed-wasm.test.ts`
Run: `RUN moon run ts:fmt ts:lint`
Expected: PASS.

- [ ] **Step 6: Commit**

Write `$SP/msg-task4.txt`:

```text
test(ts): hold wasm-pack to wasm-opt = false with drift-gate checks 6, 7

Check 6 requires a raw fresh build: one name section and no marker.
Check 7 reads Cargo.toml and requires wasm-opt = false in the wasm-pack
release and profiling profiles, which catches wasm-opt = ['-O', '-g'].
The profiling profile gets the line, because --profiling runs wasm-opt.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

Run: `git add ts/packages/paigasus-kernel/tests/committed-wasm.test.ts rs/crates/bindings/paigasus-wasm/Cargo.toml`
Run: `git commit -F /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/msg-task4.txt`

---

### Task 5: Optimize and verify the published binary in `prebuild.yml`

**Files:**
- Modify: `.github/workflows/prebuild.yml:16-23` (`push.paths`), `:32-47` (`pull_request.paths`), `:331-345` (build step), after `:373` (new verify step)
- Scratch only: `$SP/rehearse-prebuild.sh`

**Interfaces:**
- Consumes: the CLI of Task 2; `tests/wasm-probe.mjs --interfaces <dir>` and `--corpus <dir>`.
- Produces: a published `wasm-dist/paigasus_wasm_bg.wasm` that carries the marker; a verify step that fails the job before the upload when it does not.

- [ ] **Step 1: Rehearse the two steps locally and see `verify` fail on a raw build (the failing test)**

Write `$SP/rehearse-prebuild.sh`:

```bash
#!/bin/bash
# Rehearse the prebuild.yml wasm steps on this host (SMA-435 plan Task 5).
# Stage 1: verify must FAIL on the raw wasm-pack output. Stage 2: optimize, stage, verify, probe.
set -euo pipefail
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
WT=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-435-binaryen
kernel="$WT/ts/packages/paigasus-kernel"
cd "$WT/rs/crates/bindings/paigasus-wasm"
wasm-pack build . --target bundler --release --no-pack --out-dir .wasmpack-release-out --out-name paigasus_wasm
test -s .wasmpack-release-out/paigasus_wasm_bg.wasm
rc=0
node "$kernel/scripts/optimize-wasm.mjs" verify .wasmpack-release-out || rc=$?
echo "verify on the raw build: rc=$rc (expected 1)"
[ "$rc" -eq 1 ]
if [ "${1:-}" = "raw-only" ]; then exit 0; fi
node "$kernel/scripts/optimize-wasm.mjs" optimize .wasmpack-release-out
rm -rf wasm-dist && mkdir wasm-dist
cp .wasmpack-release-out/paigasus_wasm* wasm-dist/
cp package.json wasm-dist/package.json
ls -la wasm-dist
dist="$PWD/wasm-dist"
node "$kernel/scripts/optimize-wasm.mjs" verify "$dist"
node "$kernel/tests/wasm-probe.mjs" --interfaces "$dist"
node "$kernel/tests/wasm-probe.mjs" --corpus "$dist"
rm -rf .wasmpack-release-out wasm-dist
echo "rehearsal: PASS"
```

Run (timeout 600000): `/bin/bash /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/rehearse-prebuild.sh raw-only`
Expected: `verify on the raw build: rc=1 (expected 1)`, and the stderr line `optimize-wasm: … has 0 paigasus.wasm-opt sections, not 1. …`.

- [ ] **Step 2: Add the optimize line to "Build the wasm distribution"**

In `.github/workflows/prebuild.yml`, replace:

```yaml
      # in .moon/toolchains.yml's rust.targets), so no `rustup target add` is needed here.
      - name: Build the wasm distribution
        run: |
          set -euo pipefail
          cd rs/crates/bindings/paigasus-wasm
          wasm-pack build . --target bundler --release --no-pack \
            --out-dir .wasmpack-release-out --out-name paigasus_wasm
          test -s .wasmpack-release-out/paigasus_wasm_bg.wasm \
            || { echo "::error::wasm-pack produced no binary"; exit 1; }
```

with:

```yaml
      # in .moon/toolchains.yml's rust.targets), so no `rustup target add` is needed here.
      #
      # SMA-435: then optimize with the proto-pinned binaryen. wasm-pack keeps `wasm-opt = false`,
      # because its own optimizer downloads a binaryen that no file pins. optimize-wasm.mjs runs
      # `wasm-opt -O` as its own step and appends the marker that the verify step below checks. It
      # installs the binaryen pinned in rs/crates/bindings/paigasus-wasm/.prototools itself, so this
      # job needs no `proto install wasm-opt` step. Its exit 1 or 2 fails this step.
      - name: Build the wasm distribution
        run: |
          set -euo pipefail
          cd rs/crates/bindings/paigasus-wasm
          wasm-pack build . --target bundler --release --no-pack \
            --out-dir .wasmpack-release-out --out-name paigasus_wasm
          test -s .wasmpack-release-out/paigasus_wasm_bg.wasm \
            || { echo "::error::wasm-pack produced no binary"; exit 1; }
          node "$GITHUB_WORKSPACE/ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs" optimize .wasmpack-release-out
```

- [ ] **Step 3: Add the verify step after "Stage the wasm distribution for upload"**

Replace:

```yaml
          test -s wasm-dist/package.json \
            || { echo "::error::no package.json staged for upload"; exit 1; }
          ls -la wasm-dist

      - name: Upload the wasm distribution
```

with:

```yaml
          test -s wasm-dist/package.json \
            || { echo "::error::no package.json staged for upload"; exit 1; }
          ls -la wasm-dist

      # SMA-435: the lasting check on the binary that ships, before the upload. `verify` needs no
      # proto and no network. It requires exactly one `paigasus.wasm-opt` marker with the pinned
      # payload, and no `name` section. The two probe runs are the drift gate's own
      # (tests/wasm-probe.mjs): the interface lists, and all six parity corpora through the staged
      # glue and binary. wasm-dist holds a package.json with "type": "module", so the probe can
      # import the glue there.
      - name: Verify the staged wasm distribution
        run: |
          set -euo pipefail
          kernel="$GITHUB_WORKSPACE/ts/packages/paigasus-kernel"
          dist="$GITHUB_WORKSPACE/rs/crates/bindings/paigasus-wasm/wasm-dist"
          node "$kernel/scripts/optimize-wasm.mjs" verify "$dist"
          node "$kernel/tests/wasm-probe.mjs" --interfaces "$dist"
          node "$kernel/tests/wasm-probe.mjs" --corpus "$dist"

      - name: Upload the wasm distribution
```

- [ ] **Step 4: Add the `paths:` entries**

Replace the `push` list:

```yaml
      - 'rs/**'                              # includes rs/Cargo.lock + rs/rust-toolchain.toml
      - '.github/workflows/prebuild.yml'
      - '.prototools'
      - '.moon/**'                           # `moon setup` needs workspace.yml as well as toolchains.yml
```

with:

```yaml
      - 'rs/**'                              # includes rs/Cargo.lock + rs/rust-toolchain.toml
      - '.github/workflows/prebuild.yml'
      - '.prototools'
      - '.moon/**'                           # `moon setup` needs workspace.yml as well as toolchains.yml
      - '.proto/plugins/binaryen.toml'       # SMA-435: the plugin of the binaryen that the wasm build runs
      # SMA-435: an exception to the "no ts/ path" rule above. That rule is about the weekly npm
      # lockfile PR. These two files are on the release path of @paigasus/wasm: one optimizes the
      # published binary and one checks it.
      - 'ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs'
      - 'ts/packages/paigasus-kernel/tests/wasm-probe.mjs'
```

Replace the end of the `pull_request` list:

```yaml
      - 'rs/.cargo/config.toml'              # the -undefined dynamic_lookup flags the cdylib needs
      - 'rs/crates/bindings/paigasus-node-bindings/package.json'   # napi binaryName + target list
```

with:

```yaml
      - 'rs/.cargo/config.toml'              # the -undefined dynamic_lookup flags the cdylib needs
      - 'rs/crates/bindings/paigasus-node-bindings/package.json'   # napi binaryName + target list
      # SMA-435: the binaryen pin, its plugin, and the two release-path files of @paigasus/wasm.
      # `rs/**` covers the pin on push only.
      - '.proto/plugins/binaryen.toml'
      - 'rs/crates/bindings/paigasus-wasm/.prototools'
      - 'ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs'
      - 'ts/packages/paigasus-kernel/tests/wasm-probe.mjs'
```

- [ ] **Step 5: Run the full rehearsal (green)**

Run (timeout 600000): `/bin/bash /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/rehearse-prebuild.sh`
Expected: the `optimize-wasm:` line, the `verify` line `… carries binaryen=133;flags=-O`, the interfaces JSON (23 exports), the corpus JSON (`"sum":67,"uuid7":20,"prn_canonical":23,"prn_cedar":7,"prn_fields":6,"prn_parse":24`, spec F12), and `rehearsal: PASS`.
Run: `git status --porcelain`
Expected: only `.github/workflows/prebuild.yml` is modified. The rehearsal removed its two scratch directories.

- [ ] **Step 6: Run the workflow lint gate**

`repo:actionlint` needs bash 5 and a pipe of at least 8192 bytes on this Mac (root `CLAUDE.md`, "This development Mac only").
Run (timeout 600000): `RUN /opt/homebrew/bin/bash ci/actionlint/run.sh`
Expected: rc 0 and no FAIL row. Its check 5 confirms that every new `paths:` glob matches a tracked file. If the preflight line reports a pipe below 8192 bytes (rc 2, "small"), there is no local verdict: record that, and rely on the PR's CI run (memory: "Small pipe: run gates in a Linux container" gives the Docker fallback).

- [ ] **Step 7: Commit**

Write `$SP/msg-task5.txt`:

```text
ci(ci): optimize and verify the published wasm binary in prebuild

The assemble job runs optimize-wasm.mjs on the wasm-pack release output,
then verifies the staged wasm-dist binary and replays the parity corpora
through it before the upload. The binaryen pin, its plugin and the two
release-path scripts start the workflow.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

Run: `git add .github/workflows/prebuild.yml`
Run: `git commit -F /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/msg-task5.txt`

---

### Task 6: Documentation (spec §5.8)

The comments in the plugin, the nested pin, `Cargo.toml`, `moon.yml` and the test header are already done in Tasks 1-4. This task does the remaining files.

**Files:**
- Modify: `rs/CLAUDE.md:202` and after `:248` (new entry before `## Container images`)
- Modify: `ts/CLAUDE.md:127-130`
- Modify: `contracts/CLAUDE.md` after `:59`
- Modify: `ci/wasm-lockstep/README.md:103-105`
- Modify: `ci/wasm-lockstep/lockstep_check.py:63`

**Interfaces:**
- Consumes: the binary size from Task 3 Step 5.
- Produces: documentation only.

- [ ] **Step 1: `rs/CLAUDE.md` — the network prerequisite**

Replace:

```markdown
  - Check network access (`wasm-pack` downloads `wasm-bindgen-cli`).
```

with:

```markdown
  - Check network access (`wasm-pack` downloads `wasm-bindgen-cli`, and the first `generate-wasm`
    run downloads binaryen through proto).
```

- [ ] **Step 2: `rs/CLAUDE.md` — the new entry**

Replace:

```markdown
  Finish before the next Monday 06:00 UTC dependabot run (INFERRED). A new run can supersede a
  grouped PR.

## Container images
```

with:

```markdown
  Finish before the next Monday 06:00 UTC dependabot run (INFERRED). A new run can supersede a
  grouped PR.
- **binaryen's `wasm-opt` is pinned in the crate, not in the root `.prototools` (SMA-435).** The
  pin is `rs/crates/bindings/paigasus-wasm/.prototools`, and the plugin is
  `.proto/plugins/binaryen.toml`. proto reads `.prototools` files upwards from the working
  directory. So the bare `proto install` of `ci.yml` at the repository root does not read the pin,
  and CI does not download the 111 MB Linux tarball. `ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs`
  runs every `proto` call from the crate directory, runs `wasm-opt -O`, and appends a
  `paigasus.wasm-opt` marker section. `generate-wasm` (the committed binary) and `prebuild.yml`
  (the published binary) call it.
  - To bump binaryen: change the version in the nested `.prototools`, run
    `moon run paigasus-kernel-ts:generate-wasm`, and commit the five artifacts. Check 5 of
    `tests/committed-wasm.test.ts` fails until you do.
  - Never optimize a binary two times. `wasm-opt -O` is deterministic but not idempotent: a second
    run changes the bytes again. The script refuses an input that has the marker or that has no
    `name` section.
  - Never remove `wasm-opt = false` from the `release` or the `profiling` profile in
    `paigasus-wasm/Cargo.toml`. wasm-pack then downloads a binaryen that no file pins. Checks 6
    and 7 fail.

## Container images
```

- [ ] **Step 3: `ts/CLAUDE.md` — seven checks**

Replace:

```markdown
  edit, and commit all five. `paigasus-kernel-ts:test` holds them to the source with four checks —
  the committed glue equals a fresh build, the binary's import and export lists equal a fresh
  build's, the committed pair replays all six parity corpora, and (in the console vitest
  `setupFiles`) the pnpm-installed copy equals the committed files. **No check compares the binary
```

with:

```markdown
  edit, and commit all five. `paigasus-kernel-ts:test` holds them to the source with seven checks —
  the committed glue equals a fresh build, the binary's import and export lists equal a fresh
  build's, the committed pair replays all six parity corpora, (in the console vitest
  `setupFiles`) the pnpm-installed copy equals the committed files, the committed binary carries
  the `paigasus.wasm-opt` marker of the pinned binaryen, the fresh build is a raw wasm-pack output,
  and `paigasus-wasm/Cargo.toml` keeps `wasm-opt = false` (checks 5-7, SMA-435; see
  `rs/CLAUDE.md`). **No check compares the binary
```

- [ ] **Step 4: `contracts/CLAUDE.md` — the optimize step**

Replace:

```markdown
- `wasm-pack` is **proto-pinned, not Moon-managed** — `moon setup` does not install it. Any job
  invoking `wasm-pack` needs an explicit `proto install wasm-pack` step first, the same class of
  gap the documented nextest trap already records for a different tool (SMA-579).
```

with:

```markdown
- `wasm-pack` is **proto-pinned, not Moon-managed** — `moon setup` does not install it. Any job
  invoking `wasm-pack` needs an explicit `proto install wasm-pack` step first, the same class of
  gap the documented nextest trap already records for a different tool (SMA-579).
- wasm-pack keeps `wasm-opt = false`. The pinned binaryen `wasm-opt -O` runs as its own step,
  `ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs`, after wasm-pack in `generate-wasm`
  and in `prebuild.yml` (SMA-435). Its pin is in `rs/crates/bindings/paigasus-wasm/.prototools`.
  A job that optimizes needs no `proto install` step for it: the script installs the pin.
```

- [ ] **Step 5: `ci/wasm-lockstep/README.md` — third-party code in the container**

Replace:

```markdown
  step that runs third-party code (cargo, build scripts, proc-macros, `wasm-pack`,
  `wasm-bindgen-cli`, pnpm packages) runs inside `docker run`, as the user `nobody`, over a copy of
```

with:

```markdown
  step that runs third-party code (cargo, build scripts, proc-macros, `wasm-pack`,
  `wasm-bindgen-cli`, binaryen's `wasm-opt` since SMA-435, pnpm packages) runs inside
  `docker run`, as the user `nobody`, over a copy of
```

- [ ] **Step 6: `ci/wasm-lockstep/lockstep_check.py` — the size comment**

Replace:

```python
# 8 MiB per file. The committed paigasus_wasm_bg.wasm is 50,950 bytes (2026-10-02).
```

with this line, where `<N>` is the size that Task 3 Step 5 recorded, written with a comma as the thousands separator:

```python
# 8 MiB per file. The committed paigasus_wasm_bg.wasm is <N> bytes after wasm-opt -O (SMA-435).
```

- [ ] **Step 7: Run the affected gates of the Python change**

Run (timeout 600000): `RUN moon run repo:wasm-lockstep`
Expected: PASS.
Run (timeout 600000): `RUN /opt/homebrew/bin/bash ci/ruff/run.sh`
Expected: rc 0 (`repo:ruff-ci` needs bash 4+ on this Mac).

- [ ] **Step 8: Commit**

Write `$SP/msg-task6.txt`:

```text
docs(rs): document the binaryen pin and the optimize step

rs/CLAUDE.md gets the nested pin, the bump steps and the two never-rules.
ts/CLAUDE.md counts seven drift-gate checks. contracts/CLAUDE.md and the
wasm-lockstep README name the optimize step and binaryen.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

Run: `git add rs/CLAUDE.md ts/CLAUDE.md contracts/CLAUDE.md ci/wasm-lockstep/README.md ci/wasm-lockstep/lockstep_check.py`
Run: `git commit -F /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/msg-task6.txt`

---

### Task 7: The mutation battery, spike S4b, and the full local gate

Each mutation deletes or changes the feature, and a gate must turn red. Undo each mutation with the exact reverse edit (the Edit tool with `old_string` and `new_string` swapped). Never use `git checkout --`. After any fix to the code, run the WHOLE battery again (memory: "Re-run a mutation battery whole").

**Files:**
- Modify (then restore): `ts/packages/paigasus-kernel/moon.yml`, `rs/crates/bindings/paigasus-wasm/Cargo.toml`, `ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs`, `rs/crates/bindings/paigasus-wasm/.prototools`, `rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm` (regenerated by M1 and restored by a regeneration)
- Modify: `docs/superpowers/specs/2026-10-05-sma-435-measurements.md` (results)
- Scratch only: `$SP/s4b.sh`, `$SP/full-gate.sh`

**Interfaces:**
- Consumes: everything from Tasks 1-6.
- Produces: the mutation table for the PR body, the S4b record, the local gate verdict.

- [ ] **Step 1: Mutation 3 — `wasm-opt = ['-O', '-g']` in the release profile**

Edit `rs/crates/bindings/paigasus-wasm/Cargo.toml`. Replace (in the `release` table; the comment line `# changes.` makes the match unique):

```toml
# changes.
wasm-opt = false
```

with:

```toml
# changes.
wasm-opt = ['-O', '-g']
```

Run (timeout 600000): `RUN moon run paigasus-kernel-ts:test`
Expected: FAIL in `check 7: … release profile`. `check 6` PASSES (the `name` section stays with `-g`). This proves why check 7 exists. Record both results.
Undo: replace `wasm-opt = ['-O', '-g']` (after `# changes.`) with `wasm-opt = false`.

- [ ] **Step 2: Mutation 2 — remove `wasm-opt = false` from the release profile**

Write `$SP/m2-cache.sh`:

```bash
#!/bin/bash
# List wasm-pack's own binaryen cache, to see whether mutation 2 downloaded an unpinned binaryen.
ls -la "$HOME/Library/Caches/.wasm-pack" 2>/dev/null || echo "no wasm-pack cache directory"
```

Run: `/bin/bash /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/m2-cache.sh` and record the listing (before).
Edit `rs/crates/bindings/paigasus-wasm/Cargo.toml`. Replace:

```toml
# changes.
wasm-opt = false
```

with:

```toml
# changes.
```

Run (timeout 600000): `RUN moon run paigasus-kernel-ts:test`
Expected: FAIL in `check 6` (the fresh build has no `name` section) and in `check 7: … release profile`.
Record which `wasm-opt` wasm-pack ran: read `.moon/cache/states/paigasus-kernel-ts/test/stdout.log` and `stderr.log` for the wasm-pack `wasm-opt` lines, and run `m2-cache.sh` again (after). With the proto shim `~/.proto/shims/wasm-opt` on `PATH`, wasm-pack can run the pinned binary through the shim instead of its own download. If the cache listing shows a new `wasm-opt-*` entry, record its name, then delete only that new entry with `rm -rf` of its exact path.
Undo (the exact reverse edit): replace

```toml
# changes.

[package.metadata.wasm-pack.profile.profiling]
```

with

```toml
# changes.
wasm-opt = false

[package.metadata.wasm-pack.profile.profiling]
```

- [ ] **Step 3: Mutation 4 — delete the step-8 checks**

Edit `ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs`. Replace:

```js
    // Step 8.
    const outputExports = exportNames(after, 'the wasm-opt output');
    const names = customSectionCount(after, 'name');
    if (names !== 0) throw finding(`the wasm-opt output still has ${names} name section(s). \`-O\` removes it; check the flags in this script.`);
    if (customSectionCount(after, MARKER_SECTION) !== 0) throw finding(`the wasm-opt output already has a ${MARKER_SECTION} section.`);
    if (outputExports.join(',') !== inputExports.join(',')) {
      throw finding(`the wasm-opt output exports [${outputExports.join(', ')}] and the input exports [${inputExports.join(', ')}]. wasm-opt must keep every export.`);
    }

```

with:

```js
    // Step 8 (MUTATION 4: deleted).

```

(If Prettier reflowed the block in Task 2, copy the exact current text from the file as `old_string`, and keep it in `$SP/m4-original.txt` for the undo.)
Run: `RUN pnpm -C ts/packages/paigasus-kernel exec vitest run tests/optimize-wasm.test.ts`
Expected: FAIL in `the output keeps a name section`, `the output is not valid wasm` and `the output drops an export`. Record the failing test names. ESLint is not run here; an unused `inputExports` is expected while the mutation is in place.
Undo: the exact reverse edit.

- [ ] **Step 4: Mutation 5 — delete the step-5 version comparison, then run Moon WITHOUT `--force`**

Edit `ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs`. Replace:

```js
  if (version.status !== 0 || actual !== expected) {
    throw finding(`${path} reports ${JSON.stringify(actual)}, and ${fileURLToPath(PROTOTOOLS)} pins ${JSON.stringify(expected)}. Run \`proto install wasm-opt\` in ${CRATE_DIR} and check that no PROTO_WASM_OPT_VERSION or global pin is set.`);
  }
  return path;
```

with:

```js
  return path;
```

(Use the exact current text as `old_string` if Prettier reflowed it.)
Run (timeout 600000): `RUN moon run paigasus-kernel-ts:test`
Expected: the task RUNS (it is not a cache hit, because the script is an input) and FAILS in `exits 1 on another wasm-opt version and names both`. Record that Moon ran the task and the failing test name.
Undo: the exact reverse edit.

- [ ] **Step 5: Mutation 6 — bump the nested pin to `132.0.0` without a regeneration**

Edit `rs/crates/bindings/paigasus-wasm/.prototools`. Replace `wasm-opt = "133.0.0"` with `wasm-opt = "132.0.0"`.
Run (timeout 600000): `RUN moon run paigasus-kernel-ts:test`
Expected: FAIL in `check 5` (expected `[ 'binaryen=133;flags=-O' ]` to equal `[ 'binaryen=132;flags=-O' ]`). Record it.
Undo: replace `wasm-opt = "132.0.0"` with `wasm-opt = "133.0.0"`.

- [ ] **Step 6: Mutation 1 — remove the optimize line from `generate-wasm`, and regenerate**

Edit `ts/packages/paigasus-kernel/moon.yml`. Replace:

```yaml
      node scripts/optimize-wasm.mjs optimize ../../../rs/crates/bindings/paigasus-wasm/.wasmpack-regen-out
      node scripts/generate-wasm.mjs --post
```

with:

```yaml
      node scripts/generate-wasm.mjs --post
```

Run (timeout 600000): `RUN moon run paigasus-kernel-ts:generate-wasm`
Run (timeout 600000): `RUN moon run paigasus-kernel-ts:test`
Expected: FAIL in `check 5` (expected `[]` to equal `[ 'binaryen=133;flags=-O' ]`). Record it.
Undo: the exact reverse edit of `moon.yml`. Then run (timeout 600000) `RUN moon run paigasus-kernel-ts:generate-wasm` again.
Run: `git status --porcelain`
Expected: no modified file except `docs/superpowers/specs/2026-10-05-sma-435-measurements.md`. The regeneration on the same host gives the committed bytes again (spec F10: deterministic). If the binary still differs, record it and STOP: the committed binary then does not come from this source on this host.

- [ ] **Step 7: Confirm that the battery left a clean, green tree**

Run: `git status --porcelain`
Expected: only the measurements file (if you already wrote to it).
Run (timeout 600000): `RUN moon run paigasus-kernel-ts:test`
Expected: PASS.

- [ ] **Step 8: Spike S4b — the full `container.sh build` of the wasm-lockstep workflow**

Write `$SP/s4b.sh`:

```bash
#!/bin/bash
# S4b (SMA-435 spec § 6, part 2): the real ci/wasm-lockstep/container.sh build, with the flags of
# wasm-lockstep.yml:138 plus --platform linux/amd64, on a git archive of HEAD owned by 65534.
set -euo pipefail
WT=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-435-binaryen
SP=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad
IMAGE='docker.io/library/rust:1.95.0-bookworm@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1'
VOL=sma435-s4b
rm -rf "$SP/s4b" && mkdir -p "$SP/s4b/src" "$SP/s4b/out"
git -C "$WT" archive HEAD | tar -x -C "$SP/s4b/src"
docker volume rm -f "$VOL" > /dev/null 2>&1 || true
docker volume create "$VOL" > /dev/null
docker run --rm --platform linux/amd64 --volume "$VOL:/work" --volume "$SP/s4b:/spike:ro" "$IMAGE" \
  bash -c 'cp -a /spike/src/. /work/ && chmod -R a+rwX /work && chown -R 65534:65534 /work'
docker rm -f sma435-s4b > /dev/null 2>&1 || true
docker run -d --name sma435-s4b --platform linux/amd64 --cap-drop=ALL --security-opt=no-new-privileges --user 65534:65534 --volume "$VOL:/work" --workdir /work "$IMAGE" bash ci/wasm-lockstep/container.sh build
echo "started sma435-s4b; run docker-wait.sh sma435-s4b $SP/s4b/result.log, then s4b-collect.sh"
```

Write `$SP/s4b-collect.sh`:

```bash
#!/bin/bash
# Copy the five artifacts out of the S4b volume and compare them with the committed files.
set -uo pipefail
WT=/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-435-binaryen
SP=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad
IMAGE='docker.io/library/rust:1.95.0-bookworm@sha256:6258907abe69656e41cd992e0b705cdcfabcbbe3db374f92ed2d47121282d4a1'
docker run --rm --platform linux/amd64 --volume sma435-s4b:/work:ro --volume "$SP/s4b/out:/out" "$IMAGE" \
  bash -c 'cp /work/rs/crates/bindings/paigasus-wasm/paigasus_wasm* /out/ && cp /work/rs/crates/bindings/paigasus-wasm/package.json /out/'
grep -n 'optimize-wasm:\|generate-wasm:' "$SP/s4b/result.log" || echo "no optimize-wasm or generate-wasm line in the log"
for f in paigasus_wasm.js paigasus_wasm_bg.js paigasus_wasm.d.ts paigasus_wasm_bg.wasm.d.ts; do
  if cmp -s "$SP/s4b/out/$f" "$WT/rs/crates/bindings/paigasus-wasm/$f"; then echo "glue $f: identical"; else echo "glue $f: DIFFERS"; fi
done
echo "binary: $(wc -c < "$SP/s4b/out/paigasus_wasm_bg.wasm") bytes (Linux) vs $(wc -c < "$WT/rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm") bytes (committed)"
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
node "$WT/ts/packages/paigasus-kernel/scripts/optimize-wasm.mjs" verify "$SP/s4b/out"
echo "verify rc=$?"
docker volume rm sma435-s4b > /dev/null
```

Run: `/bin/bash /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/s4b.sh`
Run (timeout 600000, repeat if it times out): `/bin/bash /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/docker-wait.sh sma435-s4b /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/s4b/result.log`
Run: `/bin/bash /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/s4b-collect.sh`
Expected: `container sma435-s4b exited 0`; the `optimize-wasm:` line (with the proto version that Moon used in the container) and `generate-wasm: wrote 5 files`; all four glue files `identical` (SMA-634 F15); a Linux binary size near the committed size; `verify rc=0`.
Record every line. If the container exits non-zero or a glue file differs: STOP and report before the PR.

- [ ] **Step 9: The full local gate**

Moon must run `repo:affected-smoke` with system bash 3.2, so this script puts a directory with only a `bash -> /bin/bash` link first on `PATH` (memory: "affected-smoke hang: shim bash, don't prepend /bin"). The target list is the one between the `ci-targets` markers of the root `CLAUDE.md`.

Write `$SP/full-gate.sh`:

```bash
#!/bin/bash
# The full gate graph of the root CLAUDE.md, with bash 3.2 for Moon's script tasks.
set -uo pipefail
SP=/private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad
mkdir -p "$SP/bash32"
ln -sf /bin/bash "$SP/bash32/bash"
export PATH="$SP/bash32:$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
export PROTO_REPORTER=text
cd /Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-435-binaryen
git fetch origin main
moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :moon-diagnosis-exec :wasm-lockstep :test-e2e \
  --base origin/main --include-relations > "$SP/full-gate.log" 2>&1
echo "moon ci rc=$?"
```

Run (timeout 600000): `/bin/bash /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/full-gate.sh`
Expected: `moon ci rc=0`. If the Bash tool times out, run the script again: the second run starts from a warm cache. If it still times out, split the target list into two halves in two script copies and run each one.
If a task fails, follow the root `CLAUDE.md` moon-diagnosis procedure (Step 0 first). The gates that need bash 4+ (`repo:ruff-ci`, `repo:next-public-free`, `repo:publish-metadata`) and `repo:actionlint` (bash 5 and a healthy pipe) can fail under bash 3.2 with `mapfile` or `declare -A` errors. That is a bash-version artifact, not a finding. Run each such gate directly instead:
`RUN /opt/homebrew/bin/bash ci/ruff/run.sh`, `RUN /opt/homebrew/bin/bash ci/next-public/run.sh`, `RUN /opt/homebrew/bin/bash ci/publish-metadata/run.sh`, `RUN /opt/homebrew/bin/bash ci/actionlint/run.sh`.
Expected: rc 0 for each. Record the verdict of each, and the pipe preflight line of `repo:actionlint`.

- [ ] **Step 10: Record the results and commit**

Add to `docs/superpowers/specs/2026-10-05-sma-435-measurements.md` a section "Mutation battery (Task 7)" with one row per mutation: the edit, the command, the expected red, the observed result (test names verbatim), and the undo. Add the mutation 2 record (which `wasm-opt` wasm-pack ran, the cache listing before and after). Add a section "S4b — the full container run" with the lines of Step 8. Add a section "Local gate" with the verdicts of Step 9.

Write `$SP/msg-task7.txt`:

```text
docs(rs): record the SMA-435 mutation battery and container run

Six mutations, each with its red gate and its exact undo. S4b ran the
real wasm-lockstep container build on this branch. The local gate
verdicts are recorded with the bash version each gate needs.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>
```

Run: `git add docs/superpowers/specs/2026-10-05-sma-435-measurements.md`
Run: `git commit -F /private/tmp/claude-501/-Users-smaschek-dev-paigasus-paigasus-core/4e573032-813f-492a-affa-e4823bf6dea1/scratchpad/msg-task7.txt`

- [ ] **Step 11: Hand the CI checks to the coordinator**

These checks need the pushed PR, so the coordinator does them (open-pr stage). List them in the report:
- AC 2: the `ci.yml` log of the PR shows no binaryen download in "Install pinned CLIs from .prototools".
- AC 4: the `prebuild.yml` run of the PR (it starts, because the PR edits `prebuild.yml`) shows the `optimize-wasm:` line in "Build the wasm distribution" and a green "Verify the staged wasm distribution".
- AC 8: the full `moon ci` target list passes in CI.
- `images.yml` passed, if the PR starts it.
- Spec §11: the open wasm-lockstep bot PR (SMA-693 M3) holds a raw binary and fails check 5 after this merge, until the workflow runs again.

---

## Self-review

**1. Spec coverage.**

| Spec item | Task |
|---|---|
| §5.1 plugin | 1 (Step 2) |
| §5.2 nested pin | 1 (Step 3) |
| §5.3 `Cargo.toml` `profiling` line and comment | 4 |
| §5.4, §5.4.1, §5.4.2 steps 1-11, §5.4.3 helpers | 2 |
| §5.5 `generate-wasm` line, inputs, comment | 3 |
| §5.5 `test` inputs (script, pin) | 2 (script), 3 (pin) |
| §5.5 vitest `include` | 2 |
| §5.5 `prebuild.yml` optimize, verify, `paths:` | 5 |
| §5.5 wasm-lockstep container (no code change) | 7 (S4b) |
| §5.6 checks 5, 6, 7 | 3 (5), 4 (6, 7) |
| §5.7 regenerate, glue unchanged | 3 |
| §5.8 documentation | 1, 3, 4 (comments), 6 (the rest) |
| §6 S1, S2, S4, S5 | 1 (S1, S2a, S2b, S4a, S5), 7 (S4b) |
| §7 AC 1 | 1 (S1, S4a; macOS in spec F3-F5 and Task 3 Step 4) |
| §7 AC 2, 4, 8 | 7 Step 11 (CI, coordinator) |
| §7 AC 3, 5 | 3, 4 |
| §7 AC 6 | 2 |
| §7 AC 7 | 2 (Steps 3 and 6) |
| §8 unit cases | 2 (every case of the list) |
| §8 mutation battery 1-6 | 7 |
| §11 rollout notes | 7 Step 11 |

**2. Deviations from the spec, and contradictions found in the code.**
- Spec §6 S4 records the `optimize-wasm` line before the script exists. Split into S4a (Task 1) and S4b (Task 7).
- Spec §5.4.3 does not list the check 7 reader, but §5.5 says "checks 5 and 7 use its helpers". The plan exports `wasmOptDisabled` and `pinnedMajor` from the script, in addition to the four named helpers.
- Spec §9 does not list `ts/packages/paigasus-kernel/tsconfig.json`. The kernel `tsconfig.json` has no `allowJs` (lines 1-15), so a TypeScript import of the `.mjs` fails `tsc` with TS7016 under `strict`. Task 2 adds `allowJs`, as `ts/packages/paigasus-next-config/tsconfig.json` does.
- `ts/eslint.config.js:39-40` gives a plain-Node `.mjs` only `process`, `URL` and `WebAssembly`. The script imports `Buffer` from `node:buffer` and does not use `console`.
- Spec §5.4.2 step 11 prints sizes and the `wasm-opt` version. The plan also prints the proto version, so that S2's question ("Log `proto --version` from inside `generate-wasm`") has a lasting answer in every `generate-wasm`, container and `prebuild` log.
- Check 5 moves from the drift-check task to the regeneration task, so that no commit is red.
- `ci/wasm-lockstep/lockstep_check.py:63` says 50,950 bytes, but the committed binary is 50,902 bytes (spec §1). Task 6 replaces the number anyway.

**3. Placeholder scan.** The measurements file skeleton in Task 1 Step 9 has parenthesized fill-in instructions for measured values; the step requires that each is replaced. Task 6 Step 6 uses `<N>` for the size that Task 3 Step 5 measures. No other open item.

**4. Type consistency.** `MARKER_SECTION`, `customSectionCount`, `customSectionPayloads`, `appendCustomSection`, `pinnedMajor`, `expectedMarker` and `wasmOptDisabled` have the same names and parameter orders in Tasks 2, 3, 4 and 7. The temporary name `.optimize-wasm.tmp`, the marker payload `binaryen=133;flags=-O` and the out-dirs `.wasmpack-regen-out`, `.wasmpack-test-out` and `.wasmpack-release-out` are the same in every task.

**5. Review Focus.** Five lines, each with a test in Task 2 Step 1 (marked `// Review Focus N.`).
