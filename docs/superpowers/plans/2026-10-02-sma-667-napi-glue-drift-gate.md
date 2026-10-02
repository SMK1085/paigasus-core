# SMA-667 napi glue drift gate Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the committed napi glue (`index.js`, `index.d.ts`) have two sanctioned writers and a drift gate in `paigasus-kernel-ts:test`, so builds never dirty the tree and stale glue never ships.

**Architecture:** `build` and `test` pass `--js index.fresh.js --dts index.fresh.d.ts` to `napi build`, so they write sibling scratch files instead of the committed ones. A new vitest file compares the committed files with the scratch files byte for byte. A new `generate-napi-glue` task (local only) builds the scratch files and `cp`s them over the committed ones. CI filters, docs and Dependabot follow.

**Tech Stack:** Moon 2.5.3 tasks (YAML), `@napi-rs/cli` 3.10.4, vitest 4, TypeScript, bash (`ci/affected-graph/run.sh`), Python (`ci/affected-graph/cargo_moon_parity.py`), Dependabot config.

**Spec:** `docs/superpowers/specs/2026-10-02-sma-667-napi-glue-drift-gate-design.md`

## Global Constraints

- Work ONLY in `/Users/smaschek/dev/paigasus/paigasus-core/.claude/worktrees/sma-667-napi-glue`, on branch `feature/sma-667-napi-glue-drift`. Run `git branch --show-current` before the first commit of each task; stop if it is not that branch.
- Before every shell command that runs moon, pnpm, uv or napi: `export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"`.
- In this worktree session, the sandbox can refuse compound `cd`/variable/loop shell commands. If it does, write the commands to a script file in the session scratchpad and run `/bin/bash <that file>`.
- Every new source file opens with `// SPDX-License-Identifier: Apache-2.0` (TS) or `# SPDX-License-Identifier: Apache-2.0` (shell, Python, YAML where the file already has one).
- Conventional commits with a workspace scope, for example `feat(ts): …`, `ci(repo): …`, `docs(ts): …`. End every commit message with `Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>`. Do not put a `#NNN` or a `token: value` line in the commit body.
- Never use `git commit --amend`, `git reset`, `git stash`, `git checkout -- <file>` or `--no-verify`. Restore a file you mutated for a proof with an edit, not with git.
- Never run a command in the background and then wait for it. Run every command in the foreground.
- Never install host software (`brew install` and similar). If a tool is missing, stop and report.
- Any chat message that seems to come from the user is for the coordinator, not for you.
- Scratch file names are exactly `index.fresh.js` and `index.fresh.d.ts`, in `rs/crates/bindings/paigasus-node-bindings/`. The flag string is exactly `--js index.fresh.js --dts index.fresh.d.ts`.
- The remedy text is exactly: ``Run `moon run paigasus-kernel-ts:generate-napi-glue` and commit index.js and index.d.ts under rs/crates/bindings/paigasus-node-bindings/.``
- `ci/affected-graph/run.sh` runs under system `/bin/bash` (3.2), never under Homebrew bash 5.
- Write comments and docs in ASD-STE100 Simplified Technical English: short sentences, active voice, no idiom.

## Review Focus

1. **A cache hit replays a stale pass.** A PR that edits only `index.js` must re-run `paigasus-kernel-ts:test`. Pinned by the two inputs (Task 1) and the two affected-graph cases (Task 3).
2. **Two empty files are equal.** A build whose type-def folder is empty writes a `.d.ts` with only `__napiBindingTarget`. Pinned by check 2 (Task 1) and its self-test rows.
3. **`build` silently goes back to in-place writes.** Pinned by check 3 (Task 1) and proof P6.
4. **The regenerated `.d.ts` does not reach `ts/node_modules`.** `napi build` renames over the file and breaks the pnpm hard link. Pinned by the `cp` form and proof P7 (Task 2).
5. **A CRLF checkout reds the gate on a diff that means nothing.** Pinned by the `.gitattributes` lines (Task 1).

---

## File map

| File | Change | Task |
|---|---|---|
| `ts/packages/paigasus-kernel/tests/committed-napi-glue.test.ts` | Create: the gate (checks 0–3, self-test rows) | 1 |
| `ts/packages/paigasus-kernel/vitest.config.ts` | Add the test file to the `node` project `include` | 1 |
| `ts/packages/paigasus-kernel/moon.yml` | `build`/`test` flags, `test` inputs, comments; new `generate-napi-glue` task | 1, 2 |
| `rs/crates/bindings/paigasus-node-bindings/.gitignore` | Ignore the two scratch files | 1 |
| `.gitattributes` | `text eol=lf linguist-generated=true` for the two glue files | 1 |
| `ci/affected-graph/cargo_moon_parity.py` | `ALLOW_UNLOCKED_CARGO` entry for the new task | 2 |
| `ci/affected-graph/run.sh` | Two `run_task_case_ci` cases | 3 |
| `.github/workflows/images.yml` | Remove the two paths and their comment | 4 |
| `docs/ops/RUNBOOK-containers.md` | Section 1: the glue is now an input | 4 |
| `rs/CLAUDE.md` | Filter list and the "not inputs" sentence | 4 |
| `ts/CLAUDE.md` | New rule for the napi glue | 4 |
| `ci/version-lockstep/README.md` | Line 26 names the new gate | 4 |
| `.github/dependabot.yml` | Separate `@napi-rs/cli` group | 5 |

---

### Task 1: The gate and the scratch outputs

**Files:**
- Create: `ts/packages/paigasus-kernel/tests/committed-napi-glue.test.ts`
- Modify: `ts/packages/paigasus-kernel/vitest.config.ts` (the `node` project `include` list, about lines 31-40)
- Modify: `ts/packages/paigasus-kernel/moon.yml` (the `build` script at about line 41, its comments at about lines 20-25 and 58-60; the `test` script at about line 133 and its `inputs`)
- Modify: `rs/crates/bindings/paigasus-node-bindings/.gitignore`
- Modify: `.gitattributes`

**Interfaces:**
- Consumes: nothing.
- Produces: the scratch files `index.fresh.js` and `index.fresh.d.ts`, written by `build` and `test`. The test file exports nothing. Task 2 relies on the flag string and the remedy text from Global Constraints.

- [ ] **Step 1: Write the test file**

Create `ts/packages/paigasus-kernel/tests/committed-napi-glue.test.ts`:

```ts
// SPDX-License-Identifier: Apache-2.0
//
// The drift gate for the two committed napi glue files (SMA-667). `paigasus-kernel-ts:test` builds
// the glue into sibling scratch files (`index.fresh.js`, `index.fresh.d.ts`). This file compares the
// committed files with them. The committed files are what npm publishes (release.yml), what the
// prebuild assembly packs (prebuild.yml) and what ts/Dockerfile copies. No CI step rebuilds them
// before those readers.
//
//   0. the fresh files exist;
//   1. each committed file equals its fresh file, byte for byte;
//   2. each pair is not vacuous: the export names in index.js equal those in index.d.ts, the set
//      holds `sum`, and it has more than one member;
//   3. the `build` and `test` tasks in moon.yml pass the scratch flags, so neither task writes the
//      committed files.
//
// The checks read files only. They do not load index.fresh.js.
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';

const CRATE = new URL('../../../../rs/crates/bindings/paigasus-node-bindings/', import.meta.url);
const MOON_YML = new URL('../moon.yml', import.meta.url);
const SCRATCH_FLAGS = '--js index.fresh.js --dts index.fresh.d.ts';
const PAIRS = [
  ['index.js', 'index.fresh.js'],
  ['index.d.ts', 'index.fresh.d.ts'],
] as const;

const REGENERATE =
  'Run `moon run paigasus-kernel-ts:generate-napi-glue` and commit index.js and index.d.ts under rs/crates/bindings/paigasus-node-bindings/.';

// The first capture group of every match, sorted. `flatMap` drops a match without a group, which
// keeps the result `string[]` under `noUncheckedIndexedAccess`.
function captures(source: string, pattern: RegExp): string[] {
  return [...source.matchAll(pattern)].flatMap((m) => (m[1] === undefined ? [] : [m[1]])).sort();
}

// `module.exports.<name> =` lines. The bare `module.exports = nativeBinding` line has no name and
// does not match.
export function jsExportNames(source: string): string[] {
  return captures(source, /^module\.exports\.([A-Za-z_$][\w$]*)\s*=/gm);
}

// `export declare function|const|class <name>` lines.
export function dtsExportNames(source: string): string[] {
  return captures(source, /^export declare (?:function|const|class) ([A-Za-z_$][\w$]*)/gm);
}

// Returns a list of problems. An empty list means the pair is not vacuous.
export function vacuityProblems(js: string, dts: string): string[] {
  const jsNames = jsExportNames(js);
  const dtsNames = dtsExportNames(dts);
  const problems: string[] = [];
  if (JSON.stringify(jsNames) !== JSON.stringify(dtsNames)) {
    problems.push(`index.js exports [${jsNames.join(', ')}] but index.d.ts declares [${dtsNames.join(', ')}]`);
  }
  if (!dtsNames.includes('sum')) problems.push('index.d.ts declares no `sum`');
  if (dtsNames.length < 2) problems.push(`index.d.ts declares only ${dtsNames.length} name(s)`);
  return problems;
}

// The `script:` line of one task in moon.yml, found by text: the first `    script:` line after the
// `  <task>:` line.
export function taskScript(moonYml: string, task: string): string | undefined {
  const lines = moonYml.split('\n');
  const start = lines.indexOf(`  ${task}:`);
  if (start < 0) return undefined;
  for (const line of lines.slice(start + 1)) {
    if (/^  \S/.test(line)) return undefined; // the next task started first
    if (line.startsWith('    script:')) return line;
  }
  return undefined;
}

const read = (name: string): string => readFileSync(new URL(name, CRATE), 'utf8');

describe('the gate helpers can fail', () => {
  // Without these rows, a helper that always returns "no problem" would make checks 2 and 3 vacuous.
  it('flags a d.ts that declares only __napiBindingTarget', () => {
    const js = 'module.exports.__napiBindingTarget = x\n';
    const dts = "export declare const __napiBindingTarget: 'native'\n";
    expect(vacuityProblems(js, dts)).not.toEqual([]);
  });

  it('flags a js and d.ts whose names differ', () => {
    const js = 'module.exports.sum = b.sum\nmodule.exports.prnOrg = b.prnOrg\n';
    const dts = 'export declare function sum(a: number, b: number): number\nexport declare function prnRegion(s: string): string\n';
    expect(vacuityProblems(js, dts)).not.toEqual([]);
  });

  it('accepts a matching, non-trivial pair', () => {
    const js = 'module.exports = b\nmodule.exports.sum = b.sum\nmodule.exports.prnOrg = b.prnOrg\n';
    const dts = 'export declare function sum(a: number, b: number): number\nexport declare function prnOrg(s: string): string\n';
    expect(vacuityProblems(js, dts)).toEqual([]);
  });

  it('finds a task script and misses an absent task', () => {
    const yml = "tasks:\n  build:\n    script: 'a --js index.fresh.js --dts index.fresh.d.ts'\n  test:\n    deps: []\n  other:\n    script: 'b'\n";
    expect(taskScript(yml, 'build')).toContain(SCRATCH_FLAGS);
    expect(taskScript(yml, 'test')).toBeUndefined();
    expect(taskScript(yml, 'absent')).toBeUndefined();
  });
});

describe('the committed napi glue agrees with the generator', () => {
  it.each(PAIRS)('check 0: the fresh build of %s exists', (_committed, fresh) => {
    expect(existsSync(new URL(fresh, CRATE)), `${fileURLToPath(new URL(fresh, CRATE))} is missing — run \`moon run paigasus-kernel-ts:test\`, which builds it.`).toBe(true);
  });

  it.each(PAIRS)('check 1: the committed %s equals the fresh build', (committed, fresh) => {
    expect(read(committed), `the committed ${committed} is not the generator's output. ${REGENERATE}`).toBe(read(fresh));
  });

  it.each([
    ['committed', 'index.js', 'index.d.ts'],
    ['fresh', 'index.fresh.js', 'index.fresh.d.ts'],
  ] as const)('check 2: the %s pair is not vacuous', (label, js, dts) => {
    expect(vacuityProblems(read(js), read(dts)), `the ${label} glue is vacuous or inconsistent. ${REGENERATE}`).toEqual([]);
  });

  it.each(['build', 'test'])('check 3: the %s task writes only the scratch files', (task) => {
    const script = taskScript(readFileSync(MOON_YML, 'utf8'), task);
    expect(script, `moon.yml has no \`script:\` line for the ${task} task`).toBeDefined();
    expect(script, `the ${task} task must pass \`${SCRATCH_FLAGS}\` to napi build, or it overwrites the committed glue (SMA-667).`).toContain(SCRATCH_FLAGS);
  });
});
```

- [ ] **Step 2: Add the file to the vitest `node` project**

In `ts/packages/paigasus-kernel/vitest.config.ts`, in the `node` project `include` array, add one line after `'tests/committed-wasm.test.ts',`:

```ts
            'tests/committed-napi-glue.test.ts',
```

- [ ] **Step 3: Run the test and see it fail**

Run (from the worktree root):

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
pnpm -C ts/packages/paigasus-kernel exec vitest run --project node tests/committed-napi-glue.test.ts
```

Expected: the four helper rows PASS. Check 0 FAILS for both files (no fresh files yet). Check 1 and check 2 for the fresh pair FAIL (`ENOENT`). Check 3 FAILS for `build` and `test` (no flags yet). Check 2 for the committed pair PASSES.

- [ ] **Step 4: Change the `build` and `test` scripts**

In `ts/packages/paigasus-kernel/moon.yml`, in BOTH the `build` `script:` line and the `test` `script:` line, replace:

```
pnpm exec napi build --platform --cwd ../../../rs/crates/bindings/paigasus-node-bindings &&
```

with:

```
pnpm exec napi build --platform --cwd ../../../rs/crates/bindings/paigasus-node-bindings --js index.fresh.js --dts index.fresh.d.ts &&
```

Change nothing else on those two lines.

- [ ] **Step 5: Add the two `test` inputs**

In the `test` task `inputs`, directly after the five `paigasus-wasm/paigasus_wasm*` lines and their comment, add:

```yaml
      # SMA-667: the two committed napi glue files that tests/committed-napi-glue.test.ts compares
      # with this task's scratch build. Named one by one, not as an `index.*` glob, so the scratch
      # files `index.fresh.*` cannot re-key this task.
      - '/rs/crates/bindings/paigasus-node-bindings/index.js'
      - '/rs/crates/bindings/paigasus-node-bindings/index.d.ts'
```

- [ ] **Step 6: Correct the comments that become false**

In `moon.yml`:
- In the comment above `build` (about line 25), replace `the .node + glue land in the crate dir, not here.` with `the .node lands in the crate dir, not here. The glue goes to the scratch files index.fresh.js and index.fresh.d.ts beside it (SMA-667): the committed index.js and index.d.ts have two writers only, generate-napi-glue and version-lockstep --write.`
- In the `build` inputs comment (about lines 58-60), replace the two lines `# The generated glue (index.js / index.d.ts) is this task's OUTPUT, not an input — its content` / `# is fully determined by the Rust src + napi version (pnpm-lock.yaml), both already listed.` with:

```yaml
      # This task writes the glue to the scratch files index.fresh.js and index.fresh.d.ts only
      # (SMA-667). The committed index.js and index.d.ts are not its inputs or its outputs.
```

- In the comment above `test`, after the sentence that ends `…would import a STALE store .node (verified: a kernel edit otherwise passes against the old value).`, add:

```yaml
  # SMA-667: `napi build` writes the glue to the scratch files index.fresh.js and index.fresh.d.ts,
  # and tests/committed-napi-glue.test.ts compares them with the committed index.js and index.d.ts.
  # The vitest alias still loads the COMMITTED index.js, which loads the fresh .node.
```

- [ ] **Step 7: Ignore the scratch files and fix their line endings**

Append to `rs/crates/bindings/paigasus-node-bindings/.gitignore`:

```
# SMA-667: the napi build's scratch glue. paigasus-kernel-ts:build, :test and :generate-napi-glue
# write these. tests/committed-napi-glue.test.ts compares them with the committed index.js and
# index.d.ts.
index.fresh.js
index.fresh.d.ts
```

Append to `.gitattributes`:

```
#
# SMA-667. The two committed napi glue files. The drift gate (tests/committed-napi-glue.test.ts)
# compares WORKING-TREE bytes with a fresh build, so a CRLF checkout must not change them. A rebase
# or merge conflict in these files is NEVER resolved by hand: take either side, run
# `moon run paigasus-kernel-ts:generate-napi-glue`, and commit its result.
rs/crates/bindings/paigasus-node-bindings/index.js text eol=lf linguist-generated=true
rs/crates/bindings/paigasus-node-bindings/index.d.ts text eol=lf linguist-generated=true
```

- [ ] **Step 8: Run the task and see it pass**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-kernel-ts:test --force
git status --short
```

Expected: `paigasus-kernel-ts:test` passes, including every row of `committed-napi-glue.test.ts`. `git status --short` lists only the five files this task changed and the new test file. It does NOT list `index.js`, `index.d.ts` or `index.fresh.*`.

- [ ] **Step 9: Proofs P1, P2, P3 and P4**

Run each proof and copy the failing row name and its message into the task report.

- **P1:** Add the line `// p1` at the end of the committed `rs/crates/bindings/paigasus-node-bindings/index.d.ts` with an edit. Run `pnpm -C ts/packages/paigasus-kernel exec vitest run --project node tests/committed-napi-glue.test.ts`. Expected: `check 1: the committed index.d.ts equals the fresh build` FAILS and prints the remedy text. Remove the line with an edit. Re-run: PASS.
- **P2:** Add the line `// p2` at the end of the committed `index.js` with an edit. Run `pnpm -C ts/packages/paigasus-kernel exec vitest run --project node`. Expected: only `check 1: the committed index.js equals the fresh build` FAILS; every other node test passes. Remove the line with an edit. Re-run: PASS.
- **P3:** `rm rs/crates/bindings/paigasus-node-bindings/index.fresh.js rs/crates/bindings/paigasus-node-bindings/index.fresh.d.ts`, then run the single vitest file. Expected: both check 0 rows FAIL. Then run `moon run paigasus-kernel-ts:test --force`: PASS.
- **P4:** `moon run paigasus-kernel-ts:build --force && moon run paigasus-kernel-ts:test --force && git status --short`. Expected: both pass, and the status lists no `index.*` file.
- **P6a:** Remove ` --js index.fresh.js --dts index.fresh.d.ts` from the `build` script line with an edit. Run the single vitest file. Expected: `check 3: the build task writes only the scratch files` FAILS. Restore the flags with an edit. Re-run: PASS.
- **P6c:** Replace the content of `index.fresh.d.ts` with the one line `export declare const __napiBindingTarget: 'native' | 'wasm32-wasi' | 'wasm32-wasip1'`, then run the single vitest file. Expected: `check 2: the fresh pair is not vacuous` FAILS (and check 1 for `index.d.ts`). Run `moon run paigasus-kernel-ts:test --force` to rebuild the file: PASS.

- [ ] **Step 10: Run the lint, format and typecheck tasks**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run paigasus-kernel-ts:lint paigasus-kernel-ts:typecheck ts:fmt
```

Expected: all pass. If `ts:fmt` reports the new file, run `pnpm -C ts exec prettier --write packages/paigasus-kernel/tests/committed-napi-glue.test.ts`, then re-run. If moon says that a target does not exist, list the project's tasks with `moon project paigasus-kernel-ts`, and run the lint, typecheck and format tasks that it lists.

- [ ] **Step 11: Commit**

```bash
git branch --show-current   # must print feature/sma-667-napi-glue-drift
git add ts/packages/paigasus-kernel/tests/committed-napi-glue.test.ts ts/packages/paigasus-kernel/vitest.config.ts ts/packages/paigasus-kernel/moon.yml rs/crates/bindings/paigasus-node-bindings/.gitignore .gitattributes
git commit -m "feat(ts): hold the committed napi glue to a scratch build (SMA-667)

build and test now write index.fresh.js and index.fresh.d.ts, and a new
vitest file compares them with the committed glue.

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 2: The `generate-napi-glue` writer

**Files:**
- Modify: `ts/packages/paigasus-kernel/moon.yml` (new task after `generate-wasm`)
- Modify: `ci/affected-graph/cargo_moon_parity.py` (`ALLOW_UNLOCKED_CARGO`, about lines 418-439)

**Interfaces:**
- Consumes: the scratch flag string and the remedy text from Task 1.
- Produces: the moon target `paigasus-kernel-ts:generate-napi-glue`, which Task 4's docs name.

- [ ] **Step 1: Add the A8 entry first (the parity check must fail without the task, then pass with it)**

In `ci/affected-graph/cargo_moon_parity.py`, inside `ALLOW_UNLOCKED_CARGO`, after the `"paigasus-kernel-ts:generate-wasm": (…),` entry, add:

```python
    "paigasus-kernel-ts:generate-napi-glue": (
        "as paigasus-kernel-ts:build — it reaches cargo through `napi build`, which exposes no "
        "--locked and no cargo passthrough (SMA-601). SMA-667 made this task the sanctioned local "
        "writer of the committed napi glue; it is runInCI: false, so CI never runs it, but A8 "
        "reads the declaration."
    ),
```

- [ ] **Step 2: Run the parity check and record its verdict**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py; echo "rc=$?"
```

Expected: either rc 0, or rc 1 with a message about an allowlist entry for a task that does not exist. Record the output in the task report. (If it is rc 0, the checker does not flag stale entries; that is acceptable.)

- [ ] **Step 3: Add the task**

In `ts/packages/paigasus-kernel/moon.yml`, after the whole `generate-wasm` task (after its last `options` line), add:

```yaml
  # SMA-667. The sanctioned local writer of the two committed napi glue files
  # (rs/crates/bindings/paigasus-node-bindings/index.js and index.d.ts). `version-lockstep --write`
  # is the only other writer; it stamps the version guards in index.js during a release.
  #
  # Run it after a Rust change to the napi binding or the kernel, or after a `@napi-rs/cli` change
  # in ts/pnpm-lock.yaml. Then commit both files. tests/committed-napi-glue.test.ts reds until the
  # committed files agree with the generator. Run it as its OWN command, not in the same `moon run`
  # as `test`: the two tasks have no order, and `test` can hash its inputs before this task writes.
  #
  # It builds into the scratch files, like `build` and `test`, and then `cp`s them over the
  # committed files. `napi build` writes by rename, which replaces the inode and breaks the pnpm
  # hard link of the `file:` dependency @paigasus/node-bindings, so `tsc` would read a stale
  # index.d.ts through ts/node_modules. `cp` writes into the existing inode.
  #
  # The literal `napi build` stays HERE, not behind a wrapper: cargo_moon_parity.py finds an FFI
  # task by its resolved command (FFI_MARKERS). runInCI: false — CI never regenerates; it only
  # compares. cache: false — the task writes tracked files, and a cached replay writes nothing.
  generate-napi-glue:
    script: 'touch ../../../rs/crates/libs/paigasus-kernel/src/lib.rs ../../../rs/crates/bindings/paigasus-node-bindings/src/lib.rs ../../../rs/crates/bindings/paigasus-wasm/src/lib.rs && pnpm exec napi build --platform --cwd ../../../rs/crates/bindings/paigasus-node-bindings --js index.fresh.js --dts index.fresh.d.ts && cp ../../../rs/crates/bindings/paigasus-node-bindings/index.fresh.js ../../../rs/crates/bindings/paigasus-node-bindings/index.js && cp ../../../rs/crates/bindings/paigasus-node-bindings/index.fresh.d.ts ../../../rs/crates/bindings/paigasus-node-bindings/index.d.ts'
    deps: ['^:build']
    # A5 and A7 read the declaration, so the inputs are declared although the task is
    # `cache: false`: the napi crate's sources, manifests and build.rs, the kernel, the wasm crate
    # (the dependsOn closure) and the five workspace files.
    inputs:
      - 'package.json'
      - '/ts/pnpm-lock.yaml'
      - '/rs/crates/libs/paigasus-kernel/src/**/*'
      - '/rs/crates/libs/paigasus-kernel/Cargo.toml'
      - '/rs/crates/bindings/paigasus-node-bindings/build.rs'
      - '/rs/crates/bindings/paigasus-node-bindings/src/**/*'
      - '/rs/crates/bindings/paigasus-node-bindings/Cargo.toml'
      - '/rs/crates/bindings/paigasus-node-bindings/package.json'
      - '/rs/crates/bindings/paigasus-wasm/src/**/*'
      - '/rs/crates/bindings/paigasus-wasm/Cargo.toml'
      - '/rs/crates/bindings/paigasus-wasm/package.json'
      - '/rs/Cargo.lock'
      - '/rs/Cargo.toml'
      - '/rs/rust-toolchain.toml'
      - '/.prototools'
      - '/rs/.cargo/config.toml'
    options:
      runInCI: false
      cache: false
```

Before you save, read the `generate-wasm` task's `options:` block and copy any other key it has (for example `outputStyle`) only if its comment says the reason applies to every writer task. Do not add keys that are not in `generate-wasm`.

- [ ] **Step 4: Run the parity check and the affected-graph suite**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
python3 ci/affected-graph/cargo_moon_parity.py; echo "parity rc=$?"
/bin/bash ci/affected-graph/run.sh; echo "suite rc=$?"
```

Expected: parity rc 0; suite rc 0. If the suite reports an A5 or A7 violation for `generate-napi-glue`, add exactly the input it names, then re-run. Record each added input in the task report.

- [ ] **Step 5: Proof P5 (the writer repairs drift)**

1. In `rs/crates/bindings/paigasus-node-bindings/src/lib.rs`, rename the `sum` function's first parameter (for example `a` to `first`) with an edit.
2. `moon run paigasus-kernel-ts:test --force`. Expected: `check 1: the committed index.d.ts equals the fresh build` FAILS.
3. `moon run paigasus-kernel-ts:generate-napi-glue`. Then `git diff --stat`: expected `index.d.ts` changed.
4. `moon run paigasus-kernel-ts:test --force`. Expected: PASS.
5. Revert the parameter name with an edit. Run `moon run paigasus-kernel-ts:generate-napi-glue`. `git status --short` must not list `index.js`, `index.d.ts` or `src/lib.rs`.

- [ ] **Step 6: Proof P7 (the hard link survives)**

```bash
ls -li rs/crates/bindings/paigasus-node-bindings/index.d.ts
find ts/node_modules -path '*@paigasus/node-bindings/index.d.ts' -exec ls -liL {} \;
moon run paigasus-kernel-ts:generate-napi-glue
ls -li rs/crates/bindings/paigasus-node-bindings/index.d.ts
```

Expected: the inode number of the crate's `index.d.ts` is the same before and after the task. If the `find` shows the same inode as the crate file before the task, it must show the same inode after it. If the inodes already differed before the task, record that in the report (the link was broken before this task ran) and do not try to repair it.

- [ ] **Step 7: Commit**

```bash
git branch --show-current   # must print feature/sma-667-napi-glue-drift
git add ts/packages/paigasus-kernel/moon.yml ci/affected-graph/cargo_moon_parity.py
git commit -m "feat(ts): add generate-napi-glue as the local napi glue writer (SMA-667)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 3: Pin the new inputs in the affected graph

**Files:**
- Modify: `ci/affected-graph/run.sh` (after the `wasm-artifact->console` case and the case that follows it, about lines 729-760)

**Interfaces:**
- Consumes: the two `test` inputs from Task 1.
- Produces: two cases named `napi-glue-js->kernel-test` and `napi-glue-dts->kernel-test`.

- [ ] **Step 1: Measure the expected task sets**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
q="$(mktemp)"
for f in rs/crates/bindings/paigasus-node-bindings/index.js rs/crates/bindings/paigasus-node-bindings/index.d.ts; do
  echo "== $f"
  printf '%s\n' "$f" | moon query tasks --affected > "$q"
  python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(",".join(sorted(f"{p}:{t}" for p,ts in d["tasks"].items() for t in ts)))' "$q"
done
rm -f "$q"
```

If the sandbox refuses the loop, put it in a scratchpad script and run `/bin/bash <script>`. Read how `_assert_task_case_impl` in `run.sh` builds its set (one target per `tasks[project][task]`, never from `deps[]`), and confirm that the Python one-liner does the same. If the JSON shape differs, adapt the extraction to the shape `_assert_task_case_impl` reads. Expected: each set holds `paigasus-kernel-ts:test`. Record both sets.

- [ ] **Step 2: Add the two cases**

After the second wasm-artifact case (the `paigasus-kernel/src/wasm.ts` case), add (replace `<SET-JS>` and `<SET-DTS>` with the exact comma-separated sets from Step 1, in the order `_assert_task_case_impl` compares — copy the sorting rule from the existing cases):

```bash
  # SMA-667 — the two committed napi glue files. A PR that changes only one of them reaches the
  # drift gate (paigasus-kernel-ts:test, tests/committed-napi-glue.test.ts) through the `test`
  # task's two input lines alone; no `dependsOn` edge carries it. Without these cases, an edit that
  # dropped either input line would leave every other case green while a glue-only PR selected no
  # task that runs the gate. Expected sets MEASURED with the same no-flag
  # `moon query tasks --affected` traversal `_assert_task_case_impl` uses.
  run_task_case_ci "napi-glue-js->kernel-test" "rs/crates/bindings/paigasus-node-bindings/index.js" \
    "<SET-JS>"
  run_task_case_ci "napi-glue-dts->kernel-test" "rs/crates/bindings/paigasus-node-bindings/index.d.ts" \
    "<SET-DTS>"
```

- [ ] **Step 3: Run the suite and the negative control**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/bin/bash ci/affected-graph/run.sh --negative-control; echo "neg rc=$?"
/bin/bash ci/affected-graph/run.sh; echo "suite rc=$?"
```

Expected: both rc 0.

- [ ] **Step 4: Proof P6b (the case bites)**

Remove the line `- '/rs/crates/bindings/paigasus-node-bindings/index.js'` from the `test` inputs with an edit. Run `/bin/bash ci/affected-graph/run.sh`. Expected: rc 1 and a FAIL line for `napi-glue-js->kernel-test`. Restore the line with an edit. Re-run: rc 0. Record both outputs.

- [ ] **Step 5: Commit**

```bash
git branch --show-current   # must print feature/sma-667-napi-glue-drift
git add ci/affected-graph/run.sh
git commit -m "ci(repo): pin the napi glue inputs of paigasus-kernel-ts:test (SMA-667)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 4: The images.yml filter and the documentation

**Files:**
- Modify: `.github/workflows/images.yml` (about lines 53-60)
- Modify: `docs/ops/RUNBOOK-containers.md` (section 1, about lines 57-66)
- Modify: `rs/CLAUDE.md` (about lines 240-260)
- Modify: `ts/CLAUDE.md` (after the wasm artifact rule, about line 133)
- Modify: `ci/version-lockstep/README.md` (about lines 21-26)

**Interfaces:**
- Consumes: the task name `paigasus-kernel-ts:generate-napi-glue` (Task 2) and the test file name (Task 1).
- Produces: nothing code-level.

- [ ] **Step 1: images.yml**

Delete these lines from the `pull_request` `paths:` filter (the comment block that starts `# SMA-634 CR round 1: ts/Dockerfile COPYs these two files from the napi crate BY NAME.` and the two path lines after it):

```yaml
      # SMA-634 CR round 1: ts/Dockerfile COPYs these two files from the napi crate BY NAME. No
      # Moon task lists them as `inputs` — the kernel build task's own `napi build` step
      # regenerates them fresh every run, so a stale or renamed committed copy never reds the
      # ordinary build, only the Docker COPY. (The crate's `package.json` is NOT listed here: it
      # IS a `paigasus-kernel-ts:build`/`:test` input already, so the ordinary build already reds
      # on a bad edit to it.)
      - 'rs/crates/bindings/paigasus-node-bindings/index.js'
      - 'rs/crates/bindings/paigasus-node-bindings/index.d.ts'
```

Read the file first and delete the exact block; do not change the lines around it. If the same two paths are also in a `push` filter in the same file, remove them there too and say so in the report.

- [ ] **Step 2: RUNBOOK-containers.md section 1**

Replace the sentence that starts `` `rs/crates/bindings/paigasus-node-bindings/package.json` is an `input` of `` and ends `` `ts/Dockerfile` copies both by name, so they are on the filter. `` with:

```markdown
`rs/crates/bindings/paigasus-node-bindings/package.json` and its two siblings, `index.js` and
`index.d.ts`, are `inputs` of `paigasus-kernel-ts:test` for the same reason, and stay off the
filter. Since SMA-667 the build tasks do not write the two glue files, and
`tests/committed-napi-glue.test.ts` reds the ordinary `moon ci` when the committed copy is not the
generator's output.
```

- [ ] **Step 3: rs/CLAUDE.md**

The filter-list paragraph has this text:

```markdown
  `ts/packages/paigasus-kernel/package.json`,
  `rs/crates/bindings/paigasus-node-bindings/index.js` and
  `rs/crates/bindings/paigasus-node-bindings/index.d.ts`. It also lists four smoke-runtime files
```

Replace it with (keep `ts/packages/paigasus-kernel/package.json`; it becomes the last `ts/` entry, after an `and`):

```markdown
  and `ts/packages/paigasus-kernel/package.json`. It also lists four smoke-runtime files
```

Read the line before it, and remove the `and` there if one is already present, so the list has one `and`. Then replace the tail of the paragraph, from `` this is why the `` to the end of the paragraph, with:

```markdown
this is why the five committed wasm artifacts, `paigasus-wasm/package.json`,
`paigasus-node-bindings/package.json` and the napi crate's `index.js`/`index.d.ts` are absent.
The two napi glue files are `paigasus-kernel-ts:test` inputs since SMA-667, and
`tests/committed-napi-glue.test.ts` holds them to the generator.
```

- [ ] **Step 4: ts/CLAUDE.md**

After the sentence that ends `` A conflict in the five files is resolved by taking either side and running `generate-wasm` again. ``, add:

```markdown
  The napi glue `rs/crates/bindings/paigasus-node-bindings/index.js` and `index.d.ts` is committed
  too, and it is what npm publishes and `ts/Dockerfile` copies (SMA-667). `paigasus-kernel-ts:build`
  and `:test` write only the scratch files `index.fresh.js` and `index.fresh.d.ts`.
  `moon run paigasus-kernel-ts:generate-napi-glue` is the local writer, and `version-lockstep
  --write` stamps the version guards in a release. Run `generate-napi-glue` after a napi binding
  or kernel export change, or after a `@napi-rs/cli` change in `ts/pnpm-lock.yaml`, and commit both
  files. Run it as its own command, not in the same `moon run` as `test`.
  `tests/committed-napi-glue.test.ts` reds until the committed files equal the scratch build.
  `@napi-rs/cli` has its own Dependabot group: on that PR, run `generate-napi-glue` and push the
  result to the PR branch. A Dependabot rebase or recreate deletes that commit, so push it again.
  Two changes would make the glue depend on the host and red this check in CI: a WASI target in
  `napi.targets`, and a `#[cfg]`-gated `#[napi]` item.
```

- [ ] **Step 5: ci/version-lockstep/README.md**

Replace:

```markdown
`ci.yml`'s codegen-drift gate covers only the three `**/generated` proto dirs — so the
`uv.lock` and napi-glue classes still drift **silently** today.
```

with:

```markdown
`ci.yml`'s codegen-drift gate covers only the three `**/generated` proto dirs — so the
`uv.lock` class still drifts **silently** today. The napi glue does not since SMA-667:
`paigasus-kernel-ts:test` (`tests/committed-napi-glue.test.ts`) fails when the committed
`index.js`, version guards included, is not the generator's output.
```

Do not change lines 105 and 145 (L1/L2).

- [ ] **Step 6: Run actionlint**

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
/opt/homebrew/bin/bash ci/actionlint/run.sh; echo "rc=$?"
```

Expected: rc 0. If it exits rc 2 with a message that the pipe holds only 512 bytes, the host is in the small-pipe state: record that, and run `actionlint .github/workflows/images.yml` directly instead (expected: no output, rc 0). Do not read a hang or a pipe message as a failure of this change.

- [ ] **Step 7: Commit**

```bash
git branch --show-current   # must print feature/sma-667-napi-glue-drift
git add .github/workflows/images.yml docs/ops/RUNBOOK-containers.md rs/CLAUDE.md ts/CLAUDE.md ci/version-lockstep/README.md
git commit -m "docs(repo): describe the napi glue writers and drop it from the images filter (SMA-667)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 5: A separate Dependabot group for `@napi-rs/cli`

**Files:**
- Modify: `.github/dependabot.yml` (the npm `groups:` block, about lines 49-54)

**Interfaces:**
- Consumes: nothing.
- Produces: the group name `napi-cli`, which Task 4's `ts/CLAUDE.md` text describes.

- [ ] **Step 1: Edit the groups**

Replace the npm ecosystem's `groups:` block:

```yaml
    groups:
      npm-minor-patch:
        applies-to: version-updates
        update-types:
          - minor
          - patch
```

with:

```yaml
    groups:
      # SMA-667: a `@napi-rs/cli` bump can change the committed napi glue, and then
      # paigasus-kernel-ts:test reds until someone runs generate-napi-glue and pushes the result.
      # Its own group keeps that work off the shared npm-minor-patch PR, so a glue change does not
      # block the other JS updates.
      napi-cli:
        applies-to: version-updates
        patterns:
          - "@napi-rs/cli"
        update-types:
          - minor
          - patch
      npm-minor-patch:
        applies-to: version-updates
        exclude-patterns:
          - "@napi-rs/cli"
        update-types:
          - minor
          - patch
```

- [ ] **Step 2: Validate the file**

```bash
python3 -c 'import yaml,sys; d=yaml.safe_load(open(".github/dependabot.yml")); g=[u for u in d["updates"] if u["package-ecosystem"]=="npm"][0]["groups"]; print(sorted(g)); assert g["napi-cli"]["patterns"]==["@napi-rs/cli"]; assert g["npm-minor-patch"]["exclude-patterns"]==["@napi-rs/cli"]'
```

Expected: `['napi-cli', 'npm-minor-patch']` and no assertion error. If `yaml` is not importable, run the same with `uv run --with pyyaml python3 -c '…'`.

Then run the workflow-credentials gate, which reads `dependabot.yml`:

```bash
export PATH="$HOME/.proto/shims:$HOME/.proto/bin:$PATH"
moon run repo:workflow-credentials
```

Expected: pass.

- [ ] **Step 3: Commit**

```bash
git branch --show-current   # must print feature/sma-667-napi-glue-drift
git add .github/dependabot.yml
git commit -m "build(repo): give @napi-rs/cli its own dependabot group (SMA-667)

Co-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>"
```

---

### Task 6: Full verification (the coordinator runs this)

**Files:** none changed, unless a gate fails.

- [ ] **Step 1: R1 evidence.** Find the `@napi-rs/cli` version that `ts/pnpm-lock.yaml` resolved at commits `bd4e90e6` and `27df30c6`:

```bash
git show bd4e90e6:ts/pnpm-lock.yaml | grep -A1 "'@napi-rs/cli':" | head -4
git show 27df30c6:ts/pnpm-lock.yaml | grep -A1 "'@napi-rs/cli':" | head -4
```

Record both. If they are equal, the `#334` Linux stamp is a Linux-versus-macOS measurement of identical glue.

- [ ] **Step 2: The full gate graph.** Run the `moon ci` command from the root `CLAUDE.md` `ci-targets` block, with the bash rules of the root `CLAUDE.md` ("No single local bash runs every gate"). Re-run the bash-4+ gates directly with `/opt/homebrew/bin/bash ci/<gate>/run.sh` and read those results instead. Before any re-run of a failure, capture `.moon/cache/ciReport.json` and the task's `.moon/cache/states/<project>/<task>/` folder (Step 0 of the diagnosis procedure). <!-- moon-diagnosis:ok -->

- [ ] **Step 3: Tree check.** `git status --short` is empty after the full run.

- [ ] **Step 4: Proof record.** Collect P1–P7 results from the task reports into a table for the PR body.
