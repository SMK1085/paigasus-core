# SMA-536 ts tsc Upstream Inputs Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Every ts task that runs `tsc` reads binding typings equal to the committed files, keys on its whole `package.json` dependency closure, and `repo:affected-smoke` asserts both.

**Architecture:** A new Node script, `ts/scripts/check-installed-bindings.mjs`, runs before `tsc` in every task whose closure holds a `file:` binding, and fails unless the installed typings equal the committed typings. A new assertion A12 in `ci/affected-graph/cargo_moon_parity.py` derives the `tsc` task set from `moon query projects`, walks each task's `package.json` closure on disk, and asserts the inputs (A12a) and the preflight order (A12b). `ci/affected-graph/run.sh` adds `typecheck` to its task-name filter and re-baselines every task case from measured output.

**Tech Stack:** Node 24 ESM (`node:test`), Python 3.12+ (stdlib only), Moon 2.5.3 YAML, bash 3.2 (`run.sh` on this Mac), pnpm 11.

**Spec:** `docs/superpowers/specs/2026-10-04-sma-536-ts-typecheck-upstream-inputs-design.md` (revision 2).

## Global Constraints

- Work only in `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-536-ts-typecheck`, on branch `feature/sma-536-ts-typecheck-upstream-inputs`. Do not `cd` to the main checkout. Run `git` without `-C`.
- Every command block starts with the preamble in "Shell preamble" below. Shell state does not persist between tool calls.
- Every new source file starts with an SPDX header: `// SPDX-License-Identifier: Apache-2.0` (JS) or `# SPDX-License-Identifier: Apache-2.0` (Python).
- Conventional commits with a workspace scope: `feat(ci): …`, `feat(ts): …`, `fix(ts): …`, `docs(ci): …`. Header at most 100 characters.
- Every commit message ends with a blank line, then `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Use `git commit -m "<header>" -m "<body>" -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"`.
- No line in a commit body starts with `#` followed by digits (commitlint `footer-leading-blank`).
- Never use `--no-verify`. If `commit-msg` fails with `commitlint not found`, run `pnpm -C ts install`.
- If a commit fails with `failed to fill whole buffer`, 1Password is locked. Stop and ask the coordinator.
- Never use bare `git stash`. Never `git commit --amend`. Never `git reset` to drop a commit.
- Commit only at the end of a task, in the step that says so.
- `ci/affected-graph/run.sh` runs under system `/bin/bash` (3.2.57) on this Mac. Bash 5.3.15 deadlocks on its here-strings.
- `moon run repo:affected-smoke` runs with the bash-only shim directory first on `PATH` (see the preamble). Do not put `/bin` first: `/usr/bin/python3` is 3.9 and has no `tomllib`.
- `python3` is `/opt/homebrew/bin/python3` (3.14). Never use `/usr/bin/python3`.
- `export PROTO_REPORTER=text` before any command that captures `moon` or `proto` output (SMA-609).
- Use `--force` on every `moon run` whose result is evidence. A cache hit replays an old log.
- Foreground commands only. Do not install host software (`brew`, `npm -g`, `pip install`).
- Run `/opt/homebrew/bin/bash ci/ruff/run.sh` after any change under `ci/**/*.py` (Ruff `line-length = 200`, rules `E F W I N UP B A C4 SIM TCH RUF`).
- Run `pnpm -C ts exec prettier --write <files>` and `moon run ts:lint ts:fmt --force` after any change under `ts/scripts/`.
- Write all prose (comments, docs, commit bodies) in ASD-STE100 Simplified Technical English.
- If `git status` shows `ts/packages/paigasus-proto/src/generated/google/rpc/error_details_pb.ts` deleted after a run, a BSR rate limit hit `contracts:generate`. Restore it with `git checkout -- ts/packages/paigasus-proto/src/generated`.
- From Task 3 to Task 7, `python3 ci/affected-graph/cargo_moon_parity.py` (the real run) exits 1 with A12 rows. From Task 5 to Task 8, `/bin/bash ci/affected-graph/run.sh` exits 1. That is expected. Each task proves its own work with the command its steps name.

### Shell preamble

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma536}"
mkdir -p "$SCRATCH/bashshim" && ln -sf /bin/bash "$SCRATCH/bashshim/bash"
```

## Review Focus

These five input classes are the most likely to break the design and the least likely to be caught by a happy-path test. Most likely first. Each has a test in the owning task.

1. **A dangling installed symlink.** After `rm -rf ts/node_modules` with no reinstall, `ts/packages/paigasus-kernel/node_modules/@paigasus/wasm` still exists as a symlink to a deleted target. `realpath` throws. The preflight must exit 1 with "no installed copy" and the repair command, not crash with exit 2 or exit 0. Test: Task 2, `a dangling installed symlink … gives exit 1`.
2. **Moon's joined invocation text for a script task.** `moon_projects()` joins `command` + `script` + `args`. For a script task moon reports `command` as the script's first word, so the text reads `node node ../../../ts/scripts/check-installed-bindings.mjs && pnpm exec tsc …`. The preflight path is relative (`../../../ts/scripts/…`). A12b must accept both and must reject `…mjsx`, an `echo` of the path, and every join that is not `&&` (`;`, `||`, `|`, `&`, a newline). Test: Task 4, the `preflight_order_verdict` table.
3. **A byte-equal copy with a new inode.** CI installs fresh, and some hosts copy instead of hard-linking. The preflight must compare bytes, not inodes, and pass. Test: Task 2, `equal copies with new inodes give exit 0` (asserts the inodes differ first).
4. **`exports` shapes.** A condition object two levels deep (`{"import": {"types": …}}`), a `null` leaf, a root-level file target (`./tsconfig.app.json`), a list (fallback array), and a target without `./`. The first three must produce the right requirement. The last two must red, never be skipped. Test: Task 3, rows A12a-e to A12a-h.
5. **A workspace cycle back to the own package.** If `@paigasus/proto` gained a devDependency on `@paigasus/console-core`, console-core's closure walk reaches itself. The own package must be excluded at every depth, the walk must end, and console-core must not be asked to key on its own `src/**/*`. Test: Task 3, row A12a-q.

## Spec coverage map

| Spec section | Task |
|---|---|
| §3.1 preflight script, exit codes, host test task | 2 |
| §3.2 kernel `build`/`typecheck`, comments at `moon.yml:69-74`, `:282-283` | 5 |
| §3.3 A12 task set, closure, A12a, floor | 3 |
| §3.3 A12b, registration, README, PASS text | 4 |
| §3.4 rows A12a forces | 5, 6, 7 (measured in 4) |
| §3.5 `run.sh` filter, re-baseline, new case, stale comments | 8 |
| §4.1 red-first proof, steps 1-2 before the change | 1 |
| §4.1 steps 3-5 after the change, §4.3 mutation battery | 9 |
| §4.2 tests | 2, 3, 4 |
| §4.4 forced runs, full graph | 5, 6, 7, 10 |
| §5 docs | 4 (README, `ci/CLAUDE.md`), 5 (`ts/CLAUDE.md`), 8 (README filter note) |
| §7 follow-up issue | 10 (the coordinator creates it) |

---

## Task 1: Red-first baseline before any change (spec §4.1 steps 1, 2 and the napi half of 5)

**Files:**
- Modify: `docs/superpowers/plans/2026-10-04-sma-536-ts-typecheck-upstream-inputs.md` (section "Execution log", entry E1 only)
- Test: none. This task measures and records. It changes no code.

**Interfaces:**
- Consumes: the current `paigasus-kernel-ts:typecheck` (bare inherited `pnpm exec tsc -p tsconfig.json --noEmit`).
- Produces: execution-log entry E1, which Task 9 compares against.

- [ ] **Step 1: Confirm the start state.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
git status --short && git rev-parse --abbrev-ref HEAD
ls -li rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts \
  "ts/node_modules/.pnpm/@paigasus+wasm@file+..+rs+crates+bindings+paigasus-wasm/node_modules/@paigasus/wasm/paigasus_wasm.d.ts"
```

Expected: no status lines; branch `feature/sma-536-ts-typecheck-upstream-inputs`; both files show the same inode and link count 2. If the inodes differ, run `rm -rf ts/node_modules && pnpm -C ts install` first and repeat this step.

- [ ] **Step 2: Record where `tsc` resolves both bindings (spec §4.1 step 1).**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
for pkg in ts/packages/paigasus-kernel ts/packages/paigasus-console-core; do
  echo "== $pkg"
  (cd "$pkg" && pnpm exec tsc -p tsconfig.json --noEmit --traceResolution) \
    | grep -E "Module name '@paigasus/(wasm|node-bindings)' was successfully resolved" | sort -u
done
```

Expected: the kernel prints one line for `@paigasus/wasm` and one for `@paigasus/node-bindings`. Console-core prints the `@paigasus/wasm` line (it imports the wasm `.` entry only). Each resolved path contains `ts/node_modules/.pnpm/@paigasus+wasm@file+` or `ts/node_modules/.pnpm/@paigasus+node-bindings@file+`. Copy the lines into E1 row 1.

- [ ] **Step 3: Build a stale installed wasm typings file (spec §4.1 step 2).** `rm` removes only the installed name of the hard link. The committed file keeps its content.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma536}"; mkdir -p "$SCRATCH"
INST="ts/node_modules/.pnpm/@paigasus+wasm@file+..+rs+crates+bindings+paigasus-wasm/node_modules/@paigasus/wasm/paigasus_wasm.d.ts"
sed 's/^export function sum(a: number, b: number): number;$/export function sum(a: string, b: number): number;/' "$INST" > "$SCRATCH/stale-wasm.d.ts"
rm "$INST" && cp "$SCRATCH/stale-wasm.d.ts" "$INST"
ls -li rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts "$INST"
cmp rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts "$INST"; echo "cmp rc=$?"
git status --short
```

Expected: two different inodes, `cmp rc=1`, and no `git status` line (the committed file is unchanged).

- [ ] **Step 4: Run the kernel `typecheck` against the stale copy and record the result.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
moon run paigasus-kernel-ts:typecheck --force; echo "rc=$?"
INST="ts/node_modules/.pnpm/@paigasus+wasm@file+..+rs+crates+bindings+paigasus-wasm/node_modules/@paigasus/wasm/paigasus_wasm.d.ts"
cmp -s rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts "$INST"; echo "still stale: cmp rc=$?"
```

Expected: `rc=1`, with a `tsc` error in `src/binding-parity.types.ts` on the `_sum` line (for example `error TS2322: Type 'boolean' is not assignable to type 'never'`). No line names the installed file or a repair command. `still stale: cmp rc=1` proves that Moon did not repair the copy. Copy the `error TS…` line and both rc values into E1 row 2. This is the defect: `tsc` read a file that `git` does not show, and the error points at a source file that did not change.

- [ ] **Step 5: Restore and confirm the pass.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
rm -rf ts/node_modules && pnpm -C ts install
moon run paigasus-kernel-ts:typecheck --force; echo "rc=$?"
```

Expected: `rc=0`. Record it in E1 row 3.

- [ ] **Step 6: Repeat Steps 3 to 5 for the napi `index.d.ts` (spec §4.1 step 5, "before" half).**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma536}"; mkdir -p "$SCRATCH"
INST="ts/node_modules/.pnpm/@paigasus+node-bindings@file+..+rs+crates+bindings+paigasus-node-bindings/node_modules/@paigasus/node-bindings/index.d.ts"
sed 's/^export declare function sum(a: number, b: number): number$/export declare function sum(a: string, b: number): number/' "$INST" > "$SCRATCH/stale-napi.d.ts"
rm "$INST" && cp "$SCRATCH/stale-napi.d.ts" "$INST"
cmp -s rs/crates/bindings/paigasus-node-bindings/index.d.ts "$INST"; echo "cmp rc=$?"
git status --short
moon run paigasus-kernel-ts:typecheck --force; echo "rc=$?"
rm -rf ts/node_modules && pnpm -C ts install
moon run paigasus-kernel-ts:typecheck --force; echo "restored rc=$?"
```

Expected: `cmp rc=1`, no status line, `rc=1` with an `error TS…` line on `_sum` in `src/binding-parity.types.ts`, then `restored rc=0`. Record the three results in E1 row 4.

- [ ] **Step 7: Commit the record.**

```bash
git add docs/superpowers/plans/2026-10-04-sma-536-ts-typecheck-upstream-inputs.md
git commit -m "docs(ci): record the SMA-536 red-first baseline before the change" \
  -m "A stale installed binding typings file makes paigasus-kernel-ts:typecheck fail on an unchanged source file, and nothing names the installed copy. This is the before state for the preflight." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 2: The installed-typings preflight script, its tests, and the `ts:test` host task

**Files:**
- Create: `ts/scripts/check-installed-bindings.mjs`
- Create: `ts/scripts/check-installed-bindings.test.mjs`
- Modify: `ts/moon.yml:10-29` (add `scripts/**/*` to `fileGroups.sources`), `ts/moon.yml:44-61` (add the `test` task)

**Interfaces:**
- Consumes: `ts/packages/*/package.json`, `ts/apps/*/package.json`, each `file:` binding's `package.json` and `files` list, `<declaring package>/node_modules/<name>`.
- Produces: CLI `node ts/scripts/check-installed-bindings.mjs [--ts-root <dir>]` → exit 0 (all equal), 1 (a file differs or the installed copy is missing; stderr names each file and prints `Repair: rm -rf ts/node_modules && pnpm -C ts install`), 2 (infrastructure error; stderr starts `installed-bindings: infrastructure error:`). Stdout on exit 0: `installed-bindings: checked <N> file: dependencies; the installed typings equal the committed files`. Moon task `ts:test` (`node --test scripts/check-installed-bindings.test.mjs`).

Host choice: the `ts` configuration project has no test task today, and `ts/scripts/` is plain Node ESM with no dependencies. `node --test` follows the precedent of `ci/kind/*.test.mjs`. The task must be named `test`, because `ci.yml`'s `:test` target selects it; any other name never runs in CI. Its inputs are the script and the test file only. The `scripts/**/*` source-group entry makes `ts:lint` and `ts:fmt` key on `ts/scripts/`, which they do not do today (`check-config-only.mjs` has the same gap).

- [ ] **Step 1: Write the failing tests.** Create `ts/scripts/check-installed-bindings.test.mjs`:

```js
// SPDX-License-Identifier: Apache-2.0
// SMA-536 — unit tests for check-installed-bindings.mjs, the installed-typings preflight.
// Run from ts/:  node --test scripts/check-installed-bindings.test.mjs
// `moon run ts:test` runs them, and ci.yml's `:test` target selects that task.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { cpSync, mkdirSync, mkdtempSync, rmSync, statSync, symlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import process from 'node:process';
import { test } from 'node:test';

const SCRIPT = join(import.meta.dirname, 'check-installed-bindings.mjs');
const REPAIR = 'rm -rf ts/node_modules && pnpm -C ts install';
const TYPINGS = 'export declare function sum(a: number, b: number): number\n';
const DECLARING = { name: '@x/k', dependencies: { '@x/b': 'file:../../../rs/b' } };

// <root>/rs/b is the committed binding. <root>/ts/packages/k declares it as `file:../../../rs/b`.
// The installed copy is a COPY under <root>/ts/node_modules/.pnpm (new inodes, equal bytes), and
// <root>/ts/packages/k/node_modules/@x/b is a symlink to it. That is the shape pnpm makes.
function makeFixture({ declaring = DECLARING, install = true } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'sma536-'));
  const committed = join(root, 'rs', 'b');
  mkdirSync(committed, { recursive: true });
  writeFileSync(join(committed, 'package.json'), JSON.stringify({ name: '@x/b', types: 'index.d.ts', files: ['index.js', 'index.d.ts'] }));
  writeFileSync(join(committed, 'index.d.ts'), TYPINGS);
  writeFileSync(join(committed, 'index.js'), 'module.exports = {};\n');
  const pkg = join(root, 'ts', 'packages', 'k');
  mkdirSync(pkg, { recursive: true });
  writeFileSync(join(pkg, 'package.json'), typeof declaring === 'string' ? declaring : JSON.stringify(declaring));
  const installed = join(root, 'ts', 'node_modules', '.pnpm', 'b', 'node_modules', '@x', 'b');
  if (install) {
    cpSync(committed, installed, { recursive: true });
    mkdirSync(join(pkg, 'node_modules', '@x'), { recursive: true });
    symlinkSync(installed, join(pkg, 'node_modules', '@x', 'b'));
  }
  return { root, committed, installed };
}

function check(fx) {
  try {
    return spawnSync(process.execPath, [SCRIPT, '--ts-root', join(fx.root, 'ts')], { encoding: 'utf8' });
  } finally {
    rmSync(fx.root, { recursive: true, force: true });
  }
}

test('equal copies with new inodes give exit 0', () => {
  const fx = makeFixture();
  assert.notEqual(statSync(join(fx.installed, 'index.d.ts')).ino, statSync(join(fx.committed, 'index.d.ts')).ino);
  const r = check(fx);
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /checked 1 file: dependencies/);
});

test('a differing .d.ts gives exit 1 and names the file and the repair command', () => {
  const fx = makeFixture();
  writeFileSync(join(fx.installed, 'index.d.ts'), 'export declare function sum(a: string, b: number): number\n');
  const r = check(fx);
  assert.equal(r.status, 1, r.stderr);
  assert.match(r.stderr, /@x\/b: the installed index\.d\.ts differs from the committed rs\/b\/index\.d\.ts/);
  assert.ok(r.stderr.includes(REPAIR), r.stderr);
});

test('a differing binding package.json gives exit 1', () => {
  const fx = makeFixture();
  writeFileSync(join(fx.installed, 'package.json'), JSON.stringify({ name: '@x/b', types: 'other.d.ts', files: ['index.js', 'index.d.ts'] }));
  const r = check(fx);
  assert.equal(r.status, 1, r.stderr);
  assert.match(r.stderr, /@x\/b: the installed package\.json differs/);
  assert.ok(r.stderr.includes(REPAIR), r.stderr);
});

test('a missing installed copy gives exit 1', () => {
  const r = check(makeFixture({ install: false }));
  assert.equal(r.status, 1, r.stderr);
  assert.match(r.stderr, /@x\/b: no installed copy at ts\/packages\/k\/node_modules\/@x\/b/);
  assert.ok(r.stderr.includes(REPAIR), r.stderr);
});

test('a dangling installed symlink (ts/node_modules removed, no reinstall) gives exit 1', () => {
  const fx = makeFixture();
  rmSync(join(fx.root, 'ts', 'node_modules'), { recursive: true, force: true });
  const r = check(fx);
  assert.equal(r.status, 1, r.stderr);
  assert.match(r.stderr, /@x\/b: no installed copy/);
  assert.ok(r.stderr.includes(REPAIR), r.stderr);
});

test('an installed copy that lacks a listed .d.ts gives exit 1', () => {
  const fx = makeFixture();
  rmSync(join(fx.installed, 'index.d.ts'));
  const r = check(fx);
  assert.equal(r.status, 1, r.stderr);
  assert.match(r.stderr, /@x\/b: the installed copy has no index\.d\.ts/);
});

test('a differing listed file that is not a .d.ts (runtime glue) gives exit 0', () => {
  const fx = makeFixture();
  writeFileSync(join(fx.installed, 'index.js'), 'module.exports = { stale: true };\n');
  const r = check(fx);
  assert.equal(r.status, 0, r.stderr);
});

test('a package with no file: dependency gives exit 0', () => {
  const r = check(makeFixture({ declaring: { name: '@x/k', dependencies: { '@x/other': 'workspace:*' } }, install: false }));
  assert.equal(r.status, 0, r.stderr);
  assert.match(r.stdout, /checked 0 file: dependencies/);
});

test('an unparseable declaring package.json gives exit 2', () => {
  const r = check(makeFixture({ declaring: '{' }));
  assert.equal(r.status, 2, r.stderr);
  assert.match(r.stderr, /installed-bindings: infrastructure error: .*package\.json is not valid JSON/);
});

test('an unparseable binding package.json gives exit 2', () => {
  const fx = makeFixture();
  writeFileSync(join(fx.committed, 'package.json'), '{');
  const r = check(fx);
  assert.equal(r.status, 2, r.stderr);
  assert.match(r.stderr, /installed-bindings: infrastructure error: .*package\.json is not valid JSON/);
});

test('a file: path that does not exist gives exit 2', () => {
  const r = check(makeFixture({ declaring: { name: '@x/k', dependencies: { '@x/b': 'file:../../../rs/ghost' } } }));
  assert.equal(r.status, 2, r.stderr);
  assert.match(r.stderr, /installed-bindings: infrastructure error: @x\/b: the `file:` path .* does not exist/);
});

test('a .d.ts listed in files but not committed gives exit 2', () => {
  const fx = makeFixture();
  rmSync(join(fx.committed, 'index.d.ts'));
  const r = check(fx);
  assert.equal(r.status, 2, r.stderr);
  assert.match(r.stderr, /installed-bindings: infrastructure error: .*`files` lists index\.d\.ts/);
});
```

- [ ] **Step 2: Run the tests and see them fail.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
(cd ts && node --test scripts/check-installed-bindings.test.mjs) 2>&1 | tail -8
```

Expected: `ℹ tests 12`, `ℹ pass 0`, `ℹ fail 12` (Node 24 spec reporter). Each child process exits 1 with `Cannot find module …check-installed-bindings.mjs`, so every status or message assertion fails.

- [ ] **Step 3: Write the script.** Create `ts/scripts/check-installed-bindings.mjs`:

```js
// SPDX-License-Identifier: Apache-2.0
// SMA-536 — the installed-typings preflight.
//
// `tsc` resolves @paigasus/node-bindings and @paigasus/wasm through ts/node_modules. pnpm installs
// a `file:` dependency as hard links. A rename-write of a committed file (napi, generate-wasm,
// version-lockstep --write, most editors, BSD `sed -i`) breaks the link, and the installed copy
// then stays old. pnpm does not repair it (ts/CLAUDE.md).
//
// This script runs first in every `tsc` task whose package.json closure holds a `file:` binding.
// It compares, byte for byte, the installed copy of each binding's package.json and of each
// `.d.ts` in the binding's `files` list with the committed file. When `tsc` runs after it, `tsc`
// reads bytes equal to the committed files, so an input on a committed file is a real cache key.
// ci/affected-graph/cargo_moon_parity.py A12 asserts the inputs and the order.
//
// Exit 0: every compared file is equal.
// Exit 1: a file differs, or the installed copy is missing. The message names each file and the
//         repair command.
// Exit 2: an infrastructure error (an unparseable package.json, a `file:` path that does not
//         exist, a listed `.d.ts` that is not committed). It never exits 0 on an error.
//
// usage: node ts/scripts/check-installed-bindings.mjs [--ts-root <dir>]
//   With no argument it checks the ts/ directory that holds this script, so one invocation works
//   from any task. --ts-root exists for the tests.
import { existsSync, readFileSync, readdirSync, realpathSync, statSync } from 'node:fs';
import { dirname, join, relative, resolve } from 'node:path';
import process from 'node:process';

const REPAIR = 'rm -rf ts/node_modules && pnpm -C ts install';
const DEP_FIELDS = ['dependencies', 'devDependencies', 'peerDependencies'];
const PACKAGE_PARENTS = ['packages', 'apps'];

class InfraError extends Error {}

function readJson(path) {
  let text;
  try {
    text = readFileSync(path, 'utf8');
  } catch (err) {
    throw new InfraError(`cannot read ${path}: ${err.message}`);
  }
  try {
    return JSON.parse(text);
  } catch (err) {
    throw new InfraError(`${path} is not valid JSON: ${err.message}`);
  }
}

function isPlainObject(value) {
  return value !== null && typeof value === 'object' && !Array.isArray(value);
}

function tsRootFrom(argv) {
  const at = argv.indexOf('--ts-root');
  if (at === -1) return resolve(import.meta.dirname, '..');
  const value = argv[at + 1];
  if (value === undefined || value.startsWith('--')) throw new InfraError('--ts-root needs a directory');
  return resolve(value);
}

// Every `file:` dependency that a ts/packages/* or ts/apps/* package.json declares.
function fileDependencies(tsRoot) {
  const found = [];
  for (const parent of PACKAGE_PARENTS) {
    const parentDir = join(tsRoot, parent);
    if (!existsSync(parentDir)) continue;
    for (const entry of readdirSync(parentDir, { withFileTypes: true })) {
      if (!entry.isDirectory()) continue;
      const declaringDir = join(parentDir, entry.name);
      const manifestPath = join(declaringDir, 'package.json');
      if (!existsSync(manifestPath)) continue;
      const manifest = readJson(manifestPath);
      if (!isPlainObject(manifest)) throw new InfraError(`${manifestPath} is not a JSON object`);
      for (const field of DEP_FIELDS) {
        const deps = manifest[field] ?? {};
        if (!isPlainObject(deps)) throw new InfraError(`${manifestPath}: \`${field}\` is not an object`);
        for (const [name, spec] of Object.entries(deps)) {
          if (typeof spec === 'string' && spec.startsWith('file:')) {
            found.push({ name, declaringDir, committedDir: resolve(declaringDir, spec.slice('file:'.length)) });
          }
        }
      }
    }
  }
  return found;
}

// The files `tsc` reads from a binding: its package.json (`types`, `exports`) and each `.d.ts`
// in its `files` list. The runtime glue, the `.wasm` and the `.node` are out of scope (spec N4).
function comparedFiles(dep) {
  if (!existsSync(dep.committedDir) || !statSync(dep.committedDir).isDirectory()) {
    throw new InfraError(`${dep.name}: the \`file:\` path ${dep.committedDir} does not exist`);
  }
  const manifestPath = join(dep.committedDir, 'package.json');
  const manifest = readJson(manifestPath);
  if (!isPlainObject(manifest)) throw new InfraError(`${manifestPath} is not a JSON object`);
  const files = manifest.files ?? [];
  if (!Array.isArray(files)) throw new InfraError(`${manifestPath}: \`files\` is not a list`);
  const typings = files.filter((file) => typeof file === 'string' && file.endsWith('.d.ts'));
  for (const file of typings) {
    if (!existsSync(join(dep.committedDir, file))) {
      throw new InfraError(`${manifestPath}: \`files\` lists ${file}, which is not committed in ${dep.committedDir}`);
    }
  }
  return ['package.json', ...typings];
}

function compare(dep, repoRoot) {
  const show = (path) => relative(repoRoot, path);
  const files = comparedFiles(dep);
  const link = join(dep.declaringDir, 'node_modules', dep.name);
  let installedDir;
  try {
    installedDir = realpathSync(link);
  } catch {
    return [`${dep.name}: no installed copy at ${show(link)}`];
  }
  const problems = [];
  for (const file of files) {
    const committed = join(dep.committedDir, file);
    const installed = join(installedDir, file);
    if (!existsSync(installed)) {
      problems.push(`${dep.name}: the installed copy has no ${file} (expected ${show(installed)})`);
    } else if (!readFileSync(committed).equals(readFileSync(installed))) {
      problems.push(`${dep.name}: the installed ${file} differs from the committed ${show(committed)}`);
    }
  }
  return problems;
}

function main() {
  const tsRoot = tsRootFrom(process.argv.slice(2));
  const repoRoot = dirname(tsRoot);
  const deps = fileDependencies(tsRoot);
  const problems = deps.flatMap((dep) => compare(dep, repoRoot));
  if (problems.length > 0) {
    const lines = [
      'installed-bindings: the installed binding typings are not the committed files, so `tsc` would read stale typings.',
      ...problems.map((problem) => `  - ${problem}`),
      '',
      `Repair: ${REPAIR}`,
      'pnpm installs a `file:` dependency as hard links. A rename-write of a committed file breaks the link, and pnpm does not repair it (ts/CLAUDE.md).',
    ];
    process.stderr.write(`${lines.join('\n')}\n`);
    return 1;
  }
  process.stdout.write(`installed-bindings: checked ${deps.length} file: dependencies; the installed typings equal the committed files\n`);
  return 0;
}

try {
  process.exitCode = main();
} catch (err) {
  const kind = err instanceof InfraError ? 'infrastructure error' : 'unexpected error';
  process.stderr.write(`installed-bindings: ${kind}: ${err.message}\n`);
  process.exitCode = 2;
}
```

- [ ] **Step 4: Run the tests and the real tree.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
(cd ts && node --test scripts/check-installed-bindings.test.mjs) 2>&1 | tail -8
node ts/scripts/check-installed-bindings.mjs; echo "rc=$?"
```

Expected: `ℹ pass 12`, `ℹ fail 0` (measured on 2026-10-04 against a scratch copy of this exact code); then `installed-bindings: checked 2 file: dependencies; the installed typings equal the committed files` and `rc=0`.

- [ ] **Step 5: Add the host task and the source-group entry.** In `ts/moon.yml`, after the line `    - 'packages/*/scripts/**/*'` (line 25), insert:

```yaml
    # SMA-536: ts/scripts/ holds plain Node scripts (check-config-only.mjs, the installed-typings
    # preflight). Without this, ts:lint and ts:fmt do not key on ts/scripts and an edit there serves
    # a cached pass.
    - 'scripts/**/*'
```

At the end of `ts/moon.yml` (after the `check-config-only` task, line 61), append:

```yaml
  # SMA-536. The unit tests of scripts/check-installed-bindings.mjs, the installed-typings
  # preflight that runs before `tsc` in every task whose package closure holds a `file:` binding.
  # `node --test`, not vitest: the script is plain Node ESM with no dependencies, and this
  # configuration root has no vitest config. The name is `test` so that ci.yml's `:test` target
  # selects it; another name would never run in CI. The inputs are the script and its test only.
  test:
    command: 'node --test scripts/check-installed-bindings.test.mjs'
    inputs:
      - 'scripts/check-installed-bindings.mjs'
      - 'scripts/check-installed-bindings.test.mjs'
```

- [ ] **Step 6: Format, lint and run the host task.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
pnpm -C ts exec prettier --write scripts/check-installed-bindings.mjs scripts/check-installed-bindings.test.mjs
moon run ts:test ts:lint ts:fmt --force; echo "rc=$?"
moon query tasks 2>/dev/null | python3 -c 'import json,sys; t=json.load(sys.stdin)["tasks"]["ts"]["test"]; f=set(t.get("inputFiles") or {}); print(sorted(p for p in f if p.startswith("ts/scripts/")))'
```

Expected: `rc=0`; the last line prints `['ts/scripts/check-installed-bindings.mjs', 'ts/scripts/check-installed-bindings.test.mjs']`. (Moon also adds its own config files to `inputFiles`; the filter hides them.)

- [ ] **Step 7: Commit.**

```bash
git add ts/scripts/check-installed-bindings.mjs ts/scripts/check-installed-bindings.test.mjs ts/moon.yml
git commit -m "feat(ts): add the installed-typings preflight for binding typings" \
  -m "check-installed-bindings.mjs compares the installed copy of each file: binding's package.json and .d.ts files with the committed files, and fails with the repair command when they differ. ts:test runs its node --test suite, and ts:lint and ts:fmt now key on ts/scripts." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 3: A12a — the shared `tsc` derivation, the closure walk, the input assertion and its floor

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py`
  - header comment, after line 32 (the A11 paragraph)
  - insert the A12 block after `check_wrapper_upstream_inputs` (after line 2101 `    return a7`, before line 2104 `def moon_projects():`)
  - `self_test()`: insert after line 4352 (`failures.append("CONFIG_SENSITIVE_VERBS is empty — A10 would examine nothing")`); edit line 4359 (the `OK` text)
  - `EXPECTED_FINDING_KEYS` line 4376
  - `collect_findings`: insert after line 4514 (`"    missing."),`)
- Test: `python3 ci/affected-graph/cargo_moon_parity.py --self-test`

**Interfaces:**
- Consumes: `moon_projects()` shape `{pid: {"source_dir": str, "language": str, "tasks": {task: [dep target, …]}, "task_inputs": {task: [str] | None}, "task_input_globs": {task: [str] | None}, "invocations": {task: str | None}}}`; `MoonOutputError`; package.json files under `root`.
- Produces:
  - constants `TSC_TOKEN_RE`, `PREFLIGHT_SCRIPT = "ts/scripts/check-installed-bindings.mjs"`, `PREFLIGHT_RE`, `TS_BASE_TSCONFIG = "ts/tsconfig.base.json"`, `PROTO_PACKAGE = "@paigasus/proto"`, `PROTO_GENERATE_DEP = "contracts:generate"`, `TS_DEP_FIELDS`, `TS_PACKAGE_GLOBS`, `REQUIRED_TSC_TASKS: tuple[str, ...]`, `REQUIRED_TS_CLOSURE: dict[str, set[str]]`
  - `derive_tsc_tasks(projects) -> set[str]` (raises `MoonOutputError` on a `None` invocation)
  - `ts_workspace_packages(root: Path, rows: list[str]) -> dict[str, str]` (package name → workspace-relative dir)
  - `ts_closure(root: Path, own_dir: str, names: dict[str, str], rows: list[str]) -> dict[str, tuple[str, str]]` (name → (`"workspace"` | `"file"`, dir))
  - `tsc_required_inputs(root: Path, closure, rows: list[str]) -> tuple[set[str], bool]` (required inputs, has a `file:` binding)
  - `ts_tsc_analysis(projects, root: Path) -> tuple[set[str], dict[str, dict], dict[str, str], list[str]]` (tasks, per-task info `{"own_name", "closure", "want", "binding"}`, names, walk rows)
  - `check_ts_tsc_inputs(projects, root, floor=REQUIRED_TSC_TASKS, closure_floor=REQUIRED_TS_CLOSURE) -> list[str]` (A12a rows; `root` positional and required)
  - findings key `"a12a"` in `collect_findings` and `EXPECTED_FINDING_KEYS`

- [ ] **Step 1: Write the failing self-test rows.** Insert this block in `self_test()` directly after the line `        failures.append("CONFIG_SENSITIVE_VERBS is empty — A10 would examine nothing")` and its blank line:

```python
    # A12 (SMA-536). Two halves, like A7-h: a package.json tree written to a tmp root, and moon's
    # resolved view of the `tsc` tasks. The package names are the real ones, so the shapes match
    # what REQUIRED_TS_CLOSURE names.
    if not REQUIRED_TSC_TASKS:
        failures.append("REQUIRED_TSC_TASKS is empty — A12's task floor would assert nothing")
    if not REQUIRED_TS_CLOSURE:
        failures.append("REQUIRED_TS_CLOSURE is empty — A12's closure floor would assert nothing")

    # The tsc token, exercised directly (A10's `_var_sensitive` lesson): through the check, a blob
    # the regex rejects is simply not derived, so a missing row proves nothing about the regex.
    for blob, want in (
        ("pnpm exec tsc -p tsconfig.json --noEmit", True),
        ("tsc --noEmit", True),
        ("touch a && pnpm exec tsc -p tsconfig.json", True),
        ("(tsc -p x)", True),
        ("pnpm exec tsc-alias -p x", False),
        ("pnpm exec napi build --dts index.fresh.d.ts", False),
        ("pnpm exec vitest run", False),
        ("node scripts/tscheck.mjs", False),
    ):
        if bool(TSC_TOKEN_RE.search(blob)) is not want:
            failures.append(
                f"TSC_TOKEN_RE on {blob!r} is {not want} — the tsc token is wrong in the "
                f"{'false-negative' if want else 'false-positive'} direction"
            )

    a12_tree = {
        "ts/packages/kernel": {
            "name": "@paigasus/kernel",
            "dependencies": {"@paigasus/node-bindings": "file:../../../rs/crates/bindings/nb"},
            "exports": {".": "./src/wasm.ts", "./napi": {"types": "./src/index.ts", "default": None}},
        },
        "ts/packages/core": {
            "name": "@paigasus/console-core",
            "dependencies": {"@paigasus/kernel": "workspace:*"},
            "devDependencies": {"@paigasus/proto": "workspace:*"},
            "exports": {
                ".": "./src/index.ts",
                "./testing": "./testing/index.ts",
                "./fakes": {
                    "import": {"types": "./fakes/index.d.ts", "default": "./fakes/index.js"},
                    "require": None,
                },
            },
        },
        "ts/packages/proto": {"name": "@paigasus/proto", "exports": {".": "./src/index.ts"}},
        "ts/packages/nc": {
            "name": "@paigasus/next-config",
            "exports": {".": "./src/index.ts", "./tsconfig-app": "./tsconfig.app.json"},
        },
        "ts/apps/app": {
            "name": "@paigasus/app",
            "dependencies": {"@paigasus/console-core": "workspace:*", "@paigasus/next-config": "workspace:*"},
        },
        "rs/crates/bindings/nb": {"name": "@paigasus/node-bindings", "files": ["index.js", "index.d.ts"]},
    }
    a12_pre = "node ../../../ts/scripts/check-installed-bindings.mjs && pnpm exec tsc -p tsconfig.json --noEmit"
    a12_tsc = "pnpm exec tsc -p tsconfig.json --noEmit"
    a12_nb = ["rs/crates/bindings/nb/index.d.ts", "rs/crates/bindings/nb/package.json", PREFLIGHT_SCRIPT]
    a12_core_files = [TS_BASE_TSCONFIG, "ts/packages/kernel/package.json", "ts/packages/proto/package.json", *a12_nb]
    a12_core_globs = ["ts/packages/kernel/src/**/*", "ts/packages/proto/src/**/*"]

    def _a12_ts(source_dir, tasks, language="typescript"):
        # tasks: {name: (invocation, deps, inputFiles, inputGlobs)}
        return {
            "source_dir": source_dir, "deps": {}, "language": language,
            "tasks": {n: list(t[1]) for n, t in tasks.items()},
            "task_inputs": {n: sorted(set(t[2])) for n, t in tasks.items()},
            "task_input_globs": {n: sorted(set(t[3])) for n, t in tasks.items()},
            "invocations": {n: t[0] for n, t in tasks.items()},
        }

    a12 = {
        "k-ts": _a12_ts("ts/packages/kernel", {
            # Script form: moon reports `command` as the script's first word, so the joined blob
            # repeats it.
            "typecheck": ("node " + a12_pre, [], [TS_BASE_TSCONFIG, *a12_nb], []),
            # Not a tsc task. Its inputs are empty, so an over-matching derivation reds the clean row.
            "test": ("pnpm exec vitest run", [], [], []),
        }),
        "c-ts": _a12_ts("ts/packages/core", {
            "build": (a12_pre, [PROTO_GENERATE_DEP], a12_core_files, a12_core_globs),
            "typecheck": (a12_pre, [PROTO_GENERATE_DEP], a12_core_files, a12_core_globs),
        }),
        "p-ts": _a12_ts("ts/packages/proto", {
            "typecheck": (a12_tsc, [PROTO_GENERATE_DEP], [TS_BASE_TSCONFIG], []),
        }),
        "n-ts": _a12_ts("ts/packages/nc", {"typecheck": (a12_tsc, [], [TS_BASE_TSCONFIG], [])}),
        "app-ts": _a12_ts("ts/apps/app", {
            "typecheck": (
                a12_pre, [PROTO_GENERATE_DEP],
                [*a12_core_files, "ts/packages/core/package.json", "ts/packages/nc/package.json",
                 "ts/packages/nc/tsconfig.app.json"],
                [*a12_core_globs, "ts/packages/core/src/**/*", "ts/packages/core/testing/**/*",
                 "ts/packages/core/fakes/**/*", "ts/packages/nc/src/**/*"],
            ),
        }),
        # A tsc invocation in a NON-typescript project, under-declared on purpose. A12 must not
        # examine it. A clean project here would make the row vacuous (A7-c's lesson).
        "x-py": _a12_ts("py/x", {"typecheck": (a12_tsc, [], [], [])}, language="python"),
    }
    a12_floor = ("k-ts:typecheck", "c-ts:typecheck", "app-ts:typecheck")
    a12_closure_floor = {
        "@paigasus/kernel": {"@paigasus/node-bindings"},
        "@paigasus/console-core": {"@paigasus/kernel"},
    }

    def _a12_copy(obj):
        return json.loads(json.dumps(obj))

    def _a12_run(check_fn, fixture, tree, **kw):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            for rel, data in tree.items():
                (base / rel).mkdir(parents=True, exist_ok=True)
                if data is not None:
                    text = data if isinstance(data, str) else json.dumps(data)
                    (base / rel / "package.json").write_text(text)
            return check_fn(fixture, base, **kw)

    def _a12a(fixture=None, tree=None, **kw):
        kw.setdefault("floor", a12_floor)
        kw.setdefault("closure_floor", a12_closure_floor)
        return _a12_run(
            check_ts_tsc_inputs, a12 if fixture is None else fixture,
            a12_tree if tree is None else tree, **kw,
        )

    rows = _a12a()
    if rows != []:
        failures.append(f"A12a reported violations on a complete fixture: {rows}")

    # A12a-a: an upstream package's src glob, read PER TASK. `build` keeps the glob, so a union
    # across the project's tasks would hide the shortfall (A7-g's lesson).
    broken = _a12_copy(a12)
    broken["c-ts"]["task_input_globs"]["typecheck"] = ["ts/packages/proto/src/**/*"]
    rows = _a12a(broken)
    if "c-ts:typecheck inputs omit ts/packages/kernel/src/**/*" not in rows:
        failures.append("A12a did not fire on a tsc task missing an upstream package's src glob")
    if any(row.startswith("c-ts:build ") for row in rows):
        failures.append("A12a blamed `build` for a shortfall that lives on `typecheck`")

    # A12a-b: the binding half — a `.d.ts` from the binding's `files` list.
    broken = _a12_copy(a12)
    broken["k-ts"]["task_inputs"]["typecheck"].remove("rs/crates/bindings/nb/index.d.ts")
    if "k-ts:typecheck inputs omit rs/crates/bindings/nb/index.d.ts" not in _a12a(broken):
        failures.append("A12a did not demand a binding's .d.ts from its `files` list")

    # A12a-c: the base tsconfig, demanded of every tsc task.
    broken = _a12_copy(a12)
    broken["p-ts"]["task_inputs"]["typecheck"] = []
    if f"p-ts:typecheck inputs omit {TS_BASE_TSCONFIG}" not in _a12a(broken):
        failures.append("A12a did not demand ts/tsconfig.base.json")

    # A12a-d: the preflight script, demanded where the closure holds a binding. p-ts and n-ts
    # declare no preflight input, so the clean row above proves it is not demanded elsewhere.
    broken = _a12_copy(a12)
    broken["k-ts"]["task_inputs"]["typecheck"].remove(PREFLIGHT_SCRIPT)
    if f"k-ts:typecheck inputs omit {PREFLIGHT_SCRIPT}" not in _a12a(broken):
        failures.append("A12a did not demand the preflight script of a task with a binding")

    # A12a-e: an `exports` target in a subdirectory is satisfied by `<dir>/**/*`.
    broken = _a12_copy(a12)
    broken["app-ts"]["task_input_globs"]["typecheck"].remove("ts/packages/core/testing/**/*")
    if "app-ts:typecheck inputs omit ts/packages/core/testing/**/*" not in _a12a(broken):
        failures.append("A12a did not demand the directory glob of a subdirectory `exports` target")

    # A12a-f: a condition object is walked to its string leaves (two levels deep, with a null).
    broken = _a12_copy(a12)
    broken["app-ts"]["task_input_globs"]["typecheck"].remove("ts/packages/core/fakes/**/*")
    if "app-ts:typecheck inputs omit ts/packages/core/fakes/**/*" not in _a12a(broken):
        failures.append("A12a did not walk a conditional `exports` object")

    # A12a-g: an `exports` target at the package root is demanded as the file itself.
    broken = _a12_copy(a12)
    broken["app-ts"]["task_inputs"]["typecheck"].remove("ts/packages/nc/tsconfig.app.json")
    if "app-ts:typecheck inputs omit ts/packages/nc/tsconfig.app.json" not in _a12a(broken):
        failures.append("A12a did not demand a root-level `exports` target")

    # A12a-h: an `exports` shape A12 cannot read reds, and so does a target without `./`.
    tree = _a12_copy(a12_tree)
    tree["ts/packages/core"]["exports"]["./bad"] = ["./bad/a.js"]
    if not any("ts/packages/core/package.json: `exports` holds a list" in r for r in _a12a(tree=tree)):
        failures.append("A12a accepted an `exports` shape it cannot read")
    tree = _a12_copy(a12_tree)
    tree["ts/packages/core"]["exports"]["./raw"] = "raw/index.ts"
    if not any("does not start with `./`" in r for r in _a12a(tree=tree)):
        failures.append("A12a accepted an `exports` target that does not start with ./")

    # A12a-i: a missing DIRECT contracts:generate, for a closure reader and for proto itself. n-ts
    # has no proto in its closure and declares no deps, so the clean row proves no over-demand.
    broken = _a12_copy(a12)
    broken["app-ts"]["tasks"]["typecheck"] = []
    broken["p-ts"]["tasks"]["typecheck"] = []
    rows = _a12a(broken)
    for target in ("app-ts:typecheck", "p-ts:typecheck"):
        if not any(r.startswith(f"{target} deps omit {PROTO_GENERATE_DEP}") for r in rows):
            failures.append(f"A12a did not demand {PROTO_GENERATE_DEP} of {target}")

    # A12a-j: the task floor. No task matches `tsc`, so the derived set is empty and every
    # per-task row goes quiet by itself. The row must name THIS branch's message.
    broken = _a12_copy(a12)
    for proj in broken.values():
        proj["invocations"] = {n: "pnpm exec vitest run" for n in proj["invocations"]}
    if not any(
        r.startswith("FLOOR:") and "k-ts:typecheck is not matched by the `tsc` token" in r
        for r in _a12a(broken)
    ):
        failures.append("A12a's task floor did not fire when no task matches `tsc`")

    # A12a-k: the closure floor. The kernel loses its binding edge, and a floor package is absent.
    tree = _a12_copy(a12_tree)
    tree["ts/packages/kernel"]["dependencies"] = {}
    if not any(
        r.startswith("FLOOR:") and "closure of @paigasus/kernel no longer derives @paigasus/node-bindings" in r
        for r in _a12a(tree=tree)
    ):
        failures.append("A12a's closure floor did not fire on a broken package.json walk")
    if not any(
        r.startswith("FLOOR:") and "@paigasus/ghost is not a package" in r
        for r in _a12a(closure_floor={"@paigasus/ghost": {"@paigasus/kernel"}})
    ):
        failures.append("A12a's closure floor did not fire on a package that does not exist")

    # A12a-l to A12a-o: the fail-closed walk. Each case is a row, never a skip.
    tree = _a12_copy(a12_tree)
    tree["ts/packages/core"]["devDependencies"]["@paigasus/ghost"] = "workspace:*"
    if not any("@paigasus/ghost maps to no package directory" in r for r in _a12a(tree=tree)):
        failures.append("A12a skipped a `workspace:` name with no package")
    tree = _a12_copy(a12_tree)
    tree["ts/packages/core"]["dependencies"]["@paigasus/linked"] = "link:../linked"
    if not any("@paigasus/linked uses a `link:` specifier" in r for r in _a12a(tree=tree)):
        failures.append("A12a skipped a `link:` specifier")
    tree = _a12_copy(a12_tree)
    tree["rs/crates/bindings/nb"] = None
    if not any("rs/crates/bindings/nb/package.json is missing" in r for r in _a12a(tree=tree)):
        failures.append("A12a skipped a missing package.json")
    tree = _a12_copy(a12_tree)
    tree["ts/packages/proto"] = "{"
    if not any("ts/packages/proto/package.json is not valid JSON" in r for r in _a12a(tree=tree)):
        failures.append("A12a skipped an unparseable package.json")
    tree = _a12_copy(a12_tree)
    tree["ts/packages/kernel"]["dependencies"]["@paigasus/node-bindings"] = "file:../../../rs/crates/bindings/ghost"
    if not any("which does not exist" in r for r in _a12a(tree=tree)):
        failures.append("A12a skipped a `file:` path that does not exist")

    # A12a-p: a task with no input bucket is a violation, never a skip.
    broken = _a12_copy(a12)
    del broken["c-ts"]["task_input_globs"]["typecheck"]
    if not any(
        r.startswith("c-ts:typecheck reported no `inputFiles`/`inputGlobs`") and "moon's output shape changed" in r
        for r in _a12a(broken)
    ):
        failures.append("A12a skipped a task that reported no input bucket")

    # A12a-q: a workspace cycle back to the task's own package. The own package is excluded at
    # every depth, so core is not asked to key on itself, and the walk ends.
    tree = _a12_copy(a12_tree)
    tree["ts/packages/proto"]["devDependencies"] = {"@paigasus/console-core": "workspace:*"}
    if any(r.startswith("c-ts:") for r in _a12a(tree=tree)):
        failures.append("A12a demanded a package's own files through a workspace cycle")

    # A12a-r: a None invocation is moon telling us nothing. That is infra, exactly as in A5.
    broken = _a12_copy(a12)
    broken["k-ts"]["invocations"]["typecheck"] = None
    try:
        derive_tsc_tasks(broken)
    except MoonOutputError:
        pass
    else:
        failures.append("derive_tsc_tasks accepted a task with no command, script or args")
```

- [ ] **Step 2: Run the self-test and see it fail.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"
```

Expected: a traceback that ends `NameError: name 'REQUIRED_TSC_TASKS' is not defined`, and `rc=1`.

- [ ] **Step 3: Add the A12 block.** Insert after the line `    return a7` that ends `check_wrapper_upstream_inputs` (line 2101), followed by two blank lines, before `def moon_projects():`:

```python
# SMA-536 — A12. Every ts task that runs `tsc` must key on what `tsc` reads through its package.json
# dependency closure (A12a), and must run the installed-typings preflight before `tsc` when that
# closure holds a `file:` binding (A12b). See
# docs/superpowers/specs/2026-10-04-sma-536-ts-typecheck-upstream-inputs-design.md.
#
# `tsc` as a bounded token, optionally behind `pnpm exec`. Bounded, so `tsc-alias`, `tscheck` and
# the `.d.ts` in `--dts index.fresh.d.ts` never match. A `tsc` behind a wrapper script is invisible
# (spec R3); the floor catches the loss of a known task, not a new wrapper.
TSC_TOKEN_RE = re.compile(r"(^|[\s;&|(])(pnpm\s+exec\s+)?tsc(\s|$)")
PREFLIGHT_SCRIPT = "ts/scripts/check-installed-bindings.mjs"
# `node <relative prefix>ts/scripts/check-installed-bindings.mjs` as a whole word. moon reports a
# script-form task with `command` set to the script's first word, so the joined blob reads
# `node node ../../../ts/...`. The leading-separator group finds the second `node`.
PREFLIGHT_RE = re.compile(r"(?:^|[\s;&|(])node\s+(?:\S*/)?ts/scripts/check-installed-bindings\.mjs(?=$|[\s;&|)])")
TS_BASE_TSCONFIG = "ts/tsconfig.base.json"
PROTO_PACKAGE = "@paigasus/proto"
# A DIRECT task dep, because contracts:generate deletes ts/packages/paigasus-proto/src/generated
# before it rewrites it. Direct matches repo policy (iam-console/moon.yml) and what moon_projects()
# records for each task.
PROTO_GENERATE_DEP = "contracts:generate"
TS_DEP_FIELDS = ("dependencies", "devDependencies", "peerDependencies")
TS_PACKAGE_GLOBS = ("ts/packages/*/package.json", "ts/apps/*/package.json")

# A12's anti-vacuity floors. A12 asserts CONTAINMENT, and a containment check whose `want` set
# empties passes having asserted nothing (A7's lesson). The task floor catches a derivation that
# stops matching a known task. The closure floor catches a package.json walk that stops deriving.
REQUIRED_TSC_TASKS = (
    "paigasus-kernel-ts:build",
    "paigasus-kernel-ts:typecheck",
    "paigasus-console-core-ts:typecheck",
    "iam-console-ts:typecheck",
    "gateway-console-ts:typecheck",
)
REQUIRED_TS_CLOSURE = {
    "@paigasus/kernel": {"@paigasus/node-bindings", "@paigasus/wasm"},
    "@paigasus/console-core": {"@paigasus/kernel"},
}


def derive_tsc_tasks(projects):
    """Every `<pid>:<task>` of a `language: typescript` project whose resolved invocation runs `tsc`.

    Shared by A12a and A12b. Raises MoonOutputError if a task exposes none of a command, a script,
    or any args, exactly as derive_ffi_tasks does.
    """
    matched = set()
    for pid in sorted(projects):
        proj = projects[pid]
        if proj.get("language") != "typescript":
            continue
        invocations = proj.get("invocations") or {}
        for name in sorted(invocations):
            blob = invocations[name]
            if blob is None:
                raise MoonOutputError(
                    f"{pid}:{name} reported none of a `command`, a `script`, or any `args` — "
                    f"moon's output shape changed, so the tsc derivation cannot be evaluated"
                )
            if TSC_TOKEN_RE.search(blob):
                matched.add(f"{pid}:{name}")
    return matched


def _read_package_json(root, rel_dir, rows):
    """Parse `<root>/<rel_dir>/package.json`. On any failure, append a row and return None.

    A row, never an infrastructure error and never a skip (spec §3.3): a broken manifest is an
    authoring mistake in this repo, and the row names the file.
    """
    rel = f"{rel_dir}/package.json"
    try:
        data = json.loads((root / rel).read_text())
    except FileNotFoundError:
        rows.append(f"{rel} is missing")
        return None
    except OSError as exc:
        rows.append(f"{rel} cannot be read ({exc})")
        return None
    except ValueError as exc:
        rows.append(f"{rel} is not valid JSON ({exc})")
        return None
    if not isinstance(data, dict):
        rows.append(f"{rel} is not a JSON object")
        return None
    return data


def ts_workspace_packages(root, rows):
    """{package name: workspace-relative dir} for every ts/packages/* and ts/apps/* package."""
    names = {}
    for pattern in TS_PACKAGE_GLOBS:
        for path in sorted(root.glob(pattern)):
            rel_dir = path.parent.relative_to(root).as_posix()
            data = _read_package_json(root, rel_dir, rows)
            if data is None:
                continue
            name = data.get("name")
            if not isinstance(name, str) or not name:
                rows.append(f"{rel_dir}/package.json has no `name`, so no `workspace:` specifier can reach it")
                continue
            if name in names:
                rows.append(f"{name} is declared by both {names[name]} and {rel_dir}")
                continue
            names[name] = rel_dir
    return names


def ts_closure(root, own_dir, names, rows):
    """Transitive `workspace:` and `file:` closure of the package at `own_dir`.

    Returns {name: (kind, dir)} with kind "workspace" or "file". Walks `dependencies`,
    `devDependencies` and `peerDependencies` of every member, upstream devDependencies included:
    the apps reach @paigasus/proto through console-core's devDependency, because `tsc` reads
    console-core/testing/fake-iam.ts. The own package is excluded at every depth, and a visited
    set ends a cycle. Each fail-closed case appends a row to `rows`.
    """
    closure = {}
    seen = {own_dir}
    stack = [own_dir]
    while stack:
        cur = stack.pop()
        data = _read_package_json(root, cur, rows)
        if data is None:
            continue
        for field in TS_DEP_FIELDS:
            deps = data.get(field)
            if deps is None:
                continue
            if not isinstance(deps, dict):
                rows.append(f"{cur}/package.json `{field}` is not an object")
                continue
            for name in sorted(deps):
                spec = deps[name]
                if not isinstance(spec, str):
                    rows.append(f"{cur}/package.json: {name} has a specifier that is not a string")
                    continue
                if spec.startswith("link:"):
                    rows.append(
                        f"{cur}/package.json: {name} uses a `link:` specifier ({spec}); A12 has no "
                        f"rule for it, so handle it on purpose (SMA-536)"
                    )
                    continue
                if spec.startswith("workspace:"):
                    target = names.get(name)
                    if target is None:
                        rows.append(
                            f"{cur}/package.json: the `workspace:` dependency {name} maps to no package "
                            f"directory under ts/packages or ts/apps"
                        )
                        continue
                    kind = "workspace"
                elif spec.startswith("file:"):
                    target = os.path.normpath(os.path.join(cur, spec[len("file:"):]))
                    if not (root / target).is_dir():
                        rows.append(f"{cur}/package.json: {name} points at `{spec}`, which does not exist")
                        continue
                    kind = "file"
                else:
                    continue
                if target in seen:
                    continue
                seen.add(target)
                closure[name] = (kind, target)
                stack.append(target)
    return closure


def _export_leaves(node, where, rows):
    """The string leaves of an `exports` value. A condition object is walked; a null is ignored."""
    if node is None:
        return []
    if isinstance(node, str):
        return [node]
    if isinstance(node, dict):
        leaves = []
        for key in sorted(node):
            leaves += _export_leaves(node[key], where, rows)
        return leaves
    rows.append(
        f"{where}: `exports` holds a {type(node).__name__}, which A12 cannot read (it walks strings, "
        f"condition objects and null only)"
    )
    return []


def tsc_required_inputs(root, closure, rows):
    """Return (want, has_binding): the inputs one `tsc` task must declare for its closure (spec §3.3)."""
    want = {TS_BASE_TSCONFIG}
    has_binding = False
    for name in sorted(closure):
        kind, pkg_dir = closure[name]
        if kind == "file":
            has_binding = True
        want.add(f"{pkg_dir}/package.json")
        data = _read_package_json(root, pkg_dir, rows)
        if data is None:
            continue
        if kind == "workspace":
            want.add(f"{pkg_dir}/src/**/*")
            for leaf in _export_leaves(data.get("exports"), f"{pkg_dir}/package.json", rows):
                if not leaf.startswith("./"):
                    rows.append(f"{pkg_dir}/package.json: the `exports` target {leaf!r} does not start with `./`")
                    continue
                rel = leaf[2:]
                if rel.startswith("src/"):
                    continue
                head, sep, _ = rel.partition("/")
                want.add(f"{pkg_dir}/{head}/**/*" if sep else f"{pkg_dir}/{rel}")
            continue
        files = data.get("files") or []
        if not isinstance(files, list):
            rows.append(f"{pkg_dir}/package.json `files` is not a list")
            continue
        for entry in files:
            if isinstance(entry, str) and entry.endswith(".d.ts"):
                want.add(f"{pkg_dir}/{entry}")
    if has_binding:
        want.add(PREFLIGHT_SCRIPT)
    return want, has_binding


def ts_tsc_analysis(projects, root):
    """The derivation A12a and A12b share.

    Returns (tasks, per_task, names, walk_rows). `per_task[target]` holds `own_name` (the task's
    own package name or None), `closure`, `want` and `binding` (the closure holds a `file:`
    binding). `walk_rows` holds every fail-closed row from reading package.json files.
    """
    walk_rows = []
    tasks = derive_tsc_tasks(projects)
    names = ts_workspace_packages(root, walk_rows)
    per_task = {}
    for target in sorted(tasks):
        pid, _, _task = target.partition(":")
        own = projects[pid]["source_dir"]
        own_pkg = _read_package_json(root, own, walk_rows)
        closure = ts_closure(root, own, names, walk_rows)
        want, binding = tsc_required_inputs(root, closure, walk_rows)
        per_task[target] = {
            "own_name": own_pkg.get("name") if own_pkg else None,
            "closure": closure,
            "want": want,
            "binding": binding,
        }
    return tasks, per_task, names, list(dict.fromkeys(walk_rows))


def check_ts_tsc_inputs(projects, root, floor=REQUIRED_TSC_TASKS, closure_floor=REQUIRED_TS_CLOSURE):
    """Return the A12a violation list: `tsc` tasks that do not key on their package.json closure.

    CONTAINMENT, per task, over both input buckets — the shape of A7. The lists are hand-written
    and correctly hold other entries, so strict equality would be wrong. A task that reports no
    input bucket is a violation, never a skip.

    `root` is POSITIONAL AND REQUIRED, never defaulted, for the reason in A7's docstring (SMA-560
    I3): the package.json walk is the whole assertion, and a `root=None` default would make it
    opt-in.
    """
    tasks, per_task, names, walk_rows = ts_tsc_analysis(projects, root)
    rows = []
    for target in sorted(set(floor or ()) - tasks):
        rows.append(
            f"FLOOR: {target} is not matched by the `tsc` token, so A12 asserts nothing about it "
            f"(see TSC_TOKEN_RE; a `tsc` behind a wrapper script is invisible)"
        )
    for pkg, required in sorted((closure_floor or {}).items()):
        pkg_dir = names.get(pkg)
        if pkg_dir is None:
            rows.append(
                f"FLOOR: {pkg} is not a package under ts/packages or ts/apps, so its closure cannot "
                f"be derived"
            )
            continue
        derived = ts_closure(root, pkg_dir, names, walk_rows)
        for missing in sorted(set(required) - set(derived)):
            rows.append(
                f"FLOOR: the closure of {pkg} no longer derives {missing} — the package.json walk "
                f"is broken, so A12 asserts nothing"
            )
    rows += walk_rows
    for target in sorted(tasks):
        pid, _, task = target.partition(":")
        proj = projects[pid]
        info = per_task[target]
        files = (proj.get("task_inputs") or {}).get(task)
        globs = (proj.get("task_input_globs") or {}).get(task)
        if files is None or globs is None:
            rows.append(
                f"{target} reported no `inputFiles`/`inputGlobs` — moon's output shape changed, so "
                f"this assertion cannot be evaluated (treated as a violation, never skipped)"
            )
        else:
            observed = set(files) | set(globs)
            for entry in sorted(info["want"] - observed):
                rows.append(f"{target} inputs omit {entry}")
        reads_proto = info["own_name"] == PROTO_PACKAGE or PROTO_PACKAGE in info["closure"]
        deps = (proj.get("tasks") or {}).get(task) or []
        if reads_proto and PROTO_GENERATE_DEP not in deps:
            rows.append(
                f"{target} deps omit {PROTO_GENERATE_DEP} — it reads @paigasus/proto's generated "
                f"tree, so it must run after the generator for a deterministic cache key"
            )
    return list(dict.fromkeys(rows))
```

- [ ] **Step 4: Register `a12a`.** Replace line 4376:

```python
EXPECTED_FINDING_KEYS = ("a1", "a2", "a3", "a4-lint", "a4-fmt", "a5", "a6", "a7", "a8", "a9", "a10", "a11", "a12a")
```

In `collect_findings`, after the line `             "    missing."),` (the end of the `a11` tuple) and before `    ]`, insert:

```python
        ("a12a", check_ts_tsc_inputs(projects, root),
             "A ts task that runs `tsc` does not key on a file `tsc` reads through its package.json\n"
             "    dependency closure, so a change there SELECTS NOTHING for that task and Moon\n"
             "    serves a cached PASS (SMA-536).\n"
             "    Fix: add the missing entry to that task's `inputs` in its own moon.yml. For each\n"
             "    workspace package in the closure: `/<dir>/src/**/*`, `/<dir>/package.json` and each\n"
             "    `exports` target outside src/. For each `file:` binding: `/<dir>/package.json` and\n"
             "    each `.d.ts` in its `files`. Always `/ts/tsconfig.base.json`, and\n"
             "    `/ts/scripts/check-installed-bindings.mjs` when the closure holds a binding. A\n"
             "    `deps omit contracts:generate` row needs `deps: ['contracts:generate']` on that task.\n"
             "    Extra inputs are ALLOWED (this is containment, like A7).\n"
             "    A `FLOOR:` row, or a row about an unreadable package.json, means the check itself\n"
             "    cannot be trusted — fix that first."),
```

In `self_test()`, replace `print("  OK   [parity] all eleven assertions fire on synthetic violations")` with:

```python
    print("  OK   [parity] all twelve assertions fire on synthetic violations")
```

In the header comment, after the two A11 lines (31-32), insert:

```python
#
# A12 (SMA-536) is about ts `tsc` tasks: A12a asserts that each one keys on its package.json
# dependency closure, and A12b that each one whose closure holds a `file:` binding runs the
# installed-typings preflight before `tsc`. It reads package.json files from disk.
```

- [ ] **Step 5: Run the self-test, Ruff and the real run.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"
/opt/homebrew/bin/bash ci/ruff/run.sh; echo "ruff rc=$?"
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma536}"; mkdir -p "$SCRATCH"
python3 ci/affected-graph/cargo_moon_parity.py > "$SCRATCH/parity.log" 2>&1; echo "real rc=$?"; tail -45 "$SCRATCH/parity.log"
```

Expected: `  OK   [parity] all twelve assertions fire on synthetic violations`, `rc=0`; `ruff rc=0`; the real run prints the A12a title and rows such as `paigasus-kernel-ts:typecheck inputs omit rs/crates/bindings/paigasus-node-bindings/index.d.ts` (Task 4 Step 6 records the full list). The real run's rc is 1 until Task 7. If Ruff reports a finding in the new code, fix it in place and re-run this step.

- [ ] **Step 6: Commit.**

```bash
git add ci/affected-graph/cargo_moon_parity.py
git commit -m "feat(ci): add A12a, the tsc closure-input assertion, to the parity gate" \
  -m "A12a derives every ts task that runs tsc from moon query projects, walks its package.json closure on disk, and asserts containment of the inputs tsc reads. The task floor and the closure floor stop it passing vacuously. The real run reds until the moon.yml rows are fixed in later commits." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 4: A12b — the preflight runs before `tsc`; registration, README, `ci/CLAUDE.md`, and the real-tree measurement

**Files:**
- Modify: `ci/affected-graph/cargo_moon_parity.py` (append to the A12 block from Task 3; `self_test()` after the A12a rows; `EXPECTED_FINDING_KEYS`; `collect_findings`; `main()` PASS text at lines 4533-4542 before this task's edits)
- Modify: `ci/affected-graph/README.md` (insert after line 288, the end of the A10 bullet; line 355 `A1-A11`)
- Modify: `ci/CLAUDE.md` (insert after line 63)
- Modify: this plan, execution-log entry E2
- Test: `python3 ci/affected-graph/cargo_moon_parity.py --self-test`

**Interfaces:**
- Consumes: `ts_tsc_analysis`, `TSC_TOKEN_RE`, `PREFLIGHT_RE`, `PREFLIGHT_SCRIPT`, `REQUIRED_TSC_TASKS` and the self-test helpers `a12`, `a12_tree`, `a12_floor`, `a12_pre`, `a12_tsc`, `_a12_copy`, `_a12_run` (Task 3).
- Produces: `preflight_order_verdict(blob: str) -> str | None`; `check_ts_tsc_preflight(projects, root, floor=REQUIRED_TSC_TASKS) -> list[str]`; findings key `"a12b"`; scratch helper `$SCRATCH/a12rows.py`.

- [ ] **Step 1: Write the failing self-test rows.** Insert directly after the A12a-r block from Task 3 (after its `failures.append("derive_tsc_tasks accepted a task with no command, script or args")`):

```python
    def _a12b(fixture=None, tree=None, **kw):
        kw.setdefault("floor", a12_floor)
        return _a12_run(
            check_ts_tsc_preflight, a12 if fixture is None else fixture,
            a12_tree if tree is None else tree, **kw,
        )

    rows = _a12b()
    if rows != []:
        failures.append(f"A12b reported violations on a complete fixture: {rows}")

    # The order verdict, exercised directly. `None` means no violation; a string must appear in the
    # verdict. Row 2 is moon's joined form of a script task (the first word repeats).
    for blob, want in (
        ("node ../../../ts/scripts/check-installed-bindings.mjs && pnpm exec tsc -p tsconfig.json --noEmit", None),
        ("node node ../../../ts/scripts/check-installed-bindings.mjs && pnpm exec tsc -p tsconfig.json --noEmit", None),
        ("touch a && pnpm exec napi build && node ../../../ts/scripts/check-installed-bindings.mjs && pnpm exec tsc -p x", None),
        ("node ts/scripts/check-installed-bindings.mjs && tsc", None),
        ("pnpm exec tsc -p x", "does not run"),
        ("node ../../../ts/scripts/check-installed-bindings.mjsx && pnpm exec tsc -p x", "does not run"),
        ("echo ts/scripts/check-installed-bindings.mjs && pnpm exec tsc -p x", "does not run"),
        ("pnpm exec tsc -p x && node ts/scripts/check-installed-bindings.mjs", "after `tsc`"),
        ("node ts/scripts/check-installed-bindings.mjs; pnpm exec tsc -p x", "with `;`"),
        ("node ts/scripts/check-installed-bindings.mjs || pnpm exec tsc -p x", "with `||`"),
        ("node ts/scripts/check-installed-bindings.mjs | pnpm exec tsc -p x", "with `|`"),
        ("node ts/scripts/check-installed-bindings.mjs & pnpm exec tsc -p x", "with `&`"),
        ("node ts/scripts/check-installed-bindings.mjs\npnpm exec tsc -p x", "with a newline"),
    ):
        got = preflight_order_verdict(blob)
        if (want is None and got is not None) or (want is not None and (got is None or want not in got)):
            failures.append(
                f"preflight_order_verdict({blob!r}) is {got!r}, expected "
                f"{'no violation' if want is None else repr(want)}"
            )

    # A12b-a: a binding task with no preflight.
    broken = _a12_copy(a12)
    broken["k-ts"]["invocations"]["typecheck"] = a12_tsc
    if not any(r.startswith("k-ts:typecheck does not run") for r in _a12b(broken)):
        failures.append("A12b did not fire on a binding task with no preflight")

    # A12b-b: a preflight after `tsc`, read PER TASK (c-ts:build keeps the right order).
    broken = _a12_copy(a12)
    broken["c-ts"]["invocations"]["typecheck"] = a12_tsc + " && node ../../../ts/scripts/check-installed-bindings.mjs"
    rows = _a12b(broken)
    if not any(r.startswith("c-ts:typecheck runs") and "after `tsc`" in r for r in rows):
        failures.append("A12b did not fire on a preflight after `tsc`")
    if any(r.startswith("c-ts:build ") for r in rows):
        failures.append("A12b blamed `build` for a script that lives on `typecheck`")

    # A12b-c: a preflight joined with `;`.
    broken = _a12_copy(a12)
    broken["app-ts"]["invocations"]["typecheck"] = a12_pre.replace(" && ", "; ", 1)
    if not any(r.startswith("app-ts:typecheck joins") and "with `;`" in r for r in _a12b(broken)):
        failures.append("A12b did not fire on a preflight joined with `;`")

    # A12b-d: the floor. A floor task with no binding in its closure is not examined, and so is
    # every task when the kernel loses its binding edge. Both must red, with THIS branch's prefix.
    if not any(r.startswith("FLOOR:") and "p-ts:typecheck" in r for r in _a12b(floor=("p-ts:typecheck",))):
        failures.append("A12b's floor did not fire on a floor task it does not examine")
    tree = _a12_copy(a12_tree)
    tree["ts/packages/kernel"]["dependencies"] = {}
    if not any(r.startswith("FLOOR:") and "k-ts:typecheck" in r for r in _a12b(tree=tree)):
        failures.append("A12b's floor did not fire when a floor task's closure lost its binding")
```

- [ ] **Step 2: Run the self-test and see it fail.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"
```

Expected: a traceback that ends `NameError: name 'check_ts_tsc_preflight' is not defined`, and `rc=1`.

- [ ] **Step 3: Add A12b.** Append after `check_ts_tsc_inputs` (the end of the Task 3 block), with two blank lines before each `def`:

```python
# The joins between the preflight and `tsc` that let `tsc` run after a failed preflight. `||` is
# checked before `|`, because it contains it. `&&` is removed first, so a lone `&` is a background job.
_PREFLIGHT_BAD_JOINS = (("||", "`||`"), (";", "`;`"), ("\n", "a newline"), ("|", "`|`"), ("&", "`&`"))


def preflight_order_verdict(blob):
    """None if `blob` runs the preflight before its first `tsc`, joined only with `&&`; else why not."""
    tsc = TSC_TOKEN_RE.search(blob)
    if tsc is None:
        return "runs no `tsc` token"
    pre = PREFLIGHT_RE.search(blob)
    if pre is None:
        return f"does not run {PREFLIGHT_SCRIPT} before `tsc`, so `tsc` can read stale installed typings"
    if pre.start() > tsc.start():
        return f"runs {PREFLIGHT_SCRIPT} after `tsc`, so `tsc` can read stale installed typings first"
    # Up to the END of the separator group, so the character just before `tsc` (a newline, a `;`)
    # is part of what is checked.
    leftover = blob[pre.end():tsc.end(1)].replace("&&", "")
    for op, shown in _PREFLIGHT_BAD_JOINS:
        if op in leftover:
            return (
                f"joins {PREFLIGHT_SCRIPT} to `tsc` with {shown}, not `&&`, so a failed preflight "
                f"does not stop `tsc`"
            )
    return None


def check_ts_tsc_preflight(projects, root, floor=REQUIRED_TSC_TASKS):
    """Return the A12b violation list: binding `tsc` tasks that do not run the preflight first.

    Examines every task A12 derives whose closure holds a `file:` binding. The package.json walk
    rows are A12a's to report; A12b drops them, so one broken manifest does not print under two
    titles. `root` is positional and required, as in A12a.
    """
    tasks, per_task, _names, _walk_rows = ts_tsc_analysis(projects, root)
    examined = {target for target in tasks if per_task[target]["binding"]}
    rows = []
    for target in sorted(set(floor or ()) - examined):
        rows.append(
            f"FLOOR: {target} is in the A12 floor, but A12b does not examine it — it runs no `tsc` "
            f"token, or its package closure no longer holds a `file:` binding"
        )
    for target in sorted(examined):
        pid, _, task = target.partition(":")
        verdict = preflight_order_verdict(projects[pid]["invocations"][task])
        if verdict:
            rows.append(f"{target} {verdict}")
    return rows
```

Register `a12b`. Replace the `EXPECTED_FINDING_KEYS` line:

```python
EXPECTED_FINDING_KEYS = ("a1", "a2", "a3", "a4-lint", "a4-fmt", "a5", "a6", "a7", "a8", "a9", "a10", "a11", "a12a", "a12b")
```

In `collect_findings`, after the `a12a` tuple, insert:

```python
        ("a12b", check_ts_tsc_preflight(projects, root),
             "A ts `tsc` task whose package closure holds a `file:` binding does not run the\n"
             "    installed-typings preflight before `tsc`, so `tsc` can read stale typings through\n"
             "    ts/node_modules and pass (SMA-536).\n"
             "    Fix: make the task a script that starts\n"
             "    `node ../../../ts/scripts/check-installed-bindings.mjs && pnpm exec tsc ...`.\n"
             "    A `FLOOR:` row means A12b examines less than the A12 floor — fix that first."),
```

In `main()`, replace the two PASS-text lines

```python
            f"every compiling cargo task inside rs/ keys on .cargo/config.toml, and every "
            f"workspace members entry is a literal path"
```

with:

```python
            f"every compiling cargo task inside rs/ keys on .cargo/config.toml, and every "
            f"workspace members entry is a literal path, and every ts tsc task keys on its "
            f"package.json closure and runs the installed-typings preflight first when that "
            f"closure holds a binding"
```

- [ ] **Step 4: Run the self-test and Ruff.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
python3 ci/affected-graph/cargo_moon_parity.py --self-test; echo "rc=$?"
/opt/homebrew/bin/bash ci/ruff/run.sh; echo "ruff rc=$?"
```

Expected: `  OK   [parity] all twelve assertions fire on synthetic violations`, `rc=0`, `ruff rc=0`.

- [ ] **Step 5: Write the scratch helper that prints the real-tree A12 rows.** Write `$SCRATCH/a12rows.py` (not committed):

```python
# usage, from the worktree root: python3 "$SCRATCH/a12rows.py"
# Prints the real-tree A12a and A12b rows, one per line, prefixed with the findings key.
import sys
from pathlib import Path

sys.path.insert(0, "ci/affected-graph")
import cargo_moon_parity as m  # noqa: E402

projects = m.moon_projects()
root = Path.cwd()
for row in m.check_ts_tsc_inputs(projects, root):
    print("a12a\t" + row)
for row in m.check_ts_tsc_preflight(projects, root):
    print("a12b\t" + row)
```

- [ ] **Step 6: Measure A12 on the real tree and record it in E2.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma536}"
python3 "$SCRATCH/a12rows.py" | tee "$SCRATCH/a12-before.txt"; wc -l < "$SCRATCH/a12-before.txt"
```

Expected (measured on 2026-10-04 by running a scratch copy of the Task 3-4 code against this tree; the self-test of that copy also passed every A12 row; your measured list is the record): 38 `a12a` rows and 6 `a12b` rows, 44 lines, no `FLOOR:` row and no package.json row. The `a12a` rows:
- `gateway-console-ts:typecheck` and `iam-console-ts:typecheck`: each omits `rs/crates/bindings/paigasus-node-bindings/index.d.ts`, `rs/crates/bindings/paigasus-node-bindings/package.json`, `ts/scripts/check-installed-bindings.mjs`.
- `paigasus-app-shell-ts:build` and `:typecheck`: each omits `ts/packages/paigasus-next-config/package.json`, `ts/packages/paigasus-next-config/src/**/*`, `ts/packages/paigasus-next-config/tsconfig.app.json`, `ts/packages/paigasus-proto/package.json`, `ts/packages/paigasus-proto/src/**/*`, and has `deps omit contracts:generate`.
- `paigasus-console-core-ts:build` and `:typecheck`: each omits the napi `index.d.ts`, the napi `package.json` and the preflight.
- `paigasus-discovery-ts:build`/`:typecheck` and `paigasus-sdk-ts:build`/`:typecheck`: each omits `ts/packages/paigasus-proto/package.json`.
- `paigasus-kernel-ts:build`: omits the napi `index.d.ts`, `rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts`, `rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts`, the preflight.
- `paigasus-kernel-ts:typecheck`: omits the napi `index.d.ts` and `package.json`, the wasm `package.json`, both wasm `.d.ts` files, the preflight.

The `a12b` rows: `does not run ts/scripts/check-installed-bindings.mjs before …` for `gateway-console-ts:typecheck`, `iam-console-ts:typecheck`, `paigasus-console-core-ts:build`, `paigasus-console-core-ts:typecheck`, `paigasus-kernel-ts:build`, `paigasus-kernel-ts:typecheck`.

Paste the whole `a12-before.txt` into E2. If the measured list differs from the prediction, record the difference in E2 and fix the measured rows, not the predicted ones, in Tasks 5 to 7. A `FLOOR:` row or a package.json row is a defect in Task 3 or 4: stop and fix it before you go on.

- [ ] **Step 7: Document A12.** In `ci/affected-graph/README.md`, after line 288 (the last line of the A10 bullet, `  the subcommand the redirected tool will run.`), insert:

```markdown
- **A12** (`check_ts_tsc_inputs` and `check_ts_tsc_preflight` in `cargo_moon_parity.py`, SMA-536,
  findings keys `a12a` and `a12b`) covers every task of a `language: typescript` project whose
  resolved invocation runs `tsc` (`TSC_TOKEN_RE`). It reads each task's `package.json` closure
  from disk: the transitive `workspace:` and `file:` specifiers in `dependencies`,
  `devDependencies` and `peerDependencies`, with upstream devDependencies included and the own
  package excluded. **A12a** asserts CONTAINMENT, per task and over both input buckets, like A7.
  For each workspace package `D` it demands `D/src/**/*`, `D/package.json` and each `exports`
  target outside `src/` (`D/<dir>/**/*` for a subdirectory target, the file itself for a root
  target). For each `file:` binding `B` it demands `B/package.json` and each `.d.ts` in `B`'s
  `files`. It demands `ts/tsconfig.base.json` of every task,
  `ts/scripts/check-installed-bindings.mjs` of every task whose closure holds a binding, and a
  direct `contracts:generate` in `deps` of every task whose package or closure is
  `@paigasus/proto`. **A12b** asserts that every task whose closure holds a binding runs that
  preflight before its first `tsc`, joined only with `&&`. The preflight is what makes the typings
  inputs real: `tsc` reads the INSTALLED copy under `ts/node_modules`, and the preflight fails
  unless that copy equals the committed file. A broken `package.json`, a `link:` specifier, a
  `workspace:` name with no package and a `file:` path that does not exist are each a row, never a
  skip. `REQUIRED_TSC_TASKS` and `REQUIRED_TS_CLOSURE` are the floors (rows prefixed `FLOOR:`).
  Accepted over-approximations: console-core and both apps reach `@paigasus/node-bindings` only
  through the kernel's `package.json` and import only the wasm `.` entry, so a napi glue change
  also runs their `tsc` tasks; and app-shell keys on next-config and `@paigasus/proto`, although
  its `tsconfig.json` excludes the fixture that uses next-config and discovery's `./client` graph
  does not reach proto. A per-import closure would need a TypeScript resolver in the gate.
  Limits: vitest `test` tasks are out of scope, because they resolve the bindings through
  `vitest.config.ts` aliases (a follow-up issue holds them); `next build`'s own type check is not a
  `tsc` task; own-package files outside `src/` (for example `tests/**/*`) are not asserted;
  relative reads that `package.json` does not declare are not seen; and a `tsc` behind a wrapper
  script is invisible (the floor catches only the loss of a known task).
```

In the same file, replace `A1-A11` on line 355 with `A1-A12`.

In `ci/CLAUDE.md`, after line 63 (`  `[package.metadata.cargo-machete] ignored` allowlist (prune once consumed).`), insert:

```markdown
- A new ts workspace dependency needs matching inputs on every `tsc` task of the package and of
  its dependents. A new `file:` binding also needs the preflight
  `node ../../../ts/scripts/check-installed-bindings.mjs &&` before `tsc`. If either is missing,
  A12 in `repo:affected-smoke` reds (SMA-536, `ci/affected-graph/README.md`).
```

- [ ] **Step 8: Commit.**

```bash
git add ci/affected-graph/cargo_moon_parity.py ci/affected-graph/README.md ci/CLAUDE.md docs/superpowers/plans/2026-10-04-sma-536-ts-typecheck-upstream-inputs.md
git commit -m "feat(ci): add A12b, which asserts the installed-typings preflight runs before tsc" \
  -m "A12b reds a tsc task whose package closure holds a file: binding when its script does not run check-installed-bindings.mjs first, joined only with &&. README and ci/CLAUDE.md describe A12, its over-approximations and its limits. The execution log records the measured real-tree rows." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 5: `paigasus-kernel-ts` — the preflight, the typings inputs and the new `typecheck` override (spec §3.2)

**Files:**
- Modify: `ts/packages/paigasus-kernel/moon.yml:40-46` (comment and `build` script), `:67-82` (inputs and comments), after `:111` (new `typecheck`), `:282-283` (run-order warning)
- Modify: `ts/CLAUDE.md:153-156`
- Test: `python3 "$SCRATCH/a12rows.py"` (rows for `paigasus-kernel-ts:*` disappear), `moon run paigasus-kernel-ts:build paigasus-kernel-ts:typecheck --force`

**Interfaces:**
- Consumes: `ts/scripts/check-installed-bindings.mjs` (Task 2), A12 (Tasks 3-4).
- Produces: `paigasus-kernel-ts:typecheck` as a script task with the eight inputs of spec §3.2; `paigasus-kernel-ts:build` with the preflight and four new inputs.

- [ ] **Step 1: Confirm the failing rows.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma536}"
python3 "$SCRATCH/a12rows.py" | grep paigasus-kernel-ts
```

Expected: the 10 `a12a` rows and 2 `a12b` rows for `paigasus-kernel-ts` that E2 lists.

- [ ] **Step 2: Edit the `build` comment and script.** Replace lines 42-44:

```yaml
  # wasm inputs stay: `napi build` keeps this an FFI task, so A7 demands the dependsOn closure's
  # sources and manifests, and `tsc` reads the committed paigasus_wasm.d.ts through
  # src/binding-parity.types.ts.
```

with:

```yaml
  # wasm inputs stay: `napi build` keeps this an FFI task, so A7 demands the dependsOn closure's
  # sources and manifests. `tsc` reads paigasus_wasm.d.ts through src/binding-parity.types.ts, from
  # the installed copy, which the preflight holds equal to the committed file (SMA-536).
```

In the `build` script on line 46, replace `--no-dts-cache && pnpm exec tsc -p tsconfig.json --noEmit'` with:

```yaml
--no-dts-cache && node ../../../ts/scripts/check-installed-bindings.mjs && pnpm exec tsc -p tsconfig.json --noEmit'
```

- [ ] **Step 3: Replace the `build` inputs comment and add the typings inputs.** Replace lines 67-82 (from `      # ...and the binding's manifests, so a napi-config (package.json) or dependency (Cargo.toml)` through `      - '/rs/crates/bindings/paigasus-wasm/package.json'`) with:

```yaml
      # ...and the binding's manifests, so a napi-config (package.json) or dependency (Cargo.toml)
      # change re-keys the build even without a Rust src diff (cache-input completeness, CodeRabbit).
      # This task writes the glue to the scratch files index.fresh.js and index.fresh.d.ts only
      # (SMA-667). The committed index.js and index.d.ts are not its outputs.
      - '/rs/crates/bindings/paigasus-node-bindings/Cargo.toml'
      - '/rs/crates/bindings/paigasus-node-bindings/package.json'
      # SMA-536. The `tsc` of this task reads the INSTALLED copy of index.d.ts and of the two wasm
      # typings through ts/node_modules (src/binding-parity.types.ts). The preflight
      # ts/scripts/check-installed-bindings.mjs runs first and fails unless the installed copies
      # equal the committed files. So `tsc` reads bytes equal to the committed files, and the
      # committed files are real inputs here. The committed index.js stays out: `tsc` does not read
      # it. ci/affected-graph/cargo_moon_parity.py A12 asserts these lines.
      - '/rs/crates/bindings/paigasus-node-bindings/index.d.ts'
      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts'
      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts'
      - '/ts/scripts/check-installed-bindings.mjs'
      # wasm binding: same cache-input completeness as the napi crate — a kernel or wasm-binding src
      # edit or a manifest change must re-key this task. Since SMA-634 this task does not build the
      # wasm glue; `generate-wasm` is its only writer.
      - '/rs/crates/bindings/paigasus-wasm/src/**/*'
      - '/rs/crates/bindings/paigasus-wasm/Cargo.toml'
      - '/rs/crates/bindings/paigasus-wasm/package.json'
```

- [ ] **Step 4: Add the `typecheck` override.** After line 111 (`      - '/rs/crates/bindings/paigasus-node-bindings/paigasus-node-bindings.*.node'`, the end of `build`'s `outputs`), insert:

```yaml
  # SMA-536. Overrides the inherited `typecheck`. The default merge keeps its inputs
  # (@group(sources), tsconfig.json, package.json, /ts/tsconfig.base.json, /ts/pnpm-lock.yaml).
  # `tsc` reads both binding typings through src/binding-parity.types.ts, from the INSTALLED copy
  # under ts/node_modules. The preflight runs first and fails, with the repair command, unless that
  # copy equals the committed files. So the committed typings are real inputs here.
  # No `deps: ['^:build']`: `tsc` reads only committed files, so a Rust build orders nothing here
  # and does not confer affectedness (SMA-528). tests/**/* and vitest.config.ts are here because
  # tsconfig.json `include` names them. ci/affected-graph/cargo_moon_parity.py A12 asserts the
  # binding lines and the preflight.
  typecheck:
    script: 'node ../../../ts/scripts/check-installed-bindings.mjs && pnpm exec tsc -p tsconfig.json --noEmit'
    inputs:
      - 'tests/**/*'
      - 'vitest.config.ts'
      - '/rs/crates/bindings/paigasus-node-bindings/index.d.ts'
      - '/rs/crates/bindings/paigasus-node-bindings/package.json'
      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts'
      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts'
      - '/rs/crates/bindings/paigasus-wasm/package.json'
      - '/ts/scripts/check-installed-bindings.mjs'
```

- [ ] **Step 5: Extend the run-order warning.** Replace the two lines (282-283 before this task's insertions)

```yaml
  # committed files agree with the generator. Run it as its OWN command, not in the same `moon run`
  # as `test`: the two tasks have no order, and `test` can hash its inputs before this task writes.
```

with:

```yaml
  # committed files agree with the generator. Run it as its OWN command, not in the same `moon run`
  # as `test`, `build` or `typecheck` (SMA-536): this task has no order with them, and each of them
  # hashes index.d.ts, so it can hash its inputs before this task writes.
```

- [ ] **Step 6: Update `ts/CLAUDE.md`.** Replace lines 153-156:

```markdown
  The `build` task runs `tsc`, and `tsc` reads the installed copy of `index.d.ts` through
  `ts/node_modules`. A rename-write breaks the pnpm hard link of that copy. After a checkout, a
  rebase or `version-lockstep --write` that replaces `index.d.ts`, run `rm -rf ts/node_modules &&
  pnpm -C ts install`, as in the wasm paragraph above.
```

with:

```markdown
  The `build` and `typecheck` tasks run `tsc`, and `tsc` reads the installed copy of the binding
  typings through `ts/node_modules`. A rename-write breaks the pnpm hard link of that copy. Since
  SMA-536, `ts/scripts/check-installed-bindings.mjs` runs before `tsc` in every task whose
  package closure holds a binding. It compares the installed typings with the committed files. If
  they differ, the task fails and prints `rm -rf ts/node_modules && pnpm -C ts install`, as in the
  wasm paragraph above. Before SMA-536 such a task passed against the stale typings.
  `repo:affected-smoke`'s A12 asserts the inputs and the preflight of these tasks.
```

- [ ] **Step 7: Verify.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma536}"
python3 "$SCRATCH/a12rows.py" | grep paigasus-kernel-ts; echo "kernel rows grep rc=$? (1 = none)"
python3 ci/affected-graph/cargo_moon_parity.py --self-test >/dev/null; echo "self-test rc=$?"
moon run paigasus-kernel-ts:build paigasus-kernel-ts:typecheck --force; echo "rc=$?"
```

Expected: no kernel rows (`grep rc=1`); `self-test rc=0`; `rc=0`, and the task output contains `installed-bindings: checked 2 file: dependencies; the installed typings equal the committed files` for both tasks. The real parity run still has rows for other projects; A5 and A7 rows must not appear (`python3 ci/affected-graph/cargo_moon_parity.py 2>&1 | grep -c 'FFI build task\|wrapper'` prints `0`).

- [ ] **Step 8: Commit.**

```bash
git add ts/packages/paigasus-kernel/moon.yml ts/CLAUDE.md
git commit -m "feat(ts): key the kernel build and typecheck on the committed binding typings" \
  -m "Both tasks run the installed-typings preflight before tsc. The preflight proves that tsc reads bytes equal to the committed typings, so the committed files become inputs. typecheck is a new override with no Rust build dep. ts/CLAUDE.md describes the new failure and its repair." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 6: console-core and both apps — the preflight and the napi typings inputs

**Files:**
- Modify: `ts/packages/paigasus-console-core/moon.yml:26-55`
- Modify: `ts/apps/iam-console/moon.yml:204-267` (`typecheck`)
- Modify: `ts/apps/gateway-console/moon.yml:186-244` (`typecheck`)
- Test: `python3 "$SCRATCH/a12rows.py"`; forced runs of the changed tasks

**Interfaces:**
- Consumes: the preflight (Task 2); A12 (Tasks 3-4).
- Produces: `paigasus-console-core-ts:build`/`:typecheck`, `iam-console-ts:typecheck` and `gateway-console-ts:typecheck` as script tasks that run the preflight first, with three new inputs each.

- [ ] **Step 1: Confirm the failing rows.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma536}"
python3 "$SCRATCH/a12rows.py" | grep -E 'paigasus-console-core-ts|iam-console-ts|gateway-console-ts'
```

Expected: 12 `a12a` rows and 4 `a12b` rows (E2).

- [ ] **Step 2: console-core.** Replace

```yaml
  build:
    deps: ['contracts:generate']
    inputs: &upstreams
```

with:

```yaml
  # SMA-536: both `tsc` tasks run the installed-typings preflight first. Their package closure holds
  # both `file:` bindings, through @paigasus/kernel.
  build:
    script: 'node ../../../ts/scripts/check-installed-bindings.mjs && pnpm exec tsc -p tsconfig.json --noEmit'
    deps: ['contracts:generate']
    inputs: &upstreams
```

Replace

```yaml
      - 'testing/**/*'
  typecheck:
    deps: ['contracts:generate']
    inputs: *upstreams
```

with:

```yaml
      - 'testing/**/*'
      # SMA-536. The preflight, and the napi binding's typings and manifest. The napi lines are an
      # OVER-APPROXIMATION: this package reaches @paigasus/node-bindings only through the kernel's
      # package.json and imports only the wasm `.` entry. A12 in
      # ci/affected-graph/cargo_moon_parity.py demands the whole closure, as A7 accepts the .pyi
      # case (spec §3.4).
      - '/rs/crates/bindings/paigasus-node-bindings/index.d.ts'
      - '/rs/crates/bindings/paigasus-node-bindings/package.json'
      - '/ts/scripts/check-installed-bindings.mjs'
  typecheck:
    script: 'node ../../../ts/scripts/check-installed-bindings.mjs && pnpm exec tsc -p tsconfig.json --noEmit'
    deps: ['contracts:generate']
    inputs: *upstreams
```

- [ ] **Step 3: iam-console.** Replace

```yaml
    # tsc resolves @paigasus/proto's generated tree through the source `exports` of @paigasus/sdk
    # and @paigasus/discovery, so this task needs the generator ordering too.
    deps: ['contracts:generate']
    inputs:
      - 'app/**/*'
      - 'lib/**/*'
      - 'proxy.ts'
      # The root-level config files. tsconfig.json includes **/*.ts, so tsc type-checks them, but
      # no inherited input or source group reaches them. Without these, a type error there serves a
      # cached typecheck PASS (SMA-511).
```

with:

```yaml
    # tsc resolves @paigasus/proto's generated tree through the source `exports` of @paigasus/sdk
    # and @paigasus/discovery, so this task needs the generator ordering too.
    #
    # SMA-536: the installed-typings preflight runs first, because this package closure holds both
    # `file:` bindings through @paigasus/console-core and @paigasus/kernel.
    script: 'node ../../../ts/scripts/check-installed-bindings.mjs && pnpm exec tsc -p tsconfig.json --noEmit'
    deps: ['contracts:generate']
    inputs:
      - 'app/**/*'
      - 'lib/**/*'
      - 'proxy.ts'
      # The root-level config files. tsconfig.json includes **/*.ts, so tsc type-checks them, but
      # no inherited input or source group reaches them. Without these, a type error there serves a
      # cached typecheck PASS (SMA-511).
```

Replace

```yaml
      - '/rs/crates/bindings/paigasus-wasm/package.json'

  test:
    # Two jobs in one task. SMA-502's vitest suite boots the BUILT standalone server, and
```

with:

```yaml
      - '/rs/crates/bindings/paigasus-wasm/package.json'
      # SMA-536. The preflight, and the napi binding's typings and manifest: an accepted
      # OVER-APPROXIMATION, because this app reaches the napi binding only through the kernel's
      # package.json (spec §3.4). A12 in ci/affected-graph/cargo_moon_parity.py asserts these lines.
      - '/rs/crates/bindings/paigasus-node-bindings/index.d.ts'
      - '/rs/crates/bindings/paigasus-node-bindings/package.json'
      - '/ts/scripts/check-installed-bindings.mjs'

  test:
    # Two jobs in one task. SMA-502's vitest suite boots the BUILT standalone server, and
```

- [ ] **Step 4: gateway-console.** Replace

```yaml
    # tsc resolves @paigasus/proto's generated tree through the source `exports` of @paigasus/sdk
    # and @paigasus/discovery, so this task needs the generator ordering too.
    deps: ['contracts:generate']
    inputs:
      - 'app/**/*'
      - 'lib/**/*'
      - 'proxy.ts'
      # The root-level config files. tsconfig.json includes **/*.ts, so tsc type-checks them, but
      # no inherited input or source group reaches them. Without these, a type error there serves
      # a cached typecheck PASS.
```

with:

```yaml
    # tsc resolves @paigasus/proto's generated tree through the source `exports` of @paigasus/sdk
    # and @paigasus/discovery, so this task needs the generator ordering too.
    #
    # SMA-536: the installed-typings preflight runs first, because this package closure holds both
    # `file:` bindings through @paigasus/console-core and @paigasus/kernel.
    script: 'node ../../../ts/scripts/check-installed-bindings.mjs && pnpm exec tsc -p tsconfig.json --noEmit'
    deps: ['contracts:generate']
    inputs:
      - 'app/**/*'
      - 'lib/**/*'
      - 'proxy.ts'
      # The root-level config files. tsconfig.json includes **/*.ts, so tsc type-checks them, but
      # no inherited input or source group reaches them. Without these, a type error there serves
      # a cached typecheck PASS.
```

Replace

```yaml
      - '/rs/crates/bindings/paigasus-wasm/package.json'

  test:
    # Two jobs in one task. The vitest suite boots the BUILT standalone server (Task 5), and the
```

with:

```yaml
      - '/rs/crates/bindings/paigasus-wasm/package.json'
      # SMA-536. The preflight, and the napi binding's typings and manifest: an accepted
      # OVER-APPROXIMATION, because this app reaches the napi binding only through the kernel's
      # package.json (spec §3.4). A12 in ci/affected-graph/cargo_moon_parity.py asserts these lines.
      - '/rs/crates/bindings/paigasus-node-bindings/index.d.ts'
      - '/rs/crates/bindings/paigasus-node-bindings/package.json'
      - '/ts/scripts/check-installed-bindings.mjs'

  test:
    # Two jobs in one task. The vitest suite boots the BUILT standalone server (Task 5), and the
```

- [ ] **Step 5: Verify.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma536}"
python3 "$SCRATCH/a12rows.py" | grep -E 'paigasus-console-core-ts|iam-console-ts|gateway-console-ts'; echo "grep rc=$? (1 = none)"
moon run paigasus-console-core-ts:build paigasus-console-core-ts:typecheck iam-console-ts:typecheck gateway-console-ts:typecheck --force; echo "rc=$?"
git status --short
```

Expected: `grep rc=1`; `rc=0`, with the `installed-bindings: checked 2 …` line in each of the four task outputs; `git status` shows only the three `moon.yml` files (see the BSR constraint if a generated file is deleted).

- [ ] **Step 6: Commit.**

```bash
git add ts/packages/paigasus-console-core/moon.yml ts/apps/iam-console/moon.yml ts/apps/gateway-console/moon.yml
git commit -m "feat(ts): run the binding preflight in the console-core and app tsc tasks" \
  -m "Each tsc task whose closure holds a binding runs check-installed-bindings.mjs first and keys on the napi typings, the napi manifest and the preflight. The napi lines are an accepted over-approximation, as the A12 README entry records." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 7: sdk, discovery and app-shell — the remaining closure inputs, until A12 is green

**Files:**
- Modify: `ts/packages/paigasus-sdk/moon.yml:30-33`, `:44-47`
- Modify: `ts/packages/paigasus-discovery/moon.yml:35-42`
- Modify: `ts/packages/paigasus-app-shell/moon.yml:27-42`
- Test: `python3 ci/affected-graph/cargo_moon_parity.py` exits 0

**Interfaces:**
- Consumes: A12 (Tasks 3-4).
- Produces: a real parity run with no A12 row.

- [ ] **Step 1: Confirm the failing rows.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma536}"
python3 "$SCRATCH/a12rows.py"
```

Expected: 16 `a12a` rows (sdk 2, discovery 2, app-shell 12) and no `a12b` row.

- [ ] **Step 2: sdk.** Replace

```yaml
  build:
    deps: ['contracts:generate']
    inputs:
      - '/ts/packages/paigasus-proto/src/**/*'
      # SMA-575 spec § 3.3.
```

with:

```yaml
  build:
    deps: ['contracts:generate']
    inputs:
      - '/ts/packages/paigasus-proto/src/**/*'
      # SMA-536: the manifest holds the `exports` map that `tsc` resolves @paigasus/proto through.
      # ci/affected-graph/cargo_moon_parity.py A12 asserts it.
      - '/ts/packages/paigasus-proto/package.json'
      # SMA-575 spec § 3.3.
```

Replace

```yaml
  typecheck:
    deps: ['contracts:generate']
    inputs:
      - '/ts/packages/paigasus-proto/src/**/*'
      # Mirrors `build` above
```

with:

```yaml
  typecheck:
    deps: ['contracts:generate']
    inputs:
      - '/ts/packages/paigasus-proto/src/**/*'
      - '/ts/packages/paigasus-proto/package.json'
      # Mirrors `build` above
```

Note: the two `old_string`s above end mid-line (`# SMA-575 spec § 3.3.` and `# Mirrors `build` above`); the rest of each line stays as it is.

- [ ] **Step 3: discovery.** Replace

```yaml
  build:
    deps: ['contracts:generate']
    inputs:
      - '/ts/packages/paigasus-proto/src/**/*'
  typecheck:
    deps: ['contracts:generate']
    inputs:
      - '/ts/packages/paigasus-proto/src/**/*'
  test:
```

with:

```yaml
  # SMA-536: the proto manifest holds the `exports` map that `tsc` resolves @paigasus/proto
  # through. ci/affected-graph/cargo_moon_parity.py A12 asserts it on both `tsc` tasks.
  build:
    deps: ['contracts:generate']
    inputs:
      - '/ts/packages/paigasus-proto/src/**/*'
      - '/ts/packages/paigasus-proto/package.json'
  typecheck:
    deps: ['contracts:generate']
    inputs:
      - '/ts/packages/paigasus-proto/src/**/*'
      - '/ts/packages/paigasus-proto/package.json'
  test:
```

- [ ] **Step 4: app-shell.** Replace lines 27-42 (the whole `build` and `typecheck` blocks):

```yaml
  build:
    inputs:
      - '/ts/packages/paigasus-ui/src/**/*'
      - '/ts/packages/paigasus-ui/package.json'
      - '/ts/packages/paigasus-auth/src/**/*'
      - '/ts/packages/paigasus-auth/package.json'
      - '/ts/packages/paigasus-discovery/src/**/*'
      - '/ts/packages/paigasus-discovery/package.json'
  typecheck:
    inputs:
      - '/ts/packages/paigasus-ui/src/**/*'
      - '/ts/packages/paigasus-ui/package.json'
      - '/ts/packages/paigasus-auth/src/**/*'
      - '/ts/packages/paigasus-auth/package.json'
      - '/ts/packages/paigasus-discovery/src/**/*'
      - '/ts/packages/paigasus-discovery/package.json'
```

with:

```yaml
  #
  # SMA-536. ci/affected-graph/cargo_moon_parity.py A12 demands every package in this package.json
  # closure of both `tsc` tasks. next-config (a devDependency) and @paigasus/proto (through
  # @paigasus/discovery) are OVER-APPROXIMATIONS: tsconfig.json excludes the fixture that uses
  # next-config, and discovery's `./client` graph does not reach proto. They are accepted, as A7
  # accepts the .pyi case (spec §3.4). contracts:generate deletes the proto tree before it rewrites
  # it, so a task that keys on that tree also runs after it.
  build:
    deps: ['contracts:generate']
    inputs:
      - '/ts/packages/paigasus-ui/src/**/*'
      - '/ts/packages/paigasus-ui/package.json'
      - '/ts/packages/paigasus-auth/src/**/*'
      - '/ts/packages/paigasus-auth/package.json'
      - '/ts/packages/paigasus-discovery/src/**/*'
      - '/ts/packages/paigasus-discovery/package.json'
      - '/ts/packages/paigasus-next-config/src/**/*'
      - '/ts/packages/paigasus-next-config/package.json'
      - '/ts/packages/paigasus-next-config/tsconfig.app.json'
      - '/ts/packages/paigasus-proto/src/**/*'
      - '/ts/packages/paigasus-proto/package.json'
  typecheck:
    deps: ['contracts:generate']
    inputs:
      - '/ts/packages/paigasus-ui/src/**/*'
      - '/ts/packages/paigasus-ui/package.json'
      - '/ts/packages/paigasus-auth/src/**/*'
      - '/ts/packages/paigasus-auth/package.json'
      - '/ts/packages/paigasus-discovery/src/**/*'
      - '/ts/packages/paigasus-discovery/package.json'
      - '/ts/packages/paigasus-next-config/src/**/*'
      - '/ts/packages/paigasus-next-config/package.json'
      - '/ts/packages/paigasus-next-config/tsconfig.app.json'
      - '/ts/packages/paigasus-proto/src/**/*'
      - '/ts/packages/paigasus-proto/package.json'
```

- [ ] **Step 5: Verify that A12 is green and the tasks pass.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma536}"
python3 "$SCRATCH/a12rows.py" | wc -l
python3 ci/affected-graph/cargo_moon_parity.py; echo "rc=$?"
moon run paigasus-sdk-ts:build paigasus-sdk-ts:typecheck paigasus-discovery-ts:build paigasus-discovery-ts:typecheck paigasus-app-shell-ts:build paigasus-app-shell-ts:typecheck --force; echo "rc=$?"
git status --short
```

Expected: `0`; a `PASS  cargo-moon-parity  -> … runs the installed-typings preflight first when that closure holds a binding` line and `rc=0`; `rc=0`; `git status` shows only the three `moon.yml` files.

- [ ] **Step 6: Commit.**

```bash
git add ts/packages/paigasus-sdk/moon.yml ts/packages/paigasus-discovery/moon.yml ts/packages/paigasus-app-shell/moon.yml
git commit -m "feat(ts): key sdk, discovery and app-shell tsc tasks on their full package closure" \
  -m "sdk and discovery gain the proto manifest. app-shell gains next-config and proto, and a direct contracts:generate dep. The app-shell entries are accepted over-approximations. A12 is now green on the real tree." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 8: `ci/affected-graph/run.sh` — `typecheck` in the filter, every case re-measured, the new wasm-typings case, the stale comments (spec §3.5)

**Files:**
- Modify: `ci/affected-graph/run.sh:77-83`, `:117`, `:336-337`, `:423-425`, `:614-620`, `:664-666`, `:682-684`, `:764-773` (napi cases and the new case), and every task-case CSV (by the rebaseline script)
- Modify: `ts/packages/paigasus-sdk/moon.yml` (the two stale comments at the original lines 38-41 and 48-51)
- Modify: `ci/affected-graph/README.md:455` (filter-widening example)
- Modify: this plan, execution-log entry E3
- Test: `/bin/bash ci/affected-graph/run.sh` and `/bin/bash ci/affected-graph/run.sh --negative-control`

**Interfaces:**
- Consumes: the moon.yml state after Task 7.
- Produces: `_assert_task_case_impl` filter `("build", "test", "lint", "test-e2e", "typecheck")`; case `wasm-typings->typecheck`; scratch helper `$SCRATCH/rebaseline.py`.

- [ ] **Step 1: Widen the filter (the failing test).** On line 117, replace

```python
        if name in ("build", "test", "lint", "test-e2e"):
```

with:

```python
        if name in ("build", "test", "lint", "test-e2e", "typecheck"):
```

- [ ] **Step 2: Run the suite and see it fail.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma536}"; mkdir -p "$SCRATCH"
/bin/bash ci/affected-graph/run.sh > "$SCRATCH/runsh.log" 2>&1; echo "rc=$?"; grep -E '^(PASS|FAIL)' "$SCRATCH/runsh.log" | head -60
```

Expected: many `FAIL  [<label>] affected TASK set != expected set` lines (the `ui->console`, `proto->sdk`, console-core, app and wasm cases, and `napi-glue-dts->kernel-test`), each with `unexpected` `:typecheck` rows; the project cases and the Rust-only task cases `PASS`.

- [ ] **Step 3: Add the new case and update the napi-glue comment.** Replace

```bash
  run_task_case_ci "napi-glue-dts->kernel-test" "rs/crates/bindings/paigasus-node-bindings/index.d.ts" \
    "paigasus-kernel-ts:test"
```

with:

```bash
  # SMA-536: index.d.ts is also an input of every `tsc` task whose package closure holds the napi
  # binding — the kernel's build and typecheck, console-core's build and typecheck, and each app's
  # typecheck — because the preflight holds the installed copy that `tsc` reads equal to this
  # file. Outside the kernel these rows are an accepted over-approximation (spec §3.4). The case
  # keeps its label, so existing references still find it.
  run_task_case_ci "napi-glue-dts->kernel-test" "rs/crates/bindings/paigasus-node-bindings/index.d.ts" \
    "paigasus-kernel-ts:test"
  # SMA-536 — the wasm TYPINGS. `tsc` reads the installed copy of paigasus_wasm.d.ts, and the
  # preflight (ts/scripts/check-installed-bindings.mjs) holds that copy equal to this committed
  # file, so this file is a real input of every `tsc` task whose package closure holds the wasm
  # binding. This case is the behavioural control on those input lines: it must select the
  # `typecheck` of the kernel, console-core and both apps. Expected set MEASURED with the same
  # no-flag `moon query tasks --affected` traversal `_assert_task_case_impl` uses.
  run_task_case_ci "wasm-typings->typecheck" "rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts" \
    "paigasus-kernel-ts:build,paigasus-kernel-ts:test,paigasus-kernel-ts:typecheck,paigasus-console-core-ts:build,paigasus-console-core-ts:test,paigasus-console-core-ts:test-e2e,paigasus-console-core-ts:typecheck,iam-console-ts:build,iam-console-ts:test,iam-console-ts:test-e2e,iam-console-ts:typecheck,gateway-console-ts:build,gateway-console-ts:test,gateway-console-ts:test-e2e,gateway-console-ts:typecheck"
```

The new CSV is a prediction from the declared inputs. Step 5 measures it. If the measurement differs, Step 5 tells you what to do.

- [ ] **Step 4: Write the re-baseline helper.** Write `$SCRATCH/rebaseline.py` (not committed). It takes one target per `tasks[project][task]` from the JSON. It never greps `"target"`, which counts scheduled `deps` as selections.

```python
# usage, from the worktree root: python3 "$SCRATCH/rebaseline.py" [--write]
# Re-measures every run_task_case / run_task_case_ci in ci/affected-graph/run.sh with the widened
# name filter. Prints ADDED and REMOVED per case. With --write, appends each case's ADDED targets
# to its CSV. Refuses to write when any case LOST a target.
import json
import re
import subprocess
import sys
from pathlib import Path

NAMES = ("build", "test", "lint", "test-e2e", "typecheck")
path = Path("ci/affected-graph/run.sh")
text = path.read_text()
CASE_RE = re.compile(r'(run_task_case(_ci)?\s+"([^"]+)"\s+"([^"]+)"\s*\\\n\s*")([^"]*)(")')
lost = []


def measure(touched, deep):
    flags = ["--downstream", "deep"] if deep else []
    out = subprocess.run(
        ["moon", "query", "tasks", "--affected", *flags],
        input=touched + "\n", capture_output=True, text=True, check=True,
    ).stdout
    tasks = json.loads(out).get("tasks") or {}
    return sorted(f"{pid}:{name}" for pid, names in tasks.items() for name in names if name in NAMES)


def rewrite(m):
    ci, label, touched, csv = m.group(2), m.group(3), m.group(4), m.group(5)
    got = measure(touched, deep=not ci)
    want = csv.split(",")
    added = [t for t in got if t not in want]
    removed = [t for t in want if t not in got]
    line = text.count("\n", 0, m.start()) + 1
    print(f"{line}\t{label}\tADDED={','.join(added) or '-'}\tREMOVED={','.join(removed) or '-'}")
    if removed:
        lost.append(label)
    return m.group(1) + ",".join(want + added) + m.group(6)


new_text = CASE_RE.sub(rewrite, text)
if lost:
    sys.exit(f"rebaseline: these cases LOST a target: {', '.join(lost)}. Stop. Nothing was written.")
if "--write" in sys.argv[1:]:
    path.write_text(new_text)
    print("rebaseline: run.sh rewritten")
```

- [ ] **Step 5: Measure, check, then write.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma536}"
python3 "$SCRATCH/rebaseline.py" > "$SCRATCH/rebaseline.txt"; echo "rc=$?"; cat "$SCRATCH/rebaseline.txt"
```

Expected: `rc=0` and 32 lines, one per task case, every `REMOVED=-`. Check every `ADDED` target against these rules before you write:
- A `:typecheck` target is allowed in any case.
- A `:build` target is allowed only where Tasks 5-7 added the input: `paigasus-kernel-ts:build` and `paigasus-console-core-ts:build` in `napi-glue-dts->kernel-test`; `paigasus-app-shell-ts:build` in `proto->sdk` and `proto-iam->sdk`.
- `wasm-typings->typecheck` must show `ADDED=-` (the prediction was exact).
- Any other `ADDED` target, or any `REMOVED` target, is a stop: report it to the coordinator with the line from `rebaseline.txt`. Do not write.

Then write and record:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma536}"
python3 "$SCRATCH/rebaseline.py" --write
python3 "$SCRATCH/rebaseline.py" | grep -v 'ADDED=-	REMOVED=-'; echo "grep rc=$? (1 = every case exact)"
git diff --stat ci/affected-graph/run.sh
```

Expected: `rebaseline: run.sh rewritten`; `grep rc=1`. Paste `rebaseline.txt` (from the first run) into E3.

- [ ] **Step 6: Update the stale comments in `run.sh`.** Replace lines 77-83:

```bash
#   Scoped to build/test/lint — the three tasks that carry `^:build` (lint joined them in
#   SMA-526) — plus `test-e2e` (SMA-506), the one non-`^:build` task name this filter admits.
#   `test-e2e` is `@paigasus/auth`'s own Docker-backed E2E task; it is included so an
#   `auth->auth-tasks`-style case can prove the task is reachable from a source edit at all —
#   nothing else in this file did before SMA-506, since the task name is new. fmt and
#   build-release stay excluded because they carry no `^:build`: fmt is crate-local by
#   construction, and build-release does not run in CI at all.
```

with:

```bash
#   Scoped to build/test/lint — the three tasks that carry `^:build` (lint joined them in
#   SMA-526) — plus two non-`^:build` task names: `test-e2e` (SMA-506) and `typecheck`
#   (SMA-536). `test-e2e` is `@paigasus/auth`'s own Docker-backed E2E task; it is included so an
#   `auth->auth-tasks`-style case can prove the task is reachable from a source edit at all —
#   nothing else in this file did before SMA-506, since the task name is new. `typecheck` is
#   included so that the hand-written upstream inputs of every ts `tsc` task have a behavioural
#   control here, beside the declaration check A12 in cargo_moon_parity.py. The filter also
#   admits `py:typecheck` (.moon/tasks/python.yml:31); no case touches `py/` today, so no case
#   lists it. fmt and build-release stay excluded because they carry no `^:build`: fmt is
#   crate-local by construction, and build-release does not run in CI at all.
```

Replace

```bash
  # no-flag `moon query tasks --affected` traversal: a throwaway kernel edit selects exactly one
  # gateway-console-ts task, test-e2e (not build/test/lint/typecheck).
```

with:

```bash
  # no-flag `moon query tasks --affected` traversal: a throwaway kernel edit selects exactly one
  # gateway-console-ts task, test-e2e. Re-measured by SMA-536 with `typecheck` in the name filter:
  # no `typecheck` is selected, because no ts `tsc` task keys on the Rust kernel's sources.
```

Replace

```bash
  # LIMITATION: the console's `typecheck` task carries this same input, but
  # `_assert_task_case_impl` filters `moon query tasks` to build/test/lint by name, so
  # `typecheck` is structurally invisible here — this case does NOT cover it.
```

with:

```bash
  # SMA-536: `typecheck` is in the name filter now, so this case also asserts the `typecheck` of
  # every task that carries this input (the package's own, app-shell's and both consoles').
```

Replace

```bash
  # CORRECTED, final review: this case controls ONLY the `tests/**/*` line in
  # ts/packages/paigasus-sdk/moon.yml's `build` task, not the `tests/**/*` lines (plural) that
  # moon.yml's own comment once claimed. `_assert_task_case_impl` keeps only build/test/lint/
  # test-e2e from `moon query tasks --affected`, so `typecheck` is invisible to it — the `build`
  # task is the one that actually runs the expectTypeOf proof in CI. The `typecheck` task's own
  # `tests/**/*` line, and both tasks' `vitest.config.ts` line, have NO affected-graph control at
  # all: nothing here proves an edit to those three lines is load-bearing.
```

with:

```bash
  # CORRECTED, final review, and again by SMA-536: `typecheck` joined the name filter, so this case
  # now controls the `tests/**/*` line of BOTH the `build` and the `typecheck` task in
  # ts/packages/paigasus-sdk/moon.yml. Both tasks' `vitest.config.ts` line still has NO
  # affected-graph control: nothing here proves an edit to that line is load-bearing.
```

Replace

```bash
  # also returns iam-console-ts:typecheck, repo:next-env-drift, repo:actionlint, repo:input-liveness,
  # repo:next-public-free and ts:fmt; the harness's name filter drops them, so this case does NOT
  # cover those — the same limitation the ui->console case records.
```

with:

```bash
  # also returns repo:next-env-drift, repo:actionlint, repo:input-liveness, repo:next-public-free
  # and ts:fmt; the harness's name filter drops them, so this case does NOT cover those.
  # iam-console-ts:typecheck passes the filter since SMA-536, so this case asserts it.
```

Replace

```bash
  # gateway-console-ts:typecheck, ts:fmt and three repo:* gates; the harness's name filter
  # (build/test/lint/test-e2e) drops them, so this case does NOT cover `typecheck`, the same
  # limitation the ui->console case records.
```

with:

```bash
  # ts:fmt and three repo:* gates; the harness's name filter drops them, so this case does NOT
  # cover those. gateway-console-ts:typecheck passes the filter since SMA-536, so this case
  # asserts it.
```

- [ ] **Step 7: Update the stale sdk comments.** In `ts/packages/paigasus-sdk/moon.yml`, replace

```yaml
      # `expectTypeOf` proof in tests/iam-factory.test.ts is never checked in CI. The control is
      # `run_task_case_ci "sdk-tests->sdk"` in ci/affected-graph/run.sh — for THIS `tests/**/*`
      # line only (CORRECTED, final review): `_assert_task_case_impl` keeps only build/test/lint/
      # test-e2e, so it cannot see `typecheck` at all, and it asserts nothing about the
      # `vitest.config.ts` line either.
```

with:

```yaml
      # `expectTypeOf` proof in tests/iam-factory.test.ts is never checked in CI. The control is
      # `run_task_case_ci "sdk-tests->sdk"` in ci/affected-graph/run.sh, for the `tests/**/*` line
      # of this task and of `typecheck` (SMA-536 added `typecheck` to that harness's name filter).
      # Nothing there asserts the `vitest.config.ts` line.
```

and replace

```yaml
      # Mirrors `build` above (SMA-575 spec § 3.3), for local `moon run …:typecheck` runs. It
      # carries NO affected-graph control: `_assert_task_case_impl` cannot see a `typecheck` task
      # at all (it keeps only build/test/lint/test-e2e), so nothing in ci/affected-graph/run.sh
      # would catch this line, or the vitest.config.ts line below it, being dropped.
```

with:

```yaml
      # Mirrors `build` above (SMA-575 spec § 3.3). Since SMA-536 the `sdk-tests->sdk` case in
      # ci/affected-graph/run.sh controls this `tests/**/*` line too. Nothing there asserts the
      # `vitest.config.ts` line below it.
```

- [ ] **Step 8: Update the README filter example.** In `ci/affected-graph/README.md` line 455, replace `widening the task-name filter itself (e.g. `lint` joining `build`/`test` in SMA-526)` with `widening the task-name filter itself (e.g. `lint` joining `build`/`test` in SMA-526, or `typecheck` joining in SMA-536)`.

- [ ] **Step 9: Run the suite, the negative control and the gate through Moon.**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma536}"
/bin/bash ci/affected-graph/run.sh; echo "rc=$?"
/bin/bash ci/affected-graph/run.sh --negative-control; echo "neg rc=$?"
PATH="$SCRATCH/bashshim:$PATH" moon run repo:affected-smoke --force; echo "smoke rc=$?"
```

Expected: `== affected-graph cascade intact ==` and `rc=0`; `negative-control OK: harness reported red on all wrong expectations` and `neg rc=0`; `smoke rc=0`. A `JSONDecodeError: Extra data` rc 2 is the known `moon query` flake (SMA-695): re-run once with `--force`.

- [ ] **Step 10: Commit.**

```bash
git add ci/affected-graph/run.sh ci/affected-graph/README.md ts/packages/paigasus-sdk/moon.yml docs/superpowers/plans/2026-10-04-sma-536-ts-typecheck-upstream-inputs.md
git commit -m "feat(ci): add typecheck to the affected-graph task filter and re-baseline every case" \
  -m "Every task case is re-measured from moon query tasks --affected, one target per task. The new wasm-typings->typecheck case is the behavioural control on the typings inputs. The comments that called typecheck invisible are updated." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 9: Red-first proof after the change (spec §4.1 steps 3-5) and the mutation battery (spec §4.3)

**Files:**
- Modify: this plan, execution-log entry E4
- Test: each mutation reds `repo:affected-smoke`; the stale-typings state reds at the preflight

**Interfaces:**
- Consumes: the committed state after Task 8. `git status --short` must be empty before Step 1 and after every mutation, because each mutation is restored with `git checkout -- <file>`, which restores the committed version.
- Produces: E4; scratch helper `$SCRATCH/mutate.py`.

- [ ] **Step 1: Stale wasm typings now fail at the preflight (spec §4.1 step 3).**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma536}"; mkdir -p "$SCRATCH"
git status --short
INST="ts/node_modules/.pnpm/@paigasus+wasm@file+..+rs+crates+bindings+paigasus-wasm/node_modules/@paigasus/wasm/paigasus_wasm.d.ts"
sed 's/^export function sum(a: number, b: number): number;$/export function sum(a: string, b: number): number;/' "$INST" > "$SCRATCH/stale-wasm.d.ts"
rm "$INST" && cp "$SCRATCH/stale-wasm.d.ts" "$INST"
moon run paigasus-kernel-ts:typecheck --force > "$SCRATCH/after-wasm.log" 2>&1; echo "rc=$?"
grep -E 'installed-bindings|Repair:|error TS' "$SCRATCH/after-wasm.log"
```

Expected: `rc=1`. The log holds `@paigasus/wasm: the installed paigasus_wasm.d.ts differs from the committed rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts` and `Repair: rm -rf ts/node_modules && pnpm -C ts install`, and NO `error TS` line (`tsc` did not run). Record in E4 row 1, beside E1 row 2.

- [ ] **Step 2: Restore and confirm the pass (spec §4.1 step 4).**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
rm -rf ts/node_modules && pnpm -C ts install
moon run paigasus-kernel-ts:typecheck --force; echo "rc=$?"
```

Expected: `rc=0`. Record in E4 row 2.

- [ ] **Step 3: Repeat for the napi `index.d.ts` (spec §4.1 step 5).**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma536}"
INST="ts/node_modules/.pnpm/@paigasus+node-bindings@file+..+rs+crates+bindings+paigasus-node-bindings/node_modules/@paigasus/node-bindings/index.d.ts"
sed 's/^export declare function sum(a: number, b: number): number$/export declare function sum(a: string, b: number): number/' "$INST" > "$SCRATCH/stale-napi.d.ts"
rm "$INST" && cp "$SCRATCH/stale-napi.d.ts" "$INST"
moon run paigasus-kernel-ts:typecheck --force > "$SCRATCH/after-napi.log" 2>&1; echo "rc=$?"
grep -E 'installed-bindings|Repair:|error TS' "$SCRATCH/after-napi.log"
rm -rf ts/node_modules && pnpm -C ts install
moon run paigasus-kernel-ts:typecheck --force; echo "restored rc=$?"
git status --short
```

Expected: `rc=1` with `@paigasus/node-bindings: the installed index.d.ts differs from the committed rs/crates/bindings/paigasus-node-bindings/index.d.ts`, the `Repair:` line and no `error TS` line; `restored rc=0`; no status line. Record in E4 row 3.

- [ ] **Step 4: Write the mutation helper.** Write `$SCRATCH/mutate.py` (not committed):

```python
# usage, from the worktree root:
#   python3 "$SCRATCH/mutate.py" FILE TASK LINE      delete the one line equal to LINE inside task TASK
#   python3 "$SCRATCH/mutate.py" FILE --replace OLD NEW   replace the one occurrence of OLD
import re
import sys
from pathlib import Path

path = Path(sys.argv[1])
text = path.read_text()
if sys.argv[2] == "--replace":
    old, new = sys.argv[3].encode().decode("unicode_escape"), sys.argv[4].encode().decode("unicode_escape")
    if text.count(old) != 1:
        sys.exit(f"mutate: expected exactly one {old!r} in {path}, found {text.count(old)}")
    path.write_text(text.replace(old, new))
    sys.exit(0)
task, line = sys.argv[2], sys.argv[3]
lines = text.splitlines(keepends=True)
starts = [i for i, item in enumerate(lines) if item.rstrip("\n") == f"  {task}:"]
if len(starts) != 1:
    sys.exit(f"mutate: expected exactly one `  {task}:` in {path}, found {len(starts)}")
start = starts[0]
end = next((i for i in range(start + 1, len(lines)) if re.match(r"^  [A-Za-z0-9_-]+:\s*$", lines[i])), len(lines))
hits = [i for i in range(start, end) if lines[i].rstrip("\n") == line]
if len(hits) != 1:
    sys.exit(f"mutate: expected exactly one {line!r} in task {task} of {path}, found {len(hits)}")
del lines[hits[0]]
path.write_text("".join(lines))
```

- [ ] **Step 5: Run the battery.** For each row of the table, run this block with the row's `FILE` on the `FILE=` line and the row's `Mutation` command in place of the M1 command (the block shows M1). The Bash tool's shell is zsh, so the plan never reads `PIPESTATUS`; it writes logs and reads `$?` instead. Each mutation is applied alone. Each must parse and run, so that the red comes from the assertion and not from a load error.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma536}"
test -z "$(git status --short)" || { echo "tree not clean, stop"; exit 1; }
FILE=ts/packages/paigasus-kernel/moon.yml   # the row's `FILE` (M1 shown)
python3 "$SCRATCH/mutate.py" ts/packages/paigasus-kernel/moon.yml typecheck "      - '/rs/crates/bindings/paigasus-node-bindings/index.d.ts'" \
  || { echo "mutation did not apply, stop"; exit 1; }   # the row's `Mutation` command (M1 shown)
git diff --stat
python3 ci/affected-graph/cargo_moon_parity.py > "$SCRATCH/mut-parity.log" 2>&1; echo "parity rc=$?"; tail -6 "$SCRATCH/mut-parity.log"
PATH="$SCRATCH/bashshim:$PATH" moon run repo:affected-smoke --force > "$SCRATCH/mut.log" 2>&1; echo "smoke rc=$?"
grep -E 'inputs omit|deps omit|FLOOR:|does not run|after `tsc`|collect_findings reported|FAIL  \[' "$SCRATCH/mut.log" | head -8
git checkout -- "$FILE" && git status --short
```

| # | Mutation (`MUTATE`) | `FILE` | Expected |
|---|---|---|---|
| M1 | `python3 "$SCRATCH/mutate.py" ts/packages/paigasus-kernel/moon.yml typecheck "      - '/rs/crates/bindings/paigasus-node-bindings/index.d.ts'"` | `ts/packages/paigasus-kernel/moon.yml` | smoke rc=1; `paigasus-kernel-ts:typecheck inputs omit rs/crates/bindings/paigasus-node-bindings/index.d.ts` |
| M2 | same, line `"      - '/rs/crates/bindings/paigasus-node-bindings/package.json'"` | same | rc=1; `… inputs omit rs/crates/bindings/paigasus-node-bindings/package.json` |
| M3 | same, line `"      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts'"` | same | rc=1; `… inputs omit rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts` |
| M4 | same, line `"      - '/rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts'"` | same | rc=1; `… inputs omit rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts` |
| M5 | same, line `"      - '/rs/crates/bindings/paigasus-wasm/package.json'"` | same | rc=1; `… inputs omit rs/crates/bindings/paigasus-wasm/package.json` |
| M6 | same, line `"      - '/ts/scripts/check-installed-bindings.mjs'"` | same | rc=1; `… inputs omit ts/scripts/check-installed-bindings.mjs` |
| M7 | same, line `"      - 'tests/**/*'"` | same | **rc=0 expected.** Own-package files are not asserted (spec N6). Record it as the known limit. |
| M8 | same, line `"      - 'vitest.config.ts'"` | same | **rc=0 expected**, same reason as M7. |
| M9 | `python3 "$SCRATCH/mutate.py" ts/packages/paigasus-discovery/moon.yml typecheck "      - '/ts/packages/paigasus-proto/package.json'"` (no YAML alias with `build`) | `ts/packages/paigasus-discovery/moon.yml` | rc=1; `paigasus-discovery-ts:typecheck inputs omit ts/packages/paigasus-proto/package.json`, and no `paigasus-discovery-ts:build` row |
| M10 | `python3 "$SCRATCH/mutate.py" ts/apps/iam-console/moon.yml --replace "script: 'node ../../../ts/scripts/check-installed-bindings.mjs && pnpm exec tsc -p tsconfig.json --noEmit'" "script: 'pnpm exec tsc -p tsconfig.json --noEmit'"` | `ts/apps/iam-console/moon.yml` | rc=1; `iam-console-ts:typecheck does not run ts/scripts/check-installed-bindings.mjs before `tsc`` |
| M11 | same file, `--replace` the same old text with `"script: 'pnpm exec tsc -p tsconfig.json --noEmit && node ../../../ts/scripts/check-installed-bindings.mjs'"` | same | rc=1; `iam-console-ts:typecheck runs … after `tsc`` |
| M12 | `python3 "$SCRATCH/mutate.py" ts/packages/paigasus-console-core/package.json --replace '    "@paigasus/kernel": "workspace:*",\n' ''` | `ts/packages/paigasus-console-core/package.json` | rc=1; `FLOOR: the closure of @paigasus/console-core no longer derives @paigasus/kernel` |
| M13 | `python3 "$SCRATCH/mutate.py" rs/crates/bindings/paigasus-wasm/package.json --replace '"paigasus_wasm_bg.wasm.d.ts"\n' '"paigasus_wasm_bg.wasm.d.ts",\n    "paigasus_wasm_extra.d.ts"\n'` | `rs/crates/bindings/paigasus-wasm/package.json` | rc=1; `… inputs omit rs/crates/bindings/paigasus-wasm/paigasus_wasm_extra.d.ts` for the kernel, console-core and app `tsc` tasks |
| M14 | `python3 "$SCRATCH/mutate.py" .moon/tasks/typescript-project.yml typecheck "      - '/ts/tsconfig.base.json'"` | `.moon/tasks/typescript-project.yml` | rc=1; `<pid>:typecheck inputs omit ts/tsconfig.base.json` for every ts `typecheck` |
| M15 | `python3 "$SCRATCH/mutate.py" ts/packages/paigasus-sdk/moon.yml typecheck "    deps: ['contracts:generate']"` | `ts/packages/paigasus-sdk/moon.yml` | rc=1; `paigasus-sdk-ts:typecheck deps omit contracts:generate` |
| M16 | `python3 "$SCRATCH/mutate.py" ci/affected-graph/cargo_moon_parity.py --replace '"a11", "a12a", "a12b")' '"a11", "a12a")'` | `ci/affected-graph/cargo_moon_parity.py` | rc=1; `collect_findings reported (…, 'a12a', 'a12b'), expected (…, 'a12a')` from the negative control |
| M17 | same file, `--replace '"a11", "a12a", "a12b")' '"a11", "a12b")'` | same | rc=1; the same `collect_findings reported` row |
| M18 | `python3 "$SCRATCH/mutate.py" ci/affected-graph/run.sh --replace 'if name in ("build", "test", "lint", "test-e2e", "typecheck"):' 'if name in ("build", "test", "lint", "test-e2e"):'` | `ci/affected-graph/run.sh` | rc=1; `FAIL  [<label>] affected TASK set != expected set` with `missing` `:typecheck` rows |

For M16 to M18 the `parity rc` line is not the evidence: M16/M17 red only the `--self-test` (the negative control), and M18 changes only `run.sh`. The `smoke rc=1` line and its `grep` output are the evidence. Record one E4 row per mutation: the number, `parity rc`, `smoke rc`, and the first matching line. If a mutation that must red stays green, stop: fix the gate, then re-run the WHOLE battery (spec §4.3).

- [ ] **Step 6: One mutation through `moon ci --base origin/main` (spec §4.3).** This proves that the pull request that makes such a change selects `repo:affected-smoke`. `moon ci` reads committed changes, so the mutation goes on a scratch branch that is never pushed.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma536}"
test -z "$(git status --short)" || { echo "tree not clean, stop"; exit 1; }
git fetch origin main
git switch -c feature/sma-536-ts-typecheck-upstream-inputs-scratch
python3 "$SCRATCH/mutate.py" ts/apps/iam-console/moon.yml --replace "script: 'node ../../../ts/scripts/check-installed-bindings.mjs && pnpm exec tsc -p tsconfig.json --noEmit'" "script: 'pnpm exec tsc -p tsconfig.json --noEmit'"
git commit -am "test(ts): scratch mutation that removes the iam-console preflight" -m "Never pushed. SMA-536 mutation battery." -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
PATH="$SCRATCH/bashshim:$PATH" moon ci :affected-smoke --base origin/main > "$SCRATCH/mut-ci.log" 2>&1; echo "moon ci rc=$?"
grep -E 'repo:affected-smoke|does not run' "$SCRATCH/mut-ci.log" | head -5
git switch feature/sma-536-ts-typecheck-upstream-inputs
git branch -D feature/sma-536-ts-typecheck-upstream-inputs-scratch
git status --short && git log --oneline -1
```

Expected: `moon ci rc=1`; the log names `repo:affected-smoke` as run and failed, and holds `iam-console-ts:typecheck does not run ts/scripts/check-installed-bindings.mjs before `tsc``. After the switch back: no status line, and the last commit is Task 8's. Record in E4.

- [ ] **Step 7: Commit the record.**

```bash
git add docs/superpowers/plans/2026-10-04-sma-536-ts-typecheck-upstream-inputs.md
git commit -m "docs(ci): record the SMA-536 red-first proof and mutation battery" \
  -m "Stale installed typings now fail at the preflight with the repair command, before tsc runs. Each mutation in the battery reds repo:affected-smoke, except the two own-package kernel lines that A12 does not assert by design." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Task 10: Forced runs, the full gate graph, and the follow-up issue (spec §4.4, §7)

**Files:**
- Modify: this plan, execution-log entry E5
- Test: forced runs and the full `moon ci` graph

**Interfaces:**
- Consumes: every earlier task.
- Produces: E5; a follow-up request to the coordinator.

- [ ] **Step 1: Force-run every task whose script or inputs changed (spec §4.4).**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
moon run paigasus-kernel-ts:build paigasus-kernel-ts:typecheck paigasus-kernel-ts:test \
  paigasus-console-core-ts:build paigasus-console-core-ts:typecheck \
  paigasus-sdk-ts:build paigasus-sdk-ts:typecheck paigasus-discovery-ts:build paigasus-discovery-ts:typecheck \
  paigasus-app-shell-ts:build paigasus-app-shell-ts:typecheck \
  iam-console-ts:typecheck iam-console-ts:build gateway-console-ts:typecheck gateway-console-ts:build \
  ts:test ts:lint ts:fmt --force; echo "rc=$?"
git status --short
```

Expected: `rc=0`. Both app `build` tasks pass the SMA-634 wasm-chunk assertion. `git status` shows no change (restore a BSR-deleted generated file per the constraint). Record in E5.

- [ ] **Step 2: Run the full gate graph from the root `CLAUDE.md`.** Use the bash shim, so `repo:affected-smoke` runs under bash 3.2.

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH" PROTO_REPORTER=text
SCRATCH="${SCRATCH:-${TMPDIR:-/tmp}/sma536}"
git fetch origin main
PATH="$SCRATCH/bashshim:$PATH" moon ci :build :test :lint :fmt :deny :osv :machete :actionlint :typecheck :breaking \
  :affected-smoke :parity-corpus-drift :next-env-drift :wasm-getrandom-free \
  :redis-connect-single-site :iam-docker-policy-single-site :error-code-single-site \
  :http-extractor-envelope :input-liveness :promtool :observability-drift \
  :nats-permissions :release-parity :release-parity-py :release-parity-ts \
  :publish-metadata :version-lockstep :workflow-credentials :pyo3-stub-drift :ruff-ci \
  :next-public-free :helm-render :moon-diagnosis-exec :wasm-lockstep :test-e2e \
  --base origin/main --include-relations > "$SCRATCH/full.log" 2>&1; echo "rc=$?"
grep -E 'failed|Failed|✖' "$SCRATCH/full.log" | head -20
```

Expected: `rc=0`. If a gate that needs bash 4+ reds under the shim (`repo:ruff-ci`, `repo:next-public-free`, `repo:publish-metadata`), re-run it directly with `/opt/homebrew/bin/bash ci/<gate>/run.sh` and record that verdict instead. If `repo:actionlint` exits rc 2 with a `pipe capacity … small` message, that is the host condition (SMA-612), not a finding; CI is its verdict. For any other red, follow the root `CLAUDE.md` "Diagnosing an unattributed `moon ci` failure" procedure, Step 0 first. Record the result and every directly re-run gate in E5.

- [ ] **Step 3: Ask the coordinator for the follow-up issue (spec §7).** Do not create it. Put this text in the final report:

> Follow-up for the coordinator to create in Linear (SMA-536 N1): "ts vitest `test` tasks: assert the binding inputs that the `vitest.config.ts` aliases read". vitest resolves `@paigasus/node-bindings` and `@paigasus/wasm` through aliases in each `vitest.config.ts`, so the A12 rule does not apply to `test` tasks. The issue designs the required-input rule for them.

- [ ] **Step 4: Commit the record.**

```bash
git add docs/superpowers/plans/2026-10-04-sma-536-ts-typecheck-upstream-inputs.md
git commit -m "docs(ci): record the SMA-536 forced runs and the full gate graph" \
  -m "Every changed tsc task, both app builds and the full moon ci target list pass on the branch." \
  -m "Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

## Execution log

Fill each entry in the task that names it. Paste command output where the step says so.

### E1 — before the change (Task 1)

| Row | Measurement | Result |
|---|---|---|
| 1 | `--traceResolution`, kernel and console-core | Kernel resolves `@paigasus/node-bindings` to `ts/node_modules/.pnpm/@paigasus+node-bindings@file+..+rs+crates+bindings+paigasus-node-bindings/node_modules/@paigasus/node-bindings/index.d.ts` and `@paigasus/wasm` to `ts/node_modules/.pnpm/@paigasus+wasm@file+..+rs+crates+bindings+paigasus-wasm/node_modules/@paigasus/wasm/paigasus_wasm.d.ts`. Console-core resolves only `@paigasus/wasm`, to the same wasm path. |
| 2 | stale installed `paigasus_wasm.d.ts` → `moon run paigasus-kernel-ts:typecheck --force` | Inodes differ (166576231 and 168411783), `cmp rc=1`, no `git status` line. Moon rc=2 (the plan expected 1; `pnpm` exited 2). `src/binding-parity.types.ts(19,7): error TS2322: Type 'true' is not assignable to type 'never'.` and `tests/sum.wasm.test.ts(14,16): error TS2345: Argument of type 'number' is not assignable to parameter of type 'string'.` No line names the installed file or a repair command. After the run, `still stale: cmp rc=1`: Moon did not repair the copy. |
| 3 | restored → same command | `rm -rf ts/node_modules && pnpm -C ts install` rc=0, then `typecheck --force` rc=0. |
| 4 | stale installed napi `index.d.ts` → same command; restored | `cmp rc=1`, no `git status` line. Moon rc=2. `src/binding-parity.types.ts(19,7): error TS2322: Type 'true' is not assignable to type 'never'.` and `tests/sum.test.ts(14,16): error TS2345: Argument of type 'number' is not assignable to parameter of type 'string'.` After restore: `restored rc=0`. |

### E2 — A12 on the real tree before the moon.yml fixes (Task 4)

```text
a12a	gateway-console-ts:typecheck inputs omit rs/crates/bindings/paigasus-node-bindings/index.d.ts
a12a	gateway-console-ts:typecheck inputs omit rs/crates/bindings/paigasus-node-bindings/package.json
a12a	gateway-console-ts:typecheck inputs omit ts/scripts/check-installed-bindings.mjs
a12a	iam-console-ts:typecheck inputs omit rs/crates/bindings/paigasus-node-bindings/index.d.ts
a12a	iam-console-ts:typecheck inputs omit rs/crates/bindings/paigasus-node-bindings/package.json
a12a	iam-console-ts:typecheck inputs omit ts/scripts/check-installed-bindings.mjs
a12a	paigasus-app-shell-ts:build inputs omit ts/packages/paigasus-next-config/package.json
a12a	paigasus-app-shell-ts:build inputs omit ts/packages/paigasus-next-config/src/**/*
a12a	paigasus-app-shell-ts:build inputs omit ts/packages/paigasus-next-config/tsconfig.app.json
a12a	paigasus-app-shell-ts:build inputs omit ts/packages/paigasus-proto/package.json
a12a	paigasus-app-shell-ts:build inputs omit ts/packages/paigasus-proto/src/**/*
a12a	paigasus-app-shell-ts:build deps omit contracts:generate — it reads @paigasus/proto's generated tree, so it must run after the generator for a deterministic cache key
a12a	paigasus-app-shell-ts:typecheck inputs omit ts/packages/paigasus-next-config/package.json
a12a	paigasus-app-shell-ts:typecheck inputs omit ts/packages/paigasus-next-config/src/**/*
a12a	paigasus-app-shell-ts:typecheck inputs omit ts/packages/paigasus-next-config/tsconfig.app.json
a12a	paigasus-app-shell-ts:typecheck inputs omit ts/packages/paigasus-proto/package.json
a12a	paigasus-app-shell-ts:typecheck inputs omit ts/packages/paigasus-proto/src/**/*
a12a	paigasus-app-shell-ts:typecheck deps omit contracts:generate — it reads @paigasus/proto's generated tree, so it must run after the generator for a deterministic cache key
a12a	paigasus-console-core-ts:build inputs omit rs/crates/bindings/paigasus-node-bindings/index.d.ts
a12a	paigasus-console-core-ts:build inputs omit rs/crates/bindings/paigasus-node-bindings/package.json
a12a	paigasus-console-core-ts:build inputs omit ts/scripts/check-installed-bindings.mjs
a12a	paigasus-console-core-ts:typecheck inputs omit rs/crates/bindings/paigasus-node-bindings/index.d.ts
a12a	paigasus-console-core-ts:typecheck inputs omit rs/crates/bindings/paigasus-node-bindings/package.json
a12a	paigasus-console-core-ts:typecheck inputs omit ts/scripts/check-installed-bindings.mjs
a12a	paigasus-discovery-ts:build inputs omit ts/packages/paigasus-proto/package.json
a12a	paigasus-discovery-ts:typecheck inputs omit ts/packages/paigasus-proto/package.json
a12a	paigasus-kernel-ts:build inputs omit rs/crates/bindings/paigasus-node-bindings/index.d.ts
a12a	paigasus-kernel-ts:build inputs omit rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts
a12a	paigasus-kernel-ts:build inputs omit rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts
a12a	paigasus-kernel-ts:build inputs omit ts/scripts/check-installed-bindings.mjs
a12a	paigasus-kernel-ts:typecheck inputs omit rs/crates/bindings/paigasus-node-bindings/index.d.ts
a12a	paigasus-kernel-ts:typecheck inputs omit rs/crates/bindings/paigasus-node-bindings/package.json
a12a	paigasus-kernel-ts:typecheck inputs omit rs/crates/bindings/paigasus-wasm/package.json
a12a	paigasus-kernel-ts:typecheck inputs omit rs/crates/bindings/paigasus-wasm/paigasus_wasm.d.ts
a12a	paigasus-kernel-ts:typecheck inputs omit rs/crates/bindings/paigasus-wasm/paigasus_wasm_bg.wasm.d.ts
a12a	paigasus-kernel-ts:typecheck inputs omit ts/scripts/check-installed-bindings.mjs
a12a	paigasus-sdk-ts:build inputs omit ts/packages/paigasus-proto/package.json
a12a	paigasus-sdk-ts:typecheck inputs omit ts/packages/paigasus-proto/package.json
a12b	gateway-console-ts:typecheck does not run ts/scripts/check-installed-bindings.mjs before `tsc`, so `tsc` can read stale installed typings
a12b	iam-console-ts:typecheck does not run ts/scripts/check-installed-bindings.mjs before `tsc`, so `tsc` can read stale installed typings
a12b	paigasus-console-core-ts:build does not run ts/scripts/check-installed-bindings.mjs before `tsc`, so `tsc` can read stale installed typings
a12b	paigasus-console-core-ts:typecheck does not run ts/scripts/check-installed-bindings.mjs before `tsc`, so `tsc` can read stale installed typings
a12b	paigasus-kernel-ts:build does not run ts/scripts/check-installed-bindings.mjs before `tsc`, so `tsc` can read stale installed typings
a12b	paigasus-kernel-ts:typecheck does not run ts/scripts/check-installed-bindings.mjs before `tsc`, so `tsc` can read stale installed typings
```

Measured on 2026-10-04: 38 `a12a` rows and 6 `a12b` rows, 44 lines. No `FLOOR:` row and no package.json row. The list matches the prediction.

### E3 — run.sh re-measurement (Task 8)

```text
367	proto->svc-info-deep	ADDED=-	REMOVED=-
373	proto->svc-info-ci	ADDED=-	REMOVED=-
404	lockfile->all-lint	ADDED=-	REMOVED=-
410	lockfile->all-lint-ci	ADDED=-	REMOVED=-
419	kernel->consumer-tasks	ADDED=-	REMOVED=-
436	ui->console	ADDED=gateway-console-ts:typecheck,iam-console-ts:typecheck,paigasus-app-shell-ts:typecheck,paigasus-ui-ts:typecheck	REMOVED=-
457	ui-components->console	ADDED=gateway-console-ts:typecheck,iam-console-ts:typecheck,paigasus-app-shell-ts:typecheck,paigasus-ui-ts:typecheck	REMOVED=-
475	auth->auth-tasks	ADDED=gateway-console-ts:typecheck,iam-console-ts:typecheck,paigasus-app-shell-ts:typecheck,paigasus-auth-ts:typecheck,paigasus-console-core-ts:typecheck	REMOVED=-
496	proto->sdk	ADDED=gateway-console-ts:typecheck,iam-console-ts:typecheck,paigasus-app-shell-ts:build,paigasus-app-shell-ts:typecheck,paigasus-console-core-ts:typecheck,paigasus-discovery-ts:typecheck,paigasus-proto-ts:typecheck,paigasus-sdk-ts:typecheck	REMOVED=-
524	proto-iam->sdk	ADDED=gateway-console-ts:typecheck,iam-console-ts:typecheck,paigasus-app-shell-ts:build,paigasus-app-shell-ts:typecheck,paigasus-console-core-ts:typecheck,paigasus-discovery-ts:typecheck,paigasus-proto-ts:typecheck,paigasus-sdk-ts:typecheck	REMOVED=-
544	discovery->discovery-tasks	ADDED=gateway-console-ts:typecheck,iam-console-ts:typecheck,paigasus-app-shell-ts:typecheck,paigasus-console-core-ts:typecheck,paigasus-discovery-ts:typecheck	REMOVED=-
565	discovery-adapters->discovery-tasks	ADDED=gateway-console-ts:typecheck,iam-console-ts:typecheck,paigasus-app-shell-ts:typecheck,paigasus-console-core-ts:typecheck,paigasus-discovery-ts:typecheck	REMOVED=-
578	app-shell->app-shell-tasks	ADDED=gateway-console-ts:typecheck,iam-console-ts:typecheck,paigasus-app-shell-ts:typecheck	REMOVED=-
590	app-shell-shell->app-shell-tasks	ADDED=gateway-console-ts:typecheck,iam-console-ts:typecheck,paigasus-app-shell-ts:typecheck	REMOVED=-
610	sdk->iam-console	ADDED=gateway-console-ts:typecheck,iam-console-ts:typecheck,paigasus-console-core-ts:typecheck,paigasus-sdk-ts:typecheck	REMOVED=-
612	sdk-errors->iam-console	ADDED=gateway-console-ts:typecheck,iam-console-ts:typecheck,paigasus-console-core-ts:typecheck,paigasus-sdk-ts:typecheck	REMOVED=-
622	sdk-tests->sdk	ADDED=paigasus-sdk-ts:typecheck	REMOVED=-
633	app-shell->console	ADDED=gateway-console-ts:typecheck,iam-console-ts:typecheck,paigasus-app-shell-ts:typecheck	REMOVED=-
653	iam-console-lib->iam-console-tasks	ADDED=iam-console-ts:typecheck	REMOVED=-
655	iam-console-proxy->iam-console-tasks	ADDED=iam-console-ts:typecheck	REMOVED=-
667	iam-console-app->two-zone-tier	ADDED=iam-console-ts:typecheck	REMOVED=-
685	gateway-console-lib->gateway-console-tasks	ADDED=gateway-console-ts:typecheck	REMOVED=-
690	gateway-console-proxy->gateway-console-tasks	ADDED=gateway-console-ts:typecheck	REMOVED=-
712	console-core->consumers	ADDED=gateway-console-ts:typecheck,iam-console-ts:typecheck,paigasus-console-core-ts:typecheck	REMOVED=-
714	console-core-prn-tenancy->consumers	ADDED=gateway-console-ts:typecheck,iam-console-ts:typecheck,paigasus-console-core-ts:typecheck	REMOVED=-
737	console-core-testing->consumers	ADDED=gateway-console-ts:typecheck,iam-console-ts:typecheck,paigasus-console-core-ts:typecheck	REMOVED=-
754	wasm-artifact->console	ADDED=gateway-console-ts:typecheck,iam-console-ts:typecheck,paigasus-console-core-ts:typecheck	REMOVED=-
762	kernel-wasm-src->console	ADDED=gateway-console-ts:typecheck,iam-console-ts:typecheck,paigasus-console-core-ts:typecheck,paigasus-kernel-ts:typecheck	REMOVED=-
770	napi-glue-js->kernel-test	ADDED=-	REMOVED=-
777	napi-glue-dts->kernel-test	ADDED=gateway-console-ts:typecheck,iam-console-ts:typecheck,paigasus-console-core-ts:build,paigasus-console-core-ts:typecheck,paigasus-kernel-ts:build,paigasus-kernel-ts:typecheck	REMOVED=-
785	wasm-typings->typecheck	ADDED=-	REMOVED=-
800	gateway->sdk	ADDED=-	REMOVED=-
```

Measured on 2026-10-04: 32 task cases, no REMOVED target. `wasm-typings->typecheck` showed ADDED=- and holds the four required `typecheck` targets. `kernel->bindings` is a project case and was not re-measured.

### E4 — after the change (Task 9)

| Row | Measurement | Result |
|---|---|---|
| 1 | stale installed `paigasus_wasm.d.ts` → kernel `typecheck` | |
| 2 | restored | |
| 3 | stale installed napi `index.d.ts` → kernel `typecheck`; restored | |
| M1-M18 | one row per mutation: parity rc, smoke rc, first matching line | |
| CI | M10 through `moon ci :affected-smoke --base origin/main` | |

### E5 — forced runs and the full graph (Task 10)

| Run | Result |
|---|---|
| forced runs (Task 10 Step 1) | |
| full `moon ci` graph | |
| gates re-run directly, with the bash used | |
